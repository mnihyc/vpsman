use anyhow::Result;
use sqlx::{PgPool, Row};
use vpsman_common::{
    DEFAULT_TELEMETRY_RETENTION_DAYS, DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT,
    MIN_TRAFFIC_COUNTER_RETENTION_DAYS,
};

#[derive(Clone, Copy)]
struct RetentionPolicy {
    enabled: bool,
    prune_limit: i32,
    retention_days: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TelemetryHistoryRetentionRun {
    pub(crate) network_rates_pruned: u64,
    pub(crate) rollups_pruned: u64,
    pub(crate) traffic_counter_samples_pruned: u64,
}

pub(crate) async fn process_telemetry_history_retention(
    pool: &PgPool,
) -> Result<TelemetryHistoryRetentionRun> {
    let rollups = load_policy(pool, "telemetry_rollups").await?;
    let network_rates = load_policy(pool, "telemetry_network_rates").await?;
    let traffic_counter_samples = load_policy(pool, "traffic_counter_samples").await?;
    Ok(TelemetryHistoryRetentionRun {
        rollups_pruned: prune_domain(pool, "telemetry_rollups", rollups).await?,
        network_rates_pruned: prune_domain(pool, "telemetry_network_rates", network_rates).await?,
        traffic_counter_samples_pruned: prune_domain(
            pool,
            "traffic_counter_samples",
            traffic_counter_samples,
        )
        .await?,
    })
}

async fn load_policy(pool: &PgPool, domain: &str) -> Result<RetentionPolicy> {
    let minimum_retention_days = if domain == "traffic_counter_samples" {
        MIN_TRAFFIC_COUNTER_RETENTION_DAYS
    } else {
        1
    };
    let row = sqlx::query(
        r#"
        SELECT retention_days, prune_limit, enabled
        FROM history_retention_policies
        WHERE domain = $1
        "#,
    )
    .bind(domain)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Ok(RetentionPolicy {
            enabled: true,
            prune_limit: DEFAULT_TELEMETRY_RETENTION_PRUNE_LIMIT,
            retention_days: DEFAULT_TELEMETRY_RETENTION_DAYS,
        });
    };
    Ok(RetentionPolicy {
        enabled: row.try_get("enabled")?,
        prune_limit: row.try_get::<i32, _>("prune_limit")?.clamp(1, 100_000),
        retention_days: row
            .try_get::<i32, _>("retention_days")?
            .clamp(minimum_retention_days, 3_650),
    })
}

async fn prune_domain(pool: &PgPool, domain: &str, policy: RetentionPolicy) -> Result<u64> {
    if !policy.enabled {
        return Ok(0);
    }
    let query = match domain {
        "telemetry_rollups" => prune_query("telemetry_rollups"),
        "telemetry_network_rates" => prune_query("telemetry_network_rates"),
        "traffic_counter_samples" => traffic_counter_prune_query(),
        _ => return Ok(0),
    };
    let result = sqlx::query(&query)
        .bind(policy.retention_days)
        .bind(policy.prune_limit)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

fn prune_query(table: &str) -> String {
    format!(
        r#"
        WITH candidates AS (
            SELECT ctid
            FROM {table}
            WHERE bucket_start < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
            ORDER BY bucket_start ASC
            LIMIT $2
            FOR UPDATE SKIP LOCKED
        )
        DELETE FROM {table}
        WHERE ctid IN (SELECT ctid FROM candidates)
        "#
    )
}

fn traffic_counter_prune_query() -> String {
    r#"
        WITH candidates AS (
            SELECT sample.ctid
            FROM traffic_counter_samples sample
            WHERE sample.observed_at < (
                date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
            ) - make_interval(days => $1)
              AND EXISTS (
                  SELECT 1
                  FROM traffic_counter_samples newer
                  WHERE newer.client_id = sample.client_id
                    AND newer.source_kind = sample.source_kind
                    AND newer.interface = sample.interface
                    AND newer.observed_at < (
                        date_trunc('day', now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
                    ) - make_interval(days => $1)
                    AND newer.observed_at > sample.observed_at
              )
            ORDER BY
                sample.observed_at ASC,
                sample.client_id ASC,
                sample.source_kind ASC,
                sample.interface ASC
            LIMIT $2
            FOR UPDATE OF sample SKIP LOCKED
        )
        DELETE FROM traffic_counter_samples
        WHERE ctid IN (SELECT ctid FROM candidates)
        "#
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::{prune_query, traffic_counter_prune_query};

    #[test]
    fn telemetry_pruning_is_bounded_and_concurrency_safe() {
        let query = prune_query("telemetry_rollups");
        assert!(query.contains("LIMIT $2"));
        assert!(query.contains("FOR UPDATE SKIP LOCKED"));
        assert!(query.contains("bucket_start"));
        assert!(!query.contains("DELETE FROM telemetry_network_rates"));
    }

    #[test]
    fn traffic_counter_pruning_preserves_each_stream_baseline() {
        let query = traffic_counter_prune_query();
        assert!(query.contains("newer.client_id = sample.client_id"));
        assert!(query.contains("newer.source_kind = sample.source_kind"));
        assert!(query.contains("newer.interface = sample.interface"));
        assert!(query.contains("newer.observed_at > sample.observed_at"));
        assert!(query.contains("LIMIT $2"));
        assert!(query.contains("FOR UPDATE OF sample SKIP LOCKED"));
    }
}
