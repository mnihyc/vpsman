use uuid::Uuid;
use vpsman_common::{JobCommand, RestoreRollbackFile};

use crate::{
    job_request::validate_job_command, model::CreateRestorePlanRequest,
    routes_restores::validate_create_restore_plan,
};

#[test]
fn restore_job_validation_requires_safe_agent_local_archive() {
    let source_backup_request_id = Uuid::new_v4();
    let missing_archive = JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id: Uuid::new_v4(),
        paths: vec!["/tmp/source.txt".to_string()],
        include_config: false,
        destination_root: Some("/restore".to_string()),
        archive_path: None,
        archive_size_bytes: None,
        archive_sha256_hex: None,
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    assert_eq!(
        validate_job_command(&missing_archive).unwrap_err().code,
        "restore_archive_path_required"
    );

    let valid = JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id: Uuid::new_v4(),
        paths: vec!["/tmp/source.txt".to_string()],
        include_config: false,
        destination_root: Some("/restore".to_string()),
        archive_path: Some("/var/lib/vpsman/restore/archive.tar".to_string()),
        archive_size_bytes: Some(42),
        archive_sha256_hex: Some(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        ),
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    validate_job_command(&valid).unwrap();

    let unsafe_path = JobCommand::Restore {
        source_backup_request_id,
        archive_transfer_session_id: Uuid::new_v4(),
        paths: vec!["/tmp/../source.txt".to_string()],
        include_config: false,
        destination_root: Some("/restore".to_string()),
        archive_path: Some("/var/lib/vpsman/restore/archive.tar".to_string()),
        archive_size_bytes: Some(42),
        archive_sha256_hex: Some(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
        ),
        dry_run: false,
        post_restore_argv: Vec::new(),
    };
    assert_eq!(
        validate_job_command(&unsafe_path).unwrap_err().code,
        "restore_path_invalid"
    );
}

#[test]
fn restore_rollback_job_validation_requires_safe_manifest() {
    let empty = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: Vec::new(),
    };
    assert_eq!(
        validate_job_command(&empty).unwrap_err().code,
        "restore_rollback_files_required"
    );

    let unsafe_destination = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: vec![RestoreRollbackFile {
            archive_path: "/tmp/source.txt".to_string(),
            destination_path: "/restore/../source.txt".to_string(),
            rollback_path: None,
            restored_size_bytes: 4,
            restored_sha256_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
        }],
    };
    assert_eq!(
        validate_job_command(&unsafe_destination).unwrap_err().code,
        "restore_rollback_destination_path_invalid"
    );

    let valid = JobCommand::RestoreRollback {
        source_restore_job_id: Uuid::new_v4(),
        restored_files: vec![RestoreRollbackFile {
            archive_path: "/tmp/source.txt".to_string(),
            destination_path: "/restore/tmp/source.txt".to_string(),
            rollback_path: Some("/restore/tmp/.vpsman-restore-source.txt-job.bak".to_string()),
            restored_size_bytes: 4,
            restored_sha256_hex: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_string(),
        }],
    };
    validate_job_command(&valid).unwrap();
}

#[test]
fn restore_plan_validation_requires_scope_and_confirmation() {
    let backup_id = Uuid::new_v4();
    let missing_scope = CreateRestorePlanRequest {
        source_backup_request_id: backup_id,
        target_client_id: "client-b".to_string(),
        paths: Vec::new(),
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_restore_plan(&missing_scope)
            .unwrap_err()
            .code,
        "restore_scope_required"
    );

    let relative_path = CreateRestorePlanRequest {
        source_backup_request_id: backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["relative".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: true,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_restore_plan(&relative_path)
            .unwrap_err()
            .code,
        "file_path_must_be_absolute"
    );

    let unconfirmed = CreateRestorePlanRequest {
        source_backup_request_id: backup_id,
        target_client_id: "client-b".to_string(),
        paths: vec!["/etc/hostname".to_string()],
        include_config: false,
        destination_root: None,
        confirmed: false,
        note: None,
        privilege_assertion: None,
    };
    assert_eq!(
        validate_create_restore_plan(&unconfirmed).unwrap_err().code,
        "restore_confirmation_required"
    );
}
