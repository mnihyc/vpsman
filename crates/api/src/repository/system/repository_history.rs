use anyhow::{ensure, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_server_core::{
    preview_traffic_terminal_retention, process_traffic_terminal_retention_page,
};

use crate::{
    model::AuthContext,
    model_alert_policies::{TrafficCounterRollupRecord, TrafficCounterSampleRecord},
    model_history::{
        HistoryRetentionDomain, HistoryRetentionPolicyView, HistoryRetentionPruneOutcome,
        HistoryRetentionPrunePlan, UpsertHistoryRetentionPolicyRequest,
    },
    repository::Repository,
    repository_artifact_deletions::{
        finish_owned_artifact_deletion_in_tx, lock_owned_artifact_deletion_in_tx,
        ArtifactDeletionOwner,
    },
    unix_now,
};

const TELEMETRY_RETENTION_WAKE_CHANNEL: &str = "vpsman_telemetry_retention";

impl Repository {
    pub(crate) async fn list_history_retention_policies(
        &self,
    ) -> Result<Vec<HistoryRetentionPolicyView>> {
        let mut policies = HistoryRetentionDomain::ALL
            .iter()
            .copied()
            .map(default_policy)
            .collect::<Vec<_>>();
        let stored = match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        domain,
                        retention_days,
                        prune_limit,
                        enabled,
                        metadata_only,
                        export_enabled,
                        notes,
                        updated_by,
                        updated_at::text AS updated_at
                    FROM history_retention_policies
                    ORDER BY domain ASC
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(history_retention_policy_from_row)
                    .collect::<Result<Vec<_>>>()?
            }
        };
        for stored_policy in stored {
            if let Some(policy) = policies
                .iter_mut()
                .find(|policy| policy.domain == stored_policy.domain)
            {
                *policy = stored_policy;
            }
        }
        policies.sort_by(|left, right| left.domain.cmp(&right.domain));
        Ok(policies)
    }

    pub(crate) async fn upsert_history_retention_policy(
        &self,
        request: UpsertHistoryRetentionPolicyRequest,
        operator: &AuthContext,
    ) -> Result<HistoryRetentionPolicyView> {
        ensure!(
            request.confirmed,
            "history_retention_update_requires_confirmation"
        );
        let domain = HistoryRetentionDomain::from_str(&request.domain)
            .ok_or_else(|| anyhow::anyhow!("invalid_history_retention_domain"))?;
        let mut policy = self
            .list_history_retention_policies()
            .await?
            .into_iter()
            .find(|policy| policy.domain == domain.as_str())
            .unwrap_or_else(|| default_policy(domain));
        let previous_enabled = policy.enabled;
        let previous_retention_days = policy.retention_days;
        if let Some(retention_days) = request.retention_days {
            ensure!(
                (domain.minimum_retention_days()..=3650).contains(&retention_days),
                "history_retention_days_out_of_range"
            );
            policy.retention_days = retention_days;
        }
        if let Some(prune_limit) = request.prune_limit {
            ensure!(
                (1..=100_000).contains(&prune_limit),
                "history_prune_limit_out_of_range"
            );
            policy.prune_limit = prune_limit;
        }
        if let Some(enabled) = request.enabled {
            ensure!(
                enabled || domain.supports_disable(),
                "history_retention_domain_must_remain_enabled"
            );
            policy.enabled = enabled;
        }
        if let Some(metadata_only) = request.metadata_only {
            policy.metadata_only = metadata_only;
        }
        if let Some(export_enabled) = request.export_enabled {
            policy.export_enabled = export_enabled;
        }
        if request.clear_notes {
            policy.notes = None;
        } else if let Some(notes) = request.notes {
            let notes = notes.trim().to_string();
            ensure!(notes.len() <= 1000, "history_retention_notes_too_long");
            policy.notes = (!notes.is_empty()).then_some(notes);
        }
        policy.updated_by = Some(operator.operator.id);
        policy.updated_at = unix_now().to_string();
        policy.built_in_default = false;

        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO history_retention_policies (
                        domain,
                        retention_days,
                        prune_limit,
                        enabled,
                        metadata_only,
                        export_enabled,
                        notes,
                        updated_by
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                    ON CONFLICT (domain)
                    DO UPDATE SET
                        retention_days = EXCLUDED.retention_days,
                        prune_limit = EXCLUDED.prune_limit,
                        enabled = EXCLUDED.enabled,
                        metadata_only = EXCLUDED.metadata_only,
                        export_enabled = EXCLUDED.export_enabled,
                        notes = EXCLUDED.notes,
                        updated_by = EXCLUDED.updated_by,
                        updated_at = now()
                    RETURNING updated_at::text AS updated_at
                    "#,
                )
                .bind(&policy.domain)
                .bind(policy.retention_days)
                .bind(policy.prune_limit)
                .bind(policy.enabled)
                .bind(policy.metadata_only)
                .bind(policy.export_enabled)
                .bind(&policy.notes)
                .bind(operator.operator.id)
                .fetch_one(&mut *tx)
                .await?;
                policy.updated_at = row.try_get("updated_at")?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("history_retention.policy_updated")
                .bind(format!("history_retention:{}", policy.domain))
                .bind(Option::<String>::None)
                .bind(json!({
                    "domain": &policy.domain,
                    "retention_days": policy.retention_days,
                    "prune_limit": policy.prune_limit,
                    "enabled": policy.enabled,
                    "metadata_only": policy.metadata_only,
                    "export_enabled": policy.export_enabled,
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "history-retention-controller",
                }))
                .execute(&mut *tx)
                .await?;
                if history_retention_policy_advances_worker_frontier(
                    domain,
                    previous_enabled,
                    previous_retention_days,
                    policy.enabled,
                    policy.retention_days,
                ) {
                    sqlx::query("SELECT pg_notify($1, $2)")
                        .bind(TELEMETRY_RETENTION_WAKE_CHANNEL)
                        .bind(
                            json!({
                                "owner": "history_retention",
                                "effect": "retention_policy_changed",
                                "domain": domain.as_str(),
                            })
                            .to_string(),
                        )
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
            }
        }
        Ok(policy)
    }

    pub(crate) async fn prune_history_domain(
        &self,
        plan: &HistoryRetentionPrunePlan,
        cutoff_unix: u64,
        dry_run: bool,
    ) -> Result<HistoryRetentionPruneOutcome> {
        if !plan.enabled {
            return Ok(HistoryRetentionPruneOutcome {
                matched_rows: 0,
                pruned_rows: 0,
                object_keys: Vec::new(),
            });
        }
        match self {
            Self::Postgres(pool) => {
                prune_postgres_history_domain(
                    pool,
                    plan.domain,
                    cutoff_unix,
                    plan.prune_limit,
                    dry_run,
                )
                .await
            }
        }
    }

    pub(crate) async fn list_history_retention_object_candidates(
        &self,
        plan: &HistoryRetentionPrunePlan,
        cutoff_unix: u64,
    ) -> Result<Vec<HistoryRetentionObjectCandidate>> {
        if !plan.enabled {
            return Ok(Vec::new());
        }
        match self {
            Self::Postgres(pool) => {
                list_postgres_history_retention_object_candidates(
                    pool,
                    plan.domain,
                    cutoff_unix,
                    plan.prune_limit,
                )
                .await
            }
        }
    }

    pub(crate) async fn prune_history_retention_object_candidate(
        &self,
        candidate: &HistoryRetentionObjectCandidate,
    ) -> Result<i64> {
        match self {
            Self::Postgres(pool) => {
                prune_postgres_history_retention_object_candidate(pool, candidate).await
            }
        }
    }

    pub(crate) async fn prune_history_retention_object_candidates(
        &self,
        candidates: &[HistoryRetentionObjectCandidate],
    ) -> Result<i64> {
        let mut pruned_rows = 0_i64;
        for candidate in candidates {
            pruned_rows += self
                .prune_history_retention_object_candidate(candidate)
                .await?;
        }
        Ok(pruned_rows)
    }

    pub(crate) async fn finalize_history_retention_object_delete(
        &self,
        candidate: &HistoryRetentionObjectCandidate,
        owner: &ArtifactDeletionOwner,
    ) -> Result<i64> {
        ensure!(
            owner.source_kind == "history_retention"
                && owner.source_id == candidate.source_id()
                && owner.source_revision == candidate.source_revision()
                && owner.source_identity == candidate.deletion_identity()
                && candidate.object_key() == Some(owner.object_key.as_str()),
            "history-retention artifact deletion review changed"
        );
        match self {
            Self::Postgres(pool) => {
                finalize_postgres_history_retention_object_delete(pool, candidate, owner).await
            }
        }
    }

    pub(crate) async fn record_history_retention_prune_audit(
        &self,
        operator: &AuthContext,
        dry_run: bool,
        metadata_only: Option<bool>,
        domains: &[serde_json::Value],
    ) -> Result<()> {
        let result = if dry_run {
            "previewed"
        } else if domains.iter().any(|domain| {
            domain.get("status").and_then(serde_json::Value::as_str) == Some("partial_error")
        }) {
            "partial"
        } else {
            "succeeded"
        };
        let metadata = json!({
            "dry_run": dry_run,
            "metadata_only_requested": metadata_only,
            "domains": domains,
            "result": result,
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "operator_request",
            "component": "history-retention-controller",
        });
        match self {
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("history_retention.pruned")
                .bind("history_retention")
                .bind(Option::<String>::None)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn export_traffic_counter_samples(
        &self,
        limit: i64,
        client_id: Option<&str>,
    ) -> Result<Vec<TrafficCounterSampleRecord>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH policy_clients AS MATERIALIZED (
                        SELECT client.id
                        FROM clients client
                        WHERE $1::TEXT IS NULL OR client.id = $1
                    ), resolved_interface_policies AS MATERIALIZED (
                        SELECT policy.*
                        FROM public.resolve_telemetry_interface_policies(ARRAY(
                            SELECT client.id
                            FROM policy_clients client
                            ORDER BY client.id
                        )) policy
                    )
                    SELECT
                        sample.client_id,
                        sample.source_kind,
                        sample.interface,
                        sample.observed_at::text AS observed_at,
                        EXTRACT(EPOCH FROM sample.observed_at)::bigint AS observed_unix,
                        sample.rx_bytes,
                        sample.tx_bytes,
                        sample.rx_counter_epoch,
                        sample.tx_counter_epoch,
                        sample.sample_source
                    FROM traffic_counter_samples sample
                    JOIN resolved_interface_policies policy
                      ON policy.client_id = sample.client_id
                    WHERE ($1::text IS NULL OR sample.client_id = $1)
                      AND public.telemetry_interface_is_admitted_resolved(
                          policy.admission_mode,
                          policy.interface_patterns,
                          policy.managed_tunnel_interfaces,
                          sample.source_kind,
                          sample.interface
                      )
                    ORDER BY sample.observed_at DESC, sample.client_id ASC,
                             sample.source_kind ASC, sample.interface ASC
                    LIMIT $2
                    "#,
                )
                .bind(client_id)
                .bind(limit.clamp(1, 1000))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TrafficCounterSampleRecord {
                            client_id: row.try_get("client_id")?,
                            source_kind: row.try_get("source_kind")?,
                            interface: row.try_get("interface")?,
                            observed_at: row.try_get("observed_at")?,
                            observed_unix: row.try_get("observed_unix")?,
                            rx_bytes: row.try_get("rx_bytes")?,
                            tx_bytes: row.try_get("tx_bytes")?,
                            rx_counter_epoch: row.try_get("rx_counter_epoch")?,
                            tx_counter_epoch: row.try_get("tx_counter_epoch")?,
                            sample_source: row.try_get("sample_source")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn export_traffic_counter_rollups(
        &self,
        limit: i64,
        client_id: Option<&str>,
    ) -> Result<Vec<TrafficCounterRollupRecord>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH policy_clients AS MATERIALIZED (
                        SELECT client.id
                        FROM clients client
                        WHERE $1::TEXT IS NULL OR client.id = $1
                    ), resolved_interface_policies AS MATERIALIZED (
                        SELECT policy.*
                        FROM public.resolve_telemetry_interface_policies(ARRAY(
                            SELECT client.id
                            FROM policy_clients client
                            ORDER BY client.id
                        )) policy
                    )
                    SELECT
                        rollup.client_id, rollup.source_kind, rollup.interface,
                        rollup.origin_kind,
                        rollup.bucket_start::text AS bucket_start,
                        extract(epoch FROM rollup.bucket_start)::bigint
                            AS bucket_start_unix,
                        rollup.bucket_secs, rollup.rx_bytes, rollup.tx_bytes,
                        rollup.rx_valid_count, rollup.tx_valid_count,
                        rollup.any_valid_count, rollup.rx_reset_count,
                        rollup.tx_reset_count, rollup.any_reset_count,
                        extract(epoch FROM rollup.first_observed_at)::bigint
                            AS first_observed_unix,
                        extract(epoch FROM rollup.latest_observed_at)::bigint
                            AS latest_observed_unix
                    FROM traffic_counter_rollups rollup
                    JOIN resolved_interface_policies policy
                      ON policy.client_id = rollup.client_id
                    WHERE ($1::text IS NULL OR rollup.client_id = $1)
                      AND public.telemetry_interface_is_admitted_resolved(
                          policy.admission_mode,
                          policy.interface_patterns,
                          policy.managed_tunnel_interfaces,
                          rollup.source_kind,
                          rollup.interface
                      )
                    ORDER BY rollup.bucket_start DESC, rollup.client_id,
                             rollup.source_kind, rollup.interface,
                             rollup.origin_kind, rollup.bucket_secs
                    LIMIT $2
                    "#,
                )
                .bind(client_id)
                .bind(limit.clamp(1, 1000))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TrafficCounterRollupRecord {
                            client_id: row.try_get("client_id")?,
                            source_kind: row.try_get("source_kind")?,
                            interface: row.try_get("interface")?,
                            origin_kind: row.try_get("origin_kind")?,
                            bucket_start: row.try_get("bucket_start")?,
                            bucket_start_unix: row.try_get("bucket_start_unix")?,
                            bucket_secs: row.try_get("bucket_secs")?,
                            rx_bytes: row.try_get("rx_bytes")?,
                            tx_bytes: row.try_get("tx_bytes")?,
                            rx_valid_count: row.try_get("rx_valid_count")?,
                            tx_valid_count: row.try_get("tx_valid_count")?,
                            any_valid_count: row.try_get("any_valid_count")?,
                            rx_reset_count: row.try_get("rx_reset_count")?,
                            tx_reset_count: row.try_get("tx_reset_count")?,
                            any_reset_count: row.try_get("any_reset_count")?,
                            first_observed_unix: row.try_get("first_observed_unix")?,
                            latest_observed_unix: row.try_get("latest_observed_unix")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn export_job_outputs(
        &self,
        limit: i64,
        client_id: Option<&str>,
        job_id: Option<Uuid>,
    ) -> Result<Vec<serde_json::Value>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        seq,
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done,
                        received_at::text AS received_at,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE ($1::TEXT IS NULL OR client_id = $1)
                      AND ($2::UUID IS NULL OR job_id = $2)
                    ORDER BY created_at DESC, job_id DESC, client_id ASC, seq ASC
                    LIMIT $3
                    "#,
                )
                .bind(client_id)
                .bind(job_id)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(json!({
                            "job_id": row.try_get::<Uuid, _>("job_id")?,
                            "client_id": row.try_get::<String, _>("client_id")?,
                            "seq": row.try_get::<i32, _>("seq")?,
                            "stream": row.try_get::<String, _>("stream")?,
                            "data_base64": BASE64.encode(data),
                            "storage": row.try_get::<String, _>("storage")?,
                            "artifact_object_key": row.try_get::<Option<String>, _>("object_key")?,
                            "artifact_sha256_hex": row.try_get::<Option<String>, _>("data_sha256_hex")?,
                            "artifact_size_bytes": row.try_get::<Option<i64>, _>("data_size_bytes")?,
                            "exit_code": row.try_get::<Option<i32>, _>("exit_code")?,
                            "done": row.try_get::<bool, _>("done")?,
                            "received_at": row.try_get::<Option<String>, _>("received_at")?,
                            "created_at": row.try_get::<String, _>("created_at")?,
                        }))
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn export_client_status_history(
        &self,
        limit: i64,
        client_id: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        client_id,
                        from_status,
                        to_status,
                        reason,
                        metadata,
                        created_at::text AS created_at
                    FROM client_status_history
                    WHERE ($1::TEXT IS NULL OR client_id = $1)
                    ORDER BY created_at DESC, id DESC
                    LIMIT $2
                    "#,
                )
                .bind(client_id)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let metadata: serde_json::Value = row.try_get("metadata")?;
                        Ok(json!({
                            "id": row.try_get::<Uuid, _>("id")?,
                            "client_id": row.try_get::<String, _>("client_id")?,
                            "from_status": row.try_get::<Option<String>, _>("from_status")?,
                            "to_status": row.try_get::<String, _>("to_status")?,
                            "reason": row.try_get::<String, _>("reason")?,
                            "metadata": metadata,
                            "created_at": row.try_get::<String, _>("created_at")?,
                        }))
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn export_gateway_sessions(
        &self,
        limit: i64,
        client_id: Option<&str>,
    ) -> Result<Vec<serde_json::Value>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        gateway_id,
                        client_id,
                        noise_public_key_hex,
                        status,
                        started_at::text AS started_at,
                        last_seen_at::text AS last_seen_at,
                        ended_at::text AS ended_at,
                        end_reason
                    FROM gateway_sessions
                    WHERE ($1::TEXT IS NULL OR client_id = $1)
                    ORDER BY last_seen_at DESC, id DESC
                    LIMIT $2
                    "#,
                )
                .bind(client_id)
                .bind(limit.clamp(1, 200))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(json!({
                            "id": row.try_get::<Uuid, _>("id")?,
                            "gateway_id": row.try_get::<String, _>("gateway_id")?,
                            "client_id": row.try_get::<String, _>("client_id")?,
                            "noise_public_key_hex": row.try_get::<Option<String>, _>("noise_public_key_hex")?,
                            "status": row.try_get::<String, _>("status")?,
                            "started_at": row.try_get::<String, _>("started_at")?,
                            "last_seen_at": row.try_get::<String, _>("last_seen_at")?,
                            "ended_at": row.try_get::<Option<String>, _>("ended_at")?,
                            "end_reason": row.try_get::<Option<String>, _>("end_reason")?,
                        }))
                    })
                    .collect()
            }
        }
    }
}

