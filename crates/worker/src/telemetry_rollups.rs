use anyhow::Result;
use sqlx::PgPool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TelemetryRollupConfig {
    pub(crate) bucket_secs: i64,
    pub(crate) lookback_hours: i64,
    pub(crate) prune_after_hours: i64,
}

impl TelemetryRollupConfig {
    pub(crate) fn new(bucket_secs: u64, lookback_hours: u64, prune_after_hours: u64) -> Self {
        Self {
            bucket_secs: (bucket_secs as i64).clamp(60, 86_400),
            lookback_hours: (lookback_hours as i64).clamp(1, 720),
            prune_after_hours: (prune_after_hours as i64).clamp(1, 8_760),
        }
    }
}

pub(crate) async fn refresh_telemetry_rollups(
    pool: &PgPool,
    config: TelemetryRollupConfig,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH bucketed AS (
            SELECT
                client_id,
                to_timestamp(
                    floor(
                        extract(epoch FROM observed_at) / $1::double precision
                    ) * $1::double precision
                ) AS bucket_start,
                observed_at,
                cpu_load_1,
                memory_total_bytes,
                memory_available_bytes,
                COALESCE((
                    SELECT sum((disk->>'total_bytes')::bigint)
                    FROM jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(payload->'disks') = 'array'
                            THEN payload->'disks'
                            ELSE '[]'::jsonb
                        END
                    ) AS disk
                ), 0) AS disk_total_bytes,
                COALESCE((
                    SELECT sum((disk->>'available_bytes')::bigint)
                    FROM jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(payload->'disks') = 'array'
                            THEN payload->'disks'
                            ELSE '[]'::jsonb
                        END
                    ) AS disk
                ), 0) AS disk_available_bytes,
                COALESCE((
                    SELECT sum((network->>'rx_bytes')::bigint)
                    FROM jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(payload->'networks') = 'array'
                            THEN payload->'networks'
                            ELSE '[]'::jsonb
                        END
                    ) AS network
                ), 0) AS network_rx_bytes,
                COALESCE((
                    SELECT sum((network->>'tx_bytes')::bigint)
                    FROM jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(payload->'networks') = 'array'
                            THEN payload->'networks'
                            ELSE '[]'::jsonb
                        END
                    ) AS network
                ), 0) AS network_tx_bytes
            FROM telemetry_samples
            WHERE observed_at >= now() - ($2::bigint * interval '1 hour')
        )
        INSERT INTO telemetry_rollups (
            client_id,
            bucket_start,
            bucket_secs,
            sample_count,
            cpu_load_1_avg,
            cpu_load_1_max,
            memory_total_bytes_max,
            memory_available_bytes_avg,
            memory_available_bytes_min,
            disk_total_bytes_max,
            disk_available_bytes_avg,
            disk_available_bytes_min,
            network_rx_bytes_max,
            network_tx_bytes_max,
            latest_observed_at,
            updated_at
        )
        SELECT
            client_id,
            bucket_start,
            $1::integer AS bucket_secs,
            count(*)::integer AS sample_count,
            avg(cpu_load_1)::double precision AS cpu_load_1_avg,
            max(cpu_load_1)::double precision AS cpu_load_1_max,
            max(memory_total_bytes)::bigint AS memory_total_bytes_max,
            round(avg(memory_available_bytes))::bigint AS memory_available_bytes_avg,
            min(memory_available_bytes)::bigint AS memory_available_bytes_min,
            max(disk_total_bytes)::bigint AS disk_total_bytes_max,
            round(avg(disk_available_bytes))::bigint AS disk_available_bytes_avg,
            min(disk_available_bytes)::bigint AS disk_available_bytes_min,
            max(network_rx_bytes)::bigint AS network_rx_bytes_max,
            max(network_tx_bytes)::bigint AS network_tx_bytes_max,
            max(observed_at) AS latest_observed_at,
            now() AS updated_at
        FROM bucketed
        GROUP BY client_id, bucket_start
        ON CONFLICT (client_id, bucket_secs, bucket_start) DO UPDATE SET
            sample_count = EXCLUDED.sample_count,
            cpu_load_1_avg = EXCLUDED.cpu_load_1_avg,
            cpu_load_1_max = EXCLUDED.cpu_load_1_max,
            memory_total_bytes_max = EXCLUDED.memory_total_bytes_max,
            memory_available_bytes_avg = EXCLUDED.memory_available_bytes_avg,
            memory_available_bytes_min = EXCLUDED.memory_available_bytes_min,
            disk_total_bytes_max = EXCLUDED.disk_total_bytes_max,
            disk_available_bytes_avg = EXCLUDED.disk_available_bytes_avg,
            disk_available_bytes_min = EXCLUDED.disk_available_bytes_min,
            network_rx_bytes_max = EXCLUDED.network_rx_bytes_max,
            network_tx_bytes_max = EXCLUDED.network_tx_bytes_max,
            latest_observed_at = EXCLUDED.latest_observed_at,
            updated_at = now()
        "#,
    )
    .bind(config.bucket_secs)
    .bind(config.lookback_hours)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub(crate) async fn refresh_telemetry_network_rates(
    pool: &PgPool,
    config: TelemetryRollupConfig,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        WITH interface_samples AS (
            SELECT
                sample.client_id,
                to_timestamp(
                    floor(
                        extract(epoch FROM sample.observed_at) / $1::double precision
                    ) * $1::double precision
                ) AS bucket_start,
                sample.observed_at,
                network->>'interface' AS interface,
                (network->>'rx_bytes')::bigint AS rx_bytes,
                (network->>'tx_bytes')::bigint AS tx_bytes
            FROM telemetry_samples sample
            CROSS JOIN LATERAL jsonb_array_elements(
                CASE
                    WHEN jsonb_typeof(sample.payload->'networks') = 'array'
                    THEN sample.payload->'networks'
                    ELSE '[]'::jsonb
                END
            ) AS network
            WHERE sample.observed_at >= now() - ($2::bigint * interval '1 hour')
              AND network ? 'interface'
              AND network ? 'rx_bytes'
              AND network ? 'tx_bytes'
              AND length(network->>'interface') BETWEEN 1 AND 64
        ),
        first_samples AS (
            SELECT DISTINCT ON (client_id, interface, bucket_start)
                client_id,
                interface,
                bucket_start,
                observed_at AS first_observed_at,
                rx_bytes AS first_rx_bytes,
                tx_bytes AS first_tx_bytes
            FROM interface_samples
            ORDER BY client_id, interface, bucket_start, observed_at ASC
        ),
        latest_samples AS (
            SELECT DISTINCT ON (client_id, interface, bucket_start)
                client_id,
                interface,
                bucket_start,
                observed_at AS latest_observed_at,
                rx_bytes AS latest_rx_bytes,
                tx_bytes AS latest_tx_bytes
            FROM interface_samples
            ORDER BY client_id, interface, bucket_start, observed_at DESC
        ),
        counts AS (
            SELECT
                client_id,
                interface,
                bucket_start,
                count(*)::integer AS sample_count
            FROM interface_samples
            GROUP BY client_id, interface, bucket_start
        ),
        rates AS (
            SELECT
                latest.client_id,
                latest.interface,
                latest.bucket_start,
                counts.sample_count,
                GREATEST(latest.latest_rx_bytes - first.first_rx_bytes, 0)::bigint AS rx_bytes_delta,
                GREATEST(latest.latest_tx_bytes - first.first_tx_bytes, 0)::bigint AS tx_bytes_delta,
                GREATEST(
                    extract(epoch FROM latest.latest_observed_at - first.first_observed_at),
                    1
                )::double precision AS duration_secs,
                first.first_observed_at,
                latest.latest_observed_at
            FROM latest_samples latest
            JOIN first_samples first
              ON first.client_id = latest.client_id
             AND first.interface = latest.interface
             AND first.bucket_start = latest.bucket_start
            JOIN counts
              ON counts.client_id = latest.client_id
             AND counts.interface = latest.interface
             AND counts.bucket_start = latest.bucket_start
        )
        INSERT INTO telemetry_network_rates (
            client_id,
            interface,
            bucket_start,
            bucket_secs,
            sample_count,
            rx_bytes_delta,
            tx_bytes_delta,
            rx_bps_avg,
            tx_bps_avg,
            first_observed_at,
            latest_observed_at,
            updated_at
        )
        SELECT
            client_id,
            interface,
            bucket_start,
            $1::integer AS bucket_secs,
            sample_count,
            rx_bytes_delta,
            tx_bytes_delta,
            (rx_bytes_delta * 8)::double precision / duration_secs AS rx_bps_avg,
            (tx_bytes_delta * 8)::double precision / duration_secs AS tx_bps_avg,
            first_observed_at,
            latest_observed_at,
            now()
        FROM rates
        ON CONFLICT (client_id, interface, bucket_secs, bucket_start) DO UPDATE SET
            sample_count = EXCLUDED.sample_count,
            rx_bytes_delta = EXCLUDED.rx_bytes_delta,
            tx_bytes_delta = EXCLUDED.tx_bytes_delta,
            rx_bps_avg = EXCLUDED.rx_bps_avg,
            tx_bps_avg = EXCLUDED.tx_bps_avg,
            first_observed_at = EXCLUDED.first_observed_at,
            latest_observed_at = EXCLUDED.latest_observed_at,
            updated_at = now()
        "#,
    )
    .bind(config.bucket_secs)
    .bind(config.lookback_hours)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

pub(crate) async fn prune_telemetry_samples(
    pool: &PgPool,
    config: TelemetryRollupConfig,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        DELETE FROM telemetry_samples
        WHERE observed_at < now() - ($1::bigint * interval '1 hour')
        "#,
    )
    .bind(config.prune_after_hours)
    .execute(pool)
    .await?;

    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::TelemetryRollupConfig;

    #[test]
    fn telemetry_rollup_config_is_bounded() {
        assert_eq!(
            TelemetryRollupConfig::new(1, 0, 0),
            TelemetryRollupConfig {
                bucket_secs: 60,
                lookback_hours: 1,
                prune_after_hours: 1,
            }
        );
        assert_eq!(
            TelemetryRollupConfig::new(90_000, 1_000, 10_000),
            TelemetryRollupConfig {
                bucket_secs: 86_400,
                lookback_hours: 720,
                prune_after_hours: 8_760,
            }
        );
    }
}
