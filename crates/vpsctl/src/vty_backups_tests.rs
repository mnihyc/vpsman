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
        "id:client-a",
        "tag:edge",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert_eq!(request.timeout_secs, 90);
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["edge", "id:client-a"]);
    assert!(request.selection.confirmed);
}

#[test]
fn rejects_vty_backup_run_without_safe_scope_or_target() {
    assert!(parse_vty_backup_run(&["--path", "relative", "id:client-a"]).is_err());
    assert!(parse_vty_backup_run(&["--include-config"]).is_err());
}

#[test]
fn parses_vty_backup_policy_upsert() {
    let request = parse_vty_backup_policy_upsert(&[
        "nightly-edge",
        "--path",
        "/etc/hostname",
        "--include-config",
        "--cron=0,3,*,*,*",
        "--retention-days=45",
        "--keep-last=12",
        "--rotation-generation=keyring/v2",
        "id:client-a",
        "tag:edge",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.name, "nightly-edge");
    assert_eq!(request.paths, vec!["/etc/hostname"]);
    assert!(request.include_config);
    assert_eq!(request.cron_expr, "0 3 * * *");
    assert_eq!(request.retention_days, Some(45));
    assert_eq!(request.keep_last, Some(12));
    assert_eq!(request.rotation_generation.as_deref(), Some("keyring/v2"));
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["edge", "id:client-a"]);
    assert!(request.selection.confirmed);
}

#[test]
fn rejects_removed_vty_backup_policy_recipient_flag() {
    assert!(parse_vty_backup_policy_upsert(&[
        "nightly-edge",
        "--include-config",
        "--recipient-public-key-hex",
        "bad",
        "id:client-a",
        "--confirmed",
    ])
    .is_err());
}

#[test]
fn parses_vty_restore_plan_records_and_confirmation() {
    let source = Uuid::new_v4().to_string();
    let request = parse_vty_restore_plan(&[
        &source,
        "client-b",
        "--confirmed",
        "--note=restore-rehearsal",
    ])
    .unwrap();

    assert_eq!(request.source_backup_request_id.to_string(), source);
    assert_eq!(request.target_client_id, "client-b");
    assert!(request.confirmed);
    assert_eq!(request.note.as_deref(), Some("restore-rehearsal"));
}

#[test]
fn rejects_vty_restore_plan_manual_scope_flags() {
    let source = Uuid::new_v4().to_string();
    assert!(parse_vty_restore_plan(&[&source, "client-b", "--path", "/etc/hostname"]).is_err());
    assert!(parse_vty_restore_plan(&[
        &source,
        "client-b",
        "--include-config",
        "--destination-root",
        "/restore"
    ])
    .is_err());
}

#[test]
fn parses_vty_restore_run_for_single_target_execution() {
    let source = Uuid::new_v4().to_string();
    let archive_transfer_session_id = Uuid::new_v4();
    let request = parse_vty_restore_run(&[
        &source,
        "client-b",
        "--archive-transfer-session-id",
        &archive_transfer_session_id.to_string(),
        "--timeout=120",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.source_backup_request_id.to_string(), source);
    assert_eq!(request.target_client_id, "client-b");
    assert_eq!(
        request.archive_transfer_session_id,
        archive_transfer_session_id
    );
    assert_eq!(request.timeout_secs, 120);
    assert!(request.confirmed);
    assert!(request.force_unprivileged);
}

#[test]
fn rejects_vty_restore_run_without_archive_transfer_record() {
    let source = Uuid::new_v4().to_string();
    assert!(parse_vty_restore_run(&[&source, "client-b", "--confirmed"]).is_err());
    assert!(parse_vty_restore_run(&[
        &source,
        "client-b",
        "--archive-transfer-session-id",
        &Uuid::new_v4().to_string(),
        "--path",
        "/etc/hostname",
        "--confirmed",
    ])
    .is_err());
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
