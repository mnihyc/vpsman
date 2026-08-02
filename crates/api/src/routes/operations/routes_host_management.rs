use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;
use vpsman_common::{
    HostPackageUpdatePlanSnapshot, HostProcessSnapshot, HostServiceSnapshot, HostStorageSnapshot,
};

use crate::{
    error::ApiError,
    model_host_management::{
        HostPackageUpdatePlanView, HostProcessInventoryView, HostServiceInventoryView,
        HostStorageInventoryView,
    },
    routes_job_history::materialize_output_bytes,
    security::SCOPE_JOBS_READ,
    state::AppState,
};

#[derive(Debug, Deserialize)]
pub(crate) struct HostProcessInventoryQuery {
    pub(crate) limit: Option<usize>,
}

pub(crate) async fn get_host_process_inventory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Query(query): Query<HostProcessInventoryQuery>,
) -> Result<Json<HostProcessInventoryView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let client_id = client_id.trim();
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("client_id_invalid"));
    }
    state.repo.agent_by_id(client_id).await.map_err(|error| {
        if error.to_string().contains("agent_not_found") {
            ApiError::not_found("agent_not_found")
        } else {
            ApiError::from(error)
        }
    })?;

    let evidence = state.repo.host_process_job_evidence(client_id).await?;
    let Some(job_id) = evidence.latest_success_job_id else {
        return Ok(Json(HostProcessInventoryView {
            client_id: client_id.to_string(),
            source_job_id: None,
            source: None,
            truncated: false,
            observed_at: None,
            processes: Vec::new(),
            last_attempt: evidence.latest_attempt,
        }));
    };
    let (payload, observed_at) = materialize_job_stdout_payload(&state, job_id, client_id).await?;
    let mut snapshot = serde_json::from_slice::<HostProcessSnapshot>(&payload)
        .map_err(|_| ApiError::conflict("host_process_snapshot_invalid"))?;
    let limit = query.limit.unwrap_or(200).clamp(1, 512);
    snapshot.processes.truncate(limit);

    Ok(Json(HostProcessInventoryView {
        client_id: client_id.to_string(),
        source_job_id: Some(job_id),
        source: Some(snapshot.source),
        truncated: snapshot.truncated,
        observed_at,
        processes: snapshot.processes,
        last_attempt: evidence.latest_attempt,
    }))
}

pub(crate) async fn get_host_service_inventory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Query(query): Query<HostProcessInventoryQuery>,
) -> Result<Json<HostServiceInventoryView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let client_id = client_id.trim();
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("client_id_invalid"));
    }
    state.repo.agent_by_id(client_id).await.map_err(|error| {
        if error.to_string().contains("agent_not_found") {
            ApiError::not_found("agent_not_found")
        } else {
            ApiError::from(error)
        }
    })?;
    let evidence = state.repo.host_service_job_evidence(client_id).await?;
    let Some(job_id) = evidence.latest_success_job_id else {
        return Ok(Json(HostServiceInventoryView {
            client_id: client_id.to_string(),
            source_job_id: None,
            observed_at: None,
            capability: None,
            truncated: false,
            services: Vec::new(),
            last_attempt: evidence.latest_attempt,
        }));
    };
    let (payload, observed_at) = materialize_job_stdout_payload(&state, job_id, client_id).await?;
    let mut snapshot = serde_json::from_slice::<HostServiceSnapshot>(&payload)
        .map_err(|_| ApiError::conflict("host_service_snapshot_invalid"))?;
    let limit = query.limit.unwrap_or(500).clamp(1, 1024);
    snapshot.services.truncate(limit);
    Ok(Json(HostServiceInventoryView {
        client_id: client_id.to_string(),
        source_job_id: Some(job_id),
        observed_at,
        capability: Some(snapshot.capability),
        truncated: snapshot.truncated,
        services: snapshot.services,
        last_attempt: evidence.latest_attempt,
    }))
}

