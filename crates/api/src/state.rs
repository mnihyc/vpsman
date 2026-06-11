use anyhow::Result;
use axum::http::HeaderMap;
use serde_json::{json, Map, Value};
use tokio::sync::broadcast;

use crate::{
    error::ApiError,
    fleet_alerts::FleetAlertPolicy,
    gateway_client::GatewayDispatchClient,
    model::{
        AgentUpdateReleaseView, AuthContext, BackupArtifactView, BackupRequestView,
        MigrationLinkView, NetworkObservationTrendView, NetworkOspfRecommendationView,
        RestorePlanView, TunnelPlanView, WsEvent,
    },
    model_data_sources::DataSourceStatusView,
    object_store::BackupObjectStore,
    repository::Repository,
    security::{bearer_token, constant_time_eq, operator_has_scope, role_allows},
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) repo: Repository,
    pub(crate) events: broadcast::Sender<WsEvent>,
    pub(crate) internal_token: Option<String>,
    pub(crate) gateway: GatewayDispatchClient,
    pub(crate) backup_object_store: Option<BackupObjectStore>,
    pub(crate) update_object_store: Option<BackupObjectStore>,
    pub(crate) update_artifact_public_base_url: Option<String>,
    pub(crate) update_release_policy: UpdateReleasePolicy,
    pub(crate) fleet_alert_policy: FleetAlertPolicy,
    pub(crate) job_output_artifact_min_bytes: usize,
    pub(crate) require_registered_agent_updates: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct UpdateReleasePolicy {
    allowed_channels: Vec<String>,
    trusted_signing_keys_hex: Vec<String>,
}

impl UpdateReleasePolicy {
    pub(crate) fn new(
        allowed_channels: Vec<String>,
        trusted_signing_keys_hex: Vec<String>,
    ) -> Result<Self> {
        let mut normalized_channels = Vec::new();
        for channel in allowed_channels {
            let channel = channel.trim().to_ascii_lowercase();
            if channel.is_empty() {
                continue;
            }
            if !is_safe_release_token(&channel, 32) {
                anyhow::bail!("update release channel {channel:?} is invalid");
            }
            if !normalized_channels.iter().any(|stored| stored == &channel) {
                normalized_channels.push(channel);
            }
        }

        let mut normalized_keys = Vec::new();
        for key in trusted_signing_keys_hex {
            let key = key.trim().to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            if !is_fixed_hex(&key, 64) {
                anyhow::bail!("trusted update signing key must be 32-byte hex");
            }
            if !normalized_keys.iter().any(|stored| stored == &key) {
                normalized_keys.push(key);
            }
        }

        Ok(Self {
            allowed_channels: normalized_channels,
            trusted_signing_keys_hex: normalized_keys,
        })
    }

    pub(crate) fn validate_channel(&self, channel: &str) -> Result<(), ApiError> {
        if self.allowed_channels.is_empty() {
            return Ok(());
        }
        let channel = channel.trim().to_ascii_lowercase();
        if self
            .allowed_channels
            .iter()
            .any(|allowed| allowed == &channel)
        {
            Ok(())
        } else {
            Err(ApiError::forbidden(
                "agent_update_release_channel_not_allowed",
            ))
        }
    }

    pub(crate) fn validate_signing_key(
        &self,
        signing_key_hex: &str,
        error_code: &'static str,
    ) -> Result<(), ApiError> {
        if self.trusted_signing_keys_hex.is_empty() {
            return Ok(());
        }
        let signing_key_hex = signing_key_hex.trim().to_ascii_lowercase();
        if self
            .trusted_signing_keys_hex
            .iter()
            .any(|trusted| trusted == &signing_key_hex)
        {
            Ok(())
        } else {
            Err(ApiError::forbidden(error_code))
        }
    }
}

