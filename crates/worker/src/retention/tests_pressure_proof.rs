use super::*;
use crate::traffic_retention::{
    candidate_stream_scan_limit_for_pressure_proof,
    reset_candidate_stream_cursor_for_pressure_proof,
};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, types::Json, Row};
use std::{path::PathBuf, time::Duration};

const PRESSURE_CLIENTS: i64 = 120;
const REVIEW_TRAFFIC_STREAMS: i64 = 7;
const MAX_FULL_ROTATIONS: usize = 8;
const DELEGATED_SEMANTIC_ENV: &str = "VPSMAN_RETAINED_HISTORY_SEMANTICS_DELEGATED";
const SEMANTIC_HASH_SQL: &str =
    include_str!("../../../../scripts/fixtures/prove-monitoring-five-year-semantic-hashes.sql");

#[derive(Default)]
struct RotationTotals {
    destination_conflicts: u64,
    mutations: u64,
    network_observation_destination_conflicts: u64,
    network_observation_destination_rows_written: u64,
    network_observation_expired_exact_rows_pruned: u64,
    network_observation_expired_rollup_rows_pruned: u64,
    network_observation_source_rows_promoted: u64,
    network_rate_spans_merged: u64,
    ping_spans_merged: u64,
    resource_spans_merged: u64,
    system_metric_spans_merged: u64,
    traffic_raw_rows_promoted: u64,
    traffic_rollup_rows_promoted: u64,
    traffic_rollup_rows_pruned: u64,
}

impl RotationTotals {
    fn add(&mut self, run: TelemetryHistoryRetentionRun) {
        self.resource_spans_merged = self
            .resource_spans_merged
            .saturating_add(run.resource_spans_merged);
        self.network_rate_spans_merged = self
            .network_rate_spans_merged
            .saturating_add(run.network_rate_spans_merged);
        self.ping_spans_merged = self.ping_spans_merged.saturating_add(run.ping_spans_merged);
        self.system_metric_spans_merged = self
            .system_metric_spans_merged
            .saturating_add(run.system_metric_spans_merged);
        self.traffic_raw_rows_promoted = self
            .traffic_raw_rows_promoted
            .saturating_add(run.traffic_raw_rows_promoted);
        self.traffic_rollup_rows_promoted = self
            .traffic_rollup_rows_promoted
            .saturating_add(run.traffic_rollup_rows_promoted);
        self.traffic_rollup_rows_pruned = self
            .traffic_rollup_rows_pruned
            .saturating_add(run.traffic_rollup_rows_pruned);
        self.network_observation_source_rows_promoted = self
            .network_observation_source_rows_promoted
            .saturating_add(run.network_observation_source_rows_promoted);
        self.network_observation_destination_rows_written = self
            .network_observation_destination_rows_written
            .saturating_add(run.network_observation_destination_rows_written);
        self.network_observation_expired_exact_rows_pruned = self
            .network_observation_expired_exact_rows_pruned
            .saturating_add(run.network_observation_expired_exact_rows_pruned);
        self.network_observation_expired_rollup_rows_pruned = self
            .network_observation_expired_rollup_rows_pruned
            .saturating_add(run.network_observation_expired_rollup_rows_pruned);
        self.destination_conflicts = self
            .destination_conflicts
            .saturating_add(run.resource_promotion_conflicts)
            .saturating_add(run.network_rate_promotion_conflicts)
            .saturating_add(run.ping_promotion_conflicts)
            .saturating_add(run.system_metric_promotion_conflicts)
            .saturating_add(run.traffic_promotion_conflicts);
        self.network_observation_destination_conflicts = self
            .network_observation_destination_conflicts
            .saturating_add(run.network_observation_destination_conflicts);
        self.mutations = self
            .mutations
            .saturating_add(run.resource_spans_merged)
            .saturating_add(run.network_rate_spans_merged)
            .saturating_add(run.ping_spans_merged)
            .saturating_add(run.system_metric_spans_merged)
            .saturating_add(run.samples_pruned)
            .saturating_add(run.rollups_pruned)
            .saturating_add(run.network_rates_pruned)
            .saturating_add(run.ping_rollups_pruned)
            .saturating_add(run.ping_facts_pruned)
            .saturating_add(run.system_metric_rollups_pruned)
            .saturating_add(run.traffic_raw_rows_promoted)
            .saturating_add(run.traffic_rollup_rows_promoted)
            .saturating_add(run.traffic_rollup_rows_pruned)
            .saturating_add(run.network_observation_destination_rows_written)
            .saturating_add(run.network_observation_expired_exact_rows_pruned)
            .saturating_add(run.network_observation_expired_rollup_rows_pruned)
            .saturating_add(run.network_observation_inactive_latest_pruned)
            .saturating_add(run.network_observation_inactive_series_pruned);
    }

