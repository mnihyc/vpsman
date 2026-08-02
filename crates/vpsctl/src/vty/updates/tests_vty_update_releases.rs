use super::*;

#[test]
fn parses_vty_agent_update_release_record() {
    let sha256_hex = "aa".repeat(32);
    let rollback_sha256_hex = "bb".repeat(32);
    let request = parse_vty_agent_update_release_record(&[
        "--name",
        "vpsman-agent",
        "--version",
        "1.2.3",
        "--channel",
        "stable",
        "--artifact-url",
        "https://updates.example/vpsman-agent",
        "--sha256-hex",
        &sha256_hex,
        "--rollback-artifact-url",
        "https://updates.example/vpsman-agent.rollback",
        "--rollback-sha256-hex",
        &rollback_sha256_hex,
        "--size-bytes",
        "1024",
        "--rollback-size-bytes",
        "512",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.name, "vpsman-agent");
    assert_eq!(request.version, "1.2.3");
    assert_eq!(request.channel, "stable");
    assert_eq!(request.size_bytes, Some(1024));
    assert_eq!(request.rollback_size_bytes, Some(512));
    assert_eq!(
        request.rollback_artifact_url.as_deref(),
        Some("https://updates.example/vpsman-agent.rollback")
    );
    assert_eq!(
        request.rollback_sha256_hex.as_deref(),
        Some(&*rollback_sha256_hex)
    );
}

#[test]
fn parses_vty_latest_release_command() {
    let (name, channel) = parse_vty_latest_release_command(
        "agent-update-release-latest --name vpsman-agent --channel beta",
    )
    .unwrap();
    assert_eq!(name, "vpsman-agent");
    assert_eq!(channel, "beta");
}

#[test]
fn rejects_unconfirmed_or_bad_vty_agent_update_release_record() {
    assert!(parse_vty_agent_update_release_record(&["--name", "vpsman-agent"]).is_err());
    assert!(parse_vty_agent_update_release_record(&[
        "--name",
        "vpsman-agent",
        "--version",
        "1.2.3",
        "--artifact-url",
        "https://updates.example/vpsman-agent",
        "--sha256-hex",
        &"aa".repeat(32),
        "--rollback-artifact-url",
        "https://updates.example/vpsman-agent.rollback",
        "--confirmed",
    ])
    .is_err());
    assert!(submit_vty_agent_update_releases(
        "http://127.0.0.1:1",
        None,
        "agent-update-releases --limit 0"
    )
    .is_err());
}
