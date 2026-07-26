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
    repository_history::HistoryRetentionObjectCandidate,
    security::{
        operator_has_scope, SCOPE_AUDIT_READ, SCOPE_BACKUPS_READ, SCOPE_FLEET_READ,
        SCOPE_HISTORY_WRITE, SCOPE_JOBS_READ, SCOPE_NETWORK_READ,
    },
    state::AppState,
    unix_now,
    util::limit_or_default,
};

const RETENTION_DAY_SECS: u64 = 86_400;

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
        .require_operator_role_and_scope(&headers, "operator", SCOPE_HISTORY_WRITE)
        .await?;
    let domain = parse_history_domain(&request.domain)?;
    ensure_history_retention_domain_authority(&operator.operator.scopes, &[domain])?;
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
        .require_operator_role_and_scope(&headers, "operator", SCOPE_HISTORY_WRITE)
        .await?;
    let selected_domains = selected_history_retention_prune_domains(request.domain.as_deref())?;
    ensure_history_retention_domain_authority(&operator.operator.scopes, &selected_domains)?;
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
    let plan = history_retention_prune_plan(&state, &request).await?;
    let preview_outputs = history_retention_prune_preview_outputs(&state, &plan).await?;
    let preview_hash = history_retention_prune_preview_hash(
        request.domain.as_deref(),
        request.metadata_only,
        &plan,
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
    let outputs = execute_history_retention_prune_plan(&state, plan).await?;
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

struct HistoryRetentionPruneDomainPlan {
    policy: HistoryRetentionPolicyView,
    prune_plan: HistoryRetentionPrunePlan,
    cutoff_unix: u64,
    metadata_only: bool,
    object_candidates: Option<Vec<HistoryRetentionObjectCandidate>>,
}

async fn history_retention_prune_plan(
    state: &AppState,
    request: &HistoryRetentionPruneRequest,
) -> Result<Vec<HistoryRetentionPruneDomainPlan>, ApiError> {
    let requested_domain = request
        .domain
        .as_deref()
        .map(parse_history_domain)
        .transpose()?;
    let policies = state.repo.list_history_retention_policies().await?;
    let mut plan = Vec::new();
    for policy in policies {
        let domain = parse_history_domain(&policy.domain)?;
        if requested_domain.is_some_and(|requested| requested != domain) {
            continue;
        }
        let cutoff_unix = retention_cutoff_unix(policy.retention_days);
        let metadata_only = request.metadata_only.unwrap_or(policy.metadata_only);
        let prune_plan = HistoryRetentionPrunePlan {
            domain,
            prune_limit: policy.prune_limit,
            enabled: policy.enabled,
        };
        let object_candidates = if domain.object_backed() {
            Some(
                state
                    .repo
                    .list_history_retention_object_candidates(&prune_plan, cutoff_unix)
                    .await?,
            )
        } else {
            None
        };
        plan.push(HistoryRetentionPruneDomainPlan {
            policy,
            prune_plan,
            cutoff_unix,
            metadata_only,
            object_candidates,
        });
    }
    if plan.is_empty() {
        return Err(ApiError::bad_request("history_retention_domain_not_found"));
    }
    Ok(plan)
}

async fn history_retention_prune_preview_outputs(
    state: &AppState,
    plan: &[HistoryRetentionPruneDomainPlan],
) -> Result<Vec<HistoryRetentionPruneDomainView>, ApiError> {
    let mut outputs = Vec::new();
    for domain_plan in plan {
        let outcome = if let Some(candidates) = &domain_plan.object_candidates {
            crate::model_history::HistoryRetentionPruneOutcome {
                matched_rows: candidates.len() as i64,
                pruned_rows: 0,
                object_keys: candidates
                    .iter()
                    .filter_map(|candidate| candidate.object_key().map(str::to_string))
                    .collect::<Vec<_>>(),
            }
        } else {
            state
                .repo
                .prune_history_domain(&domain_plan.prune_plan, domain_plan.cutoff_unix, true)
                .await?
        };
        let status = if !domain_plan.policy.enabled {
            "disabled"
        } else {
            "dry_run"
        };
        outputs.push(HistoryRetentionPruneDomainView {
            domain: domain_plan.policy.domain.clone(),
            enabled: domain_plan.policy.enabled,
            retention_days: domain_plan.policy.retention_days,
            cutoff_unix: domain_plan.cutoff_unix,
            matched_rows: outcome.matched_rows,
            pruned_rows: outcome.pruned_rows,
            object_keys: outcome.object_keys,
            object_delete_attempted: false,
            object_delete_errors: Vec::new(),
            metadata_only: domain_plan.metadata_only,
            status: status.to_string(),
        });
    }
    Ok(outputs)
}

async fn execute_history_retention_prune_plan(
    state: &AppState,
    plan: Vec<HistoryRetentionPruneDomainPlan>,
) -> Result<Vec<HistoryRetentionPruneDomainView>, ApiError> {
    let mut outputs = Vec::new();
    for domain_plan in plan {
        if domain_plan.prune_plan.domain.object_backed()
            && !domain_plan.metadata_only
            && state.backup_object_store.is_none()
        {
            return Err(ApiError::bad_request(
                "history_retention_object_store_required",
            ));
        }
        let mut object_delete_attempted = false;
        let mut object_delete_errors = Vec::new();
        let outcome = if let Some(candidates) = domain_plan.object_candidates {
            let matched_rows = candidates.len() as i64;
            let mut pruned_rows = 0_i64;
            let mut object_keys = if domain_plan.metadata_only {
                candidates
                    .iter()
                    .filter_map(|candidate| candidate.object_key().map(str::to_string))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            if domain_plan.metadata_only {
                pruned_rows = state
                    .repo
                    .prune_history_retention_object_candidates(&candidates)
                    .await?;
            } else if !candidates.is_empty() {
                object_delete_attempted = true;
                if let Some(store) = state.backup_object_store.as_ref() {
                    for candidate in &candidates {
                        let Some(object_key) = candidate.object_key() else {
                            pruned_rows += state
                                .repo
                                .prune_history_retention_object_candidate(candidate)
                                .await?;
                            continue;
                        };
                        if !state
                            .repo
                            .begin_history_retention_object_delete(candidate)
                            .await?
                        {
                            continue;
                        }
                        object_keys.push(object_key.to_string());
                        match store.delete_confirmed(object_key).await {
                            Ok(()) => {
                                pruned_rows += state
                                    .repo
                                    .finalize_history_retention_object_delete(candidate)
                                    .await?;
                            }
                            Err(error) => {
                                let error_text = error.to_string();
                                state
                                    .repo
                                    .mark_history_retention_object_delete_failed(
                                        candidate,
                                        &error_text,
                                    )
                                    .await?;
                                object_delete_errors.push(format!("{object_key}: {error_text}"));
                                break;
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
                .prune_history_domain(&domain_plan.prune_plan, domain_plan.cutoff_unix, false)
                .await?
        };
        let status = if !domain_plan.policy.enabled {
            "disabled"
        } else if !object_delete_errors.is_empty() {
            "partial_error"
        } else if outcome.pruned_rows == 0 {
            "no_matches"
        } else {
            "pruned"
        };
        outputs.push(HistoryRetentionPruneDomainView {
            domain: domain_plan.policy.domain,
            enabled: domain_plan.policy.enabled,
            retention_days: domain_plan.policy.retention_days,
            cutoff_unix: domain_plan.cutoff_unix,
            matched_rows: outcome.matched_rows,
            pruned_rows: outcome.pruned_rows,
            object_keys: outcome.object_keys,
            object_delete_attempted,
            object_delete_errors,
            metadata_only: domain_plan.metadata_only,
            status: status.to_string(),
        });
    }
    Ok(outputs)
}

fn history_retention_prune_preview_hash(
    requested_domain: Option<&str>,
    metadata_only: Option<bool>,
    plan: &[HistoryRetentionPruneDomainPlan],
    outputs: &[HistoryRetentionPruneDomainView],
) -> Result<String, ApiError> {
    if plan.len() != outputs.len() {
        return Err(ApiError::from(anyhow::anyhow!(
            "history_retention_preview_hash_failed: plan_output_length_mismatch"
        )));
    }
    let payload = serde_json::to_vec(&json!({
        "version": 1,
        "requested_domain": requested_domain,
        "metadata_only_requested": metadata_only,
        "domains": plan.iter().zip(outputs.iter()).map(|(domain_plan, domain)| {
            json!({
                "domain": domain.domain,
                "enabled": domain.enabled,
                "retention_days": domain.retention_days,
                "prune_limit": domain_plan.prune_plan.prune_limit,
                "cutoff_day": retention_cutoff_day(domain.cutoff_unix),
                "matched_rows": domain.matched_rows,
                "object_keys": &domain.object_keys,
                "candidate_keys": history_retention_candidate_hash_keys(
                    domain_plan.object_candidates.as_deref(),
                ),
                "metadata_only": domain.metadata_only,
                "status": &domain.status,
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

fn history_retention_candidate_hash_keys(
    candidates: Option<&[HistoryRetentionObjectCandidate]>,
) -> Vec<serde_json::Value> {
    candidates
        .unwrap_or(&[])
        .iter()
        .map(|candidate| match candidate {
            HistoryRetentionObjectCandidate::JobOutput {
                job_id,
                client_id,
                seq,
                object_key,
            } => json!({
                "type": "job_output",
                "job_id": job_id,
                "client_id": client_id,
                "seq": seq,
                "object_key": object_key,
            }),
            HistoryRetentionObjectCandidate::BackupArtifact {
                artifact_id,
                object_key,
            } => json!({
                "type": "backup_artifact",
                "artifact_id": artifact_id,
                "object_key": object_key,
            }),
        })
        .collect()
}

fn retention_cutoff_unix(retention_days: i32) -> u64 {
    let today_start = unix_now() / RETENTION_DAY_SECS * RETENTION_DAY_SECS;
    today_start.saturating_sub(retention_days.max(1) as u64 * RETENTION_DAY_SECS)
}

fn retention_cutoff_day(cutoff_unix: u64) -> u64 {
    cutoff_unix / RETENTION_DAY_SECS
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
            HistoryDomain::TelemetryNetworkRates => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(
                        state
                            .repo
                            .list_telemetry_network_rates(
                                limit,
                                query.client_id.as_deref(),
                                None,
                                None,
                            )
                            .await?
                    ),
                );
            }
            HistoryDomain::TrafficCounterSamples => {
                data.insert(
                    domain.as_str().to_string(),
                    json!(
                        state
                            .repo
                            .export_traffic_counter_samples(limit, query.client_id.as_deref())
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
            HistoryDomain::ClientStatusHistory => {
                data.insert(
                    domain.as_str().to_string(),
                    Value::Array(
                        state
                            .repo
                            .export_client_status_history(limit, query.client_id.as_deref())
                            .await?,
                    ),
                );
            }
            HistoryDomain::GatewaySessions => {
                data.insert(
                    domain.as_str().to_string(),
                    Value::Array(
                        state
                            .repo
                            .export_gateway_sessions(limit, query.client_id.as_deref())
                            .await?,
                    ),
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

fn selected_history_retention_prune_domains(
    value: Option<&str>,
) -> Result<Vec<HistoryDomain>, ApiError> {
    match value {
        Some(value) => Ok(vec![parse_history_domain(value)?]),
        None => Ok(HistoryDomain::ALL.to_vec()),
    }
}

fn ensure_history_retention_domain_authority(
    scopes: &[String],
    domains: &[HistoryDomain],
) -> Result<(), ApiError> {
    for domain in domains {
        if !operator_has_scope(scopes, history_retention_authority_scope(*domain)) {
            return Err(ApiError::forbidden("operator_scope_insufficient"));
        }
    }
    Ok(())
}

fn history_retention_authority_scope(domain: HistoryDomain) -> &'static str {
    match domain {
        HistoryDomain::AuditLogs => SCOPE_AUDIT_READ,
        HistoryDomain::SystemMetricRollups
        | HistoryDomain::TelemetryRollups
        | HistoryDomain::TelemetryNetworkRates
        | HistoryDomain::TrafficCounterSamples
        | HistoryDomain::ClientStatusHistory
        | HistoryDomain::GatewaySessions => "inventory:write",
        HistoryDomain::JobOutputs => "jobs:write",
        HistoryDomain::BackupArtifacts => "backups:write",
        HistoryDomain::NetworkObservations | HistoryDomain::TopologyHistory => "network:write",
    }
}

fn history_export_scope(domain: HistoryDomain) -> &'static str {
    match domain {
        HistoryDomain::JobOutputs => SCOPE_JOBS_READ,
        HistoryDomain::BackupArtifacts => SCOPE_BACKUPS_READ,
        HistoryDomain::AuditLogs => SCOPE_AUDIT_READ,
        HistoryDomain::NetworkObservations | HistoryDomain::TopologyHistory => SCOPE_NETWORK_READ,
        HistoryDomain::SystemMetricRollups
        | HistoryDomain::TelemetryRollups
        | HistoryDomain::TelemetryNetworkRates
        | HistoryDomain::TrafficCounterSamples
        | HistoryDomain::ClientStatusHistory
        | HistoryDomain::GatewaySessions => SCOPE_FLEET_READ,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_retention_prune_preview_hash_ignores_moving_cutoff() {
        let plan = vec![HistoryRetentionPruneDomainPlan {
            policy: HistoryRetentionPolicyView {
                domain: "job_outputs".to_string(),
                retention_days: 30,
                prune_limit: 1000,
                enabled: true,
                metadata_only: false,
                export_enabled: true,
                notes: None,
                updated_by: None,
                updated_at: "0".to_string(),
                built_in_default: false,
            },
            prune_plan: HistoryRetentionPrunePlan {
                domain: HistoryDomain::JobOutputs,
                prune_limit: 1000,
                enabled: true,
            },
            cutoff_unix: 10_000,
            metadata_only: false,
            object_candidates: Some(vec![HistoryRetentionObjectCandidate::JobOutput {
                job_id: uuid::Uuid::new_v4(),
                client_id: "edge-a".to_string(),
                seq: 1,
                object_key: Some("job-output/a".to_string()),
            }]),
        }];
        let mut domain = HistoryRetentionPruneDomainView {
            domain: "job_outputs".to_string(),
            enabled: true,
            retention_days: 30,
            cutoff_unix: 10_000,
            matched_rows: 2,
            pruned_rows: 0,
            object_keys: vec!["job-output/a".to_string(), "job-output/b".to_string()],
            object_delete_attempted: false,
            object_delete_errors: Vec::new(),
            metadata_only: false,
            status: "dry_run".to_string(),
        };

        let first = history_retention_prune_preview_hash(
            Some("job_outputs"),
            Some(false),
            &plan,
            &[domain.clone()],
        )
        .expect("first history prune preview hash");
        domain.cutoff_unix += 60;
        let same_candidates = history_retention_prune_preview_hash(
            Some("job_outputs"),
            Some(false),
            &plan,
            &[domain.clone()],
        )
        .expect("second history prune preview hash");
        assert_eq!(first, same_candidates);

        domain.cutoff_unix += RETENTION_DAY_SECS;
        let next_day = history_retention_prune_preview_hash(
            Some("job_outputs"),
            Some(false),
            &plan,
            &[domain.clone()],
        )
        .expect("next-day history prune preview hash");
        assert_ne!(first, next_day);

        domain.cutoff_unix -= RETENTION_DAY_SECS;
        domain.object_keys.push("job-output/c".to_string());
        domain.matched_rows += 1;
        let changed_candidates = history_retention_prune_preview_hash(
            Some("job_outputs"),
            Some(false),
            &plan,
            &[domain],
        )
        .expect("changed history prune preview hash");
        assert_ne!(first, changed_candidates);
    }
}
