pub const SERVER_BUILD_NUMBER: &str = env!("VPSMAN_SERVER_BUILD_NUMBER");

pub fn release_version() -> &'static str {
    vpsman_common::release_version()
}

pub fn release_tag() -> Option<&'static str> {
    vpsman_common::release_tag()
}

pub fn server_build_number() -> u64 {
    vpsman_common::parse_build_number(Some(SERVER_BUILD_NUMBER))
}
