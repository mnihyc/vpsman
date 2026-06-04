use std::path::PathBuf;

use uuid::Uuid;

use crate::vty_backups::{
    parse_vty_backup_policy_upsert, parse_vty_backup_request, parse_vty_backup_run,
    parse_vty_restore_plan, parse_vty_restore_rollback, parse_vty_restore_run,
};

#[test]
fn parses_vty_backup_request_scope_and_confirmation() {
    let request = parse_vty_backup_request(&[
        "client-a",
        "--path",
        "/etc/hostname",
        "--include-config",
        "--confirmed",
        "--note=pre-migration",
    ])
    .unwrap();

    assert_eq!(request.client_id, "client-a");
    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert!(request.confirmed);
    assert_eq!(request.note.as_deref(), Some("pre-migration"));
}

#[test]
fn rejects_vty_backup_without_scope_or_absolute_path() {
    assert!(parse_vty_backup_request(&["client-a"]).is_err());
    assert!(parse_vty_backup_request(&["client-a", "--path", "relative"]).is_err());
}

#[test]
fn parses_vty_backup_run_targets_and_scope() {
    let request = parse_vty_backup_run(&[
        "--path",
        "/etc/hostname",
        "--include-config",
        "--timeout=90",
        "client:client-a",
        "tag:edge",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert_eq!(request.timeout_secs, 90);
    assert_eq!(request.selection.clients, vec!["client-a"]);
    assert_eq!(request.selection.tags, vec!["edge"]);
    assert!(request.selection.confirmed);
}

#[test]
fn rejects_vty_backup_run_without_safe_scope_or_target() {
    assert!(parse_vty_backup_run(&["--path", "relative", "client:client-a"]).is_err());
    assert!(parse_vty_backup_run(&["--include-config"]).is_err());
}

#[test]
fn parses_vty_backup_policy_upsert() {
    let recipient = "a".repeat(64);
    let request = parse_vty_backup_policy_upsert(&[
        "nightly-edge",
        "--path",
        "/etc/hostname",
        "--include-config",
        "--recipient-public-key-hex",
        &recipient,
        "--interval=3600",
        "--retention-days=45",
        "--keep-last=12",
        "--rotation-generation=keyring/v2",
        "client:client-a",
        "tag:edge",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.name, "nightly-edge");
    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert_eq!(
        request.recipient_public_key_hex.as_deref(),
        Some(recipient.as_str())
    );
    assert_eq!(request.interval_secs, 3600);
    assert_eq!(request.retention_days, Some(45));
    assert_eq!(request.keep_last, Some(12));
    assert_eq!(request.rotation_generation.as_deref(), Some("keyring/v2"));
    assert_eq!(request.selection.clients, vec!["client-a"]);
    assert_eq!(request.selection.tags, vec!["edge"]);
    assert!(request.selection.confirmed);
}

#[test]
fn rejects_vty_backup_policy_without_safe_recipient() {
    assert!(parse_vty_backup_policy_upsert(&[
        "nightly-edge",
        "--include-config",
        "--recipient-public-key-hex",
        "bad",
        "client:client-a",
        "--confirmed",
    ])
    .is_err());
}

#[test]
fn parses_vty_restore_plan_scope_and_confirmation() {
    let source = Uuid::new_v4().to_string();
    let request = parse_vty_restore_plan(&[
        &source,
        "client-b",
        "--path",
        "/etc/hostname",
        "--include-config",
        "--destination-root=/restore",
        "--confirmed",
        "--note=restore-rehearsal",
    ])
    .unwrap();

    assert_eq!(request.source_backup_request_id.to_string(), source);
    assert_eq!(request.target_client_id, "client-b");
    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert_eq!(request.destination_root.as_deref(), Some("/restore"));
    assert!(request.confirmed);
    assert_eq!(request.note.as_deref(), Some("restore-rehearsal"));
}

#[test]
fn rejects_vty_restore_plan_without_scope_or_absolute_path() {
    let source = Uuid::new_v4().to_string();
    assert!(parse_vty_restore_plan(&[&source, "client-b"]).is_err());
    assert!(parse_vty_restore_plan(&[&source, "client-b", "--path", "relative"]).is_err());
    assert!(parse_vty_restore_plan(&[
        &source,
        "client-b",
        "--include-config",
        "--destination-root",
        "relative"
    ])
    .is_err());
}

#[test]
fn parses_vty_restore_run_for_single_target_execution() {
    let source = Uuid::new_v4().to_string();
    let request = parse_vty_restore_run(&[
        &source,
        "client-b",
        "--artifact-file",
        "./artifact.json",
        "--private-key-env=BACKUP_KEY",
        "--path",
        "/etc/hostname",
        "--include-config",
        "--destination-root=/restore",
        "--timeout=120",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.source_backup_request_id.to_string(), source);
    assert_eq!(request.target_client_id, "client-b");
    assert_eq!(
        request.artifact_file,
        Some(PathBuf::from("./artifact.json"))
    );
    assert_eq!(request.private_key_env, "BACKUP_KEY");
    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert_eq!(request.destination_root.as_deref(), Some("/restore"));
    assert_eq!(request.timeout_secs, 120);
    assert!(request.confirmed);
    assert!(request.force_unprivileged);
}

#[test]
fn parses_vty_restore_run_without_local_artifact_file() {
    let source = Uuid::new_v4().to_string();
    let request = parse_vty_restore_run(&[
        &source,
        "client-b",
        "--path",
        "/etc/hostname",
        "--destination-root",
        "/restore",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.source_backup_request_id.to_string(), source);
    assert_eq!(request.target_client_id, "client-b");
    assert_eq!(request.artifact_file, None);
    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert_eq!(request.destination_root.as_deref(), Some("/restore"));
    assert!(request.confirmed);
}

#[test]
fn parses_vty_restore_rollback_for_single_target_execution() {
    let restore_job_id = Uuid::new_v4();
    let request = parse_vty_restore_rollback(&[
        &restore_job_id.to_string(),
        "client-b",
        "--timeout",
        "30",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.restore_job_id, restore_job_id);
    assert_eq!(request.target_client_id, "client-b");
    assert_eq!(request.timeout_secs, 30);
    assert!(request.confirmed);
    assert!(request.force_unprivileged);
}

#[test]
fn rejects_vty_restore_run_without_confirmation_or_safe_scope() {
    let source = Uuid::new_v4().to_string();
    assert!(parse_vty_restore_run(&[&source, "client-b", "--path", "/etc/hostname",]).is_err());
    assert!(
        parse_vty_restore_run(&[&source, "client-b", "--path", "relative", "--confirmed",])
            .is_err()
    );
    assert!(parse_vty_restore_run(&[&source, "client-b", "--confirmed"]).is_err());
}
