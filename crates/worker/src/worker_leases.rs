use anyhow::Result;
use sqlx::{PgPool, Postgres, Row, Transaction};

const WORKER_ADVISORY_LOCK_QUERY: &str = r#"
    SELECT pg_try_advisory_xact_lock(
        hashtextextended('vpsman.worker.' || $1, 0::bigint)
    )
"#;

pub(crate) struct WorkerLease {
    task_name: String,
    transaction: Option<Transaction<'static, Postgres>>,
}

impl WorkerLease {
    pub(crate) async fn finish(mut self) -> Result<()> {
        if let Some(mut transaction) = self.transaction.take() {
            sqlx::query(
                r#"
                UPDATE worker_leases
                SET lease_expires_at = now(), updated_at = now()
                WHERE task_name = $1
                "#,
            )
            .bind(&self.task_name)
            .execute(&mut *transaction)
            .await?;
            transaction.commit().await?;
        }
        Ok(())
    }
}

pub(crate) async fn acquire_worker_lease(
    pool: &PgPool,
    task_name: &str,
    owner: &str,
    lease_secs: i32,
) -> Result<Option<WorkerLease>> {
    let lease_secs = lease_secs.clamp(1, 3600);
    let mut transaction = pool.begin().await?;
    let advisory_lock_acquired: bool = sqlx::query_scalar(WORKER_ADVISORY_LOCK_QUERY)
        .bind(task_name)
        .fetch_one(&mut *transaction)
        .await?;
    if !advisory_lock_acquired {
        transaction.rollback().await?;
        return Ok(None);
    }

    // The transaction-scoped advisory lock is authoritative and remains held
    // for the task's full lifetime. This row is retained for operator
    // observability and also serializes old TTL-only workers during an upgrade.
    let row = sqlx::query(
        r#"
        INSERT INTO worker_leases (
            task_name,
            owner,
            lease_expires_at,
            updated_at
        )
        VALUES ($1, $2, now() + make_interval(secs => $3::double precision), now())
        ON CONFLICT (task_name) DO UPDATE
        SET
            owner = EXCLUDED.owner,
            lease_expires_at = EXCLUDED.lease_expires_at,
            updated_at = now()
        WHERE worker_leases.lease_expires_at <= now()
        RETURNING task_name
        "#,
    )
    .bind(task_name)
    .bind(owner)
    .bind(lease_secs)
    .fetch_optional(&mut *transaction)
    .await?;
    let Some(row) = row else {
        transaction.rollback().await?;
        return Ok(None);
    };
    let persisted_task_name: String = row.try_get("task_name")?;
    anyhow::ensure!(
        persisted_task_name == task_name,
        "worker lease observability row mismatch"
    );
    Ok(Some(WorkerLease {
        task_name: task_name.to_string(),
        transaction: Some(transaction),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_duration_bounds_are_documented() {
        assert_eq!(0_i32.clamp(1, 3600), 1);
        assert_eq!(4_000_i32.clamp(1, 3600), 3600);
    }

    #[test]
    fn worker_lease_uses_a_task_scoped_transaction_advisory_lock() {
        assert!(WORKER_ADVISORY_LOCK_QUERY.contains("pg_try_advisory_xact_lock"));
        assert!(WORKER_ADVISORY_LOCK_QUERY.contains("vpsman.worker."));
    }
}
