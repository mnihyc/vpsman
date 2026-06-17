use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BuildInfoView {
    pub(crate) component: &'static str,
    pub(crate) version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) release_tag: Option<&'static str>,
    pub(crate) package_version: &'static str,
    pub(crate) build_number: u64,
    pub(crate) build_number_scope: &'static str,
}

pub(crate) fn release_version() -> &'static str {
    vpsman_server_build_info::release_version()
}

pub(crate) fn release_tag() -> Option<&'static str> {
    vpsman_server_build_info::release_tag()
}

pub(crate) fn server_build_number() -> u64 {
    vpsman_server_build_info::server_build_number()
}

pub(crate) fn server_build_info() -> BuildInfoView {
    BuildInfoView {
        component: "server",
        version: release_version(),
        release_tag: release_tag(),
        package_version: vpsman_common::package_version(),
        build_number: server_build_number(),
        build_number_scope: "server",
    }
}

#[cfg(test)]
mod tests {
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
}
