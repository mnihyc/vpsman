use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    Connection, PgConnection, PgPool, Postgres, Row, Transaction,
};
use tokio::{sync::watch, task::JoinHandle, time};
use tracing::{debug, warn};

use crate::repository::Repository;

const DASHBOARD_PROJECTION_ACQUIRE_SQL: &str = r#"
SELECT owner_id, client_id, domain, ready_revision
FROM acquire_next_telemetry_dashboard_projection_owner()
"#;
const DASHBOARD_PROJECTION_CLAIM_SQL: &str = r#"
SELECT client_id, domain, change, event_kind, source_bucket_secs,
       block_start_unix, bucket_start_unix,
       captured_block_event_ids, captured_generation_event_ids,
       expected_generation, expected_revision
FROM claim_telemetry_dashboard_projection($1)
"#;
const DASHBOARD_PROJECTION_PUBLISH_SQL: &str =
    "SELECT publish_telemetry_dashboard_projection($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)";
const DASHBOARD_PROJECTION_RELEASE_SQL: &str = "SELECT pg_advisory_unlock($1)";
const DASHBOARD_PROJECTION_ACKNOWLEDGE_SQL: &str = r#"
DELETE FROM telemetry_dashboard_ready_owners
WHERE owner_id = $1 AND wake_revision = $2
"#;
const DASHBOARD_PROJECTION_DEFER_FAILED_SQL: &str = r#"
UPDATE telemetry_dashboard_ready_owners
SET retry_not_before = clock_timestamp() + INTERVAL '1 second'
WHERE owner_id = $1 AND wake_revision = $2
"#;

// This is only the delay before looking for newly queued work after an empty
// claim. A non-empty consumer drains continuously and is never duty-cycle capped.
const DASHBOARD_IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DASHBOARD_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(1);
const DASHBOARD_MAINTENANCE_APPLICATION_NAME: &str = "vpsman-dashboard-projection-maintenance";

pub(crate) struct DashboardProjectionMaintenanceTask {
    shutdown: watch::Sender<bool>,
    handles: Vec<JoinHandle<()>>,
    maintenance_pool: PgPool,
}

impl DashboardProjectionMaintenanceTask {
    pub(crate) fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub(crate) async fn wait_for_unexpected_exit(&mut self) -> Result<()> {
        let (result, lane, remaining) =
            futures_util::future::select_all(self.handles.iter_mut()).await;
        drop(remaining);
        drop(self.handles.swap_remove(lane));
        match result {
            Ok(()) => {
                anyhow::bail!("dashboard projection maintenance lane {lane} exited unexpectedly")
            }
            Err(error) => Err(error)
                .with_context(|| format!("dashboard projection maintenance lane {lane} failed")),
        }
    }

    async fn join(mut self) -> Result<()> {
        let mut first_join_error = None;
        for handle in self.handles.drain(..) {
            if let Err(error) = handle.await {
                if first_join_error.is_none() {
                    first_join_error = Some(error);
                }
            }
        }
        self.maintenance_pool.close().await;
        match first_join_error {
            Some(error) => Err(error).context("dashboard projection maintenance task failed"),
            None => Ok(()),
        }
    }

    /// Stops new claims and waits for the current source transaction to finish.
    pub(crate) async fn shutdown(self) -> Result<()> {
        self.request_shutdown();
        self.join().await
    }
}

pub(crate) fn spawn_dashboard_projection_worker(
    connect_options: PgConnectOptions,
) -> DashboardProjectionMaintenanceTask {
    // An exact owner-visible coordinate/full-block union or a full-generation
    // snapshot is the atomic publication boundary. Session-scoped owner locks
    // coordinate independent publishers across API processes before RR snapshots.
    let maintenance_pool = PgPoolOptions::new()
        // One work-conserving owner makes publication cost observable. It
        // drains immediately until empty; parallel lanes may be considered
        // only after this exact path meets the freshness boundary by itself.
        .max_connections(1)
        .max_lifetime(None)
        .idle_timeout(None)
        .connect_lazy_with(connect_options);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let handles = vec![tokio::spawn(run_dashboard_projection_worker(
        maintenance_pool.clone(),
        shutdown_rx,
    ))];
    DashboardProjectionMaintenanceTask {
        shutdown,
        handles,
        maintenance_pool,
    }
}

pub(crate) fn spawn_dashboard_projection_maintenance_task(
    repo: &Repository,
) -> Option<DashboardProjectionMaintenanceTask> {
    let Repository::Postgres(pool) = repo;
    let connect_options = (*pool.connect_options())
        .clone()
        .application_name(DASHBOARD_MAINTENANCE_APPLICATION_NAME);
    Some(spawn_dashboard_projection_worker(connect_options))
}

