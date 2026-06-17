pub(crate) const CLI_BUILD_NUMBER: &str = env!("VPSMAN_CLI_BUILD_NUMBER");

pub(crate) fn cli_release_version() -> &'static str {
    vpsman_common::release_version()
}
