use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use vpsman_common::{payload_hash, CommandOutput, OutputStream};

use crate::{
    backup_handoff::{
        backup_artifact_streaming_max_bytes, stage_retained_backup_artifact_stdout,
        StagedRetainedBackupArtifact,
    },
    model::{AuthContext, BackupArtifactView, RecordBackupArtifactMetadataRequest, WsEvent},
    repository_backup_artifacts::backup_server_artifact,
    routes_backups::{
        validate_plain_backup_artifact, validate_plain_backup_artifact_file_with_limit,
    },
    state::AppState,
};

pub(crate) async fn try_auto_record_backup_artifact(
    state: &AppState,
    operator: &AuthContext,
    client_id: &str,
    command_hash: &str,
    job_id: uuid::Uuid,
    outputs: &[CommandOutput],
) -> Result<Option<BackupArtifactView>> {
    let Some(store) = state.backup_object_store.as_ref() else {
        return Ok(None);
    };
    let Some(backup_request) = state
        .repo
        .find_open_backup_request_for_job_artifact(client_id, command_hash, job_id)
        .await?
    else {
        return Ok(None);
    };
    let mut staged = match state
        .repo
        .find_backup_artifact_output_candidate(&backup_request, Some(job_id))
        .await?
    {
        Some(candidate) => Some(
            Box::pin(stage_retained_backup_artifact_stdout(
                state,
                &candidate.outputs,
            ))
            .await
            .map_err(|error| anyhow::anyhow!(error.code))?,
        ),
        None => None,
    };
    let artifact_bytes = if staged.is_none() {
        Some(collect_backup_stdout(outputs)?)
    } else {
        None
    };
    let validation_result = if let Some(prepared) = staged.as_ref() {
        validate_plain_backup_artifact_file_with_limit(
            &prepared.staging_path,
            client_id,
            backup_artifact_streaming_max_bytes(),
        )
    } else if let Some(bytes) = artifact_bytes.as_ref() {
        validate_plain_backup_artifact(bytes, client_id)
    } else {
        Err(crate::error::ApiError::conflict(
            "backup_artifact_stdout_empty",
        ))
    };
    if let Err(error) = validation_result {
        cleanup_staged_backup_artifact(staged.take()).await;
        return Err(anyhow::anyhow!(error.code));
    }
    let object_key = backup_artifact_object_key(client_id, backup_request.id);
    let (artifact_sha256_hex, artifact_size_bytes) = if let Some(prepared) = staged.as_ref() {
        (prepared.sha256_hex.clone(), prepared.size_bytes)
    } else {
        let artifact_bytes = artifact_bytes
            .as_ref()
            .expect("inline backup artifact bytes must be collected");
        (
            payload_hash(artifact_bytes),
            i64::try_from(artifact_bytes.len()).context("backup artifact too large")?,
        )
    };
    let artifact_id = backup_auto_artifact_id(backup_request.id);
    let metadata = RecordBackupArtifactMetadataRequest {
        object_key: object_key.clone(),
        sha256_hex: artifact_sha256_hex,
        size_bytes: artifact_size_bytes,
        confirmed: true,
    };
    reserve_backup_auto_artifact(state, &backup_request, artifact_id, &metadata).await?;
    let created_object = if let Some(prepared) = staged.as_ref() {
        let artifact_size_bytes = match prepared.size_bytes.try_into() {
            Ok(size) => size,
            Err(error) => {
                release_backup_auto_reservation(state, &object_key).await;
                cleanup_staged_backup_artifact(staged.take()).await;
                return Err(error).context("backup artifact size is invalid");
            }
        };
        match store
            .put_file_idempotent(
                &object_key,
                &prepared.staging_path,
                &prepared.sha256_hex,
                artifact_size_bytes,
            )
            .await
        {
            Ok(created_object) => created_object,
            Err(error) => match store
                .get_with_limit(
                    &object_key,
                    usize::try_from(prepared.size_bytes).unwrap_or(usize::MAX),
                )
                .await
            {
                Ok(existing)
                    if i64::try_from(existing.len()).ok() == Some(prepared.size_bytes)
                        && payload_hash(&existing) == prepared.sha256_hex =>
                {
                    false
                }
                Ok(_) => {
                    release_backup_auto_reservation(state, &object_key).await;
                    cleanup_staged_backup_artifact(staged.take()).await;
                    return Err(anyhow::anyhow!("backup_artifact_object_key_conflict"));
                }
                Err(_) => {
                    release_backup_auto_reservation(state, &object_key).await;
                    cleanup_staged_backup_artifact(staged.take()).await;
                    return Err(error);
                }
            },
        }
    } else {
        let artifact_bytes = artifact_bytes
            .as_ref()
            .expect("inline backup artifact bytes must be collected");
        match put_backup_artifact_object(store, &object_key, artifact_bytes).await {
            Ok(created_object) => created_object,
            Err(error) => {
                release_backup_auto_reservation(state, &object_key).await;
                cleanup_staged_backup_artifact(staged.take()).await;
                return Err(error);
            }
        }
    };
    match state
        .repo
        .record_backup_artifact_metadata(&backup_request, artifact_id, &metadata, operator)
        .await
    {
        Ok(artifact) => {
            state.publish(WsEvent::BackupArtifactRecorded {
                backup_request_id: backup_request.id,
                client_id: client_id.to_string(),
                artifact_id: artifact.id,
            });
            cleanup_staged_backup_artifact(staged.take()).await;
            Ok(Some(artifact))
        }
        Err(error) => {
            cleanup_backup_auto_reserved_object_after_error(
                state,
                store,
                &object_key,
                &error.to_string(),
                created_object,
            )
            .await;
            cleanup_staged_backup_artifact(staged.take()).await;
            Err(error)
        }
    }
}