fn history_retention_policy_advances_worker_frontier(
    domain: HistoryRetentionDomain,
    previous_enabled: bool,
    previous_retention_days: i32,
    next_enabled: bool,
    next_retention_days: i32,
) -> bool {
    let worker_owned_domain = matches!(
        domain,
        HistoryRetentionDomain::SystemMetricRollups
            | HistoryRetentionDomain::TelemetryRollups
            | HistoryRetentionDomain::TelemetryNetworkRates
            | HistoryRetentionDomain::TelemetryPingRollups
            | HistoryRetentionDomain::TrafficCounterRollups
            | HistoryRetentionDomain::NetworkObservations
    );
    worker_owned_domain
        && next_enabled
        && (!previous_enabled || next_retention_days < previous_retention_days)
}

fn default_policy(domain: HistoryRetentionDomain) -> HistoryRetentionPolicyView {
    HistoryRetentionPolicyView {
        domain: domain.as_str().to_string(),
        retention_days: domain.default_retention_days(),
        prune_limit: domain.default_prune_limit(),
        enabled: true,
        metadata_only: false,
        export_enabled: true,
        notes: None,
        updated_by: None,
        updated_at: unix_now().to_string(),
        built_in_default: true,
    }
}

fn history_retention_policy_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<HistoryRetentionPolicyView> {
    Ok(HistoryRetentionPolicyView {
        domain: row.try_get("domain")?,
        retention_days: row.try_get("retention_days")?,
        prune_limit: row.try_get("prune_limit")?,
        enabled: row.try_get("enabled")?,
        metadata_only: row.try_get("metadata_only")?,
        export_enabled: row.try_get("export_enabled")?,
        notes: row.try_get("notes")?,
        updated_by: row.try_get("updated_by")?,
        updated_at: row.try_get("updated_at")?,
        built_in_default: false,
    })
}

