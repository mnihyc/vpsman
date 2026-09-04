use std::{collections::HashMap, time::Duration};

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
const DASHBOARD_COORDINATE_ACQUIRE_SQL: &str = r#"
SELECT owner_id, client_id, domain, ready_revision
FROM acquire_telemetry_dashboard_coordinate_projection_owners()
"#;
const DASHBOARD_COORDINATE_CLAIMS_SQL: &str = r#"
SELECT owner_id, client_id, domain, ready_revision,
       event_kind, source_bucket_secs, block_start_unix,
       bucket_start_unix, captured_block_event_ids,
       expected_generation, expected_revision,
       generation_interfaces, generation_source_kinds
FROM telemetry_dashboard_coordinate_projection_claims($1)
"#;
const DASHBOARD_RESOURCE_PREPARE_SQL: &str =
    "SELECT * FROM prepare_telemetry_dashboard_resource_coordinate_blocks($1)";
const DASHBOARD_NETWORK_PREPARE_SQL: &str =
    "SELECT * FROM prepare_telemetry_dashboard_network_coordinate_blocks($1)";
const DASHBOARD_TRAFFIC_PREPARE_SQL: &str =
    "SELECT * FROM prepare_telemetry_dashboard_traffic_coordinate_blocks($1)";
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
const DASHBOARD_PREPARED_BEGIN_SQL: &str = r#"
DELETE FROM telemetry_dashboard_ready_owners
WHERE owner_id = $1 AND wake_revision = $2
RETURNING TRUE
"#;
const DASHBOARD_PREPARED_COMPLETE_SQL: &str = r#"
SELECT complete_telemetry_dashboard_coordinate_projection(
    $1,$2,$3,$4,$5,$6,$7,$8,$9
)
"#;
const DASHBOARD_RESOURCE_BLOCK_APPLY_SQL: &str = r#"
WITH removed AS (
    DELETE FROM telemetry_dashboard_resource_blocks
    WHERE client_id = $1
      AND generation = $2
      AND source_bucket_secs = $4
      AND block_start_unix = $5
      AND NOT $6
), applied AS (
    INSERT INTO telemetry_dashboard_resource_blocks (
        client_id, generation, source_bucket_secs,
        block_start_unix, published_revision,
        sample_counts, cpu_load_1_sums, cpu_load_1_maxes,
        memory_total_bytes_maxes, memory_used_ratio_sums,
        memory_used_ratio_maxes, disk_sample_counts,
        disk_total_bytes_maxes, disk_used_ratio_sums,
        disk_used_ratio_maxes, latest_observed_unix
    )
    SELECT $1,$2,$4,$5,$3,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17
    WHERE $6
    ON CONFLICT (
        client_id, generation, source_bucket_secs, block_start_unix
    ) DO UPDATE SET
        published_revision = EXCLUDED.published_revision,
        sample_counts = EXCLUDED.sample_counts,
        cpu_load_1_sums = EXCLUDED.cpu_load_1_sums,
        cpu_load_1_maxes = EXCLUDED.cpu_load_1_maxes,
        memory_total_bytes_maxes = EXCLUDED.memory_total_bytes_maxes,
        memory_used_ratio_sums = EXCLUDED.memory_used_ratio_sums,
        memory_used_ratio_maxes = EXCLUDED.memory_used_ratio_maxes,
        disk_sample_counts = EXCLUDED.disk_sample_counts,
        disk_total_bytes_maxes = EXCLUDED.disk_total_bytes_maxes,
        disk_used_ratio_sums = EXCLUDED.disk_used_ratio_sums,
        disk_used_ratio_maxes = EXCLUDED.disk_used_ratio_maxes,
        latest_observed_unix = EXCLUDED.latest_observed_unix
)
SELECT TRUE
"#;
const DASHBOARD_NETWORK_BLOCK_APPLY_SQL: &str = r#"
WITH removed AS (
    DELETE FROM telemetry_dashboard_network_blocks
    WHERE client_id = $1
      AND generation = $2
      AND source_bucket_secs = $4
      AND block_start_unix = $5
      AND NOT $7
), applied AS (
    INSERT INTO telemetry_dashboard_network_blocks (
        client_id, generation, interface_width,
        source_bucket_secs, block_start_unix, published_revision,
        sample_counts, latest_observed_unix,
        rx_bytes_last, tx_bytes_last, rx_counter_epoch, tx_counter_epoch
    )
    SELECT $1,$2,$6,$4,$5,$3,$8,$9,$10,$11,$12,$13
    WHERE $7
    ON CONFLICT (
        client_id, generation, source_bucket_secs, block_start_unix
    ) DO UPDATE SET
        published_revision = EXCLUDED.published_revision,
        interface_width = EXCLUDED.interface_width,
        sample_counts = EXCLUDED.sample_counts,
        latest_observed_unix = EXCLUDED.latest_observed_unix,
        rx_bytes_last = EXCLUDED.rx_bytes_last,
        tx_bytes_last = EXCLUDED.tx_bytes_last,
        rx_counter_epoch = EXCLUDED.rx_counter_epoch,
        tx_counter_epoch = EXCLUDED.tx_counter_epoch
)
SELECT TRUE
"#;
const DASHBOARD_TRAFFIC_BLOCK_APPLY_SQL: &str = r#"
WITH removed AS (
    DELETE FROM telemetry_dashboard_traffic_blocks
    WHERE client_id = $1
      AND generation = $2
      AND source_bucket_secs = $4
      AND block_start_unix = $5
      AND NOT $6
), applied AS (
    INSERT INTO telemetry_dashboard_traffic_blocks (
        client_id, generation, source_bucket_secs,
        block_start_unix, published_revision,
        rx_valid_counts, tx_valid_counts, rx_bytes, tx_bytes
    )
    SELECT $1,$2,$4,$5,$3,$7,$8,$9,$10
    WHERE $6
    ON CONFLICT (
        client_id, generation, source_bucket_secs, block_start_unix
    ) DO UPDATE SET
        published_revision = EXCLUDED.published_revision,
        rx_valid_counts = EXCLUDED.rx_valid_counts,
        tx_valid_counts = EXCLUDED.tx_valid_counts,
        rx_bytes = EXCLUDED.rx_bytes,
        tx_bytes = EXCLUDED.tx_bytes
)
SELECT TRUE
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

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
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