fn backup_auto_artifact_id(backup_request_id: uuid::Uuid) -> uuid::Uuid {
    let mut digest =
        Sha256::digest(format!("vpsman:backup-auto-artifact:{backup_request_id}").as_bytes());
    digest[6] = (digest[6] & 0x0f) | 0x50;
    digest[8] = (digest[8] & 0x3f) | 0x80;
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes)
}

pub(crate) fn backup_auto_artifact_error_is_permanent(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    permanent_backup_auto_artifact_error(&message).is_some()
}

pub(crate) fn backup_auto_artifact_request_failure_reason(
    error: &anyhow::Error,
) -> Option<&'static str> {
    permanent_backup_auto_artifact_error(&error.to_string()).and_then(|(_, reason)| reason)
}

fn permanent_backup_auto_artifact_error(
    message: &str,
) -> Option<(&'static str, Option<&'static str>)> {
    [
        (
            "backup artifact exceeds auto-record limit",
            Some("backup_artifact_size_invalid"),
        ),
        (
            "backup artifact stdout is empty",
            Some("backup_artifact_stdout_empty"),
        ),
        (
            "backup artifact size is invalid",
            Some("backup_artifact_size_invalid"),
        ),
        (
            "backup_artifact_stdout_empty",
            Some("backup_artifact_stdout_empty"),
        ),
        (
            "backup_artifact_size_invalid",
            Some("backup_artifact_size_invalid"),
        ),
        (
            "backup_artifact_tar_invalid",
            Some("backup_artifact_tar_invalid"),
        ),
        (
            "backup_artifact_manifest_duplicate",
            Some("backup_artifact_manifest_duplicate"),
        ),
        (
            "backup_artifact_manifest_invalid",
            Some("backup_artifact_manifest_invalid"),
        ),
        (
            "backup_artifact_manifest_too_large",
            Some("backup_artifact_manifest_too_large"),
        ),
        (
            "backup_artifact_manifest_required",
            Some("backup_artifact_manifest_required"),
        ),
        (
            "backup_artifact_format_invalid",
            Some("backup_artifact_format_invalid"),
        ),
        (
            "backup_artifact_client_mismatch",
            Some("backup_artifact_client_mismatch"),
        ),
        (
            "backup_artifact_handoff_output_artifact_missing",
            Some("backup_artifact_handoff_output_artifact_missing"),
        ),
        (
            "backup_artifact_handoff_output_hash_mismatch",
            Some("backup_artifact_handoff_output_hash_mismatch"),
        ),
        (
            "backup_artifact_handoff_output_size_mismatch",
            Some("backup_artifact_handoff_output_size_mismatch"),
        ),
        (
            "backup_artifact_handoff_inline_output_invalid",
            Some("backup_artifact_handoff_inline_output_invalid"),
        ),
        (
            "backup_artifact_handoff_stdout_empty",
            Some("backup_artifact_handoff_stdout_empty"),
        ),
        (
            "backup_artifact_object_key_conflict",
            Some("backup_artifact_object_key_conflict"),
        ),
        ("backup_artifact_already_recorded", None),
        (
            "backup_artifact_material_conflict",
            Some("backup_artifact_material_conflict"),
        ),
        (
            "server_artifact_object_key_conflict",
            Some("server_artifact_object_key_conflict"),
        ),
        ("backup_request_not_found", None),
        ("invalid_job_operation", None),
    ]
    .into_iter()
    .find(|(permanent, _)| message == *permanent || message.starts_with(&format!("{permanent}:")))
}