#[derive(Clone, Debug)]
pub(crate) enum HistoryRetentionObjectCandidate {
    JobOutput {
        job_id: Uuid,
        client_id: String,
        seq: i32,
        object_key: Option<String>,
    },
}

impl HistoryRetentionObjectCandidate {
    pub(crate) fn object_key(&self) -> Option<&str> {
        match self {
            Self::JobOutput { object_key, .. } => object_key.as_deref(),
        }
    }

    pub(crate) fn source_id(&self) -> Uuid {
        match self {
            Self::JobOutput { job_id, .. } => *job_id,
        }
    }

    pub(crate) fn source_revision(&self) -> i64 {
        match self {
            Self::JobOutput { seq, .. } => i64::from(*seq).saturating_add(1).max(1),
        }
    }

    pub(crate) fn deletion_identity(&self) -> serde_json::Value {
        match self {
            Self::JobOutput {
                job_id,
                client_id,
                seq,
                object_key,
            } => json!({
                "job_id": job_id,
                "client_id": client_id,
                "seq": seq,
                "object_key": object_key,
            }),
        }
    }
}

async fn list_postgres_history_retention_object_candidates(
    pool: &sqlx::PgPool,
    domain: HistoryRetentionDomain,
    cutoff_unix: u64,
    limit: i32,
) -> Result<Vec<HistoryRetentionObjectCandidate>> {
    match domain {
        HistoryRetentionDomain::JobOutputs => {
            let rows = sqlx::query(
                r#"
                SELECT job_id, client_id, seq, object_key
                FROM job_outputs
                WHERE created_at < to_timestamp($1)
                ORDER BY created_at ASC, job_id ASC, client_id ASC, seq ASC
                LIMIT $2
                "#,
            )
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?;
            rows.into_iter()
                .map(|row| {
                    Ok(HistoryRetentionObjectCandidate::JobOutput {
                        job_id: row.try_get("job_id")?,
                        client_id: row.try_get("client_id")?,
                        seq: row.try_get("seq")?,
                        object_key: row.try_get("object_key")?,
                    })
                })
                .collect()
        }
        _ => Ok(Vec::new()),
    }
}

