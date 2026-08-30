use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::{postgres::PgRow, types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    tunnel_topology_identity_hash, CommandOutput, JobCommand, OutputStream, TunnelPlan,
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

const NETWORK_OBSERVATION_ID_LOCK_HASH_SEED: i64 = 0x4e4f_4253_4944_4c4b;

/// Serializes only writers proposing the same global observation UUID. This
/// query is deliberately separate from the writer statement: after a waiter
/// acquires ownership, READ COMMITTED gives that statement a fresh snapshot
/// containing the winner's registry/latest changes.
async fn lock_network_observation_ids_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    observation_ids: &[Uuid],
) -> Result<()> {
    if observation_ids.is_empty() {
        return Ok(());
    }
    let locked = sqlx::query_scalar::<_, i64>(
        r#"
        WITH ordered_ids AS MATERIALIZED (
            SELECT DISTINCT observation_id
            FROM unnest($1::uuid[]) AS proposed(observation_id)
            ORDER BY observation_id
        ), locked_ids AS MATERIALIZED (
            SELECT pg_advisory_xact_lock(hashtextextended(
                'vpsman.network_observation.id:' || observation_id::text,
                $2
            )) AS acquired
            FROM ordered_ids
            ORDER BY observation_id
        )
        SELECT count(*)::bigint FROM locked_ids
        "#,
    )
    .bind(observation_ids)
    .bind(NETWORK_OBSERVATION_ID_LOCK_HASH_SEED)
    .fetch_one(&mut **tx)
    .await?;
    anyhow::ensure!(
        usize::try_from(locked).ok()
            == Some(
                observation_ids
                    .iter()
                    .copied()
                    .collect::<HashSet<_>>()
                    .len()
            ),
        "network observation UUID ownership changed while locking"
    );
    Ok(())
}

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

/// The one transaction-frozen plan view shared by tunnel classification and
/// automatic reachability validation for a claimed telemetry suffix. Only
/// enabled, undeleted plans for which the projecting client is an endpoint are
/// present in this map.
#[derive(Clone, Debug)]
pub(crate) struct FrozenAutomaticTunnelPlan {
    plan_name: String,
    left_client_id: String,
    right_client_id: String,
    plan: TunnelPlan,
    topology_identity_hash: String,
}

/// One immutable raw-journal envelope in a claimed telemetry suffix. The
/// original payload ordinal is retained before validation so the compact
/// automatic locator always addresses the exact JSON array element.
pub(crate) struct AutomaticTunnelReachabilitySample<'a> {
    pub(crate) sample_id: Uuid,
    pub(crate) accepted_seq: i64,
    pub(crate) observations: &'a [TunnelReachabilityObservation],
}

