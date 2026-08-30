use anyhow::{ensure, Context, Result};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time,
};
use uuid::Uuid;

use crate::{
    model::BackupPolicyView, object_store::BackupObjectStore, repository::Repository,
    repository_backup_policies::BackupPolicyPruneCandidate,
    repository_history::HistoryRetentionObjectCandidate,
};

// These values fence ownership; they do not cap throughput. Renewal at one
// third of the lease leaves two missed renewal opportunities before takeover,
// while a crashed consumer becomes recoverable within 30 seconds.
const ARTIFACT_DELETION_LEASE_SECS: i32 = 30;
const ARTIFACT_DELETION_RENEW_SECS: u64 = 10;
const ARTIFACT_DELETION_RETRY_SECS: i32 = 30;

#[derive(Clone, Debug)]
struct ArtifactDeletionReview {
    pub(crate) domain: &'static str,
    pub(crate) object_key: String,
    pub(crate) source_kind: &'static str,
    pub(crate) source_id: Uuid,
    pub(crate) source_revision: i64,
    pub(crate) source_identity: Value,
}

#[derive(Clone, Debug)]
pub(crate) struct ArtifactDeletionOwner {
    pub(crate) artifact_id: Uuid,
    pub(crate) object_key: String,
    pub(crate) sha256_hex: String,
    pub(crate) size_bytes: i64,
    pub(crate) source_kind: String,
    pub(crate) source_id: Uuid,
    pub(crate) source_revision: i64,
    pub(crate) source_identity: Value,
    pub(crate) lease_id: Uuid,
    pool: PgPool,
}

pub(crate) enum ReviewedArtifactDeletionOutcome {
    NotClaimed,
    Deleted(i64),
    DeleteFailed(anyhow::Error),
}

enum ReviewedArtifactDeletionSource {
    BackupPolicy {
        policy: Box<BackupPolicyView>,
        candidate: BackupPolicyPruneCandidate,
    },
    HistoryRetention {
        candidate: HistoryRetentionObjectCandidate,
    },
}

struct ReviewedArtifactDeletionCommand {
    source: ReviewedArtifactDeletionSource,
    completed: oneshot::Sender<Result<ReviewedArtifactDeletionOutcome>>,
}

#[derive(Clone)]
pub(crate) struct ReviewedArtifactDeletionProducer {
    commands: mpsc::UnboundedSender<ReviewedArtifactDeletionCommand>,
}

pub(crate) struct ReviewedArtifactDeletionInbox {
    commands: mpsc::UnboundedReceiver<ReviewedArtifactDeletionCommand>,
}

pub(crate) fn reviewed_artifact_deletion_channel() -> (
    ReviewedArtifactDeletionProducer,
    ReviewedArtifactDeletionInbox,
) {
    let (commands, inbox) = mpsc::unbounded_channel();
    (
        ReviewedArtifactDeletionProducer { commands },
        ReviewedArtifactDeletionInbox { commands: inbox },
    )
}

impl ReviewedArtifactDeletionProducer {
    pub(crate) async fn delete_backup_policy_candidate(
        &self,
        policy: BackupPolicyView,
        candidate: BackupPolicyPruneCandidate,
    ) -> Result<ReviewedArtifactDeletionOutcome> {
        self.submit(ReviewedArtifactDeletionSource::BackupPolicy {
            policy: Box::new(policy),
            candidate,
        })
        .await
    }

    pub(crate) async fn delete_history_retention_candidate(
        &self,
        candidate: HistoryRetentionObjectCandidate,
    ) -> Result<ReviewedArtifactDeletionOutcome> {
        self.submit(ReviewedArtifactDeletionSource::HistoryRetention { candidate })
            .await
    }

    async fn submit(
        &self,
        source: ReviewedArtifactDeletionSource,
    ) -> Result<ReviewedArtifactDeletionOutcome> {
        let (completed, result) = oneshot::channel();
        self.commands
            .send(ReviewedArtifactDeletionCommand { source, completed })
            .map_err(|_| anyhow::anyhow!("reviewed artifact deletion consumer stopped"))?;
        result
            .await
            .context("reviewed artifact deletion consumer stopped before completing work")?
    }
}

/// Owns reviewed API artifact deletion from durable intent creation through
/// exact source finalization. Request handlers only submit commands and await
/// their correlated result; no route claims work or performs object-store I/O.
pub(crate) fn spawn_reviewed_artifact_deletion_consumer(
    repo: Repository,
    store: BackupObjectStore,
    mut inbox: ReviewedArtifactDeletionInbox,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(command) = inbox.commands.recv().await {
            let result = process_reviewed_artifact_deletion(&repo, &store, command.source).await;
            // The durable intent remains authoritative when the submitting
            // request is canceled, so completion never depends on a receiver.
            let _ = command.completed.send(result);
        }
    })
}

