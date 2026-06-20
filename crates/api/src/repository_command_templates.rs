use anyhow::{ensure, Result};
use base64::Engine as _;
use sqlx::{types::Json as SqlJson, Row};
use std::collections::BTreeMap;
use uuid::Uuid;
use vpsman_common::{
    job_command_display_group, job_command_requires_confirmation, job_command_type_label,
    AgentUpdateConfig, JobCommand, TerminalUserPolicy,
};

use crate::{
    model::{AuditLogView, AuthContext, JobOutputView},
    model_command_templates::{
        CommandTemplateView, JobOutputComparisonGroupView, JobOutputComparisonRowView,
        JobOutputComparisonView, UpsertCommandTemplateRequest,
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
        display_group: Option<&str>,
    ) -> Result<Vec<CommandTemplateView>> {
        let limit = limit.clamp(1, 200);
        let mut builtins = builtin_command_templates()
            .into_iter()
            .filter(|row| {
                command_template_matches_filters(
                    row,
                    scope_kind,
                    scope_value,
                    command_type,
                    display_group,
                )
            })
            .collect::<Vec<_>>();
        let mut user_rows = match self {
            Self::Memory(memory) => {
                let mut rows = memory.command_templates.read().await.clone();
                rows.retain(|row| {
                    command_template_matches_filters(
                        row,
                        scope_kind,
                        scope_value,
                        command_type,
                        display_group,
                    )
                });
                rows.sort_by(|left, right| {
                    right
                        .updated_at
                        .cmp(&left.updated_at)
                        .then_with(|| left.name.cmp(&right.name))
                });
                rows.into_iter().take(limit as usize).collect()
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
                        display_group,
                        operation,
                        defaults,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM command_templates
                    WHERE ($2::text IS NULL OR scope_kind = $2)
                      AND ($3::text IS NULL OR scope_value = $3)
                      AND ($4::text IS NULL OR command_type = $4)
                      AND ($5::text IS NULL OR display_group = $5)
                    ORDER BY updated_at DESC, name ASC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .bind(scope_kind)
                .bind(scope_value)
                .bind(command_type)
                .bind(display_group)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(command_template_from_row)
                    .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
                    .map_err(anyhow::Error::from)?
            }
        };
        builtins.append(&mut user_rows);
        Ok(builtins.into_iter().take(limit as usize).collect())
    }

    pub(crate) async fn upsert_command_template(
        &self,
        request: &UpsertCommandTemplateRequest,
        operator: &AuthContext,
    ) -> Result<CommandTemplateView> {
        let now = unix_now().to_string();
        ensure!(
            !command_template_name_scope_is_builtin(
                &request.name,
                &request.scope_kind,
                request.scope_value.as_deref(),
            ),
            "command_template_builtin_immutable"
        );
        let parts = command_template_parts(request)?;
        match self {
            Self::Memory(memory) => {
                let mut rows = memory.command_templates.write().await;
                if let Some(existing) = rows.iter_mut().find(|row| {
                    row.name == request.name
                        && row.scope_kind == request.scope_kind
                        && row.scope_value == request.scope_value
                }) {
                    existing.command_type = parts.command_type.clone();
                    existing.display_group = parts.display_group.clone();
                    existing.operation = parts.operation.clone();
                    existing.defaults = normalized_defaults(request);
                    existing.actor_id = Some(operator.operator.id);
                    existing.built_in = false;
                    existing.updated_at = now.clone();
                    let view = existing.clone();
                    drop(rows);
                    record_command_template_audit(
                        self,
                        &view,
                        operator,
                        "command_template.upserted",
                    )
                    .await?;
                    return Ok(view);
                }
                let view = CommandTemplateView {
                    id: Uuid::new_v4(),
                    name: request.name.clone(),
                    built_in: false,
                    scope_kind: request.scope_kind.clone(),
                    scope_value: request.scope_value.clone(),
                    command_type: parts.command_type.clone(),
                    display_group: parts.display_group.clone(),
                    operation: parts.operation.clone(),
                    defaults: normalized_defaults(request),
                    actor_id: Some(operator.operator.id),
                    created_at: now.clone(),
                    updated_at: now,
                };
                rows.push(view.clone());
                drop(rows);
                record_command_template_audit(self, &view, operator, "command_template.upserted")
                    .await?;
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
                            display_group = $3,
                            operation = $4,
                            defaults = $5,
                            actor_id = $6,
                            updated_at = now()
                        WHERE id = $1
                        RETURNING
                            id,
                            name,
                            scope_kind,
                            scope_value,
                            command_type,
                            display_group,
                            operation,
                            defaults,
                            actor_id,
                            created_at::text AS created_at,
                            updated_at::text AS updated_at
                        "#,
                    )
                    .bind(existing_id)
                    .bind(&parts.command_type)
                    .bind(&parts.display_group)
                    .bind(SqlJson(&parts.operation))
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
                            display_group,
                            operation,
                            defaults,
                            actor_id
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                        RETURNING
                            id,
                            name,
                            scope_kind,
                            scope_value,
                            command_type,
                            display_group,
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
                    .bind(&parts.command_type)
                    .bind(&parts.display_group)
                    .bind(SqlJson(&parts.operation))
                    .bind(SqlJson(normalized_defaults(request)))
                    .bind(operator.operator.id)
                    .fetch_one(&mut *tx)
                    .await?
                };
                tx.commit().await?;
                let view = command_template_from_row(row)?;
                record_command_template_audit(self, &view, operator, "command_template.upserted")
                    .await?;
                Ok(view)
            }
        }
    }

    pub(crate) async fn delete_command_template(
        &self,
        template_id: Uuid,
        operator: &AuthContext,
    ) -> Result<Option<CommandTemplateView>> {
        ensure!(
            !command_template_id_is_builtin(template_id),
            "command_template_builtin_immutable"
        );
        match self {
            Self::Memory(memory) => {
                let mut rows = memory.command_templates.write().await;
                let Some(index) = rows.iter().position(|row| row.id == template_id) else {
                    return Ok(None);
                };
                let view = rows.remove(index);
                drop(rows);
                record_command_template_audit(self, &view, operator, "command_template.deleted")
                    .await?;
                Ok(Some(view))
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    DELETE FROM command_templates
                    WHERE id = $1
                    RETURNING
                        id,
                        name,
                        scope_kind,
                        scope_value,
                        command_type,
                        display_group,
                        operation,
                        defaults,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(template_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let view = command_template_from_row(row)?;
                record_command_template_audit(self, &view, operator, "command_template.deleted")
                    .await?;
                Ok(Some(view))
            }
        }
    }

    pub(crate) async fn compare_job_outputs(
        &self,
        job_id: Uuid,
        mode: &str,
    ) -> Result<JobOutputComparisonView> {
        let mode = normalize_output_compare_mode(mode);
        let targets = self.list_job_targets(job_id).await?;
        let outputs = self.list_job_outputs(job_id).await?;
        let compared_at = unix_now().to_string();
        let mut outputs_by_client = BTreeMap::<String, Vec<JobOutputView>>::new();
        for output in outputs {
            outputs_by_client
                .entry(output.client_id.clone())
                .or_default()
                .push(output);
        }

        let mut group_rows = BTreeMap::<OutputComparisonKey, Vec<OutputComparisonRowBuild>>::new();
        for target in targets {
            let signature = output_signature(
                outputs_by_client
                    .remove(&target.client_id)
                    .unwrap_or_default(),
                &mode,
            );
            let key = OutputComparisonKey {
                status: target.status.clone(),
                exit_code: target.exit_code,
                output_digest_hex: signature.output_digest_hex.clone(),
                output_compare_basis: signature.output_compare_basis.clone(),
            };
            group_rows
                .entry(key)
                .or_default()
                .push(OutputComparisonRowBuild {
                    client_id: target.client_id,
                    status: target.status,
                    exit_code: target.exit_code,
                    output_digest_hex: signature.output_digest_hex,
                    output_compare_basis: signature.output_compare_basis,
                    stream_count: signature.stream_count,
                    byte_count: signature.byte_count,
                    preview: signature.preview,
                });
        }
        for (client_id, client_outputs) in outputs_by_client {
            let signature = output_signature(client_outputs, &mode);
            let key = OutputComparisonKey {
                status: "unknown".to_string(),
                exit_code: None,
                output_digest_hex: signature.output_digest_hex.clone(),
                output_compare_basis: signature.output_compare_basis.clone(),
            };
            group_rows
                .entry(key)
                .or_default()
                .push(OutputComparisonRowBuild {
                    client_id,
                    status: "unknown".to_string(),
                    exit_code: None,
                    output_digest_hex: signature.output_digest_hex,
                    output_compare_basis: signature.output_compare_basis,
                    stream_count: signature.stream_count,
                    byte_count: signature.byte_count,
                    preview: signature.preview,
                });
        }

        let total_targets = group_rows.values().map(|rows| rows.len()).sum::<usize>() as i32;
        let mut ordered_groups = group_rows.into_iter().collect::<Vec<_>>();
        ordered_groups.sort_by(|(left_key, left_rows), (right_key, right_rows)| {
            right_rows
                .len()
                .cmp(&left_rows.len())
                .then_with(|| left_key.status.cmp(&right_key.status))
                .then_with(|| left_key.exit_code.cmp(&right_key.exit_code))
                .then_with(|| left_key.output_digest_hex.cmp(&right_key.output_digest_hex))
        });
        let largest_group_size = ordered_groups
            .first()
            .map(|(_, rows)| rows.len())
            .unwrap_or_default();
        let mut groups = Vec::with_capacity(ordered_groups.len());
        let mut rows = Vec::with_capacity(total_targets as usize);
        for (index, (key, mut group_members)) in ordered_groups.into_iter().enumerate() {
            group_members.sort_by(|left, right| left.client_id.cmp(&right.client_id));
            let group_id = format!("g{}", index + 1);
            let group_size = group_members.len();
            let representative_client_id = group_members
                .first()
                .map(|row| row.client_id.clone())
                .unwrap_or_default();
            let client_ids = group_members
                .iter()
                .map(|row| row.client_id.clone())
                .collect::<Vec<_>>();
            let stream_count = group_members
                .iter()
                .map(|row| row.stream_count)
                .sum::<i32>();
            let byte_count = group_members.iter().map(|row| row.byte_count).sum::<i64>();
            let preview = group_members
                .first()
                .map(|row| row.preview.clone())
                .unwrap_or_else(|| "No retained output".to_string());
            groups.push(JobOutputComparisonGroupView {
                group_id: group_id.clone(),
                status: key.status,
                exit_code: key.exit_code,
                output_digest_hex: key.output_digest_hex,
                output_compare_basis: key.output_compare_basis,
                target_count: group_size as i32,
                stream_count,
                byte_count,
                representative_client_id,
                client_ids,
                preview,
            });
            for row in group_members {
                rows.push(JobOutputComparisonRowView {
                    job_id,
                    client_id: row.client_id,
                    group_id: group_id.clone(),
                    status: row.status,
                    exit_code: row.exit_code,
                    output_digest_hex: row.output_digest_hex,
                    output_compare_basis: row.output_compare_basis,
                    stream_count: row.stream_count,
                    byte_count: row.byte_count,
                    matches_largest_group: largest_group_size > 0
                        && group_size == largest_group_size,
                    preview: row.preview,
                });
            }
        }
        rows.sort_by(|left, right| left.client_id.cmp(&right.client_id));
        Ok(JobOutputComparisonView {
            job_id,
            mode,
            compared_at,
            total_targets,
            compared_targets: rows.len() as i32,
            group_count: groups.len() as i32,
            groups,
            rows,
        })
    }
}

