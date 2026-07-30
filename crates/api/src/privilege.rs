use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use vpsman_common::PrivilegeAssertion;

use crate::{gateway_client::GatewayControlResponseError, state::AppState, ApiError};

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
    headers: HeaderMap,
    Json(request): Json<PrivilegeUnlockVerificationRequest>,
) -> Result<Json<PrivilegeUnlockVerificationResponse>, ApiError> {
    let operator = state.require_operator(&headers).await?;
    let target = operator.operator.id.to_string();
    let intent = DbPrivilegeIntent::new(PRIVILEGE_UNLOCK_ACTION, &target, None, &[], true, None);
    verify_privilege_intent(&state, &intent, request.privilege_assertion).await?;
    Ok(Json(PrivilegeUnlockVerificationResponse { verified: true }))
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
