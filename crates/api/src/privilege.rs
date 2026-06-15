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
    let result = state
        .gateway
        .verify_privilege(intent, assertion)
        .await
        .map_err(|_| ApiError::forbidden("privilege_verification_failed"))?;
    if result.approved {
        Ok(())
    } else {
        Err(ApiError::forbidden("privilege_verification_denied"))
    }
}
