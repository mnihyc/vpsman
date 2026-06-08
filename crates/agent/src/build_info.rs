pub(crate) const AGENT_BUILD_NUMBER: &str = env!("VPSMAN_AGENT_BUILD_NUMBER");

pub(crate) fn agent_build_number() -> u64 {
    vpsman_common::parse_build_number(Some(AGENT_BUILD_NUMBER))
}
