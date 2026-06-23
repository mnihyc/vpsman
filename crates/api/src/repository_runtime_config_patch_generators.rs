use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::validate_incremental_config_patch_section;

use crate::{
    model::{
        AuditLogView, AuthContext, RenderRuntimeConfigPatchGeneratorRequest,
        RuntimeConfigPatchGeneratorRenderView, RuntimeConfigPatchGeneratorView,
        UpsertRuntimeConfigPatchGeneratorRequest,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_runtime_config_patch_generators(
        &self,
    ) -> Result<Vec<RuntimeConfigPatchGeneratorView>> {
        self.ensure_builtin_runtime_config_patch_generators()
            .await?;
        match self {
            Self::Memory(memory) => {
                let mut generators = memory.runtime_config_patch_generators.read().await.clone();
                generators.sort_by(|left, right| {
                    left.category
                        .cmp(&right.category)
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(generators)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        name,
                        category,
                        domain,
                        description,
                        field_schema,
                        raw_generator_body,
                        docs_metadata,
                        built_in,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM runtime_config_patch_generators
                    ORDER BY category, name, id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(patch_generator_from_row).collect()
            }
        }
    }

    pub(crate) async fn upsert_runtime_config_patch_generator(
        &self,
        request: &UpsertRuntimeConfigPatchGeneratorRequest,
        operator: &AuthContext,
    ) -> Result<RuntimeConfigPatchGeneratorView> {
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        let now = unix_now().to_string();
        let generator = RuntimeConfigPatchGeneratorView {
            id,
            name: request.name.trim().to_string(),
            category: request.category.trim().to_string(),
            domain: request.domain.trim().to_string(),
            description: request.description.trim().to_string(),
            field_schema: request.field_schema.clone(),
            raw_generator_body: request.raw_generator_body.trim().to_string(),
            docs_metadata: request.docs_metadata.clone(),
            built_in: false,
            actor_id: Some(operator.operator.id),
            created_at: now.clone(),
            updated_at: now,
        };
        validate_patch_generator_renderable(
            &generator.raw_generator_body,
            &generator.field_schema,
        )?;
        match self {
            Self::Memory(memory) => {
                self.ensure_builtin_runtime_config_patch_generators()
                    .await?;
                let mut generators = memory.runtime_config_patch_generators.write().await;
                let saved = if let Some(existing) =
                    generators.iter_mut().find(|existing| existing.id == id)
                {
                    anyhow::ensure!(
                        !existing.built_in,
                        "runtime_config_patch_generator_builtin_immutable"
                    );
                    let created_at = existing.created_at.clone();
                    *existing = RuntimeConfigPatchGeneratorView {
                        id: generator.id,
                        name: generator.name.clone(),
                        category: generator.category.clone(),
                        domain: generator.domain.clone(),
                        description: generator.description.clone(),
                        field_schema: generator.field_schema.clone(),
                        raw_generator_body: generator.raw_generator_body.clone(),
                        docs_metadata: generator.docs_metadata.clone(),
                        built_in: false,
                        actor_id: generator.actor_id,
                        created_at,
                        updated_at: generator.updated_at.clone(),
                    };
                    existing.clone()
                } else {
                    generators.push(generator.clone());
                    generator.clone()
                };
                memory
                    .audits
                    .write()
                    .await
                    .push(runtime_config_patch_generator_audit(
                        "runtime_config_patch_generator.saved",
                        &saved,
                        operator,
                        unix_now().to_string(),
                    ));
                Ok(saved)
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO runtime_config_patch_generators (
                        id,
                        name,
                        category,
                        domain,
                        description,
                        field_schema,
                        raw_generator_body,
                        docs_metadata,
                        built_in,
                        actor_id
                    )
                    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, $9)
                    ON CONFLICT (id) DO UPDATE SET
                        name = EXCLUDED.name,
                        category = EXCLUDED.category,
                        domain = EXCLUDED.domain,
                        description = EXCLUDED.description,
                        field_schema = EXCLUDED.field_schema,
                        raw_generator_body = EXCLUDED.raw_generator_body,
                        docs_metadata = EXCLUDED.docs_metadata,
                        actor_id = EXCLUDED.actor_id,
                        updated_at = now()
                    WHERE runtime_config_patch_generators.built_in = FALSE
                    RETURNING
                        id,
                        name,
                        category,
                        domain,
                        description,
                        field_schema,
                        raw_generator_body,
                        docs_metadata,
                        built_in,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(id)
                .bind(&generator.name)
                .bind(&generator.category)
                .bind(&generator.domain)
                .bind(&generator.description)
                .bind(SqlJson(&generator.field_schema))
                .bind(&generator.raw_generator_body)
                .bind(SqlJson(&generator.docs_metadata))
                .bind(operator.operator.id)
                .fetch_optional(pool)
                .await?;
                let row =
                    row.with_context(|| "runtime_config_patch_generator_builtin_immutable")?;
                let saved = patch_generator_from_row(row)?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("runtime_config_patch_generator.saved")
                .bind(format!("runtime_config_patch_generator:{}", saved.id))
                .bind(Option::<String>::None)
                .bind(runtime_config_patch_generator_audit_metadata(
                    &saved, operator,
                ))
                .execute(pool)
                .await?;
                Ok(saved)
            }
        }
    }

    pub(crate) async fn render_runtime_config_patch_generator(
        &self,
        generator_id: Uuid,
        request: &RenderRuntimeConfigPatchGeneratorRequest,
    ) -> Result<RuntimeConfigPatchGeneratorRenderView> {
        let generator = self
            .list_runtime_config_patch_generators()
            .await?
            .into_iter()
            .find(|candidate| candidate.id == generator_id)
            .with_context(|| format!("runtime_config_patch_generator_not_found:{generator_id}"))?;
        let rendered = render_generator_body(
            &generator.raw_generator_body,
            &request.values,
            &generator.field_schema,
        )?;
        let patch: toml::Value =
            toml::from_str(&rendered).context("failed to parse rendered config patch TOML")?;
        let affected_sections = validate_rendered_patch(&patch)?;
        Ok(RuntimeConfigPatchGeneratorRenderView {
            generator_id: generator.id,
            name: generator.name,
            toml: rendered,
            patch: serde_json::to_value(&patch).context("failed to serialize rendered patch")?,
            affected_sections,
            docs_metadata: generator.docs_metadata,
            generated_at: unix_now().to_string(),
        })
    }

    pub(crate) async fn delete_runtime_config_patch_generator(
        &self,
        generator_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                self.ensure_builtin_runtime_config_patch_generators()
                    .await?;
                let mut generators = memory.runtime_config_patch_generators.write().await;
                let existing = generators
                    .iter()
                    .find(|generator| generator.id == generator_id)
                    .cloned()
                    .with_context(|| "runtime_config_patch_generator_not_found")?;
                anyhow::ensure!(
                    !existing.built_in,
                    "runtime_config_patch_generator_builtin_immutable"
                );
                generators.retain(|generator| generator.id != generator_id);
                memory
                    .audits
                    .write()
                    .await
                    .push(runtime_config_patch_generator_audit(
                        "runtime_config_patch_generator.deleted",
                        &existing,
                        operator,
                        unix_now().to_string(),
                    ));
                Ok(())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    DELETE FROM runtime_config_patch_generators
                    WHERE id = $1 AND built_in = FALSE
                    RETURNING
                        id,
                        name,
                        category,
                        domain,
                        description,
                        field_schema,
                        raw_generator_body,
                        docs_metadata,
                        built_in,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    "#,
                )
                .bind(generator_id)
                .fetch_optional(pool)
                .await?;
                let deleted = row
                    .map(patch_generator_from_row)
                    .transpose()?
                    .with_context(|| "runtime_config_patch_generator_not_found")?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("runtime_config_patch_generator.deleted")
                .bind(format!("runtime_config_patch_generator:{}", deleted.id))
                .bind(Option::<String>::None)
                .bind(runtime_config_patch_generator_audit_metadata(
                    &deleted, operator,
                ))
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    async fn ensure_builtin_runtime_config_patch_generators(&self) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut seeded = memory.runtime_config_patch_generators_seeded.write().await;
                if *seeded {
                    return Ok(());
                }
                let mut generators = memory.runtime_config_patch_generators.write().await;
                for generator in builtin_patch_generators() {
                    if !generators
                        .iter()
                        .any(|existing| existing.id == generator.id)
                    {
                        generators.push(generator);
                    }
                }
                *seeded = true;
                Ok(())
            }
            Self::Postgres(_) => Ok(()),
        }
    }
}

