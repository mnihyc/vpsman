use anyhow::Result;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::sync::OnceLock;
use tokio::{
    sync::{oneshot, Notify},
    task::JoinHandle,
    time,
};
use uuid::Uuid;

// These values fence ownership; they do not cap throughput. Renewing at one
// third of the lease leaves two missed renewal opportunities before takeover,
// while a crashed owner becomes recoverable within 30 seconds.
pub(crate) const ARTIFACT_DELETION_LEASE_SECS: i32 = 30;
const ARTIFACT_DELETION_RENEW_SECS: u64 = 10;
pub(crate) const ARTIFACT_DELETION_RETRY_SECS: i32 = 30;
pub(crate) const ARTIFACT_DELETION_COMPLETED_CHANNEL: &str = "vpsman_artifact_deletion_completed";

static ARTIFACT_DELETION_READY: OnceLock<Notify> = OnceLock::new();
static ARTIFACT_DELETION_COMPLETED: OnceLock<Notify> = OnceLock::new();

fn artifact_deletion_ready() -> &'static Notify {
    ARTIFACT_DELETION_READY.get_or_init(Notify::new)
}

pub(crate) fn artifact_deletion_completion_signal() -> &'static Notify {
    ARTIFACT_DELETION_COMPLETED.get_or_init(Notify::new)
}

pub(crate) fn wake_artifact_deletion_consumer() {
    artifact_deletion_ready().notify_one();
}

pub(crate) async fn wait_for_artifact_deletion_work() {
    artifact_deletion_ready().notified().await;
}

pub(crate) fn publish_artifact_deletion_completion() {
    artifact_deletion_completion_signal().notify_waiters();
}

pub(crate) async fn publish_artifact_deletion_completion_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    source_id: Uuid,
) -> Result<()> {
    // PostgreSQL delivers NOTIFY only after this transaction commits. The
    // payload is diagnostic; the waiting producer always rereads its exact
    // durable targets and never treats a notification as completion state.
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(ARTIFACT_DELETION_COMPLETED_CHANNEL)
        .bind(source_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactDeletionReview {
    pub(crate) artifact_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) source_kind: &'static str,
    pub(crate) source_id: Uuid,
    pub(crate) source_revision: i64,
    pub(crate) source_identity: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactDeletionOwner {
    pub(crate) artifact_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) source_kind: String,
    pub(crate) source_id: Uuid,
    pub(crate) source_revision: i64,
    pub(crate) source_identity: Value,
    pub(crate) lease_id: Uuid,
}

pub(crate) async fn enqueue_artifact_deletion(
    pool: &PgPool,
    review: &ArtifactDeletionReview,
) -> Result<bool> {
    let inserted = sqlx::query(
        r#"
        INSERT INTO server_artifact_deletion_intents (
            artifact_id,
            object_key,
            sha256_hex,
            size_bytes,
            source_kind,
            source_id,
            source_revision,
            source_identity
        )
        SELECT
            artifact.id,
            artifact.object_key,
            artifact.sha256_hex,
            artifact.size_bytes,
            $5,
            $6,
            $7,
            $8
        FROM server_artifacts artifact
        WHERE artifact.id = $1
          AND artifact.object_key = $2
          AND artifact.sha256_hex = $3
          AND artifact.size_bytes = $4
          AND artifact.status IN ('active', 'delete_failed')
        ON CONFLICT (artifact_id) DO NOTHING
        "#,
    )
    .bind(review.artifact_id)
    .bind(&review.object_key)
    .bind(&review.sha256_hex)
    .bind(review.size_bytes)
    .bind(review.source_kind)
    .bind(review.source_id)
    .bind(review.source_revision)
    .bind(&review.source_identity)
    .execute(pool)
    .await?;
    let inserted = inserted.rows_affected() == 1;
    if inserted {
        wake_artifact_deletion_consumer();
    }
    Ok(inserted)
}

pub(crate) async fn claim_artifact_deletion(
    pool: &PgPool,
    source_kind: Option<&str>,
    source_id: Option<Uuid>,
    artifact_id: Option<Uuid>,
) -> Result<Option<ArtifactDeletionOwner>> {
    let lease_id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT intent.artifact_id
            FROM server_artifact_deletion_intents intent
            WHERE ($1::text IS NULL OR intent.source_kind = $1)
              AND ($2::uuid IS NULL OR intent.source_id = $2)
              AND ($3::uuid IS NULL OR intent.artifact_id = $3)
              AND intent.next_attempt_at <= now()
              AND (intent.lease_until IS NULL OR intent.lease_until <= now())
            ORDER BY intent.next_attempt_at ASC, intent.created_at ASC, intent.artifact_id ASC
            LIMIT 1
            FOR UPDATE SKIP LOCKED
        ),
        claimed AS (
            UPDATE server_artifact_deletion_intents intent
            SET lease_id = $4,
                lease_until = now() + ($5::int * interval '1 second'),
                attempt_count = intent.attempt_count + 1,
                updated_at = now()
            FROM candidate
            WHERE intent.artifact_id = candidate.artifact_id
            RETURNING intent.*
        ),
        marked AS (
            UPDATE server_artifacts artifact
            SET status = 'deleting',
                metadata = metadata - 'delete_error' - 'delete_failed_at'
            FROM claimed
            WHERE artifact.id = claimed.artifact_id
              AND artifact.object_key = claimed.object_key
              AND artifact.sha256_hex = claimed.sha256_hex
              AND artifact.size_bytes = claimed.size_bytes
              AND artifact.status IN ('active', 'deleting', 'delete_failed')
            RETURNING artifact.id AS artifact_id
        )
        SELECT
            claimed.artifact_id,
            claimed.object_key,
            claimed.source_kind,
            claimed.source_id,
            claimed.source_revision,
            claimed.source_identity,
            claimed.lease_id
        FROM claimed
        JOIN marked ON marked.artifact_id = claimed.artifact_id
        "#,
    )
    .bind(source_kind)
    .bind(source_id)
    .bind(artifact_id)
    .bind(lease_id)
    .bind(ARTIFACT_DELETION_LEASE_SECS)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        Ok(ArtifactDeletionOwner {
            artifact_id: row.try_get("artifact_id")?,
            object_key: row.try_get("object_key")?,
            source_kind: row.try_get("source_kind")?,
            source_id: row.try_get("source_id")?,
            source_revision: row.try_get("source_revision")?,
            source_identity: row.try_get("source_identity")?,
            lease_id: row.try_get("lease_id")?,
        })
    })
    .transpose()
}