async fn process_reviewed_artifact_deletion(
    repo: &Repository,
    store: &BackupObjectStore,
    source: ReviewedArtifactDeletionSource,
) -> Result<ReviewedArtifactDeletionOutcome> {
    let review = match &source {
        ReviewedArtifactDeletionSource::BackupPolicy { policy, candidate } => {
            ArtifactDeletionReview {
                domain: "backup_artifact",
                object_key: candidate.object_key.clone(),
                source_kind: "backup_policy",
                source_id: policy.schedule_id,
                source_revision: policy.definition_revision,
                source_identity: candidate.deletion_identity(),
            }
        }
        ReviewedArtifactDeletionSource::HistoryRetention { candidate } => {
            let Some(object_key) = candidate.object_key() else {
                anyhow::bail!("history-retention object deletion requires an object key");
            };
            ArtifactDeletionReview {
                domain: "job_output",
                object_key: object_key.to_string(),
                source_kind: "history_retention",
                source_id: candidate.source_id(),
                source_revision: candidate.source_revision(),
                source_identity: candidate.deletion_identity(),
            }
        }
    };
    let Some(owner) = repo.claim_reviewed_artifact_deletion(&review).await? else {
        return Ok(ReviewedArtifactDeletionOutcome::NotClaimed);
    };
    if let Err(error) = delete_confirmed_owned_artifact(store, &owner).await {
        return Ok(ReviewedArtifactDeletionOutcome::DeleteFailed(error));
    }
    let pruned_rows = match source {
        ReviewedArtifactDeletionSource::BackupPolicy { policy, candidate } => {
            repo.finalize_backup_policy_candidate_object_delete(&policy, &candidate, &owner)
                .await?
        }
        ReviewedArtifactDeletionSource::HistoryRetention { candidate } => {
            repo.finalize_history_retention_object_delete(&candidate, &owner)
                .await?
        }
    };
    Ok(ReviewedArtifactDeletionOutcome::Deleted(pruned_rows))
}

impl Repository {
    async fn claim_reviewed_artifact_deletion(
        &self,
        review: &ArtifactDeletionReview,
    ) -> Result<Option<ArtifactDeletionOwner>> {
        ensure!(
            review.source_revision >= 1,
            "artifact deletion source revision invalid"
        );
        match self {
            Self::Postgres(pool) => claim_reviewed_artifact_deletion(pool, review).await,
        }
    }
}

async fn claim_reviewed_artifact_deletion(
    pool: &PgPool,
    review: &ArtifactDeletionReview,
) -> Result<Option<ArtifactDeletionOwner>> {
    // Intent creation and exact leasing share one transaction. Other durable
    // consumers therefore observe either no intent or an already leased one,
    // never an unowned API intent between producer phases.
    let mut tx = pool.begin().await?;
    sqlx::query(
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
            $3,
            $4,
            $5,
            $6
        FROM server_artifacts artifact
        WHERE artifact.domain = $1
          AND artifact.object_key = $2
          AND artifact.status IN ('active', 'delete_failed')
        ON CONFLICT (artifact_id) DO NOTHING
        "#,
    )
    .bind(review.domain)
    .bind(&review.object_key)
    .bind(review.source_kind)
    .bind(review.source_id)
    .bind(review.source_revision)
    .bind(&review.source_identity)
    .execute(&mut *tx)
    .await?;

    let lease_id = Uuid::new_v4();
    let row = sqlx::query(
        r#"
        WITH candidate AS (
            SELECT intent.artifact_id
            FROM server_artifact_deletion_intents intent
            WHERE intent.object_key = $1
              AND intent.source_kind = $2
              AND intent.source_id = $3
              AND intent.source_revision = $4
              AND intent.source_identity = $5
              AND intent.next_attempt_at <= now()
              AND (intent.lease_until IS NULL OR intent.lease_until <= now())
            FOR UPDATE SKIP LOCKED
        ),
        claimed AS (
            UPDATE server_artifact_deletion_intents intent
            SET lease_id = $6,
                lease_until = now() + ($7::int * interval '1 second'),
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
            claimed.sha256_hex,
            claimed.size_bytes,
            claimed.source_kind,
            claimed.source_id,
            claimed.source_revision,
            claimed.source_identity,
            claimed.lease_id
        FROM claimed
        JOIN marked ON marked.artifact_id = claimed.artifact_id
        "#,
    )
    .bind(&review.object_key)
    .bind(review.source_kind)
    .bind(review.source_id)
    .bind(review.source_revision)
    .bind(&review.source_identity)
    .bind(lease_id)
    .bind(ARTIFACT_DELETION_LEASE_SECS)
    .fetch_optional(&mut *tx)
    .await?;

    tx.commit().await?;

    row.map(|row| {
        Ok(ArtifactDeletionOwner {
            artifact_id: row.try_get("artifact_id")?,
            object_key: row.try_get("object_key")?,
            sha256_hex: row.try_get("sha256_hex")?,
            size_bytes: row.try_get("size_bytes")?,
            source_kind: row.try_get("source_kind")?,
            source_id: row.try_get("source_id")?,
            source_revision: row.try_get("source_revision")?,
            source_identity: row.try_get("source_identity")?,
            lease_id: row.try_get("lease_id")?,
            pool: pool.clone(),
        })
    })
    .transpose()
}

async fn renew_artifact_deletion(owner: &ArtifactDeletionOwner) -> Result<bool> {
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
    .execute(&owner.pool)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

fn spawn_artifact_deletion_heartbeat(
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
                    if !renew_artifact_deletion(&owner).await? {
                        return Ok(false);
                    }
                }
            }
        }
    });
    (stop_tx, task)
}