    fn as_json(&self, ordinal: usize, calls: usize) -> Value {
        json!({
            "ordinal": ordinal,
            "calls": calls,
            "mutations": self.mutations,
            "destination_conflicts": self.destination_conflicts,
            "resource_spans_merged": self.resource_spans_merged,
            "network_rate_spans_merged": self.network_rate_spans_merged,
            "ping_spans_merged": self.ping_spans_merged,
            "system_metric_spans_merged": self.system_metric_spans_merged,
            "traffic_raw_rows_promoted": self.traffic_raw_rows_promoted,
            "traffic_rollup_rows_promoted": self.traffic_rollup_rows_promoted,
            "traffic_rollup_rows_pruned": self.traffic_rollup_rows_pruned,
            "network_observation_source_rows_promoted":
                self.network_observation_source_rows_promoted,
            "network_observation_destination_rows_written":
                self.network_observation_destination_rows_written,
            "network_observation_destination_conflicts":
                self.network_observation_destination_conflicts,
            "network_observation_expired_exact_rows_pruned":
                self.network_observation_expired_exact_rows_pruned,
            "network_observation_expired_rollup_rows_pruned":
                self.network_observation_expired_rollup_rows_pruned,
        })
    }
}

async fn conservation_snapshot(pool: &PgPool) -> Result<Value> {
    let row = sqlx::query(
        r#"
        SELECT jsonb_build_object(
            'resource_raw_rows', (SELECT count(*) FROM telemetry_samples
                WHERE client_id LIKE 'pressure-%'),
            'counter_fact_rows', (SELECT count(*) FROM telemetry_counter_facts
                WHERE client_id LIKE 'pressure-%'),
            'resource_represented_minutes', (SELECT COALESCE(sum(sample_count), 0)
                FROM telemetry_rollups WHERE client_id LIKE 'pressure-%'),
            'resource_latest_rows', (SELECT count(*) FROM telemetry_resource_latest
                WHERE client_id LIKE 'pressure-%'),
            'network_rate_represented_minutes', (SELECT COALESCE(sum(sample_count), 0)
                FROM telemetry_network_rates WHERE client_id LIKE 'pressure-%'),
            'ping_fact_rows', (SELECT count(*) FROM telemetry_ping_facts fact
                JOIN telemetry_ping_series series ON series.id = fact.series_id
                WHERE series.client_id LIKE 'pressure-%'),
            'ping_represented_minutes', (SELECT COALESCE(sum(rollup.sample_count), 0)
                FROM telemetry_ping_rollups rollup
                JOIN telemetry_ping_series series ON series.id = rollup.series_id
                WHERE series.client_id LIKE 'pressure-%'),
            'ping_current_rows', (SELECT count(*) FROM telemetry_ping_current current
                JOIN telemetry_ping_series series ON series.id = current.series_id
                WHERE series.client_id LIKE 'pressure-%'),
            'network_observation_represented_checks', (
                SELECT
                    (SELECT count(*) FROM network_observations observation
                        JOIN tunnel_plans plan ON plan.id = observation.plan_id
                        WHERE plan.name LIKE 'pressure-history-plan-%')
                    +
                    (SELECT COALESCE(sum(rollup.sample_count), 0)
                        FROM network_observation_rollups rollup
                        JOIN network_observation_series series ON series.id = rollup.series_id
                        JOIN tunnel_plans plan ON plan.id = series.plan_id
                        WHERE plan.name LIKE 'pressure-history-plan-%')
            ),
            'network_observation_latest_rows', (
                SELECT count(*) FROM network_observation_latest latest
                JOIN network_observation_series series ON series.id = latest.series_id
                JOIN tunnel_plans plan ON plan.id = series.plan_id
                WHERE plan.name LIKE 'pressure-history-plan-%'
            ),
            'system_metric_represented_minutes', (
                SELECT COALESCE(sum(sample_count), 0) FROM system_metric_rollups
                WHERE metric LIKE 'pressure.%'
            ),
            'traffic_hourly_rows', (SELECT count(*) FROM traffic_counter_hourly_usage
                WHERE client_id LIKE 'pressure-%'),
            'traffic_hourly_samples', (SELECT COALESCE(sum(sample_count), 0)
                FROM traffic_counter_hourly_usage WHERE client_id LIKE 'pressure-%'),
            'traffic_hourly_rx_bytes', (SELECT COALESCE(sum(rx_bytes), 0)
                FROM traffic_counter_hourly_usage WHERE client_id LIKE 'pressure-%'),
            'traffic_hourly_tx_bytes', (SELECT COALESCE(sum(tx_bytes), 0)
                FROM traffic_counter_hourly_usage WHERE client_id LIKE 'pressure-%'),
            'clean_traffic_streams', (SELECT count(*)
                FROM traffic_counter_hourly_usage_streams
                WHERE client_id LIKE 'pressure-%'
                  AND source_revision = materialized_revision)
        ) AS snapshot
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(row.try_get::<Json<Value>, _>("snapshot")?.0)
}

async fn semantic_snapshot(pool: &PgPool) -> Result<Value> {
    let mut tx = pool.begin().await?;
    // The standalone ignored proof owns this expensive query.  The integrated
    // pressure harness delegates semantic equality to its already-required
    // before/after snapshots and does not call this helper.
    sqlx::query("SET LOCAL statement_timeout = '300s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL work_mem = '512MB'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL max_parallel_workers_per_gather = 4")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL jit = off").execute(&mut *tx).await?;
    let semantic_row = sqlx::query(SEMANTIC_HASH_SQL).fetch_one(&mut *tx).await?;
    let hashes = semantic_row.try_get::<Json<Value>, _>("semantic_hashes")?.0;
    tx.commit().await?;
    Ok(hashes)
}

async fn row_shape_snapshot(pool: &PgPool) -> Result<Value> {
    let row = sqlx::query(
        r#"
        SELECT jsonb_build_object(
            'telemetry_samples', (SELECT count(*) FROM telemetry_samples
                WHERE client_id LIKE 'pressure-%'),
            'telemetry_counter_facts', (SELECT count(*) FROM telemetry_counter_facts
                WHERE client_id LIKE 'pressure-%'),
            'telemetry_rollups', (SELECT count(*) FROM telemetry_rollups
                WHERE client_id LIKE 'pressure-%'),
            'telemetry_network_rates', (SELECT count(*) FROM telemetry_network_rates
                WHERE client_id LIKE 'pressure-%'),
            'telemetry_ping_facts', (SELECT count(*) FROM telemetry_ping_facts fact
                JOIN telemetry_ping_series series ON series.id = fact.series_id
                WHERE series.client_id LIKE 'pressure-%'),
            'telemetry_ping_rollups', (SELECT count(*) FROM telemetry_ping_rollups rollup
                JOIN telemetry_ping_series series ON series.id = rollup.series_id
                WHERE series.client_id LIKE 'pressure-%'),
            'network_observations', (SELECT count(*) FROM network_observations observation
                JOIN tunnel_plans plan ON plan.id = observation.plan_id
                WHERE plan.name LIKE 'pressure-history-plan-%'),
            'network_observation_rollups', (
                SELECT count(*) FROM network_observation_rollups rollup
                JOIN network_observation_series series ON series.id = rollup.series_id
                JOIN tunnel_plans plan ON plan.id = series.plan_id
                WHERE plan.name LIKE 'pressure-history-plan-%'
            ),
            'system_metric_rollups', (SELECT count(*) FROM system_metric_rollups
                WHERE metric LIKE 'pressure.%'),
            'traffic_counter_samples', (SELECT count(*) FROM traffic_counter_samples
                WHERE client_id LIKE 'pressure-%'),
            'traffic_counter_rollups', (SELECT count(*) FROM traffic_counter_rollups
                WHERE client_id LIKE 'pressure-%')
        ) AS snapshot
        "#,
    )
    .fetch_one(pool)
    .await?;
    Ok(row.try_get::<Json<Value>, _>("snapshot")?.0)
}

async fn run_pressure_maintenance(pool: &PgPool) -> Result<Value> {
    let semantic_delegated = std::env::var(DELEGATED_SEMANTIC_ENV).as_deref() == Ok("1");
    let pressure_clients: i64 =
        sqlx::query_scalar("SELECT count(*) FROM clients WHERE id LIKE 'pressure-%'")
            .fetch_one(pool)
            .await?;
    let traffic_streams: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*) FROM traffic_counter_hourly_usage_streams
        WHERE client_id LIKE 'pressure-%'
        "#,
    )
    .fetch_one(pool)
    .await?;
    let traffic_registry_clean: bool = sqlx::query_scalar(
        r#"
        SELECT COALESCE(bool_and(source_revision = materialized_revision), TRUE)
        FROM traffic_counter_hourly_usage_streams
        WHERE client_id LIKE 'pressure-%'
        "#,
    )
    .fetch_one(pool)
    .await?;
    let registry_streams: i64 =
        sqlx::query_scalar("SELECT count(*) FROM traffic_counter_hourly_usage_streams")
            .fetch_one(pool)
            .await?;
    let non_pressure_registry_streams = registry_streams - traffic_streams;
    ensure!(
        pressure_clients == PRESSURE_CLIENTS,
        "pressure client count changed"
    );
    ensure!(
        traffic_streams == PRESSURE_CLIENTS,
        "traffic stream count changed"
    );
    ensure!(
        traffic_registry_clean,
        "pressure traffic registry contains dirty source/materialized revisions"
    );
    ensure!(
        non_pressure_registry_streams == REVIEW_TRAFFIC_STREAMS,
        "review traffic registry stream count changed"
    );

    let traffic_stream_scan_limit = candidate_stream_scan_limit_for_pressure_proof();
    ensure!(
        traffic_stream_scan_limit > 0,
        "traffic cursor limit is invalid"
    );
    let calls_per_rotation =
        ((registry_streams + traffic_stream_scan_limit - 1) / traffic_stream_scan_limit) as usize;
    ensure!(
        calls_per_rotation == 10,
        "unexpected traffic cursor rotation width"
    );
    // The cursor is process-local scheduling state. Establish a known first
    // page before counting rotations in this isolated ignored proof.
    reset_candidate_stream_cursor_for_pressure_proof();

    let before_conservation = conservation_snapshot(pool).await?;
    let before_semantics = if semantic_delegated {
        None
    } else {
        Some(semantic_snapshot(pool).await?)
    };
    let mut before_shape = row_shape_snapshot(pool).await?;
    let mut rotations = Vec::new();
    let mut stable_rotation = None;
    for rotation in 1..=MAX_FULL_ROTATIONS {
        let mut totals = RotationTotals::default();
        for _ in 0..calls_per_rotation {
            totals.add(process_telemetry_history_retention(pool).await?);
        }
        let after_conservation = conservation_snapshot(pool).await?;
        ensure!(
            after_conservation == before_conservation,
            "retention changed represented samples, exact current state, or hourly traffic bytes"
        );
        let after_shape = row_shape_snapshot(pool).await?;
        if totals.mutations == 0 {
            ensure!(
                after_shape == before_shape,
                "zero-write rotation changed retained row cardinalities"
            );
            if rotation >= 2 {
                if let Some(before_semantics) = before_semantics.as_ref() {
                    let after_semantics = semantic_snapshot(pool).await?;
                    ensure!(
                        after_semantics == *before_semantics,
                        "retention changed canonical monitoring values or traffic reset/epoch semantics"
                    );
                }
                stable_rotation = Some(rotation);
            }
        }
        rotations.push(totals.as_json(rotation, calls_per_rotation));
        before_shape = after_shape;
        if stable_rotation.is_some() {
            break;
        }
    }
    let stable_rotation = stable_rotation
        .context("maintenance did not reach a complete idempotent zero-write cursor rotation")?;
    let mut reported_conservation = before_conservation;
    if let Some(before_semantics) = before_semantics {
        reported_conservation
            .as_object_mut()
            .context("conservation snapshot was not a JSON object")?
            .insert("semantic_hashes".to_string(), before_semantics);
    }
    Ok(json!({
        "schema": "vpsman-five-year-retained-maintenance/v1",
        "pressure_clients": pressure_clients,
        "traffic_streams": traffic_streams,
        "registry_streams": registry_streams,
        "non_pressure_registry_streams": non_pressure_registry_streams,
        "traffic_registry_clean": traffic_registry_clean,
        "cursor_reset_before_first_rotation": true,
        "traffic_stream_scan_limit": traffic_stream_scan_limit,
        "calls_per_full_rotation": calls_per_rotation,
        "completed_rotations": rotations.len(),
        "maximum_rotations": MAX_FULL_ROTATIONS,
        "stable_zero_write_rotation": stable_rotation,
        "conservation": reported_conservation,
        "final_row_shape": before_shape,
        "rotations": rotations,
        "full_cursor_rotation_proven": true,
        "idempotent_empty_rotation_proven": true,
        "conservation_proven": true,
        "semantic_conservation_proven": true,
        "semantic_conservation_delegated": semantic_delegated,
    }))
}

#[tokio::test]
#[ignore = "storage-backed 120-client retained-history pressure proof"]
async fn postgres_pressure_retained_history_worker_is_conservative_and_idempotent() {
    assert_eq!(
        std::env::var("VPSMAN_RETAINED_HISTORY_PRESSURE").as_deref(),
        Ok("1"),
        "pressure helper requires VPSMAN_RETAINED_HISTORY_PRESSURE=1"
    );
    let database_url = std::env::var("VPSMAN_RETAINED_HISTORY_PRESSURE_DATABASE_URL")
        .expect("pressure helper database URL is required");
    let report_path = PathBuf::from(
        std::env::var("VPSMAN_RETAINED_HISTORY_PRESSURE_REPORT")
            .expect("pressure helper report path is required"),
    );
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(Duration::from_secs(30))
        .connect(&database_url)
        .await
        .expect("pressure helper could not connect");
    let run = run_pressure_maintenance(&pool).await;
    pool.close().await;
    let report = run.expect("pressure helper maintenance failed");
    std::fs::write(
        &report_path,
        serde_json::to_vec_pretty(&report).expect("pressure report serialization failed"),
    )
    .expect("pressure helper report could not be written");
}
