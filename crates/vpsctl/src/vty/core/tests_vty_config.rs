use super::{
    parse_vty_agent_update, parse_vty_agent_update_activate, parse_vty_agent_update_check,
    parse_vty_agent_update_rollback,
};

#[test]
fn parses_agent_update_request() {
    let sha256_hex = "ab".repeat(32);
    let request = parse_vty_agent_update(&[
        "--artifact-url",
        "https://updates.example/vpsman-agent",
        "--sha256-hex",
        &sha256_hex,
        "id:edge-a",
        "--max-timeout",
        "300",
        "--privilege-ttl",
        "120",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.artifact_url, "https://updates.example/vpsman-agent");
    assert_eq!(request.sha256_hex, "ab".repeat(32));
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["id:edge-a"]);
    assert_eq!(request.max_timeout_secs, 300);
    assert_eq!(request.privilege_ttl_secs, 120);
    assert!(request.force_unprivileged);
}

#[test]
fn parses_agent_update_check_request() {
    let request = parse_vty_agent_update_check(&[
        "--version-url",
        "https://github.com/mnihyc/vpsman/releases/latest/download/version.json",
        "tag:edge",
        "--max-timeout",
        "300",
        "--privilege-ttl",
        "120",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(
        request.version_url,
        Some("https://github.com/mnihyc/vpsman/releases/latest/download/version.json".to_string())
    );
    assert!(!request.activate);
    assert!(!request.restart_agent);
    assert_eq!(request.selection.tags, vec!["edge"]);
    assert_eq!(request.max_timeout_secs, 300);
    assert_eq!(request.privilege_ttl_secs, 120);
    assert!(request.force_unprivileged);

    let activate = parse_vty_agent_update_check(&[
        "--version-url",
        "file:///tmp/version.json",
        "id:edge-a",
        "--activate",
        "--restart-agent",
        "--confirmed",
    ])
    .unwrap();
    assert!(activate.activate);
    assert!(activate.restart_agent);

    assert!(
        parse_vty_agent_update_check(&["id:edge-a", "--restart-agent", "--confirmed",]).is_err()
    );
}

#[test]
fn parses_agent_update_activation_and_rollback_requests() {
    let activate = parse_vty_agent_update_activate(&[
        "--staged-sha256-hex",
        &"aa".repeat(32),
        "id:edge-a",
        "--max-timeout",
        "30",
        "--restart-agent",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();
    assert_eq!(activate.staged_sha256_hex, "aa".repeat(32));
    assert!(activate.selection.clients.is_empty());
    assert_eq!(activate.selection.tags, vec!["id:edge-a"]);
    assert!(activate.restart_agent);
    assert!(activate.selection.confirmed);
    assert!(activate.force_unprivileged);

    let rollback = parse_vty_agent_update_rollback(&[
        "--rollback-sha256-hex",
        &"bb".repeat(32),
        "tag:bgp",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();
    assert_eq!(rollback.rollback_sha256_hex, Some("bb".repeat(32)));
    assert_eq!(rollback.selection.tags, vec!["bgp"]);
    assert!(rollback.selection.confirmed);
    assert!(rollback.force_unprivileged);
}

#[test]
fn rejects_unconfirmed_or_bad_agent_update_activation_requests() {
    assert!(parse_vty_agent_update_activate(&[
        "--staged-sha256-hex",
        &"aa".repeat(32),
        "id:edge-a",
    ])
    .is_err());
    assert!(parse_vty_agent_update_activate(&[
        "--staged-sha256-hex",
        "not-a-hash",
        "id:edge-a",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_agent_update_rollback(&[
        "--rollback-sha256-hex",
        "not-a-hash",
        "id:edge-a",
        "--confirmed",
    ])
    .is_err());
}

#[test]
fn rejects_unconfirmed_or_non_https_agent_update() {
    assert!(parse_vty_agent_update(&[
        "--artifact-url",
        "https://updates.example/vpsman-agent",
        "--sha256-hex",
        &"ab".repeat(32),
        "tag:edge",
    ])
    .is_err());
    assert!(parse_vty_agent_update(&[
        "--artifact-url",
        "http://updates.example/vpsman-agent",
        "--sha256-hex",
        &"ab".repeat(32),
        "tag:edge",
        "--confirmed",
    ])
    .is_err());
    assert!(parse_vty_agent_update_check(&[
        "--version-url",
        "http://updates.example/version.json",
        "tag:edge",
        "--confirmed",
    ])
    .is_err());
}
