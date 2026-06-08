use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BuildInfoView {
    pub(crate) component: &'static str,
    pub(crate) version: &'static str,
    pub(crate) build_number: u64,
    pub(crate) build_number_scope: &'static str,
}

pub(crate) fn server_build_number() -> u64 {
    vpsman_server_build_info::server_build_number()
}

pub(crate) fn server_build_info() -> BuildInfoView {
    BuildInfoView {
        component: "server",
        version: env!("CARGO_PKG_VERSION"),
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
    }
}