fn runtime_config_patch_generator_audit(
    action: &str,
    generator: &RuntimeConfigPatchGeneratorView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("runtime_config_patch_generator:{}", generator.id),
        command_hash: None,
        metadata: runtime_config_patch_generator_audit_metadata(generator, operator),
        created_at,
    }
}

fn runtime_config_patch_generator_audit_metadata(
    generator: &RuntimeConfigPatchGeneratorView,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "generator_id": generator.id,
        "name": generator.name,
        "category": generator.category,
        "domain": generator.domain,
        "description": generator.description,
        "field_schema": generator.field_schema,
        "raw_generator_body": generator.raw_generator_body,
        "docs_metadata": generator.docs_metadata,
        "built_in": generator.built_in,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
    })
}

fn patch_generator_from_row(row: sqlx::postgres::PgRow) -> Result<RuntimeConfigPatchGeneratorView> {
    Ok(RuntimeConfigPatchGeneratorView {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        category: row.try_get("category")?,
        domain: row.try_get("domain")?,
        description: row.try_get("description")?,
        field_schema: row.try_get::<SqlJson<JsonValue>, _>("field_schema")?.0,
        raw_generator_body: row.try_get("raw_generator_body")?,
        docs_metadata: row.try_get::<SqlJson<JsonValue>, _>("docs_metadata")?.0,
        built_in: row.try_get("built_in")?,
        actor_id: row.try_get("actor_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn validate_patch_generator_renderable(body: &str, field_schema: &JsonValue) -> Result<()> {
    let rendered = render_generator_body(body, &serde_json::json!({}), field_schema)?;
    let patch: toml::Value =
        toml::from_str(&rendered).context("failed to parse config generator TOML")?;
    validate_rendered_patch(&patch)?;
    Ok(())
}

fn render_generator_body(
    body: &str,
    values: &JsonValue,
    field_schema: &JsonValue,
) -> Result<String> {
    let mut rendered = body.to_string();
    let values = values.as_object();
    for placeholder in placeholders(body) {
        let value = values
            .and_then(|values| values.get(&placeholder))
            .or_else(|| schema_default(field_schema, &placeholder));
        let literal = value.map(toml_literal).transpose()?.unwrap_or_default();
        rendered = rendered.replace(&format!("{{{{{placeholder}}}}}"), &literal);
    }
    Ok(rendered)
}

fn schema_default<'a>(field_schema: &'a JsonValue, placeholder: &str) -> Option<&'a JsonValue> {
    for section in ["fields", "properties"] {
        let default = field_schema
            .get(section)
            .and_then(JsonValue::as_object)
            .and_then(|fields| fields.get(placeholder))
            .and_then(|field| field.get("default"));
        if default.is_some() {
            return default;
        }
    }
    None
}

fn placeholders(body: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let after_start = &rest[start + 2..];
        let Some(end) = after_start.find("}}") else {
            break;
        };
        let name = after_start[..end].trim();
        if !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            result.push(name.to_string());
        }
        rest = &after_start[end + 2..];
    }
    result.sort();
    result.dedup();
    result
}