impl AppState {
    pub(crate) fn enrich_agent_update_release_urls(
        &self,
        mut release: AgentUpdateReleaseView,
    ) -> AgentUpdateReleaseView {
        if let Some(path) = release.artifact_download_path.as_deref() {
            release.artifact_download_url = self.public_update_artifact_url(path);
        }
        if let Some(path) = release.rollback_artifact_download_path.as_deref() {
            release.rollback_artifact_download_url = self.public_update_artifact_url(path);
        }
        release
    }

    pub(crate) fn public_update_artifact_url(&self, path: &str) -> Option<String> {
        let base = self.update_artifact_public_base_url.as_deref()?;
        Some(format!("{}{}", base.trim_end_matches('/'), path))
    }

    pub(crate) async fn list_data_source_status(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<DataSourceStatusView>> {
        let mut rows = self.repo.list_data_source_status(client_id, domain).await?;
        if rows.iter().any(|row| {
            matches!(
                row.domain.as_str(),
                "backup_object_store" | "restore_path_mapping"
            )
        }) {
            let artifacts = self.repo.list_backup_artifacts(1000).await?;
            let backup_requests = self.repo.list_backup_requests(1000).await?;
            let restore_plans = self.repo.list_restore_plans(1000).await?;
            let migration_links = self.repo.list_migration_links(1000).await?;
            enrich_backup_status_rows(
                &mut rows,
                self.backup_object_store.as_ref(),
                &artifacts,
                &backup_requests,
                &restore_plans,
                &migration_links,
            );
        }
        if rows.iter().any(|row| {
            matches!(
                row.domain.as_str(),
                "update_artifact_source"
                    | "update_restart_policy"
                    | "update_rollback_heartbeat_source"
            )
        }) {
            let releases = self.repo.list_agent_update_releases(1000).await?;
            enrich_update_status_rows(&mut rows, self.update_object_store.as_ref(), &releases);
        }
        if rows.iter().any(|row| {
            matches!(
                row.domain.as_str(),
                "runtime_tunnel_adapter"
                    | "runtime_traffic_accounting_source"
                    | "traffic_limit_status_source"
                    | "routing_daemon_adapter"
            )
        }) {
            let plans = self.repo.list_tunnel_plans().await?;
            let trends = self.repo.list_network_observation_trends(1000).await?;
            let recommendations = self.repo.list_network_ospf_recommendations(1000).await?;
            enrich_runtime_tunnel_status_rows(&mut rows, &plans, &trends, &recommendations);
            enrich_runtime_traffic_status_rows(&mut rows, &plans);
        }
        Ok(rows)
    }

    pub(crate) fn require_internal_gateway(&self, headers: &HeaderMap) -> Result<(), ApiError> {
        let Some(expected_token) = self.internal_token.as_deref() else {
            return Err(ApiError::unauthorized("missing_internal_token"));
        };
        let provided = bearer_token(headers)
            .ok_or_else(|| ApiError::unauthorized("missing_internal_token"))?;
        if constant_time_eq(provided.as_bytes(), expected_token.as_bytes()) {
            Ok(())
        } else {
            Err(ApiError::unauthorized("invalid_internal_token"))
        }
    }

    pub(crate) async fn require_operator(
        &self,
        headers: &HeaderMap,
    ) -> Result<AuthContext, ApiError> {
        let token =
            bearer_token(headers).ok_or_else(|| ApiError::unauthorized("missing_bearer_token"))?;
        self.repo
            .authenticate_access_token(token)
            .await?
            .ok_or_else(|| ApiError::unauthorized("invalid_bearer_token"))
    }

    pub(crate) async fn require_operator_role(
        &self,
        headers: &HeaderMap,
        required_role: &'static str,
    ) -> Result<AuthContext, ApiError> {
        let operator = self.require_operator(headers).await?;
        if role_allows(&operator.operator.role, required_role) {
            Ok(operator)
        } else {
            Err(ApiError::forbidden("operator_role_insufficient"))
        }
    }

    pub(crate) async fn require_operator_role_and_scope(
        &self,
        headers: &HeaderMap,
        required_role: &'static str,
        required_scope: &'static str,
    ) -> Result<AuthContext, ApiError> {
        let operator = self.require_operator_role(headers, required_role).await?;
        if operator_has_scope(&operator.operator.scopes, required_scope) {
            Ok(operator)
        } else {
            Err(ApiError::forbidden("operator_scope_insufficient"))
        }
    }

    pub(crate) async fn require_operator_scope(
        &self,
        headers: &HeaderMap,
        required_scope: &'static str,
    ) -> Result<AuthContext, ApiError> {
        let operator = self.require_operator(headers).await?;
        if operator_has_scope(&operator.operator.scopes, required_scope) {
            Ok(operator)
        } else {
            Err(ApiError::forbidden("operator_scope_insufficient"))
        }
    }

    pub(crate) fn publish(&self, event: WsEvent) {
        let _ = self.events.send(event);
    }

    pub(crate) async fn fleet_snapshot(&self) -> Result<WsEvent> {
        Ok(WsEvent::FleetSnapshot {
            summary: self.repo.fleet_summary().await?,
            agents: self.repo.list_agents().await?,
        })
    }
}

fn is_safe_release_token(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && !value
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
}

fn is_fixed_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn enrich_backup_status_rows(
    rows: &mut [DataSourceStatusView],
    store: Option<&BackupObjectStore>,
    artifacts: &[BackupArtifactView],
    backup_requests: &[BackupRequestView],
    restore_plans: &[RestorePlanView],
    migration_links: &[MigrationLinkView],
) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "backup_object_store" | "restore_path_mapping"
        )
    }) {
        let artifact_count = artifacts
            .iter()
            .filter(|artifact| artifact.client_id == row.client_id)
            .count();
        let backup_request_count = backup_requests
            .iter()
            .filter(|request| request.client_id == row.client_id)
            .count();
        let restore_source_count = restore_plans
            .iter()
            .filter(|plan| plan.source_client_id == row.client_id)
            .count();
        let restore_target_count = restore_plans
            .iter()
            .filter(|plan| plan.target_client_id == row.client_id)
            .count();
        let migration_source_count = migration_links
            .iter()
            .filter(|link| link.source_client_id == row.client_id)
            .count();
        let migration_target_count = migration_links
            .iter()
            .filter(|link| link.target_client_id == row.client_id)
            .count();
        let runtime_evidence = json!({
            "workflow": "backup_artifacts",
            "restore_workflow": "restore_migration",
            "server_object_store_configured": store.is_some(),
            "server_object_store_kind": store.map(BackupObjectStore::kind),
            "artifact_count": artifact_count,
            "backup_request_count": backup_request_count,
            "restore_source_count": restore_source_count,
            "restore_target_count": restore_target_count,
            "migration_source_count": migration_source_count,
            "migration_target_count": migration_target_count,
            "continuous_status": false,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.status == "agent_offline" {
            continue;
        }
        if row.domain == "restore_path_mapping" {
            row.status = "ready_on_demand".to_string();
            row.status_reason =
                "restore path-mapping preset is selected; restore plans and migration links provide concrete mappings"
                    .to_string();
            continue;
        }
        if store.is_some() {
            row.status = "ready".to_string();
            row.status_reason =
                "backup object store is configured; encrypted artifacts can be uploaded"
                    .to_string();
        } else {
            row.status = "selected_no_store".to_string();
            row.status_reason =
                "backup object-store preset is selected, but no server object store is configured"
                    .to_string();
        }
    }
}

fn enrich_update_status_rows(
    rows: &mut [DataSourceStatusView],
    store: Option<&BackupObjectStore>,
    releases: &[AgentUpdateReleaseView],
) {
    let release_count = releases.len();
    let hosted_release_count = releases
        .iter()
        .filter(|release| release.artifact_object_key.is_some())
        .count();
    let external_release_count = releases
        .iter()
        .filter(|release| release.artifact_url_sha256_hex.is_some())
        .count();
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "update_artifact_source" | "update_restart_policy" | "update_rollback_heartbeat_source"
        )
    }) {
        let runtime_evidence = json!({
            "workflow": "agent_update_releases",
            "server_object_store_configured": store.is_some(),
            "server_object_store_kind": store.map(BackupObjectStore::kind),
            "release_count": release_count,
            "hosted_release_count": hosted_release_count,
            "external_release_count": external_release_count,
            "continuous_status": false,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.status == "agent_offline" {
            continue;
        }
        if row.domain == "update_restart_policy" {
            row.status = "ready_on_demand".to_string();
            row.status_reason =
                "update restart policy is selected; activation and rollback jobs report restart evidence"
                    .to_string();
            continue;
        }
        if row.domain == "update_rollback_heartbeat_source" {
            row.status = "ready_on_demand".to_string();
            row.status_reason =
                "rollback heartbeat source is selected; activation and rollback jobs report heartbeat evidence"
                    .to_string();
            continue;
        }
        if store.is_some() {
            row.status = "ready".to_string();
            row.status_reason =
                "update artifact store is configured; hosted signed releases can be published"
                    .to_string();
        } else if external_release_count > 0 {
            row.status = "metadata_only".to_string();
            row.status_reason =
                "signed HTTPS update release metadata exists; hosted artifact storage is optional"
                    .to_string();
        } else if update_source_accepts_external_url(row) {
            row.status = "selected_no_artifacts".to_string();
            row.status_reason =
                "update artifact source is selected, but no hosted store or signed external release metadata exists"
                    .to_string();
        } else {
            row.status = "selected_no_store".to_string();
            row.status_reason =
                "update artifact-source preset is selected, but no server object store is configured"
                    .to_string();
        }
    }
}

fn update_source_accepts_external_url(row: &DataSourceStatusView) -> bool {
    row.source_kind.contains("https") || row.preset_name.contains("https")
}

fn enrich_runtime_tunnel_status_rows(
    rows: &mut [DataSourceStatusView],
    plans: &[TunnelPlanView],
    trends: &[NetworkObservationTrendView],
    recommendations: &[NetworkOspfRecommendationView],
) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "runtime_tunnel_adapter" | "routing_daemon_adapter"
        )
    }) {
        let client_plans = plans
            .iter()
            .filter(|plan| tunnel_plan_touches_client(plan, &row.client_id))
            .collect::<Vec<_>>();
        let client_trends = trends
            .iter()
            .filter(|trend| network_trend_touches_client(trend, &row.client_id))
            .collect::<Vec<_>>();
        let observation_sample_count: i64 =
            client_trends.iter().map(|trend| trend.sample_count).sum();
        let degraded_observation_count: i64 =
            client_trends.iter().map(|trend| trend.degraded_count).sum();
        let network_status_sample_count: i64 = client_trends
            .iter()
            .filter(|trend| trend.kind == "network_status")
            .map(|trend| trend.sample_count)
            .sum();
        let probe_sample_count: i64 = client_trends
            .iter()
            .filter(|trend| trend.kind == "network_probe")
            .map(|trend| trend.sample_count)
            .sum();
        let speed_sample_count: i64 = client_trends
            .iter()
            .filter(|trend| trend.kind == "network_speed_test")
            .map(|trend| trend.sample_count)
            .sum();
        let routing_recommendation_count = recommendations
            .iter()
            .filter(|recommendation| {
                ospf_recommendation_touches_client(recommendation, &row.client_id)
            })
            .count();
        let ospf_update_candidate_count = recommendations
            .iter()
            .filter(|recommendation| {
                ospf_recommendation_touches_client(recommendation, &row.client_id)
                    && recommendation.cost_delta != 0
            })
            .count();
        let runtime_evidence = json!({
            "network_status_sample_count": network_status_sample_count,
            "network_observation_sample_count": observation_sample_count,
            "network_observation_degraded_count": degraded_observation_count,
            "probe_sample_count": probe_sample_count,
            "speed_sample_count": speed_sample_count,
            "saved_plan_count": client_plans.len(),
            "routing_recommendation_count": routing_recommendation_count,
            "ospf_update_candidate_count": ospf_update_candidate_count,
            "routing_status_source": "network_observation_trends",
            "continuous_status": true,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.domain == "routing_daemon_adapter" && row.status != "agent_offline" {
            row.status = if degraded_observation_count > 0 {
                "degraded".to_string()
            } else if routing_recommendation_count > 0 || network_status_sample_count > 0 {
                "ready".to_string()
            } else {
                "ready_on_demand".to_string()
            };
            row.status_reason =
                "routing daemon adapter is selected; network status, topology trends, and OSPF recommendations provide evidence"
                    .to_string();
        }
    }
}

fn enrich_runtime_traffic_status_rows(rows: &mut [DataSourceStatusView], plans: &[TunnelPlanView]) {
    for row in rows.iter_mut().filter(|row| {
        matches!(
            row.domain.as_str(),
            "runtime_traffic_accounting_source" | "traffic_limit_status_source"
        )
    }) {
        let client_plans = plans
            .iter()
            .filter(|plan| tunnel_plan_touches_client(plan, &row.client_id))
            .collect::<Vec<_>>();
        let traffic_limit_plan_count = client_plans
            .iter()
            .filter(|plan| tunnel_plan_has_traffic_limit(plan))
            .count();
        let traffic_limit_apply_plan_count = client_plans
            .iter()
            .filter(|plan| plan.plan.runtime_control.traffic_limit_apply.is_some())
            .count();
        let runtime_evidence = json!({
            "traffic_shaping_status_source": "tunnel_plan_runtime_control",
            "saved_plan_count": client_plans.len(),
            "traffic_limit_plan_count": traffic_limit_plan_count,
            "traffic_limit_apply_plan_count": traffic_limit_apply_plan_count,
            "continuous_status": true,
        });
        row.evidence = merge_evidence(row.evidence.take(), runtime_evidence);
        if row.domain == "traffic_limit_status_source" && row.status != "agent_offline" {
            row.status = if traffic_limit_plan_count > 0 {
                "ready".to_string()
            } else {
                "selected_no_limits".to_string()
            };
            row.status_reason = if traffic_limit_plan_count > 0 {
                "traffic-limit status source is selected and tunnel plans contain limit intent"
                    .to_string()
            } else {
                "traffic-limit status source is selected, but no tunnel traffic limits are planned"
                    .to_string()
            };
        }
    }
}

fn tunnel_plan_touches_client(plan: &TunnelPlanView, client_id: &str) -> bool {
    plan.left_client_id == client_id || plan.right_client_id == client_id
}

fn tunnel_plan_has_traffic_limit(plan: &TunnelPlanView) -> bool {
    plan.plan.runtime_control.traffic_limit_apply.is_some()
        || !plan.plan.runtime_control.traffic_limit.is_default()
}

fn network_trend_touches_client(trend: &NetworkObservationTrendView, client_id: &str) -> bool {
    trend.client_id == client_id || trend.peer_client_id.as_deref() == Some(client_id)
}

fn ospf_recommendation_touches_client(
    recommendation: &NetworkOspfRecommendationView,
    client_id: &str,
) -> bool {
    recommendation.left_client_id == client_id || recommendation.right_client_id == client_id
}

fn merge_evidence(base: Value, extra: Value) -> Value {
    let mut merged = match base {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if let Value::Object(extra) = extra {
        for (key, value) in extra {
            merged.insert(key, value);
        }
    }
    Value::Object(merged)
}
