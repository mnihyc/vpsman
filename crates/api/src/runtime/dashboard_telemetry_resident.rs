use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::Deserialize;
use sqlx::{
    postgres::{PgConnectOptions, PgListener, PgPoolOptions, PgRow},
    FromRow, Row,
};
use tokio::{
    sync::{watch, Notify},
    task::JoinHandle,
    time,
};
use tracing::{info, warn};

use crate::{
    model::{TelemetryNetworkRateView, TelemetryRollupView},
    model_alert_policies::NetworkRateInterfaceSelection,
    repository::Repository,
    repository_telemetry_rollups::{
        DashboardTelemetryNetworkProjection, DashboardTelemetryResourceProjection,
        DashboardTelemetryTrafficPoint, DashboardTelemetryTrafficProjection,
    },
    state::{WsEventBus, FLEET_TELEMETRY_INVALIDATION_WINDOW},
};

pub(crate) const TELEMETRY_PROJECTION_CHANNEL: &str = "vpsman_telemetry_projection";
const BLOCK_SLOTS: usize = 16;
const RECONNECT_MIN_DELAY: Duration = Duration::from_millis(250);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(15);
const DASHBOARD_RESIDENT_LISTENER_APPLICATION_NAME: &str = "vpsman-dashboard-resident";
const DASHBOARD_RESIDENT_RECONCILER_APPLICATION_NAME: &str = "vpsman-dashboard-resident-reconciler";

const HEADS_SQL: &str = r#"
SELECT resource.client_id,
       resource.resource_generation, resource.resource_revision,
       resource.resource_change,
       resource.resource_change_source_bucket_secs,
       resource.resource_change_block_start_unix,
       floor(extract(epoch FROM resource.resource_first_at))::BIGINT
           AS resource_first_unix,
       floor(extract(epoch FROM resource.resource_through_at))::BIGINT
           AS resource_through_unix,
       network.network_generation, network.network_revision,
       network.network_change,
       network.network_change_source_bucket_secs,
       network.network_change_block_start_unix,
       network.network_generation_interfaces,
       floor(extract(epoch FROM network.network_first_at))::BIGINT
           AS network_first_unix,
       floor(extract(epoch FROM network.network_through_at))::BIGINT
           AS network_through_unix,
       traffic.traffic_generation, traffic.traffic_revision,
       traffic.traffic_change,
       traffic.traffic_change_source_bucket_secs,
       traffic.traffic_change_block_start_unix,
       traffic.traffic_generation_source_kinds,
       traffic.traffic_generation_interfaces,
       traffic.traffic_stream_width,
       floor(extract(epoch FROM traffic.traffic_first_at))::BIGINT
           AS traffic_first_unix,
       floor(extract(epoch FROM traffic.traffic_through_at))::BIGINT
           AS traffic_through_unix
FROM telemetry_dashboard_resource_projection_heads resource
JOIN telemetry_dashboard_network_projection_heads network USING (client_id)
JOIN telemetry_dashboard_traffic_projection_heads traffic USING (client_id)
ORDER BY resource.client_id
"#;

const RESOURCE_BLOCKS_SQL: &str = r#"
SELECT source_bucket_secs, block_start_unix, published_revision,
       sample_counts, cpu_load_1_sums,
       cpu_load_1_maxes::DOUBLE PRECISION[] AS cpu_load_1_maxes,
       memory_total_bytes_maxes, memory_used_ratio_sums,
       memory_used_ratio_maxes::DOUBLE PRECISION[] AS memory_used_ratio_maxes,
       disk_sample_counts, disk_total_bytes_maxes, disk_used_ratio_sums,
       disk_used_ratio_maxes::DOUBLE PRECISION[] AS disk_used_ratio_maxes,
       latest_observed_unix
FROM telemetry_dashboard_resource_blocks
WHERE client_id = $1 AND generation = $2 AND published_revision <= $3
ORDER BY source_bucket_secs, block_start_unix
"#;

const RESOURCE_OVERLAY_SQL: &str = r#"
SELECT bucket_secs AS source_bucket_secs,
       telemetry_dashboard_block_start(
           extract(epoch FROM bucket_start)::BIGINT, bucket_secs
       ) AS block_start_unix,
       extract(epoch FROM bucket_start)::BIGINT AS bucket_start_unix,
       sample_count::BIGINT AS sample_count, cpu_load_1_sum,
       cpu_load_1_max::DOUBLE PRECISION AS cpu_load_1_max,
       memory_total_bytes_max, memory_used_ratio_sum,
       memory_used_ratio_max::DOUBLE PRECISION AS memory_used_ratio_max,
       disk_sample_count::BIGINT AS disk_sample_count,
       disk_total_bytes_max, disk_used_ratio_sum,
       disk_used_ratio_max::DOUBLE PRECISION AS disk_used_ratio_max,
       extract(epoch FROM latest_observed_at)::BIGINT
           AS latest_observed_unix
FROM telemetry_dashboard_resource_overlay_source(
    ARRAY[$1::TEXT], $2::INTEGER[], $3::BIGINT[]
)
ORDER BY source_bucket_secs, block_start_unix, bucket_start_unix
"#;

const NETWORK_BLOCKS_SQL: &str = r#"
SELECT source_bucket_secs, block_start_unix, published_revision,
       sample_counts, latest_observed_unix, rx_bytes_last, tx_bytes_last,
       rx_counter_epoch, tx_counter_epoch
FROM telemetry_dashboard_network_blocks
WHERE client_id = $1 AND generation = $2 AND published_revision <= $3
ORDER BY source_bucket_secs, block_start_unix
"#;

const TRAFFIC_BLOCKS_SQL: &str = r#"
SELECT source_bucket_secs, block_start_unix, published_revision,
       rx_valid_counts, tx_valid_counts, rx_bytes, tx_bytes
FROM telemetry_dashboard_traffic_blocks
WHERE client_id = $1 AND generation = $2 AND published_revision <= $3
ORDER BY source_bucket_secs, block_start_unix
"#;

const TRAFFIC_OVERLAY_SQL: &str = r#"
SELECT bucket_secs AS source_bucket_secs,
       telemetry_dashboard_block_start(
           extract(epoch FROM bucket_start)::BIGINT, bucket_secs
       ) AS block_start_unix,
       extract(epoch FROM bucket_start)::BIGINT AS bucket_start_unix,
       rx_valid_count::BIGINT, tx_valid_count::BIGINT,
       rx_bytes, tx_bytes
FROM telemetry_dashboard_traffic_overlay_source(
    ARRAY[$1::TEXT], $2::INTEGER[], $3::BIGINT[]
)
ORDER BY source_bucket_secs, block_start_unix, bucket_start_unix
"#;

const NETWORK_OVERLAY_SQL: &str = r#"
WITH source AS (
    SELECT overlay.*
    FROM telemetry_dashboard_network_overlay_source(
        ARRAY[$1::TEXT], $2::INTEGER[], $3::BIGINT[]
    ) overlay
), buckets AS (
    SELECT DISTINCT source.bucket_secs, source.bucket_start
    FROM source
    WHERE source.interface = ANY($4::TEXT[])
)
SELECT bucket.bucket_secs AS source_bucket_secs,
       telemetry_dashboard_block_start(
           extract(epoch FROM bucket.bucket_start)::BIGINT,
           bucket.bucket_secs
       ) AS block_start_unix,
       extract(epoch FROM bucket.bucket_start)::BIGINT AS bucket_start_unix,
       array_agg(
           COALESCE(source.sample_count, 0)::BIGINT
           ORDER BY interface.ordinal
       ) AS sample_counts,
       array_agg(
           extract(epoch FROM source.latest_observed_at)::BIGINT
           ORDER BY interface.ordinal
       ) AS latest_observed_unix,
       array_agg(source.rx_bytes_last ORDER BY interface.ordinal)
           AS rx_bytes_last,
       array_agg(source.tx_bytes_last ORDER BY interface.ordinal)
           AS tx_bytes_last,
       array_agg(source.rx_counter_epoch ORDER BY interface.ordinal)
           AS rx_counter_epoch,
       array_agg(source.tx_counter_epoch ORDER BY interface.ordinal)
           AS tx_counter_epoch
FROM buckets bucket
CROSS JOIN unnest($4::TEXT[]) WITH ORDINALITY interface(name, ordinal)
LEFT JOIN source
  ON source.interface = interface.name
 AND source.bucket_secs = bucket.bucket_secs
 AND source.bucket_start = bucket.bucket_start
GROUP BY bucket.bucket_secs, bucket.bucket_start
ORDER BY source_bucket_secs, block_start_unix, bucket_start_unix
"#;

// Startup owns the whole resident fleet, so read each retained projection
// relation once instead of repeating one client-key heap walk per owner. The
// generation predicate remains exact; IS NOT DISTINCT FROM gives PostgreSQL
// the correct fleet-wide plan for this correlation between the small head set
// and retained block relations (both operands are schema-NOT-NULL).
const SEED_RESOURCE_BLOCKS_SQL: &str = r#"
SELECT block.client_id AS seed_client_id,
       block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.sample_counts,
       block.cpu_load_1_sums,
       block.cpu_load_1_maxes::DOUBLE PRECISION[] AS cpu_load_1_maxes,
       block.memory_total_bytes_maxes, block.memory_used_ratio_sums,
       block.memory_used_ratio_maxes::DOUBLE PRECISION[]
           AS memory_used_ratio_maxes,
       block.disk_sample_counts, block.disk_total_bytes_maxes,
       block.disk_used_ratio_sums,
       block.disk_used_ratio_maxes::DOUBLE PRECISION[]
           AS disk_used_ratio_maxes,
       block.latest_observed_unix
FROM telemetry_dashboard_resource_blocks block
JOIN telemetry_dashboard_resource_projection_heads head
  ON head.client_id = block.client_id
 AND block.generation IS NOT DISTINCT FROM head.resource_generation
WHERE block.published_revision <= head.resource_revision
"#;

const SEED_NETWORK_BLOCKS_SQL: &str = r#"
SELECT block.client_id AS seed_client_id,
       block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.sample_counts,
       block.latest_observed_unix, block.rx_bytes_last,
       block.tx_bytes_last, block.rx_counter_epoch, block.tx_counter_epoch
FROM telemetry_dashboard_network_blocks block
JOIN telemetry_dashboard_network_projection_heads head
  ON head.client_id = block.client_id
 AND block.generation IS NOT DISTINCT FROM head.network_generation
WHERE block.published_revision <= head.network_revision
"#;

const SEED_TRAFFIC_BLOCKS_SQL: &str = r#"
SELECT block.client_id AS seed_client_id,
       block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.rx_valid_counts,
       block.tx_valid_counts, block.rx_bytes, block.tx_bytes
FROM telemetry_dashboard_traffic_blocks block
JOIN telemetry_dashboard_traffic_projection_heads head
  ON head.client_id = block.client_id
 AND block.generation IS NOT DISTINCT FROM head.traffic_generation
WHERE block.published_revision <= head.traffic_revision
"#;

// Incremental notices name exact F16 block coordinates. Drive those reads
// from the small coordinate set so PostgreSQL can probe the composite primary
// key instead of materializing and sorting the owner's complete generation.
const RESOURCE_COORDINATE_BLOCKS_SQL: &str = r#"
SELECT block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.sample_counts,
       block.cpu_load_1_sums,
       block.cpu_load_1_maxes::DOUBLE PRECISION[] AS cpu_load_1_maxes,
       block.memory_total_bytes_maxes, block.memory_used_ratio_sums,
       block.memory_used_ratio_maxes::DOUBLE PRECISION[]
           AS memory_used_ratio_maxes,
       block.disk_sample_counts, block.disk_total_bytes_maxes,
       block.disk_used_ratio_sums,
       block.disk_used_ratio_maxes::DOUBLE PRECISION[]
           AS disk_used_ratio_maxes,
       block.latest_observed_unix
FROM UNNEST(
    $4::INTEGER[], $5::BIGINT[]
) AS coordinate(source_bucket_secs, block_start_unix)
JOIN telemetry_dashboard_resource_blocks block
  ON block.client_id = $1
 AND block.generation = $2
 AND block.source_bucket_secs = coordinate.source_bucket_secs
 AND block.block_start_unix = coordinate.block_start_unix
WHERE block.published_revision <= $3
ORDER BY block.source_bucket_secs, block.block_start_unix
"#;

const NETWORK_COORDINATE_BLOCKS_SQL: &str = r#"
SELECT block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.sample_counts,
       block.latest_observed_unix, block.rx_bytes_last, block.tx_bytes_last,
       block.rx_counter_epoch, block.tx_counter_epoch
FROM UNNEST(
    $4::INTEGER[], $5::BIGINT[]
) AS coordinate(source_bucket_secs, block_start_unix)
JOIN telemetry_dashboard_network_blocks block
  ON block.client_id = $1
 AND block.generation = $2
 AND block.source_bucket_secs = coordinate.source_bucket_secs
 AND block.block_start_unix = coordinate.block_start_unix
WHERE block.published_revision <= $3
ORDER BY block.source_bucket_secs, block.block_start_unix
"#;

const TRAFFIC_COORDINATE_BLOCKS_SQL: &str = r#"
SELECT block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.rx_valid_counts,
       block.tx_valid_counts, block.rx_bytes, block.tx_bytes
FROM UNNEST(
    $4::INTEGER[], $5::BIGINT[]
) AS coordinate(source_bucket_secs, block_start_unix)
JOIN telemetry_dashboard_traffic_blocks block
  ON block.client_id = $1
 AND block.generation = $2
 AND block.source_bucket_secs = coordinate.source_bucket_secs
 AND block.block_start_unix = coordinate.block_start_unix
WHERE block.published_revision <= $3
ORDER BY block.source_bucket_secs, block.block_start_unix
"#;

const OVERLAY_RESOURCE_BLOCKS_SQL: &str = r#"
SELECT coordinate.client_id AS overlay_client_id,
       block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.sample_counts,
       block.cpu_load_1_sums,
       block.cpu_load_1_maxes::DOUBLE PRECISION[] AS cpu_load_1_maxes,
       block.memory_total_bytes_maxes, block.memory_used_ratio_sums,
       block.memory_used_ratio_maxes::DOUBLE PRECISION[]
           AS memory_used_ratio_maxes,
       block.disk_sample_counts, block.disk_total_bytes_maxes,
       block.disk_used_ratio_sums,
       block.disk_used_ratio_maxes::DOUBLE PRECISION[]
           AS disk_used_ratio_maxes,
       block.latest_observed_unix
FROM UNNEST(
    $1::TEXT[], $2::BIGINT[], $3::BIGINT[], $4::INTEGER[], $5::BIGINT[]
) AS coordinate(
    client_id, generation, revision, source_bucket_secs, block_start_unix
)
JOIN telemetry_dashboard_resource_blocks block
  ON block.client_id = coordinate.client_id
 AND block.generation = coordinate.generation
 AND block.source_bucket_secs = coordinate.source_bucket_secs
 AND block.block_start_unix = coordinate.block_start_unix
 AND block.published_revision <= coordinate.revision
"#;

const OVERLAY_RESOURCE_SOURCE_SQL: &str = r#"
SELECT overlay.client_id AS overlay_client_id,
       overlay.bucket_secs AS source_bucket_secs,
       telemetry_dashboard_block_start(
           extract(epoch FROM overlay.bucket_start)::BIGINT,
           overlay.bucket_secs
       ) AS block_start_unix,
       extract(epoch FROM overlay.bucket_start)::BIGINT
           AS bucket_start_unix,
       overlay.sample_count::BIGINT AS sample_count,
       overlay.cpu_load_1_sum,
       overlay.cpu_load_1_max::DOUBLE PRECISION AS cpu_load_1_max,
       overlay.memory_total_bytes_max, overlay.memory_used_ratio_sum,
       overlay.memory_used_ratio_max::DOUBLE PRECISION
           AS memory_used_ratio_max,
       overlay.disk_sample_count::BIGINT AS disk_sample_count,
       overlay.disk_total_bytes_max, overlay.disk_used_ratio_sum,
       overlay.disk_used_ratio_max::DOUBLE PRECISION
           AS disk_used_ratio_max,
       extract(epoch FROM overlay.latest_observed_at)::BIGINT
           AS latest_observed_unix
FROM telemetry_dashboard_resource_overlay_source(
    $1::TEXT[], NULL::INTEGER[], NULL::BIGINT[]
) overlay
ORDER BY overlay.client_id, source_bucket_secs,
         block_start_unix, bucket_start_unix
"#;

const OVERLAY_NETWORK_BLOCKS_SQL: &str = r#"
SELECT coordinate.client_id AS overlay_client_id,
       block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.sample_counts,
       block.latest_observed_unix, block.rx_bytes_last,
       block.tx_bytes_last, block.rx_counter_epoch, block.tx_counter_epoch
FROM UNNEST(
    $1::TEXT[], $2::BIGINT[], $3::BIGINT[], $4::INTEGER[], $5::BIGINT[]
) AS coordinate(
    client_id, generation, revision, source_bucket_secs, block_start_unix
)
JOIN telemetry_dashboard_network_blocks block
  ON block.client_id = coordinate.client_id
 AND block.generation = coordinate.generation
 AND block.source_bucket_secs = coordinate.source_bucket_secs
 AND block.block_start_unix = coordinate.block_start_unix
 AND block.published_revision <= coordinate.revision
"#;

const OVERLAY_NETWORK_SOURCE_SQL: &str = r#"
SELECT overlay.client_id, overlay.interface,
       overlay.bucket_secs AS source_bucket_secs,
       telemetry_dashboard_block_start(
           extract(epoch FROM overlay.bucket_start)::BIGINT,
           overlay.bucket_secs
       ) AS block_start_unix,
       extract(epoch FROM overlay.bucket_start)::BIGINT AS bucket_start_unix,
       overlay.sample_count::BIGINT AS sample_count,
       extract(epoch FROM overlay.latest_observed_at)::BIGINT
           AS latest_observed_unix,
       overlay.rx_bytes_last, overlay.tx_bytes_last,
       overlay.rx_counter_epoch, overlay.tx_counter_epoch
FROM telemetry_dashboard_network_overlay_source(
    $1::TEXT[], NULL::INTEGER[], NULL::BIGINT[]
) overlay
ORDER BY client_id, source_bucket_secs, bucket_start_unix, interface
"#;

const OVERLAY_TRAFFIC_BLOCKS_SQL: &str = r#"
SELECT coordinate.client_id AS overlay_client_id,
       block.source_bucket_secs, block.block_start_unix,
       block.published_revision, block.rx_valid_counts,
       block.tx_valid_counts, block.rx_bytes, block.tx_bytes
FROM UNNEST(
    $1::TEXT[], $2::BIGINT[], $3::BIGINT[], $4::INTEGER[], $5::BIGINT[]
) AS coordinate(
    client_id, generation, revision, source_bucket_secs, block_start_unix
)
JOIN telemetry_dashboard_traffic_blocks block
  ON block.client_id = coordinate.client_id
 AND block.generation = coordinate.generation
 AND block.source_bucket_secs = coordinate.source_bucket_secs
 AND block.block_start_unix = coordinate.block_start_unix
 AND block.published_revision <= coordinate.revision
"#;

const OVERLAY_TRAFFIC_SOURCE_SQL: &str = r#"
SELECT overlay.client_id,
       overlay.bucket_secs AS source_bucket_secs,
       telemetry_dashboard_block_start(
           extract(epoch FROM overlay.bucket_start)::BIGINT,
           overlay.bucket_secs
       ) AS block_start_unix,
       extract(epoch FROM overlay.bucket_start)::BIGINT AS bucket_start_unix,
       overlay.rx_valid_count, overlay.tx_valid_count,
       overlay.rx_bytes, overlay.tx_bytes
FROM telemetry_dashboard_traffic_overlay_source(
    $1::TEXT[], NULL::INTEGER[], NULL::BIGINT[]
) overlay
ORDER BY overlay.client_id, source_bucket_secs, bucket_start_unix
"#;

// A ready dashboard-notice cohort names exact (client, F16 block) owners.
// Keep the client coordinate paired through the setwise overlay read: the
// underlying source accepts a shared coordinate relation, while this join
// prevents one client's requested block from admitting another client's row.
const NOTICE_RESOURCE_OVERLAY_SQL: &str = r#"
WITH requested AS MATERIALIZED (
    SELECT DISTINCT coordinate.client_id,
           coordinate.source_bucket_secs,
           coordinate.block_start_unix
    FROM unnest($1::TEXT[], $2::INTEGER[], $3::BIGINT[])
        coordinate(client_id, source_bucket_secs, block_start_unix)
), source AS MATERIALIZED (
    SELECT overlay.client_id AS overlay_client_id,
           overlay.bucket_secs AS source_bucket_secs,
           telemetry_dashboard_block_start(
               extract(epoch FROM overlay.bucket_start)::BIGINT,
               overlay.bucket_secs
           ) AS block_start_unix,
           extract(epoch FROM overlay.bucket_start)::BIGINT
               AS bucket_start_unix,
           overlay.sample_count::BIGINT AS sample_count,
           overlay.cpu_load_1_sum,
           overlay.cpu_load_1_max::DOUBLE PRECISION AS cpu_load_1_max,
           overlay.memory_total_bytes_max,
           overlay.memory_used_ratio_sum,
           overlay.memory_used_ratio_max::DOUBLE PRECISION
               AS memory_used_ratio_max,
           overlay.disk_sample_count::BIGINT AS disk_sample_count,
           overlay.disk_total_bytes_max,
           overlay.disk_used_ratio_sum,
           overlay.disk_used_ratio_max::DOUBLE PRECISION
               AS disk_used_ratio_max,
           extract(epoch FROM overlay.latest_observed_at)::BIGINT
               AS latest_observed_unix
    FROM telemetry_dashboard_resource_overlay_source($1, $2, $3) overlay
)
SELECT source.*
FROM source
JOIN requested
  ON requested.client_id = source.overlay_client_id
 AND requested.source_bucket_secs = source.source_bucket_secs
 AND requested.block_start_unix = source.block_start_unix
ORDER BY overlay_client_id, source_bucket_secs,
         block_start_unix, bucket_start_unix
"#;

const NOTICE_NETWORK_OVERLAY_SQL: &str = r#"
WITH requested AS MATERIALIZED (
    SELECT DISTINCT coordinate.client_id,
           coordinate.source_bucket_secs,
           coordinate.block_start_unix
    FROM unnest($1::TEXT[], $2::INTEGER[], $3::BIGINT[])
        coordinate(client_id, source_bucket_secs, block_start_unix)
), selected_interfaces AS MATERIALIZED (
    SELECT DISTINCT selected.client_id, selected.interface
    FROM unnest($4::TEXT[], $5::TEXT[])
        selected(client_id, interface)
), source AS MATERIALIZED (
    SELECT overlay.client_id, overlay.interface,
           overlay.bucket_secs AS source_bucket_secs,
           telemetry_dashboard_block_start(
               extract(epoch FROM overlay.bucket_start)::BIGINT,
               overlay.bucket_secs
           ) AS block_start_unix,
           extract(epoch FROM overlay.bucket_start)::BIGINT
               AS bucket_start_unix,
           overlay.sample_count::BIGINT AS sample_count,
           extract(epoch FROM overlay.latest_observed_at)::BIGINT
               AS latest_observed_unix,
           overlay.rx_bytes_last, overlay.tx_bytes_last,
           overlay.rx_counter_epoch, overlay.tx_counter_epoch
    FROM telemetry_dashboard_network_overlay_source($1, $2, $3) overlay
)
SELECT source.*
FROM source
JOIN requested USING (client_id, source_bucket_secs, block_start_unix)
JOIN selected_interfaces USING (client_id, interface)
ORDER BY client_id, source_bucket_secs,
         block_start_unix, bucket_start_unix, interface
"#;

const NOTICE_TRAFFIC_OVERLAY_SQL: &str = r#"
WITH requested AS MATERIALIZED (
    SELECT DISTINCT coordinate.client_id,
           coordinate.source_bucket_secs,
           coordinate.block_start_unix
    FROM unnest($1::TEXT[], $2::INTEGER[], $3::BIGINT[])
        coordinate(client_id, source_bucket_secs, block_start_unix)
), source AS MATERIALIZED (
    SELECT overlay.client_id AS overlay_client_id,
           overlay.bucket_secs AS source_bucket_secs,
           telemetry_dashboard_block_start(
               extract(epoch FROM overlay.bucket_start)::BIGINT,
               overlay.bucket_secs
           ) AS block_start_unix,
           extract(epoch FROM overlay.bucket_start)::BIGINT
               AS bucket_start_unix,
           overlay.rx_valid_count::BIGINT, overlay.tx_valid_count::BIGINT,
           overlay.rx_bytes, overlay.tx_bytes
    FROM telemetry_dashboard_traffic_overlay_source($1, $2, $3) overlay
)
SELECT source.*
FROM source
JOIN requested
  ON requested.client_id = source.overlay_client_id
 AND requested.source_bucket_secs = source.source_bucket_secs
 AND requested.block_start_unix = source.block_start_unix
ORDER BY overlay_client_id, source_bucket_secs,
         block_start_unix, bucket_start_unix
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
struct BlockKey {
    source_bucket_secs: i32,
    block_start_unix: i64,
}