async fn prune_postgres_history_retention_object_candidate(
    pool: &sqlx::PgPool,
    candidate: &HistoryRetentionObjectCandidate,
) -> Result<i64> {
    match candidate {
        HistoryRetentionObjectCandidate::JobOutput {
            job_id,
            client_id,
            seq,
            ..
        } => sqlx::query_scalar::<_, i64>(
            r#"
                WITH deleted_outputs AS (
                    DELETE FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                      AND seq = $3
                    RETURNING object_key
                )
                SELECT count(*)::bigint FROM deleted_outputs
                "#,
        )
        .bind(job_id)
        .bind(client_id)
        .bind(seq)
        .fetch_one(pool)
        .await
        .map_err(Into::into),
    }
}

async fn finalize_postgres_history_retention_object_delete(
    pool: &sqlx::PgPool,
    candidate: &HistoryRetentionObjectCandidate,
    owner: &ArtifactDeletionOwner,
) -> Result<i64> {
    match candidate {
        HistoryRetentionObjectCandidate::JobOutput {
            job_id,
            client_id,
            seq,
            object_key,
        } => {
            let Some(object_key) = object_key else {
                return prune_postgres_history_retention_object_candidate(pool, candidate).await;
            };
            let mut tx = pool.begin().await?;
            ensure!(
                lock_owned_artifact_deletion_in_tx(&mut tx, owner).await?,
                "artifact deletion ownership lost before finalization"
            );
            let pruned_rows = sqlx::query_scalar::<_, i64>(
                r#"
                WITH deleted_outputs AS (
                    DELETE FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                      AND seq = $3
                      AND object_key = $4
                    RETURNING object_key
                )
                SELECT count(*)::bigint FROM deleted_outputs
                "#,
            )
            .bind(job_id)
            .bind(client_id)
            .bind(seq)
            .bind(object_key)
            .fetch_one(&mut *tx)
            .await?;
            ensure!(
                finish_owned_artifact_deletion_in_tx(&mut tx, owner).await?,
                "artifact deletion ownership lost during finalization"
            );
            tx.commit().await?;
            Ok(pruned_rows)
        }
    }
}

