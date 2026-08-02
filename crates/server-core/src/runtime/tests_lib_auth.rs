use super::{default_operator_scopes, operator_is_active_authorized};

#[test]
fn active_operator_authority_requires_status_role_and_all_scopes() {
    let scopes = default_operator_scopes("operator");
    assert!(operator_is_active_authorized(
        "active",
        "operator",
        &scopes,
        "operator",
        &["jobs:write", "schedules:write"],
    ));
    assert!(!operator_is_active_authorized(
        "disabled",
        "operator",
        &scopes,
        "operator",
        &["jobs:write"],
    ));
    assert!(!operator_is_active_authorized(
        "active",
        "viewer",
        &default_operator_scopes("viewer"),
        "operator",
        &["jobs:write"],
    ));
    assert!(!operator_is_active_authorized(
        "active",
        "operator",
        &["jobs:write".to_string()],
        "operator",
        &["jobs:write", "schedules:write"],
    ));
}
