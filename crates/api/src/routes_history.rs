use axum::{
    extract::{Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use serde_json::{json, Map, Value};
use vpsman_common::payload_hash;

use crate::{
    error::ApiError,
    model_history::{
        HistoryDomain, HistoryExportQuery, HistoryExportView, HistoryRetentionPolicyView,
        HistoryRetentionPruneDomainView, HistoryRetentionPrunePlan, HistoryRetentionPruneRequest,
        HistoryRetentionPruneResponse, UpsertHistoryRetentionPolicyRequest,
    },
    security::{operator_has_scope, SCOPE_BACKUPS_READ, SCOPE_FLEET_READ, SCOPE_JOBS_READ},
    state::AppState,
    unix_now,
    util::limit_or_default,
};

pub(crate) async fn list_history_retention_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<HistoryRetentionPolicyView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_FLEET_READ)
        .await?;
    Ok(Json(state.repo.list_history_retention_policies().await?))
}

pub(crate) async fn upsert_history_retention_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<UpsertHistoryRetentionPolicyRequest>,
) -> Result<(StatusCode, Json<HistoryRetentionPolicyView>), ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    if !request.confirmed {
        return Err(ApiError::bad_request(
            "history_retention_policy_confirmation_required",
        ));
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(
            state
                .repo
                .upsert_history_retention_policy(request, &operator)
                .await?,
        ),
    ))
}

pub(crate) async fn prune_history_retention(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<HistoryRetentionPruneRequest>,
) -> Result<Json<HistoryRetentionPruneResponse>, ApiError> {
    let operator = state
        .require_operator_role_and_scope(&headers, "operator", "inventory:write")
        .await?;
    if !request.dry_run && !request.confirmed {
        return Err(ApiError::bad_request(
            "history_retention_prune_requires_confirmation",
        ));
    }
    if !request.dry_run
        && request
            .preview_hash
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        return Err(ApiError::bad_request(
            "history_retention_prune_preview_hash_required",
        ));
    }
    let preview_outputs = collect_history_retention_prune_outputs(&state, &request, false).await?;
    let preview_hash = history_retention_prune_preview_hash(
        request.domain.as_deref(),
        request.metadata_only,
        &preview_outputs,
    )?;
    if request.dry_run {
        return Ok(Json(HistoryRetentionPruneResponse {
            dry_run: true,
            metadata_only_requested: request.metadata_only,
            preview_hash,
            domains: preview_outputs,
        }));
    }
    if request
        .preview_hash
        .as_deref()
        .is_some_and(|submitted| submitted.trim() != preview_hash)
    {
        return Err(ApiError::conflict(
            "history_retention_prune_preview_hash_mismatch",
        ));
    }
    let outputs = collect_history_retention_prune_outputs(&state, &request, true).await?;
    let audit_domains = outputs
        .iter()
        .map(|domain| {
            json!({
                "domain": domain.domain,
                "matched_rows": domain.matched_rows,
                "pruned_rows": domain.pruned_rows,
                "metadata_only": domain.metadata_only,
                "object_delete_attempted": domain.object_delete_attempted,
                "object_delete_errors": &domain.object_delete_errors,
                "status": &domain.status,
                "preview_hash": preview_hash,
            })
        })
        .collect::<Vec<_>>();
    state
        .repo
        .record_history_retention_prune_audit(
            &operator,
            request.dry_run,
            request.metadata_only,
            &audit_domains,
        )
        .await?;
    Ok(Json(HistoryRetentionPruneResponse {
        dry_run: false,
        metadata_only_requested: request.metadata_only,
        preview_hash,
        domains: outputs,
    }))
}