fn shutdown_requested(shutdown: &watch::Receiver<bool>) -> bool {
    *shutdown.borrow() || shutdown.has_changed().is_err()
}

async fn shutdown_signal(shutdown: &mut watch::Receiver<bool>) {
    while !shutdown_requested(shutdown) {
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_or_shutdown(shutdown: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    if shutdown_requested(shutdown) {
        return true;
    }
    tokio::select! {
        biased;
        _ = shutdown_signal(shutdown) => true,
        _ = time::sleep(delay) => false,
    }
}

fn dashboard_domain_is_valid(domain: &str) -> bool {
    matches!(domain, "resource" | "network" | "traffic")
}

fn dashboard_source_tier_is_valid(domain: &str, tier: i32) -> bool {
    match domain {
        "resource" | "network" => {
            matches!(tier, 60 | 300 | 1_800 | 3_600 | 10_800 | 21_600 | 86_400)
        }
        "traffic" => matches!(tier, 60 | 3_600 | 10_800 | 21_600 | 86_400),
        _ => false,
    }
}

#[derive(Debug)]
struct DashboardOwner {
    owner_id: i64,
    client_id: String,
    domain: String,
    ready_revision: i64,
}

impl DashboardOwner {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        let owner = Self {
            owner_id: row.try_get("owner_id")?,
            client_id: row.try_get("client_id")?,
            domain: row.try_get("domain")?,
            ready_revision: row.try_get("ready_revision")?,
        };
        anyhow::ensure!(
            owner.owner_id > 0
                && !owner.client_id.is_empty()
                && dashboard_domain_is_valid(&owner.domain)
                && owner.ready_revision > 0,
            "dashboard publication owner has an invalid identity"
        );
        Ok(owner)
    }
}

#[derive(Debug)]
struct DashboardClaim {
    client_id: String,
    domain: String,
    change: String,
    event_kind: Vec<String>,
    source_bucket_secs: Vec<i32>,
    block_start_unix: Vec<i64>,
    bucket_start_unix: Vec<Option<i64>>,
    captured_block_event_ids: Vec<i64>,
    captured_generation_event_ids: Vec<i64>,
    expected_generation: i64,
    expected_revision: i64,
}

impl DashboardClaim {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        let claim = Self {
            client_id: row.try_get("client_id")?,
            domain: row.try_get("domain")?,
            change: row.try_get("change")?,
            event_kind: row.try_get("event_kind")?,
            source_bucket_secs: row.try_get("source_bucket_secs")?,
            block_start_unix: row.try_get("block_start_unix")?,
            bucket_start_unix: row.try_get("bucket_start_unix")?,
            captured_block_event_ids: row.try_get("captured_block_event_ids")?,
            captured_generation_event_ids: row.try_get("captured_generation_event_ids")?,
            expected_generation: row.try_get("expected_generation")?,
            expected_revision: row.try_get("expected_revision")?,
        };
        claim.validate()?;
        Ok(claim)
    }

    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            !self.client_id.is_empty(),
            "dashboard publication claim has an empty owner identity"
        );
        anyhow::ensure!(
            dashboard_domain_is_valid(&self.domain)
                && matches!(self.change.as_str(), "block" | "generation"),
            "dashboard publication claim has an invalid kind"
        );
        anyhow::ensure!(
            self.expected_generation > 0 && self.expected_revision >= 0,
            "dashboard publication claim has an invalid head fence"
        );
        anyhow::ensure!(
            self.event_kind.len() == self.source_bucket_secs.len()
                && self.source_bucket_secs.len() == self.block_start_unix.len()
                && self.block_start_unix.len() == self.bucket_start_unix.len(),
            "dashboard publication claim has misaligned work coordinates"
        );
        let mut prior = None;
        for (((kind, &tier), &block_start), &bucket_start) in self
            .event_kind
            .iter()
            .zip(&self.source_bucket_secs)
            .zip(&self.block_start_unix)
            .zip(&self.bucket_start_unix)
        {
            let tier_is_valid = dashboard_source_tier_is_valid(&self.domain, tier);
            let coordinate_is_valid = match (tier_is_valid, kind.as_str(), bucket_start) {
                (true, "coordinate", Some(bucket_start)) => {
                    bucket_start.rem_euclid(i64::from(tier)) == 0
                        && block_start
                            == bucket_start.div_euclid(i64::from(tier) * 16) * i64::from(tier) * 16
                }
                (true, "full_block", None) => true,
                _ => false,
            };
            let key = (tier, block_start, kind.as_str(), bucket_start);
            anyhow::ensure!(
                tier_is_valid
                    && block_start.rem_euclid(i64::from(tier) * 16) == 0
                    && coordinate_is_valid
                    && prior.is_none_or(|value| value < key),
                "dashboard publication claim has non-canonical exact work"
            );
            prior = Some(key);
        }
        anyhow::ensure!(
            self.captured_block_event_ids.iter().all(|id| *id > 0)
                && self.captured_generation_event_ids.iter().all(|id| *id > 0),
            "dashboard publication claim has an invalid event identity"
        );
        match self.change.as_str() {
            "block" => anyhow::ensure!(
                !self.event_kind.is_empty()
                    && !self.captured_block_event_ids.is_empty()
                    && self.captured_generation_event_ids.is_empty(),
                "dashboard block claim has an invalid event shape"
            ),
            "generation" => anyhow::ensure!(
                self.event_kind.is_empty()
                    && self.source_bucket_secs.is_empty()
                    && self.block_start_unix.is_empty()
                    && self.bucket_start_unix.is_empty()
                    && !self.captured_generation_event_ids.is_empty(),
                "dashboard generation claim has an invalid event shape"
            ),
            _ => unreachable!(),
        }
        Ok(())
    }
}

