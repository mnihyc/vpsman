use super::{
    backup_policy_cap_notice, backup_policy_list_path, backup_policy_prune_payload,
    backup_policy_upsert_target, build_restore_plan_privilege,
    generated_restore_destination_root_with_base, restore_rollback_operation_from_outputs,
    restore_run_operation, validate_backup_policy_upsert_mode, JobOutputRecord, BASE64,
};
use base64::Engine as _;
use uuid::Uuid;
use vpsman_common::{
    canonical_job_privilege_intent, derive_super_key, encode_json, payload_hash,
    verify_privilege_assertion, BackupMissingPathPolicy, JobCommand, JobPrivilegeIntentInput,
    PrivilegeAssertionReplayCache, DEFAULT_MAX_JOB_TIMEOUT_SECS,
};

const TEST_RESTORE_ARCHIVE_PATH: &str = "/etc/hostname";
const TEST_RESTORE_DESTINATION_PATH: &str = "/restore/etc/hostname";
const TEST_RESTORE_ROLLBACK_PATH: &str = "/restore/etc/.vpsman-restore-hostname.bak";

#[test]
fn backup_policy_upsert_target_selects_create_or_update() {
    let create = backup_policy_upsert_target(None);
    assert_eq!(create.action, "backup_policy.create");
    assert_eq!(create.path, "/api/v1/backup-policies");
    assert_eq!(create.schedule_id, None);

    let schedule_id = Uuid::parse_str("52ff9113-03bd-4fa5-a166-3243681826fe").unwrap();
    let update = backup_policy_upsert_target(Some(schedule_id));
    assert_eq!(update.action, "backup_policy.update");
    assert_eq!(
        update.path,
        "/api/v1/backup-policies/52ff9113-03bd-4fa5-a166-3243681826fe"
    );
    assert_eq!(
        update.schedule_id.as_deref(),
        Some("52ff9113-03bd-4fa5-a166-3243681826fe")
    );
}

#[test]
fn backup_policy_updates_require_explicit_retention_values() {
    let schedule_id = Some(Uuid::new_v4());
    assert!(validate_backup_policy_upsert_mode(
        schedule_id,
        None,
        Some(7),
        Some("keyring/v2"),
        false,
    )
    .unwrap_err()
    .to_string()
    .contains("--retention-days"));
    assert!(validate_backup_policy_upsert_mode(
        schedule_id,
        Some(30),
        None,
        Some("keyring/v2"),
        false,
    )
    .unwrap_err()
    .to_string()
    .contains("--keep-last"));
    validate_backup_policy_upsert_mode(schedule_id, Some(30), Some(7), Some("keyring/v2"), false)
        .unwrap();
    validate_backup_policy_upsert_mode(None, None, None, None, false).unwrap();
}

#[test]
fn backup_policy_updates_require_explicit_rotation_intent() {
    let schedule_id = Some(Uuid::new_v4());
    assert!(
        validate_backup_policy_upsert_mode(schedule_id, Some(30), Some(7), None, false)
            .unwrap_err()
            .to_string()
            .contains("--rotation-generation or --clear-rotation-generation")
    );
    validate_backup_policy_upsert_mode(schedule_id, Some(30), Some(7), None, true).unwrap();
    assert!(validate_backup_policy_upsert_mode(
        schedule_id,
        Some(30),
        Some(7),
        Some("keyring/v2"),
        true,
    )
    .unwrap_err()
    .to_string()
    .contains("mutually exclusive"));
    assert!(
        validate_backup_policy_upsert_mode(None, None, None, None, true)
            .unwrap_err()
            .to_string()
            .contains("requires --schedule-id")
    );
}

#[test]
fn backup_policy_pages_are_explicit_and_disclose_a_reached_cap() {
    assert_eq!(
        backup_policy_list_path(200, 400).unwrap(),
        "/api/v1/backup-policies?limit=200&offset=400"
    );
    assert!(backup_policy_list_path(0, 0).is_err());
    assert!(backup_policy_list_path(1000, 100_001).is_err());
    assert_eq!(
        backup_policy_cap_notice(200, 400),
        "loaded 200 backup policies at offset 400; more may exist; rerun with --offset 600"
    );
    assert_eq!(
        backup_policy_cap_notice(200, 100_000),
        "loaded 200 backup policies at offset 100000; more may exist beyond the supported paging boundary"
    );
}

#[test]
fn builds_restore_rollback_operation_from_restore_status_output() {
    let restore_job_id = Uuid::new_v4();
    let status = serde_json::json!({
        "type": "restore",
        "rollback_available": true,
        "restored_files": [
            {
                "archive_path": TEST_RESTORE_ARCHIVE_PATH,
                "destination_path": TEST_RESTORE_DESTINATION_PATH,
                "size_bytes": 12,
                "sha256_hex": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "rollback_path": TEST_RESTORE_ROLLBACK_PATH
            },
            {
                "archive_path": "agent_config",
                "destination_path": "/restore/vpsman/agent_config.toml",
                "size_bytes": 21,
                "sha256_hex": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "rollback_path": null
            }
        ]
    });
    let outputs = vec![JobOutputRecord {
        client_id: "client-a".to_string(),
        stream: "status".to_string(),
        data_base64: BASE64.encode(serde_json::to_vec(&status).unwrap()),
        exit_code: Some(0),
        done: true,
    }];

    let operation =
        restore_rollback_operation_from_outputs(restore_job_id, "client-a", &outputs).unwrap();

    let JobCommand::RestoreRollback {
        source_restore_job_id,
        restored_files,
    } = operation
    else {
        panic!("expected restore rollback operation");
    };
    assert_eq!(source_restore_job_id, restore_job_id);
    assert_eq!(restored_files.len(), 2);
    assert_eq!(restored_files[0].archive_path, TEST_RESTORE_ARCHIVE_PATH);
    assert_eq!(
        restored_files[0].rollback_path.as_deref(),
        Some(TEST_RESTORE_ROLLBACK_PATH)
    );
    assert_eq!(restored_files[1].rollback_path, None);
}

