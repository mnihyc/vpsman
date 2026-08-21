use anyhow::{ensure, Result};
use sqlx::{pool::PoolConnection, PgConnection, PgPool, Postgres};

const CLIENT_POLICY_SUPPRESSION_LOCK_PREFIX: &str = "vpsman.client_policy_suppression:";

pub fn client_policy_suppression_lock_key(client_id: &str) -> String {
    format!("{CLIENT_POLICY_SUPPRESSION_LOCK_PREFIX}{client_id}")
}

/// A cross-process read-side fence for client-scoped external side effects.
///
/// Suspension takes the matching transaction-level exclusive advisory lock.
/// This guard deliberately uses a session-level shared lock so it can remain
/// held while an external request is in flight without keeping a PostgreSQL
/// transaction or MVCC snapshot open. Callers must recheck the side effect's
/// eligibility after acquiring the guard and hold it until both the external
/// request and its durable outcome update have completed.
pub struct ClientPolicySuppressionSharedGuard {
    connection: Option<PoolConnection<Postgres>>,
    lock_keys: Vec<String>,
    lock_may_be_held: bool,
}

impl ClientPolicySuppressionSharedGuard {
    pub async fn acquire(pool: &PgPool, client_id: &str) -> Result<Self> {
        Self::acquire_many(pool, std::iter::once(client_id)).await
    }

    pub async fn acquire_many<'a, I>(pool: &PgPool, client_ids: I) -> Result<Self>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut lock_keys = client_ids
            .into_iter()
            .map(client_policy_suppression_lock_key)
            .collect::<Vec<_>>();
        lock_keys.sort();
        lock_keys.dedup();
        ensure!(
            !lock_keys.is_empty(),
            "client policy suppression guard requires a client"
        );
        let mut guard = Self {
            connection: Some(pool.acquire().await?),
            lock_keys,
            // Set this before awaiting PostgreSQL. If the future is canceled
            // after the server grants the lock but before SQLx returns, Drop
            // must discard the session rather than return it to the pool.
            lock_may_be_held: true,
        };
        for lock_key in guard.lock_keys.clone() {
            sqlx::query("SELECT pg_advisory_lock_shared(hashtextextended($1, 0))")
                .bind(lock_key)
                .execute(guard.connection())
                .await?;
        }
        Ok(guard)
    }

    pub fn connection(&mut self) -> &mut PgConnection {
        &mut *self
            .connection
            .as_mut()
            .expect("client policy suppression guard connection missing")
    }

    pub async fn release(mut self) -> Result<()> {
        for lock_key in self.lock_keys.clone().into_iter().rev() {
            let unlocked = sqlx::query_scalar::<_, bool>(
                "SELECT pg_advisory_unlock_shared(hashtextextended($1, 0))",
            )
            .bind(lock_key)
            .fetch_one(self.connection())
            .await?;
            ensure!(
                unlocked,
                "client policy suppression shared lock was not held"
            );
        }
        self.lock_may_be_held = false;
        Ok(())
    }
}

impl Drop for ClientPolicySuppressionSharedGuard {
    fn drop(&mut self) {
        if self.lock_may_be_held {
            // A session-level advisory lock survives ordinary pool check-in.
            // Discarding the connection is the cancellation/panic-safe release
            // path; PostgreSQL drops every session lock when the backend exits.
            if let Some(connection) = self.connection.as_mut() {
                connection.close_on_drop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_policy_suppression_key_is_namespaced() {
        assert_eq!(
            client_policy_suppression_lock_key("edge-a"),
            "vpsman.client_policy_suppression:edge-a"
        );
    }
}
