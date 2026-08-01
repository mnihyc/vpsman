use uuid::Uuid;
use vpsman_common::JobCommand;

use crate::{
    model::{AuthContext, OperatorPreferences, OperatorView},
    DEFAULT_REFRESH_TOKEN_TTL_SECS,
};

pub(crate) fn system_operator(username: &str) -> AuthContext {
    AuthContext {
        operator: OperatorView {
            id: Uuid::nil(),
            username: username.to_string(),
            role: "system".to_string(),
            scopes: vec!["*".to_string()],
            preferences: OperatorPreferences::default(),
            totp_enabled: false,
            status: "active".to_string(),
            session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
            created_at: crate::unix_now().to_string(),
            disabled_at: None,
            deleted_at: None,
        },
        session_id: None,
    }
}

pub(crate) fn persisted_actor_id(operator: &AuthContext) -> Option<Uuid> {
    if operator.operator.id.is_nil() && operator.operator.role == "system" {
        None
    } else {
        Some(operator.operator.id)
    }
}

pub(crate) fn server_issued_job_actor(command: &JobCommand) -> Option<&'static str> {
    match command {
        JobCommand::RuntimeConfigSync { .. } => Some("runtime-config-controller"),
        JobCommand::NetworkRoutingStatus { .. } | JobCommand::NetworkRoutingApply { .. } => {
            Some("network-routing-controller")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::system_operator;

    #[test]
    fn system_authority_has_no_operator_session_evidence() {
        assert_eq!(system_operator("test-controller").audit_session_id(), None);
    }
}