#[derive(Debug)]
enum DashboardPublishOutcome {
    Idle,
    Contended,
    Published(DashboardClaim),
    Failed {
        claim: DashboardClaim,
        error: anyhow::Error,
    },
}

async fn set_repeatable_read(transaction: &mut Transaction<'_, Postgres>) -> Result<()> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn acquire_owner(connection: &mut PgConnection) -> Result<Option<DashboardOwner>> {
    let row = sqlx::query(DASHBOARD_PROJECTION_ACQUIRE_SQL)
        .fetch_optional(connection)
        .await?;
    row.as_ref().map(DashboardOwner::from_row).transpose()
}

async fn release_owner(connection: &mut PgConnection, owner_id: i64) -> Result<()> {
    let released = sqlx::query_scalar::<_, bool>(DASHBOARD_PROJECTION_RELEASE_SQL)
        .bind(owner_id)
        .fetch_one(connection)
        .await?;
    anyhow::ensure!(released, "dashboard publication owner lock was not held");
    Ok(())
}

async fn acknowledge_owner(
    transaction: &mut Transaction<'_, Postgres>,
    owner: &DashboardOwner,
) -> std::result::Result<(), sqlx::Error> {
    // The ready row is a derived hint, so consume it in the same transaction as
    // its immutable events. An enqueue that changed wake_revision before this
    // statement is not deleted. An enqueue that reaches the row afterward
    // waits for this transaction and recreates the hint after the delete. A
    // publication rollback restores both the events and this row.
    sqlx::query(DASHBOARD_PROJECTION_ACKNOWLEDGE_SQL)
        .bind(owner.owner_id)
        .bind(owner.ready_revision)
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn defer_failed_owner(connection: &mut PgConnection, owner: &DashboardOwner) -> Result<()> {
    // The failure transaction already rolled back. Defer only the exact ready
    // revision it observed; a concurrent/newer enqueue increments the revision
    // and resets retry_not_before, so that new work stays immediately eligible.
    sqlx::query(DASHBOARD_PROJECTION_DEFER_FAILED_SQL)
        .bind(owner.owner_id)
        .bind(owner.ready_revision)
        .execute(connection)
        .await?;
    Ok(())
}

fn is_serialization_failure(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(database) if database.code().as_deref() == Some("40001"))
}

async fn commit_acknowledged(
    mut transaction: Transaction<'_, Postgres>,
    owner: &DashboardOwner,
    outcome: DashboardPublishOutcome,
) -> Result<DashboardPublishOutcome> {
    match acknowledge_owner(&mut transaction, owner).await {
        Ok(()) => {
            transaction.commit().await?;
            Ok(outcome)
        }
        Err(error) if is_serialization_failure(&error) => {
            // A post-snapshot enqueue committed a newer ready-row version.
            // PostgreSQL RR reports this normal optimistic conflict as 40001.
            // Roll back publication and event consumption together, then let
            // this consumer immediately reacquire the still-ready owner.
            transaction.rollback().await?;
            Ok(DashboardPublishOutcome::Contended)
        }
        Err(error) => {
            transaction.rollback().await?;
            Err(error).context("failed to acknowledge dashboard publication owner")
        }
    }
}