async fn collect_history_retention_prune_outputs(
    state: &AppState,
    request: &HistoryRetentionPruneRequest,
    execute: bool,
) -> Result<Vec<HistoryRetentionPruneDomainView>, ApiError> {
    let requested_domain = request
        .domain
        .as_deref()
        .map(parse_history_domain)
        .transpose()?;
    let policies = state.repo.list_history_retention_policies().await?;
    let mut outputs = Vec::new();
    for policy in policies {
        let domain = parse_history_domain(&policy.domain)?;
        if requested_domain.is_some_and(|requested| requested != domain) {
            continue;
        }
        let cutoff_unix = unix_now().saturating_sub(policy.retention_days.max(1) as u64 * 86_400);
        let metadata_only = request.metadata_only.unwrap_or(policy.metadata_only);
        if domain.object_backed()
            && execute
            && !metadata_only
            && state.backup_object_store.is_none()
        {
            return Err(ApiError::bad_request(
                "history_retention_object_store_required",
            ));
        }
        let plan = HistoryRetentionPrunePlan {
            domain,
            prune_limit: policy.prune_limit,
            enabled: policy.enabled,
        };
        let mut object_delete_attempted = false;
        let mut object_delete_errors = Vec::new();
        let outcome = if domain.object_backed() {
            let candidates = state
                .repo
                .list_history_retention_object_candidates(&plan, cutoff_unix)
                .await?;
            let matched_rows = candidates.len() as i64;
            let mut pruned_rows = 0_i64;
            let mut object_keys = if !execute || metadata_only {
                candidates
                    .iter()
                    .filter_map(|candidate| candidate.object_key().map(str::to_string))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            if execute {
                if metadata_only {
                    pruned_rows = state
                        .repo
                        .prune_history_retention_object_candidates(&candidates)
                        .await?;
                } else if !candidates.is_empty() {
                    object_delete_attempted = true;
                    if let Some(store) = state.backup_object_store.as_ref() {
                        for candidate in &candidates {
                            let rows = state
                                .repo
                                .prune_history_retention_object_candidate(candidate)
                                .await?;
                            pruned_rows += rows;
                            if rows == 0 {
                                continue;
                            }
                            if let Some(object_key) = candidate.object_key() {
                                object_keys.push(object_key.to_string());
                                match store.delete_confirmed(object_key).await {
                                    Ok(()) => {}
                                    Err(error) => {
                                        object_delete_errors.push(format!("{object_key}: {error}"));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            crate::model_history::HistoryRetentionPruneOutcome {
                matched_rows,
                pruned_rows,
                object_keys,
            }
        } else {
            state
                .repo
                .prune_history_domain(&plan, cutoff_unix, !execute)
                .await?
        };
        let status = if !policy.enabled {
            "disabled"
        } else if !execute {
            "dry_run"
        } else if !object_delete_errors.is_empty() {
            "partial_error"
        } else if outcome.pruned_rows == 0 {
            "no_matches"
        } else {
            "pruned"
        };
        outputs.push(HistoryRetentionPruneDomainView {
            domain: policy.domain,
            enabled: policy.enabled,
            retention_days: policy.retention_days,
            cutoff_unix,
            matched_rows: outcome.matched_rows,
            pruned_rows: outcome.pruned_rows,
            object_keys: outcome.object_keys,
            object_delete_attempted,
            object_delete_errors,
            metadata_only,
            status: status.to_string(),
        });
    }
    if outputs.is_empty() {
        return Err(ApiError::bad_request("history_retention_domain_not_found"));
    }
    Ok(outputs)
}

fn history_retention_prune_preview_hash(
    requested_domain: Option<&str>,
    metadata_only: Option<bool>,
    outputs: &[HistoryRetentionPruneDomainView],
) -> Result<String, ApiError> {
    let payload = serde_json::to_vec(&json!({
        "version": 1,
        "requested_domain": requested_domain,
        "metadata_only_requested": metadata_only,
        "domains": outputs.iter().map(|domain| {
            json!({
                "domain": domain.domain,
                "enabled": domain.enabled,
                "retention_days": domain.retention_days,
                "cutoff_unix": domain.cutoff_unix,
                "matched_rows": domain.matched_rows,
                "object_keys": domain.object_keys,
                "metadata_only": domain.metadata_only,
                "status": domain.status,
            })
        }).collect::<Vec<_>>(),
    }))
    .map_err(|error| {
        ApiError::from(anyhow::anyhow!(
            "history_retention_preview_hash_failed: {error}"
        ))
    })?;
    Ok(payload_hash(&payload))
}

pub(crate) async fn export_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryExportQuery>,
) -> Result<Json<HistoryExportView>, ApiError> {
    let selected = parse_history_domains(query.domains.as_deref())?;
    let operator = state.require_operator(&headers).await?;
    for domain in &selected {
        let required_scope = history_export_scope(*domain);
        if !operator_has_scope(&operator.operator.scopes, required_scope) {
            return Err(ApiError::forbidden("operator_scope_insufficient"));
        }
    }
    let limit = limit_or_default(query.limit);
    let policies = state.repo.list_history_retention_policies().await?;
    let mut exported_domains = Vec::new();
    let mut data = Map::new();
    for domain in selected {
        let policy = policies
            .iter()
            .find(|policy| policy.domain == domain.as_str())
            .ok_or_else(|| ApiError::bad_request("history_retention_domain_not_found"))?;
        if !policy.export_enabled {
            return Err(ApiError::forbidden("history_export_domain_disabled"));
        }
        exported_domains.push(domain.as_str().to_string());
        match domain {
            HistoryDomain::AuditLogs => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(state.repo.list_audit_logs(limit).await?),
                );
            }
            HistoryDomain::TelemetryRollups => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(
                        state
                            .repo
                            .list_telemetry_rollups(limit, query.client_id.as_deref(), None)
                            .await?
                    ),
                );
            }
            HistoryDomain::SystemMetricRollups => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(
                        state
                            .repo
                            .list_system_metric_rollups(0, unix_now(), limit)
                            .await?
                    ),
                );
            }
            HistoryDomain::JobOutputs => {
                data.insert(
                    domain.as_str().to_string(),
                    Value::Array(
                        state
                            .repo
                            .export_job_outputs(limit, query.client_id.as_deref(), query.job_id)
                            .await?,
                    ),
                );
            }
            HistoryDomain::BackupArtifacts => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(state.repo.list_backup_artifacts(limit).await?),
                );
            }
            HistoryDomain::NetworkObservations => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(state.repo.list_network_observations(limit).await?),
                );
            }
            HistoryDomain::TopologyHistory => {
                data.insert(
                    domain.as_str().to_string(),
                    json!({
                        "graph": state.repo.topology_graph(limit).await?,
                        "trends": state.repo.list_network_observation_trends(limit).await?,
                    }),
                );
            }
        }
    }
    Ok(Json(HistoryExportView {
        generated_at: unix_now().to_string(),
        limit,
        domains: exported_domains,
        data: Value::Object(data),
    }))
}

fn parse_history_domains(value: Option<&str>) -> Result<Vec<HistoryDomain>, ApiError> {
    let Some(value) = value else {
        return Ok(HistoryDomain::ALL.to_vec());
    };
    let mut domains = Vec::new();
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let domain = parse_history_domain(part)?;
        if !domains.contains(&domain) {
            domains.push(domain);
        }
    }
    if domains.is_empty() {
        return Err(ApiError::bad_request("history_export_domains_required"));
    }
    Ok(domains)
}

fn parse_history_domain(value: &str) -> Result<HistoryDomain, ApiError> {
    HistoryDomain::from_str(value).ok_or_else(|| ApiError::bad_request("invalid_history_domain"))
}

fn history_export_scope(domain: HistoryDomain) -> &'static str {
    match domain {
        HistoryDomain::JobOutputs => SCOPE_JOBS_READ,
        HistoryDomain::BackupArtifacts => SCOPE_BACKUPS_READ,
        HistoryDomain::AuditLogs
        | HistoryDomain::SystemMetricRollups
        | HistoryDomain::TelemetryRollups
        | HistoryDomain::NetworkObservations
        | HistoryDomain::TopologyHistory => SCOPE_FLEET_READ,
    }
}