fn toml_literal(value: &JsonValue) -> Result<String> {
    Ok(match value {
        JsonValue::String(value) => serde_json::to_string(value)?,
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Array(values) => {
            let items = values
                .iter()
                .map(toml_literal)
                .collect::<Result<Vec<_>>>()?
                .join(", ");
            format!("[{items}]")
        }
        JsonValue::Null => String::new(),
        JsonValue::Object(_) => anyhow::bail!("generator object values are not supported"),
    })
}

fn validate_rendered_patch(patch: &toml::Value) -> Result<Vec<String>> {
    let Some(table) = patch.as_table() else {
        anyhow::bail!("config patch generator output must be a TOML table");
    };
    anyhow::ensure!(
        !table.is_empty(),
        "config patch generator output must contain at least one section"
    );
    let mut sections = Vec::new();
    for section in table.keys() {
        validate_incremental_config_patch_section(section)
            .map_err(|message| anyhow::anyhow!(message))?;
        sections.push(section.clone());
    }
    sections.sort();
    Ok(sections)
}

fn builtin_patch_generators() -> Vec<RuntimeConfigPatchGeneratorView> {
    vec![
        predefined_patch_generator(
            "11111111-1111-4111-8111-111111111111",
            "Telemetry source",
            "telemetry",
            "metrics",
            "Switch telemetry collection source and optional Linux paths.",
            serde_json::json!({
                "fields": {
                    "source": {"type": "string", "enum": ["linux_procfs", "custom_command", "linux_procfs_and_custom_command"], "default": "linux_procfs"},
                    "proc_root": {"type": "string", "default": "/proc"},
                    "sys_class_net_dir": {"type": "string", "default": "/sys/class/net"}
                }
            }),
            "[telemetry]\nsource = {{source}}\nproc_root = {{proc_root}}\nsys_class_net_dir = {{sys_class_net_dir}}\n",
        ),
        predefined_patch_generator(
            "22222222-2222-4222-8222-222222222222",
            "Execution policy",
            "execution",
            "command",
            "Set command execution environment and PTY policy.",
            serde_json::json!({
                "fields": {
                    "environment_policy": {"type": "string", "enum": ["inherit", "clean", "minimal_path"], "default": "inherit"},
                    "pty_policy": {"type": "string", "enum": ["native_pty", "disabled"], "default": "native_pty"}
                }
            }),
            "[execution]\nenvironment_policy = {{environment_policy}}\npty_policy = {{pty_policy}}\n",
        ),
        predefined_patch_generator(
            "33333333-3333-4333-8333-333333333333",
            "Runtime tunnel adapter",
            "network",
            "runtime",
            "Adjust runtime tunnel adapter safety and reconciliation flags.",
            serde_json::json!({
                "fields": {
                    "apply_enabled": {"type": "boolean"},
                    "runtime_reconcile_enabled": {"type": "boolean"},
                    "runtime_command_timeout_secs": {"type": "number", "minimum": 1, "maximum": 120}
                }
            }),
            "[network]\napply_enabled = {{apply_enabled}}\nruntime_reconcile_enabled = {{runtime_reconcile_enabled}}\nruntime_command_timeout_secs = {{runtime_command_timeout_secs}}\n",
        ),
        predefined_patch_generator(
            "55555555-5555-4555-8555-555555555555",
            "Autonomous updater enabled",
            "update",
            "agent_update",
            "Enable agent autonomous self-update from an external version manifest.",
            serde_json::json!({
                "fields": {
                    "unmanaged_version_url": {"type": "string", "default": "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"},
                    "unmanaged_interval_secs": {"type": "integer", "minimum": 300, "maximum": 604800, "default": 86400},
                    "unmanaged_jitter_secs": {"type": "integer", "minimum": 0, "maximum": 604800, "default": 86400},
                    "unmanaged_activate": {"type": "boolean", "default": true},
                    "unmanaged_restart_agent": {"type": "boolean", "default": true}
                }
            }),
            "[update]\nunmanaged_enabled = true\nunmanaged_version_url = {{unmanaged_version_url}}\nunmanaged_interval_secs = {{unmanaged_interval_secs}}\nunmanaged_jitter_secs = {{unmanaged_jitter_secs}}\nunmanaged_activate = {{unmanaged_activate}}\nunmanaged_restart_agent = {{unmanaged_restart_agent}}\n",
        ),
        predefined_patch_generator(
            "66666666-6666-4666-8666-666666666666",
            "Autonomous updater disabled",
            "update",
            "agent_update",
            "Disable agent autonomous self-update while keeping manifest URL and interval values explicit in runtime config.",
            serde_json::json!({
                "fields": {
                    "unmanaged_version_url": {"type": "string", "default": "https://github.com/mnihyc/vpsman/releases/latest/download/version.json"},
                    "unmanaged_interval_secs": {"type": "integer", "minimum": 300, "maximum": 604800, "default": 86400},
                    "unmanaged_jitter_secs": {"type": "integer", "minimum": 0, "maximum": 604800, "default": 86400},
                    "unmanaged_activate": {"type": "boolean", "default": true},
                    "unmanaged_restart_agent": {"type": "boolean", "default": true}
                }
            }),
            "[update]\nunmanaged_enabled = false\nunmanaged_version_url = {{unmanaged_version_url}}\nunmanaged_interval_secs = {{unmanaged_interval_secs}}\nunmanaged_jitter_secs = {{unmanaged_jitter_secs}}\nunmanaged_activate = {{unmanaged_activate}}\nunmanaged_restart_agent = {{unmanaged_restart_agent}}\n",
        ),
        predefined_patch_generator(
            "44444444-4444-4444-8444-444444444444",
            "Routing daemon adapter",
            "network",
            "routing",
            "Configure interval latency monitoring and the agent-level fallback external OSPF cost updater. Tunnel-local updaters remain higher priority.",
            serde_json::json!({
                "fields": {
                    "latency_monitoring_enabled": {"type": "boolean", "default": true},
                    "latency_monitoring_interval_secs": {"type": "number", "minimum": 15, "maximum": 3600, "default": 60},
                    "latency_down_windows": {"type": "number", "minimum": 1, "maximum": 60, "default": 3},
                    "auto_ospf_enabled": {"type": "boolean", "default": false},
                    "auto_ospf_min_cost_delta": {"type": "number", "minimum": 1, "maximum": 65535, "default": 5},
                    "auto_ospf_healthy_windows": {"type": "number", "minimum": 1, "maximum": 10, "default": 2},
                    "updater_argv": {"type": "array", "default": ["/usr/local/libexec/vpsman-ospf-cost"]}
                }
            }),
            "[network]\nlatency_monitoring_enabled = {{latency_monitoring_enabled}}\nlatency_monitoring_interval_secs = {{latency_monitoring_interval_secs}}\nlatency_down_windows = {{latency_down_windows}}\nauto_ospf_enabled = {{auto_ospf_enabled}}\nauto_ospf_min_cost_delta = {{auto_ospf_min_cost_delta}}\nauto_ospf_healthy_windows = {{auto_ospf_healthy_windows}}\nauto_ospf_updater = { argv = {{updater_argv}}, max_timeout_secs = 10, max_output_bytes = 16384 }\n",
        ),
    ]
}

fn predefined_patch_generator(
    id: &str,
    name: &str,
    category: &str,
    domain: &str,
    description: &str,
    field_schema: JsonValue,
    raw_generator_body: &str,
) -> RuntimeConfigPatchGeneratorView {
    RuntimeConfigPatchGeneratorView {
        id: Uuid::parse_str(id).expect("predefined patch generator UUID must parse"),
        name: name.to_string(),
        category: category.to_string(),
        domain: domain.to_string(),
        description: description.to_string(),
        field_schema,
        raw_generator_body: raw_generator_body.to_string(),
        docs_metadata: serde_json::json!({
            "expandable": true,
            "affected_sections": [category],
            "patch_only": true,
            "predefined": true
        }),
        built_in: true,
        actor_id: None,
        created_at: "0".to_string(),
        updated_at: "0".to_string(),
    }
}
