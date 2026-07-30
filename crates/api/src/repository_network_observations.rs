use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{postgres::PgRow, types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{payload_hash, CommandOutput, JobCommand, OutputStream, TunnelPlan};

use crate::{
    model::{NetworkObservationTrendView, NetworkObservationView, TunnelPlanView},
    repository::Repository,
    util::compare_timestamps_desc,
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
                    compare_timestamps_desc(&left.observed_at, &right.observed_at)
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
                        plan_id,
                        topology_identity_hash,
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
                    .map(|row| network_observation_from_row(row).map_err(Into::into))
                    .collect()
            }
        }
    }

    pub(crate) async fn list_network_observations_for_plans_since(
        &self,
        plan_ids: &[Uuid],
        since_unix: i64,
        probe_limit_per_plan: usize,
        speed_limit_per_plan: usize,
    ) -> Result<Vec<NetworkObservationView>> {
        if plan_ids.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            Self::Memory(memory) => {
                let plan_ids = plan_ids.iter().copied().collect::<HashSet<_>>();
                let mut observations = memory
                    .network_observations
                    .read()
                    .await
                    .iter()
                    .filter(|observation| {
                        observation
                            .plan_id
                            .is_some_and(|plan_id| plan_ids.contains(&plan_id))
                            && matches!(
                                observation.kind.as_str(),
                                "network_probe" | "network_speed_test"
                            )
                            && observation_timestamp_unix(&observation.observed_at)
                                .is_some_and(|observed_at| observed_at >= since_unix)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                observations.sort_by(|left, right| {
                    compare_timestamps_desc(&left.observed_at, &right.observed_at)
                        .then_with(|| right.id.cmp(&left.id))
                });
                let mut counts = HashMap::<(Uuid, String), usize>::new();
                observations.retain(|observation| {
                    let Some(plan_id) = observation.plan_id else {
                        return false;
                    };
                    let limit = if observation.kind == "network_probe" {
                        probe_limit_per_plan
                    } else {
                        speed_limit_per_plan
                    };
                    let count = counts
                        .entry((plan_id, observation.kind.clone()))
                        .or_default();
                    let keep = *count < limit;
                    *count = count.saturating_add(1);
                    keep
                });
                Ok(observations)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH ranked AS (
                    SELECT
                        id,
                        job_id,
                        client_id,
                        seq,
                        kind,
                        role,
                        plan_id,
                        topology_identity_hash,
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
                        observed_at,
                        row_number() OVER (
                            PARTITION BY plan_id, kind
                            ORDER BY observed_at DESC, id DESC
                        ) AS sample_rank
                    FROM network_observations
                    WHERE plan_id = ANY($1::uuid[])
                      AND observed_at >= to_timestamp($2)
                      AND kind IN ('network_probe', 'network_speed_test')
                    )
                    SELECT
                        id,
                        job_id,
                        client_id,
                        seq,
                        kind,
                        role,
                        plan_id,
                        topology_identity_hash,
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
                    FROM ranked
                    WHERE
                        (kind = 'network_probe' AND sample_rank <= $3)
                        OR (kind = 'network_speed_test' AND sample_rank <= $4)
                    ORDER BY observed_at DESC, id DESC
                    "#,
                )
                .bind(plan_ids)
                .bind(since_unix)
                .bind(probe_limit_per_plan as i64)
                .bind(speed_limit_per_plan as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| network_observation_from_row(row).map_err(Into::into))
                    .collect()
            }
        }
    }

    pub(crate) async fn list_network_observations_for_topology(
        &self,
        plan_topologies: &[(Uuid, String, String, String)],
        sample_limit_per_plan_kind: usize,
    ) -> Result<Vec<NetworkObservationView>> {
        if plan_topologies.is_empty() {
            return Ok(Vec::new());
        }
        let sample_limit_per_plan_kind = sample_limit_per_plan_kind.max(1);
        match self {
            Self::Memory(memory) => {
                let plan_topologies = plan_topologies
                    .iter()
                    .map(|(plan_id, identity, left_client_id, right_client_id)| {
                        (
                            *plan_id,
                            (
                                identity.as_str(),
                                left_client_id.as_str(),
                                right_client_id.as_str(),
                            ),
                        )
                    })
                    .collect::<HashMap<_, _>>();
                let stored = memory.network_observations.read().await;
                let mut probe_samples = HashMap::<Uuid, Vec<&NetworkObservationView>>::new();
                let mut speed_samples = HashMap::<Uuid, Vec<&NetworkObservationView>>::new();
                let mut latest_status = HashMap::<(Uuid, String), &NetworkObservationView>::new();
                for observation in stored.iter() {
                    let Some(plan_id) = observation.plan_id else {
                        continue;
                    };
                    let Some((identity, left_client_id, right_client_id)) =
                        plan_topologies.get(&plan_id)
                    else {
                        continue;
                    };
                    if observation.topology_identity_hash.as_deref() != Some(*identity) {
                        continue;
                    }
                    match observation.kind.as_str() {
                        "network_probe" => retain_recent_observation(
                            probe_samples.entry(plan_id).or_default(),
                            observation,
                            sample_limit_per_plan_kind,
                        ),
                        "network_speed_test" => retain_recent_observation(
                            speed_samples.entry(plan_id).or_default(),
                            observation,
                            sample_limit_per_plan_kind,
                        ),
                        "network_status"
                            if observation.client_id == *left_client_id
                                || observation.client_id == *right_client_id =>
                        {
                            let key = (plan_id, observation.client_id.clone());
                            match latest_status.entry(key) {
                                std::collections::hash_map::Entry::Occupied(mut current)
                                    if compare_network_observations_desc(
                                        observation,
                                        current.get(),
                                    )
                                    .is_lt() =>
                                {
                                    current.insert(observation);
                                }
                                std::collections::hash_map::Entry::Vacant(current) => {
                                    current.insert(observation);
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                }
                let mut observations = probe_samples
                    .into_values()
                    .chain(speed_samples.into_values())
                    .flatten()
                    .chain(latest_status.into_values())
                    .cloned()
                    .collect::<Vec<_>>();
                observations.sort_by(compare_network_observations_desc);
                Ok(observations)
            }
            Self::Postgres(pool) => {
                let plan_ids = plan_topologies
                    .iter()
                    .map(|(plan_id, _, _, _)| *plan_id)
                    .collect::<Vec<_>>();
                let topology_identities = plan_topologies
                    .iter()
                    .map(|(_, identity, _, _)| identity.as_str())
                    .collect::<Vec<_>>();
                let left_client_ids = plan_topologies
                    .iter()
                    .map(|(_, _, left_client_id, _)| left_client_id.as_str())
                    .collect::<Vec<_>>();
                let right_client_ids = plan_topologies
                    .iter()
                    .map(|(_, _, _, right_client_id)| right_client_id.as_str())
                    .collect::<Vec<_>>();
                let rows = sqlx::query(
                    r#"
                    WITH requested AS (
                        SELECT *
                        FROM unnest($1::uuid[], $2::text[], $3::text[], $4::text[])
                            AS requested(
                                plan_id,
                                topology_identity_hash,
                                left_client_id,
                                right_client_id
                            )
                    ),
                    measurement_kinds(kind) AS (
                        VALUES ('network_probe'::text), ('network_speed_test'::text)
                    ),
                    recent_measurements AS (
                        SELECT observation.*
                        FROM requested
                        CROSS JOIN measurement_kinds
                        CROSS JOIN LATERAL (
                            SELECT observation.*
                            FROM network_observations observation
                            WHERE observation.plan_id = requested.plan_id
                              AND observation.topology_identity_hash
                                  = requested.topology_identity_hash
                              AND observation.kind = measurement_kinds.kind
                            ORDER BY observation.observed_at DESC, observation.id DESC
                            LIMIT $5
                        ) observation
                    ),
                    requested_endpoints AS (
                        SELECT
                            plan_id,
                            topology_identity_hash,
                            left_client_id AS client_id
                        FROM requested
                        UNION
                        SELECT
                            plan_id,
                            topology_identity_hash,
                            right_client_id AS client_id
                        FROM requested
                    ),
                    latest_status AS (
                        SELECT observation.*
                        FROM requested_endpoints endpoint
                        CROSS JOIN LATERAL (
                            SELECT observation.*
                            FROM network_observations observation
                            WHERE observation.plan_id = endpoint.plan_id
                              AND observation.topology_identity_hash
                                  = endpoint.topology_identity_hash
                              AND observation.kind = 'network_status'
                              AND observation.client_id = endpoint.client_id
                            ORDER BY observation.observed_at DESC, observation.id DESC
                            LIMIT 1
                        ) observation
                    ),
                    selected AS (
                        SELECT * FROM recent_measurements
                        UNION ALL
                        SELECT * FROM latest_status
                    )
                    SELECT
                        id,
                        job_id,
                        client_id,
                        seq,
                        kind,
                        role,
                        plan_id,
                        topology_identity_hash,
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
                    FROM selected
                    ORDER BY observed_at DESC, id DESC
                    "#,
                )
                .bind(&plan_ids)
                .bind(&topology_identities)
                .bind(&left_client_ids)
                .bind(&right_client_ids)
                .bind(sample_limit_per_plan_kind as i64)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| network_observation_from_row(row).map_err(Into::into))
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
                    compare_timestamps_desc(&left.latest_observed_at, &right.latest_observed_at)
                        .then_with(|| left.kind.cmp(&right.kind))
                        .then_with(|| left.client_id.cmp(&right.client_id))
                });
                Ok(trends.into_iter().take(limit as usize).collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        kind,
                        plan_id,
                        topology_identity_hash,
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
                    GROUP BY kind, plan_id, topology_identity_hash, plan_name, interface_name, client_id, peer_client_id
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
                            plan_id: row.try_get("plan_id")?,
                            topology_identity_hash: row.try_get("topology_identity_hash")?,
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
    async fn bind_network_observations_to_current_topology(
        &self,
        observations: &mut [NetworkObservationView],
    ) -> Result<()> {
        let plans = self.list_tunnel_plans().await?;
        for observation in observations {
            let Some(plan) = plans
                .iter()
                .find(|plan| observation_matches_plan(observation, plan))
            else {
                continue;
            };
            observation.plan_id = Some(plan.id);
            observation.topology_identity_hash = Some(topology_identity_hash_for_plan(plan));
        }
        Ok(())
    }

    async fn bind_network_observations_to_job_snapshot(
        &self,
        job_id: Uuid,
        observations: &mut Vec<NetworkObservationView>,
    ) -> Result<bool> {
        let Some(context) = self.get_job_completion_context(job_id).await? else {
            return Ok(false);
        };
        let (plan_id, plan) = match &context.operation {
            JobCommand::NetworkStatus { plan_id, plan, .. }
            | JobCommand::NetworkProbe { plan_id, plan, .. }
            | JobCommand::NetworkSpeedTest { plan_id, plan, .. } => {
                (Uuid::parse_str(plan_id)?, plan.as_ref())
            }
            _ => return Ok(false),
        };
        observations.retain(|observation| observation_matches_declared_plan(observation, plan));
        let topology_identity_hash = topology_identity_hash_for_snapshot(plan_id, plan);
        for observation in observations {
            observation.plan_id = Some(plan_id);
            observation.topology_identity_hash = Some(topology_identity_hash.clone());
        }
        Ok(true)
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
        let observed_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let mut observations = outputs
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
        let bound_to_job = self
            .bind_network_observations_to_job_snapshot(job_id, &mut observations)
            .await?;
        #[cfg(test)]
        if !bound_to_job {
            self.bind_network_observations_to_current_topology(&mut observations)
                .await?;
        }
        #[cfg(not(test))]
        if !bound_to_job {
            return Ok(());
        }
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
                            plan_id,
                            topology_identity_hash,
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
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)
                        ON CONFLICT (job_id, client_id, seq)
                        DO UPDATE SET
                            kind = EXCLUDED.kind,
                            role = EXCLUDED.role,
                            plan_id = EXCLUDED.plan_id,
                            topology_identity_hash = EXCLUDED.topology_identity_hash,
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
                    .bind(observation.plan_id)
                    .bind(&observation.topology_identity_hash)
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

fn network_observation_from_row(row: PgRow) -> Result<NetworkObservationView, sqlx::Error> {
    let metadata: SqlJson<serde_json::Value> = row.try_get("metadata")?;
    Ok(NetworkObservationView {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        client_id: row.try_get("client_id")?,
        seq: row.try_get("seq")?,
        kind: row.try_get("kind")?,
        role: row.try_get("role")?,
        plan_id: row.try_get("plan_id")?,
        topology_identity_hash: row.try_get("topology_identity_hash")?,
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
}

fn observation_timestamp_unix(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().or_else(|| {
        DateTime::parse_from_rfc3339(value)
            .or_else(|_| DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f%#z"))
            .ok()
            .map(|value| value.with_timezone(&Utc).timestamp())
    })
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct TrendKey {
    kind: String,
    plan_id: Option<Uuid>,
    topology_identity_hash: Option<String>,
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
                plan_id: observation.plan_id,
                topology_identity_hash: observation.topology_identity_hash.clone(),
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
        if compare_timestamps_desc(&observation.observed_at, &self.latest_observed_at).is_lt() {
            self.latest_observed_at = observation.observed_at.clone();
        }
    }

    fn into_view(self) -> NetworkObservationTrendView {
        NetworkObservationTrendView {
            kind: self.key.kind,
            plan_id: self.key.plan_id,
            topology_identity_hash: self.key.topology_identity_hash,
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

pub(crate) fn summarize_network_observation_trends(
    observations: &[NetworkObservationView],
) -> Vec<NetworkObservationTrendView> {
    let mut groups: HashMap<TrendKey, TrendAccumulator> = HashMap::new();
    for observation in observations {
        let key = TrendKey {
            kind: observation.kind.clone(),
            plan_id: observation.plan_id,
            topology_identity_hash: observation.topology_identity_hash.clone(),
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

fn compare_network_observations_desc(
    left: &NetworkObservationView,
    right: &NetworkObservationView,
) -> std::cmp::Ordering {
    compare_timestamps_desc(&left.observed_at, &right.observed_at)
        .then_with(|| right.id.cmp(&left.id))
}

fn retain_recent_observation<'a>(
    selected: &mut Vec<&'a NetworkObservationView>,
    candidate: &'a NetworkObservationView,
    limit: usize,
) {
    if selected.len() < limit {
        selected.push(candidate);
        return;
    }
    let Some((oldest_index, oldest)) = selected
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| compare_network_observations_desc(left, right))
    else {
        return;
    };
    if compare_network_observations_desc(candidate, oldest).is_lt() {
        selected[oldest_index] = candidate;
    }
}

fn average(sum: f64, count: i64) -> Option<f64> {
    if count > 0 {
        Some(sum / count as f64)
    } else {
        None
    }
}

pub(crate) fn topology_identity_hash_for_plan(plan: &TunnelPlanView) -> String {
    topology_identity_hash_for_snapshot(plan.id, &plan.plan)
}

fn topology_identity_hash_for_snapshot(plan_id: Uuid, plan: &TunnelPlan) -> String {
    let payload = serde_json::to_vec(&serde_json::json!({
        "plan_id": plan_id.to_string(),
        "name": &plan.name,
        "kind": format!("{:?}", plan.kind),
        "left_client_id": &plan.left_client_id,
        "right_client_id": &plan.right_client_id,
        "interface_name": &plan.interface_name,
        "left_tunnel_address": &plan.left_tunnel_address,
        "right_tunnel_address": &plan.right_tunnel_address,
        "ipv4_tunnel": &plan.ipv4_tunnel,
        "ipv6_tunnel": &plan.ipv6_tunnel,
        "latency_primary_family": format!("{:?}", plan.latency_primary_family),
    }))
    .expect("topology identity payload serializes");
    payload_hash(&payload)
}

#[cfg(test)]
fn observation_matches_plan(observation: &NetworkObservationView, plan: &TunnelPlanView) -> bool {
    observation_matches_declared_plan(observation, &plan.plan)
}

fn observation_matches_declared_plan(
    observation: &NetworkObservationView,
    plan: &TunnelPlan,
) -> bool {
    if observation.plan_name.as_deref() != Some(plan.name.as_str()) {
        return false;
    }
    if observation.interface_name.as_deref() != Some(plan.interface_name.as_str()) {
        return false;
    }
    match (
        observation.client_id.as_str(),
        observation.peer_client_id.as_deref(),
    ) {
        (client_id, Some(peer_client_id))
            if client_id == plan.left_client_id.as_str()
                && peer_client_id == plan.right_client_id.as_str() =>
        {
            true
        }
        (client_id, Some(peer_client_id))
            if client_id == plan.right_client_id.as_str()
                && peer_client_id == plan.left_client_id.as_str() =>
        {
            true
        }
        _ => false,
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
    let runtime_health = runtime_summary.get("healthy").and_then(as_bool);
    Some(NetworkObservationView {
        id: Uuid::new_v4(),
        job_id,
        client_id: client_id.to_string(),
        seq,
        kind,
        role: as_string(metadata.get("role")),
        plan_id: None,
        topology_identity_hash: None,
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
        } else {
            parsed
                .get("healthy")
                .and_then(as_bool)
                .or_else(|| metadata.get("success").and_then(as_bool))
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
    fn parses_network_status_runtime_summary_and_adapter_evidence() {
        let job_id = Uuid::new_v4();
        let status = CommandOutput {
            job_id,
            stream: OutputStream::Status,
            data: serde_json::to_vec(&serde_json::json!({
                "type": "network_status",
                "plan": "external-edge",
                "interface": "ovpn42",
                "peer_client_id": "right",
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