/// Publishes either one exact owner-visible work union or one full owner
/// generation snapshot behind one head CAS. Concurrent ingest never takes the
/// owner lock.
async fn publish_owned(
    connection: &mut PgConnection,
    owner: &DashboardOwner,
) -> Result<DashboardPublishOutcome> {
    let mut transaction = connection.begin().await?;
    set_repeatable_read(&mut transaction).await?;
    let row = sqlx::query(DASHBOARD_PROJECTION_CLAIM_SQL)
        .bind(owner.owner_id)
        .fetch_optional(&mut *transaction)
        .await?;
    let Some(row) = row else {
        // An empty hint is not publication authority. Its RR transaction
        // consumes the exact captured revision without rereading history or
        // republishing; rollback restores the hint atomically.
        return commit_acknowledged(transaction, owner, DashboardPublishOutcome::Idle).await;
    };
    let claim = DashboardClaim::from_row(&row)?;
    anyhow::ensure!(
        claim.client_id == owner.client_id && claim.domain == owner.domain,
        "dashboard publication claim changed its acquired owner"
    );
    // Claim consumption, resident rows, the published head, and NOTIFY are one
    // derived transaction. If PostgreSQL crashes before this asynchronous
    // commit reaches durable WAL, the queue consumption and cursor disappear
    // with the derived rows and the canonical mutations are replayed.
    sqlx::query("SET LOCAL synchronous_commit = OFF")
        .execute(&mut *transaction)
        .await?;
    let published = sqlx::query_scalar::<_, bool>(DASHBOARD_PROJECTION_PUBLISH_SQL)
        .bind(&claim.client_id)
        .bind(&claim.domain)
        .bind(&claim.change)
        .bind(&claim.event_kind)
        .bind(&claim.source_bucket_secs)
        .bind(&claim.block_start_unix)
        .bind(&claim.bucket_start_unix)
        .bind(&claim.captured_block_event_ids)
        .bind(&claim.captured_generation_event_ids)
        .bind(claim.expected_generation)
        .bind(claim.expected_revision)
        .fetch_one(&mut *transaction)
        .await;
    let failure = match published {
        Ok(true) => None,
        Ok(false) => Some(anyhow::anyhow!(
            "claimed dashboard publication lost its exact head CAS"
        )),
        Err(error) => Some(
            anyhow::Error::from(error)
                .context("claimed dashboard publication failed before commit"),
        ),
    };
    if let Some(error) = failure {
        transaction.rollback().await?;
        return Ok(DashboardPublishOutcome::Failed { claim, error });
    }
    commit_acknowledged(
        transaction,
        owner,
        DashboardPublishOutcome::Published(claim),
    )
    .await
}

async fn publish_one(connection: &mut PgConnection) -> Result<DashboardPublishOutcome> {
    let Some(owner) = acquire_owner(connection).await? else {
        return Ok(DashboardPublishOutcome::Idle);
    };

    let publication = publish_owned(connection, &owner).await;
    // A deterministic publication failure leaves every captured event queued.
    // Move only the exact failed ready revision out of the due set, then release
    // its owner immediately so this single work-conserving consumer can drain
    // later healthy owners. A newer wake revision makes this CAS a no-op.
    let deferral = if matches!(&publication, Ok(DashboardPublishOutcome::Failed { .. })) {
        defer_failed_owner(connection, &owner).await
    } else {
        Ok(())
    };
    let release = release_owner(connection, owner.owner_id).await;
    if let Err(release_error) = release {
        return match publication {
            Err(publication_error) => Err(release_error).with_context(|| {
                format!(
                    "failed to release dashboard publication owner after publication error: {publication_error:#}"
                )
            }),
            Ok(_) => Err(release_error).context(
                "failed to release dashboard publication owner; connection must close",
            ),
        };
    }
    deferral.context("failed to defer failed dashboard publication owner")?;
    publication
}

#[cfg(test)]
mod commit_scope_tests {
    #[test]
    fn asynchronous_commit_is_scoped_only_to_a_nonempty_atomic_publication() {
        let source = include_str!("dashboard_projection_maintenance.rs");
        let (production, _) = source
            .split_once("#[cfg(test)]\nmod commit_scope_tests")
            .expect("dashboard publication production boundary");
        assert_eq!(
            production
                .matches("SET LOCAL synchronous_commit = OFF")
                .count(),
            1,
            "owner acquisition and empty claims must remain synchronous"
        );
        let (_, publish) = source
            .split_once("async fn publish_owned")
            .expect("dashboard publication function");
        let (publish, _) = publish
            .split_once("async fn publish_one")
            .expect("dashboard publication function boundary");
        let empty_commit = publish
            .find("DashboardPublishOutcome::Idle")
            .expect("empty publication path");
        let asynchronous_commit = publish
            .find("SET LOCAL synchronous_commit = OFF")
            .expect("derived publication commit mode");
        assert!(empty_commit < asynchronous_commit);
        assert_eq!(publish.matches("commit_acknowledged(").count(), 2);
        assert!(publish.contains("DASHBOARD_PROJECTION_CLAIM_SQL"));
        assert!(publish.contains("DASHBOARD_PROJECTION_PUBLISH_SQL"));
        let (_, commit_acknowledged) = production
            .split_once("async fn commit_acknowledged")
            .expect("atomic ready acknowledgement");
        let (commit_acknowledged, _) = commit_acknowledged
            .split_once("/// Publishes either one exact owner-visible work union")
            .expect("atomic acknowledgement boundary");
        let acknowledgement = commit_acknowledged
            .find("acknowledge_owner(&mut transaction, owner).await")
            .expect("ready acknowledgement in publication transaction");
        let commit = commit_acknowledged
            .find("transaction.commit().await?")
            .expect("atomic publication commit");
        assert!(acknowledgement < commit);
        assert!(commit_acknowledged.contains("is_serialization_failure(&error)"));
        assert!(commit_acknowledged.contains("DashboardPublishOutcome::Contended"));
        assert!(commit_acknowledged.contains("transaction.rollback().await?"));
    }

