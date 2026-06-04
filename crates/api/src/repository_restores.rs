use anyhow::Result;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::CommandEnvelope;

use crate::{
    model::{
        AuditLogView, AuthContext, BackupRequestView, CreateRestorePlanRequest, RestorePlanStatus,
        RestorePlanView,
    },
    repository::Repository,
    repository_backups::backup_request_from_row,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_restore_plans(&self, limit: i64) -> Result<Vec<RestorePlanView>> {
        match self {
            Self::Memory(memory) => {
                let plans = memory.restore_plans.read().await;
                Ok(plans.iter().rev().take(limit as usize).cloned().collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        payload_hash,
                        proof_scope,
                        proof_command_id,
                        proof_expires_unix,
                        note,
                        created_at::text AS created_at
                    FROM restore_plans
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(restore_plan_from_row).collect()
            }
        }
    }

    pub(crate) async fn find_backup_request(&self, id: Uuid) -> Result<Option<BackupRequestView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .backup_requests
                .read()
                .await
                .iter()
                .find(|request| request.id == id)
                .cloned()),
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        status,
                        payload_hash,
                        proof_scope,
                        proof_command_id,
                        proof_expires_unix,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE id = $1
                    "#,
                )
                .bind(id)
                .fetch_optional(pool)
                .await?;
                row.map(backup_request_from_row).transpose()
            }
        }
    }

    pub(crate) async fn record_restore_plan(
        &self,
        request: &CreateRestorePlanRequest,
        source_backup: &BackupRequestView,
        payload_hash: &str,
        envelope: &CommandEnvelope,
        operator: &AuthContext,
        status: RestorePlanStatus,
    ) -> Result<RestorePlanView> {
        let proof_expires_unix = envelope.proof.as_ref().map(|proof| proof.expires_unix);
        let view = RestorePlanView {
            id: Uuid::new_v4(),
            actor_id: Some(operator.operator.id),
            source_backup_request_id: request.source_backup_request_id,
            source_client_id: source_backup.client_id.clone(),
            target_client_id: request.target_client_id.clone(),
            paths: request.paths.clone(),
            include_config: request.include_config,
            destination_root: request.destination_root.clone(),
            status: status.as_str().to_string(),
            payload_hash: payload_hash.to_string(),
            proof_scope: envelope.scope.clone(),
            proof_command_id: Some(envelope.command_id),
            proof_expires_unix,
            note: request.note.clone(),
            created_at: unix_now().to_string(),
        };
        match self {
            Self::Memory(memory) => {
                memory.restore_plans.write().await.push(view.clone());
                memory.audits.write().await.push(restore_plan_audit(
                    &view,
                    request.confirmed,
                    operator,
                    unix_now().to_string(),
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    INSERT INTO restore_plans (
                        id,
                        actor_id,
                        source_backup_request_id,
                        source_client_id,
                        target_client_id,
                        paths,
                        include_config,
                        destination_root,
                        status,
                        payload_hash,
                        proof_scope,
                        proof_command_id,
                        proof_expires_unix,
                        note
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(view.id)
                .bind(operator.operator.id)
                .bind(view.source_backup_request_id)
                .bind(&view.source_client_id)
                .bind(&view.target_client_id)
                .bind(&view.paths)
                .bind(view.include_config)
                .bind(&view.destination_root)
                .bind(&view.status)
                .bind(&view.payload_hash)
                .bind(&view.proof_scope)
                .bind(view.proof_command_id)
                .bind(proof_expires_unix.map(|value| value as i64))
                .bind(&view.note)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = RestorePlanView {
                    created_at: row.try_get("created_at")?,
                    ..view
                };
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("restore.planned_metadata_only")
                .bind(format!("restore_plan:{}", persisted.id))
                .bind(&persisted.payload_hash)
                .bind(restore_plan_metadata(
                    &persisted,
                    request.confirmed,
                    operator,
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                return Ok(persisted);
            }
        }
        Ok(view)
    }

    pub(crate) async fn record_rejected_restore_plan(
        &self,
        request: &CreateRestorePlanRequest,
        payload_hash: Option<&str>,
        operator: &AuthContext,
        reason: &'static str,
    ) -> Result<()> {
        let metadata = restore_rejection_metadata(request, payload_hash, operator, reason);
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "restore.rejected_authorization_required".to_string(),
                    target: format!("client:{}", request.target_client_id),
                    command_hash: payload_hash.map(ToOwned::to_owned),
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("restore.rejected_authorization_required")
                .bind(format!("client:{}", request.target_client_id))
                .bind(payload_hash)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

fn restore_plan_from_row(row: sqlx::postgres::PgRow) -> Result<RestorePlanView> {
    let proof_expires_unix = row
        .try_get::<Option<i64>, _>("proof_expires_unix")?
        .map(|value| value.max(0) as u64);
    let status: String = row.try_get("status")?;
    Ok(RestorePlanView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        source_backup_request_id: row.try_get("source_backup_request_id")?,
        source_client_id: row.try_get("source_client_id")?,
        target_client_id: row.try_get("target_client_id")?,
        paths: row.try_get("paths")?,
        include_config: row.try_get("include_config")?,
        destination_root: row.try_get("destination_root")?,
        status: RestorePlanStatus::from_storage(&status)
            .map(|status| status.as_str().to_string())
            .unwrap_or(status),
        payload_hash: row.try_get("payload_hash")?,
        proof_scope: row.try_get("proof_scope")?,
        proof_command_id: row.try_get("proof_command_id")?,
        proof_expires_unix,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

fn restore_plan_audit(
    view: &RestorePlanView,
    confirmed: bool,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "restore.planned_metadata_only".to_string(),
        target: format!("restore_plan:{}", view.id),
        command_hash: Some(view.payload_hash.clone()),
        metadata: restore_plan_metadata(view, confirmed, operator),
        created_at,
    }
}

fn restore_plan_metadata(
    view: &RestorePlanView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "source_backup_request_id": view.source_backup_request_id,
        "source_client_id": &view.source_client_id,
        "target_client_id": &view.target_client_id,
        "paths": &view.paths,
        "include_config": view.include_config,
        "destination_root": &view.destination_root,
        "status": &view.status,
        "payload_hash": &view.payload_hash,
        "proof_scope": &view.proof_scope,
        "proof_command_id": view.proof_command_id,
        "proof_expires_unix": view.proof_expires_unix,
        "confirmed": confirmed,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}

fn restore_rejection_metadata(
    request: &CreateRestorePlanRequest,
    payload_hash: Option<&str>,
    operator: &AuthContext,
    reason: &'static str,
) -> serde_json::Value {
    json!({
        "source_backup_request_id": request.source_backup_request_id,
        "target_client_id": &request.target_client_id,
        "paths": &request.paths,
        "include_config": request.include_config,
        "destination_root": &request.destination_root,
        "confirmed": request.confirmed,
        "payload_hash": payload_hash,
        "has_envelope": request.envelope.is_some(),
        "reason": reason,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}
