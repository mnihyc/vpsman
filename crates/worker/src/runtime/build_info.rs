pub(crate) fn release_version() -> &'static str {
    vpsman_server_build_info::release_version()
}

pub(crate) fn server_build_number() -> u64 {
    vpsman_server_build_info::server_build_number()
}
