use super::require_configuration_preset_changes;
use axum::http::StatusCode;

#[test]
fn preset_update_rejects_an_unchanged_preview() {
    let error = require_configuration_preset_changes(&[]).unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.code, "configuration_preset_no_changes");
    require_configuration_preset_changes(&["description".to_string()]).unwrap();
}