fn canonical_block_keys(
    tiers: Vec<i32>,
    starts: Vec<i64>,
    change: &str,
) -> Result<Arc<[BlockKey]>> {
    anyhow::ensure!(
        tiers.len() == starts.len(),
        "dashboard head block descriptor is misaligned"
    );
    let mut prior = None;
    let mut blocks = Vec::with_capacity(tiers.len());
    for (source_bucket_secs, block_start_unix) in tiers.into_iter().zip(starts) {
        anyhow::ensure!(
            valid_tier(source_bucket_secs)
                && block_start_unix.rem_euclid(i64::from(source_bucket_secs) * BLOCK_SLOTS as i64)
                    == 0
                && prior.is_none_or(|value| value < (source_bucket_secs, block_start_unix)),
            "dashboard head block descriptor is not canonical"
        );
        prior = Some((source_bucket_secs, block_start_unix));
        blocks.push(BlockKey {
            source_bucket_secs,
            block_start_unix,
        });
    }
    anyhow::ensure!(
        (change == "generation" && blocks.is_empty()) || (change == "block" && !blocks.is_empty()),
        "dashboard head change and block descriptor disagree"
    );
    Ok(blocks.into())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ResourceHead {
    generation: i64,
    revision: i64,
    change: String,
    blocks: Arc<[BlockKey]>,
    first_unix: Option<i64>,
    through_unix: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NetworkHead {
    generation: i64,
    revision: i64,
    change: String,
    blocks: Arc<[BlockKey]>,
    interfaces: Arc<[String]>,
    first_unix: Option<i64>,
    through_unix: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrafficHead {
    generation: i64,
    revision: i64,
    change: String,
    blocks: Arc<[BlockKey]>,
    source_kinds: Arc<[String]>,
    interfaces: Arc<[String]>,
    first_unix: Option<i64>,
    through_unix: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClientHeads {
    resource: ResourceHead,
    network: NetworkHead,
    traffic: TrafficHead,
}

fn parse_head(row: &PgRow) -> Result<(String, ClientHeads)> {
    let client_id: String = row.try_get("client_id")?;
    let resource_change: String = row.try_get("resource_change")?;
    let resource_blocks = canonical_block_keys(
        row.try_get("resource_change_source_bucket_secs")?,
        row.try_get("resource_change_block_start_unix")?,
        &resource_change,
    )?;
    let resource = ResourceHead {
        generation: row.try_get("resource_generation")?,
        revision: row.try_get("resource_revision")?,
        change: resource_change,
        blocks: resource_blocks,
        first_unix: row.try_get("resource_first_unix")?,
        through_unix: row.try_get("resource_through_unix")?,
    };
    let interfaces: Vec<String> = row.try_get("network_generation_interfaces")?;
    let network_change: String = row.try_get("network_change")?;
    let network_blocks = canonical_block_keys(
        row.try_get("network_change_source_bucket_secs")?,
        row.try_get("network_change_block_start_unix")?,
        &network_change,
    )?;
    let network = NetworkHead {
        generation: row.try_get("network_generation")?,
        revision: row.try_get("network_revision")?,
        change: network_change,
        blocks: network_blocks,
        interfaces: interfaces.into(),
        first_unix: row.try_get("network_first_unix")?,
        through_unix: row.try_get("network_through_unix")?,
    };
    let source_kinds: Vec<String> = row.try_get("traffic_generation_source_kinds")?;
    let traffic_interfaces: Vec<String> = row.try_get("traffic_generation_interfaces")?;
    let traffic_width: i32 = row.try_get("traffic_stream_width")?;
    let traffic_change: String = row.try_get("traffic_change")?;
    let traffic_blocks = canonical_block_keys(
        row.try_get("traffic_change_source_bucket_secs")?,
        row.try_get("traffic_change_block_start_unix")?,
        &traffic_change,
    )?;
    let traffic = TrafficHead {
        generation: row.try_get("traffic_generation")?,
        revision: row.try_get("traffic_revision")?,
        change: traffic_change,
        blocks: traffic_blocks,
        source_kinds: source_kinds.into(),
        interfaces: traffic_interfaces.into(),
        first_unix: row.try_get("traffic_first_unix")?,
        through_unix: row.try_get("traffic_through_unix")?,
    };
    anyhow::ensure!(
        !client_id.is_empty(),
        "dashboard resident head has an empty client id"
    );
    anyhow::ensure!(
        resource.generation > 0 && resource.revision >= 0,
        "dashboard resident resource head is invalid"
    );
    anyhow::ensure!(
        network.generation > 0 && network.revision >= 0,
        "dashboard resident network head is invalid"
    );
    anyhow::ensure!(
        traffic.generation > 0 && traffic.revision >= 0,
        "dashboard resident traffic head is invalid"
    );
    anyhow::ensure!(
        resource.first_unix.is_some() == resource.through_unix.is_some(),
        "dashboard resident resource bounds are one-sided"
    );
    anyhow::ensure!(
        network.first_unix.is_some() == network.through_unix.is_some(),
        "dashboard resident network bounds are one-sided"
    );
    anyhow::ensure!(
        traffic.first_unix.is_some() == traffic.through_unix.is_some(),
        "dashboard resident traffic bounds are one-sided"
    );
    if let (Some(first), Some(through)) = (resource.first_unix, resource.through_unix) {
        anyhow::ensure!(
            first <= through,
            "dashboard resident resource bounds are reversed"
        );
    }
    if let (Some(first), Some(through)) = (network.first_unix, network.through_unix) {
        anyhow::ensure!(
            first <= through,
            "dashboard resident network bounds are reversed"
        );
    }
    if let (Some(first), Some(through)) = (traffic.first_unix, traffic.through_unix) {
        anyhow::ensure!(
            first <= through,
            "dashboard resident traffic bounds are reversed"
        );
    }
    anyhow::ensure!(
        network.interfaces.windows(2).all(|pair| pair[0] < pair[1])
            && network.interfaces.iter().all(|value| !value.is_empty()),
        "dashboard resident network interface map is not canonical"
    );
    anyhow::ensure!(
        traffic_width >= 0
            && usize::try_from(traffic_width)? == traffic.source_kinds.len()
            && traffic.source_kinds.len() == traffic.interfaces.len()
            && traffic
                .source_kinds
                .iter()
                .zip(traffic.interfaces.iter())
                .all(|(source_kind, interface)| {
                    matches!(source_kind.as_str(), "host" | "tunnel") && !interface.is_empty()
                })
            && traffic
                .source_kinds
                .iter()
                .zip(traffic.interfaces.iter())
                .zip(
                    traffic
                        .source_kinds
                        .iter()
                        .zip(traffic.interfaces.iter())
                        .skip(1),
                )
                .all(|(left, right)| left < right),
        "dashboard resident traffic stream map is not canonical"
    );
    Ok((
        client_id,
        ClientHeads {
            resource,
            network,
            traffic,
        },
    ))
}

async fn load_heads(listener: &mut PgListener) -> Result<BTreeMap<String, ClientHeads>> {
    parse_heads(sqlx::query(HEADS_SQL).fetch_all(&mut *listener).await?)
}

async fn load_selected_heads(
    listener: &mut PgListener,
    client_ids: &[String],
) -> Result<BTreeMap<String, ClientHeads>> {
    if client_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    parse_heads(
        sqlx::query(&format!(
            "SELECT * FROM ({HEADS_SQL}) heads WHERE client_id = ANY($1::TEXT[]) ORDER BY client_id"
        ))
        .bind(client_ids)
        .fetch_all(&mut *listener)
        .await?,
    )
}

fn parse_heads(rows: Vec<PgRow>) -> Result<BTreeMap<String, ClientHeads>> {
    rows.into_iter().map(|row| parse_head(&row)).collect()
}

async fn load_optional_client_heads(
    listener: &mut PgListener,
    client_id: &str,
) -> Result<Option<ClientHeads>> {
    let row = sqlx::query(&format!(
        "SELECT * FROM ({HEADS_SQL}) heads WHERE client_id = $1"
    ))
    .bind(client_id)
    .fetch_optional(&mut *listener)
    .await?;
    row.map(|row| parse_head(&row).map(|(_, heads)| heads))
        .transpose()
}

#[derive(Clone, Copy, Debug, Default)]
struct ResourceSummary {
    sample_count: i64,
    cpu_sum: f64,
    cpu_max: f64,
    memory_total: i64,
    memory_sum: f64,
    memory_max: f64,
    disk_count: i64,
    disk_total: i64,
    disk_sum: f64,
    disk_max: f64,
    latest: i64,
}

impl ResourceSummary {
    fn merge(&mut self, right: Self) {
        self.sample_count = self.sample_count.saturating_add(right.sample_count);
        self.cpu_sum += right.cpu_sum;
        self.cpu_max = self.cpu_max.max(right.cpu_max);
        self.memory_total = self.memory_total.max(right.memory_total);
        self.memory_sum += right.memory_sum;
        self.memory_max = self.memory_max.max(right.memory_max);
        self.disk_count = self.disk_count.saturating_add(right.disk_count);
        self.disk_total = self.disk_total.max(right.disk_total);
        self.disk_sum += right.disk_sum;
        self.disk_max = self.disk_max.max(right.disk_max);
        self.latest = self.latest.max(right.latest);
    }

    fn valid(self) -> Result<Self> {
        anyhow::ensure!(
            self.sample_count >= 0 && self.disk_count >= 0 && self.disk_count <= self.sample_count,
            "dashboard resident resource sample count is invalid"
        );
        anyhow::ensure!(
            self.memory_total >= 0 && self.disk_total >= 0,
            "dashboard resident resource capacity is negative"
        );
        for value in [
            self.cpu_sum,
            self.cpu_max,
            self.memory_sum,
            self.memory_max,
            self.disk_sum,
            self.disk_max,
        ] {
            anyhow::ensure!(
                value.is_finite() && value >= 0.0,
                "dashboard resident resource statistic is invalid"
            );
        }
        anyhow::ensure!(
            (0.0..=1.0).contains(&self.memory_max) && (0.0..=1.0).contains(&self.disk_max),
            "dashboard resident resource ratio is outside [0,1]"
        );
        anyhow::ensure!(
            self.latest >= 0,
            "dashboard resident resource time is negative"
        );
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct ResourceBlock {
    start: i64,
    slots: [ResourceSummary; BLOCK_SLOTS],
}

impl ResourceBlock {
    fn empty(start: i64) -> Self {
        Self {
            start,
            slots: [ResourceSummary::default(); BLOCK_SLOTS],
        }
    }
    fn summary(&self) -> ResourceSummary {
        let mut result = ResourceSummary::default();
        for slot in self.slots {
            result.merge(slot);
        }
        result
    }
}

#[derive(Debug)]
struct ResourceBlockRow {
    tier: i32,
    start: i64,
    revision: i64,
    sample_counts: Vec<i64>,
    cpu_sums: Vec<Option<f64>>,
    cpu_maxes: Vec<Option<f64>>,
    memory_totals: Vec<Option<i64>>,
    memory_sums: Vec<Option<f64>>,
    memory_maxes: Vec<Option<f64>>,
    disk_counts: Vec<i64>,
    disk_totals: Vec<Option<i64>>,
    disk_sums: Vec<Option<f64>>,
    disk_maxes: Vec<Option<f64>>,
    latest: Vec<Option<i64>>,
}

impl<'r> FromRow<'r, PgRow> for ResourceBlockRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            tier: row.try_get("source_bucket_secs")?,
            start: row.try_get("block_start_unix")?,
            revision: row.try_get("published_revision")?,
            sample_counts: row.try_get("sample_counts")?,
            cpu_sums: row.try_get("cpu_load_1_sums")?,
            cpu_maxes: row.try_get("cpu_load_1_maxes")?,
            memory_totals: row.try_get("memory_total_bytes_maxes")?,
            memory_sums: row.try_get("memory_used_ratio_sums")?,
            memory_maxes: row.try_get("memory_used_ratio_maxes")?,
            disk_counts: row.try_get("disk_sample_counts")?,
            disk_totals: row.try_get("disk_total_bytes_maxes")?,
            disk_sums: row.try_get("disk_used_ratio_sums")?,
            disk_maxes: row.try_get("disk_used_ratio_maxes")?,
            latest: row.try_get("latest_observed_unix")?,
        })
    }
}

impl ResourceBlockRow {
    fn into_block(self, head: &ResourceHead) -> Result<(i32, ResourceBlock)> {
        anyhow::ensure!(
            self.revision <= head.revision,
            "dashboard resource block is newer than its head fence"
        );
        anyhow::ensure!(
            valid_tier(self.tier)
                && self
                    .start
                    .rem_euclid(i64::from(self.tier) * BLOCK_SLOTS as i64)
                    == 0,
            "dashboard resource block key is invalid"
        );
        for len in [
            self.sample_counts.len(),
            self.cpu_sums.len(),
            self.cpu_maxes.len(),
            self.memory_totals.len(),
            self.memory_sums.len(),
            self.memory_maxes.len(),
            self.disk_counts.len(),
            self.disk_totals.len(),
            self.disk_sums.len(),
            self.disk_maxes.len(),
            self.latest.len(),
        ] {
            anyhow::ensure!(
                len == BLOCK_SLOTS,
                "dashboard resource block width is not F16"
            );
        }
        let mut block = ResourceBlock::empty(self.start);
        for slot in 0..BLOCK_SLOTS {
            let values = (
                self.cpu_sums[slot],
                self.cpu_maxes[slot],
                self.memory_totals[slot],
                self.memory_sums[slot],
                self.memory_maxes[slot],
                self.disk_totals[slot],
                self.disk_sums[slot],
                self.disk_maxes[slot],
                self.latest[slot],
            );
            if self.sample_counts[slot] == 0 {
                anyhow::ensure!(
                    self.disk_counts[slot] == 0
                        && matches!(
                            values,
                            (None, None, None, None, None, None, None, None, None)
                        ),
                    "dashboard resource absent slot carries evidence"
                );
                continue;
            }
            let (
                Some(cpu_sum),
                Some(cpu_max),
                Some(memory_total),
                Some(memory_sum),
                Some(memory_max),
                Some(disk_total),
                Some(disk_sum),
                Some(disk_max),
                Some(latest),
            ) = values
            else {
                anyhow::bail!("dashboard resource present slot is missing evidence");
            };
            block.slots[slot] = ResourceSummary {
                sample_count: self.sample_counts[slot],
                cpu_sum,
                cpu_max,
                memory_total,
                memory_sum,
                memory_max,
                disk_count: self.disk_counts[slot],
                disk_total,
                disk_sum,
                disk_max,
                latest,
            }
            .valid()?;
        }
        Ok((self.tier, block))
    }
}

#[derive(Debug)]
struct ResourceOverlayRow {
    tier: i32,
    block_start: i64,
    bucket_start: i64,
    state: ResourceSummary,
}

impl<'r> FromRow<'r, PgRow> for ResourceOverlayRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            tier: row.try_get("source_bucket_secs")?,
            block_start: row.try_get("block_start_unix")?,
            bucket_start: row.try_get("bucket_start_unix")?,
            state: ResourceSummary {
                sample_count: row.try_get("sample_count")?,
                cpu_sum: row.try_get("cpu_load_1_sum")?,
                cpu_max: row.try_get("cpu_load_1_max")?,
                memory_total: row.try_get("memory_total_bytes_max")?,
                memory_sum: row.try_get("memory_used_ratio_sum")?,
                memory_max: row.try_get("memory_used_ratio_max")?,
                disk_count: row.try_get("disk_sample_count")?,
                disk_total: row.try_get("disk_total_bytes_max")?,
                disk_sum: row.try_get("disk_used_ratio_sum")?,
                disk_max: row.try_get("disk_used_ratio_max")?,
                latest: row.try_get("latest_observed_unix")?,
            },
        })
    }
}

impl ResourceOverlayRow {
    fn apply(
        self,
        blocks: &mut BTreeMap<(i32, i64), ResourceBlock>,
        _head: &ResourceHead,
    ) -> Result<()> {
        anyhow::ensure!(
            valid_tier(self.tier),
            "dashboard resource overlay row has an invalid tier"
        );
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        anyhow::ensure!(
            self.block_start.rem_euclid(span) == 0
                && self.bucket_start >= self.block_start
                && self.bucket_start < self.block_start + span
                && (self.bucket_start - self.block_start).rem_euclid(i64::from(self.tier)) == 0,
            "dashboard resource overlay-row key is invalid"
        );
        let slot = usize::try_from((self.bucket_start - self.block_start) / i64::from(self.tier))?;
        blocks
            .entry((self.tier, self.block_start))
            .or_insert_with(|| ResourceBlock::empty(self.block_start))
            .slots[slot] = self.state.valid()?;
        Ok(())
    }
}

fn valid_tier(tier: i32) -> bool {
    matches!(tier, 60 | 300 | 1_800 | 3_600 | 10_800 | 21_600 | 86_400)
}

fn floor_multiple(value: i64, unit: i64) -> i64 {
    value.div_euclid(unit) * unit
}
fn ceil_div(value: i64, unit: i64) -> i64 {
    (-value).div_euclid(unit).saturating_neg()
}

#[derive(Clone, Debug)]
struct ResourceTier {
    tier: i32,
    blocks: Vec<Option<Arc<ResourceBlock>>>,
    head: usize,
    len: usize,
    first_start: i64,
    tree_base: usize,
    tree: Vec<ResourceSummary>,
}

impl ResourceTier {
    fn from_sparse(tier: i32, sparse: BTreeMap<i64, ResourceBlock>) -> Result<Self> {
        anyhow::ensure!(valid_tier(tier), "dashboard resource tier is invalid");
        if sparse.is_empty() {
            return Ok(Self::empty(tier));
        }
        let span = i64::from(tier) * BLOCK_SLOTS as i64;
        let first_start = *sparse.first_key_value().unwrap().0;
        let last = *sparse.last_key_value().unwrap().0;
        let len = usize::try_from((last - first_start) / span + 1)?;
        let tree_base = len.next_power_of_two();
        let mut blocks = vec![None; tree_base];
        for (ordinal, block_slot) in blocks.iter_mut().enumerate().take(len) {
            let start = first_start + i64::try_from(ordinal)? * span;
            *block_slot = sparse.get(&start).cloned().map(Arc::new);
        }
        let mut result = Self {
            tier,
            blocks,
            head: 0,
            len,
            first_start,
            tree_base,
            tree: vec![ResourceSummary::default(); tree_base * 2],
        };
        result.rebuild_tree();
        Ok(result)
    }

    fn empty(tier: i32) -> Self {
        Self {
            tier,
            blocks: vec![None],
            head: 0,
            len: 0,
            first_start: 0,
            tree_base: 1,
            tree: vec![ResourceSummary::default(); 2],
        }
    }

    fn first_start(&self) -> Option<i64> {
        (self.len > 0).then_some(self.first_start)
    }
    fn slot_len(&self) -> usize {
        self.len * BLOCK_SLOTS
    }
    fn physical(&self, logical: usize) -> usize {
        debug_assert!(logical < self.len);
        (self.head + logical) % self.tree_base
    }
    fn block(&self, logical: usize) -> Option<&ResourceBlock> {
        self.blocks[self.physical(logical)].as_deref()
    }

    fn rebuild_tree(&mut self) {
        self.tree.fill(ResourceSummary::default());
        for physical in 0..self.tree_base {
            if let Some(block) = &self.blocks[physical] {
                self.tree[self.tree_base + physical] = block.summary();
            }
        }
        for node in (1..self.tree_base).rev() {
            let mut state = self.tree[node * 2];
            state.merge(self.tree[node * 2 + 1]);
            self.tree[node] = state;
        }
    }

    fn update_leaf(&mut self, physical: usize) -> usize {
        let mut node = self.tree_base + physical;
        self.tree[node] = self.blocks[physical]
            .as_deref()
            .map(ResourceBlock::summary)
            .unwrap_or_default();
        let mut touched = 1;
        while node > 1 {
            node /= 2;
            let mut state = self.tree[node * 2];
            state.merge(self.tree[node * 2 + 1]);
            self.tree[node] = state;
            touched += 1;
        }
        touched
    }

    fn grow(&mut self) {
        let ordered = (0..self.len)
            .map(|logical| self.blocks[self.physical(logical)].clone())
            .collect::<Vec<_>>();
        self.tree_base = self.tree_base.saturating_mul(2).max(1);
        self.blocks = vec![None; self.tree_base];
        for (logical, block) in ordered.into_iter().enumerate() {
            self.blocks[logical] = block;
        }
        self.head = 0;
        self.tree = vec![ResourceSummary::default(); self.tree_base * 2];
        self.rebuild_tree();
    }

    fn append(&mut self, block: Option<ResourceBlock>) -> usize {
        if self.len == self.tree_base {
            self.grow();
        }
        let physical = (self.head + self.len) % self.tree_base;
        debug_assert!(self.blocks[physical].is_none());
        self.blocks[physical] = block.map(Arc::new);
        self.len += 1;
        self.update_leaf(physical)
    }

    fn prepend(&mut self, block: Option<ResourceBlock>) -> usize {
        if self.len == self.tree_base {
            self.grow();
        }
        self.head = (self.head + self.tree_base - 1) % self.tree_base;
        debug_assert!(self.blocks[self.head].is_none());
        self.blocks[self.head] = block.map(Arc::new);
        self.len += 1;
        self.first_start -= i64::from(self.tier) * BLOCK_SLOTS as i64;
        self.update_leaf(self.head)
    }

    fn trim_empty_edges(&mut self) -> usize {
        let mut touched = 0;
        while self.len > 0 {
            let physical = self.head;
            if self.tree[self.tree_base + physical].sample_count > 0 {
                break;
            }
            self.blocks[physical] = None;
            touched += self.update_leaf(physical);
            self.head = (self.head + 1) % self.tree_base;
            self.len -= 1;
            self.first_start += i64::from(self.tier) * BLOCK_SLOTS as i64;
        }
        while self.len > 0 {
            let physical = self.physical(self.len - 1);
            if self.tree[self.tree_base + physical].sample_count > 0 {
                break;
            }
            self.blocks[physical] = None;
            touched += self.update_leaf(physical);
            self.len -= 1;
        }
        if self.len == 0 {
            self.head = 0;
            self.first_start = 0;
        }
        touched
    }

    fn set_block(&mut self, start: i64, replacement: Option<ResourceBlock>) -> usize {
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        debug_assert_eq!(start.rem_euclid(span), 0);
        debug_assert!(replacement
            .as_ref()
            .is_none_or(|block| block.start == start));
        if self.len == 0 {
            let Some(block) = replacement else {
                return 0;
            };
            self.first_start = start;
            return self.append(Some(block));
        }
        let end = self.first_start + i64::try_from(self.len).unwrap_or(i64::MAX) * span;
        if start >= self.first_start && start < end {
            let logical = usize::try_from((start - self.first_start) / span)
                .expect("nonnegative resident block ordinal");
            let physical = self.physical(logical);
            self.blocks[physical] = replacement.map(Arc::new);
            return self.update_leaf(physical) + self.trim_empty_edges();
        }
        let Some(block) = replacement else {
            return 0;
        };
        let mut block = Some(block);
        let mut touched = 0;
        if start >= end {
            let mut cursor = end;
            loop {
                touched += self.append((cursor == start).then(|| block.take().unwrap()));
                if cursor == start {
                    break;
                }
                cursor = cursor.saturating_add(span);
            }
        } else {
            while self.first_start > start {
                let next = self.first_start - span;
                touched += self.prepend((next == start).then(|| block.take().unwrap()));
            }
        }
        touched
    }

    fn find_present_physical(
        &self,
        node: usize,
        node_lo: usize,
        node_hi: usize,
        query_lo: usize,
        query_hi: usize,
        reverse: bool,
    ) -> Option<usize> {
        if node_hi <= query_lo || query_hi <= node_lo || self.tree[node].sample_count == 0 {
            return None;
        }
        if node_hi - node_lo == 1 {
            return Some(node_lo);
        }
        let middle = (node_lo + node_hi) / 2;
        if reverse {
            self.find_present_physical(node * 2 + 1, middle, node_hi, query_lo, query_hi, true)
                .or_else(|| {
                    self.find_present_physical(node * 2, node_lo, middle, query_lo, query_hi, true)
                })
        } else {
            self.find_present_physical(node * 2, node_lo, middle, query_lo, query_hi, false)
                .or_else(|| {
                    self.find_present_physical(
                        node * 2 + 1,
                        middle,
                        node_hi,
                        query_lo,
                        query_hi,
                        false,
                    )
                })
        }
    }

    fn first_last_present(&self) -> Option<(i64, i64)> {
        if self.len == 0 || self.tree[1].sample_count == 0 {
            return None;
        }
        let tail = (self.head + self.len).min(self.tree_base);
        let first_physical = self
            .find_present_physical(1, 0, self.tree_base, self.head, tail, false)
            .or_else(|| {
                let wrapped = (self.head + self.len).saturating_sub(self.tree_base);
                self.find_present_physical(1, 0, self.tree_base, 0, wrapped, false)
            })?;
        let wrapped = (self.head + self.len).saturating_sub(self.tree_base);
        let last_physical = self
            .find_present_physical(1, 0, self.tree_base, 0, wrapped, true)
            .or_else(|| self.find_present_physical(1, 0, self.tree_base, self.head, tail, true))?;
        let first_logical = (first_physical + self.tree_base - self.head) % self.tree_base;
        let last_logical = (last_physical + self.tree_base - self.head) % self.tree_base;
        let first_block = self.block(first_logical)?;
        let last_block = self.block(last_logical)?;
        let first_slot = first_block
            .slots
            .iter()
            .position(|state| state.sample_count > 0)?;
        let last_slot = last_block
            .slots
            .iter()
            .rposition(|state| state.sample_count > 0)?;
        Some((
            first_block.start + i64::try_from(first_slot).ok()? * i64::from(self.tier),
            last_block.start + i64::try_from(last_slot).ok()? * i64::from(self.tier),
        ))
    }

    fn merge_physical(&self, mut lo: usize, mut hi: usize, result: &mut ResourceSummary) {
        if lo >= hi {
            return;
        }
        lo += self.tree_base;
        hi += self.tree_base;
        let mut right = [ResourceSummary::default(); 32];
        let mut right_len = 0;
        while lo < hi {
            if lo & 1 == 1 {
                result.merge(self.tree[lo]);
                lo += 1;
            }
            if hi & 1 == 1 {
                hi -= 1;
                right[right_len] = self.tree[hi];
                right_len += 1;
            }
            lo /= 2;
            hi /= 2;
        }
        for index in (0..right_len).rev() {
            result.merge(right[index]);
        }
    }

    fn merge_block_range(&self, lo: usize, hi: usize, result: &mut ResourceSummary) {
        if lo >= hi {
            return;
        }
        let physical = (self.head + lo) % self.tree_base;
        let count = hi - lo;
        let first_count = count.min(self.tree_base - physical);
        self.merge_physical(physical, physical + first_count, result);
        if first_count < count {
            self.merge_physical(0, count - first_count, result);
        }
    }

    fn range(&self, lo: usize, hi: usize) -> ResourceSummary {
        if lo >= hi {
            return ResourceSummary::default();
        }
        debug_assert!(hi <= self.slot_len());
        let first_block = lo / BLOCK_SLOTS;
        let last_block = (hi - 1) / BLOCK_SLOTS;
        let mut result = ResourceSummary::default();
        if first_block == last_block {
            if let Some(block) = self.block(first_block) {
                for slot in lo % BLOCK_SLOTS..((hi - 1) % BLOCK_SLOTS + 1) {
                    result.merge(block.slots[slot]);
                }
            }
            return result;
        }
        if let Some(block) = self.block(first_block) {
            for slot in lo % BLOCK_SLOTS..BLOCK_SLOTS {
                result.merge(block.slots[slot]);
            }
        }
        self.merge_block_range(first_block + 1, last_block, &mut result);
        if let Some(block) = self.block(last_block) {
            for slot in 0..((hi - 1) % BLOCK_SLOTS + 1) {
                result.merge(block.slots[slot]);
            }
        }
        result
    }
}
#[derive(Clone, Debug, Default)]
struct ResourceIndex {
    tiers: BTreeMap<i32, ResourceTier>,
}

impl ResourceIndex {
    fn from_blocks(blocks: BTreeMap<(i32, i64), ResourceBlock>) -> Result<Self> {
        let mut by_tier = BTreeMap::<i32, BTreeMap<i64, ResourceBlock>>::new();
        for ((tier, start), block) in blocks {
            by_tier.entry(tier).or_default().insert(start, block);
        }
        let tiers = by_tier
            .into_iter()
            .map(|(tier, blocks)| Ok((tier, ResourceTier::from_sparse(tier, blocks)?)))
            .collect::<Result<_>>()?;
        Ok(Self { tiers })
    }

    fn apply_blocks(&mut self, changes: Vec<(i32, i64, Option<ResourceBlock>)>) -> usize {
        let mut touched = 0;
        for (tier, start, replacement) in changes {
            if replacement.is_some() {
                self.tiers
                    .entry(tier)
                    .or_insert_with(|| ResourceTier::empty(tier));
            }
            let remove = if let Some(owner) = self.tiers.get_mut(&tier) {
                touched += owner.set_block(start, replacement);
                owner.len == 0
            } else {
                false
            };
            if remove {
                self.tiers.remove(&tier);
            }
        }
        touched
    }

    fn fold(&self, start: i64, end: i64, requested_step: i64) -> Vec<ResourcePoint> {
        if start < 0 || start > end || requested_step <= 0 {
            return Vec::new();
        }
        let mut points = BTreeMap::<(i64, i64), ResourceSummary>::new();
        for tier in self.tiers.values() {
            let Some((retained_first, retained_last)) = tier.first_last_present() else {
                continue;
            };
            let source_tier = i64::from(tier.tier);
            let first_source = retained_first.max(floor_multiple(start, source_tier));
            let last_source = retained_last.min(floor_multiple(end, source_tier));
            if first_source > last_source {
                continue;
            }
            let effective = requested_step.max(source_tier);
            let base = tier.first_start().unwrap();
            let mut bucket = floor_multiple(first_source, effective);
            let last_bucket = floor_multiple(last_source, effective);
            while bucket <= last_bucket {
                let lo = ceil_div(first_source.max(bucket) - base, source_tier)
                    .clamp(0, tier.slot_len() as i64) as usize;
                let hi_source = (last_source + source_tier).min(bucket.saturating_add(effective));
                let hi = ceil_div(hi_source - base, source_tier).clamp(0, tier.slot_len() as i64)
                    as usize;
                if lo < hi {
                    points
                        .entry((bucket, effective))
                        .or_default()
                        .merge(tier.range(lo, hi));
                }
                bucket = bucket.saturating_add(effective);
            }
        }
        points
            .into_iter()
            .filter_map(|((bucket, step), state)| {
                (state.sample_count > 0).then_some(ResourcePoint {
                    step,
                    bucket,
                    state,
                })
            })
            .collect()
    }
}

#[derive(Clone, Copy, Debug)]
struct ResourcePoint {
    step: i64,
    bucket: i64,
    state: ResourceSummary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ResourceMetric {
    Cpu,
    Memory,
    Disk,
}

impl ResourceMetric {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "cpu_load" => Ok(Self::Cpu),
            "memory_used" => Ok(Self::Memory),
            "disk_free" => Ok(Self::Disk),
            _ => anyhow::bail!("invalid dashboard resource metric"),
        }
    }
    fn values(self, state: ResourceSummary) -> Option<(f64, f64)> {
        match self {
            Self::Cpu if state.sample_count > 0 => {
                let average = state.cpu_sum / state.sample_count as f64;
                Some((average, average.max(state.cpu_max)))
            }
            Self::Memory if state.sample_count > 0 && state.memory_total > 0 => Some((
                state.memory_sum / state.sample_count as f64,
                state.memory_max,
            )),
            Self::Disk if state.disk_count > 0 && state.disk_total > 0 => Some((
                1.0 - state.disk_sum / state.disk_count as f64,
                1.0 - state.disk_max,
            )),
            _ => None,
        }
    }
    fn top_score(self, peak: f64) -> f64 {
        match self {
            Self::Cpu => peak.max(0.0),
            Self::Memory => peak,
            Self::Disk => 1.0 - peak,
        }
    }
}

#[derive(Clone, Debug)]
struct ResourceOwner {
    generation: i64,
    revision: i64,
    overlay_blocks: BTreeSet<(i32, i64)>,
    index: ResourceIndex,
}

fn unix_timestamp(value: i64, field: &str) -> Result<String> {
    anyhow::ensure!(value >= 0, "dashboard resident {field} is negative");
    DateTime::<Utc>::from_timestamp(value, 0)
        .with_context(|| format!("dashboard resident {field} is outside the timestamp range"))
        .map(|value| value.to_rfc3339_opts(SecondsFormat::AutoSi, false))
}

fn resource_view(
    client_id: &str,
    point: ResourcePoint,
    metric: ResourceMetric,
    values: Option<(f64, f64)>,
) -> Result<TelemetryRollupView> {
    let (value, peak, has_metric) = values
        .map(|(value, peak)| (value, peak, true))
        .unwrap_or((0.0, 0.0, false));
    anyhow::ensure!(
        value.is_finite() && peak.is_finite(),
        "dashboard resident resource output is invalid"
    );
    if matches!(metric, ResourceMetric::Memory | ResourceMetric::Disk) && has_metric {
        anyhow::ensure!(
            (0.0..=1.0).contains(&value) && (0.0..=1.0).contains(&peak),
            "dashboard resident resource ratio output is invalid"
        );
    }
    let bucket_start = unix_timestamp(point.bucket, "resource bucket")?;
    let latest = unix_timestamp(point.state.latest, "resource observation")?;
    Ok(TelemetryRollupView {
        client_id: client_id.to_string(),
        bucket_start,
        bucket_secs: i32::try_from(point.step)?,
        sample_count: i32::from(metric == ResourceMetric::Disk && has_metric),
        cpu_usage_sample_count: 0,
        cpu_usage_avg: None,
        cpu_usage_max: None,
        cpu_cores_max: 0,
        cpu_load_1_avg: if metric == ResourceMetric::Cpu {
            value
        } else {
            0.0
        },
        cpu_load_1_max: if metric == ResourceMetric::Cpu {
            peak
        } else {
            0.0
        },
        cpu_load_5_avg: 0.0,
        cpu_load_5_max: 0.0,
        cpu_load_15_avg: 0.0,
        cpu_load_15_max: 0.0,
        memory_total_bytes_max: i64::from(metric == ResourceMetric::Memory && has_metric),
        memory_available_bytes_avg: 0,
        memory_available_bytes_min: 0,
        memory_used_ratio_avg: if metric == ResourceMetric::Memory {
            value
        } else {
            0.0
        },
        memory_used_ratio_max: if metric == ResourceMetric::Memory {
            peak
        } else {
            0.0
        },
        swap_sample_count: 0,
        swap_total_bytes_max: None,
        swap_available_bytes_avg: None,
        swap_available_bytes_min: None,
        swap_used_ratio_avg: None,
        swap_used_ratio_max: None,
        disk_sample_count: i32::from(metric == ResourceMetric::Disk && has_metric),
        disk_total_bytes_max: i64::from(metric == ResourceMetric::Disk && has_metric),
        disk_available_bytes_avg: 0,
        disk_available_bytes_min: 0,
        disk_used_ratio_avg: if metric == ResourceMetric::Disk && has_metric {
            1.0 - value
        } else {
            0.0
        },
        disk_used_ratio_max: if metric == ResourceMetric::Disk && has_metric {
            1.0 - peak
        } else {
            0.0
        },
        connections_sample_count: 0,
        tcp_sockets_latest: None,
        udp_sockets_latest: None,
        connections_observed_at: None,
        latest_observed_at: latest.clone(),
        updated_at: latest,
    })
}

#[derive(Clone, Copy, Debug, Default)]
struct NetworkState {
    count: i64,
    latest: i64,
    rx: i64,
    tx: i64,
    rx_epoch: i64,
    tx_epoch: i64,
}

impl NetworkState {
    fn present(self) -> bool {
        self.count > 0
    }
    fn merge(&mut self, right: Self) {
        let left_empty = !self.present();
        self.count = self.count.saturating_add(right.count);
        if right.present() && (left_empty || right.latest >= self.latest) {
            self.latest = right.latest;
            self.rx = right.rx;
            self.tx = right.tx;
            self.rx_epoch = right.rx_epoch;
            self.tx_epoch = right.tx_epoch;
        }
    }
    fn valid(self) -> Result<Self> {
        anyhow::ensure!(
            self.count >= 0
                && self.latest >= 0
                && self.rx >= 0
                && self.tx >= 0
                && self.rx_epoch >= 0
                && self.tx_epoch >= 0,
            "dashboard resident network state is invalid"
        );
        anyhow::ensure!(
            self.present()
                || (self.latest == 0
                    && self.rx == 0
                    && self.tx == 0
                    && self.rx_epoch == 0
                    && self.tx_epoch == 0),
            "dashboard resident absent network state carries evidence"
        );
        Ok(self)
    }
}

#[derive(Clone, Debug)]
struct NetworkBlock {
    start: i64,
    width: usize,
    slots: Box<[NetworkState]>,
}

impl NetworkBlock {
    fn empty(start: i64, width: usize) -> Self {
        Self {
            start,
            width,
            slots: vec![NetworkState::default(); BLOCK_SLOTS * width].into_boxed_slice(),
        }
    }
    fn slot(&self, slot: usize) -> &[NetworkState] {
        &self.slots[slot * self.width..(slot + 1) * self.width]
    }
}

#[derive(Debug)]
struct NetworkBlockRow {
    tier: i32,
    start: i64,
    revision: i64,
    counts: Vec<i64>,
    latest: Vec<Option<i64>>,
    rx: Vec<Option<i64>>,
    tx: Vec<Option<i64>>,
    rx_epoch: Vec<Option<i64>>,
    tx_epoch: Vec<Option<i64>>,
}

impl<'r> FromRow<'r, PgRow> for NetworkBlockRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            tier: row.try_get("source_bucket_secs")?,
            start: row.try_get("block_start_unix")?,
            revision: row.try_get("published_revision")?,
            counts: row.try_get("sample_counts")?,
            latest: row.try_get("latest_observed_unix")?,
            rx: row.try_get("rx_bytes_last")?,
            tx: row.try_get("tx_bytes_last")?,
            rx_epoch: row.try_get("rx_counter_epoch")?,
            tx_epoch: row.try_get("tx_counter_epoch")?,
        })
    }
}

impl NetworkBlockRow {
    fn into_block(self, head: &NetworkHead) -> Result<(i32, NetworkBlock)> {
        let width = head.interfaces.len();
        anyhow::ensure!(
            self.revision <= head.revision && valid_tier(self.tier),
            "dashboard network block is outside its head fence"
        );
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        anyhow::ensure!(
            self.start.rem_euclid(span) == 0,
            "dashboard network block key is invalid"
        );
        let expected = BLOCK_SLOTS * width;
        for len in [
            self.counts.len(),
            self.latest.len(),
            self.rx.len(),
            self.tx.len(),
            self.rx_epoch.len(),
            self.tx_epoch.len(),
        ] {
            anyhow::ensure!(
                len == expected,
                "dashboard network block width does not match its generation map"
            );
        }
        let mut block = NetworkBlock::empty(self.start, width);
        for index in 0..expected {
            let values = (
                self.latest[index],
                self.rx[index],
                self.tx[index],
                self.rx_epoch[index],
                self.tx_epoch[index],
            );
            if self.counts[index] == 0 {
                anyhow::ensure!(
                    matches!(values, (None, None, None, None, None)),
                    "dashboard network absent slot carries evidence"
                );
                continue;
            }
            let (Some(latest), Some(rx), Some(tx), Some(rx_epoch), Some(tx_epoch)) = values else {
                anyhow::bail!("dashboard network present slot is missing evidence");
            };
            block.slots[index] = NetworkState {
                count: self.counts[index],
                latest,
                rx,
                tx,
                rx_epoch,
                tx_epoch,
            }
            .valid()?;
        }
        Ok((self.tier, block))
    }
}

#[derive(Debug)]
struct NetworkOverlayRow {
    tier: i32,
    block_start: i64,
    bucket_start: i64,
    counts: Vec<i64>,
    latest: Vec<Option<i64>>,
    rx: Vec<Option<i64>>,
    tx: Vec<Option<i64>>,
    rx_epoch: Vec<Option<i64>>,
    tx_epoch: Vec<Option<i64>>,
}