#[derive(Debug)]
struct PreparedResourceBlock {
    owner_id: i64,
    source_bucket_secs: i32,
    block_start_unix: i64,
    has_samples: bool,
    sample_counts: Vec<i64>,
    cpu_load_1_sums: Vec<Option<f64>>,
    cpu_load_1_maxes: Vec<Option<f32>>,
    memory_total_bytes_maxes: Vec<Option<i64>>,
    memory_used_ratio_sums: Vec<Option<f64>>,
    memory_used_ratio_maxes: Vec<Option<f32>>,
    disk_sample_counts: Vec<i64>,
    disk_total_bytes_maxes: Vec<Option<i64>>,
    disk_used_ratio_sums: Vec<Option<f64>>,
    disk_used_ratio_maxes: Vec<Option<f32>>,
    latest_observed_unix: Vec<Option<i64>>,
}

impl PreparedResourceBlock {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        let block = Self {
            owner_id: row.try_get("owner_id")?,
            source_bucket_secs: row.try_get("source_bucket_secs")?,
            block_start_unix: row.try_get("block_start_unix")?,
            has_samples: row.try_get("has_samples")?,
            sample_counts: row.try_get("sample_counts")?,
            cpu_load_1_sums: row.try_get("cpu_load_1_sums")?,
            cpu_load_1_maxes: row.try_get("cpu_load_1_maxes")?,
            memory_total_bytes_maxes: row.try_get("memory_total_bytes_maxes")?,
            memory_used_ratio_sums: row.try_get("memory_used_ratio_sums")?,
            memory_used_ratio_maxes: row.try_get("memory_used_ratio_maxes")?,
            disk_sample_counts: row.try_get("disk_sample_counts")?,
            disk_total_bytes_maxes: row.try_get("disk_total_bytes_maxes")?,
            disk_used_ratio_sums: row.try_get("disk_used_ratio_sums")?,
            disk_used_ratio_maxes: row.try_get("disk_used_ratio_maxes")?,
            latest_observed_unix: row.try_get("latest_observed_unix")?,
        };
        let lengths = [
            block.sample_counts.len(),
            block.cpu_load_1_sums.len(),
            block.cpu_load_1_maxes.len(),
            block.memory_total_bytes_maxes.len(),
            block.memory_used_ratio_sums.len(),
            block.memory_used_ratio_maxes.len(),
            block.disk_sample_counts.len(),
            block.disk_total_bytes_maxes.len(),
            block.disk_used_ratio_sums.len(),
            block.disk_used_ratio_maxes.len(),
            block.latest_observed_unix.len(),
        ];
        anyhow::ensure!(
            block.owner_id > 0
                && dashboard_source_tier_is_valid("resource", block.source_bucket_secs)
                && block
                    .block_start_unix
                    .rem_euclid(i64::from(block.source_bucket_secs) * 16)
                    == 0
                && lengths.iter().all(|length| *length == 16)
                && block.has_samples == block.sample_counts.iter().any(|count| *count > 0),
            "prepared resource dashboard block is invalid"
        );
        Ok(block)
    }
}

