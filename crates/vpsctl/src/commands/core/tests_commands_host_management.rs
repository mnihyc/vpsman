use super::*;

#[test]
fn provider_and_action_names_are_explicit() {
    assert_eq!(
        parse_service_provider("openrc").unwrap(),
        HostServiceProvider::Openrc
    );
    assert_eq!(
        parse_service_action("restart").unwrap(),
        HostServiceAction::Restart
    );
    assert_eq!(
        parse_package_provider("yum").unwrap(),
        HostPackageProvider::Yum
    );
    assert!(parse_service_provider("auto").is_err());
    assert!(parse_package_provider("auto").is_err());
}