async fn prune_postgres_history_domain(
    pool: &sqlx::PgPool,
    domain: HistoryRetentionDomain,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    match (domain, dry_run) {
        (HistoryRetentionDomain::AuditLogs, true) => {
            select_id_count(
                pool,
                "audit_logs",
                "created_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryRetentionDomain::AuditLogs, false) => {
            delete_by_id(
                pool,
                "audit_logs",
                "created_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryRetentionDomain::TelemetryRollups, true) => {
            prune_telemetry_rollups(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::TelemetryRollups, false) => {
            prune_telemetry_rollups(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::TelemetryNetworkRates, true) => {
            prune_telemetry_network_rates(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::TelemetryNetworkRates, false) => {
            prune_telemetry_network_rates(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::TelemetryPingRollups, true) => {
            prune_telemetry_ping_rollups(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::TelemetryPingRollups, false) => {
            prune_telemetry_ping_rollups(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::TrafficCounterRollups, true) => {
            prune_traffic_rollups(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::TrafficCounterRollups, false) => {
            prune_traffic_rollups(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::SystemMetricRollups, true) => {
            prune_system_metric_rollups(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::SystemMetricRollups, false) => {
            prune_system_metric_rollups(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::JobOutputs, true) => {
            prune_job_outputs(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::JobOutputs, false) => {
            prune_job_outputs(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::NetworkObservations, true) => {
            prune_network_observation_history(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::NetworkObservations, false) => {
            prune_network_observation_history(pool, cutoff_unix, limit, false).await
        }
        (HistoryRetentionDomain::ClientStatusHistory, true) => {
            select_id_count(
                pool,
                "client_status_history",
                "created_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryRetentionDomain::ClientStatusHistory, false) => {
            delete_by_id(
                pool,
                "client_status_history",
                "created_at",
                "id",
                "TRUE",
                cutoff_unix,
                limit,
            )
            .await
        }
        (HistoryRetentionDomain::GatewaySessions, true) => {
            prune_gateway_sessions(pool, cutoff_unix, limit, true).await
        }
        (HistoryRetentionDomain::GatewaySessions, false) => {
            prune_gateway_sessions(pool, cutoff_unix, limit, false).await
        }
    }
}

async fn select_id_count(
    pool: &sqlx::PgPool,
    table: &str,
    timestamp_column: &str,
    id_column: &str,
    filter: &str,
    cutoff_unix: u64,
    limit: i32,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = format!(
        r#"
        SELECT {id_column}
        FROM {table}
        WHERE {timestamp_column} < to_timestamp($1)
          AND {filter}
        ORDER BY {timestamp_column} ASC, {id_column} ASC
        LIMIT $2
        "#
    );
    let rows = sqlx::query(&query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: 0,
        object_keys: Vec::new(),
    })
}

async fn delete_by_id(
    pool: &sqlx::PgPool,
    table: &str,
    timestamp_column: &str,
    id_column: &str,
    filter: &str,
    cutoff_unix: u64,
    limit: i32,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = format!(
        r#"
        WITH doomed AS (
            SELECT {id_column}
            FROM {table}
            WHERE {timestamp_column} < to_timestamp($1)
              AND {filter}
            ORDER BY {timestamp_column} ASC, {id_column} ASC
            LIMIT $2
        )
        DELETE FROM {table} target_row
        USING doomed
        WHERE target_row.{id_column} = doomed.{id_column}
        RETURNING target_row.{id_column}
        "#
    );
    let rows = sqlx::query(&query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: rows.len() as i64,
        object_keys: Vec::new(),
    })
}

async fn prune_network_observation_history(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    // One history item is either an exact observation or one physical retained
    // health component. The shared limit remains a hard upper bound. Latest
    // snapshots and empty inactive-series metadata are current-state lifecycle
    // rows; their dedicated two-day worker is the sole deletion owner.
    let query = if dry_run {
        r#"
        WITH candidates AS (
            SELECT *
            FROM (
                SELECT 0 AS source_kind,
                       observation.observed_at AS source_time,
                       observation.id::text AS source_key
                FROM network_observations observation
                WHERE observation.source = 'manual'
                  AND observation.observed_at < to_timestamp($1)
                UNION ALL
                SELECT 1 AS source_kind,
                       observation.observed_at AS source_time,
                       observation.id::text AS source_key
                FROM network_observations observation
                WHERE observation.source = 'automatic'
                  AND observation.observed_at < to_timestamp($1)
                UNION ALL
                SELECT 2 AS source_kind,
                       rollup.bucket_start AS source_time,
                       concat_ws(':', rollup.series_id, rollup.bucket_secs,
                           extract(epoch FROM rollup.bucket_start)::bigint,
                           rollup.health_state) AS source_key
                FROM network_observation_rollups rollup
                WHERE rollup.bucket_start < to_timestamp($1)
                  AND rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
                    <= to_timestamp($1)
            ) eligible
            ORDER BY source_time, source_kind, source_key
            LIMIT $2
        )
        SELECT source_key FROM candidates
        "#
    } else {
        r#"
        WITH candidates AS MATERIALIZED (
            SELECT source_kind, observation_id, series_id, bucket_secs,
                   bucket_start, health_state
            FROM (
                SELECT 0 AS source_kind,
                       observation.observed_at AS source_time,
                       observation.id AS observation_id,
                       NULL::bigint AS series_id,
                       NULL::integer AS bucket_secs,
                       NULL::timestamptz AS bucket_start,
                       NULL::smallint AS health_state
                FROM network_observations observation
                WHERE observation.source = 'manual'
                  AND observation.observed_at < to_timestamp($1)
                UNION ALL
                SELECT 1 AS source_kind,
                       observation.observed_at AS source_time,
                       observation.id AS observation_id,
                       NULL::bigint AS series_id,
                       NULL::integer AS bucket_secs,
                       NULL::timestamptz AS bucket_start,
                       NULL::smallint AS health_state
                FROM network_observations observation
                WHERE observation.source = 'automatic'
                  AND observation.observed_at < to_timestamp($1)
                UNION ALL
                SELECT 2 AS source_kind,
                       rollup.bucket_start AS source_time,
                       NULL::uuid AS observation_id,
                       rollup.series_id,
                       rollup.bucket_secs,
                       rollup.bucket_start,
                       rollup.health_state
                FROM network_observation_rollups rollup
                WHERE rollup.bucket_start < to_timestamp($1)
                  AND rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
                    <= to_timestamp($1)
            ) eligible
            ORDER BY source_time, source_kind,
                     observation_id, series_id, bucket_secs, bucket_start,
                     health_state
            LIMIT $2
        ),
        deleted_manual AS (
            DELETE FROM network_observations observation
            USING candidates
            WHERE candidates.source_kind = 0
              AND observation.id = candidates.observation_id
            RETURNING observation.id
        ),
        deleted_automatic AS (
            DELETE FROM network_observations observation
            USING candidates
            WHERE candidates.source_kind = 1
              AND observation.id = candidates.observation_id
            RETURNING observation.id
        ),
        deleted_tiered AS (
            DELETE FROM network_observation_rollups rollup
            USING candidates
            WHERE candidates.source_kind = 2
              AND rollup.series_id = candidates.series_id
              AND rollup.bucket_secs = candidates.bucket_secs
              AND rollup.bucket_start = candidates.bucket_start
              AND rollup.health_state = candidates.health_state
            RETURNING rollup.series_id
        )
        SELECT id::text AS source_key
        FROM deleted_manual
        UNION ALL
        SELECT id::text AS source_key
        FROM deleted_automatic
        UNION ALL
        SELECT series_id::text AS source_key
        FROM deleted_tiered
        "#
    };
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_telemetry_rollups(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT client_id, bucket_secs, bucket_start
        FROM telemetry_rollups
        WHERE bucket_start < to_timestamp($1)
          AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1)) <= to_timestamp($1)
        ORDER BY bucket_start ASC, client_id ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT client_id, bucket_secs, bucket_start
            FROM telemetry_rollups
            WHERE bucket_start < to_timestamp($1)
              AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1)) <= to_timestamp($1)
            ORDER BY bucket_start ASC, client_id ASC
            LIMIT $2
        )
        DELETE FROM telemetry_rollups rollup
        USING doomed
        WHERE rollup.client_id = doomed.client_id
          AND rollup.bucket_secs = doomed.bucket_secs
          AND rollup.bucket_start = doomed.bucket_start
        RETURNING rollup.client_id
        "#
    };
    let rows = if dry_run {
        sqlx::query(query)
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?
    } else {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL vpsman.telemetry_history_compaction = 'on'")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(query)
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        rows
    };
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_system_metric_rollups(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT metric, bucket_secs, bucket_start
        FROM system_metric_rollups
        WHERE bucket_start < to_timestamp($1)
          AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1))
            <= to_timestamp($1)
        ORDER BY bucket_start ASC, metric ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT metric, bucket_secs, bucket_start
            FROM system_metric_rollups
            WHERE bucket_start < to_timestamp($1)
              AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1))
                <= to_timestamp($1)
            ORDER BY bucket_start ASC, metric ASC
            LIMIT $2
        )
        DELETE FROM system_metric_rollups rollup
        USING doomed
        WHERE rollup.metric = doomed.metric
          AND rollup.bucket_secs = doomed.bucket_secs
          AND rollup.bucket_start = doomed.bucket_start
        RETURNING rollup.metric
        "#
    };
    let rows = if dry_run {
        sqlx::query(query)
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?
    } else {
        let mut tx = pool.begin().await?;
        let rows = sqlx::query(query)
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        rows
    };
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_telemetry_network_rates(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT client_id, interface, bucket_secs, bucket_start
        FROM telemetry_network_rates
        WHERE bucket_start < to_timestamp($1)
          AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1)) <= to_timestamp($1)
        ORDER BY bucket_start ASC, client_id ASC, interface ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT client_id, interface, bucket_secs, bucket_start
            FROM telemetry_network_rates
            WHERE bucket_start < to_timestamp($1)
              AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1)) <= to_timestamp($1)
            ORDER BY bucket_start ASC, client_id ASC, interface ASC
            LIMIT $2
        )
        DELETE FROM telemetry_network_rates rate
        USING doomed
        WHERE rate.client_id = doomed.client_id
          AND rate.interface = doomed.interface
          AND rate.bucket_secs = doomed.bucket_secs
          AND rate.bucket_start = doomed.bucket_start
        RETURNING rate.client_id
        "#
    };
    let rows = if dry_run {
        sqlx::query(query)
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?
    } else {
        let mut tx = pool.begin().await?;
        sqlx::query("SET LOCAL vpsman.telemetry_history_compaction = 'on'")
            .execute(&mut *tx)
            .await?;
        let rows = sqlx::query(query)
            .bind(cutoff_unix as i64)
            .bind(limit)
            .fetch_all(&mut *tx)
            .await?;
        tx.commit().await?;
        rows
    };
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_telemetry_ping_rollups(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT series_id, bucket_secs, bucket_start
        FROM telemetry_ping_rollups
        WHERE bucket_start < to_timestamp($1)
          AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1)) <= to_timestamp($1)
        ORDER BY bucket_start ASC, series_id ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT series_id, bucket_secs, bucket_start
            FROM telemetry_ping_rollups
            WHERE bucket_start < to_timestamp($1)
              AND bucket_start + make_interval(secs => GREATEST(bucket_secs, 1)) <= to_timestamp($1)
            ORDER BY bucket_start ASC, series_id ASC
            LIMIT $2
        )
        DELETE FROM telemetry_ping_rollups rollup
        USING doomed
        WHERE rollup.series_id = doomed.series_id
          AND rollup.bucket_secs = doomed.bucket_secs
          AND rollup.bucket_start = doomed.bucket_start
        RETURNING rollup.series_id
        "#
    };
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_traffic_rollups(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let cutoff_unix = i64::try_from(cutoff_unix)?;
    if dry_run {
        return Ok(HistoryRetentionPruneOutcome {
            matched_rows: preview_traffic_terminal_retention(pool, cutoff_unix, limit).await?,
            pruned_rows: 0,
            object_keys: Vec::new(),
        });
    }

    let mut pruned_rows = 0_i64;
    let mut remaining = limit;
    while remaining > 0 {
        let page = process_traffic_terminal_retention_page(pool, cutoff_unix, remaining).await?;
        if !page.attempted {
            break;
        }
        let page_rows = i64::try_from(page.pruned_rows)?;
        ensure!(
            page_rows <= i64::from(remaining),
            "traffic terminal owner exceeded the request prune limit"
        );
        pruned_rows += page_rows;
        remaining -= i32::try_from(page_rows)?;
        if page_rows == 0 {
            // A concurrent client owner was unavailable. Return promptly; the
            // durable cursor keeps this stream reachable for the next call.
            break;
        }
    }
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: pruned_rows,
        pruned_rows,
        object_keys: Vec::new(),
    })
}

