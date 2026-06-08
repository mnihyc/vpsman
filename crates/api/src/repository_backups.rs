use std::cmp::Ordering;

use anyhow::Result;
use base64::Engine as _;
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::CommandEnvelope;

use crate::{
    model::{
        AuditLogView, AuthContext, BackupRequestStatus, BackupRequestView, CreateBackupRequest,
        JobOutputView, ListQuery,
    },
    repository::Repository,
    unix_now,
    util::{limit_or_default, offset_or_default, search_pattern, sort_descending},
};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BackupRequestSourceLink {
    pub(crate) job_id: Option<Uuid>,
    pub(crate) schedule_id: Option<Uuid>,
}

fn compare_text_or_number(left: &str, right: &str) -> Ordering {
    match (left.parse::<i128>(), right.parse::<i128>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn compare_backup_request(
    left: &BackupRequestView,
    right: &BackupRequestView,
    sort: Option<&str>,
) -> Ordering {
    match sort.unwrap_or("created_at") {
        "artifact_id" | "artifact" => left.artifact_id.cmp(&right.artifact_id),
        "client_id" | "client" => left.client_id.cmp(&right.client_id),
        "include_config" | "scope" => left.include_config.cmp(&right.include_config),
        "paths" => left.paths.len().cmp(&right.paths.len()),
        "payload_hash" | "hash" => left.payload_hash.cmp(&right.payload_hash),
        "signed_command_scope" => left.signed_command_scope.cmp(&right.signed_command_scope),
        "status" => left.status.cmp(&right.status),
        _ => compare_text_or_number(&left.created_at, &right.created_at),
    }
}

fn backup_request_matches_search(request: &BackupRequestView, needle: &str) -> bool {
    request.id.to_string().to_ascii_lowercase().contains(needle)
        || request
            .actor_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || request.client_id.to_ascii_lowercase().contains(needle)
        || request.status.to_ascii_lowercase().contains(needle)
        || request.payload_hash.to_ascii_lowercase().contains(needle)
        || request
            .signed_command_scope
            .to_ascii_lowercase()
            .contains(needle)
        || request
            .artifact_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || request
            .source_job_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || request
            .source_schedule_id
            .map(|id| id.to_string().to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || request
            .note
            .as_deref()
            .map(|value| value.to_ascii_lowercase().contains(needle))
            .unwrap_or(false)
        || request
            .paths
            .iter()
            .any(|path| path.to_ascii_lowercase().contains(needle))
}

fn backup_request_order_by(sort: Option<&str>, descending: bool) -> &'static str {
    match (sort.unwrap_or("created_at"), descending) {
        ("artifact_id" | "artifact", true) => "artifact_id DESC NULLS LAST, id DESC",
        ("artifact_id" | "artifact", false) => "artifact_id ASC NULLS LAST, id ASC",
        ("client_id" | "client", true) => "client_id DESC, id DESC",
        ("client_id" | "client", false) => "client_id ASC, id ASC",
        ("include_config" | "scope", true) => "include_config DESC, id DESC",
        ("include_config" | "scope", false) => "include_config ASC, id ASC",
        ("paths", true) => "cardinality(paths) DESC, id DESC",
        ("paths", false) => "cardinality(paths) ASC, id ASC",
        ("payload_hash" | "hash", true) => "payload_hash DESC, id DESC",
        ("payload_hash" | "hash", false) => "payload_hash ASC, id ASC",
        ("signed_command_scope", true) => "signed_command_scope DESC, id DESC",
        ("signed_command_scope", false) => "signed_command_scope ASC, id ASC",
        ("status", true) => "status DESC, id DESC",
        ("status", false) => "status ASC, id ASC",
        (_, true) => "created_at DESC, id DESC",
        (_, false) => "created_at ASC, id ASC",
    }
}

impl Repository {
    pub(crate) async fn list_backup_requests(&self, limit: i64) -> Result<Vec<BackupRequestView>> {
        match self {
            Self::Memory(memory) => {
                let requests = memory.backup_requests.read().await;
                Ok(requests
                    .iter()
                    .rev()
                    .take(limit as usize)
                    .cloned()
                    .collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        status,
                        payload_hash,
                        signed_command_scope,
                        signed_command_id,
                        signed_command_expires_unix,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    ORDER BY created_at DESC, id DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(backup_request_from_row).collect()
            }
        }
    }

    pub(crate) async fn query_backup_requests(
        &self,
        query: &ListQuery,
    ) -> Result<Vec<BackupRequestView>> {
        let limit = limit_or_default(query.limit);
        let offset = offset_or_default(query.offset);
        let descending = sort_descending(query.dir.as_deref(), true);
        let q = query
            .q
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match self {
            Self::Memory(memory) => {
                let q = q.map(|value| value.to_ascii_lowercase());
                let mut requests = memory
                    .backup_requests
                    .read()
                    .await
                    .iter()
                    .filter(|request| {
                        q.as_deref()
                            .map(|needle| backup_request_matches_search(request, needle))
                            .unwrap_or(true)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                requests.sort_by(|left, right| {
                    compare_backup_request(left, right, query.sort.as_deref())
                        .then_with(|| left.id.cmp(&right.id))
                });
                if descending {
                    requests.reverse();
                }
                Ok(requests
                    .into_iter()
                    .skip(offset as usize)
                    .take(limit as usize)
                    .collect())
            }
            Self::Postgres(pool) => {
                let order_by = backup_request_order_by(query.sort.as_deref(), descending);
                let rows = sqlx::query(&format!(
                    r#"
                    SELECT
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        status,
                        payload_hash,
                        signed_command_scope,
                        signed_command_id,
                        signed_command_expires_unix,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE (
                        $3::text IS NULL
                        OR id::text ILIKE $3 ESCAPE '\'
                        OR actor_id::text ILIKE $3 ESCAPE '\'
                        OR client_id ILIKE $3 ESCAPE '\'
                        OR array_to_string(paths, ' ') ILIKE $3 ESCAPE '\'
                        OR status ILIKE $3 ESCAPE '\'
                        OR payload_hash ILIKE $3 ESCAPE '\'
                        OR signed_command_scope ILIKE $3 ESCAPE '\'
                        OR artifact_id::text ILIKE $3 ESCAPE '\'
                        OR source_job_id::text ILIKE $3 ESCAPE '\'
                        OR source_schedule_id::text ILIKE $3 ESCAPE '\'
                        OR note ILIKE $3 ESCAPE '\'
                    )
                    ORDER BY {order_by}
                    LIMIT $1
                    OFFSET $2
                    "#,
                ))
                .bind(limit)
                .bind(offset)
                .bind(search_pattern(&query.q))
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(backup_request_from_row).collect()
            }
        }
    }

    pub(crate) async fn record_backup_request(
        &self,
        request: &CreateBackupRequest,
        payload_hash: &str,
        envelope: &CommandEnvelope,
        operator: &AuthContext,
        status: BackupRequestStatus,
    ) -> Result<BackupRequestView> {
        self.record_backup_request_with_source(
            request,
            payload_hash,
            envelope,
            operator,
            status,
            BackupRequestSourceLink::default(),
        )
        .await
    }

    pub(crate) async fn record_backup_request_with_source(
        &self,
        request: &CreateBackupRequest,
        payload_hash: &str,
        envelope: &CommandEnvelope,
        operator: &AuthContext,
        status: BackupRequestStatus,
        source: BackupRequestSourceLink,
    ) -> Result<BackupRequestView> {
        let signed_command_expires_unix = None;
        let view = BackupRequestView {
            id: Uuid::new_v4(),
            actor_id: Some(operator.operator.id),
            client_id: request.client_id.clone(),
            paths: request.paths.clone(),
            include_config: request.include_config,
            status: status.as_str().to_string(),
            payload_hash: payload_hash.to_string(),
            signed_command_scope: envelope.scope.clone(),
            signed_command_id: Some(envelope.command_id),
            signed_command_expires_unix,
            artifact_id: None,
            source_job_id: source.job_id,
            source_schedule_id: source.schedule_id,
            note: request.note.clone(),
            created_at: unix_now().to_string(),
        };
        match self {
            Self::Memory(memory) => {
                memory.backup_requests.write().await.push(view.clone());
                memory.audits.write().await.push(backup_request_audit(
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
                    INSERT INTO backup_requests (
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        status,
                        payload_hash,
                        signed_command_scope,
                        signed_command_id,
                        signed_command_expires_unix,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL, $11, $12, $13)
                    RETURNING created_at::text AS created_at
                    "#,
                )
                .bind(view.id)
                .bind(operator.operator.id)
                .bind(&view.client_id)
                .bind(&view.paths)
                .bind(view.include_config)
                .bind(&view.status)
                .bind(&view.payload_hash)
                .bind(&view.signed_command_scope)
                .bind(view.signed_command_id)
                .bind(signed_command_expires_unix.map(|value| value as i64))
                .bind(source.job_id)
                .bind(source.schedule_id)
                .bind(&view.note)
                .fetch_one(&mut *tx)
                .await?;
                let persisted = BackupRequestView {
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
                .bind("backup.requested_metadata_only")
                .bind(format!("backup_request:{}", persisted.id))
                .bind(&persisted.payload_hash)
                .bind(backup_request_metadata(
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

    pub(crate) async fn attach_backup_request_source(
        &self,
        backup_request_id: Uuid,
        source_job_id: Option<Uuid>,
        source_schedule_id: Option<Uuid>,
        operator: &AuthContext,
    ) -> Result<Option<BackupRequestView>> {
        match self {
            Self::Memory(memory) => {
                let mut requests = memory.backup_requests.write().await;
                let Some(request) = requests
                    .iter_mut()
                    .find(|request| request.id == backup_request_id)
                else {
                    return Ok(None);
                };
                let changed = request.source_job_id != source_job_id
                    || request.source_schedule_id != source_schedule_id;
                if changed {
                    request.source_job_id = source_job_id;
                    request.source_schedule_id = source_schedule_id;
                    memory
                        .audits
                        .write()
                        .await
                        .push(backup_request_source_audit(
                            request,
                            operator,
                            unix_now().to_string(),
                        ));
                }
                Ok(Some(request.clone()))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let Some(row) = sqlx::query(
                    r#"
                    UPDATE backup_requests
                    SET
                        source_job_id = COALESCE(source_job_id, $2),
                        source_schedule_id = COALESCE(source_schedule_id, $3)
                    WHERE id = $1
                    RETURNING
                        id,
                        actor_id,
                        client_id,
                        paths,
                        include_config,
                        status,
                        payload_hash,
                        signed_command_scope,
                        signed_command_id,
                        signed_command_expires_unix,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note,
                        created_at::text AS created_at
                    "#,
                )
                .bind(backup_request_id)
                .bind(source_job_id)
                .bind(source_schedule_id)
                .fetch_optional(&mut *tx)
                .await?
                else {
                    tx.commit().await?;
                    return Ok(None);
                };
                let request = backup_request_from_row(row)?;
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
                .bind("backup.request_source_linked")
                .bind(format!("backup_request:{}", request.id))
                .bind(&request.payload_hash)
                .bind(backup_request_source_metadata(&request, operator))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(Some(request))
            }
        }
    }

    pub(crate) async fn record_rejected_backup_request(
        &self,
        request: &CreateBackupRequest,
        payload_hash: &str,
        operator: &AuthContext,
        reason: &'static str,
    ) -> Result<()> {
        let metadata = backup_rejection_metadata(request, payload_hash, operator, reason);
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "backup.rejected_authorization_required".to_string(),
                    target: format!("client:{}", request.client_id),
                    command_hash: Some(payload_hash.to_string()),
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
                .bind("backup.rejected_authorization_required")
                .bind(format!("client:{}", request.client_id))
                .bind(payload_hash)
                .bind(metadata)
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn find_open_backup_request_for_artifact(
        &self,
        client_id: &str,
        payload_hash: &str,
    ) -> Result<Option<BackupRequestView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .backup_requests
                .read()
                .await
                .iter()
                .rev()
                .find(|request| {
                    request.client_id == client_id
                        && request.payload_hash == payload_hash
                        && request.artifact_id.is_none()
                })
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
                        signed_command_scope,
                        signed_command_id,
                        signed_command_expires_unix,
                        artifact_id,
                        source_job_id,
                        source_schedule_id,
                        note,
                        created_at::text AS created_at
                    FROM backup_requests
                    WHERE client_id = $1
                      AND payload_hash = $2
                      AND artifact_id IS NULL
                    ORDER BY created_at DESC, id DESC
                    LIMIT 1
                    "#,
                )
                .bind(client_id)
                .bind(payload_hash)
                .fetch_optional(pool)
                .await?;
                row.map(backup_request_from_row).transpose()
            }
        }
    }

    pub(crate) async fn find_backup_artifact_output_candidate(
        &self,
        backup_request: &BackupRequestView,
        selected_job_id: Option<Uuid>,
    ) -> Result<Option<BackupArtifactOutputCandidate>> {
        match self {
            Self::Memory(memory) => {
                let jobs = memory.jobs.read().await;
                let targets = memory.job_targets.read().await;
                let outputs = memory.job_outputs.read().await;
                let mut candidates = jobs
                    .iter()
                    .filter(|job| {
                        job.command_type == "backup"
                            && job.payload_hash == backup_request.payload_hash
                            && selected_job_id.is_none_or(|job_id| job.id == job_id)
                            && targets.iter().any(|target| {
                                target.job_id == job.id
                                    && target.client_id == backup_request.client_id
                                    && target.status == "completed"
                            })
                    })
                    .map(|job| {
                        let mut stdout = outputs
                            .iter()
                            .filter(|output| {
                                output.job_id == job.id
                                    && output.client_id == backup_request.client_id
                                    && output.stream == "stdout"
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        stdout.sort_by_key(|output| output.seq);
                        (job.id, job.created_at.clone(), stdout)
                    })
                    .filter(|(_, _, outputs)| !outputs.is_empty())
                    .collect::<Vec<_>>();
                candidates
                    .sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
                Ok(candidates
                    .into_iter()
                    .next()
                    .map(
                        |(job_id, _created_at, outputs)| BackupArtifactOutputCandidate {
                            job_id,
                            outputs,
                        },
                    ))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        job.id
                    FROM jobs job
                    JOIN job_targets target
                      ON target.job_id = job.id
                     AND target.client_id = $2
                     AND target.status = 'completed'
                    WHERE job.command_type = 'backup'
                      AND job.payload_hash = $1
                      AND ($3::uuid IS NULL OR job.id = $3)
                      AND EXISTS (
                        SELECT 1
                        FROM job_outputs output
                        WHERE output.job_id = job.id
                          AND output.client_id = $2
                          AND output.stream = 'stdout'
                      )
                    ORDER BY job.created_at DESC, job.id DESC
                    LIMIT 1
                    "#,
                )
                .bind(&backup_request.payload_hash)
                .bind(&backup_request.client_id)
                .bind(selected_job_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let job_id: Uuid = row.try_get("id")?;
                let output_rows = sqlx::query(
                    r#"
                    SELECT
                        job_id,
                        client_id,
                        seq,
                        stream,
                        data,
                        storage,
                        object_key,
                        data_sha256_hex,
                        data_size_bytes,
                        exit_code,
                        done,
                        created_at::text AS created_at
                    FROM job_outputs
                    WHERE job_id = $1
                      AND client_id = $2
                      AND stream = 'stdout'
                    ORDER BY seq
                    "#,
                )
                .bind(job_id)
                .bind(&backup_request.client_id)
                .fetch_all(pool)
                .await?;
                let outputs = output_rows
                    .into_iter()
                    .map(|row| {
                        let data: Vec<u8> = row.try_get("data")?;
                        Ok(JobOutputView {
                            job_id: row.try_get("job_id")?,
                            client_id: row.try_get("client_id")?,
                            seq: row.try_get("seq")?,
                            stream: row.try_get("stream")?,
                            data_base64: base64::engine::general_purpose::STANDARD.encode(data),
                            storage: row.try_get("storage")?,
                            artifact_object_key: row.try_get("object_key")?,
                            artifact_sha256_hex: row.try_get("data_sha256_hex")?,
                            artifact_size_bytes: row.try_get("data_size_bytes")?,
                            exit_code: row.try_get("exit_code")?,
                            done: row.try_get("done")?,
                            created_at: row.try_get("created_at")?,
                        })
                    })
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()?;
                Ok(Some(BackupArtifactOutputCandidate { job_id, outputs }))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct BackupArtifactOutputCandidate {
    pub(crate) job_id: Uuid,
    pub(crate) outputs: Vec<JobOutputView>,
}

pub(crate) fn backup_request_from_row(row: sqlx::postgres::PgRow) -> Result<BackupRequestView> {
    let signed_command_expires_unix = row
        .try_get::<Option<i64>, _>("signed_command_expires_unix")?
        .map(|value| value.max(0) as u64);
    let status: String = row.try_get("status")?;
    Ok(BackupRequestView {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        client_id: row.try_get("client_id")?,
        paths: row.try_get("paths")?,
        include_config: row.try_get("include_config")?,
        status: BackupRequestStatus::from_storage(&status)
            .map(|status| status.as_str().to_string())
            .unwrap_or(status),
        payload_hash: row.try_get("payload_hash")?,
        signed_command_scope: row.try_get("signed_command_scope")?,
        signed_command_id: row.try_get("signed_command_id")?,
        signed_command_expires_unix,
        artifact_id: row.try_get("artifact_id")?,
        source_job_id: row.try_get("source_job_id")?,
        source_schedule_id: row.try_get("source_schedule_id")?,
        note: row.try_get("note")?,
        created_at: row.try_get("created_at")?,
    })
}

fn backup_request_audit(
    view: &BackupRequestView,
    confirmed: bool,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "backup.requested_metadata_only".to_string(),
        target: format!("backup_request:{}", view.id),
        command_hash: Some(view.payload_hash.clone()),
        metadata: backup_request_metadata(view, confirmed, operator),
        created_at,
    }
}

fn backup_request_metadata(
    view: &BackupRequestView,
    confirmed: bool,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "client_id": &view.client_id,
        "paths": &view.paths,
        "include_config": view.include_config,
        "status": &view.status,
        "payload_hash": &view.payload_hash,
        "signed_command_scope": &view.signed_command_scope,
        "signed_command_id": view.signed_command_id,
        "signed_command_expires_unix": view.signed_command_expires_unix,
        "artifact_id": view.artifact_id,
        "source_job_id": view.source_job_id,
        "source_schedule_id": view.source_schedule_id,
        "confirmed": confirmed,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}

fn backup_request_source_audit(
    view: &BackupRequestView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: "backup.request_source_linked".to_string(),
        target: format!("backup_request:{}", view.id),
        command_hash: Some(view.payload_hash.clone()),
        metadata: backup_request_source_metadata(view, operator),
        created_at,
    }
}

fn backup_request_source_metadata(
    view: &BackupRequestView,
    operator: &AuthContext,
) -> serde_json::Value {
    json!({
        "client_id": &view.client_id,
        "payload_hash": &view.payload_hash,
        "source_job_id": view.source_job_id,
        "source_schedule_id": view.source_schedule_id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}

fn backup_rejection_metadata(
    request: &CreateBackupRequest,
    payload_hash: &str,
    operator: &AuthContext,
    reason: &'static str,
) -> serde_json::Value {
    json!({
        "client_id": &request.client_id,
        "paths": &request.paths,
        "include_config": request.include_config,
        "confirmed": request.confirmed,
        "payload_hash": payload_hash,
        "reason": reason,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
        "metadata_only": true,
    })
}
