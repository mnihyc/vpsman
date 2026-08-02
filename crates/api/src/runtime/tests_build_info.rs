use super::server_build_info;

#[test]
fn build_info_uses_server_scope() {
    let info = server_build_info();
    assert_eq!(info.component, "server");
    assert_eq!(info.build_number_scope, "server");
    assert!(info.build_number > 0);
    assert!(!info.version.is_empty());
    assert!(!info.package_version.is_empty());
}