impl<'r> FromRow<'r, PgRow> for NetworkOverlayRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            tier: row.try_get("source_bucket_secs")?,
            block_start: row.try_get("block_start_unix")?,
            bucket_start: row.try_get("bucket_start_unix")?,
            counts: row.try_get("sample_counts")?,
            latest: row.try_get("latest_observed_unix")?,
            rx: row.try_get("rx_bytes_last")?,
            tx: row.try_get("tx_bytes_last")?,
            rx_epoch: row.try_get("rx_counter_epoch")?,
            tx_epoch: row.try_get("tx_counter_epoch")?,
        })
    }
}

impl NetworkOverlayRow {
    fn apply(
        self,
        blocks: &mut BTreeMap<(i32, i64), NetworkBlock>,
        head: &NetworkHead,
    ) -> Result<()> {
        let width = head.interfaces.len();
        anyhow::ensure!(
            valid_tier(self.tier),
            "dashboard network overlay row has an invalid tier"
        );
        for len in [
            self.counts.len(),
            self.latest.len(),
            self.rx.len(),
            self.tx.len(),
            self.rx_epoch.len(),
            self.tx_epoch.len(),
        ] {
            anyhow::ensure!(
                len == width,
                "dashboard network overlay width does not match its generation map"
            );
        }
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        anyhow::ensure!(
            self.block_start.rem_euclid(span) == 0
                && self.bucket_start >= self.block_start
                && self.bucket_start < self.block_start + span
                && (self.bucket_start - self.block_start).rem_euclid(i64::from(self.tier)) == 0,
            "dashboard network overlay-row key is invalid"
        );
        let slot = usize::try_from((self.bucket_start - self.block_start) / i64::from(self.tier))?;
        let block = blocks
            .entry((self.tier, self.block_start))
            .or_insert_with(|| NetworkBlock::empty(self.block_start, width));
        anyhow::ensure!(
            block.width == width,
            "dashboard network block width changed within a generation"
        );
        for interface in 0..width {
            let values = (
                self.latest[interface],
                self.rx[interface],
                self.tx[interface],
                self.rx_epoch[interface],
                self.tx_epoch[interface],
            );
            if self.counts[interface] == 0 {
                anyhow::ensure!(
                    matches!(values, (None, None, None, None, None)),
                    "dashboard network absent overlay slot carries evidence"
                );
                continue;
            }
            let (Some(latest), Some(rx), Some(tx), Some(rx_epoch), Some(tx_epoch)) = values else {
                anyhow::bail!("dashboard network present overlay slot is missing evidence");
            };
            block.slots[slot * width + interface] = NetworkState {
                count: self.counts[interface],
                latest,
                rx,
                tx,
                rx_epoch,
                tx_epoch,
            }
            .valid()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct NetworkOverlaySourceRow {
    client_id: String,
    interface: String,
    source_bucket_secs: i32,
    block_start_unix: i64,
    bucket_start_unix: i64,
    sample_count: i64,
    latest_observed_unix: i64,
    rx_bytes_last: i64,
    tx_bytes_last: i64,
    rx_counter_epoch: i64,
    tx_counter_epoch: i64,
}

impl<'r> FromRow<'r, PgRow> for NetworkOverlaySourceRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            client_id: row.try_get("client_id")?,
            interface: row.try_get("interface")?,
            source_bucket_secs: row.try_get("source_bucket_secs")?,
            block_start_unix: row.try_get("block_start_unix")?,
            bucket_start_unix: row.try_get("bucket_start_unix")?,
            sample_count: row.try_get("sample_count")?,
            latest_observed_unix: row.try_get("latest_observed_unix")?,
            rx_bytes_last: row.try_get("rx_bytes_last")?,
            tx_bytes_last: row.try_get("tx_bytes_last")?,
            rx_counter_epoch: row.try_get("rx_counter_epoch")?,
            tx_counter_epoch: row.try_get("tx_counter_epoch")?,
        })
    }
}

#[derive(Clone, Debug)]
struct NetworkTier {
    tier: i32,
    width: usize,
    blocks: Vec<Option<Arc<NetworkBlock>>>,
    head: usize,
    len: usize,
    first_start: i64,
    tree_base: usize,
    tree: Box<[NetworkState]>,
}

impl NetworkTier {
    fn from_sparse(tier: i32, width: usize, sparse: BTreeMap<i64, NetworkBlock>) -> Result<Self> {
        anyhow::ensure!(valid_tier(tier), "dashboard network tier is invalid");
        if sparse.is_empty() {
            return Ok(Self::empty(tier, width));
        }
        anyhow::ensure!(
            width > 0,
            "nonempty dashboard network tier has no interfaces"
        );
        let span = i64::from(tier) * BLOCK_SLOTS as i64;
        let first_start = *sparse.first_key_value().unwrap().0;
        let last = *sparse.last_key_value().unwrap().0;
        let len = usize::try_from((last - first_start) / span + 1)?;
        let tree_base = len.next_power_of_two();
        let mut blocks = vec![None; tree_base];
        for (ordinal, block_slot) in blocks.iter_mut().enumerate().take(len) {
            let start = first_start + i64::try_from(ordinal)? * span;
            if let Some(block) = sparse.get(&start).cloned() {
                anyhow::ensure!(
                    block.width == width,
                    "dashboard network block width is inconsistent"
                );
                *block_slot = Some(Arc::new(block));
            }
        }
        let mut result = Self {
            tier,
            width,
            blocks,
            head: 0,
            len,
            first_start,
            tree_base,
            tree: vec![NetworkState::default(); tree_base * 2 * width].into_boxed_slice(),
        };
        result.rebuild_tree();
        Ok(result)
    }

    fn empty(tier: i32, width: usize) -> Self {
        Self {
            tier,
            width,
            blocks: vec![None],
            head: 0,
            len: 0,
            first_start: 0,
            tree_base: 1,
            tree: vec![NetworkState::default(); 2 * width].into_boxed_slice(),
        }
    }

    fn first_start(&self) -> Option<i64> {
        (self.len > 0).then_some(self.first_start)
    }
    fn slot_len(&self) -> usize {
        self.len * BLOCK_SLOTS
    }
    fn any_present(states: &[NetworkState]) -> bool {
        states.iter().any(|state| state.present())
    }
    fn physical(&self, logical: usize) -> usize {
        debug_assert!(logical < self.len);
        (self.head + logical) % self.tree_base
    }
    fn block(&self, logical: usize) -> Option<&NetworkBlock> {
        self.blocks[self.physical(logical)].as_deref()
    }
    fn node_present(&self, node: usize) -> bool {
        Self::any_present(&self.tree[node * self.width..(node + 1) * self.width])
    }

    fn rebuild_tree(&mut self) {
        self.tree.fill(NetworkState::default());
        for physical in 0..self.tree_base {
            let Some(block) = &self.blocks[physical] else {
                continue;
            };
            for slot in 0..BLOCK_SLOTS {
                for (interface, source) in block.slot(slot).iter().copied().enumerate() {
                    self.tree[(self.tree_base + physical) * self.width + interface].merge(source);
                }
            }
        }
        for node in (1..self.tree_base).rev() {
            for interface in 0..self.width {
                let mut state = self.tree[node * 2 * self.width + interface];
                state.merge(self.tree[(node * 2 + 1) * self.width + interface]);
                self.tree[node * self.width + interface] = state;
            }
        }
    }

    fn update_leaf(&mut self, physical: usize) -> usize {
        let mut node = self.tree_base + physical;
        for interface in 0..self.width {
            self.tree[node * self.width + interface] = NetworkState::default();
        }
        if let Some(block) = &self.blocks[physical] {
            for slot in 0..BLOCK_SLOTS {
                for (interface, source) in block.slot(slot).iter().copied().enumerate() {
                    self.tree[node * self.width + interface].merge(source);
                }
            }
        }
        let mut touched = 1;
        while node > 1 {
            node /= 2;
            for interface in 0..self.width {
                let mut state = self.tree[node * 2 * self.width + interface];
                state.merge(self.tree[(node * 2 + 1) * self.width + interface]);
                self.tree[node * self.width + interface] = state;
            }
            touched += 1;
        }
        touched
    }

    fn grow(&mut self) {
        let ordered = (0..self.len)
            .map(|logical| self.blocks[self.physical(logical)].clone())
            .collect::<Vec<_>>();
        self.tree_base = self.tree_base.saturating_mul(2).max(1);
        self.blocks = vec![None; self.tree_base];
        for (logical, block) in ordered.into_iter().enumerate() {
            self.blocks[logical] = block;
        }
        self.head = 0;
        self.tree =
            vec![NetworkState::default(); self.tree_base * 2 * self.width].into_boxed_slice();
        self.rebuild_tree();
    }

    fn append(&mut self, block: Option<NetworkBlock>) -> usize {
        if self.len == self.tree_base {
            self.grow();
        }
        let physical = (self.head + self.len) % self.tree_base;
        debug_assert!(self.blocks[physical].is_none());
        self.blocks[physical] = block.map(Arc::new);
        self.len += 1;
        self.update_leaf(physical)
    }

    fn prepend(&mut self, block: Option<NetworkBlock>) -> usize {
        if self.len == self.tree_base {
            self.grow();
        }
        self.head = (self.head + self.tree_base - 1) % self.tree_base;
        debug_assert!(self.blocks[self.head].is_none());
        self.blocks[self.head] = block.map(Arc::new);
        self.len += 1;
        self.first_start -= i64::from(self.tier) * BLOCK_SLOTS as i64;
        self.update_leaf(self.head)
    }

    fn trim_empty_edges(&mut self) -> usize {
        let mut touched = 0;
        while self.len > 0 {
            let physical = self.head;
            if self.node_present(self.tree_base + physical) {
                break;
            }
            self.blocks[physical] = None;
            touched += self.update_leaf(physical);
            self.head = (self.head + 1) % self.tree_base;
            self.len -= 1;
            self.first_start += i64::from(self.tier) * BLOCK_SLOTS as i64;
        }
        while self.len > 0 {
            let physical = self.physical(self.len - 1);
            if self.node_present(self.tree_base + physical) {
                break;
            }
            self.blocks[physical] = None;
            touched += self.update_leaf(physical);
            self.len -= 1;
        }
        if self.len == 0 {
            self.head = 0;
            self.first_start = 0;
        }
        touched
    }

    fn set_block(&mut self, start: i64, replacement: Option<NetworkBlock>) -> usize {
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        debug_assert_eq!(start.rem_euclid(span), 0);
        debug_assert!(replacement
            .as_ref()
            .is_none_or(|block| block.start == start && block.width == self.width));
        if self.len == 0 {
            let Some(block) = replacement else {
                return 0;
            };
            self.first_start = start;
            return self.append(Some(block));
        }
        let end = self.first_start + i64::try_from(self.len).unwrap_or(i64::MAX) * span;
        if start >= self.first_start && start < end {
            let logical = usize::try_from((start - self.first_start) / span)
                .expect("nonnegative resident block ordinal");
            let physical = self.physical(logical);
            self.blocks[physical] = replacement.map(Arc::new);
            return self.update_leaf(physical) + self.trim_empty_edges();
        }
        let Some(block) = replacement else {
            return 0;
        };
        let mut block = Some(block);
        let mut touched = 0;
        if start >= end {
            let mut cursor = end;
            loop {
                touched += self.append((cursor == start).then(|| block.take().unwrap()));
                if cursor == start {
                    break;
                }
                cursor = cursor.saturating_add(span);
            }
        } else {
            while self.first_start > start {
                let next = self.first_start - span;
                touched += self.prepend((next == start).then(|| block.take().unwrap()));
            }
        }
        touched
    }

    fn find_present_physical(
        &self,
        node: usize,
        node_lo: usize,
        node_hi: usize,
        query_lo: usize,
        query_hi: usize,
        reverse: bool,
    ) -> Option<usize> {
        if node_hi <= query_lo || query_hi <= node_lo || !self.node_present(node) {
            return None;
        }
        if node_hi - node_lo == 1 {
            return Some(node_lo);
        }
        let middle = (node_lo + node_hi) / 2;
        if reverse {
            self.find_present_physical(node * 2 + 1, middle, node_hi, query_lo, query_hi, true)
                .or_else(|| {
                    self.find_present_physical(node * 2, node_lo, middle, query_lo, query_hi, true)
                })
        } else {
            self.find_present_physical(node * 2, node_lo, middle, query_lo, query_hi, false)
                .or_else(|| {
                    self.find_present_physical(
                        node * 2 + 1,
                        middle,
                        node_hi,
                        query_lo,
                        query_hi,
                        false,
                    )
                })
        }
    }

    fn first_last_present(&self) -> Option<(i64, i64)> {
        if self.len == 0 || !self.node_present(1) {
            return None;
        }
        let tail = (self.head + self.len).min(self.tree_base);
        let first_physical = self
            .find_present_physical(1, 0, self.tree_base, self.head, tail, false)
            .or_else(|| {
                let wrapped = (self.head + self.len).saturating_sub(self.tree_base);
                self.find_present_physical(1, 0, self.tree_base, 0, wrapped, false)
            })?;
        let wrapped = (self.head + self.len).saturating_sub(self.tree_base);
        let last_physical = self
            .find_present_physical(1, 0, self.tree_base, 0, wrapped, true)
            .or_else(|| self.find_present_physical(1, 0, self.tree_base, self.head, tail, true))?;
        let first_logical = (first_physical + self.tree_base - self.head) % self.tree_base;
        let last_logical = (last_physical + self.tree_base - self.head) % self.tree_base;
        let first_block = self.block(first_logical)?;
        let last_block = self.block(last_logical)?;
        let first_slot =
            (0..BLOCK_SLOTS).find(|slot| Self::any_present(first_block.slot(*slot)))?;
        let last_slot = (0..BLOCK_SLOTS).rfind(|slot| Self::any_present(last_block.slot(*slot)))?;
        Some((
            first_block.start + i64::try_from(first_slot).ok()? * i64::from(self.tier),
            last_block.start + i64::try_from(last_slot).ok()? * i64::from(self.tier),
        ))
    }

    fn merge_tree_node(&self, node: usize, target: &mut [NetworkState]) {
        for (interface, value) in target.iter_mut().enumerate() {
            value.merge(self.tree[node * self.width + interface]);
        }
    }

    fn merge_physical(&self, mut lo: usize, mut hi: usize, target: &mut [NetworkState]) {
        if lo >= hi {
            return;
        }
        lo += self.tree_base;
        hi += self.tree_base;
        let mut right = [0_usize; 32];
        let mut right_len = 0;
        while lo < hi {
            if lo & 1 == 1 {
                self.merge_tree_node(lo, target);
                lo += 1;
            }
            if hi & 1 == 1 {
                hi -= 1;
                right[right_len] = hi;
                right_len += 1;
            }
            lo /= 2;
            hi /= 2;
        }
        for index in (0..right_len).rev() {
            self.merge_tree_node(right[index], target);
        }
    }

    fn merge_block_range(&self, lo: usize, hi: usize, target: &mut [NetworkState]) {
        if lo >= hi {
            return;
        }
        let physical = (self.head + lo) % self.tree_base;
        let count = hi - lo;
        let first_count = count.min(self.tree_base - physical);
        self.merge_physical(physical, physical + first_count, target);
        if first_count < count {
            self.merge_physical(0, count - first_count, target);
        }
    }

    fn range_into(&self, lo: usize, hi: usize, target: &mut [NetworkState]) {
        debug_assert!(target.len() == self.width && hi <= self.slot_len());
        if lo >= hi {
            return;
        }
        let first_block = lo / BLOCK_SLOTS;
        let last_block = (hi - 1) / BLOCK_SLOTS;
        if first_block == last_block {
            if let Some(block) = self.block(first_block) {
                for slot in lo % BLOCK_SLOTS..((hi - 1) % BLOCK_SLOTS + 1) {
                    for (value, source) in target.iter_mut().zip(block.slot(slot)) {
                        value.merge(*source);
                    }
                }
            }
            return;
        }
        if let Some(block) = self.block(first_block) {
            for slot in lo % BLOCK_SLOTS..BLOCK_SLOTS {
                for (value, source) in target.iter_mut().zip(block.slot(slot)) {
                    value.merge(*source);
                }
            }
        }
        self.merge_block_range(first_block + 1, last_block, target);
        if let Some(block) = self.block(last_block) {
            for slot in 0..((hi - 1) % BLOCK_SLOTS + 1) {
                for (value, source) in target.iter_mut().zip(block.slot(slot)) {
                    value.merge(*source);
                }
            }
        }
    }
}
#[derive(Clone, Debug)]
struct NetworkIndex {
    interfaces: Arc<[String]>,
    tiers: BTreeMap<i32, NetworkTier>,
}

impl NetworkIndex {
    fn from_blocks(
        interfaces: Arc<[String]>,
        blocks: BTreeMap<(i32, i64), NetworkBlock>,
    ) -> Result<Self> {
        let mut by_tier = BTreeMap::<i32, BTreeMap<i64, NetworkBlock>>::new();
        for ((tier, start), block) in blocks {
            by_tier.entry(tier).or_default().insert(start, block);
        }
        let width = interfaces.len();
        let tiers = by_tier
            .into_iter()
            .map(|(tier, blocks)| Ok((tier, NetworkTier::from_sparse(tier, width, blocks)?)))
            .collect::<Result<_>>()?;
        Ok(Self { interfaces, tiers })
    }

    fn apply_blocks(&mut self, changes: Vec<(i32, i64, Option<NetworkBlock>)>) -> usize {
        let mut touched = 0;
        let width = self.interfaces.len();
        for (tier, start, replacement) in changes {
            if replacement.is_some() {
                self.tiers
                    .entry(tier)
                    .or_insert_with(|| NetworkTier::empty(tier, width));
            }
            let remove = if let Some(owner) = self.tiers.get_mut(&tier) {
                touched += owner.set_block(start, replacement);
                owner.len == 0
            } else {
                false
            };
            if remove {
                self.tiers.remove(&tier);
            }
        }
        touched
    }

    fn fold_states(&self, start: i64, end: i64, requested_step: i64) -> NetworkFold {
        let width = self.interfaces.len();
        if start < 0 || start > end || requested_step <= 0 {
            return NetworkFold {
                predecessor: vec![NetworkState::default(); width],
                points: Vec::new(),
            };
        }
        let mut predecessor = vec![NetworkState::default(); width];
        let mut bins = BTreeMap::<(i64, i64), Vec<NetworkState>>::new();
        for tier in self.tiers.values() {
            let Some((retained_first, retained_last)) = tier.first_last_present() else {
                continue;
            };
            let source_tier = i64::from(tier.tier);
            let first_source = retained_first.max(floor_multiple(start, source_tier));
            let last_source = retained_last.min(floor_multiple(end, source_tier));
            let base = tier.first_start().unwrap();
            if retained_first < first_source {
                let lo = ceil_div(retained_first - base, source_tier)
                    .clamp(0, tier.slot_len() as i64) as usize;
                let hi = ceil_div(first_source - base, source_tier).clamp(0, tier.slot_len() as i64)
                    as usize;
                tier.range_into(lo, hi, &mut predecessor);
            }
            if first_source > last_source {
                continue;
            }
            let effective = requested_step.max(source_tier);
            let mut bucket = floor_multiple(first_source, effective);
            let last_bucket = floor_multiple(last_source, effective);
            while bucket <= last_bucket {
                let lo = ceil_div(first_source.max(bucket) - base, source_tier)
                    .clamp(0, tier.slot_len() as i64) as usize;
                let hi_source = (last_source + source_tier).min(bucket.saturating_add(effective));
                let hi = ceil_div(hi_source - base, source_tier).clamp(0, tier.slot_len() as i64)
                    as usize;
                if lo < hi {
                    let target = bins
                        .entry((bucket, effective))
                        .or_insert_with(|| vec![NetworkState::default(); width]);
                    tier.range_into(lo, hi, target);
                }
                bucket = bucket.saturating_add(effective);
            }
        }
        let points = bins
            .into_iter()
            .filter_map(|((bucket, step), states)| {
                NetworkTier::any_present(&states).then_some(NetworkChartState {
                    bucket,
                    step,
                    states,
                })
            })
            .collect();
        NetworkFold {
            predecessor,
            points,
        }
    }
}

#[derive(Clone, Debug)]
struct NetworkOwner {
    generation: i64,
    revision: i64,
    overlay_blocks: BTreeSet<(i32, i64)>,
    index: NetworkIndex,
}

fn valid_traffic_tier(tier: i32) -> bool {
    matches!(tier, 60 | 3_600 | 10_800 | 21_600 | 86_400)
}

#[derive(Clone, Copy, Debug, Default)]
struct TrafficState {
    present: bool,
    rx_valid_count: i64,
    tx_valid_count: i64,
    rx_bytes: i64,
    tx_bytes: i64,
}

impl TrafficState {
    fn merge(&mut self, right: Self) {
        if !right.present {
            return;
        }
        self.present = true;
        self.rx_valid_count = self.rx_valid_count.saturating_add(right.rx_valid_count);
        self.tx_valid_count = self.tx_valid_count.saturating_add(right.tx_valid_count);
        self.rx_bytes = self.rx_bytes.saturating_add(right.rx_bytes);
        self.tx_bytes = self.tx_bytes.saturating_add(right.tx_bytes);
    }

    fn from_columns(
        rx_valid_count: Option<i64>,
        tx_valid_count: Option<i64>,
        rx_bytes: Option<i64>,
        tx_bytes: Option<i64>,
    ) -> Result<Self> {
        let present = rx_valid_count.is_some() || tx_valid_count.is_some();
        anyhow::ensure!(
            rx_valid_count.is_some() == tx_valid_count.is_some(),
            "dashboard traffic slot has one-sided presence"
        );
        if !present {
            anyhow::ensure!(
                rx_bytes.is_none() && tx_bytes.is_none(),
                "dashboard absent traffic slot carries bytes"
            );
            return Ok(Self::default());
        }
        let rx_valid_count = rx_valid_count.expect("checked traffic presence");
        let tx_valid_count = tx_valid_count.expect("checked traffic presence");
        anyhow::ensure!(
            rx_valid_count >= 0
                && tx_valid_count >= 0
                && (rx_valid_count == 0) == rx_bytes.is_none()
                && (tx_valid_count == 0) == tx_bytes.is_none()
                && rx_bytes.is_none_or(|value| value >= 0)
                && tx_bytes.is_none_or(|value| value >= 0),
            "dashboard traffic slot validity disagrees with its bytes"
        );
        Ok(Self {
            present: true,
            rx_valid_count,
            tx_valid_count,
            rx_bytes: rx_bytes.unwrap_or_default(),
            tx_bytes: tx_bytes.unwrap_or_default(),
        })
    }
}

#[derive(Clone, Debug)]
struct TrafficBlock {
    start: i64,
    slots: [TrafficState; BLOCK_SLOTS],
}

impl TrafficBlock {
    fn empty(start: i64) -> Self {
        Self {
            start,
            slots: [TrafficState::default(); BLOCK_SLOTS],
        }
    }
}

#[derive(Debug)]
struct TrafficBlockRow {
    tier: i32,
    start: i64,
    revision: i64,
    rx_valid_counts: Vec<Option<i64>>,
    tx_valid_counts: Vec<Option<i64>>,
    rx_bytes: Vec<Option<i64>>,
    tx_bytes: Vec<Option<i64>>,
}

impl<'r> FromRow<'r, PgRow> for TrafficBlockRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            tier: row.try_get("source_bucket_secs")?,
            start: row.try_get("block_start_unix")?,
            revision: row.try_get("published_revision")?,
            rx_valid_counts: row.try_get("rx_valid_counts")?,
            tx_valid_counts: row.try_get("tx_valid_counts")?,
            rx_bytes: row.try_get("rx_bytes")?,
            tx_bytes: row.try_get("tx_bytes")?,
        })
    }
}

impl TrafficBlockRow {
    fn into_block(self, head: &TrafficHead) -> Result<(i32, TrafficBlock)> {
        anyhow::ensure!(
            self.revision <= head.revision && valid_traffic_tier(self.tier),
            "dashboard traffic block is outside its head fence"
        );
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        anyhow::ensure!(
            self.start.rem_euclid(span) == 0,
            "dashboard traffic block key is invalid"
        );
        for len in [
            self.rx_valid_counts.len(),
            self.tx_valid_counts.len(),
            self.rx_bytes.len(),
            self.tx_bytes.len(),
        ] {
            anyhow::ensure!(
                len == BLOCK_SLOTS,
                "dashboard traffic block does not contain sixteen slots"
            );
        }
        let mut block = TrafficBlock::empty(self.start);
        for slot in 0..BLOCK_SLOTS {
            block.slots[slot] = TrafficState::from_columns(
                self.rx_valid_counts[slot],
                self.tx_valid_counts[slot],
                self.rx_bytes[slot],
                self.tx_bytes[slot],
            )?;
        }
        Ok((self.tier, block))
    }
}

#[derive(Debug)]
struct TrafficOverlayRow {
    tier: i32,
    block_start: i64,
    bucket_start: i64,
    rx_valid_count: i64,
    tx_valid_count: i64,
    rx_bytes: Option<i64>,
    tx_bytes: Option<i64>,
}

impl<'r> FromRow<'r, PgRow> for TrafficOverlayRow {
    fn from_row(row: &'r PgRow) -> std::result::Result<Self, sqlx::Error> {
        Ok(Self {
            tier: row.try_get("source_bucket_secs")?,
            block_start: row.try_get("block_start_unix")?,
            bucket_start: row.try_get("bucket_start_unix")?,
            rx_valid_count: row.try_get("rx_valid_count")?,
            tx_valid_count: row.try_get("tx_valid_count")?,
            rx_bytes: row.try_get("rx_bytes")?,
            tx_bytes: row.try_get("tx_bytes")?,
        })
    }
}