async fn prune_gateway_sessions(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT id
        FROM gateway_sessions
        WHERE status <> 'active'
          AND COALESCE(ended_at, last_seen_at) < to_timestamp($1)
        ORDER BY COALESCE(ended_at, last_seen_at) ASC, id ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT id
            FROM gateway_sessions
            WHERE status <> 'active'
              AND COALESCE(ended_at, last_seen_at) < to_timestamp($1)
            ORDER BY COALESCE(ended_at, last_seen_at) ASC, id ASC
            LIMIT $2
        )
        DELETE FROM gateway_sessions session
        USING doomed
        WHERE session.id = doomed.id
          AND session.status <> 'active'
        RETURNING session.id
        "#
    };
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys: Vec::new(),
    })
}

async fn prune_job_outputs(
    pool: &sqlx::PgPool,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let query = if dry_run {
        r#"
        SELECT object_key
        FROM job_outputs
        WHERE created_at < to_timestamp($1)
        ORDER BY created_at ASC, job_id ASC, client_id ASC, seq ASC
        LIMIT $2
        "#
    } else {
        r#"
        WITH doomed AS (
            SELECT job_id, client_id, seq, object_key
            FROM job_outputs
            WHERE created_at < to_timestamp($1)
            ORDER BY created_at ASC, job_id ASC, client_id ASC, seq ASC
            LIMIT $2
        )
        DELETE FROM job_outputs output
        USING doomed
        WHERE output.job_id = doomed.job_id
          AND output.client_id = doomed.client_id
          AND output.seq = doomed.seq
        RETURNING output.object_key
        "#
    };
    object_key_outcome(pool, query, cutoff_unix, limit, dry_run).await
}

