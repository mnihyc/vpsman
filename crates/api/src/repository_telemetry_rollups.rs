use anyhow::Result;
use sqlx::Row;

use crate::{
    model::{
        TelemetryNetworkRateView, TelemetryRollupView, TelemetryTunnelAdapterHealthView,
        TelemetryTunnelView, TunnelPlanView,
    },
    repository::Repository,
};

impl Repository {
    pub(crate) async fn list_telemetry_rollups(
        &self,
        limit: i64,
        client_id: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        match self {
            Self::Memory(memory) => {
                let mut rows = memory
                    .telemetry_rollups
                    .read()
                    .await
                    .iter()
                    .filter(|rollup| {
                        client_id.is_none_or(|client_id| rollup.client_id == client_id)
                            && bucket_secs
                                .is_none_or(|bucket_secs| rollup.bucket_secs == bucket_secs)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(|left, right| {
                    right
                        .bucket_start
                        .cmp(&left.bucket_start)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                rows.truncate(limit.clamp(1, 200) as usize);
                Ok(rows)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        bucket_start::text AS bucket_start,
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
                        latest_observed_at::text AS latest_observed_at,
                        updated_at::text AS updated_at
                    FROM telemetry_rollups
                    WHERE
                        ($1::TEXT IS NULL OR client_id = $1)
                        AND ($2::INTEGER IS NULL OR bucket_secs = $2)
                    ORDER BY bucket_start DESC, client_id ASC
                    LIMIT $3
                    "#,
                )
                .bind(client_id)
                .bind(bucket_secs)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;

                rows.into_iter()
                    .map(|row| {
                        Ok(TelemetryRollupView {
                            client_id: row.try_get("client_id")?,
                            bucket_start: row.try_get("bucket_start")?,
                            bucket_secs: row.try_get("bucket_secs")?,
                            sample_count: row.try_get("sample_count")?,
                            cpu_load_1_avg: row.try_get("cpu_load_1_avg")?,
                            cpu_load_1_max: row.try_get("cpu_load_1_max")?,
                            memory_total_bytes_max: row.try_get("memory_total_bytes_max")?,
                            memory_available_bytes_avg: row
                                .try_get("memory_available_bytes_avg")?,
                            memory_available_bytes_min: row
                                .try_get("memory_available_bytes_min")?,
                            disk_total_bytes_max: row.try_get("disk_total_bytes_max")?,
                            disk_available_bytes_avg: row.try_get("disk_available_bytes_avg")?,
                            disk_available_bytes_min: row.try_get("disk_available_bytes_min")?,
                            network_rx_bytes_max: row.try_get("network_rx_bytes_max")?,
                            network_tx_bytes_max: row.try_get("network_tx_bytes_max")?,
                            latest_observed_at: row.try_get("latest_observed_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_telemetry_network_rates(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        match self {
            Self::Memory(_) => Ok(Vec::new()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        client_id,
                        interface,
                        bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        rx_bytes_delta,
                        tx_bytes_delta,
                        rx_bps_avg,
                        tx_bps_avg,
                        first_observed_at::text AS first_observed_at,
                        latest_observed_at::text AS latest_observed_at,
                        updated_at::text AS updated_at
                    FROM telemetry_network_rates
                    WHERE
                        ($1::TEXT IS NULL OR client_id = $1)
                        AND ($2::TEXT IS NULL OR interface = $2)
                        AND ($3::INTEGER IS NULL OR bucket_secs = $3)
                    ORDER BY bucket_start DESC, client_id ASC, interface ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(interface)
                .bind(bucket_secs)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;

                rows.into_iter()
                    .map(|row| {
                        Ok(TelemetryNetworkRateView {
                            client_id: row.try_get("client_id")?,
                            interface: row.try_get("interface")?,
                            bucket_start: row.try_get("bucket_start")?,
                            bucket_secs: row.try_get("bucket_secs")?,
                            sample_count: row.try_get("sample_count")?,
                            rx_bytes_delta: row.try_get("rx_bytes_delta")?,
                            tx_bytes_delta: row.try_get("tx_bytes_delta")?,
                            rx_bps_avg: row.try_get("rx_bps_avg")?,
                            tx_bps_avg: row.try_get("tx_bps_avg")?,
                            first_observed_at: row.try_get("first_observed_at")?,
                            latest_observed_at: row.try_get("latest_observed_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_telemetry_tunnels(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
    ) -> Result<Vec<TelemetryTunnelView>> {
        match self {
            Self::Memory(memory) => {
                let mut records = memory.telemetry_tunnels.read().await.clone();
                let plans = memory.tunnel_plans.read().await.clone();
                correlate_telemetry_tunnels_with_plans(&mut records, &plans);
                records.retain(|record| {
                    client_id.is_none_or(|expected| record.client_id == expected)
                        && interface.is_none_or(|expected| record.interface == expected)
                });
                records.sort_by(|left, right| {
                    right
                        .observed_at
                        .cmp(&left.observed_at)
                        .then_with(|| left.client_id.cmp(&right.client_id))
                        .then_with(|| left.interface.cmp(&right.interface))
                });
                records.truncate(limit.clamp(1, 200) as usize);
                Ok(records)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH latest_samples AS (
                        SELECT DISTINCT ON (client_id)
                            client_id,
                            observed_at,
                            payload
                        FROM telemetry_samples
                        WHERE $1::TEXT IS NULL OR client_id = $1
                        ORDER BY client_id, observed_at DESC
                    )
                    SELECT
                        sample.client_id,
                        sample.observed_at::text AS observed_at,
                        tunnel->>'interface' AS interface,
                        tunnel->>'kind' AS kind,
                        COALESCE(tunnel->>'ownership_mode', 'runtime_observed') AS ownership_mode,
                        COALESCE(
                            tunnel->>'mutation_policy',
                            CASE
                                WHEN COALESCE(tunnel->>'ownership_mode', 'runtime_observed') = 'runtime_observed'
                                THEN 'observe_only_import_candidate'
                                ELSE 'unknown'
                            END
                        ) AS mutation_policy,
                        CASE
                            WHEN jsonb_typeof(tunnel->'promotion_required') = 'boolean'
                            THEN (tunnel->>'promotion_required')::boolean
                            ELSE COALESCE(tunnel->>'ownership_mode', 'runtime_observed') = 'runtime_observed'
                        END AS promotion_required,
                        COALESCE(tunnel->>'source', 'telemetry_payload') AS source,
                        tunnel->>'operstate' AS operstate,
                        CASE
                            WHEN jsonb_typeof(tunnel->'mtu') = 'number'
                            THEN (tunnel->>'mtu')::bigint
                            ELSE NULL
                        END AS mtu,
                        CASE
                            WHEN jsonb_typeof(tunnel->'link_type') = 'number'
                            THEN (tunnel->>'link_type')::bigint
                            ELSE NULL
                        END AS link_type,
                        tunnel->>'address' AS address,
                        CASE
                            WHEN jsonb_typeof(tunnel->'rx_bytes') = 'number'
                            THEN (tunnel->>'rx_bytes')::bigint
                            ELSE 0
                        END AS rx_bytes,
                        CASE
                            WHEN jsonb_typeof(tunnel->'tx_bytes') = 'number'
                            THEN (tunnel->>'tx_bytes')::bigint
                            ELSE 0
                        END AS tx_bytes,
                        tunnel->>'traffic_source' AS traffic_source,
                        tunnel->>'traffic_status' AS traffic_status,
                        tunnel->>'traffic_reason' AS traffic_reason,
                        CASE
                            WHEN jsonb_typeof(tunnel->'traffic_checked_unix') = 'number'
                            THEN (tunnel->>'traffic_checked_unix')::bigint
                            ELSE NULL
                        END AS traffic_checked_unix,
                        tunnel->>'plan_id' AS telemetry_plan_id,
                        tunnel->>'plan_name' AS telemetry_plan_name,
                        tunnel->>'plan_runtime_manager' AS telemetry_plan_runtime_manager,
                        tunnel->>'endpoint_side' AS telemetry_endpoint_side,
                        tunnel->>'peer_client_id' AS telemetry_peer_client_id,
                        tunnel->'adapter_health' AS adapter_health
                    FROM latest_samples sample
                    CROSS JOIN LATERAL jsonb_array_elements(
                        CASE
                            WHEN jsonb_typeof(sample.payload->'tunnels') = 'array'
                            THEN sample.payload->'tunnels'
                            ELSE '[]'::jsonb
                        END
                    ) AS tunnel
                    WHERE tunnel ? 'interface'
                      AND tunnel ? 'kind'
                      AND length(tunnel->>'interface') BETWEEN 1 AND 64
                      AND length(tunnel->>'kind') BETWEEN 1 AND 64
                      AND ($2::TEXT IS NULL OR tunnel->>'interface' = $2)
                    ORDER BY sample.observed_at DESC, sample.client_id ASC, interface ASC
                    LIMIT $3
                    "#,
                )
                .bind(client_id)
                .bind(interface)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;

                let mut records = rows
                    .into_iter()
                    .map(|row| {
                        let telemetry_plan_id = row
                            .try_get::<Option<String>, _>("telemetry_plan_id")?
                            .and_then(|value| uuid::Uuid::parse_str(&value).ok());
                        let telemetry_plan_name =
                            row.try_get::<Option<String>, _>("telemetry_plan_name")?;
                        let plan_correlation =
                            if telemetry_plan_id.is_some() || telemetry_plan_name.is_some() {
                                "telemetry_reported_plan"
                            } else {
                                "unmatched"
                            };
                        Ok(TelemetryTunnelView {
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            interface: row.try_get("interface")?,
                            kind: row.try_get("kind")?,
                            ownership_mode: row.try_get("ownership_mode")?,
                            mutation_policy: row.try_get("mutation_policy")?,
                            promotion_required: row.try_get("promotion_required")?,
                            plan_correlation: plan_correlation.to_string(),
                            plan_id: telemetry_plan_id,
                            plan_name: telemetry_plan_name,
                            plan_runtime_manager: row.try_get("telemetry_plan_runtime_manager")?,
                            endpoint_side: row.try_get("telemetry_endpoint_side")?,
                            peer_client_id: row.try_get("telemetry_peer_client_id")?,
                            source: row.try_get("source")?,
                            operstate: row.try_get("operstate")?,
                            mtu: row.try_get("mtu")?,
                            link_type: row.try_get("link_type")?,
                            address: row.try_get("address")?,
                            rx_bytes: row.try_get("rx_bytes")?,
                            tx_bytes: row.try_get("tx_bytes")?,
                            traffic_source: row.try_get("traffic_source")?,
                            traffic_status: row.try_get("traffic_status")?,
                            traffic_reason: row.try_get("traffic_reason")?,
                            traffic_checked_unix: row.try_get("traffic_checked_unix")?,
                            adapter_health: parse_adapter_health(row.try_get("adapter_health")?),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let plans = self.list_tunnel_plans().await?;
                correlate_telemetry_tunnels_with_plans(&mut records, &plans);
                Ok(records)
            }
        }
    }
}

fn correlate_telemetry_tunnels_with_plans(
    records: &mut [TelemetryTunnelView],
    plans: &[TunnelPlanView],
) {
    for record in records {
        record.plan_correlation = if record.promotion_required {
            "unmatched_import_candidate".to_string()
        } else if record.plan_id.is_some() || record.plan_name.is_some() {
            "telemetry_reported_plan".to_string()
        } else {
            "unmatched".to_string()
        };
        let Some(match_record) = plans.iter().find_map(|plan| {
            if plan.plan.interface_name != record.interface {
                return None;
            }
            if plan.left_client_id == record.client_id {
                return Some((plan, "left", plan.right_client_id.as_str()));
            }
            if plan.right_client_id == record.client_id {
                return Some((plan, "right", plan.left_client_id.as_str()));
            }
            None
        }) else {
            continue;
        };
        let (plan, side, peer_client_id) = match_record;
        let manager = plan.plan.runtime_control.manager;
        let runtime_manager = runtime_manager_label(manager);
        record.plan_correlation = "matched_saved_plan".to_string();
        record.plan_id = Some(plan.id);
        record.plan_name = Some(plan.name.clone());
        record.plan_runtime_manager = Some(runtime_manager.to_string());
        record.endpoint_side = Some(side.to_string());
        record.peer_client_id = Some(peer_client_id.to_string());
        record.ownership_mode = runtime_manager.to_string();
        record.mutation_policy = matched_plan_mutation_policy(manager).to_string();
        record.promotion_required = false;
    }
}

fn parse_adapter_health(
    value: Option<serde_json::Value>,
) -> Option<TelemetryTunnelAdapterHealthView> {
    let value = value?;
    if !value.is_object() {
        return None;
    }
    Some(TelemetryTunnelAdapterHealthView {
        status: value.get("status")?.as_str()?.to_string(),
        checked_unix: value
            .get("checked_unix")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        configured: value
            .get("configured")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        success: value
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        exit_code: value
            .get("exit_code")
            .and_then(|value| value.as_i64())
            .and_then(|value| i32::try_from(value).ok()),
        reason: value
            .get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        duration_ms: value
            .get("duration_ms")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        command_sha256_hex: value
            .get("command_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        timed_out: value
            .get("timed_out")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        output_truncated: value
            .get("output_truncated")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        stdout_sha256_hex: value
            .get("stdout_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        stderr_sha256_hex: value
            .get("stderr_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn runtime_manager_label(manager: vpsman_common::RuntimeTunnelManager) -> &'static str {
    match manager {
        vpsman_common::RuntimeTunnelManager::AgentIproute2Managed => "agent_iproute2_managed",
        vpsman_common::RuntimeTunnelManager::ExternalObserved => "external_observed",
        vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter => "external_managed_adapter",
    }
}

fn matched_plan_mutation_policy(manager: vpsman_common::RuntimeTunnelManager) -> &'static str {
    match manager {
        vpsman_common::RuntimeTunnelManager::ExternalObserved => "observe_only_saved_plan",
        vpsman_common::RuntimeTunnelManager::AgentIproute2Managed
        | vpsman_common::RuntimeTunnelManager::ExternalManagedAdapter => "managed_desired",
    }
}