impl TrafficOverlayRow {
    fn apply(self, blocks: &mut BTreeMap<(i32, i64), TrafficBlock>) -> Result<()> {
        let span = i64::from(self.tier) * BLOCK_SLOTS as i64;
        anyhow::ensure!(
            valid_traffic_tier(self.tier)
                && self.block_start.rem_euclid(span) == 0
                && self.bucket_start >= self.block_start
                && self.bucket_start < self.block_start + span
                && (self.bucket_start - self.block_start).rem_euclid(i64::from(self.tier)) == 0,
            "dashboard traffic overlay-row key is invalid"
        );
        let slot = usize::try_from((self.bucket_start - self.block_start) / i64::from(self.tier))?;
        let state = TrafficState::from_columns(
            Some(self.rx_valid_count),
            Some(self.tx_valid_count),
            self.rx_bytes,
            self.tx_bytes,
        )?;
        blocks
            .entry((self.tier, self.block_start))
            .or_insert_with(|| TrafficBlock::empty(self.block_start))
            .slots[slot] = state;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
struct TrafficPoint {
    bucket: i64,
    step: i64,
    state: TrafficState,
}

#[derive(Clone, Debug, Default)]
struct TrafficIndex {
    blocks: BTreeMap<(i32, i64), TrafficBlock>,
}

impl TrafficIndex {
    fn from_blocks(blocks: BTreeMap<(i32, i64), TrafficBlock>) -> Self {
        Self { blocks }
    }

    fn apply_blocks(&mut self, changes: Vec<(i32, i64, Option<TrafficBlock>)>) {
        for (tier, start, replacement) in changes {
            if let Some(block) = replacement {
                self.blocks.insert((tier, start), block);
            } else {
                self.blocks.remove(&(tier, start));
            }
        }
    }

    fn fold(&self, start: i64, end: i64, requested_step: i64) -> Vec<TrafficPoint> {
        if start < 0 || start > end || requested_step <= 0 {
            return Vec::new();
        }
        let mut points = BTreeMap::<(i64, i64), TrafficState>::new();
        for source_tier in [60, 3_600, 10_800, 21_600, 86_400] {
            let tier = i64::from(source_tier);
            let span = tier * BLOCK_SLOTS as i64;
            let first_block = floor_multiple(start, span);
            let last_block = floor_multiple(end, span);
            for (_, block) in self
                .blocks
                .range((source_tier, first_block)..=(source_tier, last_block))
            {
                let effective = requested_step.max(tier);
                for (slot, state) in block.slots.iter().copied().enumerate() {
                    if !state.present {
                        continue;
                    }
                    let source_bucket = block.start.saturating_add(
                        i64::try_from(slot).unwrap_or_default().saturating_mul(tier),
                    );
                    if source_bucket < start || source_bucket > end {
                        continue;
                    }
                    points
                        .entry((floor_multiple(source_bucket, effective), effective))
                        .or_default()
                        .merge(state);
                }
            }
        }
        points
            .into_iter()
            .map(|((bucket, step), state)| TrafficPoint {
                bucket,
                step,
                state,
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
struct TrafficOwner {
    generation: i64,
    revision: i64,
    overlay_blocks: BTreeSet<(i32, i64)>,
    source_kinds: Arc<[String]>,
    interfaces: Arc<[String]>,
    index: TrafficIndex,
}

#[derive(Clone)]
struct ResourceOverlayTarget {
    owner: Arc<RwLock<ResourceOwner>>,
    generation: i64,
    revision: i64,
    prior_blocks: BTreeSet<(i32, i64)>,
}

#[derive(Clone)]
struct NetworkOverlayTarget {
    owner: Arc<RwLock<NetworkOwner>>,
    generation: i64,
    revision: i64,
    interfaces: Arc<[String]>,
    prior_blocks: BTreeSet<(i32, i64)>,
}

#[derive(Clone)]
struct TrafficOverlayTarget {
    owner: Arc<RwLock<TrafficOwner>>,
    generation: i64,
    revision: i64,
    prior_blocks: BTreeSet<(i32, i64)>,
}

fn overlay_target_client_ids<T>(targets: &HashMap<String, T>) -> Vec<String> {
    let mut client_ids = targets.keys().cloned().collect::<Vec<_>>();
    client_ids.sort_unstable();
    client_ids
}

fn resource_block_change_is_contiguous(
    owner: &ResourceOwner,
    head: &ResourceHead,
    notice_generation: i64,
    previous_revision: i64,
    notice_revision: i64,
) -> bool {
    head.change == "block"
        && head.generation == notice_generation
        && head.revision == notice_revision
        && owner.generation == head.generation
        && owner.revision == previous_revision
}

fn network_block_change_is_contiguous(
    owner: &NetworkOwner,
    head: &NetworkHead,
    notice_generation: i64,
    previous_revision: i64,
    notice_revision: i64,
) -> bool {
    head.change == "block"
        && head.generation == notice_generation
        && head.revision == notice_revision
        && owner.generation == head.generation
        && owner.revision == previous_revision
        && owner.index.interfaces.as_ref() == head.interfaces.as_ref()
}

fn traffic_block_change_is_contiguous(
    owner: &TrafficOwner,
    head: &TrafficHead,
    notice_generation: i64,
    previous_revision: i64,
    notice_revision: i64,
) -> bool {
    head.change == "block"
        && head.generation == notice_generation
        && head.revision == notice_revision
        && owner.generation == head.generation
        && owner.revision == previous_revision
        && owner.source_kinds.as_ref() == head.source_kinds.as_ref()
        && owner.interfaces.as_ref() == head.interfaces.as_ref()
}

fn block_notice_is_waiting_for_successor(
    owner_generation: i64,
    owner_revision: i64,
    head_generation: i64,
    head_revision: i64,
    head_change: &str,
    notice_generation: i64,
    previous_revision: i64,
    notice_revision: i64,
) -> bool {
    head_change == "block"
        && owner_generation == notice_generation
        && owner_revision == previous_revision
        && head_generation == notice_generation
        && head_revision > notice_revision
}

#[derive(Clone, Debug)]
struct NetworkChartState {
    bucket: i64,
    step: i64,
    states: Vec<NetworkState>,
}

#[derive(Clone, Debug)]
struct NetworkFold {
    predecessor: Vec<NetworkState>,
    points: Vec<NetworkChartState>,
}

#[derive(Clone, Debug, Default)]
struct NetworkPoint {
    bucket: i64,
    step: i64,
    sample_count: i64,
    rx_delta: i64,
    tx_delta: i64,
    rx_bps: f64,
    tx_bps: f64,
    latest: i64,
    interfaces: Vec<String>,
}

fn derive_network_points(
    index: &NetworkIndex,
    fold: NetworkFold,
    selection: &NetworkRateInterfaceSelection,
    client_id: &str,
    limit: usize,
) -> Vec<NetworkPoint> {
    let mut derived = BTreeMap::<(i64, i64), NetworkPoint>::new();
    for (interface, name) in index.interfaces.iter().enumerate() {
        if !selection.allows(client_id, name) {
            continue;
        }
        let present = fold
            .points
            .iter()
            .filter_map(|point| {
                let state = point.states[interface];
                state.present().then_some((point.bucket, point.step, state))
            })
            .collect::<Vec<_>>();
        let keep_from = present.len().saturating_sub(limit);
        let predecessor = if keep_from > 0 {
            Some(present[keep_from - 1])
        } else {
            let state = fold.predecessor[interface];
            state.present().then_some((i64::MIN, 0, state))
        };
        let mut ordered = present[keep_from..]
            .iter()
            .copied()
            .map(|(bucket, step, state)| (false, bucket, step, state))
            .chain(predecessor.map(|(bucket, step, state)| (true, bucket, step, state)))
            .collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            left.3
                .latest
                .cmp(&right.3.latest)
                .then_with(|| right.0.cmp(&left.0))
                .then_with(|| left.1.cmp(&right.1))
                .then_with(|| left.2.cmp(&right.2))
        });
        let mut previous = None;
        for (is_predecessor, bucket, step, state) in ordered {
            let before = previous.replace(state);
            if is_predecessor {
                continue;
            }
            let Some(before) = before else {
                continue;
            };
            if state.latest <= before.latest
                || state.rx_epoch != before.rx_epoch
                || state.tx_epoch != before.tx_epoch
                || state.rx < before.rx
                || state.tx < before.tx
            {
                continue;
            }
            let rx = state.rx - before.rx;
            let tx = state.tx - before.tx;
            let seconds = (state.latest - before.latest).max(1) as f64;
            let point = derived
                .entry((bucket, step))
                .or_insert_with(|| NetworkPoint {
                    bucket,
                    step,
                    ..NetworkPoint::default()
                });
            point.sample_count = point.sample_count.saturating_add(state.count);
            point.rx_delta = point.rx_delta.saturating_add(rx);
            point.tx_delta = point.tx_delta.saturating_add(tx);
            point.rx_bps += rx as f64 * 8.0 / seconds;
            point.tx_bps += tx as f64 * 8.0 / seconds;
            point.latest = point.latest.max(state.latest);
            point.interfaces.push(name.clone());
        }
    }
    let mut points = derived.into_values().collect::<Vec<_>>();
    if points.len() > limit {
        points.drain(..points.len() - limit);
    }
    points
}

fn network_view(client_id: &str, point: &NetworkPoint) -> Result<TelemetryNetworkRateView> {
    anyhow::ensure!(
        !point.interfaces.is_empty()
            && point.rx_bps.is_finite()
            && point.tx_bps.is_finite()
            && point.rx_bps >= 0.0
            && point.tx_bps >= 0.0,
        "dashboard resident network output is invalid"
    );
    let bucket_start = unix_timestamp(point.bucket, "network bucket")?;
    let latest = unix_timestamp(point.latest, "network observation")?;
    Ok(TelemetryNetworkRateView {
        client_id: client_id.to_string(),
        interface: point.interfaces[0].clone(),
        bucket_start,
        bucket_secs: i32::try_from(point.step)?,
        sample_count: i32::try_from(point.sample_count.min(i64::from(i32::MAX)))?,
        rx_bytes_avg: 0,
        tx_bytes_avg: 0,
        latest_observed_at: latest.clone(),
        rx_bytes_delta: point.rx_delta,
        tx_bytes_delta: point.tx_delta,
        rx_bps_avg: point.rx_bps,
        tx_bps_avg: point.tx_bps,
        updated_at: latest,
    })
}

#[derive(Clone, Debug, Default)]
struct ResidentFleet {
    resources: HashMap<String, Arc<RwLock<ResourceOwner>>>,
    networks: HashMap<String, Arc<RwLock<NetworkOwner>>>,
    traffics: HashMap<String, Arc<RwLock<TrafficOwner>>>,
}

#[derive(Clone)]
pub(crate) struct DashboardTelemetryResident {
    snapshot: Arc<RwLock<Arc<ResidentFleet>>>,
}

impl DashboardTelemetryResident {
    fn snapshot(&self) -> Arc<ResidentFleet> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn resource_projection(
        &self,
        points_per_client: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        step_secs: i32,
        client_ids: &[String],
        resource_metric: &str,
        curve_client_ids_in_label_order: &[String],
        curve_top_limit: usize,
    ) -> Result<DashboardTelemetryResourceProjection> {
        let metric = ResourceMetric::parse(resource_metric)?;
        let limit = points_per_client.clamp(2, 1_440) as usize;
        let step = i64::from(normalized_step(step_secs));
        let start = start_unix.unwrap_or(0).min(i64::MAX as u64) as i64;
        let end = end_unix.unwrap_or(i64::MAX as u64).min(i64::MAX as u64) as i64;
        let requested = client_ids.iter().collect::<BTreeSet<_>>();
        let snapshot = self.snapshot();
        let mut histories = BTreeMap::<String, Vec<ResourcePoint>>::new();
        for client_id in requested {
            let owner = snapshot
                .resources
                .get(client_id.as_str())
                .with_context(|| {
                    format!("dashboard resource resident owner is missing for {client_id}")
                })?;
            let owner = owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut points = owner.index.fold(start, end, step);
            if points.len() > limit {
                points.drain(..points.len() - limit);
            }
            histories.insert(client_id.clone(), points);
        }
        #[derive(Debug)]
        struct Summary {
            first: Option<i64>,
            step: i64,
            current: Option<f64>,
            peak: Option<f64>,
            latest: i64,
        }
        let summaries = histories
            .iter()
            .map(|(client_id, points)| {
                let current = points
                    .iter()
                    .rev()
                    .find_map(|point| metric.values(point.state).map(|values| values.0));
                let peak = points
                    .iter()
                    .filter_map(|point| metric.values(point.state).map(|values| values.1))
                    .reduce(|left, right| {
                        if metric == ResourceMetric::Disk {
                            left.min(right)
                        } else {
                            left.max(right)
                        }
                    });
                (
                    client_id.clone(),
                    Summary {
                        first: points.first().map(|point| point.bucket),
                        step: points.iter().map(|point| point.step).max().unwrap_or(step),
                        current,
                        peak,
                        latest: points
                            .iter()
                            .map(|point| point.state.latest)
                            .max()
                            .unwrap_or(0),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let label_ranks = curve_client_ids_in_label_order
            .iter()
            .enumerate()
            .map(|(rank, client_id)| (client_id.as_str(), rank))
            .collect::<HashMap<_, _>>();
        let mut candidates = summaries
            .iter()
            .filter(|(client_id, summary)| {
                label_ranks.contains_key(client_id.as_str()) && summary.current.is_some()
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            metric
                .top_score(right.1.peak.unwrap())
                .total_cmp(&metric.top_score(left.1.peak.unwrap()))
                .then_with(|| label_ranks[left.0.as_str()].cmp(&label_ranks[right.0.as_str()]))
        });
        let top = candidates
            .into_iter()
            .take(curve_top_limit)
            .map(|(client_id, _)| client_id.clone())
            .collect::<HashSet<_>>();
        let mut selected = Vec::<(i64, String, TelemetryRollupView)>::new();
        for (client_id, points) in &histories {
            if top.contains(client_id) {
                for point in points {
                    selected.push((
                        point.bucket,
                        client_id.clone(),
                        resource_view(client_id, *point, metric, metric.values(point.state))?,
                    ));
                }
            } else {
                let summary = &summaries[client_id];
                if let Some(bucket) = summary.first {
                    let point = ResourcePoint {
                        step: summary.step,
                        bucket,
                        state: ResourceSummary {
                            latest: summary.latest,
                            ..ResourceSummary::default()
                        },
                    };
                    selected.push((
                        bucket,
                        client_id.clone(),
                        resource_view(client_id, point, metric, summary.current.zip(summary.peak))?,
                    ));
                }
            }
        }
        selected.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        Ok(DashboardTelemetryResourceProjection {
            rollups: selected.into_iter().map(|(_, _, view)| view).collect(),
            latest_rollups: Vec::new(),
        })
    }

    pub(crate) fn network_projection(
        &self,
        points_per_client: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<DashboardTelemetryNetworkProjection> {
        let client_ids = selection.client_ids();
        if client_ids.is_empty() {
            return Ok(DashboardTelemetryNetworkProjection {
                rates: Vec::new(),
                fleet_rates: Some(Vec::new()),
                latest_rates: Vec::new(),
                interfaces_by_rate: HashMap::new(),
            });
        }
        let limit = points_per_client.clamp(2, 1_440) as usize;
        let step = i64::from(normalized_step(step_secs));
        let start = start_unix.unwrap_or(0).min(i64::MAX as u64) as i64;
        let end = end_unix.unwrap_or(i64::MAX as u64).min(i64::MAX as u64) as i64;
        let snapshot = self.snapshot();
        let mut histories = BTreeMap::<String, Vec<NetworkPoint>>::new();
        for client_id in client_ids.into_iter().collect::<BTreeSet<_>>() {
            let owner = snapshot.networks.get(&client_id).with_context(|| {
                format!("dashboard network resident owner is missing for {client_id}")
            })?;
            let owner = owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let fold = owner.index.fold_states(start, end, step);
            histories.insert(
                client_id.clone(),
                derive_network_points(&owner.index, fold, selection, &client_id, limit),
            );
        }
        let mut fleet = BTreeMap::<(i64, i64), NetworkPoint>::new();
        for points in histories.values() {
            for point in points {
                let aggregate =
                    fleet
                        .entry((point.bucket, point.step))
                        .or_insert_with(|| NetworkPoint {
                            bucket: point.bucket,
                            step: point.step,
                            interfaces: vec!["__fleet__".to_string()],
                            ..NetworkPoint::default()
                        });
                aggregate.sample_count = aggregate.sample_count.saturating_add(point.sample_count);
                aggregate.rx_delta = aggregate.rx_delta.saturating_add(point.rx_delta);
                aggregate.tx_delta = aggregate.tx_delta.saturating_add(point.tx_delta);
                aggregate.rx_bps += point.rx_bps;
                aggregate.tx_bps += point.tx_bps;
                aggregate.latest = aggregate.latest.max(point.latest);
            }
        }
        let fleet_rates = fleet
            .values()
            .map(|point| network_view("__fleet__", point))
            .collect::<Result<Vec<_>>>()?;
        let mut rates_with_key = Vec::new();
        let mut interfaces_by_rate = HashMap::new();
        for (client_id, points) in &histories {
            for point in points {
                let view = network_view(client_id, point)?;
                interfaces_by_rate.insert(
                    (client_id.clone(), view.bucket_start.clone()),
                    point.interfaces.clone(),
                );
                rates_with_key.push((point.bucket, client_id.clone(), view));
            }
            if let Some(point) = points.last() {
                let bucket = unix_timestamp(point.bucket, "network history latest bucket")?;
                interfaces_by_rate.insert((client_id.clone(), bucket), point.interfaces.clone());
            }
        }
        rates_with_key
            .sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        Ok(DashboardTelemetryNetworkProjection {
            rates: rates_with_key
                .into_iter()
                .map(|(_, _, view)| view)
                .collect(),
            fleet_rates: Some(fleet_rates),
            latest_rates: Vec::new(),
            interfaces_by_rate,
        })
    }

    pub(crate) fn traffic_projection(
        &self,
        points_per_client: i64,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        step_secs: i32,
        client_ids: &[String],
        client_ids_in_label_order: &[String],
        top_limit: usize,
    ) -> Result<DashboardTelemetryTrafficProjection> {
        let limit = points_per_client.clamp(2, 1_440) as usize;
        let step = i64::from(normalized_step(step_secs));
        let start = start_unix.unwrap_or(0).min(i64::MAX as u64) as i64;
        let end = end_unix.unwrap_or(i64::MAX as u64).min(i64::MAX as u64) as i64;
        let snapshot = self.snapshot();
        let mut histories = BTreeMap::<String, Vec<TrafficPoint>>::new();
        let mut stream_names = BTreeMap::<String, Vec<String>>::new();
        for client_id in client_ids.iter().collect::<BTreeSet<_>>() {
            let owner = snapshot.traffics.get(client_id.as_str()).with_context(|| {
                format!("dashboard traffic resident owner is missing for {client_id}")
            })?;
            let owner = owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut points = owner.index.fold(start, end, step);
            if points.len() > limit {
                points.drain(..points.len() - limit);
            }
            let names = owner
                .source_kinds
                .iter()
                .zip(owner.interfaces.iter())
                .map(|(source_kind, interface)| {
                    if source_kind == "tunnel" {
                        format!("tunnel:{interface}")
                    } else {
                        interface.clone()
                    }
                })
                .collect();
            stream_names.insert(client_id.clone(), names);
            histories.insert(client_id.clone(), points);
        }

        let label_ranks = client_ids_in_label_order
            .iter()
            .enumerate()
            .map(|(rank, client_id)| (client_id.as_str(), rank))
            .collect::<HashMap<_, _>>();
        let mut candidates = histories
            .iter()
            .filter(|(client_id, points)| {
                label_ranks.contains_key(client_id.as_str()) && !points.is_empty()
            })
            .map(|(client_id, points)| {
                let bytes = points.iter().fold(0_i128, |total, point| {
                    total + i128::from(point.state.rx_bytes) + i128::from(point.state.tx_bytes)
                });
                (client_id, bytes)
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| label_ranks[left.0.as_str()].cmp(&label_ranks[right.0.as_str()]))
        });
        let client_ids_in_rank_order = candidates
            .into_iter()
            .take(top_limit)
            .map(|(client_id, _)| client_id.clone())
            .collect::<Vec<_>>();
        let top = client_ids_in_rank_order
            .iter()
            .cloned()
            .collect::<HashSet<_>>();

        let mut fleet = BTreeMap::<(i64, i64), TrafficState>::new();
        let mut client_points = Vec::new();
        for (client_id, points) in &histories {
            for point in points {
                fleet
                    .entry((point.bucket, point.step))
                    .or_default()
                    .merge(point.state);
                if top.contains(client_id) {
                    client_points.push(DashboardTelemetryTrafficPoint {
                        client_id: client_id.clone(),
                        bucket_start: unix_timestamp(point.bucket, "traffic bucket")?,
                        rx_bytes: (point.state.rx_valid_count > 0).then_some(point.state.rx_bytes),
                        tx_bytes: (point.state.tx_valid_count > 0).then_some(point.state.tx_bytes),
                    });
                }
            }
        }
        client_points.sort_by(|left, right| {
            left.bucket_start
                .cmp(&right.bucket_start)
                .then_with(|| left.client_id.cmp(&right.client_id))
        });
        let fleet_points = fleet
            .into_iter()
            .map(|((bucket, _step), state)| {
                Ok(DashboardTelemetryTrafficPoint {
                    client_id: "__fleet__".to_string(),
                    bucket_start: unix_timestamp(bucket, "fleet traffic bucket")?,
                    rx_bytes: (state.rx_valid_count > 0).then_some(state.rx_bytes),
                    tx_bytes: (state.tx_valid_count > 0).then_some(state.tx_bytes),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let interfaces_by_client = top
            .into_iter()
            .map(|client_id| {
                let interfaces = stream_names.remove(&client_id).unwrap_or_default();
                (client_id, interfaces)
            })
            .collect();
        Ok(DashboardTelemetryTrafficProjection {
            client_points,
            fleet_points,
            interfaces_by_client,
            client_ids_in_rank_order,
        })
    }

    #[cfg(test)]
    pub(crate) fn empty_for_tests() -> Self {
        Self {
            snapshot: Arc::new(RwLock::new(Arc::new(ResidentFleet::default()))),
        }
    }

    #[cfg(test)]
    pub(crate) fn revisions_for_test(&self, client_id: &str) -> Option<(i64, i64, i64)> {
        let snapshot = self.snapshot();
        let resource = snapshot.resources.get(client_id)?;
        let network = snapshot.networks.get(client_id)?;
        let traffic = snapshot.traffics.get(client_id)?;
        let resource_revision = resource
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision;
        let network_revision = network
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision;
        let traffic_revision = traffic
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .revision;
        Some((resource_revision, network_revision, traffic_revision))
    }
}

fn normalized_step(step_secs: i32) -> i32 {
    step_secs.max(60).saturating_add(59) / 60 * 60
}

async fn load_resource_owner(
    listener: &mut PgListener,
    client_id: &str,
    head: &ResourceHead,
) -> Result<ResourceOwner> {
    let rows = sqlx::query_as::<_, ResourceBlockRow>(RESOURCE_BLOCKS_SQL)
        .bind(client_id)
        .bind(head.generation)
        .bind(head.revision)
        .fetch_all(&mut *listener)
        .await?;
    let mut blocks = BTreeMap::new();
    for row in rows {
        let (tier, block) = row.into_block(head)?;
        anyhow::ensure!(
            blocks.insert((tier, block.start), block).is_none(),
            "dashboard resource block key is duplicated"
        );
    }
    anyhow::ensure!(
        head.first_unix.is_none() || !blocks.is_empty(),
        "dashboard resource head emptiness disagrees with its F16 generation"
    );
    Ok(ResourceOwner {
        generation: head.generation,
        revision: head.revision,
        overlay_blocks: BTreeSet::new(),
        index: ResourceIndex::from_blocks(blocks)?,
    })
}

async fn load_network_owner(
    listener: &mut PgListener,
    client_id: &str,
    head: &NetworkHead,
) -> Result<NetworkOwner> {
    let rows = sqlx::query_as::<_, NetworkBlockRow>(NETWORK_BLOCKS_SQL)
        .bind(client_id)
        .bind(head.generation)
        .bind(head.revision)
        .fetch_all(&mut *listener)
        .await?;
    let mut blocks = BTreeMap::new();
    for row in rows {
        let (tier, block) = row.into_block(head)?;
        anyhow::ensure!(
            blocks.insert((tier, block.start), block).is_none(),
            "dashboard network block key is duplicated"
        );
    }
    anyhow::ensure!(
        head.first_unix.is_none() || !blocks.is_empty(),
        "dashboard network head emptiness disagrees with its F16 generation"
    );
    Ok(NetworkOwner {
        generation: head.generation,
        revision: head.revision,
        overlay_blocks: BTreeSet::new(),
        index: NetworkIndex::from_blocks(Arc::clone(&head.interfaces), blocks)?,
    })
}

async fn load_traffic_owner(
    listener: &mut PgListener,
    client_id: &str,
    head: &TrafficHead,
) -> Result<TrafficOwner> {
    let rows = sqlx::query_as::<_, TrafficBlockRow>(TRAFFIC_BLOCKS_SQL)
        .bind(client_id)
        .bind(head.generation)
        .bind(head.revision)
        .fetch_all(&mut *listener)
        .await?;
    let mut blocks = BTreeMap::new();
    for row in rows {
        let (tier, block) = row.into_block(head)?;
        anyhow::ensure!(
            blocks.insert((tier, block.start), block).is_none(),
            "dashboard traffic block key is duplicated"
        );
    }
    anyhow::ensure!(
        head.first_unix.is_none() || !blocks.is_empty(),
        "dashboard traffic head emptiness disagrees with its F16 generation"
    );
    Ok(TrafficOwner {
        generation: head.generation,
        revision: head.revision,
        overlay_blocks: BTreeSet::new(),
        source_kinds: Arc::clone(&head.source_kinds),
        interfaces: Arc::clone(&head.interfaces),
        index: TrafficIndex::from_blocks(blocks),
    })
}

async fn load_fenced_client(
    listener: &mut PgListener,
    client_id: &str,
) -> Result<Option<(ClientHeads, ResourceOwner, NetworkOwner, TrafficOwner)>> {
    loop {
        let Some(before) = load_optional_client_heads(listener, client_id).await? else {
            return Ok(None);
        };
        let resource = load_resource_owner(listener, client_id, &before.resource).await;
        let network = load_network_owner(listener, client_id, &before.network).await;
        let traffic = load_traffic_owner(listener, client_id, &before.traffic).await;
        let after = load_optional_client_heads(listener, client_id).await?;
        let Some(after) = after else {
            return Ok(None);
        };
        if after != before {
            tokio::task::yield_now().await;
            continue;
        }
        let resource = resource?;
        let network = network?;
        let traffic = traffic?;
        return Ok(Some((after, resource, network, traffic)));
    }
}

async fn load_fenced_resource(
    listener: &mut PgListener,
    client_id: &str,
) -> Result<Option<(ResourceHead, ResourceOwner)>> {
    loop {
        let Some(before) = load_optional_client_heads(listener, client_id)
            .await?
            .map(|heads| heads.resource)
        else {
            return Ok(None);
        };
        let owner = load_resource_owner(listener, client_id, &before).await;
        let Some(after) = load_optional_client_heads(listener, client_id)
            .await?
            .map(|heads| heads.resource)
        else {
            return Ok(None);
        };
        if before != after {
            tokio::task::yield_now().await;
            continue;
        }
        return Ok(Some((after, owner?)));
    }
}

async fn load_fenced_network(
    listener: &mut PgListener,
    client_id: &str,
) -> Result<Option<(NetworkHead, NetworkOwner)>> {
    loop {
        let Some(before) = load_optional_client_heads(listener, client_id)
            .await?
            .map(|heads| heads.network)
        else {
            return Ok(None);
        };
        let owner = load_network_owner(listener, client_id, &before).await;
        let Some(after) = load_optional_client_heads(listener, client_id)
            .await?
            .map(|heads| heads.network)
        else {
            return Ok(None);
        };
        if before != after {
            tokio::task::yield_now().await;
            continue;
        }
        return Ok(Some((after, owner?)));
    }
}

async fn load_fenced_traffic(
    listener: &mut PgListener,
    client_id: &str,
) -> Result<Option<(TrafficHead, TrafficOwner)>> {
    loop {
        let Some(before) = load_optional_client_heads(listener, client_id)
            .await?
            .map(|heads| heads.traffic)
        else {
            return Ok(None);
        };
        let owner = load_traffic_owner(listener, client_id, &before).await;
        let Some(after) = load_optional_client_heads(listener, client_id)
            .await?
            .map(|heads| heads.traffic)
        else {
            return Ok(None);
        };
        if before != after {
            tokio::task::yield_now().await;
            continue;
        }
        return Ok(Some((after, owner?)));
    }
}

async fn seed_fleet(pool: &sqlx::PgPool) -> Result<ResidentFleet> {
    let mut transaction = pool.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *transaction)
        .await?;
    let heads = parse_heads(sqlx::query(HEADS_SQL).fetch_all(&mut *transaction).await?)?;

    let mut resource_blocks = HashMap::<String, BTreeMap<(i32, i64), ResourceBlock>>::new();
    for row in sqlx::query(SEED_RESOURCE_BLOCKS_SQL)
        .fetch_all(&mut *transaction)
        .await?
    {
        let client_id: String = row.try_get("seed_client_id")?;
        let head = &heads
            .get(&client_id)
            .with_context(|| format!("dashboard resource seed head is missing for {client_id}"))?
            .resource;
        let (tier, block) = ResourceBlockRow::from_row(&row)?.into_block(head)?;
        anyhow::ensure!(
            resource_blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block)
                .is_none(),
            "dashboard resource seed block key is duplicated"
        );
    }
    let mut network_blocks = HashMap::<String, BTreeMap<(i32, i64), NetworkBlock>>::new();
    for row in sqlx::query(SEED_NETWORK_BLOCKS_SQL)
        .fetch_all(&mut *transaction)
        .await?
    {
        let client_id: String = row.try_get("seed_client_id")?;
        let head = &heads
            .get(&client_id)
            .with_context(|| format!("dashboard network seed head is missing for {client_id}"))?
            .network;
        let (tier, block) = NetworkBlockRow::from_row(&row)?.into_block(head)?;
        anyhow::ensure!(
            network_blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block)
                .is_none(),
            "dashboard network seed block key is duplicated"
        );
    }
    let mut traffic_blocks = HashMap::<String, BTreeMap<(i32, i64), TrafficBlock>>::new();
    for row in sqlx::query(SEED_TRAFFIC_BLOCKS_SQL)
        .fetch_all(&mut *transaction)
        .await?
    {
        let client_id: String = row.try_get("seed_client_id")?;
        let head = &heads
            .get(&client_id)
            .with_context(|| format!("dashboard traffic seed head is missing for {client_id}"))?
            .traffic;
        let (tier, block) = TrafficBlockRow::from_row(&row)?.into_block(head)?;
        anyhow::ensure!(
            traffic_blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block)
                .is_none(),
            "dashboard traffic seed block key is duplicated"
        );
    }
    transaction.commit().await?;

    let mut fleet = ResidentFleet::default();
    for (client_id, head) in heads {
        let resource_blocks = resource_blocks.remove(&client_id).unwrap_or_default();
        anyhow::ensure!(
            head.resource.first_unix.is_none() || !resource_blocks.is_empty(),
            "dashboard resource head emptiness disagrees with its F16 generation"
        );
        let network_blocks = network_blocks.remove(&client_id).unwrap_or_default();
        anyhow::ensure!(
            head.network.first_unix.is_none() || !network_blocks.is_empty(),
            "dashboard network head emptiness disagrees with its F16 generation"
        );
        let traffic_blocks = traffic_blocks.remove(&client_id).unwrap_or_default();
        anyhow::ensure!(
            head.traffic.first_unix.is_none() || !traffic_blocks.is_empty(),
            "dashboard traffic head emptiness disagrees with its F16 generation"
        );
        fleet.resources.insert(
            client_id.clone(),
            Arc::new(RwLock::new(ResourceOwner {
                generation: head.resource.generation,
                revision: head.resource.revision,
                overlay_blocks: BTreeSet::new(),
                index: ResourceIndex::from_blocks(resource_blocks)?,
            })),
        );
        fleet.networks.insert(
            client_id.clone(),
            Arc::new(RwLock::new(NetworkOwner {
                generation: head.network.generation,
                revision: head.network.revision,
                overlay_blocks: BTreeSet::new(),
                index: NetworkIndex::from_blocks(
                    Arc::clone(&head.network.interfaces),
                    network_blocks,
                )?,
            })),
        );
        fleet.traffics.insert(
            client_id,
            Arc::new(RwLock::new(TrafficOwner {
                generation: head.traffic.generation,
                revision: head.traffic.revision,
                overlay_blocks: BTreeSet::new(),
                source_kinds: Arc::clone(&head.traffic.source_kinds),
                interfaces: Arc::clone(&head.traffic.interfaces),
                index: TrafficIndex::from_blocks(traffic_blocks),
            })),
        );
    }
    anyhow::ensure!(
        resource_blocks.is_empty() && network_blocks.is_empty() && traffic_blocks.is_empty(),
        "dashboard seed returned a block without a joined fleet head"
    );
    Ok(fleet)
}

async fn load_resource_blocks(
    listener: &mut PgListener,
    client_id: &str,
    head: &ResourceHead,
) -> Result<(Vec<(i32, i64, Option<ResourceBlock>)>, BTreeSet<(i32, i64)>)> {
    let tiers = head
        .blocks
        .iter()
        .map(|block| block.source_bucket_secs)
        .collect::<Vec<_>>();
    let starts = head
        .blocks
        .iter()
        .map(|block| block.block_start_unix)
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, ResourceBlockRow>(RESOURCE_COORDINATE_BLOCKS_SQL)
        .bind(client_id)
        .bind(head.generation)
        .bind(head.revision)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?;
    let mut blocks = BTreeMap::new();
    for row in rows {
        let (tier, block) = row.into_block(head)?;
        anyhow::ensure!(
            blocks.insert((tier, block.start), block).is_none(),
            "dashboard resource exact block query returned a duplicate"
        );
    }
    let overlay = sqlx::query_as::<_, ResourceOverlayRow>(RESOURCE_OVERLAY_SQL)
        .bind(client_id)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?;
    let mut overlay_blocks = BTreeSet::new();
    for row in overlay {
        let key = (row.tier, row.block_start);
        overlay_blocks.insert(key);
        row.apply(&mut blocks, head)?;
    }
    Ok((
        head.blocks
            .iter()
            .map(|key| {
                (
                    key.source_bucket_secs,
                    key.block_start_unix,
                    blocks.remove(&(key.source_bucket_secs, key.block_start_unix)),
                )
            })
            .collect(),
        overlay_blocks,
    ))
}

async fn load_network_blocks(
    listener: &mut PgListener,
    client_id: &str,
    head: &NetworkHead,
) -> Result<(Vec<(i32, i64, Option<NetworkBlock>)>, BTreeSet<(i32, i64)>)> {
    let tiers = head
        .blocks
        .iter()
        .map(|block| block.source_bucket_secs)
        .collect::<Vec<_>>();
    let starts = head
        .blocks
        .iter()
        .map(|block| block.block_start_unix)
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, NetworkBlockRow>(NETWORK_COORDINATE_BLOCKS_SQL)
        .bind(client_id)
        .bind(head.generation)
        .bind(head.revision)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?;
    let mut blocks = BTreeMap::new();
    for row in rows {
        let (tier, block) = row.into_block(head)?;
        anyhow::ensure!(
            blocks.insert((tier, block.start), block).is_none(),
            "dashboard network exact block query returned a duplicate"
        );
    }
    let overlay = sqlx::query_as::<_, NetworkOverlayRow>(NETWORK_OVERLAY_SQL)
        .bind(client_id)
        .bind(&tiers)
        .bind(&starts)
        .bind(head.interfaces.as_ref())
        .fetch_all(&mut *listener)
        .await?;
    let mut overlay_blocks = BTreeSet::new();
    for row in overlay {
        let key = (row.tier, row.block_start);
        overlay_blocks.insert(key);
        row.apply(&mut blocks, head)?;
    }
    Ok((
        head.blocks
            .iter()
            .map(|key| {
                (
                    key.source_bucket_secs,
                    key.block_start_unix,
                    blocks.remove(&(key.source_bucket_secs, key.block_start_unix)),
                )
            })
            .collect(),
        overlay_blocks,
    ))
}

async fn load_traffic_blocks(
    listener: &mut PgListener,
    client_id: &str,
    head: &TrafficHead,
) -> Result<(Vec<(i32, i64, Option<TrafficBlock>)>, BTreeSet<(i32, i64)>)> {
    let tiers = head
        .blocks
        .iter()
        .map(|block| block.source_bucket_secs)
        .collect::<Vec<_>>();
    let starts = head
        .blocks
        .iter()
        .map(|block| block.block_start_unix)
        .collect::<Vec<_>>();
    let rows = sqlx::query_as::<_, TrafficBlockRow>(TRAFFIC_COORDINATE_BLOCKS_SQL)
        .bind(client_id)
        .bind(head.generation)
        .bind(head.revision)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?;
    let mut blocks = BTreeMap::new();
    for row in rows {
        let (tier, block) = row.into_block(head)?;
        anyhow::ensure!(
            blocks.insert((tier, block.start), block).is_none(),
            "dashboard traffic exact block query returned a duplicate"
        );
    }
    let overlay = sqlx::query_as::<_, TrafficOverlayRow>(TRAFFIC_OVERLAY_SQL)
        .bind(client_id)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?;
    let mut overlay_blocks = BTreeSet::new();
    for row in overlay {
        let key = (row.tier, row.block_start);
        overlay_blocks.insert(key);
        row.apply(&mut blocks)?;
    }
    Ok((
        head.blocks
            .iter()
            .map(|key| {
                (
                    key.source_bucket_secs,
                    key.block_start_unix,
                    blocks.remove(&(key.source_bucket_secs, key.block_start_unix)),
                )
            })
            .collect(),
        overlay_blocks,
    ))
}

struct ResourceNoticeTarget {
    ordinal: usize,
    notice: DashboardNotice,
    owner: Arc<RwLock<ResourceOwner>>,
    head: ResourceHead,
    blocks: Arc<[BlockKey]>,
}

struct NetworkNoticeTarget {
    ordinal: usize,
    notice: DashboardNotice,
    owner: Arc<RwLock<NetworkOwner>>,
    head: NetworkHead,
    blocks: Arc<[BlockKey]>,
}

struct TrafficNoticeTarget {
    ordinal: usize,
    notice: DashboardNotice,
    owner: Arc<RwLock<TrafficOwner>>,
    head: TrafficHead,
    blocks: Arc<[BlockKey]>,
}

struct ResourceNoticeChange {
    blocks: Vec<(i32, i64, Option<ResourceBlock>)>,
    overlay_blocks: BTreeSet<(i32, i64)>,
}

struct NetworkNoticeChange {
    blocks: Vec<(i32, i64, Option<NetworkBlock>)>,
    overlay_blocks: BTreeSet<(i32, i64)>,
}

struct TrafficNoticeChange {
    blocks: Vec<(i32, i64, Option<TrafficBlock>)>,
    overlay_blocks: BTreeSet<(i32, i64)>,
}

async fn load_resource_notice_changes(
    listener: &mut PgListener,
    targets: &BTreeMap<String, ResourceNoticeTarget>,
) -> Result<HashMap<String, ResourceNoticeChange>> {
    let mut clients = Vec::new();
    let mut generations = Vec::new();
    let mut revisions = Vec::new();
    let mut tiers = Vec::new();
    let mut starts = Vec::new();
    for (client_id, target) in targets {
        for key in target.blocks.iter() {
            clients.push(client_id.clone());
            generations.push(target.head.generation);
            revisions.push(target.head.revision);
            tiers.push(key.source_bucket_secs);
            starts.push(key.block_start_unix);
        }
    }
    if clients.is_empty() {
        return Ok(HashMap::new());
    }

    let mut blocks = HashMap::<String, BTreeMap<(i32, i64), ResourceBlock>>::new();
    for row in sqlx::query(OVERLAY_RESOURCE_BLOCKS_SQL)
        .bind(&clients)
        .bind(&generations)
        .bind(&revisions)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?
    {
        let client_id: String = row.try_get("overlay_client_id")?;
        let target = targets
            .get(&client_id)
            .context("dashboard resource notice target disappeared")?;
        let (tier, block) = ResourceBlockRow::from_row(&row)?.into_block(&target.head)?;
        anyhow::ensure!(
            blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block)
                .is_none(),
            "dashboard resource notice block key is duplicated"
        );
    }

    let mut overlay_blocks = HashMap::<String, BTreeSet<(i32, i64)>>::new();
    for row in sqlx::query(NOTICE_RESOURCE_OVERLAY_SQL)
        .bind(&clients)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?
    {
        let client_id: String = row.try_get("overlay_client_id")?;
        let target = targets
            .get(&client_id)
            .context("dashboard resource notice target disappeared")?;
        let overlay = ResourceOverlayRow::from_row(&row)?;
        overlay_blocks
            .entry(client_id.clone())
            .or_default()
            .insert((overlay.tier, overlay.block_start));
        overlay.apply(blocks.entry(client_id).or_default(), &target.head)?;
    }

    let mut changes = HashMap::new();
    for (client_id, target) in targets {
        let mut client_blocks = blocks.remove(client_id).unwrap_or_default();
        let replacement = target
            .blocks
            .iter()
            .map(|key| {
                (
                    key.source_bucket_secs,
                    key.block_start_unix,
                    client_blocks.remove(&(key.source_bucket_secs, key.block_start_unix)),
                )
            })
            .collect();
        anyhow::ensure!(
            client_blocks.is_empty(),
            "dashboard resource notice returned an unrequested block"
        );
        changes.insert(
            client_id.clone(),
            ResourceNoticeChange {
                blocks: replacement,
                overlay_blocks: overlay_blocks.remove(client_id).unwrap_or_default(),
            },
        );
    }
    anyhow::ensure!(
        blocks.is_empty() && overlay_blocks.is_empty(),
        "dashboard resource notice rows escaped their exact owner"
    );
    Ok(changes)
}

async fn load_network_notice_changes(
    listener: &mut PgListener,
    targets: &BTreeMap<String, NetworkNoticeTarget>,
) -> Result<HashMap<String, NetworkNoticeChange>> {
    let mut clients = Vec::new();
    let mut generations = Vec::new();
    let mut revisions = Vec::new();
    let mut tiers = Vec::new();
    let mut starts = Vec::new();
    let mut interface_clients = Vec::new();
    let mut interfaces = Vec::new();
    for (client_id, target) in targets {
        for key in target.blocks.iter() {
            clients.push(client_id.clone());
            generations.push(target.head.generation);
            revisions.push(target.head.revision);
            tiers.push(key.source_bucket_secs);
            starts.push(key.block_start_unix);
        }
        for interface in target.head.interfaces.iter() {
            interface_clients.push(client_id.clone());
            interfaces.push(interface.clone());
        }
    }
    if clients.is_empty() {
        return Ok(HashMap::new());
    }

    let mut blocks = HashMap::<String, BTreeMap<(i32, i64), NetworkBlock>>::new();
    for row in sqlx::query(OVERLAY_NETWORK_BLOCKS_SQL)
        .bind(&clients)
        .bind(&generations)
        .bind(&revisions)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?
    {
        let client_id: String = row.try_get("overlay_client_id")?;
        let target = targets
            .get(&client_id)
            .context("dashboard network notice target disappeared")?;
        let (tier, block) = NetworkBlockRow::from_row(&row)?.into_block(&target.head)?;
        anyhow::ensure!(
            blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block)
                .is_none(),
            "dashboard network notice block key is duplicated"
        );
    }

    let mut states = BTreeMap::<(String, i32, i64, i64), Vec<NetworkState>>::new();
    for row in sqlx::query_as::<_, NetworkOverlaySourceRow>(NOTICE_NETWORK_OVERLAY_SQL)
        .bind(&clients)
        .bind(&tiers)
        .bind(&starts)
        .bind(&interface_clients)
        .bind(&interfaces)
        .fetch_all(&mut *listener)
        .await?
    {
        let target = targets
            .get(&row.client_id)
            .context("dashboard network notice target disappeared")?;
        let interface = target
            .head
            .interfaces
            .iter()
            .position(|candidate| candidate == &row.interface)
            .context("dashboard network notice returned an unselected interface")?;
        let span = i64::from(row.source_bucket_secs) * BLOCK_SLOTS as i64;
        anyhow::ensure!(
            valid_tier(row.source_bucket_secs)
                && row.block_start_unix.rem_euclid(span) == 0
                && row.bucket_start_unix >= row.block_start_unix
                && row.bucket_start_unix < row.block_start_unix + span
                && (row.bucket_start_unix - row.block_start_unix)
                    .rem_euclid(i64::from(row.source_bucket_secs))
                    == 0,
            "dashboard network notice overlay key is invalid"
        );
        let values = states
            .entry((
                row.client_id,
                row.source_bucket_secs,
                row.block_start_unix,
                row.bucket_start_unix,
            ))
            .or_insert_with(|| vec![NetworkState::default(); target.head.interfaces.len()]);
        anyhow::ensure!(
            !values[interface].present(),
            "dashboard network notice overlay owner is duplicated"
        );
        values[interface] = NetworkState {
            count: row.sample_count,
            latest: row.latest_observed_unix,
            rx: row.rx_bytes_last,
            tx: row.tx_bytes_last,
            rx_epoch: row.rx_counter_epoch,
            tx_epoch: row.tx_counter_epoch,
        }
        .valid()?;
    }
    let mut overlay_blocks = HashMap::<String, BTreeSet<(i32, i64)>>::new();
    for ((client_id, tier, block_start, bucket_start), values) in states {
        let target = targets
            .get(&client_id)
            .context("dashboard network notice target disappeared")?;
        overlay_blocks
            .entry(client_id.clone())
            .or_default()
            .insert((tier, block_start));
        NetworkOverlayRow {
            tier,
            block_start,
            bucket_start,
            counts: values.iter().map(|state| state.count).collect(),
            latest: values
                .iter()
                .map(|state| state.present().then_some(state.latest))
                .collect(),
            rx: values
                .iter()
                .map(|state| state.present().then_some(state.rx))
                .collect(),
            tx: values
                .iter()
                .map(|state| state.present().then_some(state.tx))
                .collect(),
            rx_epoch: values
                .iter()
                .map(|state| state.present().then_some(state.rx_epoch))
                .collect(),
            tx_epoch: values
                .iter()
                .map(|state| state.present().then_some(state.tx_epoch))
                .collect(),
        }
        .apply(blocks.entry(client_id).or_default(), &target.head)?;
    }

    let mut changes = HashMap::new();
    for (client_id, target) in targets {
        let mut client_blocks = blocks.remove(client_id).unwrap_or_default();
        let replacement = target
            .blocks
            .iter()
            .map(|key| {
                (
                    key.source_bucket_secs,
                    key.block_start_unix,
                    client_blocks.remove(&(key.source_bucket_secs, key.block_start_unix)),
                )
            })
            .collect();
        anyhow::ensure!(
            client_blocks.is_empty(),
            "dashboard network notice returned an unrequested block"
        );
        changes.insert(
            client_id.clone(),
            NetworkNoticeChange {
                blocks: replacement,
                overlay_blocks: overlay_blocks.remove(client_id).unwrap_or_default(),
            },
        );
    }
    anyhow::ensure!(
        blocks.is_empty() && overlay_blocks.is_empty(),
        "dashboard network notice rows escaped their exact owner"
    );
    Ok(changes)
}

async fn load_traffic_notice_changes(
    listener: &mut PgListener,
    targets: &BTreeMap<String, TrafficNoticeTarget>,
) -> Result<HashMap<String, TrafficNoticeChange>> {
    let mut clients = Vec::new();
    let mut generations = Vec::new();
    let mut revisions = Vec::new();
    let mut tiers = Vec::new();
    let mut starts = Vec::new();
    for (client_id, target) in targets {
        for key in target.blocks.iter() {
            clients.push(client_id.clone());
            generations.push(target.head.generation);
            revisions.push(target.head.revision);
            tiers.push(key.source_bucket_secs);
            starts.push(key.block_start_unix);
        }
    }
    if clients.is_empty() {
        return Ok(HashMap::new());
    }

    let mut blocks = HashMap::<String, BTreeMap<(i32, i64), TrafficBlock>>::new();
    for row in sqlx::query(OVERLAY_TRAFFIC_BLOCKS_SQL)
        .bind(&clients)
        .bind(&generations)
        .bind(&revisions)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?
    {
        let client_id: String = row.try_get("overlay_client_id")?;
        let target = targets
            .get(&client_id)
            .context("dashboard traffic notice target disappeared")?;
        let (tier, block) = TrafficBlockRow::from_row(&row)?.into_block(&target.head)?;
        anyhow::ensure!(
            blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block)
                .is_none(),
            "dashboard traffic notice block key is duplicated"
        );
    }

    let mut overlay_blocks = HashMap::<String, BTreeSet<(i32, i64)>>::new();
    for row in sqlx::query(NOTICE_TRAFFIC_OVERLAY_SQL)
        .bind(&clients)
        .bind(&tiers)
        .bind(&starts)
        .fetch_all(&mut *listener)
        .await?
    {
        let client_id: String = row.try_get("overlay_client_id")?;
        anyhow::ensure!(
            targets.contains_key(&client_id),
            "dashboard traffic notice target disappeared"
        );
        let overlay = TrafficOverlayRow::from_row(&row)?;
        overlay_blocks
            .entry(client_id.clone())
            .or_default()
            .insert((overlay.tier, overlay.block_start));
        overlay.apply(blocks.entry(client_id).or_default())?;
    }

    let mut changes = HashMap::new();
    for (client_id, target) in targets {
        let mut client_blocks = blocks.remove(client_id).unwrap_or_default();
        let replacement = target
            .blocks
            .iter()
            .map(|key| {
                (
                    key.source_bucket_secs,
                    key.block_start_unix,
                    client_blocks.remove(&(key.source_bucket_secs, key.block_start_unix)),
                )
            })
            .collect();
        anyhow::ensure!(
            client_blocks.is_empty(),
            "dashboard traffic notice returned an unrequested block"
        );
        changes.insert(
            client_id.clone(),
            TrafficNoticeChange {
                blocks: replacement,
                overlay_blocks: overlay_blocks.remove(client_id).unwrap_or_default(),
            },
        );
    }
    anyhow::ensure!(
        blocks.is_empty() && overlay_blocks.is_empty(),
        "dashboard traffic notice rows escaped their exact owner"
    );
    Ok(changes)
}

async fn reconcile_live_overlays(
    listener: &mut PgListener,
    resident: &DashboardTelemetryResident,
    client_ids: &[String],
) -> Result<()> {
    if client_ids.is_empty() {
        return Ok(());
    }
    let snapshot = resident.snapshot();
    let mut resource_targets = HashMap::new();
    let mut network_targets = HashMap::new();
    let mut traffic_targets = HashMap::new();
    for client_id in client_ids {
        if let Some(owner) = snapshot.resources.get(client_id).cloned() {
            let installed = owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            resource_targets.insert(
                client_id.clone(),
                ResourceOverlayTarget {
                    owner: Arc::clone(&owner),
                    generation: installed.generation,
                    revision: installed.revision,
                    prior_blocks: installed.overlay_blocks.clone(),
                },
            );
        }
        if let Some(owner) = snapshot.networks.get(client_id).cloned() {
            let installed = owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The published generation vector is the network-selection fence.
            // With no selected interface there is no live suffix to install.
            if !installed.index.interfaces.is_empty() {
                network_targets.insert(
                    client_id.clone(),
                    NetworkOverlayTarget {
                        owner: Arc::clone(&owner),
                        generation: installed.generation,
                        revision: installed.revision,
                        interfaces: Arc::clone(&installed.index.interfaces),
                        prior_blocks: installed.overlay_blocks.clone(),
                    },
                );
            }
        }
        if let Some(owner) = snapshot.traffics.get(client_id).cloned() {
            let installed = owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // The published generation map is the traffic-selection fence.
            // An empty map is authoritative and cannot own a live suffix.
            if !installed.source_kinds.is_empty() {
                traffic_targets.insert(
                    client_id.clone(),
                    TrafficOverlayTarget {
                        owner: Arc::clone(&owner),
                        generation: installed.generation,
                        revision: installed.revision,
                        prior_blocks: installed.overlay_blocks.clone(),
                    },
                );
            }
        }
    }

    // Read overlays before retained bases. If a close or publisher commits
    // between the two statements, the later base is at least as new; this
    // ordering cannot observe neither side of the active-to-closed handoff.
    let resource_target_client_ids = overlay_target_client_ids(&resource_targets);
    let mut resource_overlays = HashMap::<String, Vec<ResourceOverlayRow>>::new();
    if !resource_target_client_ids.is_empty() {
        for row in sqlx::query(OVERLAY_RESOURCE_SOURCE_SQL)
            .bind(&resource_target_client_ids)
            .fetch_all(&mut *listener)
            .await?
        {
            let client_id: String = row.try_get("overlay_client_id")?;
            if resource_targets.contains_key(&client_id) {
                resource_overlays
                    .entry(client_id)
                    .or_default()
                    .push(ResourceOverlayRow::from_row(&row)?);
            }
        }
    }

    let network_target_client_ids = overlay_target_client_ids(&network_targets);
    let mut network_states = BTreeMap::<(String, i32, i64, i64), Vec<NetworkState>>::new();
    if !network_target_client_ids.is_empty() {
        for row in sqlx::query_as::<_, NetworkOverlaySourceRow>(OVERLAY_NETWORK_SOURCE_SQL)
            .bind(&network_target_client_ids)
            .fetch_all(&mut *listener)
            .await?
        {
            let Some(target) = network_targets.get(&row.client_id) else {
                continue;
            };
            let Some(interface) = target
                .interfaces
                .iter()
                .position(|candidate| candidate == &row.interface)
            else {
                continue;
            };
            let span = i64::from(row.source_bucket_secs) * BLOCK_SLOTS as i64;
            anyhow::ensure!(
                valid_tier(row.source_bucket_secs)
                    && row.block_start_unix.rem_euclid(span) == 0
                    && row.bucket_start_unix >= row.block_start_unix
                    && row.bucket_start_unix < row.block_start_unix + span
                    && (row.bucket_start_unix - row.block_start_unix)
                        .rem_euclid(i64::from(row.source_bucket_secs))
                        == 0,
                "dashboard network canonical overlay key is invalid"
            );
            let states = network_states
                .entry((
                    row.client_id,
                    row.source_bucket_secs,
                    row.block_start_unix,
                    row.bucket_start_unix,
                ))
                .or_insert_with(|| vec![NetworkState::default(); target.interfaces.len()]);
            anyhow::ensure!(
                !states[interface].present(),
                "dashboard network canonical overlay owner is duplicated"
            );
            states[interface] = NetworkState {
                count: row.sample_count,
                latest: row.latest_observed_unix,
                rx: row.rx_bytes_last,
                tx: row.tx_bytes_last,
                rx_epoch: row.rx_counter_epoch,
                tx_epoch: row.tx_counter_epoch,
            }
            .valid()?;
        }
    }
    let mut network_overlays = HashMap::<String, Vec<NetworkOverlayRow>>::new();
    for ((client_id, tier, block_start, bucket_start), states) in network_states {
        network_overlays
            .entry(client_id)
            .or_default()
            .push(NetworkOverlayRow {
                tier,
                block_start,
                bucket_start,
                counts: states.iter().map(|state| state.count).collect(),
                latest: states
                    .iter()
                    .map(|state| state.present().then_some(state.latest))
                    .collect(),
                rx: states
                    .iter()
                    .map(|state| state.present().then_some(state.rx))
                    .collect(),
                tx: states
                    .iter()
                    .map(|state| state.present().then_some(state.tx))
                    .collect(),
                rx_epoch: states
                    .iter()
                    .map(|state| state.present().then_some(state.rx_epoch))
                    .collect(),
                tx_epoch: states
                    .iter()
                    .map(|state| state.present().then_some(state.tx_epoch))
                    .collect(),
            });
    }

    let traffic_target_client_ids = overlay_target_client_ids(&traffic_targets);
    let mut traffic_overlays = HashMap::<String, Vec<TrafficOverlayRow>>::new();
    if !traffic_target_client_ids.is_empty() {
        for row in sqlx::query(OVERLAY_TRAFFIC_SOURCE_SQL)
            .bind(&traffic_target_client_ids)
            .fetch_all(&mut *listener)
            .await?
        {
            let client_id: String = row.try_get("client_id")?;
            if traffic_targets.contains_key(&client_id) {
                traffic_overlays
                    .entry(client_id)
                    .or_default()
                    .push(TrafficOverlayRow::from_row(&row)?);
            }
        }
    }

    let resource_new_blocks = resource_overlays
        .iter()
        .map(|(client_id, rows)| {
            (
                client_id.clone(),
                rows.iter()
                    .map(|row| (row.tier, row.block_start))
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    let network_new_blocks = network_overlays
        .iter()
        .map(|(client_id, rows)| {
            (
                client_id.clone(),
                rows.iter()
                    .map(|row| (row.tier, row.block_start))
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();
    let traffic_new_blocks = traffic_overlays
        .iter()
        .map(|(client_id, rows)| {
            (
                client_id.clone(),
                rows.iter()
                    .map(|row| (row.tier, row.block_start))
                    .collect::<BTreeSet<_>>(),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut resource_coordinates = Vec::new();
    for (client_id, target) in &resource_targets {
        let new = resource_new_blocks.get(client_id);
        for (tier, start) in target
            .prior_blocks
            .iter()
            .copied()
            .chain(new.into_iter().flat_map(|blocks| blocks.iter().copied()))
            .collect::<BTreeSet<_>>()
        {
            resource_coordinates.push((
                client_id.clone(),
                target.generation,
                target.revision,
                tier,
                start,
            ));
        }
    }
    let mut resource_blocks = HashMap::<String, BTreeMap<(i32, i64), ResourceBlock>>::new();
    if !resource_coordinates.is_empty() {
        let clients = resource_coordinates
            .iter()
            .map(|value| value.0.clone())
            .collect::<Vec<_>>();
        let generations = resource_coordinates
            .iter()
            .map(|value| value.1)
            .collect::<Vec<_>>();
        let revisions = resource_coordinates
            .iter()
            .map(|value| value.2)
            .collect::<Vec<_>>();
        let tiers = resource_coordinates
            .iter()
            .map(|value| value.3)
            .collect::<Vec<_>>();
        let starts = resource_coordinates
            .iter()
            .map(|value| value.4)
            .collect::<Vec<_>>();
        for row in sqlx::query(OVERLAY_RESOURCE_BLOCKS_SQL)
            .bind(&clients)
            .bind(&generations)
            .bind(&revisions)
            .bind(&tiers)
            .bind(&starts)
            .fetch_all(&mut *listener)
            .await?
        {
            let client_id: String = row.try_get("overlay_client_id")?;
            let target = resource_targets
                .get(&client_id)
                .context("dashboard resource overlay target disappeared")?;
            let head = ResourceHead {
                generation: target.generation,
                revision: target.revision,
                change: "generation".to_string(),
                blocks: Arc::from([]),
                first_unix: None,
                through_unix: None,
            };
            let (tier, block) = ResourceBlockRow::from_row(&row)?.into_block(&head)?;
            resource_blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block);
        }
    }
    for (client_id, rows) in resource_overlays {
        let target = resource_targets
            .get(&client_id)
            .context("dashboard resource overlay target disappeared")?;
        let head = ResourceHead {
            generation: target.generation,
            revision: target.revision,
            change: "generation".to_string(),
            blocks: Arc::from([]),
            first_unix: None,
            through_unix: None,
        };
        let blocks = resource_blocks.entry(client_id).or_default();
        for row in rows {
            row.apply(blocks, &head)?;
        }
    }
    for (client_id, target) in resource_targets {
        let new_blocks = resource_new_blocks
            .get(&client_id)
            .cloned()
            .unwrap_or_default();
        let touched = target
            .prior_blocks
            .union(&new_blocks)
            .copied()
            .collect::<Vec<_>>();
        let blocks = resource_blocks.entry(client_id.clone()).or_default();
        let changes = touched
            .into_iter()
            .map(|(tier, start)| (tier, start, blocks.remove(&(tier, start))))
            .collect();
        let mut owner = target
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.generation == target.generation && owner.revision == target.revision {
            owner.index.apply_blocks(changes);
            owner.overlay_blocks = new_blocks;
        }
    }

    let mut network_coordinates = Vec::new();
    for (client_id, target) in &network_targets {
        let new = network_new_blocks.get(client_id);
        for (tier, start) in target
            .prior_blocks
            .iter()
            .copied()
            .chain(new.into_iter().flat_map(|blocks| blocks.iter().copied()))
            .collect::<BTreeSet<_>>()
        {
            network_coordinates.push((
                client_id.clone(),
                target.generation,
                target.revision,
                tier,
                start,
            ));
        }
    }
    let mut network_blocks = HashMap::<String, BTreeMap<(i32, i64), NetworkBlock>>::new();
    if !network_coordinates.is_empty() {
        let clients = network_coordinates
            .iter()
            .map(|value| value.0.clone())
            .collect::<Vec<_>>();
        let generations = network_coordinates
            .iter()
            .map(|value| value.1)
            .collect::<Vec<_>>();
        let revisions = network_coordinates
            .iter()
            .map(|value| value.2)
            .collect::<Vec<_>>();
        let tiers = network_coordinates
            .iter()
            .map(|value| value.3)
            .collect::<Vec<_>>();
        let starts = network_coordinates
            .iter()
            .map(|value| value.4)
            .collect::<Vec<_>>();
        for row in sqlx::query(OVERLAY_NETWORK_BLOCKS_SQL)
            .bind(&clients)
            .bind(&generations)
            .bind(&revisions)
            .bind(&tiers)
            .bind(&starts)
            .fetch_all(&mut *listener)
            .await?
        {
            let client_id: String = row.try_get("overlay_client_id")?;
            let target = network_targets
                .get(&client_id)
                .context("dashboard network overlay target disappeared")?;
            let head = NetworkHead {
                generation: target.generation,
                revision: target.revision,
                change: "generation".to_string(),
                blocks: Arc::from([]),
                interfaces: Arc::clone(&target.interfaces),
                first_unix: None,
                through_unix: None,
            };
            let (tier, block) = NetworkBlockRow::from_row(&row)?.into_block(&head)?;
            network_blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block);
        }
    }
    for (client_id, rows) in network_overlays {
        let target = network_targets
            .get(&client_id)
            .context("dashboard network overlay target disappeared")?;
        let head = NetworkHead {
            generation: target.generation,
            revision: target.revision,
            change: "generation".to_string(),
            blocks: Arc::from([]),
            interfaces: Arc::clone(&target.interfaces),
            first_unix: None,
            through_unix: None,
        };
        let blocks = network_blocks.entry(client_id).or_default();
        for row in rows {
            row.apply(blocks, &head)?;
        }
    }
    for (client_id, target) in network_targets {
        let new_blocks = network_new_blocks
            .get(&client_id)
            .cloned()
            .unwrap_or_default();
        let touched = target
            .prior_blocks
            .union(&new_blocks)
            .copied()
            .collect::<Vec<_>>();
        let blocks = network_blocks.entry(client_id.clone()).or_default();
        let changes = touched
            .into_iter()
            .map(|(tier, start)| (tier, start, blocks.remove(&(tier, start))))
            .collect();
        let mut owner = target
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.generation == target.generation
            && owner.revision == target.revision
            && owner.index.interfaces.as_ref() == target.interfaces.as_ref()
        {
            owner.index.apply_blocks(changes);
            owner.overlay_blocks = new_blocks;
        }
    }

    let mut traffic_coordinates = Vec::new();
    for (client_id, target) in &traffic_targets {
        let new = traffic_new_blocks.get(client_id);
        for (tier, start) in target
            .prior_blocks
            .iter()
            .copied()
            .chain(new.into_iter().flat_map(|blocks| blocks.iter().copied()))
            .collect::<BTreeSet<_>>()
        {
            traffic_coordinates.push((
                client_id.clone(),
                target.generation,
                target.revision,
                tier,
                start,
            ));
        }
    }
    let mut traffic_blocks = HashMap::<String, BTreeMap<(i32, i64), TrafficBlock>>::new();
    if !traffic_coordinates.is_empty() {
        let clients = traffic_coordinates
            .iter()
            .map(|value| value.0.clone())
            .collect::<Vec<_>>();
        let generations = traffic_coordinates
            .iter()
            .map(|value| value.1)
            .collect::<Vec<_>>();
        let revisions = traffic_coordinates
            .iter()
            .map(|value| value.2)
            .collect::<Vec<_>>();
        let tiers = traffic_coordinates
            .iter()
            .map(|value| value.3)
            .collect::<Vec<_>>();
        let starts = traffic_coordinates
            .iter()
            .map(|value| value.4)
            .collect::<Vec<_>>();
        for row in sqlx::query(OVERLAY_TRAFFIC_BLOCKS_SQL)
            .bind(&clients)
            .bind(&generations)
            .bind(&revisions)
            .bind(&tiers)
            .bind(&starts)
            .fetch_all(&mut *listener)
            .await?
        {
            let client_id: String = row.try_get("overlay_client_id")?;
            let target = traffic_targets
                .get(&client_id)
                .context("dashboard traffic overlay target disappeared")?;
            let installed = target
                .owner
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let head = TrafficHead {
                generation: target.generation,
                revision: target.revision,
                change: "generation".to_string(),
                blocks: Arc::from([]),
                source_kinds: Arc::clone(&installed.source_kinds),
                interfaces: Arc::clone(&installed.interfaces),
                first_unix: None,
                through_unix: None,
            };
            let (tier, block) = TrafficBlockRow::from_row(&row)?.into_block(&head)?;
            traffic_blocks
                .entry(client_id)
                .or_default()
                .insert((tier, block.start), block);
        }
    }
    for (client_id, rows) in traffic_overlays {
        let blocks = traffic_blocks.entry(client_id).or_default();
        for row in rows {
            row.apply(blocks)?;
        }
    }
    for (client_id, target) in traffic_targets {
        let new_blocks = traffic_new_blocks
            .get(&client_id)
            .cloned()
            .unwrap_or_default();
        let touched = target
            .prior_blocks
            .union(&new_blocks)
            .copied()
            .collect::<Vec<_>>();
        let blocks = traffic_blocks.entry(client_id).or_default();
        let changes = touched
            .into_iter()
            .map(|(tier, start)| (tier, start, blocks.remove(&(tier, start))))
            .collect();
        let mut owner = target
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.generation == target.generation && owner.revision == target.revision {
            owner.index.apply_blocks(changes);
            owner.overlay_blocks = new_blocks;
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct DashboardNotice {
    owner: String,
    client_id: String,
    domain: String,
    change: String,
    #[serde(default)]
    generation: Option<i64>,
    #[serde(default)]
    previous_revision: Option<i64>,
    #[serde(default)]
    revision: Option<i64>,
    #[serde(default)]
    source_bucket_secs: Option<Vec<i32>>,
    #[serde(default)]
    block_start_unix: Option<Vec<i64>>,
    #[serde(default)]
    complete: Option<bool>,
}

impl DashboardNotice {
    fn block_keys(&self) -> Arc<[BlockKey]> {
        canonical_block_keys(
            self.source_bucket_secs
                .clone()
                .expect("validated dashboard domain source tiers"),
            self.block_start_unix
                .clone()
                .expect("validated dashboard domain block starts"),
            &self.change,
        )
        .expect("validated dashboard notice block descriptor")
    }

    fn merge_contiguous(&mut self, incoming: &Self) -> bool {
        let (Some(current_generation), Some(incoming_generation)) =
            (self.generation, incoming.generation)
        else {
            return false;
        };
        let (Some(current_previous), Some(current_revision)) =
            (self.previous_revision, self.revision)
        else {
            return false;
        };
        let (Some(incoming_previous), Some(incoming_revision)) =
            (incoming.previous_revision, incoming.revision)
        else {
            return false;
        };
        if current_generation != incoming_generation
            || current_previous > incoming_revision
            || incoming_previous > current_revision
        {
            return false;
        }

        self.complete = match incoming_revision.cmp(&current_revision) {
            std::cmp::Ordering::Greater => incoming.complete,
            std::cmp::Ordering::Less => self.complete,
            std::cmp::Ordering::Equal => {
                Some(self.complete == Some(true) || incoming.complete == Some(true))
            }
        };
        self.previous_revision = Some(current_previous.min(incoming_previous));
        self.revision = Some(current_revision.max(incoming_revision));
        if self.change == "generation" || incoming.change == "generation" {
            // Loading the replacement generation also includes every later
            // contiguous block revision in that generation.
            self.change = "generation".to_string();
            self.source_bucket_secs = Some(Vec::new());
            self.block_start_unix = Some(Vec::new());
            return true;
        }

        let coordinates = self
            .source_bucket_secs
            .as_ref()
            .expect("validated dashboard block source tiers")
            .iter()
            .copied()
            .zip(
                self.block_start_unix
                    .as_ref()
                    .expect("validated dashboard block starts")
                    .iter()
                    .copied(),
            )
            .chain(
                incoming
                    .source_bucket_secs
                    .as_ref()
                    .expect("validated dashboard block source tiers")
                    .iter()
                    .copied()
                    .zip(
                        incoming
                            .block_start_unix
                            .as_ref()
                            .expect("validated dashboard block starts")
                            .iter()
                            .copied(),
                    ),
            )
            .collect::<BTreeSet<_>>();
        self.source_bucket_secs = Some(coordinates.iter().map(|(tier, _)| *tier).collect());
        self.block_start_unix = Some(coordinates.iter().map(|(_, start)| *start).collect());
        true
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
// The raw projection notification is shared with retention consumers. This
// resident owns only the per-client overlay coordinate, so deserialize that
// stable subset and leave retention deadlines to their worker owner.
struct RawProjectionNotice {
    client_id: String,
    generation: i64,
    projected_seq: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProjectionNotice {
    Dashboard(DashboardNotice),
    Raw(RawProjectionNotice),
}

impl ProjectionNotice {
    fn parse(payload: &str) -> Option<Self> {
        let value = serde_json::from_str::<serde_json::Value>(payload).ok()?;
        if value.get("owner").is_some() {
            let notice = serde_json::from_value::<DashboardNotice>(value).ok()?;
            let revision_is_single_commit = notice
                .previous_revision
                .zip(notice.revision)
                .is_some_and(|(previous, revision)| {
                    previous >= 0 && previous.checked_add(1) == Some(revision)
                });
            let block_descriptor_is_valid = notice
                .source_bucket_secs
                .clone()
                .zip(notice.block_start_unix.clone())
                .is_some_and(|(tiers, starts)| {
                    (notice.domain != "traffic" || tiers.iter().copied().all(valid_traffic_tier))
                        && canonical_block_keys(tiers, starts, &notice.change).is_ok()
                });
            let completion_is_valid = match notice.change.as_str() {
                "block" => {
                    notice.complete.is_some()
                        && notice.source_bucket_secs.as_ref().map(Vec::len) == Some(1)
                }
                "generation" => notice.complete == Some(true),
                _ => false,
            };
            let domain_notice =
                matches!(notice.domain.as_str(), "resource" | "network" | "traffic")
                    && matches!(notice.change.as_str(), "block" | "generation")
                    && notice.generation.is_some_and(|generation| generation > 0)
                    && revision_is_single_commit
                    && block_descriptor_is_valid
                    && completion_is_valid;
            let client_notice = notice.domain == "client"
                && matches!(notice.change.as_str(), "initialize" | "remove")
                && notice.generation.is_none()
                && notice.previous_revision.is_none()
                && notice.revision.is_none()
                && notice.source_bucket_secs.is_none()
                && notice.block_start_unix.is_none()
                && notice.complete.is_none();
            if notice.owner != "dashboard"
                || notice.client_id.is_empty()
                || (!domain_notice && !client_notice)
            {
                return None;
            }
            Some(Self::Dashboard(notice))
        } else {
            let notice = serde_json::from_value::<RawProjectionNotice>(value).ok()?;
            (!notice.client_id.is_empty() && notice.generation >= 0 && notice.projected_seq >= 0)
                .then_some(Self::Raw(notice))
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ResidentOwnerKey {
    client_id: String,
    domain: String,
}

impl ResidentOwnerKey {
    fn from_notice(notice: &DashboardNotice) -> Self {
        Self {
            client_id: notice.client_id.clone(),
            domain: notice.domain.clone(),
        }
    }
}

#[derive(Debug)]
enum ResidentMailboxEntry {
    Owner(ResidentOwnerKey),
    FleetFence,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LiveOverlayBatch {
    client_ids: Vec<String>,
    fence_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ResidentWork {
    Notices(Vec<DashboardNotice>),
    Overlay(LiveOverlayBatch),
    FleetFence,
}

#[derive(Default)]
struct ResidentMailboxState {
    pending: HashMap<ResidentOwnerKey, DashboardNotice>,
    ready_owners: HashSet<ResidentOwnerKey>,
    waiting_for_successor: HashMap<ResidentOwnerKey, i64>,
    pending_overlay: BTreeMap<String, RawProjectionNotice>,
    overlay_ready_at: Option<time::Instant>,
    order: VecDeque<ResidentMailboxEntry>,
    fleet_fence_pending: bool,
    fence_epoch: u64,
}

#[derive(Default)]
struct ResidentMailbox {
    state: Mutex<ResidentMailboxState>,
    ready: Notify,
}

impl ResidentMailbox {
    fn enqueue_live_overlay(&self, client_id: &str) {
        self.enqueue_overlay(RawProjectionNotice {
            client_id: client_id.to_string(),
            generation: 0,
            projected_seq: 0,
        });
    }

    fn enqueue_overlay(&self, notice: RawProjectionNotice) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let starts_collection = state.overlay_ready_at.is_none();
        let replace = state
            .pending_overlay
            .get(&notice.client_id)
            .is_none_or(|current| {
                (notice.generation, notice.projected_seq)
                    >= (current.generation, current.projected_seq)
            });
        if replace {
            state
                .pending_overlay
                .insert(notice.client_id.clone(), notice);
        }
        if starts_collection {
            state.overlay_ready_at =
                Some(time::Instant::now() + FLEET_TELEMETRY_INVALIDATION_WINDOW);
        }
        drop(state);
        // The first hint starts one fixed collection window. Later hints join
        // its BTreeMap without waking the sole reconciliation lane per frame.
        if starts_collection {
            self.ready.notify_one();
        }
    }

    fn enqueue(&self, notice: DashboardNotice) {
        let key = ResidentOwnerKey::from_notice(&notice);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(current) = state.pending.get_mut(&key) {
            if current.merge_contiguous(&notice) {
                // The merge below will publish this owner only when the final
                // fragment of its newest accumulated revision has arrived.
            } else {
                let replace = match (current.revision, notice.revision) {
                    // A non-contiguous newest hint proves that at least one
                    // commit hint is absent from this mailbox. Keep the newest
                    // fence; its application takes exact-owner reconciliation.
                    (Some(current), Some(incoming)) => incoming >= current,
                    // Client lifecycle notices have no revision. Their current
                    // database state is authoritative when claimed.
                    (None, None) => true,
                    _ => unreachable!("one mailbox key has one validated notice shape"),
                };
                if replace {
                    *current = notice;
                }
            }
        } else {
            state.pending.insert(key.clone(), notice);
        }
        if state
            .waiting_for_successor
            .get(&key)
            .is_some_and(|revision| {
                state
                    .pending
                    .get(&key)
                    .and_then(|notice| notice.revision)
                    .is_some_and(|pending| pending > *revision)
            })
        {
            state.waiting_for_successor.remove(&key);
        }
        let is_ready = !state.waiting_for_successor.contains_key(&key)
            && state
                .pending
                .get(&key)
                .is_some_and(|notice| notice.domain == "client" || notice.complete == Some(true));
        if is_ready {
            if state.ready_owners.insert(key.clone()) {
                state.order.push_back(ResidentMailboxEntry::Owner(key));
            }
        } else {
            // A newer incomplete revision supersedes a previously ready older
            // one. Its stale queue token is ignored by claim_ready.
            state.ready_owners.remove(&key);
        }
        drop(state);
        if is_ready {
            self.ready.notify_one();
        }
    }

    fn enqueue_fleet_fence(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A fleet fence is authoritative for every owner. Discard fragment
        // assemblies and deferred notices collected across the broken LISTEN
        // boundary; their stale queue tokens are removed with them.
        state.pending.clear();
        state.ready_owners.clear();
        state.waiting_for_successor.clear();
        state.pending_overlay.clear();
        state.overlay_ready_at = None;
        state.fence_epoch = state.fence_epoch.wrapping_add(1);
        state
            .order
            .retain(|entry| matches!(entry, ResidentMailboxEntry::FleetFence));
        if state.fleet_fence_pending {
            return;
        }
        state.fleet_fence_pending = true;
        state.order.push_back(ResidentMailboxEntry::FleetFence);
        drop(state);
        self.ready.notify_one();
    }

    fn requeue(&self, work: ResidentWork) {
        match work {
            ResidentWork::Notices(notices) => {
                for notice in notices {
                    self.enqueue(notice);
                }
            }
            ResidentWork::Overlay(batch) => self.requeue_overlay(batch),
            ResidentWork::FleetFence => self.enqueue_fleet_fence(),
        }
    }

    fn requeue_overlay(&self, batch: LiveOverlayBatch) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A newer fleet fence owns recovery for this entire failed batch.
        if state.fence_epoch != batch.fence_epoch {
            return;
        }
        for client_id in batch.client_ids {
            state
                .pending_overlay
                .entry(client_id.clone())
                .or_insert(RawProjectionNotice {
                    client_id,
                    generation: 0,
                    projected_seq: 0,
                });
        }
        // The batch already paid its collection window. Retry the exact
        // current-state read after reconnect backoff without adding another.
        state.overlay_ready_at = Some(
            state
                .overlay_ready_at
                .map_or_else(time::Instant::now, |deadline| {
                    deadline.min(time::Instant::now())
                }),
        );
        drop(state);
        self.ready.notify_one();
    }

    fn overlay_epoch_is_current(&self, fence_epoch: u64) -> bool {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fence_epoch
            == fence_epoch
    }

    fn defer_until_successor(&self, notice: DashboardNotice) {
        let key = ResidentOwnerKey::from_notice(&notice);
        let revision = notice
            .revision
            .expect("only a validated domain notice can await its successor");
        self.enqueue(notice);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(pending_revision) = state.pending.get(&key).and_then(|pending| pending.revision)
        else {
            // An authoritative fleet fence cleared this notice between the two
            // lock acquisitions.
            return;
        };
        if pending_revision > revision {
            return;
        }
        state.waiting_for_successor.insert(key.clone(), revision);
        state.ready_owners.remove(&key);
    }

    fn claim_ready(&self) -> Option<ResidentWork> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A reconnect/malformed-input fence supersedes every raw hint that was
        // collected before it. Hints received after the fence use its new
        // epoch and remain queued behind this authoritative fleet read.
        if std::mem::replace(&mut state.fleet_fence_pending, false) {
            state
                .order
                .retain(|entry| !matches!(entry, ResidentMailboxEntry::FleetFence));
            return Some(ResidentWork::FleetFence);
        }
        if state
            .overlay_ready_at
            .is_some_and(|deadline| deadline <= time::Instant::now())
        {
            state.overlay_ready_at = None;
            let client_ids = std::mem::take(&mut state.pending_overlay)
                .into_keys()
                .collect();
            return Some(ResidentWork::Overlay(LiveOverlayBatch {
                client_ids,
                fence_epoch: state.fence_epoch,
            }));
        }
        let mut notices = Vec::new();
        while let Some(entry) = state.order.pop_front() {
            match entry {
                ResidentMailboxEntry::Owner(key) => {
                    if state.ready_owners.remove(&key) {
                        if let Some(notice) = state.pending.remove(&key) {
                            notices.push(notice);
                        }
                    }
                }
                ResidentMailboxEntry::FleetFence => {
                    if std::mem::replace(&mut state.fleet_fence_pending, false) {
                        return Some(ResidentWork::FleetFence);
                    }
                }
            }
        }
        (!notices.is_empty()).then_some(ResidentWork::Notices(notices))
    }

    fn next_overlay_deadline(&self) -> Option<time::Instant> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .overlay_ready_at
    }

    async fn claim(&self, shutdown: &mut watch::Receiver<bool>) -> Option<ResidentWork> {
        loop {
            // Register before inspecting the queue so an enqueue between the
            // inspection and await cannot strand a ready owner.
            let notified = self.ready.notified();
            if let Some(work) = self.claim_ready() {
                return Some(work);
            }
            if shutdown_requested(shutdown) {
                return None;
            }
            if let Some(deadline) = self.next_overlay_deadline() {
                tokio::select! {
                    biased;
                    _ = shutdown_signal(shutdown) => return None,
                    _ = time::sleep_until(deadline) => {}
                    _ = notified => {}
                }
            } else {
                tokio::select! {
                    biased;
                    _ = shutdown_signal(shutdown) => return None,
                    _ = notified => {}
                }
            }
        }
    }
}

fn mutate_installed_fleet(
    resident: &DashboardTelemetryResident,
    mutate: impl FnOnce(&mut ResidentFleet),
) {
    let mut installed = resident
        .snapshot
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut fleet = installed.as_ref().clone();
    mutate(&mut fleet);
    *installed = Arc::new(fleet);
}

async fn reconcile_notice_owner(
    listener: &mut PgListener,
    resident: &DashboardTelemetryResident,
    client_id: &str,
    domain: &str,
) -> Result<()> {
    match domain {
        "resource" => match load_fenced_resource(listener, client_id).await? {
            Some((_, owner)) => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .resources
                        .insert(client_id.to_string(), Arc::new(RwLock::new(owner)));
                });
            }
            None => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.resources.remove(client_id);
                });
            }
        },
        "network" => match load_fenced_network(listener, client_id).await? {
            Some((_, owner)) => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .networks
                        .insert(client_id.to_string(), Arc::new(RwLock::new(owner)));
                });
            }
            None => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.networks.remove(client_id);
                });
            }
        },
        "traffic" => match load_fenced_traffic(listener, client_id).await? {
            Some((_, owner)) => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .traffics
                        .insert(client_id.to_string(), Arc::new(RwLock::new(owner)));
                });
            }
            None => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.traffics.remove(client_id);
                });
            }
        },
        "client" => match load_fenced_client(listener, client_id).await? {
            Some((_, resource, network, traffic)) => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .resources
                        .insert(client_id.to_string(), Arc::new(RwLock::new(resource)));
                    fleet
                        .networks
                        .insert(client_id.to_string(), Arc::new(RwLock::new(network)));
                    fleet
                        .traffics
                        .insert(client_id.to_string(), Arc::new(RwLock::new(traffic)));
                });
            }
            None => {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.resources.remove(client_id);
                    fleet.networks.remove(client_id);
                    fleet.traffics.remove(client_id);
                });
            }
        },
        _ => anyhow::bail!("invalid dashboard resident reconciliation domain"),
    }
    Ok(())
}