#[derive(Debug)]
struct PreparedNetworkBlock {
    owner_id: i64,
    source_bucket_secs: i32,
    block_start_unix: i64,
    interface_width: i32,
    has_samples: bool,
    sample_counts: Vec<i64>,
    latest_observed_unix: Vec<Option<i64>>,
    rx_bytes_last: Vec<Option<i64>>,
    tx_bytes_last: Vec<Option<i64>>,
    rx_counter_epoch: Vec<Option<i64>>,
    tx_counter_epoch: Vec<Option<i64>>,
}

impl PreparedNetworkBlock {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        let block = Self {
            owner_id: row.try_get("owner_id")?,
            source_bucket_secs: row.try_get("source_bucket_secs")?,
            block_start_unix: row.try_get("block_start_unix")?,
            interface_width: row.try_get("interface_width")?,
            has_samples: row.try_get("has_samples")?,
            sample_counts: row.try_get("sample_counts")?,
            latest_observed_unix: row.try_get("latest_observed_unix")?,
            rx_bytes_last: row.try_get("rx_bytes_last")?,
            tx_bytes_last: row.try_get("tx_bytes_last")?,
            rx_counter_epoch: row.try_get("rx_counter_epoch")?,
            tx_counter_epoch: row.try_get("tx_counter_epoch")?,
        };
        let expected_len = usize::try_from(block.interface_width)
            .ok()
            .and_then(|width| width.checked_mul(16));
        let lengths = [
            block.sample_counts.len(),
            block.latest_observed_unix.len(),
            block.rx_bytes_last.len(),
            block.tx_bytes_last.len(),
            block.rx_counter_epoch.len(),
            block.tx_counter_epoch.len(),
        ];
        anyhow::ensure!(
            block.owner_id > 0
                && dashboard_source_tier_is_valid("network", block.source_bucket_secs)
                && block
                    .block_start_unix
                    .rem_euclid(i64::from(block.source_bucket_secs) * 16)
                    == 0
                && expected_len.is_some_and(|length| {
                    length > 0 && lengths.iter().all(|actual| *actual == length)
                })
                && block.has_samples == block.sample_counts.iter().any(|count| *count > 0),
            "prepared network dashboard block is invalid"
        );
        Ok(block)
    }
}

#[derive(Debug)]
struct PreparedTrafficBlock {
    owner_id: i64,
    source_bucket_secs: i32,
    block_start_unix: i64,
    has_samples: bool,
    rx_valid_counts: Vec<Option<i64>>,
    tx_valid_counts: Vec<Option<i64>>,
    rx_bytes: Vec<Option<i64>>,
    tx_bytes: Vec<Option<i64>>,
}

impl PreparedTrafficBlock {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        let block = Self {
            owner_id: row.try_get("owner_id")?,
            source_bucket_secs: row.try_get("source_bucket_secs")?,
            block_start_unix: row.try_get("block_start_unix")?,
            has_samples: row.try_get("has_samples")?,
            rx_valid_counts: row.try_get("rx_valid_counts")?,
            tx_valid_counts: row.try_get("tx_valid_counts")?,
            rx_bytes: row.try_get("rx_bytes")?,
            tx_bytes: row.try_get("tx_bytes")?,
        };
        let lengths = [
            block.rx_valid_counts.len(),
            block.tx_valid_counts.len(),
            block.rx_bytes.len(),
            block.tx_bytes.len(),
        ];
        anyhow::ensure!(
            block.owner_id > 0
                && dashboard_source_tier_is_valid("traffic", block.source_bucket_secs)
                && block
                    .block_start_unix
                    .rem_euclid(i64::from(block.source_bucket_secs) * 16)
                    == 0
                && lengths.iter().all(|length| *length == 16)
                && block.has_samples == block.rx_valid_counts.iter().any(Option::is_some),
            "prepared traffic dashboard block is invalid"
        );
        Ok(block)
    }
}

