use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::validate_incremental_config_patch_section;

use crate::{
    model::{
        AuditLogView, AuthContext, HotConfigRuleTemplateRenderView, HotConfigRuleTemplateView,
        RenderHotConfigRuleTemplateRequest, UpsertHotConfigRuleTemplateRequest,
    },
    repository::Repository,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_hot_config_rule_templates(
        &self,
    ) -> Result<Vec<HotConfigRuleTemplateView>> {
        self.ensure_builtin_hot_config_rule_templates().await?;
        match self {
            Self::Memory(memory) => {
                let mut templates = memory.hot_config_rule_templates.read().await.clone();
                templates.sort_by(|left, right| {
                    left.category
                        .cmp(&right.category)
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(templates)
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
                    FROM hot_config_rule_templates
                    ORDER BY category, name, id
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(rule_template_from_row).collect()
            }
        }
    }

    pub(crate) async fn upsert_hot_config_rule_template(
        &self,
        request: &UpsertHotConfigRuleTemplateRequest,
        operator: &AuthContext,
    ) -> Result<HotConfigRuleTemplateView> {
        let id = request.id.unwrap_or_else(Uuid::new_v4);
        let now = unix_now().to_string();
        let template = HotConfigRuleTemplateView {
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
        validate_rule_template_renderable(&template.raw_generator_body, &template.field_schema)?;
        match self {
            Self::Memory(memory) => {
                self.ensure_builtin_hot_config_rule_templates().await?;
                let mut templates = memory.hot_config_rule_templates.write().await;
                let saved = if let Some(existing) =
                    templates.iter_mut().find(|existing| existing.id == id)
                {
                    anyhow::ensure!(
                        !existing.built_in,
                        "hot_config_rule_template_builtin_immutable"
                    );
                    let created_at = existing.created_at.clone();
                    *existing = HotConfigRuleTemplateView {
                        id: template.id,
                        name: template.name.clone(),
                        category: template.category.clone(),
                        domain: template.domain.clone(),
                        description: template.description.clone(),
                        field_schema: template.field_schema.clone(),
                        raw_generator_body: template.raw_generator_body.clone(),
                        docs_metadata: template.docs_metadata.clone(),
                        built_in: false,
                        actor_id: template.actor_id,
                        created_at,
                        updated_at: template.updated_at.clone(),
                    };
                    existing.clone()
                } else {
                    templates.push(template.clone());
                    template.clone()
                };
                memory.audits.write().await.push(hot_config_template_audit(
                    "hot_config_rule_template.saved",
                    &saved,
                    operator,
                    unix_now().to_string(),
                ));
                Ok(saved)
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    INSERT INTO hot_config_rule_templates (
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
                    WHERE hot_config_rule_templates.built_in = FALSE
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
                .bind(&template.name)
                .bind(&template.category)
                .bind(&template.domain)
                .bind(&template.description)
                .bind(SqlJson(&template.field_schema))
                .bind(&template.raw_generator_body)
                .bind(SqlJson(&template.docs_metadata))
                .bind(operator.operator.id)
                .fetch_optional(pool)
                .await?;
                let row = row.with_context(|| "hot_config_rule_template_builtin_immutable")?;
                let saved = rule_template_from_row(row)?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("hot_config_rule_template.saved")
                .bind(format!("hot_config_rule_template:{}", saved.id))
                .bind(Option::<String>::None)
                .bind(hot_config_template_audit_metadata(&saved, operator))
                .execute(pool)
                .await?;
                Ok(saved)
            }
        }
    }

    pub(crate) async fn render_hot_config_rule_template(
        &self,
        template_id: Uuid,
        request: &RenderHotConfigRuleTemplateRequest,
    ) -> Result<HotConfigRuleTemplateRenderView> {
        let template = self
            .list_hot_config_rule_templates()
            .await?
            .into_iter()
            .find(|candidate| candidate.id == template_id)
            .with_context(|| format!("hot_config_rule_template_not_found:{template_id}"))?;
        let rendered = render_template_body(
            &template.raw_generator_body,
            &request.values,
            &template.field_schema,
        )?;
        let patch: toml::Value =
            toml::from_str(&rendered).context("failed to parse rendered config patch TOML")?;
        let affected_sections = validate_rendered_patch(&patch)?;
        Ok(HotConfigRuleTemplateRenderView {
            template_id: template.id,
            name: template.name,
            toml: rendered,
            patch: serde_json::to_value(&patch).context("failed to serialize rendered patch")?,
            affected_sections,
            docs_metadata: template.docs_metadata,
            generated_at: unix_now().to_string(),
        })
    }

    pub(crate) async fn delete_hot_config_rule_template(
        &self,
        template_id: Uuid,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                self.ensure_builtin_hot_config_rule_templates().await?;
                let mut templates = memory.hot_config_rule_templates.write().await;
                let existing = templates
                    .iter()
                    .find(|template| template.id == template_id)
                    .cloned()
                    .with_context(|| "hot_config_rule_template_not_found")?;
                anyhow::ensure!(
                    !existing.built_in,
                    "hot_config_rule_template_builtin_immutable"
                );
                templates.retain(|template| template.id != template_id);
                memory.audits.write().await.push(hot_config_template_audit(
                    "hot_config_rule_template.deleted",
                    &existing,
                    operator,
                    unix_now().to_string(),
                ));
                Ok(())
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    DELETE FROM hot_config_rule_templates
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
                .bind(template_id)
                .fetch_optional(pool)
                .await?;
                let deleted = row
                    .map(rule_template_from_row)
                    .transpose()?
                    .with_context(|| "hot_config_rule_template_not_found")?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind("hot_config_rule_template.deleted")
                .bind(format!("hot_config_rule_template:{}", deleted.id))
                .bind(Option::<String>::None)
                .bind(hot_config_template_audit_metadata(&deleted, operator))
                .execute(pool)
                .await?;
                Ok(())
            }
        }
    }

    async fn ensure_builtin_hot_config_rule_templates(&self) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut seeded = memory.hot_config_rule_templates_seeded.write().await;
                if *seeded {
                    return Ok(());
                }
                let mut templates = memory.hot_config_rule_templates.write().await;
                for template in builtin_rule_templates() {
                    if !templates.iter().any(|existing| existing.id == template.id) {
                        templates.push(template);
                    }
                }
                *seeded = true;
                Ok(())
            }
            Self::Postgres(_) => Ok(()),
        }
    }
}

