use std::collections::HashMap;

use anyhow::Result;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{CommandOutput, OutputStream};

use crate::{
    model::{NetworkObservationTrendView, NetworkObservationView},
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_network_observations(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkObservationView>> {
        match self {
            Self::Memory(memory) => {
                let mut observations = memory.network_observations.read().await.clone();
                observations.sort_by(|left, right| {
                    right
                        .observed_at
                        .cmp(&left.observed_at)
                        .then_with(|| right.id.cmp(&left.id))
                });
                Ok(observations.into_iter().take(limit as usize).collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        job_id,
                        client_id,
                        seq,
                        kind,
                        role,
                        plan_name,
                        interface_name,
                        peer_client_id,
                        target,
                        healthy,
                        latency_avg_ms,
                        packet_loss_ratio,
                        throughput_mbps,
                        bytes,
                        metadata,
                        observed_at::text AS observed_at
                    FROM network_observations
                    ORDER BY observed_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata: SqlJson<serde_json::Value> = row.try_get("metadata")?;
                        Ok(NetworkObservationView {
                            id: row.try_get("id")?,
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            seq: row.try_get("seq")?,
                            kind: row.try_get("kind")?,
                            role: row.try_get("role")?,
                            plan_name: row.try_get("plan_name")?,
                            interface_name: row.try_get("interface_name")?,
                            peer_client_id: row.try_get("peer_client_id")?,
                            target: row.try_get("target")?,
                            healthy: row.try_get("healthy")?,
                            latency_avg_ms: row.try_get("latency_avg_ms")?,
                            packet_loss_ratio: row.try_get("packet_loss_ratio")?,
                            throughput_mbps: row.try_get("throughput_mbps")?,
                            bytes: row.try_get("bytes")?,
                            metadata: metadata.0,
                            observed_at: row.try_get("observed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_network_observation_trends(
        &self,
        limit: i64,
    ) -> Result<Vec<NetworkObservationTrendView>> {
        match self {
            Self::Memory(memory) => {
                let observations = memory.network_observations.read().await;
                let mut trends = summarize_network_observation_trends(&observations);
                trends.sort_by(|left, right| {
                    right
                        .latest_observed_at
                        .cmp(&left.latest_observed_at)
                        .then_with(|| right.kind.cmp(&left.kind))
                        .then_with(|| right.client_id.cmp(&left.client_id))
                });
                Ok(trends.into_iter().take(limit as usize).collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        kind,
                        plan_name,
                        interface_name,
                        client_id,
                        peer_client_id,
                        COUNT(*)::BIGINT AS sample_count,
                        COUNT(*) FILTER (WHERE healthy IS TRUE)::BIGINT AS healthy_count,
                        COUNT(*) FILTER (WHERE healthy IS FALSE)::BIGINT AS degraded_count,
                        AVG(latency_avg_ms) AS latency_avg_ms,
                        MIN(latency_avg_ms) AS latency_min_ms,
                        MAX(latency_avg_ms) AS latency_max_ms,
                        AVG(packet_loss_ratio) AS packet_loss_avg_ratio,
                        AVG(throughput_mbps) AS throughput_avg_mbps,
                        MAX(throughput_mbps) AS throughput_max_mbps,
                        COALESCE(SUM(bytes), 0)::BIGINT AS bytes_total,
                        MAX(observed_at)::text AS latest_observed_at
                    FROM network_observations
                    GROUP BY kind, plan_name, interface_name, client_id, peer_client_id
                    ORDER BY MAX(observed_at) DESC, kind ASC, client_id ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(NetworkObservationTrendView {
                            kind: row.try_get("kind")?,
                            plan_name: row.try_get("plan_name")?,
                            interface_name: row.try_get("interface_name")?,
                            client_id: row.try_get("client_id")?,
                            peer_client_id: row.try_get("peer_client_id")?,
                            sample_count: row.try_get("sample_count")?,
                            healthy_count: row.try_get("healthy_count")?,
                            degraded_count: row.try_get("degraded_count")?,
                            latency_avg_ms: row.try_get("latency_avg_ms")?,
                            latency_min_ms: row.try_get("latency_min_ms")?,
                            latency_max_ms: row.try_get("latency_max_ms")?,
                            packet_loss_avg_ratio: row.try_get("packet_loss_avg_ratio")?,
                            throughput_avg_mbps: row.try_get("throughput_avg_mbps")?,
                            throughput_max_mbps: row.try_get("throughput_max_mbps")?,
                            bytes_total: row.try_get("bytes_total")?,
                            latest_observed_at: row.try_get("latest_observed_at")?,
                        })
                    })
                    .collect()
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn record_network_observations(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[CommandOutput],
    ) -> Result<()> {
        self.record_network_observations_starting_at(job_id, client_id, 0, outputs)
            .await
    }

    pub(crate) async fn record_network_observations_starting_at(
        &self,
        job_id: Uuid,
        client_id: &str,
        start_seq: i32,
        outputs: &[CommandOutput],
    ) -> Result<()> {
        let observed_at = unix_now().to_string();
        let observations = outputs
            .iter()
            .enumerate()
            .filter_map(|(seq, output)| {
                let seq = start_seq.checked_add(i32::try_from(seq).ok()?)?;
                parse_network_observation(job_id, client_id, seq, output, &observed_at)
            })
            .collect::<Vec<_>>();
        if observations.is_empty() {
            return Ok(());
        }
        match self {
            Self::Memory(memory) => {
                let mut stored = memory.network_observations.write().await;
                for observation in observations {
                    if let Some(existing) = stored.iter_mut().find(|existing| {
                        existing.job_id == observation.job_id
                            && existing.client_id == observation.client_id
                            && existing.seq == observation.seq
                    }) {
                        *existing = observation;
                    } else {
                        stored.push(observation);
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for observation in observations {
                    sqlx::query(
                        r#"
                        INSERT INTO network_observations (
                            id,
                            job_id,
                            client_id,
                            seq,
                            kind,
                            role,
                            plan_name,
                            interface_name,
                            peer_client_id,
                            target,
                            healthy,
                            latency_avg_ms,
                            packet_loss_ratio,
                            throughput_mbps,
                            bytes,
                            metadata
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
                        ON CONFLICT (job_id, client_id, seq)
                        DO UPDATE SET
                            kind = EXCLUDED.kind,
                            role = EXCLUDED.role,
                            plan_name = EXCLUDED.plan_name,
                            interface_name = EXCLUDED.interface_name,
                            peer_client_id = EXCLUDED.peer_client_id,
                            target = EXCLUDED.target,
                            healthy = EXCLUDED.healthy,
                            latency_avg_ms = EXCLUDED.latency_avg_ms,
                            packet_loss_ratio = EXCLUDED.packet_loss_ratio,
                            throughput_mbps = EXCLUDED.throughput_mbps,
                            bytes = EXCLUDED.bytes,
                            metadata = EXCLUDED.metadata,
                            observed_at = now()
                        "#,
                    )
                    .bind(observation.id)
                    .bind(observation.job_id)
                    .bind(&observation.client_id)
                    .bind(observation.seq)
                    .bind(&observation.kind)
                    .bind(&observation.role)
                    .bind(&observation.plan_name)
                    .bind(&observation.interface_name)
                    .bind(&observation.peer_client_id)
                    .bind(&observation.target)
                    .bind(observation.healthy)
                    .bind(observation.latency_avg_ms)
                    .bind(observation.packet_loss_ratio)
                    .bind(observation.throughput_mbps)
                    .bind(observation.bytes)
                    .bind(SqlJson(&observation.metadata))
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TrendKey {
    kind: String,
    plan_name: Option<String>,
    interface_name: Option<String>,
    client_id: String,
    peer_client_id: Option<String>,
}

#[derive(Clone, Debug)]
struct TrendAccumulator {
    key: TrendKey,
    sample_count: i64,
    healthy_count: i64,
    degraded_count: i64,
    latency_sum_ms: f64,
    latency_count: i64,
    latency_min_ms: Option<f64>,
    latency_max_ms: Option<f64>,
    packet_loss_sum_ratio: f64,
    packet_loss_count: i64,
    throughput_sum_mbps: f64,
    throughput_count: i64,
    throughput_max_mbps: Option<f64>,
    bytes_total: i64,
    latest_observed_at: String,
}

impl TrendAccumulator {
    fn new(observation: &NetworkObservationView) -> Self {
        Self {
            key: TrendKey {
                kind: observation.kind.clone(),
                plan_name: observation.plan_name.clone(),
                interface_name: observation.interface_name.clone(),
                client_id: observation.client_id.clone(),
                peer_client_id: observation.peer_client_id.clone(),
            },
            sample_count: 0,
            healthy_count: 0,
            degraded_count: 0,
            latency_sum_ms: 0.0,
            latency_count: 0,
            latency_min_ms: None,
            latency_max_ms: None,
            packet_loss_sum_ratio: 0.0,
            packet_loss_count: 0,
            throughput_sum_mbps: 0.0,
            throughput_count: 0,
            throughput_max_mbps: None,
            bytes_total: 0,
            latest_observed_at: observation.observed_at.clone(),
        }
    }

    fn add(&mut self, observation: &NetworkObservationView) {
        self.sample_count += 1;
        match observation.healthy {
            Some(true) => self.healthy_count += 1,
            Some(false) => self.degraded_count += 1,
            None => {}
        }
        if let Some(latency) = observation.latency_avg_ms {
            self.latency_sum_ms += latency;
            self.latency_count += 1;
            self.latency_min_ms = Some(
                self.latency_min_ms
                    .map_or(latency, |current| current.min(latency)),
            );
            self.latency_max_ms = Some(
                self.latency_max_ms
                    .map_or(latency, |current| current.max(latency)),
            );
        }
        if let Some(loss) = observation.packet_loss_ratio {
            self.packet_loss_sum_ratio += loss;
            self.packet_loss_count += 1;
        }
        if let Some(throughput) = observation.throughput_mbps {
            self.throughput_sum_mbps += throughput;
            self.throughput_count += 1;
            self.throughput_max_mbps = Some(
                self.throughput_max_mbps
                    .map_or(throughput, |current| current.max(throughput)),
            );
        }
        if let Some(bytes) = observation.bytes {
            self.bytes_total = self.bytes_total.saturating_add(bytes);
        }
        if observation.observed_at > self.latest_observed_at {
            self.latest_observed_at = observation.observed_at.clone();
        }
    }

    fn into_view(self) -> NetworkObservationTrendView {
        NetworkObservationTrendView {
            kind: self.key.kind,
            plan_name: self.key.plan_name,
            interface_name: self.key.interface_name,
            client_id: self.key.client_id,
            peer_client_id: self.key.peer_client_id,
            sample_count: self.sample_count,
            healthy_count: self.healthy_count,
            degraded_count: self.degraded_count,
            latency_avg_ms: average(self.latency_sum_ms, self.latency_count),
            latency_min_ms: self.latency_min_ms,
            latency_max_ms: self.latency_max_ms,
            packet_loss_avg_ratio: average(self.packet_loss_sum_ratio, self.packet_loss_count),
            throughput_avg_mbps: average(self.throughput_sum_mbps, self.throughput_count),
            throughput_max_mbps: self.throughput_max_mbps,
            bytes_total: self.bytes_total,
            latest_observed_at: self.latest_observed_at,
        }
    }
}

fn summarize_network_observation_trends(
    observations: &[NetworkObservationView],
) -> Vec<NetworkObservationTrendView> {
    let mut groups: HashMap<TrendKey, TrendAccumulator> = HashMap::new();
    for observation in observations {
        let key = TrendKey {
            kind: observation.kind.clone(),
            plan_name: observation.plan_name.clone(),
            interface_name: observation.interface_name.clone(),
            client_id: observation.client_id.clone(),
            peer_client_id: observation.peer_client_id.clone(),
        };
        groups
            .entry(key)
            .or_insert_with(|| TrendAccumulator::new(observation))
            .add(observation);
    }
    groups
        .into_values()
        .map(TrendAccumulator::into_view)
        .collect()
}

fn average(sum: f64, count: i64) -> Option<f64> {
    if count > 0 {
        Some(sum / count as f64)
    } else {
        None
    }
}

fn parse_network_observation(
    job_id: Uuid,
    client_id: &str,
    seq: i32,
    output: &CommandOutput,
    observed_at: &str,
) -> Option<NetworkObservationView> {
    if output.stream != OutputStream::Status {
        return None;
    }
    let metadata = serde_json::from_slice::<serde_json::Value>(&output.data).ok()?;
    let kind = as_string(metadata.get("type"))?;
    if !matches!(
        kind.as_str(),
        "network_status" | "network_probe" | "network_speed_test"
    ) {
        return None;
    }
    let is_network_status = kind == "network_status";
    let parsed = metadata.get("parsed").unwrap_or(&serde_json::Value::Null);
    let runtime = metadata.get("runtime").unwrap_or(&serde_json::Value::Null);
    let runtime_summary = runtime.get("summary").unwrap_or(&serde_json::Value::Null);
    let bird2 = runtime.get("bird2").unwrap_or(&serde_json::Value::Null);
    let runtime_health = runtime_summary.get("healthy").and_then(as_bool);
    Some(NetworkObservationView {
        id: Uuid::new_v4(),
        job_id,
        client_id: client_id.to_string(),
        seq,
        kind,
        role: as_string(metadata.get("role")),
        plan_name: as_string(metadata.get("plan")),
        interface_name: as_string(metadata.get("interface")),
        peer_client_id: as_string(metadata.get("peer_client_id")),
        target: as_string(metadata.get("target")).or_else(|| {
            as_string(metadata.get("server_address")).map(|address| {
                match metadata.get("port").and_then(as_i64) {
                    Some(port) => format!("{address}:{port}"),
                    None => address,
                }
            })
        }),
        healthy: if is_network_status {
            runtime_health
                .or_else(|| bird2.get("healthy").and_then(as_bool))
                .or_else(|| metadata.get("applied").and_then(as_bool))
        } else {
            parsed
                .get("healthy")
                .and_then(as_bool)
                .or_else(|| metadata.get("success").and_then(as_bool))
                .or_else(|| metadata.get("applied").and_then(as_bool))
                .or_else(|| bird2.get("healthy").and_then(as_bool))
        },
        latency_avg_ms: parsed.get("latency_avg_ms").and_then(as_f64),
        packet_loss_ratio: parsed.get("packet_loss_ratio").and_then(as_f64),
        throughput_mbps: metadata.get("throughput_mbps").and_then(as_f64),
        bytes: metadata.get("bytes").and_then(as_i64),
        metadata,
        observed_at: observed_at.to_string(),
    })
}

fn as_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn as_bool(value: &serde_json::Value) -> Option<bool> {
    value.as_bool()
}

fn as_f64(value: &serde_json::Value) -> Option<f64> {
    value.as_f64().filter(|value| value.is_finite())
}

fn as_i64(value: &serde_json::Value) -> Option<i64> {
    value.as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_probe_and_speed_status_observations() {
        let job_id = Uuid::new_v4();
        let probe = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_probe",
                "plan": "edge",
                "interface": "tun0",
                "peer_client_id": "right",
                "target": "10.0.0.1",
                "parsed": {
                    "healthy": true,
                    "latency_avg_ms": 12.5,
                    "packet_loss_ratio": 0.01
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        };
        let speed = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_speed_test",
                "role": "client",
                "plan": "edge",
                "interface": "tun0",
                "peer_client_id": "left",
                "server_address": "10.0.0.0",
                "port": 5201,
                "success": true,
                "bytes": 1048576,
                "throughput_mbps": 33.3
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        };

        let parsed_probe = parse_network_observation(job_id, "left", 0, &probe, "1").unwrap();
        let parsed_speed = parse_network_observation(job_id, "right", 1, &speed, "1").unwrap();

        assert_eq!(parsed_probe.kind, "network_probe");
        assert_eq!(parsed_probe.latency_avg_ms, Some(12.5));
        assert_eq!(parsed_probe.packet_loss_ratio, Some(0.01));
        assert_eq!(parsed_probe.healthy, Some(true));
        assert_eq!(parsed_speed.kind, "network_speed_test");
        assert_eq!(parsed_speed.role.as_deref(), Some("client"));
        assert_eq!(parsed_speed.target.as_deref(), Some("10.0.0.0:5201"));
        assert_eq!(parsed_speed.bytes, Some(1_048_576));
        assert_eq!(parsed_speed.throughput_mbps, Some(33.3));
    }

    #[test]
    fn parses_network_status_runtime_summary_before_managed_file_state() {
        let job_id = Uuid::new_v4();
        let status = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_status",
                "plan": "external-edge",
                "interface": "ovpn42",
                "peer_client_id": "right",
                "applied": true,
                "runtime": {
                    "summary": {
                        "manager": "external_managed_adapter",
                        "status": "adapter_unhealthy",
                        "healthy": false,
                        "drift": false,
                        "reasons": ["adapter_status_failed"]
                    },
                    "adapter": {
                        "configured": true,
                        "success": false,
                        "exit_code": 7
                    }
                }
            }))
            .unwrap(),
            exit_code: Some(0),
            done: true,
        };

        let parsed = parse_network_observation(job_id, "left", 3, &status, "1").unwrap();

        assert_eq!(parsed.kind, "network_status");
        assert_eq!(parsed.plan_name.as_deref(), Some("external-edge"));
        assert_eq!(parsed.interface_name.as_deref(), Some("ovpn42"));
        assert_eq!(parsed.healthy, Some(false));
        assert_eq!(
            parsed.metadata["runtime"]["summary"]["status"],
            "adapter_unhealthy"
        );
    }
}
