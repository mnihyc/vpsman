use vpsman_common::JobCommand;

use crate::{
    job_request::validate_job_command,
    model::{
        CreateBackupPolicyRequest, CreateBackupRequest, RecordBackupArtifactMetadataRequest,
        UpdateBackupPolicyRequest,
    },
    routes_backups::{
        validate_backup_artifact_metadata_request, validate_create_backup_policy_request,
        validate_create_backup_request, validate_update_backup_policy_request,
    },
};

#[test]
fn backup_request_validation_requires_safe_scope_and_confirmation() {
    let missing_scope = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: Vec::new(),
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_backup_request(&missing_scope)
            .unwrap_err()
            .code,
        "backup_scope_required"
    );

    let relative_path = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["relative".to_string()],
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_backup_request(&relative_path)
            .unwrap_err()
            .code,
        "file_path_must_be_absolute"
    );

    let unconfirmed = CreateBackupRequest {
        client_id: "client-a".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_backup_request(&unconfirmed)
            .unwrap_err()
            .code,
        "backup_confirmation_required"
    );
}

#[test]
fn backup_policy_validation_requires_targets_retention_and_confirmation() {
    let mut request = CreateBackupPolicyRequest {
        name: "nightly".to_string(),
        selector_expression: "tag:edge".to_string(),
        target_client_ids: vec!["client-a".to_string()],
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        retention_days: Some(30),
        keep_last: Some(7),
        rotation_generation: Some("keyring/v2".to_string()),
        cron_expr: "0 3 * * *".to_string(),
        timezone: "UTC".to_string(),
        enabled: true,
        catch_up_policy: "skip_missed".to_string(),
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
        confirmed: true,
        privilege_assertion: None,
    };

    validate_create_backup_policy_request(&request).unwrap();
    request.confirmed = false;
    assert_eq!(
        validate_create_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_confirmation_required"
    );
    request.confirmed = true;
    request.retention_days = Some(0);
    assert_eq!(
        validate_create_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_retention_days_out_of_range"
    );
    request.retention_days = Some(30);
    request.keep_last = Some(0);
    assert_eq!(
        validate_create_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_keep_last_out_of_range"
    );
    request.keep_last = Some(7);
    validate_update_backup_policy_request(&request).unwrap();
    request.retention_days = None;
    assert_eq!(
        validate_update_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_retention_days_required"
    );
    request.retention_days = Some(30);
    request.keep_last = None;
    assert_eq!(
        validate_update_backup_policy_request(&request)
            .unwrap_err()
            .code,
        "backup_policy_keep_last_required"
    );
}

#[test]
fn backup_policy_update_wire_contract_requires_complete_definition() {
    let full = serde_json::json!({
        "expected_definition_revision": 1,
        "name": "nightly",
        "selector_expression": "id:client-a",
        "target_client_ids": ["client-a"],
        "expected_selector_expression": "id:client-a",
        "expected_target_client_ids": ["client-a"],
        "paths": ["/etc"],
        "include_config": true,
        "follow_symlinks": false,
        "missing_path_policy": "fail",
        "retention_days": 30,
        "keep_last": 7,
        "rotation_generation": null,
        "cron_expr": "0 3 * * *",
        "timezone": "UTC",
        "enabled": false,
        "catch_up_policy": "run_once",
        "catch_up_limit": 1,
        "retry_delay_secs": 120,
        "max_failures": 5,
        "confirmed": true
    });
    let parsed: UpdateBackupPolicyRequest = serde_json::from_value(full.clone()).unwrap();
    assert!(parsed.rotation_generation.is_none());

    for required in [
        "expected_definition_revision",
        "name",
        "selector_expression",
        "target_client_ids",
        "expected_selector_expression",
        "expected_target_client_ids",
        "paths",
        "include_config",
        "follow_symlinks",
        "missing_path_policy",
        "retention_days",
        "keep_last",
        "rotation_generation",
        "cron_expr",
        "timezone",
        "enabled",
        "catch_up_policy",
        "catch_up_limit",
        "retry_delay_secs",
        "max_failures",
        "confirmed",
    ] {
        let mut incomplete = full.clone();
        incomplete.as_object_mut().unwrap().remove(required);
        assert!(
            serde_json::from_value::<UpdateBackupPolicyRequest>(incomplete).is_err(),
            "missing {required} must be rejected"
        );
    }
}

#[test]
fn backup_artifact_metadata_validation_requires_safe_metadata() {
    let unconfirmed = RecordBackupArtifactMetadataRequest {
        object_key: "backups/client-a/artifact.tar".to_string(),
        sha256_hex: "a".repeat(64),
        size_bytes: 128,
        confirmed: false,
    };
    assert_eq!(
        validate_backup_artifact_metadata_request(&unconfirmed)
            .unwrap_err()
            .code,
        "backup_artifact_confirmation_required"
    );

    let unsafe_key = RecordBackupArtifactMetadataRequest {
        object_key: "../artifact".to_string(),
        sha256_hex: "a".repeat(64),
        size_bytes: 128,
        confirmed: true,
    };
    assert_eq!(
        validate_backup_artifact_metadata_request(&unsafe_key)
            .unwrap_err()
            .code,
        "backup_artifact_object_key_invalid"
    );

    let bad_hash = RecordBackupArtifactMetadataRequest {
        object_key: "backups/client-a/artifact.tar".to_string(),
        sha256_hex: "not-a-hash".to_string(),
        size_bytes: 128,
        confirmed: true,
    };
    assert_eq!(
        validate_backup_artifact_metadata_request(&bad_hash)
            .unwrap_err()
            .code,
        "backup_artifact_invalid_sha256"
    );
}

#[test]
fn backup_job_command_validates_executable_scope() {
    validate_job_command(&JobCommand::Backup {
        paths: vec!["/etc/hostname".to_string()],
        include_config: true,
        follow_symlinks: false,
        missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
    })
    .unwrap();
    assert_eq!(
        validate_job_command(&JobCommand::Backup {
            paths: Vec::new(),
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        })
        .unwrap_err()
        .code,
        "backup_scope_required"
    );
    assert_eq!(
        validate_job_command(&JobCommand::Backup {
            paths: vec!["relative".to_string()],
            include_config: false,
            follow_symlinks: false,
            missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
        })
        .unwrap_err()
        .code,
        "file_path_must_be_absolute"
    );
}
