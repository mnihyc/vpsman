pub(crate) const AGENT_BUILD_NUMBER: &str = env!("VPSMAN_AGENT_BUILD_NUMBER");

pub(crate) fn agent_release_version() -> &'static str {
    vpsman_common::release_version()
}

pub(crate) fn agent_build_number() -> u64 {
    vpsman_common::parse_build_number(Some(AGENT_BUILD_NUMBER))
}
