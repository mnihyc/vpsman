use super::system_operator;

#[test]
fn system_authority_has_no_operator_session_evidence() {
    assert_eq!(system_operator("test-controller").audit_session_id(), None);
}