pub(crate) fn command_template_id_is_builtin(template_id: Uuid) -> bool {
    builtin_command_templates()
        .iter()
        .any(|template| template.id == template_id)
}

pub(crate) fn command_template_name_scope_is_builtin(
    name: &str,
    scope_kind: &str,
    scope_value: Option<&str>,
) -> bool {
    builtin_command_templates().iter().any(|template| {
        template.name == name
            && template.scope_kind == scope_kind
            && template.scope_value.as_deref() == scope_value
    })
}

fn command_template_matches_filters(
    row: &CommandTemplateView,
    scope_kind: Option<&str>,
    scope_value: Option<&str>,
    command_type: Option<&str>,
    display_group: Option<&str>,
) -> bool {
    scope_kind.is_none_or(|value| row.scope_kind == value)
        && scope_value.is_none_or(|value| row.scope_value.as_deref() == Some(value))
        && command_type.is_none_or(|value| row.command_type == value)
        && display_group.is_none_or(|value| row.display_group.as_deref() == Some(value))
}

fn builtin_command_templates() -> Vec<CommandTemplateView> {
    let update = AgentUpdateConfig::default();
    vec![
        builtin_command_template(
            "00000000-0000-4100-8000-000000000001",
            "Default shell command",
            JobCommand::Shell {
                argv: vec!["/usr/bin/uptime".to_string()],
                pty: false,
            },
            30,
        ),
        builtin_command_template(
            "00000000-0000-4100-8000-000000000002",
            "Default terminal shell",
            JobCommand::TerminalOpen {
                session_id: Uuid::nil(),
                argv: vec!["/bin/sh".to_string(), "-l".to_string()],
                cwd: None,
                user: None,
                user_policy: TerminalUserPolicy::Fail,
                cols: 120,
                rows: 40,
                replay_from_seq: None,
                idle_timeout_secs: 1800,
                flow_window_bytes: 64 * 1024,
            },
            30,
        ),
        builtin_command_template(
            "00000000-0000-4100-8000-000000000003",
            "Default backup",
            JobCommand::Backup {
                paths: vec!["/etc/hostname".to_string()],
                include_config: true,
                follow_symlinks: false,
            },
            30,
        ),
        builtin_command_template(
            "00000000-0000-4100-8000-000000000004",
            "Default manual update check",
            JobCommand::AgentUpdateCheck {
                version_url: Some(update.unmanaged_version_url),
                activate: update.unmanaged_activate,
                restart_agent: update.unmanaged_restart_agent,
            },
            300,
        ),
    ]
}

