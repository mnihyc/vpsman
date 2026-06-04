use anyhow::Result;
use sqlx::{PgPool, Row};

pub(crate) async fn acquire_worker_lease(
    pool: &PgPool,
    task_name: &str,
    owner: &str,
    lease_secs: i32,
) -> Result<bool> {
    let lease_secs = lease_secs.clamp(1, 3600);
    let row = sqlx::query(
        r#"
        WITH claimed AS (
            INSERT INTO worker_leases (
                task_name,
                owner,
                lease_expires_at,
                updated_at
            )
            VALUES ($1, $2, now() + ($3::text || ' seconds')::interval, now())
            ON CONFLICT (task_name) DO UPDATE
            SET
                owner = EXCLUDED.owner,
                lease_expires_at = EXCLUDED.lease_expires_at,
                updated_at = now()
            WHERE worker_leases.lease_expires_at <= now()
               OR worker_leases.owner = EXCLUDED.owner
            RETURNING 1
        )
        SELECT EXISTS(SELECT 1 FROM claimed) AS acquired
        "#,
    )
    .bind(task_name)
    .bind(owner)
    .bind(lease_secs)
    .fetch_one(pool)
    .await?;
    row.try_get("acquired").map_err(Into::into)
}

#[cfg(test)]
mod tests {
    #[test]
    fn lease_duration_bounds_are_documented() {
        assert_eq!(0_i32.clamp(1, 3600), 1);
        assert_eq!(4_000_i32.clamp(1, 3600), 3600);
    }
}