async fn object_key_outcome(
    pool: &sqlx::PgPool,
    query: &str,
    cutoff_unix: u64,
    limit: i32,
    dry_run: bool,
) -> Result<HistoryRetentionPruneOutcome> {
    let rows = sqlx::query(query)
        .bind(cutoff_unix as i64)
        .bind(limit)
        .fetch_all(pool)
        .await?;
    let object_keys = rows
        .iter()
        .filter_map(|row| {
            row.try_get::<Option<String>, _>("object_key")
                .ok()
                .flatten()
        })
        .collect::<Vec<_>>();
    Ok(HistoryRetentionPruneOutcome {
        matched_rows: rows.len() as i64,
        pruned_rows: if dry_run { 0 } else { rows.len() as i64 },
        object_keys,
    })
}

#[cfg(test)]
mod dashboard_retention_provenance_contract_tests {
    use super::*;

    fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let (_, tail) = source.split_once(start).expect("section start");
        let (body, _) = tail.split_once(end).expect("section end");
        body
    }

    #[test]
    fn explicit_telemetry_prunes_mark_only_applied_resource_and_network_deletes() {
        let source = include_str!("repository_history.rs");
        let (runtime, _) = source
            .split_once("#[cfg(test)]\nmod dashboard_retention_provenance_contract_tests")
            .expect("history repository test boundary");
        let marker = "SET LOCAL vpsman.telemetry_history_compaction = 'on'";
        assert_eq!(runtime.matches(marker).count(), 2);

        for (start, end) in [
            (
                "async fn prune_telemetry_rollups(",
                "async fn prune_system_metric_rollups(",
            ),
            (
                "async fn prune_telemetry_network_rates(",
                "async fn prune_telemetry_ping_rollups(",
            ),
        ] {
            let body = section(runtime, start, end);
            let branches = &body[body
                .find("let rows = if dry_run")
                .expect("explicit prune execution branches")..];
            let (dry_run, applied) = branches
                .split_once("} else {")
                .expect("separate preview and applied prune branches");
            assert!(dry_run.contains("let rows = if dry_run"));
            assert!(dry_run.contains(".fetch_all(pool)"));
            assert!(!dry_run.contains("pool.begin()"));
            assert!(!dry_run.contains(marker));
            assert!(applied.contains("pool.begin().await?"));
            assert!(applied.contains(marker));
            assert!(applied.contains(".fetch_all(&mut *tx)"));
            assert!(applied.contains("tx.commit().await?"));
        }

        for (start, end) in [
            (
                "async fn prune_system_metric_rollups(",
                "async fn prune_telemetry_network_rates(",
            ),
            (
                "async fn prune_telemetry_ping_rollups(",
                "async fn prune_traffic_rollups(",
            ),
        ] {
            assert!(!section(runtime, start, end).contains(marker));
        }
    }

    #[test]
    fn retention_policy_wake_is_limited_to_earlier_worker_owned_expiry() {
        assert!(history_retention_policy_advances_worker_frontier(
            HistoryRetentionDomain::TelemetryRollups,
            true,
            365,
            true,
            30,
        ));
        assert!(history_retention_policy_advances_worker_frontier(
            HistoryRetentionDomain::NetworkObservations,
            false,
            30,
            true,
            30,
        ));
        assert!(!history_retention_policy_advances_worker_frontier(
            HistoryRetentionDomain::TelemetryRollups,
            true,
            30,
            true,
            365,
        ));
        assert!(!history_retention_policy_advances_worker_frontier(
            HistoryRetentionDomain::TelemetryRollups,
            true,
            30,
            false,
            30,
        ));
        assert!(!history_retention_policy_advances_worker_frontier(
            HistoryRetentionDomain::AuditLogs,
            true,
            365,
            true,
            30,
        ));
    }

    #[test]
    fn manual_ping_dependency_prune_uses_the_database_commit_trigger_only() {
        let source = include_str!("repository_history.rs");
        let body = section(
            source,
            "async fn prune_telemetry_ping_rollups(",
            "async fn prune_traffic_rollups(",
        );
        let (preview, applied) = body
            .split_once("} else {")
            .expect("separate preview and applied prune branches");
        assert!(!preview.contains("ping_rollups_deleted"));
        assert!(!applied.contains("ping_rollups_deleted"));
        assert!(!body.contains("pg_notify"));

        let migration = include_str!("../../../../../migrations/0003_telemetry_core.sql");
        let trigger = migration
            .split_once("CREATE TRIGGER telemetry_ping_rollups_retention_delete")
            .unwrap()
            .1
            .split_once("CREATE TRIGGER telemetry_samples_retention_delete")
            .unwrap()
            .0;
        assert!(trigger.contains("AFTER DELETE ON public.telemetry_ping_rollups"));
        assert!(trigger.contains("EXECUTE FUNCTION public.publish_telemetry_retention_effect("));
        assert!(trigger.contains("'ping_rollups_deleted'"));
    }
}