impl FrozenAutomaticTunnelPlan {
    pub(crate) fn new(
        plan_id: Uuid,
        plan_name: String,
        left_client_id: String,
        right_client_id: String,
        plan: TunnelPlan,
    ) -> Self {
        let topology_identity_hash = tunnel_topology_identity_hash(plan_id, &plan);
        Self {
            plan_name,
            left_client_id,
            right_client_id,
            plan,
            topology_identity_hash,
        }
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

                let rows = sqlx::query(
                    r#"
                    WITH target_series AS MATERIALIZED (
                        SELECT id, plan_id
                        FROM network_observation_series
                        WHERE plan_id = ANY($1::uuid[])
                    ), pending_automatic AS MATERIALIZED (
                        SELECT locator.id, series.plan_id
                        FROM network_observations locator
                        JOIN target_series series
                          ON series.id = locator.automatic_series_id
                        JOIN telemetry_samples sample
                          ON sample.id = locator.automatic_sample_id
                        JOIN telemetry_minute_materialization_heads head
                          ON head.client_id = sample.client_id
                        WHERE locator.source = 'automatic'
                          AND sample.accepted_seq > head.materialized_seq
                    ), retained_counts AS MATERIALIZED (
                        SELECT series.plan_id,
                               LEAST(
                                   COALESCE(SUM(rollup.sample_count), 0),
                                   9223372036854775807::numeric
                               )::bigint AS cleared_count
                        FROM network_observation_rollups rollup
                        JOIN target_series series ON series.id = rollup.series_id
                        GROUP BY series.plan_id
                    ), deleted_manual AS (
                        DELETE FROM network_observations observation
                        WHERE observation.source = 'manual'
                          AND observation.plan_id = ANY($1::uuid[])
                        RETURNING observation.plan_id
                    ), deleted_automatic AS (
                        DELETE FROM network_observations observation
                        USING target_series series
                        WHERE observation.source = 'automatic'
                          AND observation.automatic_series_id = series.id
                        RETURNING observation.id
                    ), automatic_delete_barrier AS MATERIALIZED (
                        SELECT count(*) FROM deleted_automatic
                    ), deleted_series AS (
                        DELETE FROM network_observation_series series
                        USING target_series target, automatic_delete_barrier
                        WHERE series.id = target.id
                        RETURNING series.id
                    ), series_delete_barrier AS MATERIALIZED (
                        SELECT count(*) FROM deleted_series
                    ), counts AS (
                        SELECT plan_id, count(*)::bigint AS cleared_count
                        FROM deleted_manual
                        GROUP BY plan_id
                        UNION ALL
                        SELECT plan_id, count(*)::bigint AS cleared_count
                        FROM pending_automatic
                        GROUP BY plan_id
                        UNION ALL
                        SELECT plan_id, cleared_count
                        FROM retained_counts
                    )
                    SELECT counts.plan_id,
                           LEAST(
                               SUM(counts.cleared_count)::numeric,
                               9223372036854775807::numeric
                           )::bigint AS cleared_count
                    FROM counts
                    CROSS JOIN series_delete_barrier
                    GROUP BY counts.plan_id
                    "#,
                )
                .bind(&plan_ids)
                .fetch_all(&mut *tx)
                .await?;
                let mut cleared_by_plan = HashMap::<Uuid, u64>::new();
                for row in rows {
                    let count = row.try_get::<i64, _>("cleared_count")?;
                    cleared_by_plan.insert(row.try_get("plan_id")?, u64::try_from(count)?);
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let observation_ids = observations
                    .iter()
                    .map(|observation| observation.id)
                    .collect::<Vec<_>>();
                lock_network_observation_ids_in_tx(&mut tx, &observation_ids).await?;
                for observation in observations {
                    insert_network_observation(&mut tx, &observation).await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }
}

/// Validates and projects a complete claimed suffix in one statement. The
/// projector owns only series identity, compact raw locators, and current
/// state; the independent closed-minute consumer owns all historical rollups.
pub(crate) async fn record_postgres_automatic_tunnel_reachability_suffix_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    plans: &HashMap<Uuid, FrozenAutomaticTunnelPlan>,
    samples: &[AutomaticTunnelReachabilitySample<'_>],
) -> Result<()> {
    if plans.is_empty() || samples.is_empty() {
        return Ok(());
    }
    let received_at = Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true);
    let mut accepted = Vec::new();
    let mut accepted_ids = Vec::new();
    for sample in samples {
        for (zero_based_ordinal, observation) in sample.observations.iter().enumerate() {
            let Some(plan_snapshot) = plans.get(&observation.plan_id) else {
                continue;
            };
            let (expected_client, expected_peer) = match observation.endpoint_side {
                vpsman_common::TunnelEndpointSide::Left => (
                    &plan_snapshot.left_client_id,
                    &plan_snapshot.right_client_id,
                ),
                vpsman_common::TunnelEndpointSide::Right => (
                    &plan_snapshot.right_client_id,
                    &plan_snapshot.left_client_id,
                ),
            };
            let expected_target = expected_reachability_target(
                &plan_snapshot.plan,
                observation.endpoint_side,
                observation.address_family,
            );
            if observation.source != TunnelReachabilitySource::Automatic
                || expected_client != client_id
                || observation.peer_client_id.as_str() != expected_peer.as_str()
                || observation.interface_name.as_str() != plan_snapshot.plan.interface_name.as_str()
                || expected_target != Some(observation.target.as_str())
                || observation.topology_identity_hash != plan_snapshot.topology_identity_hash
                || !observation.values_are_coherent()
            {
                continue;
            }
            let payload_ordinal = zero_based_ordinal
                .checked_add(1)
                .and_then(|ordinal| i16::try_from(ordinal).ok())
                .context("automatic reachability payload ordinal is exhausted")?;
            let observed_at = DateTime::from_timestamp(observation.measured_unix as i64, 0)
                .context("automatic reachability timestamp is invalid")?
                .to_rfc3339_opts(SecondsFormat::Micros, true);
            accepted.push(serde_json::json!({
                "id": observation.id,
                "sample_id": sample.sample_id,
                "accepted_seq": sample.accepted_seq,
                "payload_ordinal": payload_ordinal,
                "client_id": client_id,
                "plan_id": observation.plan_id,
                "topology_identity_hash": plan_snapshot.topology_identity_hash,
                "plan_name": plan_snapshot.plan_name,
                "interface_name": observation.interface_name,
                "peer_client_id": observation.peer_client_id,
                "target": observation.target,
                "endpoint_side": endpoint_side_label(observation.endpoint_side),
                "address_family": address_family_label(observation.address_family),
                "stale_after_secs": observation.stale_after_secs,
                "healthy": observation.healthy,
                "transmitted": observation.transmitted,
                "received": observation.received,
                "latency_min_ms": observation.latency_min_ms,
                "latency_avg_ms": observation.latency_avg_ms,
                "latency_max_ms": observation.latency_max_ms,
                "latency_mdev_ms": observation.latency_mdev_ms,
                "packet_loss_ratio": observation.packet_loss_ratio,
                "reason": observation.reason,
                "observed_at": observed_at,
                "received_at": received_at,
            }));
            accepted_ids.push(observation.id);
        }
    }
    if accepted.is_empty() {
        return Ok(());
    }
    lock_network_observation_ids_in_tx(tx, &accepted_ids).await?;
    sqlx::query(AUTOMATIC_TUNNEL_REACHABILITY_BATCH_SQL)
        .bind(SqlJson(&accepted))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) const NETWORK_OBSERVATION_TRENDS_QUERY: &str = r#"
WITH RECURSIVE automatic_physical_series AS MATERIALIZED (
    SELECT series.*
    FROM network_observation_series series
    WHERE (cardinality($3::uuid[]) = 0 OR series.plan_id = ANY($3::uuid[]))
      AND ($4::text IS NULL
           OR series.client_id = $4
           OR series.peer_client_id = $4)
      AND ($5::text IS NULL OR $5 = 'automatic')
      AND ($6::text IS NULL OR $6 = 'tunnel_reachability')
      AND (
        NOT $9
        OR (
            EXISTS (
                SELECT 1 FROM visible_clients
                WHERE id = series.client_id AND status <> 'suspended'
            )
            AND EXISTS (
                SELECT 1 FROM visible_clients
                WHERE id = series.peer_client_id AND status <> 'suspended'
            )
            AND EXISTS (
                SELECT 1 FROM tunnel_plans plan
                WHERE plan.id = series.plan_id AND plan.deleted_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id = plan.left_client_id
                        AND endpoint.status = 'suspended'
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id = plan.right_client_id
                        AND endpoint.status = 'suspended'
                  )
            )
        )
      )
),
manual_points AS NOT MATERIALIZED (
    SELECT
        NULL::bigint AS physical_series_id,
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
      AND observation.source = 'manual'
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
manual_series AS MATERIALIZED (
    -- Manual job evidence has no persistent series catalogue. This exact
    -- distinct pass is deliberately isolated from high-volume automatic
    -- telemetry; arbitrary reason substring matching cannot be index-seeked
    -- without changing the schema or its matching semantics.
    SELECT
        point.kind,
        point.plan_id,
        point.topology_identity_hash,
        point.interface_name,
        point.client_id,
        point.peer_client_id
    FROM manual_points point
    GROUP BY point.kind, point.plan_id, point.topology_identity_hash,
             point.interface_name, point.client_id, point.peer_client_id
),
pending_samples AS MATERIALIZED (
    SELECT sample.id, sample.payload
    FROM telemetry_minute_materialization_heads head
    JOIN telemetry_projection_heads projection USING (client_id)
    JOIN (
        SELECT DISTINCT client_id
        FROM automatic_physical_series
    ) eligible_client USING (client_id)
    JOIN telemetry_samples sample
      ON sample.client_id = head.client_id
     AND sample.accepted_seq > head.materialized_seq
     AND sample.accepted_seq <= projection.projected_seq
),
pending_rows AS MATERIALIZED (
    SELECT locator.id, series.plan_name, locator.observed_at,
           series.id AS series_id, series.plan_id,
           series.topology_identity_hash, series.interface_name,
           series.client_id, series.peer_client_id, series.target,
           raw.observation
    FROM pending_samples sample
    JOIN network_observations locator
      ON locator.automatic_sample_id = sample.id
     AND locator.source = 'automatic'
    JOIN automatic_physical_series series
      ON series.id = locator.automatic_series_id
    CROSS JOIN LATERAL (
        SELECT sample.payload -> 'tunnel_reachability'
                     -> (locator.automatic_payload_ordinal::integer - 1)
                     AS observation
    ) raw
    WHERE locator.observed_at >= to_timestamp($1)
      AND locator.observed_at <= to_timestamp($2)
      AND raw.observation IS NOT NULL
      AND (raw.observation ->> 'id')::uuid = locator.id
      AND (
        $7::text IS NULL
        OR ($7 = 'healthy' AND (raw.observation ->> 'healthy')::boolean)
        OR ($7 = 'unhealthy' AND NOT (raw.observation ->> 'healthy')::boolean)
      )
      AND (
        $8::text IS NULL
        OR concat_ws(' ', series.client_id, series.peer_client_id,
            series.plan_name, series.interface_name, series.target,
            raw.observation ->> 'reason',
            'tunnel_reachability', 'automatic') ILIKE '%' || $8 || '%'
      )
),
pending_fragments AS MATERIALIZED (
    SELECT
        series_id AS physical_series_id,
        'tunnel_reachability'::text AS kind,
        plan_id, topology_identity_hash, plan_name,
        interface_name, client_id, peer_client_id,
        date_bin(
            interval '1 minute', observed_at,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS bucket_start,
        60::integer AS bucket_secs,
        FALSE AS retained,
        count(*)::bigint AS sample_count,
        1::bigint AS source_bucket_count,
        60::integer AS effective_resolution_secs,
        count(*)::bigint AS automatic_count,
        0::bigint AS manual_count,
        count(*) FILTER (
            WHERE (observation ->> 'healthy')::boolean
        )::bigint AS healthy_count,
        count(*) FILTER (
            WHERE NOT (observation ->> 'healthy')::boolean
        )::bigint AS degraded_count,
        sum(COALESCE((observation ->> 'latency_avg_ms')::double precision, 0.0))
            AS latency_sum_ms,
        count((observation ->> 'latency_avg_ms')::double precision)::bigint
            AS latency_sample_count,
        min((observation ->> 'latency_avg_ms')::double precision)
            AS latency_min_ms,
        max((observation ->> 'latency_avg_ms')::double precision)
            AS latency_max_ms,
        sum((observation ->> 'packet_loss_ratio')::double precision)
            AS packet_loss_sum_ratio,
        count((observation ->> 'packet_loss_ratio')::double precision)::bigint
            AS packet_loss_sample_count,
        0.0::double precision AS throughput_sum_mbps,
        0::bigint AS throughput_sample_count,
        NULL::double precision AS throughput_max_mbps,
        0::numeric AS bytes_total,
        max(observed_at) AS latest_observed_at
    FROM pending_rows
    GROUP BY series_id, plan_id, topology_identity_hash, plan_name,
             interface_name, client_id, peer_client_id, bucket_start,
             (observation ->> 'healthy')::boolean
),
retained_fragments AS NOT MATERIALIZED (
    SELECT
        rollup.series_id AS physical_series_id,
        'tunnel_reachability'::text AS kind,
        series.plan_id,
        series.topology_identity_hash,
        series.plan_name,
        series.interface_name,
        series.client_id,
        series.peer_client_id,
        rollup.bucket_start,
        rollup.bucket_secs,
        TRUE AS retained,
        rollup.sample_count,
        1::bigint AS source_bucket_count,
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
    FROM network_observation_rollups rollup
    JOIN automatic_physical_series series ON series.id = rollup.series_id
    WHERE rollup.bucket_start > to_timestamp($1) - interval '1 day'
      AND rollup.bucket_start <= to_timestamp($2)
      AND rollup.bucket_start + make_interval(secs => rollup.bucket_secs) > to_timestamp($1)
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
            rollup.latest_reason, 'tunnel_reachability', 'automatic')
            ILIKE '%' || $8 || '%'
      )
),
series_sources AS (
    SELECT
        'tunnel_reachability'::text AS kind,
        series.plan_id,
        series.topology_identity_hash,
        series.interface_name,
        series.client_id,
        series.peer_client_id,
        series.id AS automatic_series_id,
        FALSE AS has_manual
    FROM automatic_physical_series series
    UNION ALL
    SELECT
        manual.kind,
        manual.plan_id,
        manual.topology_identity_hash,
        manual.interface_name,
        manual.client_id,
        manual.peer_client_id,
        NULL::bigint AS automatic_series_id,
        TRUE AS has_manual
    FROM manual_series manual
),
series_catalog AS MATERIALIZED (
    SELECT
        source.kind,
        source.plan_id,
        source.topology_identity_hash,
        source.interface_name,
        source.client_id,
        source.peer_client_id,
        COALESCE(
            array_agg(DISTINCT source.automatic_series_id)
                FILTER (WHERE source.automatic_series_id IS NOT NULL),
            ARRAY[]::bigint[]
        ) AS automatic_series_ids,
        bool_or(source.has_manual) AS has_manual
    FROM series_sources source
    GROUP BY source.kind, source.plan_id, source.topology_identity_hash,
             source.interface_name, source.client_id, source.peer_client_id
),
series_bounds AS MATERIALIZED (
    SELECT catalog.*,
           oldest.bucket_start AS oldest_bucket_start,
           oldest.bucket_secs AS oldest_bucket_secs,
           oldest.plan_name AS oldest_plan_name,
           oldest.latest_observed_at AS oldest_observed_at,
           latest.bucket_start AS latest_bucket_start,
           latest.bucket_secs AS latest_bucket_secs,
           latest.plan_name AS latest_plan_name,
           latest.latest_observed_at
    FROM series_catalog catalog
    CROSS JOIN LATERAL (
        SELECT retained_candidate.*
        FROM (
            SELECT retained.*
            FROM unnest(catalog.automatic_series_ids) physical(series_id)
            CROSS JOIN LATERAL (
                SELECT point.bucket_start, point.bucket_secs,
                       point.plan_name, point.latest_observed_at
                FROM retained_fragments point
                WHERE point.physical_series_id = physical.series_id
                ORDER BY point.bucket_start, point.bucket_secs,
                         point.latest_observed_at, point.plan_name
                LIMIT 1
            ) retained
            ORDER BY retained.bucket_start, retained.bucket_secs,
                     retained.latest_observed_at, retained.plan_name
            LIMIT 1
        ) retained_candidate
        UNION ALL
        SELECT pending.*
        FROM (
            SELECT point.bucket_start, point.bucket_secs,
                   point.plan_name, point.latest_observed_at
            FROM pending_fragments point
            WHERE point.physical_series_id = ANY(catalog.automatic_series_ids)
            ORDER BY point.bucket_start, point.bucket_secs,
                     point.latest_observed_at, point.plan_name
            LIMIT 1
        ) pending
        UNION ALL
        SELECT manual.*
        FROM (
            SELECT point.bucket_start, point.bucket_secs,
                   point.plan_name, point.latest_observed_at
            FROM manual_points point
            WHERE catalog.has_manual
              AND point.kind = catalog.kind
              AND point.plan_id = catalog.plan_id
              AND point.topology_identity_hash = catalog.topology_identity_hash
              AND point.interface_name = catalog.interface_name
              AND point.client_id = catalog.client_id
              AND point.peer_client_id = catalog.peer_client_id
            ORDER BY point.bucket_start, point.latest_observed_at,
                     point.plan_name
            LIMIT 1
        ) manual
        ORDER BY bucket_start, bucket_secs NULLS FIRST,
                 latest_observed_at, plan_name
        LIMIT 1
    ) oldest
    CROSS JOIN LATERAL (
        SELECT retained_candidate.*
        FROM (
            SELECT retained.*
            FROM unnest(catalog.automatic_series_ids) physical(series_id)
            CROSS JOIN LATERAL (
                SELECT point.bucket_start, point.bucket_secs,
                       point.plan_name, point.latest_observed_at
                FROM retained_fragments point
                WHERE point.physical_series_id = physical.series_id
                ORDER BY point.bucket_start DESC, point.latest_observed_at DESC,
                         point.bucket_secs DESC, point.plan_name
                LIMIT 1
            ) retained
            ORDER BY retained.bucket_start DESC,
                     retained.latest_observed_at DESC,
                     retained.bucket_secs DESC, retained.plan_name
            LIMIT 1
        ) retained_candidate
        UNION ALL
        SELECT pending.*
        FROM (
            SELECT point.bucket_start, point.bucket_secs,
                   point.plan_name, point.latest_observed_at
            FROM pending_fragments point
            WHERE point.physical_series_id = ANY(catalog.automatic_series_ids)
            ORDER BY point.bucket_start DESC, point.latest_observed_at DESC,
                     point.bucket_secs DESC, point.plan_name
            LIMIT 1
        ) pending
        UNION ALL
        SELECT manual.*
        FROM (
            SELECT point.bucket_start, point.bucket_secs,
                   point.plan_name, point.latest_observed_at
            FROM manual_points point
            WHERE catalog.has_manual
              AND point.kind = catalog.kind
              AND point.plan_id = catalog.plan_id
              AND point.topology_identity_hash = catalog.topology_identity_hash
              AND point.interface_name = catalog.interface_name
              AND point.client_id = catalog.client_id
              AND point.peer_client_id = catalog.peer_client_id
            ORDER BY point.bucket_start DESC, point.latest_observed_at DESC,
                     point.plan_name
            LIMIT 1
        ) manual
        ORDER BY bucket_start DESC, latest_observed_at DESC,
                 bucket_secs DESC NULLS LAST, plan_name
        LIMIT 1
    ) latest
),
ranked_series AS (
    SELECT bounds.*,
           row_number() OVER (
               ORDER BY bounds.latest_observed_at DESC, bounds.kind,
                        bounds.plan_id, bounds.topology_identity_hash,
                        bounds.interface_name, bounds.client_id,
                        bounds.peer_client_id
           ) AS series_ordinal,
           count(*) OVER () AS series_count
    FROM series_bounds bounds
),
budgeted_series AS MATERIALIZED (
    -- Every series receives one coordinate before any receives a second.
    -- With room for two, oldest and newest are mandatory; remaining slots are
    -- uniform interior probes. This is the only allocation compatible with a
    -- hard global response limit and fair series visibility.
    SELECT ranked.*,
           ($10::bigint / ranked.series_count)
           + CASE
               WHEN ranked.series_ordinal
                    <= ($10::bigint % ranked.series_count)
               THEN 1 ELSE 0
             END AS series_budget
    FROM ranked_series ranked
),
slot_numbers(slot) AS MATERIALIZED (
    SELECT 2::bigint
    WHERE COALESCE(
        (SELECT max(series_budget) FROM budgeted_series), 0
    ) >= 3
    UNION ALL
    SELECT slot + 1
    FROM slot_numbers
    WHERE slot + 1 < (
        SELECT max(series_budget) FROM budgeted_series
    )
),
endpoint_coordinates AS (
    SELECT series.series_ordinal,
           series.latest_bucket_start AS bucket_start,
           series.latest_bucket_secs AS bucket_secs,
           series.latest_plan_name AS plan_name
    FROM budgeted_series series
    WHERE series.series_budget = 1
    UNION ALL
    SELECT series.series_ordinal,
           series.oldest_bucket_start,
           series.oldest_bucket_secs,
           series.oldest_plan_name
    FROM budgeted_series series
    WHERE series.series_budget >= 2
    UNION ALL
    SELECT series.series_ordinal,
           series.latest_bucket_start,
           series.latest_bucket_secs,
           series.latest_plan_name
    FROM budgeted_series series
    WHERE series.series_budget >= 2
),
interior_targets AS (
    SELECT series.*,
           slot.slot,
           series.oldest_bucket_start
             + (series.latest_bucket_start - series.oldest_bucket_start)
               * ((slot.slot - 1)::double precision
                  / (series.series_budget - 1)::double precision) AS target_at
    FROM budgeted_series series
    JOIN slot_numbers slot ON slot.slot < series.series_budget
),
interior_coordinates AS (
    SELECT target.series_ordinal,
           nearest.bucket_start,
           nearest.bucket_secs,
           nearest.plan_name
    FROM interior_targets target
    CROSS JOIN LATERAL (
        SELECT candidate.*
        FROM (
            SELECT retained_before.*
            FROM (
                SELECT retained.*
                FROM unnest(target.automatic_series_ids) physical(series_id)
                CROSS JOIN LATERAL (
                    SELECT point.bucket_start, point.bucket_secs,
                           point.plan_name, point.latest_observed_at
                    FROM retained_fragments point
                    WHERE point.physical_series_id = physical.series_id
                      AND point.bucket_start <= target.target_at
                    ORDER BY point.bucket_start DESC,
                             point.latest_observed_at DESC,
                             point.bucket_secs DESC, point.plan_name
                    LIMIT 1
                ) retained
                ORDER BY retained.bucket_start DESC,
                         retained.latest_observed_at DESC,
                         retained.bucket_secs DESC, retained.plan_name
                LIMIT 1
            ) retained_before
            UNION ALL
            SELECT retained_after.*
            FROM (
                SELECT retained.*
                FROM unnest(target.automatic_series_ids) physical(series_id)
                CROSS JOIN LATERAL (
                    SELECT point.bucket_start, point.bucket_secs,
                           point.plan_name, point.latest_observed_at
                    FROM retained_fragments point
                    WHERE point.physical_series_id = physical.series_id
                      AND point.bucket_start >= target.target_at
                    ORDER BY point.bucket_start, point.bucket_secs,
                             point.latest_observed_at, point.plan_name
                    LIMIT 1
                ) retained
                ORDER BY retained.bucket_start, retained.bucket_secs,
                         retained.latest_observed_at, retained.plan_name
                LIMIT 1
            ) retained_after
            UNION ALL
            SELECT pending.*
            FROM (
                SELECT point.bucket_start, point.bucket_secs,
                       point.plan_name, point.latest_observed_at
                FROM pending_fragments point
                WHERE point.physical_series_id
                        = ANY(target.automatic_series_ids)
                  AND point.bucket_start <= target.target_at
                ORDER BY point.bucket_start DESC,
                         point.latest_observed_at DESC,
                         point.bucket_secs DESC, point.plan_name
                LIMIT 1
            ) pending
            UNION ALL
            SELECT pending.*
            FROM (
                SELECT point.bucket_start, point.bucket_secs,
                       point.plan_name, point.latest_observed_at
                FROM pending_fragments point
                WHERE point.physical_series_id
                        = ANY(target.automatic_series_ids)
                  AND point.bucket_start >= target.target_at
                ORDER BY point.bucket_start, point.bucket_secs,
                         point.latest_observed_at, point.plan_name
                LIMIT 1
            ) pending
            UNION ALL
            SELECT manual.*
            FROM (
                SELECT point.bucket_start, point.bucket_secs,
                       point.plan_name, point.latest_observed_at
                FROM manual_points point
                WHERE target.has_manual
                  AND point.kind = target.kind
                  AND point.plan_id = target.plan_id
                  AND point.topology_identity_hash
                        = target.topology_identity_hash
                  AND point.interface_name = target.interface_name
                  AND point.client_id = target.client_id
                  AND point.peer_client_id = target.peer_client_id
                  AND point.bucket_start <= target.target_at
                ORDER BY point.bucket_start DESC,
                         point.latest_observed_at DESC, point.plan_name
                LIMIT 1
            ) manual
            UNION ALL
            SELECT manual.*
            FROM (
                SELECT point.bucket_start, point.bucket_secs,
                       point.plan_name, point.latest_observed_at
                FROM manual_points point
                WHERE target.has_manual
                  AND point.kind = target.kind
                  AND point.plan_id = target.plan_id
                  AND point.topology_identity_hash
                        = target.topology_identity_hash
                  AND point.interface_name = target.interface_name
                  AND point.client_id = target.client_id
                  AND point.peer_client_id = target.peer_client_id
                  AND point.bucket_start >= target.target_at
                ORDER BY point.bucket_start, point.latest_observed_at,
                         point.plan_name
                LIMIT 1
            ) manual
        ) candidate
        ORDER BY
            abs(extract(epoch FROM (bucket_start - target.target_at))),
            bucket_start DESC, latest_observed_at DESC,
            bucket_secs DESC NULLS LAST, plan_name
        LIMIT 1
    ) nearest
),
selected_coordinates AS MATERIALIZED (
    SELECT DISTINCT series_ordinal, bucket_start, bucket_secs, plan_name
    FROM (
        SELECT * FROM endpoint_coordinates
        UNION ALL
        SELECT * FROM interior_coordinates
    ) coordinate
),
selected_fragments AS MATERIALIZED (
    SELECT point.*
    FROM selected_coordinates coordinate
    JOIN budgeted_series series USING (series_ordinal)
    JOIN manual_points point
      ON coordinate.bucket_secs IS NULL
     AND point.kind = series.kind
     AND point.plan_id = series.plan_id
     AND point.topology_identity_hash = series.topology_identity_hash
     AND point.interface_name = series.interface_name
     AND point.client_id = series.client_id
     AND point.peer_client_id = series.peer_client_id
     AND point.bucket_start = coordinate.bucket_start
     AND point.plan_name = coordinate.plan_name
    UNION ALL
    SELECT point.*
    FROM selected_coordinates coordinate
    JOIN budgeted_series series USING (series_ordinal)
    CROSS JOIN LATERAL unnest(series.automatic_series_ids)
        physical(series_id)
    CROSS JOIN LATERAL (
        -- Health is the final primary-key dimension, so one physical series
        -- has at most the two complete fragments selected here.
        SELECT retained.*
        FROM retained_fragments retained
        WHERE coordinate.bucket_secs IS NOT NULL
          AND retained.physical_series_id = physical.series_id
          AND retained.bucket_start = coordinate.bucket_start
          AND retained.bucket_secs = coordinate.bucket_secs
          AND retained.plan_name = coordinate.plan_name
        LIMIT 2
    ) point
    UNION ALL
    SELECT point.*
    FROM selected_coordinates coordinate
    JOIN budgeted_series series USING (series_ordinal)
    CROSS JOIN LATERAL unnest(series.automatic_series_ids)
        physical(series_id)
    CROSS JOIN LATERAL (
        SELECT pending.*
        FROM pending_fragments pending
        WHERE coordinate.bucket_secs IS NOT NULL
          AND pending.physical_series_id = physical.series_id
          AND pending.bucket_start = coordinate.bucket_start
          AND pending.bucket_secs = coordinate.bucket_secs
          AND pending.plan_name = coordinate.plan_name
        LIMIT 2
    ) point
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
        bool_or(retained) AS retained,
        sum(sample_count)::bigint AS sample_count,
        CASE WHEN bucket_secs IS NULL
             THEN sum(source_bucket_count)
             ELSE 1
        END::bigint AS source_bucket_count,
        max(effective_resolution_secs)::bigint AS effective_resolution_secs,
        sum(automatic_count)::bigint AS automatic_count,
        sum(manual_count)::bigint AS manual_count,
        sum(healthy_count)::bigint AS healthy_count,
        sum(degraded_count)::bigint AS degraded_count,
        sum(latency_sum_ms)::double precision AS latency_sum_ms,
        sum(latency_sample_count)::bigint AS latency_sample_count,
        min(latency_min_ms)::double precision AS latency_min_ms,
        max(latency_max_ms)::double precision AS latency_max_ms,
        sum(packet_loss_sum_ratio)::double precision AS packet_loss_sum_ratio,
        sum(packet_loss_sample_count)::bigint AS packet_loss_sample_count,
        sum(throughput_sum_mbps)::double precision AS throughput_sum_mbps,
        sum(throughput_sample_count)::bigint AS throughput_sample_count,
        max(throughput_max_mbps)::double precision AS throughput_max_mbps,
        LEAST(sum(bytes_total), 9223372036854775807::numeric)::bigint
            AS bytes_total,
        max(latest_observed_at)::text AS latest_observed_at
    FROM selected_fragments
    GROUP BY kind, plan_id, topology_identity_hash, plan_name, interface_name,
             client_id, peer_client_id, bucket_start, bucket_secs
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
FROM summarized
ORDER BY latest_observed_at DESC, kind, client_id, bucket_start DESC
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
ORDER BY rollup.bucket_start DESC, series.id, rollup.bucket_secs, rollup.health_state
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

const AUTOMATIC_TUNNEL_REACHABILITY_BATCH_SQL: &str = r#"
WITH incoming AS MATERIALIZED (
    SELECT element.input_ordinal::bigint AS input_ordinal, observation.*
    FROM jsonb_array_elements($1::jsonb)
        WITH ORDINALITY AS element(value, input_ordinal)
    CROSS JOIN LATERAL jsonb_to_record(element.value) AS observation (
        id uuid,
        sample_id uuid,
        accepted_seq bigint,
        payload_ordinal smallint,
        client_id text,
        plan_id uuid,
        topology_identity_hash text,
        plan_name text,
        interface_name text,
        peer_client_id text,
        target text,
        endpoint_side text,
        address_family text,
        stale_after_secs bigint,
        healthy boolean,
        transmitted integer,
        received integer,
        latency_min_ms double precision,
        latency_avg_ms double precision,
        latency_max_ms double precision,
        latency_mdev_ms double precision,
        packet_loss_ratio double precision,
        reason text,
        observed_at timestamptz,
        received_at timestamptz
    )
),
canonical AS MATERIALIZED (
    SELECT DISTINCT ON (id) incoming.*
    FROM incoming
    ORDER BY id, accepted_seq, payload_ordinal, input_ordinal
),
novel AS MATERIALIZED (
    SELECT canonical.*
    FROM canonical
    WHERE NOT EXISTS (
        SELECT 1
        FROM network_observations existing
        WHERE existing.id = canonical.id
    )
      AND NOT EXISTS (
        SELECT 1
        FROM network_observation_latest existing
        WHERE existing.observation_id = canonical.id
    )
      AND NOT EXISTS (
        SELECT 1
        FROM network_observations existing
        WHERE existing.source = 'automatic'
          AND existing.automatic_sample_id = canonical.sample_id
          AND existing.automatic_payload_ordinal = canonical.payload_ordinal
    )
),
series_keys AS MATERIALIZED (
    SELECT DISTINCT ON (
        plan_id, topology_identity_hash, client_id, peer_client_id,
        endpoint_side, address_family, interface_name, target
    )
        plan_id, topology_identity_hash, plan_name, interface_name,
        client_id, peer_client_id, endpoint_side, address_family, target
    FROM novel
    ORDER BY
        plan_id, topology_identity_hash, client_id, peer_client_id,
        endpoint_side, address_family, interface_name, target,
        accepted_seq DESC, payload_ordinal DESC, input_ordinal DESC
),
deactivated AS (
    UPDATE network_observation_series AS series
    SET active = FALSE
    FROM series_keys AS current
    WHERE series.plan_id = current.plan_id
      AND series.client_id = current.client_id
      AND series.endpoint_side = current.endpoint_side
      AND series.address_family = current.address_family
      AND series.topology_identity_hash <> current.topology_identity_hash
      AND series.active IS TRUE
    RETURNING series.id
),
deactivation_barrier AS MATERIALIZED (
    SELECT count(*)::bigint AS changed FROM deactivated
),
upserted_series AS (
    INSERT INTO network_observation_series (
        plan_id, topology_identity_hash, plan_name, interface_name,
        client_id, peer_client_id, endpoint_side, address_family, target
    )
    SELECT
        current.plan_id, current.topology_identity_hash, current.plan_name,
        current.interface_name, current.client_id, current.peer_client_id,
        current.endpoint_side, current.address_family, current.target
    FROM series_keys AS current
    CROSS JOIN deactivation_barrier
    ON CONFLICT (
        plan_id, topology_identity_hash, client_id, peer_client_id,
        endpoint_side, address_family, interface_name, target
    ) DO UPDATE SET
        plan_name = EXCLUDED.plan_name,
        active = TRUE,
        last_seen_at = now()
    RETURNING
        id, plan_id, topology_identity_hash, client_id, peer_client_id,
        endpoint_side, address_family, interface_name, target
),
resolved AS MATERIALIZED (
    SELECT novel.*, series.id AS series_id
    FROM novel
    JOIN upserted_series AS series
      ON series.plan_id = novel.plan_id
     AND series.topology_identity_hash = novel.topology_identity_hash
     AND series.client_id = novel.client_id
     AND series.peer_client_id = novel.peer_client_id
     AND series.endpoint_side = novel.endpoint_side
     AND series.address_family = novel.address_family
     AND series.interface_name = novel.interface_name
     AND series.target = novel.target
),
inserted AS (
    INSERT INTO network_observations (
        id, source, automatic_series_id, automatic_sample_id,
        automatic_payload_ordinal, plan_name, metadata,
        observed_at, received_at
    )
    SELECT
        id, 'automatic', series_id, sample_id, payload_ordinal,
        plan_name, '{}'::jsonb, observed_at, received_at
    FROM resolved
    ON CONFLICT DO NOTHING
    RETURNING id, automatic_series_id AS series_id
),
latest_candidates AS MATERIALIZED (
    SELECT DISTINCT ON (inserted.series_id)
        inserted.series_id, resolved.id, resolved.stale_after_secs,
        resolved.healthy, resolved.transmitted, resolved.received,
        latency_min_ms, latency_avg_ms, latency_max_ms, latency_mdev_ms,
        packet_loss_ratio, reason, observed_at, received_at
    FROM inserted
    JOIN resolved ON resolved.id = inserted.id
    ORDER BY inserted.series_id, observed_at DESC, resolved.id DESC
),
merged_latest AS (
    INSERT INTO network_observation_latest (
        series_id, observation_id, stale_after_secs, healthy,
        transmitted, received, latency_min_ms, latency_avg_ms,
        latency_max_ms, latency_mdev_ms, packet_loss_ratio, reason,
        metadata, observed_at, received_at
    )
    SELECT
        series_id, id, COALESCE(stale_after_secs, 180), healthy,
        COALESCE(transmitted, 0), COALESCE(received, 0),
        latency_min_ms, latency_avg_ms, latency_max_ms, latency_mdev_ms,
        COALESCE(packet_loss_ratio, 1.0), reason,
        jsonb_build_object('type', 'tunnel_reachability', 'source', 'automatic'),
        observed_at, received_at
    FROM latest_candidates
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
    RETURNING series_id
)
SELECT
    (SELECT count(*) FROM deactivated) AS deactivated_series,
    (SELECT count(*) FROM deactivated)
        + (SELECT count(*) FROM upserted_series)
        + (SELECT count(*) FROM inserted)
        + (SELECT count(*) FROM merged_latest) AS mutation_count
"#;

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
    let changed = result.rows_affected();
    Ok(changed)
}

