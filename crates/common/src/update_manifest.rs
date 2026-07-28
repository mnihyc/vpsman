pub fn agent_update_asset_name_for_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("vpsman-agent-linux-x86_64-musl"),
        "aarch64" => Some("vpsman-agent-linux-aarch64-musl"),
        _ => None,
    }
}
