use anyhow::{Context, Result};
use vpsman_common::{payload_hash, CommandOutput, OutputStream};

use crate::{
    backup_handoff::{
        backup_artifact_streaming_max_bytes, stage_retained_backup_artifact_stdout,
        StagedRetainedBackupArtifact,
    },
    model::{AuthContext, BackupArtifactView, RecordBackupArtifactMetadataRequest, WsEvent},
    routes_backups::validate_encrypted_backup_artifact_with_limit,
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
        .find_open_backup_request_for_artifact(client_id, command_hash)
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
            stage_retained_backup_artifact_stdout(state, &candidate.outputs)
                .await
                .map_err(|error| anyhow::anyhow!(error.code))?,
        ),
        None => None,
    };
    let artifact_bytes = if let Some(prepared) = staged.as_ref() {
        match tokio::fs::read(&prepared.staging_path).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let path = prepared.staging_path.display().to_string();
                cleanup_staged_backup_artifact(staged.take()).await;
                return Err(error)
                    .with_context(|| format!("failed to read staged backup artifact {path}"));
            }
        }
    } else {
        collect_backup_stdout(outputs)?
    };
    if let Err(error) = validate_encrypted_backup_artifact_with_limit(
        &artifact_bytes,
        client_id,
        backup_artifact_streaming_max_bytes(),
    ) {
        cleanup_staged_backup_artifact(staged.take()).await;
        return Err(anyhow::anyhow!(error.code));
    }
    let object_key = backup_artifact_object_key(client_id, backup_request.id);
    let (created_object, artifact_sha256_hex, artifact_size_bytes) =
        if let Some(prepared) = staged.as_ref() {
            let artifact_size_bytes = match prepared.size_bytes.try_into() {
                Ok(size) => size,
                Err(error) => {
                    cleanup_staged_backup_artifact(staged.take()).await;
                    return Err(error).context("backup artifact size is invalid");
                }
            };
            let created_object = match store
                .put_file_idempotent(
                    &object_key,
                    &prepared.staging_path,
                    &prepared.sha256_hex,
                    artifact_size_bytes,
                )
                .await
            {
                Ok(created_object) => created_object,
                Err(error) => {
                    cleanup_staged_backup_artifact(staged.take()).await;
                    return Err(error);
                }
            };
            (
                created_object,
                prepared.sha256_hex.clone(),
                prepared.size_bytes,
            )
        } else {
            (
                put_backup_artifact_object(store, &object_key, &artifact_bytes).await?,
                payload_hash(&artifact_bytes),
                i64::try_from(artifact_bytes.len()).context("backup artifact too large")?,
            )
        };
    let metadata = RecordBackupArtifactMetadataRequest {
        object_key: object_key.clone(),
        sha256_hex: artifact_sha256_hex,
        encrypted: true,
        size_bytes: artifact_size_bytes,
        confirmed: true,
    };
    match state
        .repo
        .record_backup_artifact_metadata(&backup_request, &metadata, operator)
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
            if created_object {
                store.delete_best_effort(&object_key).await;
            }
            cleanup_staged_backup_artifact(staged.take()).await;
            Err(error)
        }
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
    format!("backups/{client_key}/{backup_request_id}.json")
}

pub(crate) async fn put_backup_artifact_object(
    store: &crate::object_store::BackupObjectStore,
    object_key: &str,
    data: &[u8],
) -> Result<bool> {
    match store.put_new(object_key, data).await {
        Ok(()) => Ok(true),
        Err(error) => match store.get(object_key).await {
            Ok(existing) if existing == data => Ok(false),
            _ => Err(error),
        },
    }
}
