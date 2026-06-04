use anyhow::{ensure, Result};
use base64::Engine as _;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;

use crate::{
    model::{AuditLogView, AuthContext},
    model_command_templates::{
        CommandTemplateView, JobOutputComparisonView, UpsertCommandTemplateRequest,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_command_templates(
        &self,
        limit: i64,
        scope_kind: Option<&str>,
        scope_value: Option<&str>,
        command_type: Option<&str>,
    ) -> Result<Vec<CommandTemplateView>> {
        let limit = limit.clamp(1, 200);
        match self {
            Self::Memory(memory) => {
                let mut rows = memory.command_templates.read().await.clone();
                rows.retain(|row| {
                    scope_kind.is_none_or(|value| row.scope_kind == value)
                        && scope_value.is_none_or(|value| row.scope_value.as_deref() == Some(value))
                        && command_type.is_none_or(|value| row.command_type == value)
                });
                rows.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(rows.into_iter().take(limit as usize).collect())
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        command_type,
                        operation,
                        defaults,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM command_templates
                    WHERE ($2::text IS NULL OR scope_kind = $2)
                      AND ($3::text IS NULL OR scope_value = $3)
                      AND ($4::text IS NULL OR command_type = $4)
                    ORDER BY updated_at DESC, name ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .bind(scope_kind)
                .bind(scope_value)
                .bind(command_type)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(command_template_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }

    pub(crate) async fn upsert_command_template(
        &self,
        request: &UpsertCommandTemplateRequest,
        operator: &AuthContext,
    ) -> Result<CommandTemplateView> {
        let now = unix_now().to_string();
        match self {
            Self::Memory(memory) => {
                let mut rows = memory.command_templates.write().await;
                if let Some(existing) = rows.iter_mut().find(|row| {
                    row.name == request.name
                        && row.scope_kind == request.scope_kind
                        && row.scope_value == request.scope_value
                }) {
                    existing.command_type = request.command_type.clone();
                    existing.operation = request.operation.clone();
                    existing.defaults = normalized_defaults(request);
                    existing.actor_id = Some(operator.operator.id);
                    existing.updated_at = now.clone();
                    let view = existing.clone();
                    drop(rows);
                    record_command_template_audit(self, &view, operator).await?;
                    return Ok(view);
                }
                let view = CommandTemplateView {
                    id: Uuid::new_v4(),
                    name: request.name.clone(),
                    scope_kind: request.scope_kind.clone(),
                    scope_value: request.scope_value.clone(),
                    command_type: request.command_type.clone(),
                    operation: request.operation.clone(),
                    defaults: normalized_defaults(request),
                    actor_id: Some(operator.operator.id),
                    created_at: now.clone(),
                    updated_at: now,
                };
                rows.push(view.clone());
                drop(rows);
                record_command_template_audit(self, &view, operator).await?;
                Ok(view)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let existing_id = sqlx::query_scalar::<_, Uuid>(
                    r#"
                    SELECT id
                    FROM command_templates
                    WHERE name = $1
                      AND scope_kind = $2
                      AND (
                        ($2 = 'global' AND scope_value IS NULL)
                        OR ($2 <> 'global' AND scope_value = $3)
                      )
                    "#,
                )
                .bind(&request.name)
                .bind(&request.scope_kind)
                .bind(&request.scope_value)
                .fetch_optional(&mut *tx)
                .await?;
                let row = if let Some(existing_id) = existing_id {
                    sqlx::query(
                        r#"
                        UPDATE command_templates
                        SET command_type = $2,
                            operation = $3,
                            defaults = $4,
                            actor_id = $5,
                            updated_at = now()
                        WHERE id = $1
                        RETURNING
                            id,
                            name,
                            scope_kind,
                            scope_value,
                            command_type,
                            operation,
                            defaults,
                            actor_id,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        "#,
                    )
                    .bind(existing_id)
                    .bind(&request.command_type)
                    .bind(SqlJson(&request.operation))
                    .bind(SqlJson(normalized_defaults(request)))
                    .bind(operator.operator.id)
                    .fetch_one(&mut *tx)
                    .await?
                } else {
                    sqlx::query(
                        r#"
                        INSERT INTO command_templates (
                            id,
                            name,
                            scope_kind,
                            scope_value,
                            command_type,
                            operation,
                            defaults,
                            actor_id
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                        RETURNING
                            id,
                            name,
                            scope_kind,
                            scope_value,
                            command_type,
                            operation,
                            defaults,
                            actor_id,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(&request.name)
                    .bind(&request.scope_kind)
                    .bind(&request.scope_value)
                    .bind(&request.command_type)
                    .bind(SqlJson(&request.operation))
                    .bind(SqlJson(normalized_defaults(request)))
                    .bind(operator.operator.id)
                    .fetch_one(&mut *tx)
                    .await?
                };
                tx.commit().await?;
                let view = command_template_from_row(row)?;
                record_command_template_audit(self, &view, operator).await?;
                Ok(view)
            }
        }
    }

    pub(crate) async fn compare_job_outputs(
        &self,
        job_id: Uuid,
    ) -> Result<Vec<JobOutputComparisonView>> {
        let outputs = self.list_job_outputs(job_id).await?;
        let compared_at = unix_now().to_string();
        let mut grouped = std::collections::BTreeMap::<String, Vec<_>>::new();
        for output in outputs {
            if output.stream == "status" {
                continue;
            }
            grouped
                .entry(output.client_id.clone())
                .or_default()
                .push(output);
        }
        let mut rows = Vec::new();
        let mut hash_counts = std::collections::BTreeMap::<String, usize>::new();
        for (client_id, mut client_outputs) in grouped {
            client_outputs.sort_by_key(|output| output.seq);
            let mut bytes = Vec::new();
            let mut exit_code = None;
            for output in &client_outputs {
                if let Ok(decoded) =
                    base64::engine::general_purpose::STANDARD.decode(&output.data_base64)
                {
                    bytes.extend(decoded);
                }
                exit_code = output.exit_code.or(exit_code);
            }
            let output_sha256_hex = vpsman_common::payload_hash(&bytes);
            *hash_counts.entry(output_sha256_hex.clone()).or_default() += 1;
            rows.push(JobOutputComparisonView {
                job_id,
                client_id,
                output_sha256_hex,
                stream_count: client_outputs.len() as i32,
                byte_count: bytes.len() as i64,
                exit_code,
                matches_majority: false,
                preview: sanitized_preview(&bytes),
                compared_at: compared_at.clone(),
            });
        }
        let majority_hash = hash_counts
            .into_iter()
            .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
            .map(|(hash, _)| hash);
        if let Some(majority_hash) = majority_hash {
            for row in &mut rows {
                row.matches_majority = row.output_sha256_hex == majority_hash;
            }
        }
        rows.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        Ok(rows)
    }
}

fn command_template_from_row(
    row: sqlx::postgres::PgRow,
) -> std::result::Result<CommandTemplateView, sqlx::Error> {
    let operation: SqlJson<serde_json::Value> = row.try_get("operation")?;
    let defaults: SqlJson<serde_json::Value> = row.try_get("defaults")?;
    Ok(CommandTemplateView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        scope_kind: row.try_get("scope_kind")?,
        scope_value: row.try_get("scope_value")?,
        command_type: row.try_get("command_type")?,
        operation: operation.0,
        defaults: defaults.0,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn normalized_defaults(request: &UpsertCommandTemplateRequest) -> serde_json::Value {
    if request.defaults.is_object() {
        request.defaults.clone()
    } else {
        serde_json::json!({})
    }
}

async fn record_command_template_audit(
    repo: &Repository,
    template: &CommandTemplateView,
    operator: &AuthContext,
) -> Result<()> {
    let metadata = serde_json::json!({
        "template_id": template.id,
        "name": template.name,
        "scope_kind": template.scope_kind,
        "scope_value": template.scope_value,
        "command_type": template.command_type,
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
    });
    match repo {
        Repository::Memory(memory) => {
            memory.audits.write().await.push(AuditLogView {
                id: Uuid::new_v4(),
                actor_id: Some(operator.operator.id),
                action: "command_template.upserted".to_string(),
                target: format!("command_template:{}", template.id),
                command_hash: None,
                metadata,
                created_at: unix_now().to_string(),
            });
        }
        Repository::Postgres(pool) => {
            sqlx::query(
                r#"
                INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                VALUES ($1, $2, $3, $4, NULL, $5)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(operator.operator.id)
            .bind("command_template.upserted")
            .bind(format!("command_template:{}", template.id))
            .bind(metadata)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub(crate) fn validate_command_template_request(
    request: &UpsertCommandTemplateRequest,
) -> Result<()> {
    validate_template_token(&request.name, 96, "name")?;
    ensure!(
        matches!(
            request.scope_kind.as_str(),
            "global" | "provider" | "pool" | "tag" | "client"
        ),
        "command template scope kind is invalid"
    );
    match request.scope_kind.as_str() {
        "global" => ensure!(
            request.scope_value.is_none(),
            "global command templates must not set scope_value"
        ),
        _ => {
            let value = request.scope_value.as_deref().unwrap_or_default().trim();
            validate_template_token(value, 128, "scope_value")?;
        }
    }
    validate_template_token(&request.command_type, 64, "command_type")?;
    ensure!(
        request.operation.is_object(),
        "command template operation must be a JSON object"
    );
    ensure!(
        request.defaults.is_null() || request.defaults.is_object(),
        "command template defaults must be a JSON object"
    );
    Ok(())
}

fn validate_template_token(value: &str, max_len: usize, label: &str) -> Result<()> {
    let value = value.trim();
    ensure!(!value.is_empty(), "{label} is required");
    ensure!(value.len() <= max_len, "{label} is too long");
    ensure!(
        !value.chars().any(char::is_control),
        "{label} contains a control character"
    );
    Ok(())
}

fn sanitized_preview(bytes: &[u8]) -> String {
    let limit = bytes.len().min(2048);
    String::from_utf8_lossy(&bytes[..limit])
        .chars()
        .map(|ch| {
            if ch.is_control() && !matches!(ch, '\n' | '\r' | '\t') {
                ' '
            } else {
                ch
            }
        })
        .collect()
}