async fn delete_confirmed_owned_artifact(
    store: &BackupObjectStore,
    owner: &ArtifactDeletionOwner,
) -> Result<()> {
    let (heartbeat_stop, heartbeat) = spawn_artifact_deletion_heartbeat(owner.clone());
    let delete_result = store.delete_confirmed(&owner.object_key).await;
    let _ = heartbeat_stop.send(());
    ensure!(
        heartbeat
            .await
            .context("artifact deletion heartbeat stopped unexpectedly")??,
        "artifact deletion ownership lost"
    );
    if let Err(error) = delete_result {
        let error_text = error.to_string();
        ensure!(
            defer_artifact_deletion(owner, &error_text).await?,
            "artifact deletion ownership lost after object-store failure"
        );
        return Err(error);
    }
    Ok(())
}

async fn defer_artifact_deletion(owner: &ArtifactDeletionOwner, error: &str) -> Result<bool> {
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
              AND intent.lease_until > now()
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
    .fetch_optional(&owner.pool)
    .await?;
    Ok(deferred.is_some())
}

pub(crate) async fn lock_owned_artifact_deletion_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &ArtifactDeletionOwner,
) -> Result<bool> {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM server_artifact_deletion_intents
            WHERE artifact_id = $1
              AND object_key = $2
              AND sha256_hex = $3
              AND size_bytes = $4
              AND lease_id = $5
              AND lease_until > now()
            FOR UPDATE
        )
        "#,
    )
    .bind(owner.artifact_id)
    .bind(&owner.object_key)
    .bind(&owner.sha256_hex)
    .bind(owner.size_bytes)
    .bind(owner.lease_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(Into::into)
}

pub(crate) async fn finish_owned_artifact_deletion_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &ArtifactDeletionOwner,
) -> Result<bool> {
    let finished = sqlx::query_scalar::<_, Uuid>(
        r#"
        WITH released AS (
            DELETE FROM server_artifact_deletion_intents
            WHERE artifact_id = $1
              AND object_key = $2
              AND sha256_hex = $3
              AND size_bytes = $4
              AND lease_id = $5
            RETURNING artifact_id, object_key, sha256_hex, size_bytes
        ),
        marked AS (
            UPDATE server_artifacts artifact
            SET status = 'deleted',
                deleted_at = now()
            FROM released
            WHERE artifact.id = released.artifact_id
              AND artifact.object_key = released.object_key
              AND artifact.sha256_hex = released.sha256_hex
              AND artifact.size_bytes = released.size_bytes
              AND artifact.status = 'deleting'
            RETURNING artifact.id
        )
        SELECT id FROM marked
        "#,
    )
    .bind(owner.artifact_id)
    .bind(&owner.object_key)
    .bind(&owner.sha256_hex)
    .bind(owner.size_bytes)
    .bind(owner.lease_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(finished.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn producer_waits_for_its_exact_consumer_completion() {
        let (producer, mut inbox) = reviewed_artifact_deletion_channel();
        let job_id = Uuid::new_v4();
        let submitted = tokio::spawn(async move {
            producer
                .delete_history_retention_candidate(HistoryRetentionObjectCandidate::JobOutput {
                    job_id,
                    client_id: "client-a".to_string(),
                    seq: 7,
                    object_key: Some("jobs/client-a/output".to_string()),
                })
                .await
        });

        let command = inbox.commands.recv().await.expect("exact command");
        match command.source {
            ReviewedArtifactDeletionSource::HistoryRetention { candidate } => {
                assert_eq!(candidate.source_id(), job_id);
                assert_eq!(candidate.source_revision(), 8);
                assert_eq!(candidate.object_key(), Some("jobs/client-a/output"));
            }
            ReviewedArtifactDeletionSource::BackupPolicy { .. } => {
                panic!("wrong artifact deletion source")
            }
        }
        assert!(command
            .completed
            .send(Ok(ReviewedArtifactDeletionOutcome::Deleted(1)))
            .is_ok());

        assert!(matches!(
            submitted.await.expect("producer task").expect("completion"),
            ReviewedArtifactDeletionOutcome::Deleted(1)
        ));
    }
}