async fn reserve_backup_auto_artifact(
    state: &AppState,
    backup_request: &crate::model::BackupRequestView,
    artifact_id: uuid::Uuid,
    request: &RecordBackupArtifactMetadataRequest,
) -> Result<()> {
    let artifact = BackupArtifactView {
        id: artifact_id,
        client_id: backup_request.client_id.clone(),
        object_key: request.object_key.clone(),
        sha256_hex: request.sha256_hex.clone(),
        size_bytes: request.size_bytes,
        status: "creating".to_string(),
        content_available: false,
        created_at: crate::unix_now().to_string(),
    };
    state
        .repo
        .reserve_server_artifact(backup_server_artifact(backup_request, &artifact))
        .await
}

async fn release_backup_auto_reservation(state: &AppState, object_key: &str) {
    let _ = state
        .repo
        .discard_server_artifact_reservation(object_key)
        .await;
}

async fn cleanup_backup_auto_reserved_object_after_error(
    state: &AppState,
    store: &crate::object_store::BackupObjectStore,
    object_key: &str,
    error: &str,
    created_object: bool,
) {
    if created_object {
        match store.delete_confirmed(object_key).await {
            Ok(()) => {
                let _ = state
                    .repo
                    .discard_server_artifact_reservation(object_key)
                    .await;
            }
            Err(delete_error) => {
                let _ = state
                    .repo
                    .mark_server_artifact_delete_failed(
                        object_key,
                        &format!("{error}; cleanup_delete_failed: {delete_error}"),
                    )
                    .await;
            }
        }
    } else {
        let _ = state
            .repo
            .discard_server_artifact_reservation(object_key)
            .await;
    }
}

fn collect_backup_stdout(outputs: &[CommandOutput]) -> Result<Vec<u8>> {
    let mut artifact = Vec::new();
    for output in outputs {
        if output.stream != OutputStream::Stdout {
            continue;
        }
        artifact.extend_from_slice(&output.data);
        anyhow::ensure!(
            artifact.len() <= backup_artifact_streaming_max_bytes(),
            "backup artifact exceeds auto-record limit"
        );
    }
    anyhow::ensure!(!artifact.is_empty(), "backup artifact stdout is empty");
    Ok(artifact)
}

async fn cleanup_staged_backup_artifact(staged: Option<StagedRetainedBackupArtifact>) {
    if let Some(staged) = staged {
        let _ = tokio::fs::remove_file(staged.staging_path).await;
    }
}

pub(crate) fn backup_artifact_object_key(client_id: &str, backup_request_id: uuid::Uuid) -> String {
    let client_key = hex::encode(client_id.as_bytes());
    format!("backups/{client_key}/{backup_request_id}.tar")
}

pub(crate) async fn put_backup_artifact_object(
    store: &crate::object_store::BackupObjectStore,
    object_key: &str,
    data: &[u8],
) -> Result<bool> {
    match store.put_new(object_key, data).await {
        Ok(()) => Ok(true),
        Err(error) => match store
            .get_with_limit(object_key, backup_artifact_streaming_max_bytes())
            .await
        {
            Ok(existing) if existing == data => Ok(false),
            Ok(_) => anyhow::bail!("backup_artifact_object_key_conflict"),
            _ => Err(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_artifact_identity_is_stable_per_backup_request() {
        let request_id = uuid::Uuid::new_v4();
        assert_eq!(
            backup_auto_artifact_id(request_id),
            backup_auto_artifact_id(request_id)
        );
        assert_ne!(
            backup_auto_artifact_id(request_id),
            backup_auto_artifact_id(uuid::Uuid::new_v4())
        );
    }

    #[test]
    fn auto_artifact_retry_classification_keeps_transient_failures_queued() {
        assert!(backup_auto_artifact_error_is_permanent(&anyhow::anyhow!(
            "backup_artifact_tar_invalid"
        )));
        assert!(backup_auto_artifact_error_is_permanent(&anyhow::anyhow!(
            "backup artifact stdout is empty"
        )));
        assert!(backup_auto_artifact_error_is_permanent(&anyhow::anyhow!(
            "backup_artifact_handoff_output_hash_mismatch"
        )));
        assert_eq!(
            backup_auto_artifact_request_failure_reason(&anyhow::anyhow!(
                "backup artifact stdout is empty"
            )),
            Some("backup_artifact_stdout_empty")
        );
        assert_eq!(
            backup_auto_artifact_request_failure_reason(&anyhow::anyhow!(
                "backup_artifact_already_recorded"
            )),
            None
        );
        assert!(!backup_auto_artifact_error_is_permanent(&anyhow::anyhow!(
            "database connection reset"
        )));
        assert!(!backup_auto_artifact_error_is_permanent(&anyhow::anyhow!(
            "object store temporarily unavailable"
        )));
    }
}
