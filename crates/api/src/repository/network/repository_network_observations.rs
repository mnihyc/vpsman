use std::collections::{HashMap, HashSet};

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{postgres::PgRow, types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    tunnel_topology_identity_hash, CommandOutput, JobCommand, OutputStream,
    TunnelReachabilityObservation, TunnelReachabilitySource,
};

use crate::{
    internal_operator::persisted_actor_id,
    model::{
        AuditLogView, AuthContext, NetworkObservationTrendView, NetworkObservationView,
        TunnelPlanEvidenceClearResult, TunnelPlanView,
    },
    repository::Repository,
    repository_network::network_audit_metadata,
    unix_now,
    util::compare_timestamps_desc,
};

const OBSERVATION_COLUMNS: &str = r#"
    id, job_id, client_id, seq, kind, source, role, plan_id,
    topology_identity_hash, plan_name, interface_name, peer_client_id,
    target, endpoint_side, address_family, stale_after_secs, healthy,
    transmitted, received, latency_min_ms, latency_avg_ms, latency_max_ms,
    latency_mdev_ms, packet_loss_ratio, reason, throughput_mbps, bytes,
    metadata, observed_at::text AS observed_at,
    received_at::text AS received_at
"#;

#[derive(Clone, Debug)]
pub(crate) struct NetworkObservationFilter {
    pub(crate) start_unix: i64,
    pub(crate) end_unix: i64,
    pub(crate) plan_ids: Vec<Uuid>,
    pub(crate) client_id: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) health: Option<String>,
    pub(crate) search: Option<String>,
    pub(crate) limit: i64,
    pub(crate) visible_only: bool,
}

impl NetworkObservationFilter {
    fn matches(&self, observation: &NetworkObservationView) -> bool {
        let Some(observed) = observation_timestamp_unix(&observation.observed_at) else {
            return false;
        };
        if observed < self.start_unix || observed > self.end_unix {
            return false;
        }
        if !self.plan_ids.is_empty()
            && !observation
                .plan_id
                .is_some_and(|plan_id| self.plan_ids.contains(&plan_id))
        {
            return false;
        }
        if self.client_id.as_deref().is_some_and(|client_id| {
            observation.client_id != client_id
                && observation.peer_client_id.as_deref() != Some(client_id)
        }) {
            return false;
        }
        if self
            .source
            .as_deref()
            .is_some_and(|source| observation.source != source)
        {
            return false;
        }
        if self
            .kind
            .as_deref()
            .is_some_and(|kind| observation.kind != kind)
        {
            return false;
        }
        if self.health.as_deref().is_some_and(|health| match health {
            "healthy" => observation.healthy != Some(true),
            "unhealthy" => observation.healthy != Some(false),
            "unknown" => observation.healthy.is_some(),
            _ => false,
        }) {
            return false;
        }
        if let Some(search) = self.search.as_deref() {
            let search = search.to_ascii_lowercase();
            let haystack = [
                Some(observation.client_id.as_str()),
                observation.peer_client_id.as_deref(),
                observation.plan_name.as_deref(),
                observation.interface_name.as_deref(),
                observation.target.as_deref(),
                observation.reason.as_deref(),
                Some(observation.kind.as_str()),
                Some(observation.source.as_str()),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
            if !haystack.contains(&search) {
                return false;
            }
        }
        true
    }
}

impl Repository {
    pub(crate) async fn clear_tunnel_plan_evidence(
        &self,
        targets: &[(Uuid, i64)],
        operator: &AuthContext,
    ) -> Result<Vec<TunnelPlanEvidenceClearResult>> {
        anyhow::ensure!(!targets.is_empty(), "tunnel_plan_evidence_targets_required");
        anyhow::ensure!(
            targets.len() <= 1_000,
            "tunnel_plan_evidence_target_limit_exceeded"
        );
        let selected_ids = targets
            .iter()
            .map(|(plan_id, _)| *plan_id)
            .collect::<HashSet<_>>();
        anyhow::ensure!(
            selected_ids.len() == targets.len(),
            "tunnel_plan_evidence_target_duplicate"
        );

        match self {
            Self::Memory(memory) => {
                let plans = memory.tunnel_plans.read().await;
                let mut reviewed_plans = HashMap::new();
                for (plan_id, expected_revision) in targets {
                    let plan = plans
                        .iter()
                        .find(|plan| plan.id == *plan_id && plan.deleted_at.is_none())
                        .ok_or_else(|| anyhow::anyhow!("tunnel_plan_not_found"))?;
                    anyhow::ensure!(
                        plan.revision == *expected_revision,
                        "tunnel_plan_snapshot_stale"
                    );
                    reviewed_plans.insert(*plan_id, (plan.name.clone(), plan.revision));
                }

                let mut cleared_by_plan = targets
                    .iter()
                    .map(|(plan_id, _)| (*plan_id, 0_u64))
                    .collect::<HashMap<_, _>>();
                let mut observations = memory.network_observations.write().await;
                observations.retain(|observation| {
                    let Some(plan_id) = observation
                        .plan_id
                        .filter(|plan_id| selected_ids.contains(plan_id))
                    else {
                        return true;
                    };
                    *cleared_by_plan
                        .get_mut(&plan_id)
                        .expect("selected observation plan has a clear counter") += 1;
                    false
                });
                drop(observations);

                let results = targets
                    .iter()
                    .map(|(plan_id, _)| {
                        let (name, reviewed_revision) = &reviewed_plans[plan_id];
                        TunnelPlanEvidenceClearResult {
                            plan_id: *plan_id,
                            name: name.clone(),
                            reviewed_revision: *reviewed_revision,
                            cleared_observation_count: cleared_by_plan[plan_id],
                        }
                    })
                    .collect::<Vec<_>>();
                memory
                    .audits
                    .write()
                    .await
                    .push(tunnel_plan_evidence_clear_audit(&results, operator));
                Ok(results)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let plan_ids = targets
                    .iter()
                    .map(|(plan_id, _)| *plan_id)
                    .collect::<Vec<_>>();
                let locked = sqlx::query(
                    r#"
                    SELECT id, name, revision
                    FROM tunnel_plans
                    WHERE id = ANY($1::uuid[])
                      AND deleted_at IS NULL
                    ORDER BY id
                    FOR UPDATE
                    "#,
                )
                .bind(&plan_ids)
                .fetch_all(&mut *tx)
                .await?;
                anyhow::ensure!(locked.len() == targets.len(), "tunnel_plan_not_found");
                let revisions = locked
                    .iter()
                    .map(|row| {
                        Ok((
                            row.try_get("id")?,
                            (row.try_get("name")?, row.try_get("revision")?),
                        ))
                    })
                    .collect::<Result<HashMap<Uuid, (String, i64)>>>()?;
                for (plan_id, expected_revision) in targets {
                    anyhow::ensure!(
                        revisions
                            .get(plan_id)
                            .is_some_and(|(_, revision)| revision == expected_revision),
                        "tunnel_plan_snapshot_stale"
                    );
                }

                let retained_rows = sqlx::query(
                    r#"
                    SELECT series.plan_id,
                           LEAST(
                               COALESCE(SUM(rollup.sample_count), 0),
                               9223372036854775807::numeric
                           )::bigint AS cleared_count
                    FROM network_observation_rollups rollup
                    JOIN network_observation_series series ON series.id = rollup.series_id
                    WHERE series.plan_id = ANY($1::uuid[])
                    GROUP BY series.plan_id
                    "#,
                )
                .bind(&plan_ids)
                .fetch_all(&mut *tx)
                .await?;
                let rows = sqlx::query(
                    r#"
                    WITH deleted AS (
                        DELETE FROM network_observations
                        WHERE plan_id = ANY($1::uuid[])
                        RETURNING plan_id
                    )
                    SELECT plan_id, COUNT(*)::bigint AS cleared_count
                    FROM deleted
                    GROUP BY plan_id
                    "#,
                )
                .bind(&plan_ids)
                .fetch_all(&mut *tx)
                .await?;
                sqlx::query(
                    "DELETE FROM network_observation_series WHERE plan_id = ANY($1::uuid[])",
                )
                .bind(&plan_ids)
                .execute(&mut *tx)
                .await?;
                let mut cleared_by_plan = HashMap::<Uuid, u64>::new();
                for row in rows {
                    let count = row.try_get::<i64, _>("cleared_count")?;
                    cleared_by_plan.insert(row.try_get("plan_id")?, u64::try_from(count)?);
                }
                for row in retained_rows {
                    let count = u64::try_from(row.try_get::<i64, _>("cleared_count")?)?;
                    let plan_id = row.try_get("plan_id")?;
                    let total = cleared_by_plan.entry(plan_id).or_default();
                    *total = total.saturating_add(count);
                }
                let results = targets
                    .iter()
                    .map(|(plan_id, _)| {
                        let (name, reviewed_revision) = &revisions[plan_id];
                        TunnelPlanEvidenceClearResult {
                            plan_id: *plan_id,
                            name: name.clone(),
                            reviewed_revision: *reviewed_revision,
                            cleared_observation_count: *cleared_by_plan.get(plan_id).unwrap_or(&0),
                        }
                    })
                    .collect::<Vec<_>>();
                let audit = tunnel_plan_evidence_clear_audit(&results, operator);
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(audit.id)
                .bind(audit.actor_id)
                .bind(&audit.action)
                .bind(&audit.target)
                .bind(&audit.metadata)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(results)
            }
        }
    }