fn dashboard_notice_is_installed(
    resident: &DashboardTelemetryResident,
    notice: &DashboardNotice,
) -> bool {
    let Some(revision) = notice.revision else {
        return false;
    };
    let generation = notice
        .generation
        .expect("validated dashboard domain generation");
    let snapshot = resident.snapshot();
    match notice.domain.as_str() {
        "resource" => snapshot
            .resources
            .get(&notice.client_id)
            .is_some_and(|owner| {
                let owner = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                owner.revision > revision
                    || (owner.revision == revision && owner.generation == generation)
            }),
        "network" => snapshot
            .networks
            .get(&notice.client_id)
            .is_some_and(|owner| {
                let owner = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                owner.revision > revision
                    || (owner.revision == revision && owner.generation == generation)
            }),
        "traffic" => snapshot
            .traffics
            .get(&notice.client_id)
            .is_some_and(|owner| {
                let owner = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                owner.revision > revision
                    || (owner.revision == revision && owner.generation == generation)
            }),
        "client" => false,
        _ => unreachable!("validated dashboard notice domain"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DashboardNoticeApplication {
    AlreadyCurrent,
    ExactBlock,
    OwnerReplacement,
    AwaitingSuccessor,
}

impl DashboardNoticeApplication {
    fn requires_live_overlay(self) -> bool {
        matches!(self, Self::OwnerReplacement)
    }
}

async fn apply_dashboard_notice(
    listener: &mut PgListener,
    resident: &DashboardTelemetryResident,
    notice: DashboardNotice,
) -> Result<DashboardNoticeApplication> {
    // PostgreSQL notifications are commit hints, not a mutation journal.  A
    // later coherent resident revision already includes every earlier notice,
    // so discard it without spending a database round trip.
    if dashboard_notice_is_installed(resident, &notice) {
        return Ok(DashboardNoticeApplication::AlreadyCurrent);
    }
    if notice.domain == "client" {
        // Lifecycle notices deliberately carry no revision. A remove followed
        // by a reinitialize can commit before either hint is claimed, and a
        // failed connection attempt can requeue an older verb over a newer one.
        // Ignore the verb for mutation and install the current fenced owner (or
        // authoritative absence) so every coalesced/retried claim is exact.
        reconcile_notice_owner(listener, resident, &notice.client_id, "client").await?;
        return Ok(DashboardNoticeApplication::OwnerReplacement);
    }
    let generation = notice
        .generation
        .expect("validated dashboard domain generation");
    let revision = notice
        .revision
        .expect("validated dashboard domain revision");
    let previous_revision = notice
        .previous_revision
        .expect("validated dashboard domain previous revision");
    let Some(heads) = load_optional_client_heads(listener, &notice.client_id).await? else {
        mutate_installed_fleet(resident, |fleet| {
            fleet.resources.remove(&notice.client_id);
            fleet.networks.remove(&notice.client_id);
            fleet.traffics.remove(&notice.client_id);
        });
        return Ok(DashboardNoticeApplication::OwnerReplacement);
    };
    match notice.domain.as_str() {
        "resource" => {
            let snapshot = resident.snapshot();
            let current = snapshot.resources.get(&notice.client_id).cloned();
            if current.as_ref().is_some_and(|owner| {
                let owner = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                owner.generation == heads.resource.generation
                    && owner.revision == heads.resource.revision
            }) {
                return Ok(DashboardNoticeApplication::AlreadyCurrent);
            }
            let awaiting_successor = notice.change == "block"
                && current.as_ref().is_some_and(|owner| {
                    let owner = owner
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    block_notice_is_waiting_for_successor(
                        owner.generation,
                        owner.revision,
                        heads.resource.generation,
                        heads.resource.revision,
                        &heads.resource.change,
                        generation,
                        previous_revision,
                        revision,
                    )
                });
            if awaiting_successor {
                // The head commit is visible before its same-connection LISTEN
                // message was collected. Wait for that ordered successor hint;
                // disconnect and malformed-payload paths enqueue a fleet fence.
                return Ok(DashboardNoticeApplication::AwaitingSuccessor);
            }
            let exact = notice.change == "block"
                && current.as_ref().is_some_and(|owner| {
                    let owner = owner
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    resource_block_change_is_contiguous(
                        &owner,
                        &heads.resource,
                        generation,
                        previous_revision,
                        revision,
                    )
                });
            if exact {
                let mut change_head = heads.resource.clone();
                change_head.blocks = notice.block_keys();
                let (changes, overlay_blocks) =
                    load_resource_blocks(listener, &notice.client_id, &change_head).await?;
                let after = load_optional_client_heads(listener, &notice.client_id)
                    .await?
                    .map(|value| value.resource);
                if after.as_ref() != Some(&heads.resource)
                    && after.as_ref().is_some_and(|after| {
                        let owner = current
                            .as_ref()
                            .expect("exact dashboard resource owner")
                            .read()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        block_notice_is_waiting_for_successor(
                            owner.generation,
                            owner.revision,
                            after.generation,
                            after.revision,
                            &after.change,
                            generation,
                            previous_revision,
                            revision,
                        )
                    })
                {
                    return Ok(DashboardNoticeApplication::AwaitingSuccessor);
                }
                if after.as_ref() == Some(&heads.resource) {
                    let current = current.expect("exact dashboard resource owner");
                    let mut owner = current
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if owner.generation == heads.resource.generation
                        && owner.revision == previous_revision
                    {
                        owner.index.apply_blocks(changes);
                        for key in notice.block_keys().iter() {
                            owner
                                .overlay_blocks
                                .remove(&(key.source_bucket_secs, key.block_start_unix));
                        }
                        owner.overlay_blocks.extend(overlay_blocks);
                        owner.revision = heads.resource.revision;
                        return Ok(DashboardNoticeApplication::ExactBlock);
                    }
                }
            }
            if let Some((_, owner)) = load_fenced_resource(listener, &notice.client_id).await? {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .resources
                        .insert(notice.client_id, Arc::new(RwLock::new(owner)));
                });
            } else {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.resources.remove(&notice.client_id);
                });
            }
        }
        "network" => {
            let snapshot = resident.snapshot();
            let current = snapshot.networks.get(&notice.client_id).cloned();
            if current.as_ref().is_some_and(|owner| {
                let owner = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                owner.generation == heads.network.generation
                    && owner.revision == heads.network.revision
                    && owner.index.interfaces.as_ref() == heads.network.interfaces.as_ref()
            }) {
                return Ok(DashboardNoticeApplication::AlreadyCurrent);
            }
            let awaiting_successor = notice.change == "block"
                && current.as_ref().is_some_and(|owner| {
                    let owner = owner
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    owner.index.interfaces.as_ref() == heads.network.interfaces.as_ref()
                        && block_notice_is_waiting_for_successor(
                            owner.generation,
                            owner.revision,
                            heads.network.generation,
                            heads.network.revision,
                            &heads.network.change,
                            generation,
                            previous_revision,
                            revision,
                        )
                });
            if awaiting_successor {
                return Ok(DashboardNoticeApplication::AwaitingSuccessor);
            }
            let exact = notice.change == "block"
                && current.as_ref().is_some_and(|owner| {
                    let owner = owner
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    network_block_change_is_contiguous(
                        &owner,
                        &heads.network,
                        generation,
                        previous_revision,
                        revision,
                    )
                });
            if exact {
                let mut change_head = heads.network.clone();
                change_head.blocks = notice.block_keys();
                let (changes, overlay_blocks) =
                    load_network_blocks(listener, &notice.client_id, &change_head).await?;
                let after = load_optional_client_heads(listener, &notice.client_id)
                    .await?
                    .map(|value| value.network);
                if after.as_ref() != Some(&heads.network)
                    && after.as_ref().is_some_and(|after| {
                        let owner = current
                            .as_ref()
                            .expect("exact dashboard network owner")
                            .read()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        owner.index.interfaces.as_ref() == after.interfaces.as_ref()
                            && block_notice_is_waiting_for_successor(
                                owner.generation,
                                owner.revision,
                                after.generation,
                                after.revision,
                                &after.change,
                                generation,
                                previous_revision,
                                revision,
                            )
                    })
                {
                    return Ok(DashboardNoticeApplication::AwaitingSuccessor);
                }
                if after.as_ref() == Some(&heads.network) {
                    let current = current.expect("exact dashboard network owner");
                    let mut owner = current
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if owner.generation == heads.network.generation
                        && owner.revision == previous_revision
                        && owner.index.interfaces.as_ref() == heads.network.interfaces.as_ref()
                    {
                        owner.index.apply_blocks(changes);
                        for key in notice.block_keys().iter() {
                            owner
                                .overlay_blocks
                                .remove(&(key.source_bucket_secs, key.block_start_unix));
                        }
                        owner.overlay_blocks.extend(overlay_blocks);
                        owner.revision = heads.network.revision;
                        return Ok(DashboardNoticeApplication::ExactBlock);
                    }
                }
            }
            if let Some((_, owner)) = load_fenced_network(listener, &notice.client_id).await? {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .networks
                        .insert(notice.client_id, Arc::new(RwLock::new(owner)));
                });
            } else {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.networks.remove(&notice.client_id);
                });
            }
        }
        "traffic" => {
            let snapshot = resident.snapshot();
            let current = snapshot.traffics.get(&notice.client_id).cloned();
            if current.as_ref().is_some_and(|owner| {
                let owner = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                owner.generation == heads.traffic.generation
                    && owner.revision == heads.traffic.revision
                    && owner.source_kinds.as_ref() == heads.traffic.source_kinds.as_ref()
                    && owner.interfaces.as_ref() == heads.traffic.interfaces.as_ref()
            }) {
                return Ok(DashboardNoticeApplication::AlreadyCurrent);
            }
            let awaiting_successor = notice.change == "block"
                && current.as_ref().is_some_and(|owner| {
                    let owner = owner
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    owner.source_kinds.as_ref() == heads.traffic.source_kinds.as_ref()
                        && owner.interfaces.as_ref() == heads.traffic.interfaces.as_ref()
                        && block_notice_is_waiting_for_successor(
                            owner.generation,
                            owner.revision,
                            heads.traffic.generation,
                            heads.traffic.revision,
                            &heads.traffic.change,
                            generation,
                            previous_revision,
                            revision,
                        )
                });
            if awaiting_successor {
                return Ok(DashboardNoticeApplication::AwaitingSuccessor);
            }
            let exact = notice.change == "block"
                && current.as_ref().is_some_and(|owner| {
                    let owner = owner
                        .read()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    traffic_block_change_is_contiguous(
                        &owner,
                        &heads.traffic,
                        generation,
                        previous_revision,
                        revision,
                    )
                });
            if exact {
                let mut change_head = heads.traffic.clone();
                change_head.blocks = notice.block_keys();
                let (changes, overlay_blocks) =
                    load_traffic_blocks(listener, &notice.client_id, &change_head).await?;
                let after = load_optional_client_heads(listener, &notice.client_id)
                    .await?
                    .map(|value| value.traffic);
                if after.as_ref() != Some(&heads.traffic)
                    && after.as_ref().is_some_and(|after| {
                        let owner = current
                            .as_ref()
                            .expect("exact dashboard traffic owner")
                            .read()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        owner.source_kinds.as_ref() == after.source_kinds.as_ref()
                            && owner.interfaces.as_ref() == after.interfaces.as_ref()
                            && block_notice_is_waiting_for_successor(
                                owner.generation,
                                owner.revision,
                                after.generation,
                                after.revision,
                                &after.change,
                                generation,
                                previous_revision,
                                revision,
                            )
                    })
                {
                    return Ok(DashboardNoticeApplication::AwaitingSuccessor);
                }
                if after.as_ref() == Some(&heads.traffic) {
                    let current = current.expect("exact dashboard traffic owner");
                    let mut owner = current
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if owner.generation == heads.traffic.generation
                        && owner.revision == previous_revision
                        && owner.source_kinds.as_ref() == heads.traffic.source_kinds.as_ref()
                        && owner.interfaces.as_ref() == heads.traffic.interfaces.as_ref()
                    {
                        owner.index.apply_blocks(changes);
                        for key in notice.block_keys().iter() {
                            owner
                                .overlay_blocks
                                .remove(&(key.source_bucket_secs, key.block_start_unix));
                        }
                        owner.overlay_blocks.extend(overlay_blocks);
                        owner.revision = heads.traffic.revision;
                        return Ok(DashboardNoticeApplication::ExactBlock);
                    }
                }
            }
            if let Some((_, owner)) = load_fenced_traffic(listener, &notice.client_id).await? {
                mutate_installed_fleet(resident, |fleet| {
                    fleet
                        .traffics
                        .insert(notice.client_id, Arc::new(RwLock::new(owner)));
                });
            } else {
                mutate_installed_fleet(resident, |fleet| {
                    fleet.traffics.remove(&notice.client_id);
                });
            }
        }
        _ => unreachable!(),
    }
    Ok(DashboardNoticeApplication::OwnerReplacement)
}

