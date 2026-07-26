use axum::http::StatusCode;
use serde::Serialize;
use vpsman_common::PrivilegeAssertion;

use crate::{state::AppState, ApiError};

pub(crate) use vpsman_common::{
    DbPrivilegeIntent, JobPrivilegeIntent, JobPrivilegeIntentInput, SchedulePrivilegeIntent,
    SchedulePrivilegeIntentInput,
};

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
        Err(_) => return Err(ApiError::forbidden("privilege_verification_failed")),
    };
    if result.approved {
        Ok(())
    } else {
        Err(ApiError::forbidden("privilege_verification_denied"))
    }
}