pub(crate) async fn get_host_storage_inventory(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
    Query(query): Query<HostProcessInventoryQuery>,
) -> Result<Json<HostStorageInventoryView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let client_id = client_id.trim();
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("client_id_invalid"));
    }
    state.repo.agent_by_id(client_id).await.map_err(|error| {
        if error.to_string().contains("agent_not_found") {
            ApiError::not_found("agent_not_found")
        } else {
            ApiError::from(error)
        }
    })?;
    let evidence = state.repo.host_storage_job_evidence(client_id).await?;
    let Some(job_id) = evidence.latest_success_job_id else {
        return Ok(Json(HostStorageInventoryView {
            client_id: client_id.to_string(),
            source_job_id: None,
            observed_at: None,
            capability: None,
            include_pseudo_mounts: false,
            devices_truncated: false,
            mounts_truncated: false,
            devices: Vec::new(),
            mounts: Vec::new(),
            last_attempt: evidence.latest_attempt,
        }));
    };
    let (payload, observed_at) = materialize_job_stdout_payload(&state, job_id, client_id).await?;
    let mut snapshot = serde_json::from_slice::<HostStorageSnapshot>(&payload)
        .map_err(|_| ApiError::conflict("host_storage_snapshot_invalid"))?;
    let limit = query.limit.unwrap_or(1000).clamp(1, 2048);
    snapshot.devices.truncate(limit);
    snapshot.mounts.truncate(limit);
    Ok(Json(HostStorageInventoryView {
        client_id: client_id.to_string(),
        source_job_id: Some(job_id),
        observed_at,
        capability: Some(snapshot.capability),
        include_pseudo_mounts: snapshot.include_pseudo_mounts,
        devices_truncated: snapshot.devices_truncated,
        mounts_truncated: snapshot.mounts_truncated,
        devices: snapshot.devices,
        mounts: snapshot.mounts,
        last_attempt: evidence.latest_attempt,
    }))
}

pub(crate) async fn get_host_package_update_plan(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(client_id): Path<String>,
) -> Result<Json<HostPackageUpdatePlanView>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let client_id = client_id.trim();
    if client_id.is_empty() || client_id.len() > 128 {
        return Err(ApiError::bad_request("client_id_invalid"));
    }
    state.repo.agent_by_id(client_id).await.map_err(|error| {
        if error.to_string().contains("agent_not_found") {
            ApiError::not_found("agent_not_found")
        } else {
            ApiError::from(error)
        }
    })?;
    Ok(Json(package_update_plan_view(&state, client_id).await?))
}

pub(crate) async fn list_host_package_update_plans(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<HostPackageUpdatePlanView>>, ApiError> {
    let _operator = state
        .require_operator_scope(&headers, SCOPE_JOBS_READ)
        .await?;
    let mut agents = state.repo.list_agents().await?;
    agents.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then(left.id.cmp(&right.id))
    });
    let mut views = Vec::with_capacity(agents.len());
    for agent in agents {
        match package_update_plan_view(&state, &agent.id).await {
            Ok(view) => views.push(view),
            Err(error) => views.push(HostPackageUpdatePlanView {
                client_id: agent.id,
                source_job_id: None,
                observed_at: None,
                capability: None,
                metadata_refresh_requested: false,
                metadata_refreshed: false,
                plan_hash: None,
                truncated: false,
                packages: Vec::new(),
                reboot_required_before: None,
                last_attempt: None,
                evidence_error: Some(error.code.to_string()),
            }),
        }
    }
    Ok(Json(views))
}

async fn package_update_plan_view(
    state: &AppState,
    client_id: &str,
) -> Result<HostPackageUpdatePlanView, ApiError> {
    let evidence = state.repo.host_package_plan_job_evidence(client_id).await?;
    let Some(job_id) = evidence.latest_success_job_id else {
        return Ok(HostPackageUpdatePlanView {
            client_id: client_id.to_string(),
            source_job_id: None,
            observed_at: None,
            capability: None,
            metadata_refresh_requested: false,
            metadata_refreshed: false,
            plan_hash: None,
            truncated: false,
            packages: Vec::new(),
            reboot_required_before: None,
            last_attempt: evidence.latest_attempt,
            evidence_error: None,
        });
    };
    let (payload, observed_at) = materialize_job_stdout_payload(state, job_id, client_id).await?;
    let snapshot = serde_json::from_slice::<HostPackageUpdatePlanSnapshot>(&payload)
        .map_err(|_| ApiError::conflict("host_package_plan_snapshot_invalid"))?;
    Ok(HostPackageUpdatePlanView {
        client_id: client_id.to_string(),
        source_job_id: Some(job_id),
        observed_at,
        capability: Some(snapshot.capability),
        metadata_refresh_requested: snapshot.metadata_refresh_requested,
        metadata_refreshed: snapshot.metadata_refreshed,
        plan_hash: snapshot.plan_hash,
        truncated: snapshot.truncated,
        packages: snapshot.packages,
        reboot_required_before: snapshot.reboot_required_before,
        last_attempt: evidence.latest_attempt,
        evidence_error: None,
    })
}

async fn materialize_job_stdout_payload(
    state: &AppState,
    job_id: Uuid,
    client_id: &str,
) -> Result<(Vec<u8>, Option<String>), ApiError> {
    let mut outputs = state
        .repo
        .list_job_outputs(job_id)
        .await?
        .into_iter()
        .filter(|output| output.client_id == client_id && output.stream == "stdout")
        .collect::<Vec<_>>();
    outputs.sort_by_key(|output| output.seq);
    let observed_at = outputs.last().map(|output| output.created_at.clone());
    let mut payload = Vec::new();
    for output in &outputs {
        payload.extend(materialize_output_bytes(state, output).await?);
    }
    Ok((payload, observed_at))
}