#[derive(Debug)]
enum PreparedBlocks {
    Resource(Vec<PreparedResourceBlock>),
    Network(Vec<PreparedNetworkBlock>),
    Traffic(Vec<PreparedTrafficBlock>),
}

#[derive(Debug)]
struct PreparedDashboardPublication {
    owner: DashboardOwner,
    claim: DashboardClaim,
    generation_interfaces: Vec<String>,
    generation_source_kinds: Vec<String>,
    blocks: PreparedBlocks,
}

impl PreparedDashboardPublication {
    fn expected_block_keys(&self) -> Vec<(i32, i64)> {
        let mut keys = self
            .claim
            .source_bucket_secs
            .iter()
            .copied()
            .zip(self.claim.block_start_unix.iter().copied())
            .collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn validate(&self) -> Result<()> {
        self.claim.validate()?;
        anyhow::ensure!(
            self.owner.client_id == self.claim.client_id
                && self.owner.domain == self.claim.domain
                && self.claim.change == "block"
                && self
                    .claim
                    .event_kind
                    .iter()
                    .all(|kind| kind == "coordinate"),
            "prepared dashboard publication changed its exact owner"
        );
        anyhow::ensure!(
            self.generation_source_kinds.len() == self.generation_interfaces.len()
                || self.owner.domain != "traffic",
            "prepared traffic dashboard selection is misaligned"
        );
        let expected = self.expected_block_keys();
        let actual = match &self.blocks {
            PreparedBlocks::Resource(blocks) => {
                anyhow::ensure!(self.owner.domain == "resource");
                blocks
                    .iter()
                    .map(|block| (block.source_bucket_secs, block.block_start_unix))
                    .collect::<Vec<_>>()
            }
            PreparedBlocks::Network(blocks) => {
                anyhow::ensure!(self.owner.domain == "network");
                anyhow::ensure!(blocks.iter().all(|block| {
                    usize::try_from(block.interface_width).ok()
                        == Some(self.generation_interfaces.len())
                }));
                blocks
                    .iter()
                    .map(|block| (block.source_bucket_secs, block.block_start_unix))
                    .collect::<Vec<_>>()
            }
            PreparedBlocks::Traffic(blocks) => {
                anyhow::ensure!(self.owner.domain == "traffic");
                blocks
                    .iter()
                    .map(|block| (block.source_bucket_secs, block.block_start_unix))
                    .collect::<Vec<_>>()
            }
        };
        let expected = if self.owner.domain == "network" && self.generation_interfaces.is_empty() {
            Vec::new()
        } else {
            expected
        };
        anyhow::ensure!(
            actual == expected,
            "prepared dashboard blocks do not cover the exact claimed coordinates"
        );
        Ok(())
    }
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

async fn acquire_coordinate_owners(connection: &mut PgConnection) -> Result<Vec<DashboardOwner>> {
    sqlx::query(DASHBOARD_COORDINATE_ACQUIRE_SQL)
        .fetch_all(connection)
        .await?
        .iter()
        .map(DashboardOwner::from_row)
        .collect()
}

fn prepared_claim_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<(DashboardOwner, DashboardClaim, Vec<String>, Vec<String>)> {
    let owner = DashboardOwner::from_row(row)?;
    let claim = DashboardClaim {
        client_id: row.try_get("client_id")?,
        domain: row.try_get("domain")?,
        change: "block".to_string(),
        event_kind: row.try_get("event_kind")?,
        source_bucket_secs: row.try_get("source_bucket_secs")?,
        block_start_unix: row.try_get("block_start_unix")?,
        bucket_start_unix: row.try_get("bucket_start_unix")?,
        captured_block_event_ids: row.try_get("captured_block_event_ids")?,
        captured_generation_event_ids: Vec::new(),
        expected_generation: row.try_get("expected_generation")?,
        expected_revision: row.try_get("expected_revision")?,
    };
    claim.validate()?;
    let generation_interfaces = row.try_get("generation_interfaces")?;
    let generation_source_kinds = row.try_get("generation_source_kinds")?;
    Ok((owner, claim, generation_interfaces, generation_source_kinds))
}

async fn prepare_coordinate_publications(
    connection: &mut PgConnection,
    acquired: &[DashboardOwner],
) -> Result<HashMap<i64, PreparedDashboardPublication>> {
    if acquired.is_empty() {
        return Ok(HashMap::new());
    }
    let owner_ids = acquired
        .iter()
        .map(|owner| owner.owner_id)
        .collect::<Vec<_>>();
    let mut transaction = connection.begin().await?;
    set_repeatable_read(&mut transaction).await?;
    let claim_rows = sqlx::query(DASHBOARD_COORDINATE_CLAIMS_SQL)
        .bind(&owner_ids)
        .fetch_all(&mut *transaction)
        .await?;
    let resource_rows = sqlx::query(DASHBOARD_RESOURCE_PREPARE_SQL)
        .bind(&owner_ids)
        .fetch_all(&mut *transaction)
        .await?;
    let network_rows = sqlx::query(DASHBOARD_NETWORK_PREPARE_SQL)
        .bind(&owner_ids)
        .fetch_all(&mut *transaction)
        .await?;
    let traffic_rows = sqlx::query(DASHBOARD_TRAFFIC_PREPARE_SQL)
        .bind(&owner_ids)
        .fetch_all(&mut *transaction)
        .await?;
    transaction.commit().await?;

    let mut resource_blocks: HashMap<i64, Vec<PreparedResourceBlock>> = HashMap::new();
    for row in &resource_rows {
        let block = PreparedResourceBlock::from_row(row)?;
        resource_blocks
            .entry(block.owner_id)
            .or_default()
            .push(block);
    }
    let mut network_blocks: HashMap<i64, Vec<PreparedNetworkBlock>> = HashMap::new();
    for row in &network_rows {
        let block = PreparedNetworkBlock::from_row(row)?;
        network_blocks
            .entry(block.owner_id)
            .or_default()
            .push(block);
    }
    let mut traffic_blocks: HashMap<i64, Vec<PreparedTrafficBlock>> = HashMap::new();
    for row in &traffic_rows {
        let block = PreparedTrafficBlock::from_row(row)?;
        traffic_blocks
            .entry(block.owner_id)
            .or_default()
            .push(block);
    }

    let acquired_by_id = acquired
        .iter()
        .map(|owner| (owner.owner_id, owner))
        .collect::<HashMap<_, _>>();
    let mut prepared = HashMap::new();
    for row in &claim_rows {
        let (owner, claim, generation_interfaces, generation_source_kinds) =
            prepared_claim_from_row(row)?;
        let acquired_owner = acquired_by_id
            .get(&owner.owner_id)
            .context("prepared an owner outside the acquired coordinate cohort")?;
        anyhow::ensure!(
            acquired_owner.client_id == owner.client_id
                && acquired_owner.domain == owner.domain
                && owner.ready_revision >= acquired_owner.ready_revision,
            "prepared dashboard owner changed its acquired identity"
        );
        let blocks = match owner.domain.as_str() {
            "resource" => PreparedBlocks::Resource(
                resource_blocks.remove(&owner.owner_id).unwrap_or_default(),
            ),
            "network" => {
                PreparedBlocks::Network(network_blocks.remove(&owner.owner_id).unwrap_or_default())
            }
            "traffic" => {
                PreparedBlocks::Traffic(traffic_blocks.remove(&owner.owner_id).unwrap_or_default())
            }
            _ => unreachable!(),
        };
        let publication = PreparedDashboardPublication {
            owner,
            claim,
            generation_interfaces,
            generation_source_kinds,
            blocks,
        };
        publication.validate()?;
        anyhow::ensure!(
            prepared
                .insert(publication.owner.owner_id, publication)
                .is_none(),
            "dashboard coordinate cohort returned a duplicate owner"
        );
    }
    anyhow::ensure!(
        resource_blocks.is_empty() && network_blocks.is_empty() && traffic_blocks.is_empty(),
        "prepared dashboard blocks have no matching exact owner claim"
    );
    Ok(prepared)
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

async fn apply_prepared_blocks(
    transaction: &mut Transaction<'_, Postgres>,
    publication: &PreparedDashboardPublication,
) -> Result<()> {
    let client_id = &publication.claim.client_id;
    let generation = publication.claim.expected_generation;
    let revision = publication.claim.expected_revision + 1;
    match &publication.blocks {
        PreparedBlocks::Resource(blocks) => {
            for block in blocks {
                sqlx::query(DASHBOARD_RESOURCE_BLOCK_APPLY_SQL)
                    .bind(client_id)
                    .bind(generation)
                    .bind(revision)
                    .bind(block.source_bucket_secs)
                    .bind(block.block_start_unix)
                    .bind(block.has_samples)
                    .bind(&block.sample_counts)
                    .bind(&block.cpu_load_1_sums)
                    .bind(&block.cpu_load_1_maxes)
                    .bind(&block.memory_total_bytes_maxes)
                    .bind(&block.memory_used_ratio_sums)
                    .bind(&block.memory_used_ratio_maxes)
                    .bind(&block.disk_sample_counts)
                    .bind(&block.disk_total_bytes_maxes)
                    .bind(&block.disk_used_ratio_sums)
                    .bind(&block.disk_used_ratio_maxes)
                    .bind(&block.latest_observed_unix)
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        PreparedBlocks::Network(blocks) => {
            for block in blocks {
                sqlx::query(DASHBOARD_NETWORK_BLOCK_APPLY_SQL)
                    .bind(client_id)
                    .bind(generation)
                    .bind(revision)
                    .bind(block.source_bucket_secs)
                    .bind(block.block_start_unix)
                    .bind(block.interface_width)
                    .bind(block.has_samples)
                    .bind(&block.sample_counts)
                    .bind(&block.latest_observed_unix)
                    .bind(&block.rx_bytes_last)
                    .bind(&block.tx_bytes_last)
                    .bind(&block.rx_counter_epoch)
                    .bind(&block.tx_counter_epoch)
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        PreparedBlocks::Traffic(blocks) => {
            for block in blocks {
                sqlx::query(DASHBOARD_TRAFFIC_BLOCK_APPLY_SQL)
                    .bind(client_id)
                    .bind(generation)
                    .bind(revision)
                    .bind(block.source_bucket_secs)
                    .bind(block.block_start_unix)
                    .bind(block.has_samples)
                    .bind(&block.rx_valid_counts)
                    .bind(&block.tx_valid_counts)
                    .bind(&block.rx_bytes)
                    .bind(&block.tx_bytes)
                    .execute(&mut **transaction)
                    .await?;
            }
        }
    }
    Ok(())
}

async fn publish_prepared_owned(
    connection: &mut PgConnection,
    publication: PreparedDashboardPublication,
) -> Result<DashboardPublishOutcome> {
    let mut transaction = connection.begin().await?;
    // Consume the exact ready revision before installing a prepared block. A
    // producer that already committed makes this a no-op; a producer arriving
    // later waits here and recreates the ready owner after this commit.
    let began = sqlx::query_scalar::<_, bool>(DASHBOARD_PREPARED_BEGIN_SQL)
        .bind(publication.owner.owner_id)
        .bind(publication.owner.ready_revision)
        .fetch_optional(&mut *transaction)
        .await?;
    if began.is_none() {
        transaction.rollback().await?;
        return Ok(DashboardPublishOutcome::Contended);
    }

    sqlx::query("SET LOCAL synchronous_commit = OFF")
        .execute(&mut *transaction)
        .await?;
    let applied = apply_prepared_blocks(&mut transaction, &publication).await;
    if let Err(error) = applied {
        transaction.rollback().await?;
        return Ok(DashboardPublishOutcome::Failed {
            claim: publication.claim,
            error: error.context("prepared dashboard blocks failed before commit"),
        });
    }

    let completed = sqlx::query_scalar::<_, bool>(DASHBOARD_PREPARED_COMPLETE_SQL)
        .bind(&publication.claim.client_id)
        .bind(&publication.claim.domain)
        .bind(&publication.claim.event_kind)
        .bind(&publication.claim.source_bucket_secs)
        .bind(&publication.claim.block_start_unix)
        .bind(&publication.claim.bucket_start_unix)
        .bind(&publication.claim.captured_block_event_ids)
        .bind(publication.claim.expected_generation)
        .bind(publication.claim.expected_revision)
        .fetch_one(&mut *transaction)
        .await;
    match completed {
        Ok(true) => {
            transaction.commit().await?;
            Ok(DashboardPublishOutcome::Published(publication.claim))
        }
        Ok(false) => {
            transaction.rollback().await?;
            Ok(DashboardPublishOutcome::Failed {
                claim: publication.claim,
                error: anyhow::anyhow!("prepared dashboard publication was not completed"),
            })
        }
        Err(error) => {
            transaction.rollback().await?;
            Ok(DashboardPublishOutcome::Failed {
                claim: publication.claim,
                error: anyhow::Error::from(error)
                    .context("prepared dashboard publication failed before commit"),
            })
        }
    }
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

#[derive(Debug)]
struct CoordinateCohortOutcome {
    acquired: bool,
    publications: Vec<DashboardPublishOutcome>,
}

async fn publish_coordinate_cohort(
    connection: &mut PgConnection,
) -> Result<CoordinateCohortOutcome> {
    let acquired = acquire_coordinate_owners(connection).await?;
    if acquired.is_empty() {
        return Ok(CoordinateCohortOutcome {
            acquired: false,
            publications: Vec::new(),
        });
    }
    let mut prepared = prepare_coordinate_publications(connection, &acquired).await?;
    let mut publications = Vec::with_capacity(prepared.len());
    for acquired_owner in acquired {
        let Some(publication) = prepared.remove(&acquired_owner.owner_id) else {
            // Work that changed to a generation/full-block shape after
            // acquisition is intentionally left for the established path.
            release_owner(connection, acquired_owner.owner_id).await?;
            continue;
        };
        let exact_owner = publication.owner.clone();
        let result = publish_prepared_owned(connection, publication).await;
        let deferral = if matches!(&result, Ok(DashboardPublishOutcome::Failed { .. })) {
            defer_failed_owner(connection, &exact_owner).await
        } else {
            Ok(())
        };
        let release = release_owner(connection, exact_owner.owner_id).await;
        if let Err(release_error) = release {
            return match result {
                Err(publication_error) => Err(release_error).with_context(|| {
                    format!(
                        "failed to release prepared dashboard owner after publication error: {publication_error:#}"
                    )
                }),
                Ok(_) => Err(release_error)
                    .context("failed to release prepared dashboard owner; connection must close"),
            };
        }
        deferral.context("failed to defer failed prepared dashboard publication owner")?;
        publications.push(result?);
    }
    anyhow::ensure!(
        prepared.is_empty(),
        "prepared dashboard publication escaped its acquired owner cohort"
    );
    Ok(CoordinateCohortOutcome {
        acquired: true,
        publications,
    })
}

#[cfg(test)]
pub(crate) async fn publish_coordinate_cohort_for_test(pool: &PgPool) -> Result<usize> {
    let mut connection = pool.acquire().await?;
    let outcome = publish_coordinate_cohort(&mut connection).await?;
    Ok(outcome.publications.len())
}

async fn run_connection(
    connection: &mut PgConnection,
    shutdown: &mut watch::Receiver<bool>,
) -> Result<()> {
    loop {
        if shutdown_requested(shutdown) {
            return Ok(());
        }
        let coordinate = publish_coordinate_cohort(connection).await?;
        for publication in coordinate.publications {
            match publication {
                DashboardPublishOutcome::Published(claim) => {
                    debug!(client_id = %claim.client_id, domain = %claim.domain,
                        change = %claim.change, "published prepared dashboard telemetry mutation");
                }
                DashboardPublishOutcome::Contended => {}
                DashboardPublishOutcome::Failed { claim, error } => {
                    warn!(%error, client_id = %claim.client_id, domain = %claim.domain,
                        change = %claim.change, "failed prepared dashboard mutation remains queued");
                }
                DashboardPublishOutcome::Idle => {
                    anyhow::bail!("prepared dashboard publication returned an idle outcome")
                }
            }
        }
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
                if coordinate.acquired {
                    tokio::task::yield_now().await;
                } else if wait_or_shutdown(shutdown, DASHBOARD_IDLE_POLL_INTERVAL).await {
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
