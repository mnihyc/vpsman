use std::net::SocketAddr;

use axum::{
    extract::{ConnectInfo, State},
    http::{header::USER_AGENT, HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::PrivilegeAssertion;

use crate::{
    gateway_client::GatewayControlResponseError,
    model::{AuditLogView, AuthContext},
    repository::Repository,
    state::AppState,
    unix_now, ApiError,
};

pub(crate) use vpsman_common::{
    DbPrivilegeIntent, JobPrivilegeIntent, JobPrivilegeIntentInput, SchedulePrivilegeIntent,
    SchedulePrivilegeIntentInput,
};

const PRIVILEGE_UNLOCK_ACTION: &str = "privilege.unlock";

#[derive(Debug, Deserialize)]
pub(crate) struct PrivilegeUnlockVerificationRequest {
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Debug, Serialize)]
pub(crate) struct PrivilegeUnlockVerificationResponse {
    pub(crate) verified: bool,
}

pub(crate) async fn verify_privilege_unlock(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<PrivilegeUnlockVerificationRequest>,
) -> Result<Json<PrivilegeUnlockVerificationResponse>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    let remote_ip = state.operator_client_ip(peer, &headers);
    let user_agent = headers
        .get(USER_AGENT)
        .and_then(|value| value.to_str().ok());
    let target = operator.operator.id.to_string();
    let intent = DbPrivilegeIntent::new(PRIVILEGE_UNLOCK_ACTION, &target, None, &[], true, None);
    if let Err(error) = verify_privilege_intent(&state, &intent, request.privilege_assertion).await
    {
        let result = if error.status == StatusCode::SERVICE_UNAVAILABLE
            || error.code == "gateway_control_url_missing"
        {
            "unavailable"
        } else {
            "denied"
        };
        state
            .repo
            .record_privilege_unlock_audit(
                &operator,
                &remote_ip,
                user_agent,
                result,
                Some(error.code),
            )
            .await?;
        return Err(error);
    }
    state
        .repo
        .record_privilege_unlock_audit(&operator, &remote_ip, user_agent, "succeeded", None)
        .await?;
    Ok(Json(PrivilegeUnlockVerificationResponse { verified: true }))
}

impl Repository {
    async fn record_privilege_unlock_audit(
        &self,
        operator: &AuthContext,
        remote_ip: &str,
        user_agent: Option<&str>,
        result: &str,
        reason: Option<&str>,
    ) -> anyhow::Result<()> {
        let metadata = serde_json::json!({
            "operator_id": operator.operator.id,
            "operator_username": &operator.operator.username,
            "operator_role": &operator.operator.role,
            "operator_session_id": operator.audit_session_id(),
            "origin_kind": "authentication",
            "component": "privilege-verifier",
            "privilege_scope": PRIVILEGE_UNLOCK_ACTION,
            "remote_ip": remote_ip,
            "user_agent": user_agent,
            "result": result,
            "reason": reason,
        });
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: PRIVILEGE_UNLOCK_ACTION.to_string(),
                    target: "access/privilege-vault".to_string(),
                    command_hash: None,
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, NULL, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(PRIVILEGE_UNLOCK_ACTION)
                .bind("access/privilege-vault")
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

pub(crate) async fn verify_privilege_intent<T: Serialize>(
    state: &AppState,
    intent: &T,
    assertion: Option<PrivilegeAssertion>,
) -> Result<(), ApiError> {
    if !state.gateway.privilege_configured() {
        return Err(ApiError::conflict("gateway_control_url_missing"));
    }
    #[cfg(test)]
    if state.gateway.test_privilege_auto_approves() {
        return Ok(());
    }
    let assertion = assertion.ok_or_else(|| ApiError::forbidden("privilege_assertion_required"))?;
    let intent = serde_json::to_string(intent)
        .map_err(|error| ApiError::from(anyhow::Error::from(error)))?;
    state.refresh_gateway_dispatch_timeouts();
    let result = match state.gateway.verify_privilege(intent, assertion).await {
        Ok(result) => result,
        Err(error) if error.to_string().contains("ReplayProtectionSaturated") => {
            return Err(ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "privilege_replay_protection_saturated",
                error,
                public_message: Some(
                    "Privilege verification is temporarily saturated; wait for an assertion to expire and review request volume before retrying."
                        .to_string(),
                ),
            });
        }
        Err(error)
            if error
                .downcast_ref::<GatewayControlResponseError>()
                .is_some_and(|response| matches!(response.status_code, 403 | 409)) =>
        {
            return Err(ApiError::forbidden("privilege_verification_failed"));
        }
        Err(error) => {
            return Err(ApiError {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "privilege_verification_unavailable",
                error,
                public_message: Some(
                    "The gateway could not verify privilege material; the action remains locked."
                        .to_string(),
                ),
            });
        }
    };
    if result.approved {
        Ok(())
    } else {
        Err(ApiError::forbidden("privilege_verification_denied"))
    }
}