    #[test]
    fn session_owner_wraps_the_repeatable_read_transaction() {
        let source = include_str!("dashboard_projection_maintenance.rs");
        let (production, _) = source
            .split_once("#[cfg(test)]\nmod commit_scope_tests")
            .expect("dashboard production boundary");
        let (_, publish) = source
            .split_once("async fn publish_one")
            .expect("dashboard owner wrapper");
        let (publish, _) = publish
            .split_once("#[cfg(test)]\nmod commit_scope_tests")
            .expect("dashboard owner wrapper boundary");
        let acquire = publish
            .find("acquire_owner(connection).await?")
            .expect("pre-transaction owner acquisition");
        let transaction = publish
            .find("publish_owned(connection, &owner).await")
            .expect("repeatable-read owner publication");
        let failure_defer = publish
            .find("defer_failed_owner(connection, &owner).await")
            .expect("exact failed-owner defer");
        let release = publish
            .find("release_owner(connection, owner.owner_id).await")
            .expect("post-transaction owner release");
        assert!(
            acquire < transaction && transaction < failure_defer && failure_defer < release,
            "the failed ready revision must be deferred before immediate owner release"
        );
        assert!(
            !publish.contains("acknowledge_owner("),
            "ready acknowledgement must commit atomically with source-event consumption"
        );
        assert!(production.contains("SELECT pg_advisory_unlock($1)"));
        assert!(production.contains("WHERE owner_id = $1 AND wake_revision = $2"));
        assert!(
            production.contains("SET retry_not_before = clock_timestamp() + INTERVAL '1 second'")
        );
        assert!(!publish.contains("time::sleep"));
        assert!(!publish.contains("wait_or_shutdown"));
        let (_, connection_loop) = source
            .rsplit_once("async fn run_connection")
            .expect("dashboard connection loop");
        let (connection_loop, _) = connection_loop
            .split_once("async fn run_dashboard_projection_worker")
            .expect("dashboard connection loop boundary");
        let contention_branch = connection_loop
            .split_once("Ok(DashboardPublishOutcome::Contended) => {")
            .expect("normal optimistic contention branch")
            .1
            .split_once("Ok(DashboardPublishOutcome::Failed")
            .expect("contention branch boundary")
            .0;
        assert!(contention_branch.contains("tokio::task::yield_now().await;"));
        assert!(!contention_branch.contains("wait_or_shutdown"));
        assert!(!publish.contains("DashboardPublishOutcome::Contended"));
        assert!(!production.contains("defer_telemetry_dashboard_projection"));
    }
}

async fn run_connection(
    connection: &mut PgConnection,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if shutdown_requested(shutdown) {
            return Ok(());
        }
        match publish_one(connection).await {
            Ok(DashboardPublishOutcome::Published(claim)) => {
                debug!(client_id = %claim.client_id, domain = %claim.domain,
                    change = %claim.change, "published dashboard telemetry mutation");
                tokio::task::yield_now().await;
            }
            Ok(DashboardPublishOutcome::Idle) => {
                if wait_or_shutdown(shutdown, DASHBOARD_IDLE_POLL_INTERVAL).await {
                    return Ok(());
                }
            }
            Ok(DashboardPublishOutcome::Contended) => {
                tokio::task::yield_now().await;
            }
            Ok(DashboardPublishOutcome::Failed { claim, error }) => {
                warn!(%error, client_id = %claim.client_id, domain = %claim.domain,
                    change = %claim.change, "failed dashboard mutation remains queued");
                tokio::task::yield_now().await;
            }
            Err(error) => {
                warn!(%error, "failed to publish dashboard telemetry mutation");
                if connection.ping().await.is_err() {
                    return Err(error).context("dashboard maintenance connection failed");
                }
                return Err(error)
                    .context("dashboard publication contract failed on a live connection");
            }
        }
    }
}