fn hot_config_template_audit(
    action: &str,
    template: &HotConfigRuleTemplateView,
    operator: &AuthContext,
    created_at: String,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: Some(operator.operator.id),
        action: action.to_string(),
        target: format!("hot_config_rule_template:{}", template.id),
        command_hash: None,
        metadata: hot_config_template_audit_metadata(template, operator),
        created_at,
    }
}

fn hot_config_template_audit_metadata(
    template: &HotConfigRuleTemplateView,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "template_id": template.id,
        "name": template.name,
        "category": template.category,
        "domain": template.domain,
        "description": template.description,
        "field_schema": template.field_schema,
        "raw_generator_body": template.raw_generator_body,
        "docs_metadata": template.docs_metadata,
        "built_in": template.built_in,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "session_id": operator.session_id,
    })
}

fn rule_template_from_row(row: sqlx::postgres::PgRow) -> Result<HotConfigRuleTemplateView> {
    Ok(HotConfigRuleTemplateView {
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

fn validate_rule_template_renderable(body: &str, field_schema: &JsonValue) -> Result<()> {
    let rendered = render_template_body(body, &serde_json::json!({}), field_schema)?;
    let patch: toml::Value =
        toml::from_str(&rendered).context("failed to parse config template TOML")?;
    validate_rendered_patch(&patch)?;
    Ok(())
}

fn render_template_body(
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
        JsonValue::Object(_) => anyhow::bail!("template object values are not supported"),
    })
}

fn validate_rendered_patch(patch: &toml::Value) -> Result<Vec<String>> {
    let Some(table) = patch.as_table() else {
        anyhow::bail!("config rule-template patch must be a TOML table");
    };
    anyhow::ensure!(
        !table.is_empty(),
        "config rule-template patch must contain at least one section"
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

fn builtin_rule_templates() -> Vec<HotConfigRuleTemplateView> {
    vec![
        predefined_template(
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
        predefined_template(
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
        predefined_template(
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
        predefined_template(
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
        predefined_template(
            "66666666-6666-4666-8666-666666666666",
            "Autonomous updater disabled",
            "update",
            "agent_update",
            "Disable agent autonomous self-update while keeping manifest URL and interval values explicit in agent config.",
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
        predefined_template(
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

fn predefined_template(
    id: &str,
    name: &str,
    category: &str,
    domain: &str,
    description: &str,
    field_schema: JsonValue,
    raw_generator_body: &str,
) -> HotConfigRuleTemplateView {
    HotConfigRuleTemplateView {
        id: Uuid::parse_str(id).expect("predefined rule template UUID must parse"),
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