pub(crate) async fn renew_artifact_deletion(
    pool: &PgPool,
    owner: &ArtifactDeletionOwner,
) -> Result<bool> {
    let renewed = sqlx::query(
        r#"
        UPDATE server_artifact_deletion_intents
        SET lease_until = now() + ($3::int * interval '1 second'),
            updated_at = now()
        WHERE artifact_id = $1
          AND lease_id = $2
          AND lease_until > now()
        "#,
    )
    .bind(owner.artifact_id)
    .bind(owner.lease_id)
    .bind(ARTIFACT_DELETION_LEASE_SECS)
    .execute(pool)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

pub(crate) fn spawn_artifact_deletion_heartbeat(
    pool: PgPool,
    owner: ArtifactDeletionOwner,
) -> (oneshot::Sender<()>, JoinHandle<Result<bool>>) {
    let (stop_tx, mut stop_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut ticker = time::interval(time::Duration::from_secs(ARTIFACT_DELETION_RENEW_SECS));
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = &mut stop_rx => return Ok(true),
                _ = ticker.tick() => {
                    if !renew_artifact_deletion(&pool, &owner).await? {
                        return Ok(false);
                    }
                }
            }
        }
    });
    (stop_tx, task)
}

pub(crate) async fn defer_artifact_deletion(
    pool: &PgPool,
    owner: &ArtifactDeletionOwner,
    error: &str,
) -> Result<bool> {
    let deferred = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH deferred AS (
            UPDATE server_artifact_deletion_intents intent
            SET lease_id = NULL,
                lease_until = NULL,
                next_attempt_at = now() + ($4::int * interval '1 second'),
                last_error = left($3, 1000),
                updated_at = now()
            WHERE intent.artifact_id = $1
              AND intent.lease_id = $2
            RETURNING intent.artifact_id
        ),
        failed AS (
            UPDATE server_artifacts artifact
            SET status = 'delete_failed',
                metadata = metadata || jsonb_build_object(
                    'delete_error', left($3, 1000),
                    'delete_failed_at', now()::text
                )
            FROM deferred
            WHERE artifact.id = deferred.artifact_id
            RETURNING artifact.id AS artifact_id
        )
        SELECT artifact_id FROM failed
        "#,
    )
    .bind(owner.artifact_id)
    .bind(owner.lease_id)
    .bind(error)
    .bind(ARTIFACT_DELETION_RETRY_SECS)
    .fetch_optional(pool)
    .await?;
    Ok(deferred.is_some())
}

pub(crate) async fn fail_artifact_deletion(
    pool: &PgPool,
    owner: &ArtifactDeletionOwner,
    error: &str,
) -> Result<bool> {
    let failed = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH released AS (
            DELETE FROM server_artifact_deletion_intents intent
            WHERE intent.artifact_id = $1
              AND intent.lease_id = $2
            RETURNING intent.artifact_id
        ),
        failed AS (
            UPDATE server_artifacts artifact
            SET status = 'delete_failed',
                metadata = metadata || jsonb_build_object(
                    'delete_error', left($3, 1000),
                    'delete_failed_at', now()::text
                )
            FROM released
            WHERE artifact.id = released.artifact_id
            RETURNING artifact.id AS artifact_id
        )
        SELECT artifact_id FROM failed
        "#,
    )
    .bind(owner.artifact_id)
    .bind(owner.lease_id)
    .bind(error)
    .fetch_optional(pool)
    .await?;
    Ok(failed.is_some())
}

pub(crate) async fn lock_owned_artifact_deletion_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &ArtifactDeletionOwner,
) -> Result<bool> {
    let owned = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM server_artifact_deletion_intents
            WHERE artifact_id = $1
              AND lease_id = $2
              AND lease_until > now()
            FOR UPDATE
        )
        "#,
    )
    .bind(owner.artifact_id)
    .bind(owner.lease_id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(owned)
}

pub(crate) async fn finish_artifact_deletion_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &ArtifactDeletionOwner,
) -> Result<bool> {
    let deleted = sqlx::query(
        r#"
        DELETE FROM server_artifact_deletion_intents
        WHERE artifact_id = $1
          AND lease_id = $2
        "#,
    )
    .bind(owner.artifact_id)
    .bind(owner.lease_id)
    .execute(&mut **tx)
    .await?;
    Ok(deleted.rows_affected() == 1)
}

#[cfg(test)]
#[path = "tests_artifact_deletion.rs"]
mod tests;