async fn run_dashboard_projection_worker(
    maintenance_pool: PgPool,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let mut connection = tokio::select! {
            biased;
            _ = shutdown_signal(&mut shutdown) => return,
            connection = maintenance_pool.acquire() => match connection {
                Ok(connection) => connection,
                Err(error) => {
                    warn!(%error, "failed to acquire dashboard maintenance connection");
                    if wait_or_shutdown(&mut shutdown, DASHBOARD_CONNECTION_RETRY_DELAY).await {
                        return;
                    }
                    continue;
                }
            },
        };
        match run_connection(&mut connection, &mut shutdown).await {
            Ok(()) => {
                if let Err(error) = connection.close().await {
                    warn!(%error, "failed to close dashboard maintenance connection");
                }
                return;
            }
            Err(error) => {
                warn!(%error, "dashboard maintenance connection will be reacquired");
                if let Err(close_error) = connection.close().await {
                    warn!(%close_error, "failed to close unhealthy dashboard maintenance connection");
                }
                if wait_or_shutdown(&mut shutdown, DASHBOARD_CONNECTION_RETRY_DELAY).await {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_claim(
        event_kind: Vec<&str>,
        block_start_unix: Vec<i64>,
        bucket_start_unix: Vec<Option<i64>>,
    ) -> DashboardClaim {
        block_claim_for(
            "resource",
            vec![60; event_kind.len()],
            event_kind,
            block_start_unix,
            bucket_start_unix,
        )
    }

    fn block_claim_for(
        domain: &str,
        source_bucket_secs: Vec<i32>,
        event_kind: Vec<&str>,
        block_start_unix: Vec<i64>,
        bucket_start_unix: Vec<Option<i64>>,
    ) -> DashboardClaim {
        DashboardClaim {
            client_id: "client-a".to_string(),
            domain: domain.to_string(),
            change: "block".to_string(),
            source_bucket_secs,
            event_kind: event_kind.into_iter().map(str::to_string).collect(),
            block_start_unix,
            bucket_start_unix,
            captured_block_event_ids: vec![1],
            captured_generation_event_ids: Vec::new(),
            expected_generation: 1,
            expected_revision: 0,
        }
    }

    fn full_block_claim(domain: &str, source_bucket_secs: Vec<i32>) -> DashboardClaim {
        let work_len = source_bucket_secs.len();
        block_claim_for(
            domain,
            source_bucket_secs,
            vec!["full_block"; work_len],
            vec![0; work_len],
            vec![None; work_len],
        )
    }

    fn generation_claim(domain: &str) -> DashboardClaim {
        DashboardClaim {
            client_id: "client-a".to_string(),
            domain: domain.to_string(),
            change: "generation".to_string(),
            source_bucket_secs: Vec::new(),
            event_kind: Vec::new(),
            block_start_unix: Vec::new(),
            bucket_start_unix: Vec::new(),
            captured_block_event_ids: Vec::new(),
            captured_generation_event_ids: vec![1],
            expected_generation: 1,
            expected_revision: 0,
        }
    }

    #[test]
    fn grouped_contract_is_bound_exactly_once() {
        assert!(DASHBOARD_PROJECTION_ACQUIRE_SQL
            .contains("acquire_next_telemetry_dashboard_projection_owner()"));
        assert!(DASHBOARD_PROJECTION_CLAIM_SQL.contains("claim_telemetry_dashboard_projection($1)"));
        assert_eq!(DASHBOARD_PROJECTION_CLAIM_SQL.matches('$').count(), 1);
        assert_eq!(DASHBOARD_PROJECTION_PUBLISH_SQL.matches('$').count(), 11);
        assert_eq!(DASHBOARD_PROJECTION_RELEASE_SQL.matches('$').count(), 1);
        assert_eq!(DASHBOARD_PROJECTION_ACKNOWLEDGE_SQL.matches('$').count(), 2);
        assert_eq!(
            DASHBOARD_PROJECTION_DEFER_FAILED_SQL.matches('$').count(),
            2
        );
    }

    #[test]
    fn exact_work_validation_distinguishes_coordinates_from_full_blocks() {
        assert!(block_claim(
            vec!["coordinate", "coordinate"],
            vec![0, 0],
            vec![Some(0), Some(60)],
        )
        .validate()
        .is_ok());
        assert!(block_claim(vec!["full_block"], vec![0], vec![None])
            .validate()
            .is_ok());
        assert!(block_claim(vec!["coordinate"], vec![0], vec![None])
            .validate()
            .is_err());
        assert!(block_claim(vec!["full_block"], vec![0], vec![Some(0)])
            .validate()
            .is_err());
        assert!(block_claim(vec!["coordinate"], vec![0], vec![Some(61)])
            .validate()
            .is_err());
    }

    #[test]
    fn dashboard_domains_accept_only_their_canonical_source_tiers() {
        assert!(dashboard_domain_is_valid("traffic"));
        assert!(!dashboard_domain_is_valid("unknown"));
        for domain in ["resource", "network"] {
            assert!(full_block_claim(domain, vec![300, 1_800])
                .validate()
                .is_ok());
        }
        assert!(
            full_block_claim("traffic", vec![60, 3_600, 10_800, 21_600, 86_400])
                .validate()
                .is_ok()
        );
        for invalid_tier in [300, 1_800] {
            assert!(full_block_claim("traffic", vec![invalid_tier])
                .validate()
                .is_err());
        }
        assert!(full_block_claim("unknown", vec![60]).validate().is_err());
    }

    #[test]
    fn traffic_generation_keeps_the_empty_work_contract() {
        let mut claim = generation_claim("traffic");
        assert!(claim.validate().is_ok());

        claim.event_kind.push("full_block".to_string());
        claim.source_bucket_secs.push(60);
        claim.block_start_unix.push(0);
        claim.bucket_start_unix.push(None);
        assert!(claim.validate().is_err());

        let mut block = full_block_claim("traffic", vec![60]);
        block.event_kind.clear();
        block.source_bucket_secs.clear();
        block.block_start_unix.clear();
        block.bucket_start_unix.clear();
        assert!(block.validate().is_err());
    }

    #[test]
    fn dashboard_sql_uses_only_canonical_active_plus_retained_sources() {
        let dashboard = include_str!("../../../../migrations/0006_telemetry_dashboard.sql");
        assert_eq!(
            dashboard
                .matches("public.telemetry_network_durable_points_source(")
                .count(),
            3,
            "each network projection owner must bind its client/time/tier before reading durable history"
        );
        assert!(
            dashboard
                .matches("            p_interfaces\n        )")
                .count()
                >= 2,
            "network projection owners must bind their frozen interface vectors before reading durable history"
        );
        assert!(dashboard.contains("public.telemetry_ping_rollups source"));
        assert!(!dashboard.contains("public.telemetry_ping_points source"));
        assert!(!dashboard.contains("telemetry_resource_latest"));
        assert!(!dashboard.contains("telemetry_dashboard_resource_source"));
        assert!(!dashboard.contains("telemetry_dashboard_network_source"));
        assert!(!dashboard.contains("telemetry_dashboard_block_events_coordinate_idx"));
        assert!(dashboard.contains("event_kind IN ('coordinate', 'full_block')"));
        assert!(dashboard.contains("refresh_telemetry_dashboard_resource_coordinates"));
        assert!(dashboard.contains("refresh_telemetry_dashboard_network_coordinates"));
        assert!(!dashboard.contains("refresh_telemetry_dashboard_resource_coordinate("));
        assert!(!dashboard.contains("refresh_telemetry_dashboard_network_coordinate("));
        assert!(!dashboard.contains("queue_telemetry_dashboard_coordinate("));
        assert!(!dashboard.contains("queue_telemetry_dashboard_full_block("));
        for (refresh, replacement, edges, forbidden) in [
            (
                "refresh_telemetry_dashboard_resource_coordinates",
                "replace_telemetry_dashboard_resource_coordinates",
                "telemetry_dashboard_resource_block_edges",
                "FROM public.telemetry_rollups source",
            ),
            (
                "refresh_telemetry_dashboard_resource_block",
                "replace_telemetry_dashboard_resource_closed_block",
                "telemetry_dashboard_resource_block_edges",
                "FROM public.telemetry_rollups source",
            ),
            (
                "refresh_telemetry_dashboard_network_coordinates",
                "replace_telemetry_dashboard_network_coordinates",
                "telemetry_dashboard_network_block_edges",
                "FROM public.telemetry_network_rates source",
            ),
            (
                "refresh_telemetry_dashboard_network_block",
                "replace_telemetry_dashboard_network_closed_block",
                "telemetry_dashboard_network_block_edges",
                "FROM public.telemetry_network_rates source",
            ),
        ] {
            let body = dashboard
                .split_once(&format!("CREATE FUNCTION public.{refresh}("))
                .unwrap_or_else(|| panic!("missing dashboard refresh {refresh}"))
                .1
                .split_once("$$;")
                .unwrap_or_else(|| panic!("unterminated dashboard refresh {refresh}"))
                .0;
            let replace_at = body
                .find(replacement)
                .unwrap_or_else(|| panic!("{refresh} does not replace its exact block"));
            let edges_at = body
                .find(edges)
                .unwrap_or_else(|| panic!("{refresh} does not read compact block edges"));
            assert!(replace_at < edges_at, "{refresh} reads stale block edges");
            assert!(
                !body.contains(forbidden),
                "{refresh} walks retained history to recover compact bounds"
            );
        }
        let publish = dashboard
            .split_once("CREATE FUNCTION public.publish_telemetry_dashboard_projection(")
            .expect("dashboard publisher")
            .1
            .split_once("$$;")
            .expect("dashboard publisher boundary")
            .0;
        for domain in ["resource", "network"] {
            assert_eq!(
                publish
                    .matches(&format!(
                        "PERFORM public.refresh_telemetry_dashboard_{domain}_coordinates("
                    ))
                    .count(),
                1,
                "{domain} coordinates must cross one setwise publication boundary",
            );
            assert!(!publish.contains(&format!("refresh_telemetry_dashboard_{domain}_coordinate(")));
        }
        for domain in ["resource", "network"] {
            let replacement = dashboard
                .split_once(&format!(
                    "CREATE FUNCTION public.replace_telemetry_dashboard_{domain}_coordinates("
                ))
                .unwrap_or_else(|| panic!("missing setwise {domain} replacement"))
                .1
                .split_once("$$;")
                .unwrap_or_else(|| panic!("unterminated setwise {domain} replacement"))
                .0;
            assert!(replacement.contains("requested AS MATERIALIZED"));
            assert!(replacement.contains("coordinate_source AS MATERIALIZED"));
            assert!(replacement.contains("affected_blocks AS MATERIALIZED"));
            assert!(replacement.contains("CASE WHEN source.bucket_start_unix IS NOT NULL THEN"));
            assert!(replacement.contains("WHEN MATCHED AND NOT source.has_samples THEN"));
            assert!(replacement.contains("WHEN NOT MATCHED AND source.has_samples THEN"));
        }
        let transfer_guarded_consumers = [
            "queue_telemetry_resource_blocks_after_delete",
            "queue_telemetry_resource_blocks_after_update",
            "queue_telemetry_network_blocks_after_insert",
            "queue_telemetry_network_blocks_after_delete",
            "queue_telemetry_network_blocks_after_update",
            "queue_telemetry_network_samples_after_insert",
            "queue_telemetry_network_samples_after_delete",
            "queue_telemetry_network_samples_after_update",
            "maintain_telemetry_ping_dashboard_after_insert",
            "maintain_telemetry_ping_dashboard_after_delete",
            "maintain_telemetry_ping_dashboard_after_update",
        ];
        for consumer in transfer_guarded_consumers {
            let body = dashboard
                .split_once(&format!("CREATE FUNCTION public.{consumer}()"))
                .unwrap_or_else(|| panic!("missing dashboard transfer consumer {consumer}"))
                .1
                .split_once("$$;")
                .unwrap_or_else(|| panic!("unterminated dashboard transfer consumer {consumer}"))
                .0;
            assert_eq!(
                body.matches("IF public.telemetry_dashboard_ownership_transfer_requested() THEN")
                    .count(),
                1,
                "dashboard transfer consumer {consumer} must own exactly one transfer boundary",
            );
        }
        assert_eq!(
            dashboard
                .matches("IF public.telemetry_dashboard_ownership_transfer_requested() THEN")
                .count(),
            transfer_guarded_consumers.len(),
            "every ownership-transfer guard must belong to a named physical-owner consumer",
        );
    }

    #[test]
    fn maintenance_has_one_work_conserving_owner_without_a_duty_cap() {
        let source = include_str!("dashboard_projection_maintenance.rs");
        let (runtime, _) = source
            .split_once("#[cfg(test)]\nmod tests")
            .expect("maintenance test boundary");
        assert!(runtime.contains(".max_connections(1)"));
        assert!(!runtime.contains("DASHBOARD_PROJECTION_LANES"));
        assert!(!runtime.contains("shutdown_rx.clone()"));
        assert_eq!(runtime.matches("tokio::spawn(").count(), 1);
        assert!(!runtime.contains("available_parallelism"));
        assert!(!runtime.contains("worker_count"));
        assert!(!runtime.contains("reconstruct"));
        assert!(runtime.contains("dashboard publication contract failed on a live connection"));
    }

    #[test]
    fn idle_poll_is_one_second_today() {
        assert_eq!(DASHBOARD_IDLE_POLL_INTERVAL, Duration::from_secs(1));
    }

    #[tokio::test(start_paused = true)]
    async fn empty_poll_is_shutdown_interruptible() {
        let (shutdown, mut receiver) = watch::channel(false);
        let wait =
            tokio::spawn(
                async move { wait_or_shutdown(&mut receiver, Duration::from_secs(9)).await },
            );
        tokio::task::yield_now().await;
        shutdown.send(true).unwrap();
        assert!(wait.await.unwrap());
    }
}