    pub(crate) async fn list_network_observations(
        &self,
        limit: i64,
        visible_only: bool,
    ) -> Result<Vec<NetworkObservationView>> {
        self.list_network_observations_filtered_with_mode(
            &NetworkObservationFilter {
                start_unix: 0,
                end_unix: Utc::now().timestamp(),
                plan_ids: Vec::new(),
                client_id: None,
                source: None,
                kind: None,
                health: None,
                search: None,
                limit,
                visible_only,
            },
            false,
        )
        .await
    }

    pub(crate) async fn list_network_observations_filtered(
        &self,
        filter: &NetworkObservationFilter,
    ) -> Result<Vec<NetworkObservationView>> {
        self.list_network_observations_filtered_with_mode(filter, true)
            .await
    }

    async fn list_network_observations_filtered_with_mode(
        &self,
        filter: &NetworkObservationFilter,
        fair_per_series: bool,
    ) -> Result<Vec<NetworkObservationView>> {
        const MAX_FAIR_RESPONSE_ROWS: i64 = 250_000;
        match self {
            Self::Memory(memory) => {
                let suspended_clients = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| agent.status == "suspended")
                    .map(|agent| agent.id.clone())
                    .collect::<HashSet<_>>();
                let hidden = memory.hidden_clients.read().await;
                let active_plan_ids = if filter.visible_only {
                    memory
                        .tunnel_plans
                        .read()
                        .await
                        .iter()
                        .filter(|plan| {
                            plan.deleted_at.is_none()
                                && !suspended_clients.contains(&plan.left_client_id)
                                && !suspended_clients.contains(&plan.right_client_id)
                        })
                        .map(|plan| plan.id)
                        .collect::<HashSet<_>>()
                } else {
                    HashSet::new()
                };
                let mut rows = memory
                    .network_observations
                    .read()
                    .await
                    .iter()
                    .filter(|observation| {
                        (!filter.visible_only
                            || (!hidden.contains(&observation.client_id)
                                && !suspended_clients.contains(&observation.client_id)
                                && observation.peer_client_id.as_ref().is_none_or(|peer| {
                                    !hidden.contains(peer) && !suspended_clients.contains(peer)
                                })
                                && observation
                                    .plan_id
                                    .is_some_and(|plan_id| active_plan_ids.contains(&plan_id))))
                            && filter.matches(observation)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(compare_network_observations_desc);
                if fair_per_series {
                    Ok(limit_observations_fairly(
                        rows,
                        filter.limit.max(1) as usize,
                        MAX_FAIR_RESPONSE_ROWS as usize,
                    ))
                } else {
                    rows.truncate(filter.limit.max(1) as usize);
                    Ok(rows)
                }
            }
            Self::Postgres(pool) if fair_per_series => {
                let rows = sqlx::query(&format!(
                    r#"
                    WITH ranked AS (
                        SELECT observation.*,
                            row_number() OVER (
                                PARTITION BY observation.plan_id,
                                    observation.topology_identity_hash,
                                    observation.kind,
                                    COALESCE(observation.endpoint_side, observation.client_id)
                                ORDER BY observation.observed_at DESC, observation.id DESC
                            ) AS evidence_rank
                        FROM network_observation_exact_evidence observation
                        WHERE observation.observed_at >= to_timestamp($1)
                          AND observation.observed_at <= to_timestamp($2)
                          AND (cardinality($3::uuid[]) = 0 OR observation.plan_id = ANY($3::uuid[]))
                          AND ($4::text IS NULL OR observation.client_id = $4 OR observation.peer_client_id = $4)
                          AND ($5::text IS NULL OR observation.source = $5)
                          AND ($6::text IS NULL OR observation.kind = $6)
                          AND (
                            $7::text IS NULL
                            OR ($7 = 'healthy' AND observation.healthy IS TRUE)
                            OR ($7 = 'unhealthy' AND observation.healthy IS FALSE)
                            OR ($7 = 'unknown' AND observation.healthy IS NULL)
                          )
                          AND (
                            $8::text IS NULL
                            OR concat_ws(' ', observation.client_id, observation.peer_client_id,
                                observation.plan_name, observation.interface_name, observation.target,
                                observation.reason, observation.kind, observation.source) ILIKE '%' || $8 || '%'
                          )
                          AND (
                            NOT $9
                            OR (
                                EXISTS (SELECT 1 FROM visible_clients WHERE id = observation.client_id AND status <> 'suspended')
                                AND (observation.peer_client_id IS NULL OR EXISTS (
                                    SELECT 1 FROM visible_clients WHERE id = observation.peer_client_id AND status <> 'suspended'
                                ))
                                AND EXISTS (
                                    SELECT 1
                                    FROM tunnel_plans plan
                                    WHERE plan.id = observation.plan_id
                                      AND plan.deleted_at IS NULL
                                      AND NOT EXISTS (
                                          SELECT 1 FROM visible_clients endpoint
                                          WHERE endpoint.id=plan.left_client_id
                                            AND endpoint.status = 'suspended'
                                      )
                                      AND NOT EXISTS (
                                          SELECT 1 FROM visible_clients endpoint
                                          WHERE endpoint.id=plan.right_client_id
                                            AND endpoint.status = 'suspended'
                                      )
                                )
                            )
                          )
                    )
                    SELECT {OBSERVATION_COLUMNS}
                    FROM ranked
                    WHERE evidence_rank <= $10
                    ORDER BY evidence_rank, observed_at DESC, id DESC
                    LIMIT $11
                    "#
                ))
                .bind(filter.start_unix)
                .bind(filter.end_unix)
                .bind(&filter.plan_ids)
                .bind(filter.client_id.as_deref())
                .bind(filter.source.as_deref())
                .bind(filter.kind.as_deref())
                .bind(filter.health.as_deref())
                .bind(filter.search.as_deref())
                .bind(filter.visible_only)
                .bind(filter.limit.max(1))
                .bind(MAX_FAIR_RESPONSE_ROWS)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| network_observation_from_row(row).map_err(Into::into))
                    .collect()
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT {OBSERVATION_COLUMNS}
                    FROM network_observation_exact_evidence observation
                    WHERE observation.observed_at >= to_timestamp($1)
                      AND observation.observed_at <= to_timestamp($2)
                      AND (cardinality($3::uuid[]) = 0 OR observation.plan_id = ANY($3::uuid[]))
                      AND ($4::text IS NULL OR observation.client_id = $4 OR observation.peer_client_id = $4)
                      AND ($5::text IS NULL OR observation.source = $5)
                      AND ($6::text IS NULL OR observation.kind = $6)
                      AND (
                        $7::text IS NULL
                        OR ($7 = 'healthy' AND observation.healthy IS TRUE)
                        OR ($7 = 'unhealthy' AND observation.healthy IS FALSE)
                        OR ($7 = 'unknown' AND observation.healthy IS NULL)
                      )
                      AND (
                        $8::text IS NULL
                        OR concat_ws(' ', observation.client_id, observation.peer_client_id,
                            observation.plan_name, observation.interface_name, observation.target,
                            observation.reason, observation.kind, observation.source) ILIKE '%' || $8 || '%'
                      )
                      AND (
                        NOT $9
                        OR (
                            EXISTS (SELECT 1 FROM visible_clients WHERE id = observation.client_id AND status <> 'suspended')
                            AND (observation.peer_client_id IS NULL OR EXISTS (
                                SELECT 1 FROM visible_clients WHERE id = observation.peer_client_id AND status <> 'suspended'
                            ))
                            AND EXISTS (
                                SELECT 1
                                FROM tunnel_plans plan
                                WHERE plan.id = observation.plan_id
                                  AND plan.deleted_at IS NULL
                                  AND NOT EXISTS (
                                      SELECT 1 FROM visible_clients endpoint
                                      WHERE endpoint.id=plan.left_client_id
                                        AND endpoint.status = 'suspended'
                                  )
                                  AND NOT EXISTS (
                                      SELECT 1 FROM visible_clients endpoint
                                      WHERE endpoint.id=plan.right_client_id
                                        AND endpoint.status = 'suspended'
                                  )
                            )
                        )
                      )
                    ORDER BY observation.observed_at DESC, observation.id DESC
                    LIMIT $10
                    "#
                ))
                .bind(filter.start_unix)
                .bind(filter.end_unix)
                .bind(&filter.plan_ids)
                .bind(filter.client_id.as_deref())
                .bind(filter.source.as_deref())
                .bind(filter.kind.as_deref())
                .bind(filter.health.as_deref())
                .bind(filter.search.as_deref())
                .bind(filter.visible_only)
                .bind(filter.limit.max(1))
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
        start_unix: i64,
        end_unix: i64,
        sample_limit_per_plan_kind_endpoint: usize,
    ) -> Result<Vec<NetworkObservationView>> {
        if plan_topologies.is_empty() {
            return Ok(Vec::new());
        }
        let identities = plan_topologies
            .iter()
            .map(|(plan_id, identity, _, _)| (*plan_id, identity.as_str()))
            .collect::<HashMap<_, _>>();
        let plan_ids = plan_topologies
            .iter()
            .map(|value| value.0)
            .collect::<Vec<_>>();
        let limit = sample_limit_per_plan_kind_endpoint.max(1);
        let mut rows = match self {
            Self::Memory(memory) => {
                let suspended_clients = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| agent.status == "suspended")
                    .map(|agent| agent.id.clone())
                    .collect::<HashSet<_>>();
                let eligible_plan_ids = plan_topologies
                    .iter()
                    .filter(|(_, _, left_client_id, right_client_id)| {
                        !suspended_clients.contains(left_client_id)
                            && !suspended_clients.contains(right_client_id)
                    })
                    .map(|(plan_id, _, _, _)| *plan_id)
                    .collect::<HashSet<_>>();
                let mut rows = memory
                    .network_observations
                    .read()
                    .await
                    .iter()
                    .filter(|observation| {
                        observation
                            .plan_id
                            .is_some_and(|plan_id| eligible_plan_ids.contains(&plan_id))
                            && observation_timestamp_unix(&observation.observed_at).is_some_and(
                                |observed| observed >= start_unix && observed <= end_unix,
                            )
                            && matches!(
                                observation.kind.as_str(),
                                "tunnel_reachability" | "network_speed_test" | "network_status"
                            )
                            && !suspended_clients.contains(&observation.client_id)
                            && observation
                                .peer_client_id
                                .as_ref()
                                .is_none_or(|peer| !suspended_clients.contains(peer))
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                rows.sort_by(compare_network_observations_desc);
                limit_topology_rows(rows, limit)
            }
            Self::Postgres(pool) => {
                let query = format!(
                    r#"
                    SELECT *
                    FROM (
                        SELECT {OBSERVATION_COLUMNS},
                            row_number() OVER (
                                PARTITION BY plan_id, kind, COALESCE(endpoint_side, client_id)
                                ORDER BY observed_at DESC, id DESC
                            ) AS evidence_rank
                        FROM network_observation_exact_evidence
                        WHERE observed_at >= to_timestamp($1)
                          AND observed_at <= to_timestamp($2)
                          AND plan_id = ANY($3::uuid[])
                          AND kind IN ('tunnel_reachability', 'network_speed_test', 'network_status')
                          AND NOT EXISTS (
                              SELECT 1
                              FROM visible_clients suspended_client
                              WHERE suspended_client.status = 'suspended'
                                AND (
                                    suspended_client.id = network_observation_exact_evidence.client_id
                                    OR suspended_client.id = network_observation_exact_evidence.peer_client_id
                                )
                          )
                          AND NOT EXISTS (
                              SELECT 1
                              FROM tunnel_plans plan
                              JOIN visible_clients suspended_endpoint
                                ON suspended_endpoint.status = 'suspended'
                               AND suspended_endpoint.id IN (
                                   plan.left_client_id,
                                   plan.right_client_id
                               )
                              WHERE plan.id = network_observation_exact_evidence.plan_id
                          )
                    ) ranked
                    WHERE evidence_rank <= $4
                    ORDER BY observed_at DESC, id DESC
                    "#
                );
                sqlx::query(&query)
                    .bind(start_unix)
                    .bind(end_unix)
                    .bind(&plan_ids)
                    .bind(i64::try_from(limit).unwrap_or(i64::MAX))
                    .fetch_all(pool)
                    .await?
                    .into_iter()
                    .map(|row| network_observation_from_row(row).map_err(Into::into))
                    .collect::<Result<Vec<_>>>()?
            }
        };
        rows.retain(|observation| {
            observation
                .plan_id
                .and_then(|plan_id| identities.get(&plan_id).copied())
                == observation.topology_identity_hash.as_deref()
        });
        Ok(rows)
    }

    pub(crate) async fn list_network_observation_trends(
        &self,
        limit: i64,
        visible_only: bool,
    ) -> Result<Vec<NetworkObservationTrendView>> {
        self.list_network_observation_trends_filtered(&NetworkObservationFilter {
            start_unix: 0,
            end_unix: Utc::now().timestamp(),
            plan_ids: Vec::new(),
            client_id: None,
            source: None,
            kind: None,
            health: None,
            search: None,
            limit,
            visible_only,
        })
        .await
    }

    pub(crate) async fn list_network_observation_trends_filtered(
        &self,
        filter: &NetworkObservationFilter,
    ) -> Result<Vec<NetworkObservationTrendView>> {
        let mut trends = match self {
            Self::Memory(_) => {
                let mut observations_filter = filter.clone();
                observations_filter.limit = filter.limit.max(1).saturating_mul(2_000).min(250_000);
                let observations = self
                    .list_network_observations_filtered(&observations_filter)
                    .await?;
                summarize_network_observation_trends(&observations)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(NETWORK_OBSERVATION_TRENDS_QUERY)
                    .bind(filter.start_unix)
                    .bind(filter.end_unix)
                    .bind(&filter.plan_ids)
                    .bind(filter.client_id.as_deref())
                    .bind(filter.source.as_deref())
                    .bind(filter.kind.as_deref())
                    .bind(filter.health.as_deref())
                    .bind(filter.search.as_deref())
                    .bind(filter.visible_only)
                    .bind(filter.limit.max(1))
                    .fetch_all(pool)
                    .await?;
                rows.into_iter()
                    .map(network_observation_trend_from_row)
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        trends.sort_by(|left, right| {
            compare_timestamps_desc(&left.latest_observed_at, &right.latest_observed_at)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.client_id.cmp(&right.client_id))
        });
        trends.truncate(filter.limit.max(1) as usize);
        Ok(trends)
    }

    pub(crate) async fn export_network_observation_rollups(
        &self,
        limit: i64,
    ) -> Result<Vec<serde_json::Value>> {
        match self {
            Self::Memory(_) => Ok(Vec::new()),
            Self::Postgres(pool) => sqlx::query(NETWORK_OBSERVATION_ROLLUPS_EXPORT_QUERY)
                .bind(limit.max(1))
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|row| {
                    row.try_get::<SqlJson<serde_json::Value>, _>("record")
                        .map(|value| value.0)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(Into::into),
        }
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
        let identity = tunnel_topology_identity_hash(plan_id, plan);
        for observation in observations {
            observation.plan_id = Some(plan_id);
            observation.topology_identity_hash = Some(identity.clone());
            observation.endpoint_side = Some(
                if observation.client_id == plan.left_client_id {
                    "left"
                } else {
                    "right"
                }
                .to_string(),
            );
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

    #[cfg(test)]
    pub(crate) async fn record_network_observations_starting_at(
        &self,
        job_id: Uuid,
        client_id: &str,
        start_seq: i32,
        outputs: &[CommandOutput],
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let mut observations = outputs
            .iter()
            .enumerate()
            .filter_map(|(offset, output)| {
                let seq = start_seq.checked_add(i32::try_from(offset).ok()?)?;
                parse_network_observation(job_id, client_id, seq, output, &now)
            })
            .collect::<Vec<_>>();
        self.record_bound_manual_network_observations(job_id, &mut observations)
            .await
    }

    pub(crate) async fn record_persisted_network_observations(
        &self,
        job_id: Uuid,
        client_id: &str,
        outputs: &[(i32, CommandOutput, String)],
    ) -> Result<()> {
        let mut observations = outputs
            .iter()
            .filter_map(|(seq, output, received_at)| {
                parse_network_observation(job_id, client_id, *seq, output, received_at)
            })
            .collect::<Vec<_>>();
        self.record_bound_manual_network_observations(job_id, &mut observations)
            .await
    }

    async fn record_bound_manual_network_observations(
        &self,
        job_id: Uuid,
        observations: &mut Vec<NetworkObservationView>,
    ) -> Result<()> {
        if observations.is_empty() {
            return Ok(());
        }
        if !self
            .bind_network_observations_to_job_snapshot(job_id, observations)
            .await?
        {
            #[cfg(not(test))]
            return Ok(());
            #[cfg(test)]
            self.bind_network_observations_to_current_topology(observations)
                .await?;
        }
        self.upsert_manual_observations(std::mem::take(observations))
            .await
    }

    #[cfg(test)]
    async fn bind_network_observations_to_current_topology(
        &self,
        observations: &mut [NetworkObservationView],
    ) -> Result<()> {
        let plans = self.list_tunnel_plans().await?;
        for observation in observations {
            if let Some(plan) = plans
                .iter()
                .find(|plan| observation_matches_declared_plan(observation, &plan.plan))
            {
                observation.plan_id = Some(plan.id);
                observation.topology_identity_hash = Some(topology_identity_hash_for_plan(plan));
                observation.endpoint_side = Some(
                    if observation.client_id == plan.left_client_id {
                        "left"
                    } else {
                        "right"
                    }
                    .to_string(),
                );
            }
        }
        Ok(())
    }

    async fn upsert_manual_observations(
        &self,
        observations: Vec<NetworkObservationView>,
    ) -> Result<()> {
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
                    insert_network_observation(&mut tx, &observation, true, None).await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn record_automatic_tunnel_reachability(
        &self,
        client_id: &str,
        observations: &[TunnelReachabilityObservation],
    ) -> Result<()> {
        if observations.is_empty() {
            return Ok(());
        }
        let plans = self.list_tunnel_plans().await?;
        let plans = plans
            .iter()
            .map(|plan| (plan.id, plan))
            .collect::<HashMap<_, _>>();
        let received_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
        let mut accepted = Vec::new();
        for observation in observations {
            let Some(plan) = plans.get(&observation.plan_id).copied() else {
                continue;
            };
            let expected_identity = topology_identity_hash_for_plan(plan);
            let (expected_client, expected_peer) = match observation.endpoint_side {
                vpsman_common::TunnelEndpointSide::Left => {
                    (&plan.left_client_id, &plan.right_client_id)
                }
                vpsman_common::TunnelEndpointSide::Right => {
                    (&plan.right_client_id, &plan.left_client_id)
                }
            };
            let expected_target = expected_reachability_target(
                &plan.plan,
                observation.endpoint_side,
                observation.address_family,
            );
            if !plan.enabled
                || observation.source != TunnelReachabilitySource::Automatic
                || expected_client != client_id
                || observation.peer_client_id.as_str() != expected_peer.as_str()
                || observation.interface_name.as_str() != plan.plan.interface_name.as_str()
                || expected_target != Some(observation.target.as_str())
                || observation.topology_identity_hash != expected_identity
                || !observation.values_are_coherent()
            {
                continue;
            }
            let observed_at = DateTime::from_timestamp(observation.measured_unix as i64, 0)
                .unwrap_or_else(Utc::now)
                .to_rfc3339_opts(SecondsFormat::Micros, true);
            accepted.push(NetworkObservationView {
                id: observation.id,
                job_id: None,
                client_id: client_id.to_string(),
                seq: None,
                kind: "tunnel_reachability".to_string(),
                source: source_label(observation.source).to_string(),
                role: Some("endpoint".to_string()),
                plan_id: Some(plan.id),
                topology_identity_hash: Some(expected_identity),
                plan_name: Some(plan.name.clone()),
                interface_name: Some(observation.interface_name.clone()),
                peer_client_id: Some(observation.peer_client_id.clone()),
                target: Some(observation.target.clone()),
                endpoint_side: Some(endpoint_side_label(observation.endpoint_side).to_string()),
                address_family: Some(address_family_label(observation.address_family).to_string()),
                stale_after_secs: Some(observation.stale_after_secs as i64),
                healthy: Some(observation.healthy),
                transmitted: Some(observation.transmitted as i32),
                received: Some(observation.received as i32),
                latency_min_ms: observation.latency_min_ms,
                latency_avg_ms: observation.latency_avg_ms,
                latency_max_ms: observation.latency_max_ms,
                latency_mdev_ms: observation.latency_mdev_ms,
                packet_loss_ratio: Some(observation.packet_loss_ratio),
                reason: observation.reason.clone(),
                throughput_mbps: None,
                bytes: None,
                metadata: serde_json::json!({
                    "type": "tunnel_reachability",
                    "source": "automatic",
                }),
                observed_at,
                received_at: received_at.clone(),
            });
        }
        match self {
            Self::Memory(memory) => {
                let mut stored = memory.network_observations.write().await;
                for observation in accepted {
                    if !stored.iter().any(|existing| existing.id == observation.id) {
                        stored.push(observation);
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for observation in accepted {
                    let series_id =
                        upsert_automatic_observation_series(&mut tx, &observation).await?;
                    if insert_network_observation(&mut tx, &observation, false, Some(series_id))
                        .await?
                    {
                        upsert_latest_automatic_observation(&mut tx, series_id, &observation)
                            .await?;
                    }
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }
}

const NETWORK_OBSERVATION_TRENDS_QUERY: &str = r#"
WITH raw_evidence AS (
    SELECT
        observation.kind,
        observation.plan_id,
        observation.topology_identity_hash,
        observation.plan_name,
        observation.interface_name,
        observation.client_id,
        observation.peer_client_id,
        observation.observed_at AS bucket_start,
        NULL::integer AS bucket_secs,
        FALSE AS retained,
        1::bigint AS sample_count,
        1::bigint AS source_bucket_count,
        NULL::integer AS effective_resolution_secs,
        (observation.source = 'automatic')::integer::bigint AS automatic_count,
        (observation.source = 'manual')::integer::bigint AS manual_count,
        (observation.healthy IS TRUE)::integer::bigint AS healthy_count,
        (observation.healthy IS FALSE)::integer::bigint AS degraded_count,
        COALESCE(observation.latency_avg_ms, 0.0) AS latency_sum_ms,
        (observation.latency_avg_ms IS NOT NULL)::integer::bigint AS latency_sample_count,
        observation.latency_avg_ms AS latency_min_ms,
        observation.latency_avg_ms AS latency_max_ms,
        COALESCE(observation.packet_loss_ratio, 0.0) AS packet_loss_sum_ratio,
        (observation.packet_loss_ratio IS NOT NULL)::integer::bigint
            AS packet_loss_sample_count,
        COALESCE(observation.throughput_mbps, 0.0) AS throughput_sum_mbps,
        (observation.throughput_mbps IS NOT NULL)::integer::bigint
            AS throughput_sample_count,
        observation.throughput_mbps AS throughput_max_mbps,
        COALESCE(observation.bytes, 0)::numeric AS bytes_total,
        observation.observed_at AS latest_observed_at
    FROM network_observations observation
    WHERE observation.observed_at >= to_timestamp($1)
      AND observation.observed_at <= to_timestamp($2)
      AND (cardinality($3::uuid[]) = 0 OR observation.plan_id = ANY($3::uuid[]))
      AND ($4::text IS NULL
           OR observation.client_id = $4
           OR observation.peer_client_id = $4)
      AND ($5::text IS NULL OR observation.source = $5)
      AND ($6::text IS NULL OR observation.kind = $6)
      AND (
        $7::text IS NULL
        OR ($7 = 'healthy' AND observation.healthy IS TRUE)
        OR ($7 = 'unhealthy' AND observation.healthy IS FALSE)
        OR ($7 = 'unknown' AND observation.healthy IS NULL)
      )
      AND (
        $8::text IS NULL
        OR concat_ws(' ', observation.client_id, observation.peer_client_id,
            observation.plan_name, observation.interface_name, observation.target,
            observation.reason, observation.kind, observation.source) ILIKE '%' || $8 || '%'
      )
      AND (
        NOT $9
        OR (
            EXISTS (SELECT 1 FROM visible_clients WHERE id = observation.client_id AND status <> 'suspended')
            AND EXISTS (SELECT 1 FROM visible_clients WHERE id = observation.peer_client_id AND status <> 'suspended')
            AND EXISTS (
                SELECT 1 FROM tunnel_plans plan
                WHERE plan.id = observation.plan_id AND plan.deleted_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id=plan.left_client_id
                        AND endpoint.status = 'suspended'
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id=plan.right_client_id
                        AND endpoint.status = 'suspended'
                  )
            )
        )
      )
),
filtered_rollups AS (
    SELECT rollup.*, series.plan_id, series.topology_identity_hash,
           series.plan_name, series.interface_name, series.client_id,
           series.peer_client_id, series.target
    FROM network_observation_rollups rollup
    JOIN network_observation_series series ON series.id = rollup.series_id
    WHERE rollup.bucket_start <= to_timestamp($2)
      AND rollup.bucket_start + make_interval(secs => rollup.bucket_secs) > to_timestamp($1)
      AND (cardinality($3::uuid[]) = 0 OR series.plan_id = ANY($3::uuid[]))
      AND ($4::text IS NULL OR series.client_id = $4 OR series.peer_client_id = $4)
      AND ($5::text IS NULL OR $5 = 'automatic')
      AND ($6::text IS NULL OR $6 = 'tunnel_reachability')
      AND (
        $7::text IS NULL
        OR ($7 = 'healthy' AND rollup.health_state = 1)
        OR ($7 = 'unhealthy' AND rollup.health_state = 0)
        OR ($7 = 'unknown' AND rollup.health_state = -1)
      )
      AND (
        $8::text IS NULL
        OR concat_ws(' ', series.client_id, series.peer_client_id,
            series.plan_name, series.interface_name, series.target,
            NULLIF(rollup.reason_key, ''), 'tunnel_reachability', 'automatic')
            ILIKE '%' || $8 || '%'
      )
      AND (
        NOT $9
        OR (
            EXISTS (SELECT 1 FROM visible_clients WHERE id = series.client_id AND status <> 'suspended')
            AND EXISTS (SELECT 1 FROM visible_clients WHERE id = series.peer_client_id AND status <> 'suspended')
            AND EXISTS (
                SELECT 1 FROM tunnel_plans plan
                WHERE plan.id = series.plan_id AND plan.deleted_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id=plan.left_client_id
                        AND endpoint.status = 'suspended'
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id=plan.right_client_id
                        AND endpoint.status = 'suspended'
                  )
            )
        )
      )
),
retained_evidence AS (
    SELECT
        'tunnel_reachability'::text AS kind,
        rollup.plan_id,
        rollup.topology_identity_hash,
        rollup.plan_name,
        rollup.interface_name,
        rollup.client_id,
        rollup.peer_client_id,
        rollup.bucket_start,
        rollup.bucket_secs,
        TRUE AS retained,
        rollup.sample_count,
        (row_number() OVER (
            PARTITION BY rollup.series_id, rollup.bucket_secs, rollup.bucket_start
            ORDER BY rollup.health_state, rollup.reason_key
        ) = 1)::integer::bigint AS source_bucket_count,
        rollup.bucket_secs AS effective_resolution_secs,
        rollup.sample_count AS automatic_count,
        0::bigint AS manual_count,
        CASE WHEN rollup.health_state = 1 THEN rollup.sample_count ELSE 0 END
            AS healthy_count,
        CASE WHEN rollup.health_state = 0 THEN rollup.sample_count ELSE 0 END
            AS degraded_count,
        rollup.latency_sum_ms,
        rollup.latency_sample_count,
        rollup.latency_min_ms,
        rollup.latency_max_ms,
        rollup.packet_loss_sum_ratio,
        rollup.packet_loss_sample_count,
        0.0::double precision AS throughput_sum_mbps,
        0::bigint AS throughput_sample_count,
        NULL::double precision AS throughput_max_mbps,
        0::numeric AS bytes_total,
        rollup.latest_observed_at
    FROM filtered_rollups rollup
),
evidence AS (
    SELECT * FROM raw_evidence
    UNION ALL
    SELECT * FROM retained_evidence
),
summarized AS (
SELECT
    kind,
    plan_id,
    topology_identity_hash,
    plan_name,
    interface_name,
    client_id,
    peer_client_id,
    bucket_start::text AS bucket_start,
    bucket_secs::bigint AS bucket_secs,
    BOOL_OR(retained) AS retained,
    SUM(sample_count)::bigint AS sample_count,
    CASE WHEN bucket_secs IS NULL
         THEN SUM(source_bucket_count)
         ELSE 1
    END::bigint AS source_bucket_count,
    MAX(effective_resolution_secs)::bigint AS effective_resolution_secs,
    SUM(automatic_count)::bigint AS automatic_count,
    SUM(manual_count)::bigint AS manual_count,
    SUM(healthy_count)::bigint AS healthy_count,
    SUM(degraded_count)::bigint AS degraded_count,
    SUM(latency_sum_ms)::double precision AS latency_sum_ms,
    SUM(latency_sample_count)::bigint AS latency_sample_count,
    MIN(latency_min_ms)::double precision AS latency_min_ms,
    MAX(latency_max_ms)::double precision AS latency_max_ms,
    SUM(packet_loss_sum_ratio)::double precision AS packet_loss_sum_ratio,
    SUM(packet_loss_sample_count)::bigint AS packet_loss_sample_count,
    SUM(throughput_sum_mbps)::double precision AS throughput_sum_mbps,
    SUM(throughput_sample_count)::bigint AS throughput_sample_count,
    MAX(throughput_max_mbps)::double precision AS throughput_max_mbps,
    LEAST(SUM(bytes_total), 9223372036854775807::numeric)::bigint AS bytes_total,
    MAX(latest_observed_at)::text AS latest_observed_at
FROM evidence
GROUP BY kind, plan_id, topology_identity_hash, plan_name, interface_name,
         client_id, peer_client_id, bucket_start, bucket_secs
),
ranked AS (
    SELECT summarized.*,
           row_number() OVER (
               PARTITION BY kind, plan_id, topology_identity_hash,
                            interface_name, client_id, peer_client_id
               ORDER BY bucket_start DESC, latest_observed_at DESC
           ) AS series_rank,
           COUNT(*) OVER (
               PARTITION BY kind, plan_id, topology_identity_hash,
                            interface_name, client_id, peer_client_id
           ) AS series_points,
           COUNT(*) OVER () AS total_points,
           COUNT(*) OVER (
               PARTITION BY kind, plan_id, topology_identity_hash,
                            interface_name, client_id, peer_client_id
           )::numeric / NULLIF(COUNT(*) OVER ()::numeric, 0)
               AS series_share
    FROM summarized
),
budgeted AS (
    SELECT ranked.*,
           GREATEST(2, FLOOR($10 * series_share)::bigint) AS series_budget
    FROM ranked
)
SELECT
    kind, plan_id, topology_identity_hash, plan_name, interface_name,
    client_id, peer_client_id, bucket_start, bucket_secs, retained,
    sample_count, source_bucket_count, effective_resolution_secs,
    automatic_count, manual_count, healthy_count, degraded_count,
    latency_sum_ms, latency_sample_count, latency_min_ms, latency_max_ms,
    packet_loss_sum_ratio, packet_loss_sample_count, throughput_sum_mbps,
    throughput_sample_count, throughput_max_mbps, bytes_total,
    latest_observed_at
FROM budgeted
WHERE series_rank = 1
   OR series_rank = series_points
   OR FLOOR((series_rank - 1)::numeric * (series_budget - 1) / series_points)
      <> FLOOR((series_rank - 2)::numeric * (series_budget - 1) / series_points)
ORDER BY
    CASE WHEN series_rank = 1 OR series_rank = series_points THEN 0 ELSE 1 END,
    series_rank,
    latest_observed_at DESC
LIMIT $10
"#;

const NETWORK_OBSERVATION_ROLLUPS_EXPORT_QUERY: &str = r#"
SELECT
    to_jsonb(rollup) || jsonb_build_object(
        'kind', 'tunnel_reachability',
        'source', 'automatic',
        'retained', TRUE,
        'effective_resolution_secs', rollup.bucket_secs,
        'plan_id', series.plan_id,
        'topology_identity_hash', series.topology_identity_hash,
        'plan_name', series.plan_name,
        'interface_name', series.interface_name,
        'client_id', series.client_id,
        'peer_client_id', series.peer_client_id,
        'endpoint_side', series.endpoint_side,
        'address_family', series.address_family,
        'target', series.target
    ) AS record
FROM network_observation_rollups rollup
JOIN network_observation_series series ON series.id = rollup.series_id
ORDER BY rollup.bucket_start DESC, series.id, rollup.bucket_secs
LIMIT $1
"#;

fn network_observation_trend_from_row(
    row: PgRow,
) -> Result<NetworkObservationTrendView, sqlx::Error> {
    let latency_count = row.try_get::<i64, _>("latency_sample_count")?;
    let packet_loss_count = row.try_get::<i64, _>("packet_loss_sample_count")?;
    let throughput_count = row.try_get::<i64, _>("throughput_sample_count")?;
    Ok(NetworkObservationTrendView {
        kind: row.try_get("kind")?,
        plan_id: row.try_get("plan_id")?,
        topology_identity_hash: row.try_get("topology_identity_hash")?,
        plan_name: row.try_get("plan_name")?,
        interface_name: row.try_get("interface_name")?,
        client_id: row.try_get("client_id")?,
        peer_client_id: row.try_get("peer_client_id")?,
        bucket_start: row.try_get("bucket_start")?,
        bucket_secs: row.try_get("bucket_secs")?,
        retained: row.try_get("retained")?,
        sample_count: row.try_get("sample_count")?,
        source_bucket_count: row.try_get("source_bucket_count")?,
        effective_resolution_secs: row.try_get("effective_resolution_secs")?,
        automatic_count: row.try_get("automatic_count")?,
        manual_count: row.try_get("manual_count")?,
        healthy_count: row.try_get("healthy_count")?,
        degraded_count: row.try_get("degraded_count")?,
        latency_avg_ms: average(row.try_get("latency_sum_ms")?, latency_count),
        latency_min_ms: row.try_get("latency_min_ms")?,
        latency_max_ms: row.try_get("latency_max_ms")?,
        packet_loss_avg_ratio: average(row.try_get("packet_loss_sum_ratio")?, packet_loss_count),
        throughput_avg_mbps: average(row.try_get("throughput_sum_mbps")?, throughput_count),
        throughput_max_mbps: row.try_get("throughput_max_mbps")?,
        bytes_total: row.try_get("bytes_total")?,
        latest_observed_at: row.try_get("latest_observed_at")?,
    })
}

async fn upsert_automatic_observation_series(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    observation: &NetworkObservationView,
) -> Result<i64> {
    let plan_id = observation
        .plan_id
        .ok_or_else(|| anyhow::anyhow!("automatic reachability plan is missing"))?;
    let topology_identity_hash =
        required_observation_field(&observation.topology_identity_hash, "topology identity")?;
    let endpoint_side = required_observation_field(&observation.endpoint_side, "endpoint side")?;
    let address_family = required_observation_field(&observation.address_family, "address family")?;
    sqlx::query(
        r#"
        UPDATE network_observation_series
        SET active = FALSE
        WHERE plan_id = $1
          AND client_id = $2
          AND endpoint_side = $3
          AND address_family = $4
          AND topology_identity_hash <> $5
          AND active = TRUE
        "#,
    )
    .bind(plan_id)
    .bind(&observation.client_id)
    .bind(endpoint_side)
    .bind(address_family)
    .bind(topology_identity_hash)
    .execute(&mut **tx)
    .await?;
    let row = sqlx::query(
        r#"
        INSERT INTO network_observation_series (
            plan_id, topology_identity_hash, plan_name, interface_name,
            client_id, peer_client_id, endpoint_side, address_family, target
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
        ON CONFLICT (
            plan_id, topology_identity_hash, client_id, peer_client_id,
            endpoint_side, address_family, interface_name, target
        ) DO UPDATE SET
            plan_name = EXCLUDED.plan_name,
            active = TRUE,
            last_seen_at = now()
        RETURNING id
        "#,
    )
    .bind(plan_id)
    .bind(topology_identity_hash)
    .bind(required_observation_field(
        &observation.plan_name,
        "plan name",
    )?)
    .bind(required_observation_field(
        &observation.interface_name,
        "interface",
    )?)
    .bind(&observation.client_id)
    .bind(required_observation_field(
        &observation.peer_client_id,
        "peer client",
    )?)
    .bind(endpoint_side)
    .bind(address_family)
    .bind(required_observation_field(&observation.target, "target")?)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.try_get("id")?)
}

pub(crate) async fn reconcile_postgres_automatic_observation_series_for_client(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE network_observation_series series
        SET active = FALSE
        WHERE series.client_id = $1
          AND series.active = TRUE
          AND NOT EXISTS (
              SELECT 1
              FROM telemetry_tunnels telemetry
              JOIN tunnel_plans plan
                ON plan.id = series.plan_id
               AND plan.enabled = TRUE
               AND plan.deleted_at IS NULL
              WHERE telemetry.client_id = series.client_id
                AND telemetry.telemetry_plan_id = series.plan_id::text
                AND telemetry.interface = series.interface_name
                AND telemetry.telemetry_endpoint_side = series.endpoint_side
                AND telemetry.telemetry_peer_client_id = series.peer_client_id
                AND telemetry.latency_monitoring_enabled IS TRUE
                AND telemetry.latency_primary_family = series.address_family
                AND telemetry.latency_target = series.target
          )
        "#,
    )
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

pub(crate) async fn deactivate_postgres_automatic_observation_series_for_plan(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    plan_id: Uuid,
    current_topology_identity_hash: Option<&str>,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE network_observation_series
        SET active = FALSE
        WHERE plan_id = $1
          AND active = TRUE
          AND ($2::text IS NULL OR topology_identity_hash <> $2)
        "#,
    )
    .bind(plan_id)
    .bind(current_topology_identity_hash)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected())
}

fn required_observation_field<'a>(value: &'a Option<String>, name: &str) -> Result<&'a str> {
    value
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("automatic reachability {name} is missing"))
}

async fn upsert_latest_automatic_observation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    series_id: i64,
    observation: &NetworkObservationView,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO network_observation_latest (
            series_id, observation_id, stale_after_secs, healthy,
            transmitted, received, latency_min_ms, latency_avg_ms,
            latency_max_ms, latency_mdev_ms, packet_loss_ratio, reason,
            metadata, observed_at, received_at
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,
            to_timestamp($14),to_timestamp($15)
        )
        ON CONFLICT (series_id) DO UPDATE SET
            observation_id = EXCLUDED.observation_id,
            stale_after_secs = EXCLUDED.stale_after_secs,
            healthy = EXCLUDED.healthy,
            transmitted = EXCLUDED.transmitted,
            received = EXCLUDED.received,
            latency_min_ms = EXCLUDED.latency_min_ms,
            latency_avg_ms = EXCLUDED.latency_avg_ms,
            latency_max_ms = EXCLUDED.latency_max_ms,
            latency_mdev_ms = EXCLUDED.latency_mdev_ms,
            packet_loss_ratio = EXCLUDED.packet_loss_ratio,
            reason = EXCLUDED.reason,
            metadata = EXCLUDED.metadata,
            observed_at = EXCLUDED.observed_at,
            received_at = EXCLUDED.received_at,
            updated_at = now()
        WHERE (EXCLUDED.observed_at, EXCLUDED.observation_id)
            > (network_observation_latest.observed_at,
               network_observation_latest.observation_id)
        "#,
    )
    .bind(series_id)
    .bind(observation.id)
    .bind(observation.stale_after_secs.unwrap_or(180))
    .bind(observation.healthy.unwrap_or(false))
    .bind(observation.transmitted.unwrap_or(0))
    .bind(observation.received.unwrap_or(0))
    .bind(observation.latency_min_ms)
    .bind(observation.latency_avg_ms)
    .bind(observation.latency_max_ms)
    .bind(observation.latency_mdev_ms)
    .bind(observation.packet_loss_ratio.unwrap_or(1.0))
    .bind(&observation.reason)
    .bind(SqlJson(&observation.metadata))
    .bind(
        observation_timestamp_unix(&observation.observed_at)
            .unwrap_or_else(|| Utc::now().timestamp()),
    )
    .bind(
        observation_timestamp_unix(&observation.received_at)
            .unwrap_or_else(|| Utc::now().timestamp()),
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_network_observation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    observation: &NetworkObservationView,
    manual_upsert: bool,
    automatic_series_id: Option<i64>,
) -> Result<bool> {
    let conflict = if manual_upsert {
        "ON CONFLICT (job_id, client_id, seq) WHERE job_id IS NOT NULL AND seq IS NOT NULL DO UPDATE SET kind = EXCLUDED.kind, source = EXCLUDED.source, role = EXCLUDED.role, plan_id = EXCLUDED.plan_id, topology_identity_hash = EXCLUDED.topology_identity_hash, plan_name = EXCLUDED.plan_name, interface_name = EXCLUDED.interface_name, peer_client_id = EXCLUDED.peer_client_id, target = EXCLUDED.target, endpoint_side = EXCLUDED.endpoint_side, address_family = EXCLUDED.address_family, stale_after_secs = EXCLUDED.stale_after_secs, healthy = EXCLUDED.healthy, transmitted = EXCLUDED.transmitted, received = EXCLUDED.received, latency_min_ms = EXCLUDED.latency_min_ms, latency_avg_ms = EXCLUDED.latency_avg_ms, latency_max_ms = EXCLUDED.latency_max_ms, latency_mdev_ms = EXCLUDED.latency_mdev_ms, packet_loss_ratio = EXCLUDED.packet_loss_ratio, reason = EXCLUDED.reason, throughput_mbps = EXCLUDED.throughput_mbps, bytes = EXCLUDED.bytes, metadata = EXCLUDED.metadata, observed_at = EXCLUDED.observed_at, received_at = EXCLUDED.received_at"
    } else {
        "ON CONFLICT (id) DO NOTHING"
    };
    let query = format!(
        r#"
        INSERT INTO network_observations (
            id, job_id, client_id, seq, kind, source, role, plan_id,
            topology_identity_hash, plan_name, interface_name, peer_client_id,
            target, endpoint_side, address_family, stale_after_secs, healthy,
            transmitted, received, latency_min_ms, latency_avg_ms, latency_max_ms,
            latency_mdev_ms, packet_loss_ratio, reason, throughput_mbps, bytes,
            automatic_series_id, metadata, observed_at, received_at
        ) VALUES (
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
            $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,to_timestamp($30),to_timestamp($31)
        ) {conflict}
        "#
    );
    let result = sqlx::query(&query)
        .bind(observation.id)
        .bind(observation.job_id)
        .bind(&observation.client_id)
        .bind(observation.seq)
        .bind(&observation.kind)
        .bind(&observation.source)
        .bind(&observation.role)
        .bind(observation.plan_id)
        .bind(&observation.topology_identity_hash)
        .bind(&observation.plan_name)
        .bind(&observation.interface_name)
        .bind(&observation.peer_client_id)
        .bind(&observation.target)
        .bind(&observation.endpoint_side)
        .bind(&observation.address_family)
        .bind(observation.stale_after_secs)
        .bind(observation.healthy)
        .bind(observation.transmitted)
        .bind(observation.received)
        .bind(observation.latency_min_ms)
        .bind(observation.latency_avg_ms)
        .bind(observation.latency_max_ms)
        .bind(observation.latency_mdev_ms)
        .bind(observation.packet_loss_ratio)
        .bind(&observation.reason)
        .bind(observation.throughput_mbps)
        .bind(observation.bytes)
        .bind(automatic_series_id)
        .bind(SqlJson(&observation.metadata))
        .bind(
            observation_timestamp_unix(&observation.observed_at)
                .unwrap_or_else(|| Utc::now().timestamp()),
        )
        .bind(
            observation_timestamp_unix(&observation.received_at)
                .unwrap_or_else(|| Utc::now().timestamp()),
        )
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected() > 0)
}

fn network_observation_from_row(row: PgRow) -> Result<NetworkObservationView, sqlx::Error> {
    let metadata: SqlJson<serde_json::Value> = row.try_get("metadata")?;
    Ok(NetworkObservationView {
        id: row.try_get("id")?,
        job_id: row.try_get("job_id")?,
        client_id: row.try_get("client_id")?,
        seq: row.try_get("seq")?,
        kind: row.try_get("kind")?,
        source: row.try_get("source")?,
        role: row.try_get("role")?,
        plan_id: row.try_get("plan_id")?,
        topology_identity_hash: row.try_get("topology_identity_hash")?,
        plan_name: row.try_get("plan_name")?,
        interface_name: row.try_get("interface_name")?,
        peer_client_id: row.try_get("peer_client_id")?,
        target: row.try_get("target")?,
        endpoint_side: row.try_get("endpoint_side")?,
        address_family: row.try_get("address_family")?,
        stale_after_secs: row.try_get("stale_after_secs")?,
        healthy: row.try_get("healthy")?,
        transmitted: row.try_get("transmitted")?,
        received: row.try_get("received")?,
        latency_min_ms: row.try_get("latency_min_ms")?,
        latency_avg_ms: row.try_get("latency_avg_ms")?,
        latency_max_ms: row.try_get("latency_max_ms")?,
        latency_mdev_ms: row.try_get("latency_mdev_ms")?,
        packet_loss_ratio: row.try_get("packet_loss_ratio")?,
        reason: row.try_get("reason")?,
        throughput_mbps: row.try_get("throughput_mbps")?,
        bytes: row.try_get("bytes")?,
        metadata: metadata.0,
        observed_at: row.try_get("observed_at")?,
        received_at: row.try_get("received_at")?,
    })
}

fn limit_observations_fairly(
    rows: Vec<NetworkObservationView>,
    limit_per_series: usize,
    total_limit: usize,
) -> Vec<NetworkObservationView> {
    let mut counts = HashMap::<(Option<Uuid>, Option<String>, String, String), usize>::new();
    let mut ranked = Vec::<(usize, NetworkObservationView)>::new();
    for observation in rows {
        let endpoint = observation
            .endpoint_side
            .clone()
            .unwrap_or_else(|| observation.client_id.clone());
        let key = (
            observation.plan_id,
            observation.topology_identity_hash.clone(),
            observation.kind.clone(),
            endpoint,
        );
        let rank = counts.entry(key).or_default();
        if *rank < limit_per_series {
            ranked.push((*rank, observation));
        }
        *rank = rank.saturating_add(1);
    }
    ranked.sort_by(|(left_rank, left), (right_rank, right)| {
        left_rank
            .cmp(right_rank)
            .then_with(|| compare_network_observations_desc(left, right))
    });
    ranked
        .into_iter()
        .take(total_limit)
        .map(|(_, observation)| observation)
        .collect()
}

fn limit_topology_rows(
    rows: Vec<NetworkObservationView>,
    limit: usize,
) -> Vec<NetworkObservationView> {
    let mut counts = HashMap::<(Uuid, String, String), usize>::new();
    rows.into_iter()
        .filter(|observation| {
            let Some(plan_id) = observation.plan_id else {
                return false;
            };
            let endpoint = observation
                .endpoint_side
                .clone()
                .unwrap_or_else(|| observation.client_id.clone());
            let key = (plan_id, observation.kind.clone(), endpoint);
            let count = counts.entry(key).or_default();
            let allowed = if observation.kind == "network_status" {
                1
            } else {
                limit
            };
            let keep = *count < allowed;
            *count = count.saturating_add(1);
            keep
        })
        .collect()
}

#[derive(Hash, Eq, PartialEq)]
struct TrendKey {
    kind: String,
    plan_id: Option<Uuid>,
    topology_identity_hash: Option<String>,
    plan_name: Option<String>,
    interface_name: Option<String>,
    client_id: String,
    peer_client_id: Option<String>,
    bucket_start: String,
}

struct TrendAccumulator {
    key: TrendKey,
    sample_count: i64,
    source_bucket_count: i64,
    effective_resolution_secs: Option<i64>,
    automatic_count: i64,
    manual_count: i64,
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
                bucket_start: observation.observed_at.clone(),
            },
            sample_count: 0,
            source_bucket_count: 0,
            effective_resolution_secs: None,
            automatic_count: 0,
            manual_count: 0,
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
        self.source_bucket_count += 1;
        if observation.source == "automatic" {
            self.automatic_count += 1;
        } else {
            self.manual_count += 1;
        }
        match observation.healthy {
            Some(true) => self.healthy_count += 1,
            Some(false) => self.degraded_count += 1,
            None => {}
        }
        if let Some(value) = observation.latency_avg_ms {
            self.latency_sum_ms += value;
            self.latency_count += 1;
            self.latency_min_ms = Some(
                self.latency_min_ms
                    .map_or(value, |current| current.min(value)),
            );
            self.latency_max_ms = Some(
                self.latency_max_ms
                    .map_or(value, |current| current.max(value)),
            );
        }
        if let Some(value) = observation.packet_loss_ratio {
            self.packet_loss_sum_ratio += value;
            self.packet_loss_count += 1;
        }
        if let Some(value) = observation.throughput_mbps {
            self.throughput_sum_mbps += value;
            self.throughput_count += 1;
            self.throughput_max_mbps = Some(
                self.throughput_max_mbps
                    .map_or(value, |current| current.max(value)),
            );
        }
        if let Some(value) = observation.bytes {
            self.bytes_total = self.bytes_total.saturating_add(value);
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
            bucket_start: Some(self.key.bucket_start),
            bucket_secs: None,
            retained: false,
            sample_count: self.sample_count,
            source_bucket_count: self.source_bucket_count,
            effective_resolution_secs: self.effective_resolution_secs,
            automatic_count: self.automatic_count,
            manual_count: self.manual_count,
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
    let mut groups = HashMap::<TrendKey, TrendAccumulator>::new();
    for observation in observations {
        let key = TrendKey {
            kind: observation.kind.clone(),
            plan_id: observation.plan_id,
            topology_identity_hash: observation.topology_identity_hash.clone(),
            plan_name: observation.plan_name.clone(),
            interface_name: observation.interface_name.clone(),
            client_id: observation.client_id.clone(),
            peer_client_id: observation.peer_client_id.clone(),
            bucket_start: observation.observed_at.clone(),
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

pub(crate) fn topology_identity_hash_for_plan(plan: &TunnelPlanView) -> String {
    tunnel_topology_identity_hash(plan.id, &plan.plan)
}

fn expected_reachability_target(
    plan: &vpsman_common::TunnelPlan,
    side: vpsman_common::TunnelEndpointSide,
    family: vpsman_common::TunnelAddressFamily,
) -> Option<&str> {
    let pair = match family {
        vpsman_common::TunnelAddressFamily::Ipv4 => plan.ipv4_tunnel.as_ref(),
        vpsman_common::TunnelAddressFamily::Ipv6 => plan.ipv6_tunnel.as_ref(),
    }?;
    Some(match side {
        vpsman_common::TunnelEndpointSide::Left => pair.right.as_str(),
        vpsman_common::TunnelEndpointSide::Right => pair.left.as_str(),
    })
}

fn observation_matches_declared_plan(
    observation: &NetworkObservationView,
    plan: &vpsman_common::TunnelPlan,
) -> bool {
    observation.plan_name.as_deref() == Some(plan.name.as_str())
        && observation.interface_name.as_deref() == Some(plan.interface_name.as_str())
        && matches!(
            (observation.client_id.as_str(), observation.peer_client_id.as_deref()),
            (client, Some(peer)) if (client == plan.left_client_id && peer == plan.right_client_id)
                || (client == plan.right_client_id && peer == plan.left_client_id)
        )
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
    let raw_kind = as_string(metadata.get("type"))?;
    let kind = match raw_kind.as_str() {
        "tunnel_reachability" => "tunnel_reachability",
        "network_status" => "network_status",
        "network_speed_test" => "network_speed_test",
        _ => return None,
    }
    .to_string();
    let parsed = metadata.get("parsed").unwrap_or(&serde_json::Value::Null);
    let runtime_summary = metadata
        .get("runtime")
        .and_then(|runtime| runtime.get("summary"))
        .unwrap_or(&serde_json::Value::Null);
    let root_or_parsed = |name: &str| metadata.get(name).or_else(|| parsed.get(name));
    let healthy = if kind == "network_status" {
        runtime_summary.get("healthy").and_then(as_bool)
    } else {
        root_or_parsed("healthy")
            .and_then(as_bool)
            .or_else(|| metadata.get("success").and_then(as_bool))
    };
    Some(NetworkObservationView {
        id: Uuid::new_v4(),
        job_id: Some(job_id),
        client_id: client_id.to_string(),
        seq: Some(seq),
        kind,
        source: "manual".to_string(),
        role: as_string(metadata.get("role")),
        plan_id: None,
        topology_identity_hash: None,
        plan_name: as_string(metadata.get("plan")),
        interface_name: as_string(metadata.get("interface")),
        peer_client_id: as_string(metadata.get("peer_client_id")),
        target: as_string(metadata.get("target")).or_else(|| {
            as_string(metadata.get("server_address")).map(|address| {
                metadata
                    .get("port")
                    .and_then(as_i64)
                    .map_or(address.clone(), |port| format!("{address}:{port}"))
            })
        }),
        endpoint_side: as_string(metadata.get("side")),
        address_family: as_string(metadata.get("address_family")),
        stale_after_secs: root_or_parsed("stale_after_secs").and_then(as_i64),
        healthy,
        transmitted: root_or_parsed("transmitted")
            .and_then(as_i64)
            .and_then(|value| i32::try_from(value).ok()),
        received: root_or_parsed("received")
            .and_then(as_i64)
            .and_then(|value| i32::try_from(value).ok()),
        latency_min_ms: root_or_parsed("latency_min_ms").and_then(as_f64),
        latency_avg_ms: root_or_parsed("latency_avg_ms").and_then(as_f64),
        latency_max_ms: root_or_parsed("latency_max_ms").and_then(as_f64),
        latency_mdev_ms: root_or_parsed("latency_mdev_ms").and_then(as_f64),
        packet_loss_ratio: root_or_parsed("packet_loss_ratio").and_then(as_f64),
        reason: as_string(metadata.get("reason")).or_else(|| as_string(parsed.get("reason"))),
        throughput_mbps: metadata.get("throughput_mbps").and_then(as_f64),
        bytes: metadata.get("bytes").and_then(as_i64),
        metadata,
        observed_at: observed_at.to_string(),
        received_at: observed_at.to_string(),
    })
}

fn source_label(source: TunnelReachabilitySource) -> &'static str {
    match source {
        TunnelReachabilitySource::Automatic => "automatic",
        TunnelReachabilitySource::Manual => "manual",
    }
}
fn endpoint_side_label(side: vpsman_common::TunnelEndpointSide) -> &'static str {
    match side {
        vpsman_common::TunnelEndpointSide::Left => "left",
        vpsman_common::TunnelEndpointSide::Right => "right",
    }
}
fn address_family_label(family: vpsman_common::TunnelAddressFamily) -> &'static str {
    match family {
        vpsman_common::TunnelAddressFamily::Ipv4 => "ipv4",
        vpsman_common::TunnelAddressFamily::Ipv6 => "ipv6",
    }
}
fn as_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
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
fn average(sum: f64, count: i64) -> Option<f64> {
    (count > 0).then_some(sum / count as f64)
}
fn observation_timestamp_unix(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().or_else(|| {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.timestamp())
    })
}
fn compare_network_observations_desc(
    left: &NetworkObservationView,
    right: &NetworkObservationView,
) -> std::cmp::Ordering {
    compare_timestamps_desc(&left.observed_at, &right.observed_at)
        .then_with(|| right.id.cmp(&left.id))
}

fn tunnel_plan_evidence_clear_audit(
    results: &[TunnelPlanEvidenceClearResult],
    operator: &AuthContext,
) -> AuditLogView {
    let plan_ids = results
        .iter()
        .map(|result| result.plan_id)
        .collect::<Vec<_>>();
    let cleared_observation_count = results
        .iter()
        .map(|result| result.cleared_observation_count)
        .sum::<u64>();
    let target = if let [plan_id] = plan_ids.as_slice() {
        format!("tunnel_plan:{plan_id}")
    } else {
        "tunnel_plans:bulk".to_string()
    };
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: persisted_actor_id(operator),
        action: "network.tunnel_plan_evidence_cleared".to_string(),
        target,
        command_hash: None,
        metadata: network_audit_metadata(
            serde_json::json!({
                "plan_ids": plan_ids,
                "plan_count": results.len(),
                "cleared_observation_count": cleared_observation_count,
                "plans": results,
            }),
            operator,
            "succeeded",
        ),
        created_at: unix_now().to_string(),
    }
}

#[cfg(test)]
#[path = "tests_repository_network_observations.rs"]
mod tests;