#[test]
fn restore_run_operation_preserves_dry_run_mode() {
    let operation = restore_run_operation(
        Uuid::new_v4(),
        Uuid::new_v4(),
        "/var/lib/vpsman/restores/source.tar".to_string(),
        128,
        "ab".repeat(32),
        vec!["/etc/hostname".to_string()],
        false,
        Some("/var/lib/vpsman/restores/target".to_string()),
        true,
    )
    .unwrap();

    match operation {
        JobCommand::Restore { dry_run, .. } => assert!(dry_run),
        other => panic!("unexpected restore operation: {other:?}"),
    }
}

#[test]
fn restore_plan_privilege_uses_api_default_timeout() {
    let source_backup_request_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    let operation = JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id: Uuid::nil(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        destination_root: Some("/restore".to_string()),
        archive_path: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    let target_ids = vec!["client-b".to_string()];
    let selector_expression = "id:client-b";
    let assertion = build_restore_plan_privilege(
        &target_ids,
        &operation,
        selector_expression,
        "correct horse",
        "01020304",
        300,
    )
    .unwrap();
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let api_intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
        selector_expression,
        command_type: "restore",
        operation_payload_hash: &command_hash,
        rollout_policy_hash: None,
        resolved_targets: &target_ids,
        max_timeout_secs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();
    let stale_cli_intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
        selector_expression,
        command_type: "restore",
        operation_payload_hash: &command_hash,
        rollout_policy_hash: None,
        resolved_targets: &target_ids,
        max_timeout_secs: 30,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();
    let verifier_key = derive_super_key("correct horse", &[1, 2, 3, 4]);

    assert!(verify_privilege_assertion(
        &verifier_key,
        &api_intent,
        &assertion,
        assertion.issued_unix,
        &mut PrivilegeAssertionReplayCache::default(),
    )
    .is_ok());
    assert!(verify_privilege_assertion(
        &verifier_key,
        &stale_cli_intent,
        &assertion,
        assertion.issued_unix,
        &mut PrivilegeAssertionReplayCache::default(),
    )
    .is_err());
}

#[test]
fn backup_metadata_privilege_uses_api_default_timeout() {
    let operation = JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: BackupMissingPathPolicy::Fail,
    };
    let target_ids = vec!["client-a".to_string()];
    let selector_expression = "id:client-a";
    let assertion = super::build_backup_metadata_privilege(
        &target_ids,
        &operation,
        selector_expression,
        "correct horse",
        "01020304",
        300,
    )
    .unwrap();
    let command_hash = payload_hash(&encode_json(&operation).unwrap());
    let api_intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
        selector_expression,
        command_type: "backup",
        operation_payload_hash: &command_hash,
        rollout_policy_hash: None,
        resolved_targets: &target_ids,
        max_timeout_secs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();
    let stale_cli_intent = canonical_job_privilege_intent(JobPrivilegeIntentInput {
        selector_expression,
        command_type: "backup",
        operation_payload_hash: &command_hash,
        rollout_policy_hash: None,
        resolved_targets: &target_ids,
        max_timeout_secs: 30,
        force_unprivileged: false,
        privileged: true,
    })
    .unwrap();
    let verifier_key = derive_super_key("correct horse", &[1, 2, 3, 4]);

    assert!(verify_privilege_assertion(
        &verifier_key,
        &api_intent,
        &assertion,
        assertion.issued_unix,
        &mut PrivilegeAssertionReplayCache::default(),
    )
    .is_ok());
    assert!(verify_privilege_assertion(
        &verifier_key,
        &stale_cli_intent,
        &assertion,
        assertion.issued_unix,
        &mut PrivilegeAssertionReplayCache::default(),
    )
    .is_err());
}

#[test]
fn builds_backup_policy_prune_payload() {
    let schedule_id = Uuid::new_v4();
    let payload = backup_policy_prune_payload(
        Some(schedule_id.to_string()),
        true,
        Some(false),
        Some("aa".repeat(32)),
        true,
    )
    .unwrap();

    assert_eq!(payload["schedule_id"], schedule_id.to_string());
    assert_eq!(payload["dry_run"], true);
    assert_eq!(payload["metadata_only"], false);
    assert_eq!(payload["preview_hash"], "aa".repeat(32));
    assert_eq!(payload["confirmed"], true);
    assert!(
        backup_policy_prune_payload(Some("not-a-uuid".to_string()), true, None, None, false,)
            .is_err()
    );
}

#[test]
fn generated_restore_destination_root_uses_safe_base_and_segments() {
    let backup_id = Uuid::parse_str("11111111-2222-4333-8444-555555555555").unwrap();
    let root = generated_restore_destination_root_with_base(
        "/tmp/vpsman-restores/",
        backup_id,
        "edge a/../../ignored",
    )
    .unwrap();

    assert_eq!(
        root,
        "/tmp/vpsman-restores/11111111-2222-4333-8444-555555555555/edgea....ignored"
    );
    assert!(generated_restore_destination_root_with_base("relative", backup_id, "edge").is_err());
    assert!(
        generated_restore_destination_root_with_base("/tmp/root", backup_id, "..")
            .unwrap()
            .ends_with("/unknown")
    );
}