/// Manual job evidence remains a value-bearing exact row. Automatic telemetry
/// uses the sparse locator branch above and never reaches this writer.
async fn insert_network_observation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    observation: &NetworkObservationView,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO network_observations (
            id, job_id, client_id, seq, kind, source, role, plan_id,
            topology_identity_hash, plan_name, interface_name, peer_client_id,
            target, endpoint_side, address_family, stale_after_secs, healthy,
            transmitted, received, latency_min_ms, latency_avg_ms, latency_max_ms,
            latency_mdev_ms, packet_loss_ratio, reason, throughput_mbps, bytes,
            metadata, observed_at, received_at
        ) SELECT
            $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
            $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,to_timestamp($29),to_timestamp($30)
        WHERE NOT EXISTS (
            SELECT 1
            FROM network_observation_latest latest
            WHERE latest.observation_id = $1
        )
        ON CONFLICT (job_id, client_id, seq)
            WHERE job_id IS NOT NULL AND seq IS NOT NULL
        DO UPDATE SET
            kind = EXCLUDED.kind, source = EXCLUDED.source,
            role = EXCLUDED.role, plan_id = EXCLUDED.plan_id,
            topology_identity_hash = EXCLUDED.topology_identity_hash,
            plan_name = EXCLUDED.plan_name,
            interface_name = EXCLUDED.interface_name,
            peer_client_id = EXCLUDED.peer_client_id,
            target = EXCLUDED.target, endpoint_side = EXCLUDED.endpoint_side,
            address_family = EXCLUDED.address_family,
            stale_after_secs = EXCLUDED.stale_after_secs,
            healthy = EXCLUDED.healthy, transmitted = EXCLUDED.transmitted,
            received = EXCLUDED.received,
            latency_min_ms = EXCLUDED.latency_min_ms,
            latency_avg_ms = EXCLUDED.latency_avg_ms,
            latency_max_ms = EXCLUDED.latency_max_ms,
            latency_mdev_ms = EXCLUDED.latency_mdev_ms,
            packet_loss_ratio = EXCLUDED.packet_loss_ratio,
            reason = EXCLUDED.reason,
            throughput_mbps = EXCLUDED.throughput_mbps,
            bytes = EXCLUDED.bytes, metadata = EXCLUDED.metadata,
            observed_at = EXCLUDED.observed_at,
            received_at = EXCLUDED.received_at
        "#,
    )
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

#[cfg(test)]
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
#[cfg(test)]
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