async fn reconcile_collected_notices(
    listener: &mut PgListener,
    resident: &DashboardTelemetryResident,
    events: &WsEventBus,
    notices: Vec<DashboardNotice>,
) -> Result<Vec<(DashboardNotice, DashboardNoticeApplication)>> {
    let mut completed = Vec::with_capacity(notices.len());
    let mut candidates = Vec::new();
    for (ordinal, notice) in notices.into_iter().enumerate() {
        if dashboard_notice_is_installed(resident, &notice) {
            completed.push((ordinal, notice, DashboardNoticeApplication::AlreadyCurrent));
        } else {
            candidates.push((ordinal, notice));
        }
    }
    if candidates.is_empty() {
        return Ok(completed
            .into_iter()
            .map(|(_, notice, application)| (notice, application))
            .collect());
    }

    let client_ids = candidates
        .iter()
        .map(|(_, notice)| notice.client_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let heads = load_selected_heads(listener, &client_ids).await?;
    let snapshot = resident.snapshot();
    let mut resource_targets = BTreeMap::new();
    let mut network_targets = BTreeMap::new();
    let mut traffic_targets = BTreeMap::new();
    let mut fallback = Vec::new();

    for (ordinal, notice) in candidates {
        let Some(client_heads) = heads.get(&notice.client_id) else {
            fallback.push((ordinal, notice));
            continue;
        };
        let (Some(generation), Some(previous_revision), Some(revision)) =
            (notice.generation, notice.previous_revision, notice.revision)
        else {
            // Client lifecycle notices deliberately have no revision fence and
            // retain their established current-owner reconciliation.
            fallback.push((ordinal, notice));
            continue;
        };
        match notice.domain.as_str() {
            "resource" => {
                let Some(owner) = snapshot.resources.get(&notice.client_id).cloned() else {
                    fallback.push((ordinal, notice));
                    continue;
                };
                let installed = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if installed.generation == client_heads.resource.generation
                    && installed.revision == client_heads.resource.revision
                {
                    drop(installed);
                    completed.push((ordinal, notice, DashboardNoticeApplication::AlreadyCurrent));
                } else if notice.change == "block"
                    && block_notice_is_waiting_for_successor(
                        installed.generation,
                        installed.revision,
                        client_heads.resource.generation,
                        client_heads.resource.revision,
                        &client_heads.resource.change,
                        generation,
                        previous_revision,
                        revision,
                    )
                {
                    drop(installed);
                    completed.push((
                        ordinal,
                        notice,
                        DashboardNoticeApplication::AwaitingSuccessor,
                    ));
                } else if notice.change == "block"
                    && resource_block_change_is_contiguous(
                        &installed,
                        &client_heads.resource,
                        generation,
                        previous_revision,
                        revision,
                    )
                {
                    drop(installed);
                    let blocks = notice.block_keys();
                    let client_id = notice.client_id.clone();
                    anyhow::ensure!(
                        resource_targets
                            .insert(
                                client_id,
                                ResourceNoticeTarget {
                                    ordinal,
                                    notice,
                                    owner,
                                    head: client_heads.resource.clone(),
                                    blocks,
                                },
                            )
                            .is_none(),
                        "dashboard resource notice cohort duplicated an owner"
                    );
                } else {
                    drop(installed);
                    fallback.push((ordinal, notice));
                }
            }
            "network" => {
                let Some(owner) = snapshot.networks.get(&notice.client_id).cloned() else {
                    fallback.push((ordinal, notice));
                    continue;
                };
                let installed = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if installed.generation == client_heads.network.generation
                    && installed.revision == client_heads.network.revision
                    && installed.index.interfaces.as_ref()
                        == client_heads.network.interfaces.as_ref()
                {
                    drop(installed);
                    completed.push((ordinal, notice, DashboardNoticeApplication::AlreadyCurrent));
                } else if notice.change == "block"
                    && installed.index.interfaces.as_ref()
                        == client_heads.network.interfaces.as_ref()
                    && block_notice_is_waiting_for_successor(
                        installed.generation,
                        installed.revision,
                        client_heads.network.generation,
                        client_heads.network.revision,
                        &client_heads.network.change,
                        generation,
                        previous_revision,
                        revision,
                    )
                {
                    drop(installed);
                    completed.push((
                        ordinal,
                        notice,
                        DashboardNoticeApplication::AwaitingSuccessor,
                    ));
                } else if notice.change == "block"
                    && network_block_change_is_contiguous(
                        &installed,
                        &client_heads.network,
                        generation,
                        previous_revision,
                        revision,
                    )
                {
                    drop(installed);
                    let blocks = notice.block_keys();
                    let client_id = notice.client_id.clone();
                    anyhow::ensure!(
                        network_targets
                            .insert(
                                client_id,
                                NetworkNoticeTarget {
                                    ordinal,
                                    notice,
                                    owner,
                                    head: client_heads.network.clone(),
                                    blocks,
                                },
                            )
                            .is_none(),
                        "dashboard network notice cohort duplicated an owner"
                    );
                } else {
                    drop(installed);
                    fallback.push((ordinal, notice));
                }
            }
            "traffic" => {
                let Some(owner) = snapshot.traffics.get(&notice.client_id).cloned() else {
                    fallback.push((ordinal, notice));
                    continue;
                };
                let installed = owner
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let selection_matches = installed.source_kinds.as_ref()
                    == client_heads.traffic.source_kinds.as_ref()
                    && installed.interfaces.as_ref() == client_heads.traffic.interfaces.as_ref();
                if installed.generation == client_heads.traffic.generation
                    && installed.revision == client_heads.traffic.revision
                    && selection_matches
                {
                    drop(installed);
                    completed.push((ordinal, notice, DashboardNoticeApplication::AlreadyCurrent));
                } else if notice.change == "block"
                    && selection_matches
                    && block_notice_is_waiting_for_successor(
                        installed.generation,
                        installed.revision,
                        client_heads.traffic.generation,
                        client_heads.traffic.revision,
                        &client_heads.traffic.change,
                        generation,
                        previous_revision,
                        revision,
                    )
                {
                    drop(installed);
                    completed.push((
                        ordinal,
                        notice,
                        DashboardNoticeApplication::AwaitingSuccessor,
                    ));
                } else if notice.change == "block"
                    && traffic_block_change_is_contiguous(
                        &installed,
                        &client_heads.traffic,
                        generation,
                        previous_revision,
                        revision,
                    )
                {
                    drop(installed);
                    let blocks = notice.block_keys();
                    let client_id = notice.client_id.clone();
                    anyhow::ensure!(
                        traffic_targets
                            .insert(
                                client_id,
                                TrafficNoticeTarget {
                                    ordinal,
                                    notice,
                                    owner,
                                    head: client_heads.traffic.clone(),
                                    blocks,
                                },
                            )
                            .is_none(),
                        "dashboard traffic notice cohort duplicated an owner"
                    );
                } else {
                    drop(installed);
                    fallback.push((ordinal, notice));
                }
            }
            "client" => fallback.push((ordinal, notice)),
            _ => unreachable!("validated dashboard notice domain"),
        }
    }
    drop(snapshot);

    let mut resource_changes = load_resource_notice_changes(listener, &resource_targets).await?;
    let mut network_changes = load_network_notice_changes(listener, &network_targets).await?;
    let mut traffic_changes = load_traffic_notice_changes(listener, &traffic_targets).await?;
    let exact_client_ids = resource_targets
        .keys()
        .chain(network_targets.keys())
        .chain(traffic_targets.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let after = load_selected_heads(listener, &exact_client_ids).await?;

    for (client_id, target) in resource_targets {
        let Some(mut change) = resource_changes.remove(&client_id) else {
            anyhow::bail!("dashboard resource notice change is missing");
        };
        let generation = target.notice.generation.expect("validated generation");
        let previous_revision = target
            .notice
            .previous_revision
            .expect("validated previous revision");
        let revision = target.notice.revision.expect("validated revision");
        let after_head = after.get(&client_id).map(|heads| &heads.resource);
        let mut installed = target
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if after_head != Some(&target.head) {
            let waiting = after_head.is_some_and(|head| {
                block_notice_is_waiting_for_successor(
                    installed.generation,
                    installed.revision,
                    head.generation,
                    head.revision,
                    &head.change,
                    generation,
                    previous_revision,
                    revision,
                )
            });
            drop(installed);
            if waiting {
                completed.push((
                    target.ordinal,
                    target.notice,
                    DashboardNoticeApplication::AwaitingSuccessor,
                ));
            } else {
                fallback.push((target.ordinal, target.notice));
            }
        } else if resource_block_change_is_contiguous(
            &installed,
            &target.head,
            generation,
            previous_revision,
            revision,
        ) {
            installed.index.apply_blocks(change.blocks);
            for key in target.blocks.iter() {
                installed
                    .overlay_blocks
                    .remove(&(key.source_bucket_secs, key.block_start_unix));
            }
            installed.overlay_blocks.append(&mut change.overlay_blocks);
            installed.revision = target.head.revision;
            drop(installed);
            events.notify_fleet_telemetry();
            completed.push((
                target.ordinal,
                target.notice,
                DashboardNoticeApplication::ExactBlock,
            ));
        } else {
            drop(installed);
            fallback.push((target.ordinal, target.notice));
        }
    }
    for (client_id, target) in network_targets {
        let Some(mut change) = network_changes.remove(&client_id) else {
            anyhow::bail!("dashboard network notice change is missing");
        };
        let generation = target.notice.generation.expect("validated generation");
        let previous_revision = target
            .notice
            .previous_revision
            .expect("validated previous revision");
        let revision = target.notice.revision.expect("validated revision");
        let after_head = after.get(&client_id).map(|heads| &heads.network);
        let mut installed = target
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if after_head != Some(&target.head) {
            let waiting = after_head.is_some_and(|head| {
                installed.index.interfaces.as_ref() == head.interfaces.as_ref()
                    && block_notice_is_waiting_for_successor(
                        installed.generation,
                        installed.revision,
                        head.generation,
                        head.revision,
                        &head.change,
                        generation,
                        previous_revision,
                        revision,
                    )
            });
            drop(installed);
            if waiting {
                completed.push((
                    target.ordinal,
                    target.notice,
                    DashboardNoticeApplication::AwaitingSuccessor,
                ));
            } else {
                fallback.push((target.ordinal, target.notice));
            }
        } else if network_block_change_is_contiguous(
            &installed,
            &target.head,
            generation,
            previous_revision,
            revision,
        ) {
            installed.index.apply_blocks(change.blocks);
            for key in target.blocks.iter() {
                installed
                    .overlay_blocks
                    .remove(&(key.source_bucket_secs, key.block_start_unix));
            }
            installed.overlay_blocks.append(&mut change.overlay_blocks);
            installed.revision = target.head.revision;
            drop(installed);
            events.notify_fleet_telemetry();
            completed.push((
                target.ordinal,
                target.notice,
                DashboardNoticeApplication::ExactBlock,
            ));
        } else {
            drop(installed);
            fallback.push((target.ordinal, target.notice));
        }
    }
    for (client_id, target) in traffic_targets {
        let Some(mut change) = traffic_changes.remove(&client_id) else {
            anyhow::bail!("dashboard traffic notice change is missing");
        };
        let generation = target.notice.generation.expect("validated generation");
        let previous_revision = target
            .notice
            .previous_revision
            .expect("validated previous revision");
        let revision = target.notice.revision.expect("validated revision");
        let after_head = after.get(&client_id).map(|heads| &heads.traffic);
        let mut installed = target
            .owner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if after_head != Some(&target.head) {
            let waiting = after_head.is_some_and(|head| {
                installed.source_kinds.as_ref() == head.source_kinds.as_ref()
                    && installed.interfaces.as_ref() == head.interfaces.as_ref()
                    && block_notice_is_waiting_for_successor(
                        installed.generation,
                        installed.revision,
                        head.generation,
                        head.revision,
                        &head.change,
                        generation,
                        previous_revision,
                        revision,
                    )
            });
            drop(installed);
            if waiting {
                completed.push((
                    target.ordinal,
                    target.notice,
                    DashboardNoticeApplication::AwaitingSuccessor,
                ));
            } else {
                fallback.push((target.ordinal, target.notice));
            }
        } else if traffic_block_change_is_contiguous(
            &installed,
            &target.head,
            generation,
            previous_revision,
            revision,
        ) {
            installed.index.apply_blocks(change.blocks);
            for key in target.blocks.iter() {
                installed
                    .overlay_blocks
                    .remove(&(key.source_bucket_secs, key.block_start_unix));
            }
            installed.overlay_blocks.append(&mut change.overlay_blocks);
            installed.revision = target.head.revision;
            drop(installed);
            events.notify_fleet_telemetry();
            completed.push((
                target.ordinal,
                target.notice,
                DashboardNoticeApplication::ExactBlock,
            ));
        } else {
            drop(installed);
            fallback.push((target.ordinal, target.notice));
        }
    }
    anyhow::ensure!(
        resource_changes.is_empty() && network_changes.is_empty() && traffic_changes.is_empty(),
        "dashboard notice change escaped its exact owner"
    );

    // Generation, lifecycle and raced owners are not ordinary coordinate
    // work. Preserve their established authoritative reconciliation rather
    // than broadening the setwise fast path into a different state machine.
    for (ordinal, notice) in fallback {
        let application = reconcile_collected_notice(listener, resident, events, &notice).await?;
        completed.push((ordinal, notice, application));
    }
    completed.sort_unstable_by_key(|(ordinal, _, _)| *ordinal);
    Ok(completed
        .into_iter()
        .map(|(_, notice, application)| (notice, application))
        .collect())
}

pub(crate) struct DashboardTelemetryResidentTask {
    shutdown: watch::Sender<bool>,
    handles: Vec<JoinHandle<()>>,
    listener_pool: sqlx::PgPool,
    reconciler_pool: sqlx::PgPool,
}

impl DashboardTelemetryResidentTask {
    pub(crate) fn request_shutdown(&self) {
        let _ = self.shutdown.send(true);
    }

    pub(crate) async fn wait_for_unexpected_exit(&mut self) -> Result<()> {
        let (result, lane, remaining) =
            futures_util::future::select_all(self.handles.iter_mut()).await;
        drop(remaining);
        drop(self.handles.swap_remove(lane));
        match result {
            Ok(()) => anyhow::bail!("dashboard telemetry resident lane {lane} exited unexpectedly"),
            Err(error) => Err(error)
                .with_context(|| format!("dashboard telemetry resident lane {lane} failed")),
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
        self.listener_pool.close().await;
        self.reconciler_pool.close().await;
        match first_join_error {
            Some(error) => Err(error).context("dashboard telemetry resident task failed"),
            None => Ok(()),
        }
    }

    pub(crate) async fn shutdown(self) -> Result<()> {
        self.request_shutdown();
        self.join().await
    }
}

impl DashboardTelemetryResident {
    pub(crate) async fn initialize(
        repo: &Repository,
        events: WsEventBus,
    ) -> Result<(Self, DashboardTelemetryResidentTask)> {
        let Repository::Postgres(pool) = repo;
        let listener_connect_options: PgConnectOptions = (*pool.connect_options())
            .clone()
            .application_name(DASHBOARD_RESIDENT_LISTENER_APPLICATION_NAME);
        let listener_pool = PgPoolOptions::new()
            .max_connections(1)
            .max_lifetime(None)
            .idle_timeout(None)
            .connect_lazy_with(listener_connect_options);
        // Keep the collector independent from exactly one database
        // reconciliation lane.  This makes incoming commit hints cheap without
        // hiding owner-query cost behind unproven database concurrency.
        let reconciler_connect_options: PgConnectOptions = (*pool.connect_options())
            .clone()
            .application_name(DASHBOARD_RESIDENT_RECONCILER_APPLICATION_NAME);
        let reconciler_pool = PgPoolOptions::new()
            .max_connections(1)
            .max_lifetime(None)
            .idle_timeout(None)
            .connect_lazy_with(reconciler_connect_options);
        let mut listener = PgListener::connect_with(&listener_pool)
            .await
            .context("failed to connect dashboard resident listener")?;
        listener.eager_reconnect(false);
        listener
            .listen(TELEMETRY_PROJECTION_CHANNEL)
            .await
            .context("failed to establish dashboard resident LISTEN fence")?;
        let fleet = seed_fleet(&reconciler_pool)
            .await
            .context("failed to seed dashboard resident generations")?;
        let seeded_client_ids = fleet.resources.keys().cloned().collect::<Vec<_>>();
        let resident = Self {
            snapshot: Arc::new(RwLock::new(Arc::new(fleet))),
        };
        let (shutdown, shutdown_rx) = watch::channel(false);
        let mailbox = Arc::new(ResidentMailbox::default());
        for client_id in &seeded_client_ids {
            mailbox.enqueue_live_overlay(client_id);
        }
        let listener_handle = tokio::spawn(run_resident_listener(
            listener_pool.clone(),
            listener,
            Arc::clone(&mailbox),
            events.clone(),
            shutdown_rx.clone(),
        ));
        let reconciler_handle = tokio::spawn(run_resident_reconciler(
            reconciler_pool.clone(),
            mailbox,
            resident.clone(),
            events,
            shutdown_rx,
        ));
        info!(
            channel = TELEMETRY_PROJECTION_CHANNEL,
            reconciliation_lanes = 1,
            "dashboard telemetry resident cache is ready"
        );
        Ok((
            resident,
            DashboardTelemetryResidentTask {
                shutdown,
                handles: vec![listener_handle, reconciler_handle],
                listener_pool,
                reconciler_pool,
            },
        ))
    }
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

async fn reconcile_fleet_fence(
    listener: &mut PgListener,
    resident: &DashboardTelemetryResident,
    mailbox: &ResidentMailbox,
) -> Result<()> {
    let heads = load_heads(listener).await?;
    let installed = resident.snapshot();
    // A client id can be physically deleted and recreated while LISTEN is down.
    // That new incarnation resets its head revisions, so an ordinary domain
    // notice cannot be safely ordered against the installed incarnation. This
    // exceptional fence reloads every current client authoritatively; normal
    // connected notifications retain their exact block/gap paths below.
    for client_id in heads.keys() {
        mailbox.enqueue(DashboardNotice {
            owner: "dashboard".to_string(),
            client_id: client_id.clone(),
            domain: "client".to_string(),
            change: "initialize".to_string(),
            generation: None,
            previous_revision: None,
            revision: None,
            source_bucket_secs: None,
            block_start_unix: None,
            complete: None,
        });
    }
    for client_id in installed
        .resources
        .keys()
        .chain(installed.networks.keys())
        .chain(installed.traffics.keys())
        .filter(|client_id| !heads.contains_key(*client_id))
        .collect::<BTreeSet<_>>()
    {
        mailbox.enqueue(DashboardNotice {
            owner: "dashboard".to_string(),
            client_id: client_id.clone(),
            domain: "client".to_string(),
            change: "remove".to_string(),
            generation: None,
            previous_revision: None,
            revision: None,
            source_bucket_secs: None,
            block_start_unix: None,
            complete: None,
        });
    }
    Ok(())
}

async fn connect_listener(pool: &sqlx::PgPool) -> Result<PgListener> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.eager_reconnect(false);
    listener.listen(TELEMETRY_PROJECTION_CHANNEL).await?;
    Ok(listener)
}

fn collect_notification(
    mailbox: &ResidentMailbox,
    events: &WsEventBus,
    payload: &str,
) -> Result<()> {
    match ProjectionNotice::parse(payload) {
        Some(ProjectionNotice::Dashboard(notice)) => {
            let publication_complete = notice.domain == "client" || notice.complete == Some(true);
            mailbox.enqueue(notice);
            if publication_complete {
                events.notify_fleet_telemetry();
            }
        }
        Some(ProjectionNotice::Raw(notice)) => {
            // Commit hints coalesce by client. The reconciler rereads the
            // canonical live owner, so duplicate or out-of-order hints cannot
            // replay stale telemetry into resident history.
            mailbox.enqueue_overlay(notice);
        }
        None => {
            anyhow::bail!("invalid telemetry projection notification payload");
        }
    }
    Ok(())
}

async fn reconnect_until_ready(
    pool: &sqlx::PgPool,
    mailbox: &ResidentMailbox,
    shutdown: &mut watch::Receiver<bool>,
    reconnect_delay: &mut Duration,
) -> Option<PgListener> {
    loop {
        if shutdown_requested(shutdown) {
            return None;
        }
        let waited = tokio::select! {
            biased;
            _ = shutdown_signal(shutdown) => true,
            _ = time::sleep(*reconnect_delay) => false,
        };
        if waited {
            return None;
        }
        match connect_listener(pool).await {
            Ok(listener) => {
                // LISTEN is established before the sole SQL lane scans heads.
                // Commits before this fence are visible to that scan; commits
                // after it remain queued on this connection as notifications.
                mailbox.enqueue_fleet_fence();
                *reconnect_delay = RECONNECT_MIN_DELAY;
                return Some(listener);
            }
            Err(error) => {
                warn!(%error, "dashboard resident reconnect fence failed");
                *reconnect_delay = reconnect_delay
                    .checked_mul(2)
                    .unwrap_or(RECONNECT_MAX_DELAY)
                    .min(RECONNECT_MAX_DELAY);
            }
        }
    }
}

async fn reconcile_collected_notice(
    connection: &mut PgListener,
    resident: &DashboardTelemetryResident,
    events: &WsEventBus,
    notice: &DashboardNotice,
) -> Result<DashboardNoticeApplication> {
    if dashboard_notice_is_installed(resident, notice) {
        return Ok(DashboardNoticeApplication::AlreadyCurrent);
    }
    let client_id = notice.client_id.clone();
    let domain = notice.domain.clone();
    let application = match apply_dashboard_notice(connection, resident, notice.clone()).await {
        Ok(DashboardNoticeApplication::AwaitingSuccessor) => {
            return Ok(DashboardNoticeApplication::AwaitingSuccessor);
        }
        Ok(application) => application,
        Err(error) => {
            if domain == "client" {
                // Lifecycle application already is the exact fenced owner load.
                // Let the mailbox retry it after reconnect instead of immediately
                // duplicating the same full-owner query on this connection.
                return Err(error.context("dashboard client owner reconciliation failed"));
            }
            reconcile_notice_owner(connection, resident, &client_id, &domain)
                .await
                .with_context(|| {
                    format!("dashboard {domain} owner reconciliation failed after: {error:#}")
                })?;
            warn!(%error, %client_id, %domain,
                "dashboard notice used exact owner reconciliation");
            events.invalidate_fleet_telemetry_read_cache();
            DashboardNoticeApplication::OwnerReplacement
        }
    };
    // The collector also schedules the normal coalesced refresh immediately.
    // Schedule again after installation so a slow exact reconciliation cannot
    // leave browsers on the pre-install boundary.
    events.notify_fleet_telemetry();
    Ok(application)
}

async fn run_resident_reconciler(
    reconciler_pool: sqlx::PgPool,
    mailbox: Arc<ResidentMailbox>,
    resident: DashboardTelemetryResident,
    events: WsEventBus,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut connection = None;
    let mut retry_delay = RECONNECT_MIN_DELAY;
    loop {
        let Some(mut work) = mailbox.claim(&mut shutdown).await else {
            break;
        };
        if let ResidentWork::Notices(notices) = &mut work {
            notices.retain(|notice| !dashboard_notice_is_installed(&resident, notice));
            if notices.is_empty() {
                continue;
            }
        }
        if connection.is_none() {
            let connected = tokio::select! {
                biased;
                _ = shutdown_signal(&mut shutdown) => break,
                result = PgListener::connect_with(&reconciler_pool) => result,
            };
            match connected {
                Ok(mut reconciler) => {
                    reconciler.eager_reconnect(false);
                    connection = Some(reconciler);
                }
                Err(error) => {
                    warn!(%error, "dashboard resident reconciler connection failed");
                    mailbox.requeue(work);
                    let stopped = tokio::select! {
                        biased;
                        _ = shutdown_signal(&mut shutdown) => true,
                        _ = time::sleep(retry_delay) => false,
                    };
                    if stopped {
                        break;
                    }
                    retry_delay = retry_delay
                        .checked_mul(2)
                        .unwrap_or(RECONNECT_MAX_DELAY)
                        .min(RECONNECT_MAX_DELAY);
                    continue;
                }
            }
        }
        let reconciler = connection
            .as_mut()
            .expect("connected dashboard resident reconciler");
        let result: Result<Vec<(DashboardNotice, DashboardNoticeApplication)>> = match &work {
            ResidentWork::Notices(notices) => {
                reconcile_collected_notices(reconciler, &resident, &events, notices.clone()).await
            }
            ResidentWork::Overlay(batch) => {
                match reconcile_live_overlays(reconciler, &resident, &batch.client_ids).await {
                    Ok(()) => {
                        // A fence collected while this exact reread was in
                        // flight owns the next public boundary. Do not publish
                        // the superseded raw batch ahead of it.
                        if mailbox.overlay_epoch_is_current(batch.fence_epoch) {
                            events.notify_fleet_telemetry();
                        }
                        Ok(Vec::new())
                    }
                    Err(error) => Err(error),
                }
            }
            ResidentWork::FleetFence => {
                match reconcile_fleet_fence(reconciler, &resident, &mailbox).await {
                    Ok(()) => {
                        // Match the prior reconnect boundary: the head fence is
                        // queued before browsers refetch, then exact owners emit
                        // their normal post-install coalesced notifications.
                        events.invalidate_fleet_telemetry_read_cache();
                        events.notify_fleet_telemetry();
                        Ok(Vec::new())
                    }
                    Err(error) => Err(error),
                }
            }
        };
        match result {
            Ok(applications) => {
                for (notice, application) in applications {
                    if application.requires_live_overlay() {
                        mailbox.enqueue_live_overlay(&notice.client_id);
                    }
                    if application == DashboardNoticeApplication::AwaitingSuccessor {
                        mailbox.defer_until_successor(notice);
                    }
                }
                retry_delay = RECONNECT_MIN_DELAY;
            }
            Err(error) => {
                match &work {
                    ResidentWork::Notices(notices) => {
                        warn!(%error, owners = notices.len(),
                            "dashboard resident owner cohort remains queued for reconciliation");
                    }
                    ResidentWork::Overlay(batch) => {
                        warn!(%error, clients = batch.client_ids.len(),
                            "dashboard resident live overlays remain queued for reconciliation");
                    }
                    ResidentWork::FleetFence => {
                        warn!(%error,
                            "dashboard resident fleet fence remains queued for reconciliation");
                    }
                }
                mailbox.requeue(work);
                connection = None;
                let stopped = tokio::select! {
                    biased;
                    _ = shutdown_signal(&mut shutdown) => true,
                    _ = time::sleep(retry_delay) => false,
                };
                if stopped {
                    break;
                }
                retry_delay = retry_delay
                    .checked_mul(2)
                    .unwrap_or(RECONNECT_MAX_DELAY)
                    .min(RECONNECT_MAX_DELAY);
            }
        }
    }
    drop(connection);
}

async fn run_resident_listener(
    listener_pool: sqlx::PgPool,
    mut listener: PgListener,
    mailbox: Arc<ResidentMailbox>,
    events: WsEventBus,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut reconnect_delay = RECONNECT_MIN_DELAY;
    'resident: loop {
        if shutdown_requested(&shutdown) {
            break;
        }
        let receive = tokio::select! {
            biased;
            _ = shutdown_signal(&mut shutdown) => break 'resident,
            result = listener.recv() => result,
        };
        match receive {
            Ok(notification) => {
                reconnect_delay = RECONNECT_MIN_DELAY;
                if notification.channel() != TELEMETRY_PROJECTION_CHANNEL {
                    continue;
                }
                if let Err(error) = collect_notification(&mailbox, &events, notification.payload())
                {
                    warn!(%error, payload = notification.payload(),
                        "dashboard resident notification queued a fleet fence");
                    // The LISTEN connection is healthy. Keep collecting and let
                    // the sole SQL lane establish authoritative current heads.
                    mailbox.enqueue_fleet_fence();
                }
            }
            Err(error) => {
                warn!(%error, "dashboard resident listener disconnected; serving its last coherent snapshot");
                // PgListener owns the pool's only checkout. Release it before
                // trying to acquire the replacement from this one-slot pool.
                drop(listener);
                let Some(reconnected) = reconnect_until_ready(
                    &listener_pool,
                    &mailbox,
                    &mut shutdown,
                    &mut reconnect_delay,
                )
                .await
                else {
                    break 'resident;
                };
                listener = reconnected;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_block_reads_are_coordinate_driven_and_keep_the_publication_fence() {
        for (sql, table) in [
            (
                RESOURCE_COORDINATE_BLOCKS_SQL,
                "telemetry_dashboard_resource_blocks",
            ),
            (
                NETWORK_COORDINATE_BLOCKS_SQL,
                "telemetry_dashboard_network_blocks",
            ),
            (
                TRAFFIC_COORDINATE_BLOCKS_SQL,
                "telemetry_dashboard_traffic_blocks",
            ),
        ] {
            let coordinates = sql.find("FROM UNNEST(").expect("coordinate relation");
            let owner = sql.find(&format!("JOIN {table}")).expect("owner table");
            assert!(coordinates < owner);
            assert!(sql.contains("client_id = $1"));
            assert!(sql.contains("generation = $2"));
            assert!(sql.contains("published_revision <= $3"));
            assert!(sql.contains("source_bucket_secs = coordinate.source_bucket_secs"));
            assert!(sql.contains("block_start_unix = coordinate.block_start_unix"));
        }
        assert!(RESOURCE_OVERLAY_SQL.contains(
            "telemetry_dashboard_resource_overlay_source(\n    ARRAY[$1::TEXT], $2::INTEGER[], $3::BIGINT[]"
        ));
        assert!(NETWORK_OVERLAY_SQL.contains(
            "telemetry_dashboard_network_overlay_source(\n        ARRAY[$1::TEXT], $2::INTEGER[], $3::BIGINT[]"
        ));
        assert!(TRAFFIC_OVERLAY_SQL.contains(
            "telemetry_dashboard_traffic_overlay_source(\n    ARRAY[$1::TEXT], $2::INTEGER[], $3::BIGINT[]"
        ));

        let source = include_str!("dashboard_telemetry_resident.rs");
        for loader in [
            "load_resource_blocks",
            "load_network_blocks",
            "load_traffic_blocks",
        ] {
            let (_, body) = source
                .split_once(&format!("async fn {loader}"))
                .unwrap_or_else(|| panic!("missing {loader}"));
            let (body, _) = body
                .split_once("\n}\n")
                .unwrap_or_else(|| panic!("missing {loader} boundary"));
            assert!(body.contains(".bind(&tiers)"));
            assert!(body.contains(".bind(&starts)"));
        }
    }

    #[test]
    fn live_overlays_use_one_coordinate_driven_owner_model_without_mirrors() {
        for sql in [NETWORK_OVERLAY_SQL, OVERLAY_NETWORK_SOURCE_SQL] {
            assert!(sql.contains("telemetry_dashboard_network_overlay_source"));
            assert!(!sql.contains("telemetry_network_live_rates"));
        }
        for sql in [TRAFFIC_OVERLAY_SQL, OVERLAY_TRAFFIC_SOURCE_SQL] {
            assert!(sql.contains("telemetry_dashboard_traffic_overlay_source"));
            assert!(!sql.contains("telemetry_network_live_rates"));
        }
        for sql in [RESOURCE_OVERLAY_SQL, OVERLAY_RESOURCE_SOURCE_SQL] {
            assert!(sql.contains("telemetry_dashboard_resource_overlay_source"));
            assert!(!sql.contains("telemetry_rollups retained"));
        }
        let migration = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../migrations/0006_telemetry_dashboard.sql"
        ));
        let claim = migration
            .split_once("CREATE FUNCTION public.claim_telemetry_dashboard_projection(")
            .expect("dashboard projection claim")
            .1
            .split_once("$$;")
            .expect("dashboard projection claim boundary")
            .0;
        assert!(claim.contains("captured_events AS MATERIALIZED"));
        assert!(claim.contains("SELECT 'full_block'::TEXT AS event_kind"));
        assert!(claim.contains("WHERE event.event_kind = 'full_block'"));
        assert!(claim.contains("WHERE event.event_kind = 'coordinate'"));
        assert!(claim.contains("FROM captured_events whole"));
        assert!(claim.contains("whole.event_kind = 'full_block'"));
        let (_, resource_source) = migration
            .split_once("CREATE FUNCTION public.telemetry_dashboard_resource_overlay_source(")
            .expect("resource overlay source");
        let (resource_source, _) = resource_source
            .split_once("CREATE FUNCTION public.telemetry_dashboard_network_overlay_source(")
            .expect("resource overlay source boundary");
        assert!(resource_source
            .contains("FROM public.telemetry_projected_raw_resource_minutes_source("));
        assert!(resource_source.contains("requested_blocks AS MATERIALIZED"));
        assert!(!resource_source.contains("telemetry_rollups"));

        let (_, network_source) = migration
            .split_once("CREATE FUNCTION public.telemetry_dashboard_network_overlay_source(")
            .expect("network overlay source");
        let (network_source, _) = network_source
            .split_once("CREATE INDEX telemetry_dashboard_block_events_client_age_idx")
            .expect("network overlay source boundary");
        assert!(
            network_source.contains("FROM public.telemetry_projected_raw_network_minutes_source(")
        );
        assert!(network_source.contains("requested_blocks AS MATERIALIZED"));

        let (_, traffic_source) = migration
            .split_once("CREATE FUNCTION public.telemetry_dashboard_traffic_overlay_source(")
            .expect("traffic overlay source");
        let (traffic_source, _) = traffic_source
            .split_once("CREATE FUNCTION public.telemetry_dashboard_resource_overlay_source(")
            .expect("traffic overlay source boundary");
        assert!(traffic_source.contains("requested_heads AS MATERIALIZED"));
        assert!(traffic_source.contains("head.client_id = ANY(p_client_ids)"));
        assert!(traffic_source.contains("JOIN public.traffic_counter_minute_heads minute"));
        assert!(traffic_source.contains("JOIN public.telemetry_samples sample"));
        assert!(traffic_source.contains("sample.accepted_seq > minute.materialized_seq"));

        let telemetry_migration = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../migrations/0003_telemetry_core.sql"
        ));
        let (_, durable_owner) = telemetry_migration
            .split_once("CREATE FUNCTION public.telemetry_network_durable_points_source(")
            .expect("durable network owner source");
        let (durable_owner, _) = durable_owner
            .split_once("CREATE FUNCTION public.telemetry_network_rate_points_source(")
            .expect("durable network owner boundary");
        assert!(durable_owner.contains("LANGUAGE plpgsql"));
        assert!(durable_owner.contains("FROM public.traffic_counter_samples sample"));
        assert!(durable_owner.contains("AND NOT sample.inbound_promoted"));
        assert!(migration.contains("FROM public.traffic_counter_streams stream"));
        assert!(OVERLAY_RESOURCE_SOURCE_SQL.contains(
            "telemetry_dashboard_resource_overlay_source(\n    $1::TEXT[], NULL::INTEGER[], NULL::BIGINT[]"
        ));
        assert!(OVERLAY_NETWORK_SOURCE_SQL.contains(
            "telemetry_dashboard_network_overlay_source(\n    $1::TEXT[], NULL::INTEGER[], NULL::BIGINT[]"
        ));
        assert!(OVERLAY_TRAFFIC_SOURCE_SQL.contains(
            "telemetry_dashboard_traffic_overlay_source(\n    $1::TEXT[], NULL::INTEGER[], NULL::BIGINT[]"
        ));
        let source = include_str!("dashboard_telemetry_resident.rs");
        for domain in ["resource", "network", "traffic"] {
            let mirror = format!("telemetry_dashboard_{domain}_active_rows");
            assert!(!source.contains(&mirror));
        }
    }

    fn resource_block(start: i64, slot: usize, samples: i64, latest: i64) -> ResourceBlock {
        let mut block = ResourceBlock::empty(start);
        block.slots[slot] = ResourceSummary {
            sample_count: samples,
            cpu_sum: samples as f64,
            cpu_max: 1.0,
            memory_total: 100,
            memory_sum: samples as f64 * 0.5,
            memory_max: 0.5,
            disk_count: samples,
            disk_total: 100,
            disk_sum: samples as f64 * 0.25,
            disk_max: 0.25,
            latest,
        };
        block
    }

    #[test]
    fn traffic_projection_preserves_selected_streams_and_directional_gaps() {
        let resident = DashboardTelemetryResident::empty_for_tests();
        let mut block = TrafficBlock::empty(0);
        block.slots[0] = TrafficState::from_columns(Some(1), Some(0), Some(100), None).unwrap();
        block.slots[1] = TrafficState::from_columns(Some(1), Some(1), Some(40), Some(20)).unwrap();
        let host_interfaces = (0..8)
            .map(|index| format!("eth{index}"))
            .collect::<Vec<_>>();
        let mut source_kinds = vec!["host".to_string(); host_interfaces.len()];
        source_kinds.push("tunnel".to_string());
        let mut interfaces = host_interfaces;
        interfaces.push("wg0".to_string());
        mutate_installed_fleet(&resident, |fleet| {
            fleet.traffics.insert(
                "client-a".to_string(),
                Arc::new(RwLock::new(TrafficOwner {
                    generation: 1,
                    revision: 1,
                    overlay_blocks: BTreeSet::new(),
                    source_kinds: source_kinds.into(),
                    interfaces: interfaces.into(),
                    index: TrafficIndex::from_blocks(BTreeMap::from([((60, 0), block)])),
                })),
            );
        });

        let projection = resident
            .traffic_projection(
                240,
                Some(0),
                Some(120),
                60,
                &["client-a".to_string()],
                &["client-a".to_string()],
                8,
            )
            .unwrap();

        assert_eq!(projection.client_points.len(), 2);
        assert_eq!(projection.client_points[0].rx_bytes, Some(100));
        assert_eq!(projection.client_points[0].tx_bytes, None);
        assert_eq!(projection.fleet_points[0].tx_bytes, None);
        assert_eq!(
            projection.client_ids_in_rank_order,
            vec!["client-a".to_string()]
        );
        let selected = &projection.interfaces_by_client["client-a"];
        assert_eq!(selected.len(), 9);
        assert_eq!(selected.first().map(String::as_str), Some("eth0"));
        assert_eq!(selected.last().map(String::as_str), Some("tunnel:wg0"));
    }

    fn network_block(start: i64, samples: i64, latest: i64, rx: i64) -> NetworkBlock {
        let mut block = NetworkBlock::empty(start, 1);
        block.slots[0] = NetworkState {
            count: samples,
            latest,
            rx,
            tx: rx * 2,
            rx_epoch: 1,
            tx_epoch: 1,
        };
        block
    }

    #[test]
    fn resource_ring_updates_only_paths_and_stays_bounded_across_wrap_gap_and_delete() {
        let mut sparse = BTreeMap::new();
        sparse.insert(0, resource_block(0, 0, 1, 1));
        sparse.insert(960, resource_block(960, 0, 2, 2));
        let mut tier = ResourceTier::from_sparse(60, sparse).unwrap();

        tier.set_block(0, None);
        assert_eq!((tier.head, tier.len, tier.first_start), (1, 1, 960));
        tier.set_block(1920, Some(resource_block(1920, 0, 3, 3)));
        assert_eq!((tier.head, tier.len, tier.tree_base), (1, 2, 2));
        assert_eq!(tier.range(0, 32).sample_count, 5);

        let touched = tier.set_block(1920, Some(resource_block(1920, 0, 7, 4)));
        assert_eq!(touched, 1 + tier.tree_base.ilog2() as usize);
        assert_eq!(tier.range(0, 32).sample_count, 9);

        tier.set_block(3840, Some(resource_block(3840, 0, 11, 5)));
        assert_eq!(tier.len, 4);
        assert!(tier.block(2).is_none(), "calendar gap must remain absent");
        assert_eq!(tier.first_last_present(), Some((960, 3840)));

        tier.set_block(960, None);
        tier.set_block(3840, None);
        assert_eq!((tier.len, tier.first_start), (1, 1920));
        assert_eq!(tier.first_last_present(), Some((1920, 1920)));
        assert!(tier.blocks.iter().filter(|block| block.is_some()).count() <= tier.len);
    }

    #[test]
    fn network_group_delete_and_insert_preserves_chronological_terminal_ties() {
        let mut blocks = BTreeMap::new();
        blocks.insert((60, 0), network_block(0, 1, 10, 10));
        blocks.insert((60, 960), network_block(960, 2, 20, 20));
        let mut index = NetworkIndex::from_blocks(vec!["eth0".to_string()].into(), blocks).unwrap();

        index.apply_blocks(vec![
            (60, 0, None),
            (60, 1920, Some(network_block(1920, 3, 20, 99))),
        ]);
        let tier = index.tiers.get(&60).unwrap();
        assert_eq!((tier.head, tier.len, tier.tree_base), (1, 2, 2));
        let mut state = [NetworkState::default()];
        tier.range_into(0, tier.slot_len(), &mut state);
        assert_eq!(state[0].count, 5);
        assert_eq!(state[0].latest, 20);
        assert_eq!(
            state[0].rx, 99,
            "later source must win an exact timestamp tie"
        );
    }

    #[test]
    fn closed_null_slots_decode_as_absence_and_never_as_zero_evidence() {
        let resource_head = ResourceHead {
            generation: 1,
            revision: 1,
            change: "generation".to_string(),
            blocks: Vec::<BlockKey>::new().into(),
            first_unix: None,
            through_unix: None,
        };
        let resource = ResourceBlockRow {
            tier: 60,
            start: 0,
            revision: 1,
            sample_counts: vec![0; BLOCK_SLOTS],
            cpu_sums: vec![None; BLOCK_SLOTS],
            cpu_maxes: vec![None; BLOCK_SLOTS],
            memory_totals: vec![None; BLOCK_SLOTS],
            memory_sums: vec![None; BLOCK_SLOTS],
            memory_maxes: vec![None; BLOCK_SLOTS],
            disk_counts: vec![0; BLOCK_SLOTS],
            disk_totals: vec![None; BLOCK_SLOTS],
            disk_sums: vec![None; BLOCK_SLOTS],
            disk_maxes: vec![None; BLOCK_SLOTS],
            latest: vec![None; BLOCK_SLOTS],
        };
        assert_eq!(
            resource
                .into_block(&resource_head)
                .unwrap()
                .1
                .summary()
                .sample_count,
            0
        );

        let network_head = NetworkHead {
            generation: 1,
            revision: 1,
            change: "generation".to_string(),
            blocks: Vec::<BlockKey>::new().into(),
            interfaces: vec!["eth0".to_string()].into(),
            first_unix: None,
            through_unix: None,
        };
        let absent = NetworkBlockRow {
            tier: 60,
            start: 0,
            revision: 1,
            counts: vec![0; BLOCK_SLOTS],
            latest: vec![None; BLOCK_SLOTS],
            rx: vec![None; BLOCK_SLOTS],
            tx: vec![None; BLOCK_SLOTS],
            rx_epoch: vec![None; BLOCK_SLOTS],
            tx_epoch: vec![None; BLOCK_SLOTS],
        };
        assert!(!absent.into_block(&network_head).unwrap().1.slots[0].present());
        let invalid = NetworkBlockRow {
            tier: 60,
            start: 0,
            revision: 1,
            counts: vec![0; BLOCK_SLOTS],
            latest: vec![Some(0); BLOCK_SLOTS],
            rx: vec![None; BLOCK_SLOTS],
            tx: vec![None; BLOCK_SLOTS],
            rx_epoch: vec![None; BLOCK_SLOTS],
            tx_epoch: vec![None; BLOCK_SLOTS],
        };
        assert!(invalid.into_block(&network_head).is_err());
    }

    fn dashboard_block_notice(client_id: &str, domain: &str, revision: i64) -> DashboardNotice {
        DashboardNotice {
            owner: "dashboard".to_string(),
            client_id: client_id.to_string(),
            domain: domain.to_string(),
            change: "block".to_string(),
            generation: Some(1),
            previous_revision: Some(revision - 1),
            revision: Some(revision),
            source_bucket_secs: Some(vec![60]),
            block_start_unix: Some(vec![(revision - 2).max(0) * 960]),
            complete: Some(true),
        }
    }

    fn raw_projection_notice(
        client_id: impl Into<String>,
        generation: i64,
        projected_seq: i64,
    ) -> RawProjectionNotice {
        RawProjectionNotice {
            client_id: client_id.into(),
            generation,
            projected_seq,
        }
    }

    #[test]
    fn live_overlay_reconciliation_queries_only_installed_nonempty_domains() {
        let empty = HashMap::<String, ()>::new();
        assert!(overlay_target_client_ids(&empty).is_empty());
        let targets = HashMap::from([("client-z".to_string(), ()), ("client-a".to_string(), ())]);
        assert_eq!(
            overlay_target_client_ids(&targets),
            ["client-a".to_string(), "client-z".to_string()]
        );

        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, reconcile) = source
            .split_once("async fn reconcile_live_overlays")
            .expect("live-overlay reconciliation");
        let (reconcile, _) = reconcile
            .split_once("struct DashboardNotice")
            .expect("live-overlay reconciliation boundary");
        for (domain, query) in [
            ("resource", "OVERLAY_RESOURCE_SOURCE_SQL"),
            ("network", "OVERLAY_NETWORK_SOURCE_SQL"),
            ("traffic", "OVERLAY_TRAFFIC_SOURCE_SQL"),
        ] {
            let target_ids = format!("{domain}_target_client_ids");
            let guard = reconcile
                .find(&format!("if !{target_ids}.is_empty()"))
                .unwrap_or_else(|| panic!("missing empty {domain} target guard"));
            let query = reconcile[guard..]
                .find(query)
                .unwrap_or_else(|| panic!("missing guarded {domain} source query"))
                + guard;
            let bind = reconcile[query..]
                .find(&format!(".bind(&{target_ids})"))
                .unwrap_or_else(|| panic!("missing scoped {domain} target binding"))
                + query;
            assert!(guard < query && query < bind);
        }
        assert!(!reconcile.contains("client_ids.to_vec()"));
        assert!(!reconcile.contains(".bind(&target_client_ids)"));

        let network_fence = reconcile
            .find("if !installed.index.interfaces.is_empty()")
            .expect("published network selection fence");
        let network_target = reconcile[network_fence..]
            .find("network_targets.insert(")
            .expect("network target after published selection fence")
            + network_fence;
        assert!(network_fence < network_target);

        let traffic_fence = reconcile
            .find("if !installed.source_kinds.is_empty()")
            .expect("published traffic selection fence");
        let traffic_target = reconcile[traffic_fence..]
            .find("traffic_targets.insert(")
            .expect("selected traffic target")
            + traffic_fence;
        assert!(traffic_fence < traffic_target);
    }

    #[tokio::test(start_paused = true)]
    async fn live_overlay_boundary_collects_all_clients_in_one_fixed_window() {
        assert_eq!(FLEET_TELEMETRY_INVALIDATION_WINDOW, Duration::from_secs(2));
        let mailbox = ResidentMailbox::default();
        for frame in 1..=6 {
            for client in (0..120).rev() {
                mailbox.enqueue_overlay(raw_projection_notice(
                    format!("client-{client:03}"),
                    7,
                    frame,
                ));
            }
        }
        assert!(mailbox.claim_ready().is_none());

        time::advance(Duration::from_millis(1_999)).await;
        // A late duplicate updates its client fence but must neither slide the
        // collection boundary nor create another batch.
        mailbox.enqueue_overlay(raw_projection_notice("client-000", 7, 7));
        assert!(mailbox.claim_ready().is_none());
        time::advance(Duration::from_millis(1)).await;

        let Some(ResidentWork::Overlay(batch)) = mailbox.claim_ready() else {
            panic!("one live-overlay batch expected at the shared boundary");
        };
        assert_eq!(batch.fence_epoch, 0);
        assert_eq!(batch.client_ids.len(), 120);
        assert_eq!(
            batch.client_ids.first().map(String::as_str),
            Some("client-000")
        );
        assert_eq!(
            batch.client_ids.last().map(String::as_str),
            Some("client-119")
        );
        assert!(mailbox.claim_ready().is_none());

        mailbox.enqueue_overlay(raw_projection_notice("client-000", 7, 8));
        assert!(mailbox.claim_ready().is_none());
        time::advance(FLEET_TELEMETRY_INVALIDATION_WINDOW).await;
        assert!(matches!(
            mailbox.claim_ready(),
            Some(ResidentWork::Overlay(LiveOverlayBatch { client_ids, .. }))
                if client_ids == ["client-000"]
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn live_overlay_boundary_fence_discards_only_pre_fence_raw_hints() {
        let mailbox = ResidentMailbox::default();
        mailbox.enqueue_overlay(raw_projection_notice("stale", 1, 1));
        time::advance(Duration::from_secs(1)).await;

        mailbox.enqueue_fleet_fence();
        mailbox.enqueue_overlay(raw_projection_notice("after-fence", 2, 1));
        assert_eq!(mailbox.claim_ready(), Some(ResidentWork::FleetFence));
        time::advance(Duration::from_millis(1_999)).await;
        assert!(mailbox.claim_ready().is_none());
        time::advance(Duration::from_millis(1)).await;

        assert_eq!(
            mailbox.claim_ready(),
            Some(ResidentWork::Overlay(LiveOverlayBatch {
                client_ids: vec!["after-fence".to_string()],
                fence_epoch: 1,
            }))
        );
        assert!(mailbox.claim_ready().is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn live_overlay_boundary_retry_has_no_second_collection_delay() {
        let mailbox = ResidentMailbox::default();
        mailbox.enqueue_overlay(raw_projection_notice("a", 1, 1));
        time::advance(FLEET_TELEMETRY_INVALIDATION_WINDOW).await;
        let work = mailbox.claim_ready().expect("first overlay batch");
        let ResidentWork::Overlay(batch) = work.clone() else {
            panic!("overlay batch expected");
        };
        mailbox.requeue(work);
        assert_eq!(
            mailbox.claim_ready(),
            Some(ResidentWork::Overlay(batch.clone()))
        );

        mailbox.enqueue_fleet_fence();
        mailbox.requeue(ResidentWork::Overlay(batch));
        assert_eq!(mailbox.claim_ready(), Some(ResidentWork::FleetFence));
        assert!(mailbox.claim_ready().is_none());
    }

    #[test]
    fn live_overlay_boundary_notifies_only_after_setwise_installation() {
        let mailbox = ResidentMailbox::default();
        let (events, invalidations) = WsEventBus::new(16);
        collect_notification(
            &mailbox,
            &events,
            r#"{"client_id":"a","generation":1,"projected_seq":2,"retention_minute_ready_at_unix":180,"sample_prune_ready_at_unix":null}"#,
        )
        .expect("the complete raw producer notice is an overlay hint");
        assert!(!invalidations.take_fleet_telemetry());
        assert!(
            mailbox.claim_ready().is_none(),
            "a raw producer notice must not enqueue an immediate fleet fence"
        );

        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, reconcile) = source
            .split_once("ResidentWork::Overlay(batch) => {")
            .expect("live-overlay reconciliation branch");
        let (reconcile, _) = reconcile
            .split_once("ResidentWork::FleetFence => {")
            .expect("live-overlay reconciliation boundary");
        assert!(
            reconcile
                .find("reconcile_live_overlays(")
                .expect("setwise current-state read")
                < reconcile
                    .find("events.notify_fleet_telemetry()")
                    .expect("post-install browser boundary")
        );

        let (_, setwise) = source
            .split_once("async fn reconcile_live_overlays")
            .expect("setwise live-overlay function");
        let (setwise, _) = setwise
            .split_once("struct DashboardNotice")
            .expect("setwise live-overlay boundary");
        for domain in ["resource", "network", "traffic"] {
            assert!(setwise.contains(&format!(".bind(&{domain}_target_client_ids)")));
        }
        assert!(!setwise.contains("LIMIT "));
        assert!(!setwise.contains("429"));
    }

    #[test]
    fn resident_mailbox_is_fifo_across_owners_and_unions_contiguous_owner_changes() {
        let mailbox = ResidentMailbox::default();
        mailbox.enqueue(dashboard_block_notice("a", "resource", 2));
        mailbox.enqueue(dashboard_block_notice("b", "network", 3));
        mailbox.enqueue(dashboard_block_notice("a", "resource", 3));
        mailbox.enqueue(dashboard_block_notice("a", "resource", 2));

        let ResidentWork::Notices(notices) = mailbox.claim_ready().expect("ready cohort") else {
            panic!("owner notice cohort expected");
        };
        assert_eq!(notices.len(), 2);
        let first = &notices[0];
        assert_eq!(
            (
                first.client_id.as_str(),
                first.domain.as_str(),
                first.previous_revision,
                first.revision,
                first.source_bucket_secs.clone(),
                first.block_start_unix.clone(),
            ),
            (
                "a",
                "resource",
                Some(1),
                Some(3),
                Some(vec![60, 60]),
                Some(vec![0, 960]),
            )
        );
        let second = &notices[1];
        assert_eq!(
            (
                second.client_id.as_str(),
                second.domain.as_str(),
                second.revision
            ),
            ("b", "network", Some(3))
        );
        assert!(mailbox.claim_ready().is_none());
    }

    #[test]
    fn resident_mailbox_exposes_one_revision_only_after_its_final_fragment() {
        let mailbox = ResidentMailbox::default();
        let mut first = dashboard_block_notice("a", "resource", 2);
        first.complete = Some(false);
        mailbox.enqueue(first);
        assert!(mailbox.claim_ready().is_none());

        let mut final_fragment = dashboard_block_notice("a", "resource", 2);
        final_fragment.block_start_unix = Some(vec![960]);
        mailbox.enqueue(final_fragment);
        let ResidentWork::Notices(complete) = mailbox.claim_ready().expect("complete owner") else {
            panic!("owner notice cohort expected");
        };
        assert_eq!(complete.len(), 1);
        let complete = &complete[0];
        assert_eq!(complete.previous_revision, Some(1));
        assert_eq!(complete.revision, Some(2));
        assert_eq!(complete.source_bucket_secs, Some(vec![60, 60]));
        assert_eq!(complete.block_start_unix, Some(vec![0, 960]));
        assert_eq!(complete.complete, Some(true));
        assert!(mailbox.claim_ready().is_none());
    }

    #[test]
    fn resident_mailbox_defers_a_head_ahead_notice_until_its_successor_arrives() {
        let mailbox = ResidentMailbox::default();
        mailbox.defer_until_successor(dashboard_block_notice("a", "resource", 2));
        assert!(mailbox.claim_ready().is_none());

        mailbox.enqueue(dashboard_block_notice("a", "resource", 3));
        let ResidentWork::Notices(complete) = mailbox.claim_ready().expect("successor owner")
        else {
            panic!("owner notice cohort expected");
        };
        assert_eq!(complete.len(), 1);
        let complete = &complete[0];
        assert_eq!(complete.previous_revision, Some(1));
        assert_eq!(complete.revision, Some(3));
        assert_eq!(complete.block_start_unix, Some(vec![0, 960]));
        assert!(mailbox.claim_ready().is_none());
    }

    #[test]
    fn only_a_contiguous_same_generation_block_head_can_await_its_notice() {
        assert!(block_notice_is_waiting_for_successor(
            7, 10, 7, 12, "block", 7, 10, 11
        ));
        assert!(!block_notice_is_waiting_for_successor(
            7, 9, 7, 12, "block", 7, 10, 11
        ));
        assert!(!block_notice_is_waiting_for_successor(
            7,
            10,
            8,
            12,
            "generation",
            7,
            10,
            11
        ));
        assert!(!block_notice_is_waiting_for_successor(
            7, 10, 7, 11, "block", 7, 10, 11
        ));
    }

    #[test]
    fn dashboard_block_notifications_are_ordered_bounded_transaction_fragments() {
        let migration = include_str!("../../../../migrations/0006_telemetry_dashboard.sql");
        let (_, publish) = migration
            .split_once("CREATE FUNCTION public.publish_telemetry_dashboard_projection(")
            .expect("dashboard publication function");
        let (publish, _) = publish
            .split_once("-- Ping owns no range tree here.")
            .expect("dashboard publication boundary");
        let fragment_loop = publish
            .find("WITH ORDINALITY coordinate(tier, block_start, ordinality)")
            .expect("coordinate-fragment loop");
        let fragment_order = publish[fragment_loop..]
            .find("ORDER BY coordinate.ordinality")
            .expect("fragment send order")
            + fragment_loop;
        let one_coordinate = publish[fragment_order..]
            .find("ARRAY[block_coordinate.tier]::INTEGER[]")
            .expect("one-coordinate payload")
            + fragment_order;
        let complete_fence = publish[one_coordinate..]
            .find("'complete', block_coordinate.ordinality = cardinality(")
            .expect("final-fragment fence")
            + one_coordinate;
        let notify = publish[complete_fence..]
            .find("PERFORM pg_notify(")
            .expect("transactional notification")
            + complete_fence;
        assert!(fragment_loop < fragment_order);
        assert!(fragment_order < one_coordinate);
        assert!(one_coordinate < complete_fence);
        assert!(complete_fence < notify);
    }

    #[test]
    fn resident_mailbox_coalesces_one_internal_fleet_fence() {
        let mailbox = ResidentMailbox::default();
        mailbox.enqueue_fleet_fence();
        mailbox.enqueue_fleet_fence();
        mailbox.enqueue(dashboard_block_notice("a", "resource", 2));

        assert_eq!(mailbox.claim_ready(), Some(ResidentWork::FleetFence));
        assert!(matches!(
            mailbox.claim_ready(),
            Some(ResidentWork::Notices(notices))
                if notices.len() == 1
                    && notices[0].client_id == "a"
                    && notices[0].domain == "resource"
        ));
        assert!(mailbox.claim_ready().is_none());
    }

    #[test]
    fn installed_revision_discards_only_proven_older_or_identical_notices() {
        let resident = DashboardTelemetryResident::empty_for_tests();
        mutate_installed_fleet(&resident, |fleet| {
            fleet.resources.insert(
                "a".to_string(),
                Arc::new(RwLock::new(ResourceOwner {
                    generation: 1,
                    revision: 5,
                    overlay_blocks: BTreeSet::new(),
                    index: ResourceIndex::default(),
                })),
            );
        });
        assert!(dashboard_notice_is_installed(
            &resident,
            &dashboard_block_notice("a", "resource", 4)
        ));
        assert!(dashboard_notice_is_installed(
            &resident,
            &dashboard_block_notice("a", "resource", 5)
        ));
        assert!(!dashboard_notice_is_installed(
            &resident,
            &dashboard_block_notice("a", "resource", 6)
        ));

        let mut wrong_generation = dashboard_block_notice("a", "resource", 5);
        wrong_generation.generation = Some(2);
        assert!(!dashboard_notice_is_installed(&resident, &wrong_generation));
    }

    #[test]
    fn listener_collection_has_no_database_reconciliation_path() {
        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, collector) = source
            .split_once("fn collect_notification")
            .expect("collector function");
        let (collector, _) = collector
            .split_once("async fn reconnect_until_ready")
            .expect("collector boundary");
        assert!(collector.contains("mailbox.enqueue(notice)"));
        assert!(!collector.contains(".await"));
        assert!(!collector.contains("load_optional_client_heads"));
        assert!(!collector.contains("load_fenced_"));

        let (_, reconnect) = source
            .split_once("async fn reconnect_until_ready")
            .expect("listener reconnect function");
        let (reconnect, _) = reconnect
            .split_once("async fn reconcile_collected_notice")
            .expect("listener reconnect boundary");
        assert!(reconnect.contains("connect_listener(pool).await"));
        assert!(reconnect.contains("mailbox.enqueue_fleet_fence()"));
        assert!(!reconnect.contains("load_heads"));
        assert!(!reconnect.contains("load_fenced_"));

        let (_, listener) = source
            .split_once("async fn run_resident_listener")
            .expect("resident listener task");
        let (listener, _) = listener
            .split_once("#[cfg(test)]")
            .expect("resident listener boundary");
        assert_eq!(listener.matches("reconnect_until_ready(").count(), 1);
        assert!(listener.contains("mailbox.enqueue_fleet_fence()"));
        assert!(
            listener.find("drop(listener);").expect("old listener drop")
                < listener
                    .find("reconnect_until_ready(")
                    .expect("replacement listener acquisition")
        );
        assert!(!listener.contains("load_heads"));
        assert!(!listener.contains("load_fenced_"));

        let (_, initialization) = source
            .split_once("pub(crate) async fn initialize")
            .expect("resident initialization");
        let (initialization, _) = initialization
            .split_once("fn shutdown_requested")
            .expect("resident initialization boundary");
        assert!(initialization.contains("reconciliation_lanes = 1"));
        assert!(initialization.contains("reconciler_pool = PgPoolOptions::new()"));
        assert!(!initialization.contains("available_parallelism"));
    }

    #[test]
    fn startup_seed_is_one_snapshot_with_four_committed_relation_reads() {
        for (sql, relation, head, generation, revision) in [
            (
                SEED_RESOURCE_BLOCKS_SQL,
                "telemetry_dashboard_resource_blocks",
                "telemetry_dashboard_resource_projection_heads",
                "block.generation IS NOT DISTINCT FROM head.resource_generation",
                "block.published_revision <= head.resource_revision",
            ),
            (
                SEED_NETWORK_BLOCKS_SQL,
                "telemetry_dashboard_network_blocks",
                "telemetry_dashboard_network_projection_heads",
                "block.generation IS NOT DISTINCT FROM head.network_generation",
                "block.published_revision <= head.network_revision",
            ),
            (
                SEED_TRAFFIC_BLOCKS_SQL,
                "telemetry_dashboard_traffic_blocks",
                "telemetry_dashboard_traffic_projection_heads",
                "block.generation IS NOT DISTINCT FROM head.traffic_generation",
                "block.published_revision <= head.traffic_revision",
            ),
        ] {
            assert_eq!(sql.matches(relation).count(), 1);
            assert_eq!(sql.matches(head).count(), 1);
            assert!(sql.contains("seed_client_id"));
            assert!(sql.contains(generation));
            assert!(sql.contains(revision));
            assert!(!sql.contains("ORDER BY"));
            assert!(!sql.contains("LIMIT"));
            assert!(!sql.contains("$1"));
        }
        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, seed) = source
            .split_once("async fn seed_fleet")
            .expect("fleet seed");
        let (seed, _) = seed
            .split_once("async fn load_resource_blocks")
            .expect("fleet seed boundary");
        assert!(seed.contains("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY"));
        assert_eq!(seed.matches(".fetch_all(&mut *transaction)").count(), 4);
        assert_eq!(seed.matches("transaction.commit().await?").count(), 1);
        assert!(!seed.contains("OverlayRow"));
        assert!(!seed.contains("load_fenced_"));
        assert!(!seed.contains("for client_id in heads.keys()"));

        let (_, initialization) = source
            .split_once("pub(crate) async fn initialize")
            .expect("resident initialization");
        let (initialization, _) = initialization
            .split_once("fn shutdown_requested")
            .expect("resident initialization boundary");
        assert!(
            initialization.find(".listen(TELEMETRY_PROJECTION_CHANNEL)")
                < initialization.find("seed_fleet(&reconciler_pool)")
        );

        let main = include_str!("../main.rs");
        let resident_at = main
            .find("DashboardTelemetryResident::initialize")
            .expect("resident initialization in main");
        let publisher_at = main
            .find("spawn_dashboard_projection_maintenance_task(&repo)")
            .expect("dashboard publisher startup in main");
        let bind_at = main
            .find("tokio::net::TcpListener::bind(args.bind)")
            .expect("HTTP listener bind in main");
        assert!(resident_at < publisher_at && publisher_at < bind_at);
    }

    #[test]
    fn only_full_owner_reloads_restore_live_suffix_through_the_shared_mailbox() {
        assert!(!DashboardNoticeApplication::AlreadyCurrent.requires_live_overlay());
        assert!(!DashboardNoticeApplication::ExactBlock.requires_live_overlay());
        assert!(DashboardNoticeApplication::OwnerReplacement.requires_live_overlay());

        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, owners) = source
            .split_once("async fn load_resource_owner")
            .expect("full resource owner loader");
        let (owners, _) = owners
            .split_once("async fn load_fenced_client")
            .expect("full owner loader boundary");
        assert_eq!(owners.matches("overlay_blocks: BTreeSet::new()").count(), 3);
        assert!(!owners.contains("RESOURCE_OVERLAY_SQL"));
        assert!(!owners.contains("NETWORK_OVERLAY_SQL"));
        assert!(!owners.contains("TRAFFIC_OVERLAY_SQL"));
        assert!(!owners.contains("Option::<Vec<i32>>::None"));

        let (_, initialization) = source
            .split_once("pub(crate) async fn initialize")
            .expect("resident initialization");
        let (initialization, _) = initialization
            .split_once("fn shutdown_requested")
            .expect("resident initialization boundary");
        assert!(initialization.contains("for client_id in &seeded_client_ids"));
        assert!(initialization.contains("mailbox.enqueue_live_overlay(client_id);"));

        let (_, reconciler) = source
            .split_once("async fn run_resident_reconciler")
            .expect("resident reconciler");
        let (reconciler, _) = reconciler
            .split_once("async fn run_resident_listener")
            .expect("resident reconciler boundary");
        assert!(reconciler.contains("if application.requires_live_overlay()"));
        assert!(reconciler.contains("mailbox.enqueue_live_overlay(&notice.client_id);"));

        let (_, apply) = source
            .split_once("async fn apply_dashboard_notice")
            .expect("dashboard notice application");
        let (apply, _) = apply
            .split_once("pub(crate) struct DashboardTelemetryResidentTask")
            .expect("dashboard notice application boundary");
        assert_eq!(
            apply
                .matches("return Ok(DashboardNoticeApplication::ExactBlock);")
                .count(),
            3
        );
        assert!(apply.contains(
            "reconcile_notice_owner(listener, resident, &notice.client_id, \"client\").await?;"
        ));
        assert!(apply.contains("DashboardNoticeApplication::OwnerReplacement"));

        let (_, collected) = source
            .split_once("async fn reconcile_collected_notice")
            .expect("collected notice reconciliation");
        let (collected, _) = collected
            .split_once("async fn run_resident_reconciler")
            .expect("collected notice boundary");
        assert!(
            collected.contains("reconcile_notice_owner(connection, resident, &client_id, &domain)")
        );
        assert!(collected.contains("DashboardNoticeApplication::OwnerReplacement"));
    }

    #[test]
    fn client_lifecycle_claims_ignore_the_hint_verb_and_reconcile_current_database_state() {
        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, apply) = source
            .split_once("async fn apply_dashboard_notice")
            .expect("dashboard notice application");
        let (apply, _) = apply
            .split_once("pub(crate) struct DashboardTelemetryResidentTask")
            .expect("dashboard notice application boundary");
        let normalized = apply.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(normalized.contains(
            "if notice.domain == \"client\" { // Lifecycle notices deliberately carry no revision."
        ));
        assert!(normalized.contains(
            "reconcile_notice_owner(listener, resident, &notice.client_id, \"client\").await?;"
        ));
        assert!(!normalized.contains("match notice.change.as_str()"));
    }

    #[test]
    fn exceptional_fleet_fence_authoritatively_reloads_every_current_client() {
        let source = include_str!("dashboard_telemetry_resident.rs");
        let (_, fence) = source
            .split_once("async fn reconcile_fleet_fence")
            .expect("fleet fence reconciliation");
        let (fence, _) = fence
            .split_once("async fn connect_listener")
            .expect("fleet fence reconciliation boundary");
        assert!(fence.contains("for client_id in heads.keys()"));
        assert!(fence.contains("change: \"initialize\".to_string()"));
        assert!(fence.contains("change: \"remove\".to_string()"));
        assert_eq!(fence.matches("domain: \"client\".to_string()").count(), 2);
        assert!(!fence.contains("domain: \"resource\".to_string()"));
        assert!(!fence.contains("domain: \"network\".to_string()"));
        assert!(!fence.contains("domain: \"traffic\".to_string()"));
    }

    #[test]
    fn notice_contract_is_typed_and_revision_gaps_require_a_fenced_reload() {
        assert!(matches!(
            ProjectionNotice::parse(
                r#"{"owner":"dashboard","client_id":"a","domain":"resource","change":"block","generation":1,"previous_revision":1,"revision":2,"source_bucket_secs":[60],"block_start_unix":[0],"complete":true}"#,
            ),
            Some(ProjectionNotice::Dashboard(_))
        ));
        assert!(matches!(
            ProjectionNotice::parse(
                r#"{"owner":"dashboard","client_id":"a","domain":"resource","change":"block","generation":1,"previous_revision":1,"revision":2,"source_bucket_secs":[60],"block_start_unix":[0],"complete":false}"#,
            ),
            Some(ProjectionNotice::Dashboard(_))
        ));
        assert!(ProjectionNotice::parse(
            r#"{"owner":"dashboard","client_id":"a","domain":"resource","change":"block","generation":1,"revision":2}"#,
        ).is_none());
        assert!(matches!(
            ProjectionNotice::parse(
                r#"{"owner":"dashboard","client_id":"a","domain":"traffic","change":"block","generation":1,"previous_revision":1,"revision":2,"source_bucket_secs":[3600],"block_start_unix":[0],"complete":true}"#,
            ),
            Some(ProjectionNotice::Dashboard(_))
        ));
        assert!(ProjectionNotice::parse(
            r#"{"owner":"dashboard","client_id":"a","domain":"traffic","change":"block","generation":1,"previous_revision":1,"revision":2,"source_bucket_secs":[300],"block_start_unix":[0],"complete":true}"#,
        ).is_none());
        assert!(ProjectionNotice::parse(
            r#"{"owner":"dashboard","client_id":"a","domain":"resource","change":"block","generation":1,"previous_revision":1,"revision":2,"source_bucket_secs":[60],"block_start_unix":[0],"complete":true,"blocks":[]}"#,
        ).is_none());
        assert!(ProjectionNotice::parse(
            r#"{"owner":"dashboard","client_id":"a","domain":"resource","change":"block","generation":1,"previous_revision":0,"revision":2,"source_bucket_secs":[60],"block_start_unix":[0],"complete":true}"#,
        ).is_none());

        let head = ResourceHead {
            generation: 1,
            revision: 3,
            change: "block".to_string(),
            blocks: vec![BlockKey {
                source_bucket_secs: 60,
                block_start_unix: 0,
            }]
            .into(),
            first_unix: Some(0),
            through_unix: Some(1),
        };
        let mut owner = ResourceOwner {
            generation: 1,
            revision: 1,
            overlay_blocks: BTreeSet::new(),
            index: ResourceIndex::default(),
        };
        assert!(!resource_block_change_is_contiguous(&owner, &head, 1, 1, 2));
        owner.revision = 2;
        assert!(resource_block_change_is_contiguous(&owner, &head, 1, 2, 3));
        assert!(!resource_block_change_is_contiguous(&owner, &head, 1, 2, 4));
    }

    #[test]
    fn exact_coordinate_union_is_loaded_setwise_before_independent_owner_application() {
        let source = include_str!("dashboard_telemetry_resident.rs")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let (_, source) = source
            .split_once("async fn apply_dashboard_notice")
            .expect("dashboard notice application");
        for domain in ["resource", "network", "traffic"] {
            let load = format!("let (changes, overlay_blocks) = load_{domain}_blocks(");
            let apply = "owner.index.apply_blocks(changes);".to_string();
            let load_at = source.find(&load).expect("bulk descriptor load");
            let fence_at = source[load_at..]
                .find("let after = load_optional_client_heads(")
                .expect("post-load head fence")
                + load_at;
            let write_at = source[load_at..]
                .find(".write()")
                .expect("exclusive owner write")
                + load_at;
            let apply_at = source[write_at..].find(&apply).expect("atomic group apply") + write_at;
            let overlay_at = source[apply_at..]
                .find("owner.overlay_blocks.extend(overlay_blocks);")
                .expect("overlay membership application")
                + apply_at;
            let revision_at = source[overlay_at..]
                .find("owner.revision =")
                .expect("revision publication after group")
                + overlay_at;
            assert!(
                load_at < fence_at
                    && fence_at < write_at
                    && write_at < apply_at
                    && apply_at < overlay_at
                    && overlay_at < revision_at
            );
        }

        let (_, cohort) = source
            .split_once("async fn reconcile_collected_notices")
            .expect("setwise dashboard notice cohort");
        let (cohort, _) = cohort
            .split_once("pub(crate) struct DashboardTelemetryResidentTask")
            .expect("setwise dashboard notice boundary");
        let first_head = cohort
            .find("let heads = load_selected_heads(")
            .expect("one setwise pre-read head fence");
        let after_head = cohort
            .rfind("let after = load_selected_heads(")
            .expect("one setwise post-read head fence");
        for domain in ["resource", "network", "traffic"] {
            let load = cohort
                .find(&format!("load_{domain}_notice_changes("))
                .expect("setwise exact-coordinate load");
            let owner_loop = cohort
                .find(&format!("for (client_id, target) in {domain}_targets"))
                .expect("independent owner application");
            let owner = &cohort[owner_loop..];
            let write = owner.find(".write()").expect("exclusive owner write") + owner_loop;
            let apply = owner
                .find("installed.index.apply_blocks(change.blocks);")
                .expect("exact owner block application")
                + owner_loop;
            let overlay = owner
                .find("installed.overlay_blocks.append(&mut change.overlay_blocks);")
                .expect("exact owner overlay application")
                + owner_loop;
            let revision = owner
                .find("installed.revision = target.head.revision;")
                .expect("exact owner revision publication")
                + owner_loop;
            assert!(
                first_head < load
                    && load < after_head
                    && after_head < write
                    && write < apply
                    && apply < overlay
                    && overlay < revision
            );
        }
        assert!(!cohort.contains("LIMIT "));
        assert!(!cohort.contains("time::sleep"));
        for sql in [
            OVERLAY_RESOURCE_BLOCKS_SQL,
            OVERLAY_NETWORK_BLOCKS_SQL,
            OVERLAY_TRAFFIC_BLOCKS_SQL,
            NOTICE_RESOURCE_OVERLAY_SQL,
            NOTICE_NETWORK_OVERLAY_SQL,
            NOTICE_TRAFFIC_OVERLAY_SQL,
        ] {
            assert!(sql.contains("unnest(") || sql.contains("UNNEST("));
            assert!(!sql.contains("LIMIT "));
        }
    }
}