fn builtin_command_template(
    id: &str,
    name: &str,
    command: JobCommand,
    timeout_secs: u64,
) -> CommandTemplateView {
    let command_type = job_command_type_label(&command).to_string();
    let display_group = job_command_display_group(&command_type).map(ToString::to_string);
    let requires_confirmation = job_command_requires_confirmation(&command);
    CommandTemplateView {
        id: Uuid::parse_str(id).expect("builtin command template id must be a UUID"),
        name: name.to_string(),
        built_in: true,
        scope_kind: "global".to_string(),
        scope_value: None,
        command_type,
        display_group,
        operation: serde_json::to_value(command)
            .expect("builtin command template operation must serialize"),
        defaults: serde_json::json!({
            "confirmed": requires_confirmation,
            "destructive": requires_confirmation,
            "force_unprivileged": false,
            "timeout_secs": timeout_secs,
        }),
        actor_id: None,
        created_at: "builtin".to_string(),
        updated_at: "builtin".to_string(),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct OutputComparisonKey {
    status: String,
    exit_code: Option<i32>,
    output_digest_hex: String,
    output_compare_basis: String,
}

struct OutputComparisonRowBuild {
    client_id: String,
    status: String,
    exit_code: Option<i32>,
    output_digest_hex: String,
    output_compare_basis: String,
    stream_count: i32,
    byte_count: i64,
    preview: String,
}

struct OutputSignature {
    output_digest_hex: String,
    output_compare_basis: String,
    stream_count: i32,
    byte_count: i64,
    preview: String,
}

fn normalize_output_compare_mode(mode: &str) -> String {
    match mode.trim() {
        "text" => "text".to_string(),
        _ => "binary".to_string(),
    }
}

fn output_signature(mut outputs: Vec<JobOutputView>, mode: &str) -> OutputSignature {
    outputs.sort_by_key(|output| output.seq);
    if outputs.is_empty() {
        return OutputSignature {
            output_digest_hex: vpsman_common::payload_hash(&[]),
            output_compare_basis: mode.to_string(),
            stream_count: 0,
            byte_count: 0,
            preview: "No retained output".to_string(),
        };
    }
    if mode == "text" {
        if let Some(signature) = text_output_signature(&outputs) {
            return signature;
        }
    }
    binary_output_signature(&outputs)
}

fn text_output_signature(outputs: &[JobOutputView]) -> Option<OutputSignature> {
    let mut total_bytes = 0_i64;
    let mut canonical = String::new();
    let mut preview_text = String::new();
    for output in outputs {
        if output.storage != "inline" {
            return None;
        }
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&output.data_base64)
            .ok()?;
        let byte_len = decoded.len();
        let text = String::from_utf8(decoded).ok()?;
        total_bytes += byte_len as i64;
        let normalized = normalize_text_for_comparison(&text);
        canonical.push_str(&output.stream);
        canonical.push('\0');
        canonical.push_str(&normalized);
        canonical.push('\0');
        if !normalized.is_empty() {
            if !preview_text.is_empty() {
                preview_text.push('\n');
            }
            preview_text.push_str(&normalized);
        }
    }
    Some(OutputSignature {
        output_digest_hex: vpsman_common::payload_hash(canonical.as_bytes()),
        output_compare_basis: "text".to_string(),
        stream_count: outputs.len() as i32,
        byte_count: total_bytes,
        preview: sanitized_preview(preview_text.as_bytes()),
    })
}

fn binary_output_signature(outputs: &[JobOutputView]) -> OutputSignature {
    let mut byte_count = 0_i64;
    let mut canonical = Vec::new();
    let mut preview_bytes = Vec::new();
    let mut has_artifact_backed_output = false;
    for output in outputs {
        canonical.extend_from_slice(output.stream.as_bytes());
        canonical.push(0);
        canonical.extend_from_slice(output.storage.as_bytes());
        canonical.push(0);
        if output.storage == "inline" {
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(&output.data_base64)
                .unwrap_or_default();
            byte_count += decoded.len() as i64;
            canonical.extend_from_slice(decoded.len().to_string().as_bytes());
            canonical.push(0);
            canonical.extend_from_slice(vpsman_common::payload_hash(&decoded).as_bytes());
            preview_bytes.extend_from_slice(&decoded);
        } else {
            has_artifact_backed_output = true;
            let size = output.artifact_size_bytes.unwrap_or_default().max(0);
            byte_count += size;
            canonical.extend_from_slice(size.to_string().as_bytes());
            canonical.push(0);
            canonical.extend_from_slice(
                output
                    .artifact_sha256_hex
                    .as_deref()
                    .unwrap_or("missing-artifact-hash")
                    .as_bytes(),
            );
        }
        canonical.push(0);
    }
    let preview = if has_artifact_backed_output {
        format!(
            "Artifact-backed retained output; {} chunks, {} bytes. Download chunks for full content.",
            outputs.len(),
            byte_count
        )
    } else {
        sanitized_preview(&preview_bytes)
    };
    OutputSignature {
        output_digest_hex: vpsman_common::payload_hash(&canonical),
        output_compare_basis: if has_artifact_backed_output {
            "binary_metadata".to_string()
        } else {
            "binary".to_string()
        },
        stream_count: outputs.len() as i32,
        byte_count,
        preview,
    }
}

fn normalize_text_for_comparison(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}

fn command_template_from_row(
    row: sqlx::postgres::PgRow,
) -> std::result::Result<CommandTemplateView, sqlx::Error> {
    let operation: SqlJson<serde_json::Value> = row.try_get("operation")?;
    let defaults: SqlJson<serde_json::Value> = row.try_get("defaults")?;
    Ok(CommandTemplateView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        built_in: false,
        scope_kind: row.try_get("scope_kind")?,
        scope_value: row.try_get("scope_value")?,
        command_type: row.try_get("command_type")?,
        display_group: row.try_get("display_group")?,
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
    action: &'static str,
) -> Result<()> {
    let metadata = serde_json::json!({
        "template_id": template.id,
        "name": template.name,
        "scope_kind": template.scope_kind,
        "scope_value": template.scope_value,
        "command_type": template.command_type,
        "display_group": template.display_group,
        "operator_id": operator.operator.id,
        "operator_username": operator.operator.username,
    });
    match repo {
        Repository::Memory(memory) => {
            memory.audits.write().await.push(AuditLogView {
                id: Uuid::new_v4(),
                actor_id: Some(operator.operator.id),
                action: action.to_string(),
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
            .bind(action)
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
            "global" | "provider" | "tag" | "client"
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
    if let Some(display_group) = request.display_group.as_deref() {
        validate_template_token(display_group, 64, "display_group")?;
    }
    ensure!(
        request.operation.is_object(),
        "command template operation must be a JSON object"
    );
    let _ = parsed_template_operation(request)?;
    ensure!(
        request.defaults.is_null() || request.defaults.is_object(),
        "command template defaults must be a JSON object"
    );
    Ok(())
}

struct CommandTemplateParts {
    command_type: String,
    display_group: Option<String>,
    operation: serde_json::Value,
}

fn command_template_parts(request: &UpsertCommandTemplateRequest) -> Result<CommandTemplateParts> {
    let command = parsed_template_operation(request)?;
    let command_type = job_command_type_label(&command).to_string();
    let display_group = request
        .display_group
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| job_command_display_group(&command_type).map(ToString::to_string));
    Ok(CommandTemplateParts {
        command_type,
        display_group,
        operation: request.operation.clone(),
    })
}

fn parsed_template_operation(request: &UpsertCommandTemplateRequest) -> Result<JobCommand> {
    Ok(serde_json::from_value::<JobCommand>(
        request.operation.clone(),
    )?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{OperatorPreferences, OperatorView},
        repository::{MemoryState, Repository},
        DEFAULT_REFRESH_TOKEN_TTL_SECS,
    };

    fn test_operator() -> AuthContext {
        AuthContext {
            operator: OperatorView {
                id: Uuid::new_v4(),
                username: "template-admin".to_string(),
                role: "admin".to_string(),
                scopes: vec!["*".to_string()],
                preferences: OperatorPreferences::default(),
                totp_enabled: false,
                status: "active".to_string(),
                session_refresh_ttl_secs: DEFAULT_REFRESH_TOKEN_TTL_SECS,
                created_at: unix_now().to_string(),
                disabled_at: None,
                deleted_at: None,
            },
            session_id: Uuid::new_v4(),
        }
    }

    fn shell_template_request(name: &str, scope_kind: &str) -> UpsertCommandTemplateRequest {
        UpsertCommandTemplateRequest {
            name: name.to_string(),
            scope_kind: scope_kind.to_string(),
            scope_value: None,
            display_group: None,
            operation: serde_json::json!({
                "type": "shell",
                "argv": ["/usr/bin/uptime"],
                "pty": false
            }),
            defaults: serde_json::json!({
                "timeout_secs": 30,
                "confirmed": false
            }),
            confirmed: true,
        }
    }

    #[tokio::test]
    async fn command_template_builtins_are_listed_and_immutable() {
        let repo = Repository::Memory(MemoryState::default());
        let templates = repo
            .list_command_templates(20, None, None, None, None)
            .await
            .unwrap();

        let shell = templates
            .iter()
            .find(|template| template.name == "Default shell command")
            .expect("default shell builtin missing");
        assert!(shell.built_in);
        assert_eq!(shell.scope_kind, "global");
        assert_eq!(shell.command_type, "shell_argv");

        let updates = repo
            .list_command_templates(20, None, None, Some("agent_update_check"), None)
            .await
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert!(updates[0].built_in);
        assert_eq!(updates[0].name, "Default manual update check");

        let error = repo
            .upsert_command_template(
                &shell_template_request("Default shell command", "global"),
                &test_operator(),
            )
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("command_template_builtin_immutable"));
    }

    #[tokio::test]
    async fn user_defined_command_templates_can_be_deleted() {
        let repo = Repository::Memory(MemoryState::default());
        let operator = test_operator();
        let created = repo
            .upsert_command_template(
                &shell_template_request("operator-health-check", "global"),
                &operator,
            )
            .await
            .unwrap();
        assert!(!created.built_in);

        let deleted = repo
            .delete_command_template(created.id, &operator)
            .await
            .unwrap()
            .expect("user template should delete");
        assert_eq!(deleted.id, created.id);
        assert!(!deleted.built_in);

        let templates = repo
            .list_command_templates(20, None, None, None, None)
            .await
            .unwrap();
        assert!(!templates.iter().any(|template| template.id == created.id));

        let Repository::Memory(memory) = repo else {
            unreachable!("test uses memory repository");
        };
        let audits = memory.audits.read().await;
        assert!(audits
            .iter()
            .any(|audit| audit.action == "command_template.upserted"));
        assert!(audits
            .iter()
            .any(|audit| audit.action == "command_template.deleted"));
    }
}
