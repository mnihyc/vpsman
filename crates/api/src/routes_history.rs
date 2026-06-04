use axum::{
    extract::{Query, State},
    http::HeaderMap,
    http::StatusCode,
    Json,
};
use serde_json::{json, Map, Value};

use crate::{
    error::ApiError,
    model_history::{
        HistoryDomain, HistoryExportQuery, HistoryExportView, HistoryRetentionPolicyView,
        HistoryRetentionPruneDomainView, HistoryRetentionPrunePlan, HistoryRetentionPruneRequest,
        HistoryRetentionPruneResponse, UpsertHistoryRetentionPolicyRequest,
    },
    state::AppState,
    unix_now,
    util::limit_or_default,
};

pub(crate) async fn list_history_retention_policies(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<HistoryRetentionPolicyView>>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
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
            && !request.dry_run
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
        let outcome = state
            .repo
            .prune_history_domain(&plan, cutoff_unix, request.dry_run)
            .await?;
        let mut object_delete_attempted = false;
        let mut object_delete_errors = Vec::new();
        if domain.object_backed()
            && !request.dry_run
            && !metadata_only
            && !outcome.object_keys.is_empty()
        {
            object_delete_attempted = true;
            if let Some(store) = state.backup_object_store.as_ref() {
                for object_key in &outcome.object_keys {
                    store.delete_best_effort(object_key).await;
                }
            } else {
                object_delete_errors.push("object_store_not_configured".to_string());
            }
        }
        let status = if !policy.enabled {
            "disabled"
        } else if request.dry_run {
            "dry_run"
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
    if !request.dry_run {
        let audit_domains = outputs
            .iter()
            .map(|domain| {
                json!({
                    "domain": domain.domain,
                    "matched_rows": domain.matched_rows,
                    "pruned_rows": domain.pruned_rows,
                    "metadata_only": domain.metadata_only,
                    "object_delete_attempted": domain.object_delete_attempted,
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
    }
    Ok(Json(HistoryRetentionPruneResponse {
        dry_run: request.dry_run,
        metadata_only_requested: request.metadata_only,
        domains: outputs,
    }))
}

pub(crate) async fn export_history(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryExportQuery>,
) -> Result<Json<HistoryExportView>, ApiError> {
    let _operator = state.require_operator_scope(&headers, "fleet:read").await?;
    let limit = limit_or_default(query.limit);
    let selected = parse_history_domains(query.domains.as_deref())?;
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
            HistoryDomain::TelemetrySamples => {
                data.insert(
                    domain.as_str().to_string(),
                    Value::Array(
                        state
                            .repo
                            .export_telemetry_samples(limit, query.client_id.as_deref())
                            .await?,
                    ),
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
