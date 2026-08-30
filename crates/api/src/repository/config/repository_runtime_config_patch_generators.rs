use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;

use crate::{
    model::{
        AuthContext, RenderRuntimeConfigPatchGeneratorRequest,
        RuntimeConfigPatchGeneratorRenderView, RuntimeConfigPatchGeneratorView,
        UpsertRuntimeConfigPatchGeneratorRequest,
    },
    repository::Repository,
    runtime_config_workspace::validate_runtime_config_bulk_patch,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_runtime_config_patch_generators(
        &self,
    ) -> Result<Vec<RuntimeConfigPatchGeneratorView>> {
        self.ensure_builtin_runtime_config_patch_generators()
            .await?;
        match self {
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
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if request.id.is_none() {
                    // An id-less retry is idempotent only for this complete
                    // material identity. PostgreSQL's JSONB rendering
                    // canonicalizes nested object key order before hashing, so
                    // equivalent generators share one owner without
                    // serializing unrelated definitions.
                    sqlx::query(
                        r#"
                        SELECT pg_advisory_xact_lock(hashtextextended(
                            'vpsman:runtime-config-patch-generator-material:' ||
                            jsonb_build_array(
                                $1::text,
                                $2::text,
                                $3::text,
                                $4::text,
                                $5::jsonb,
                                $6::text,
                                $7::jsonb
                            )::text,
                            0
                        ))
                        "#,
                    )
                    .bind(&generator.name)
                    .bind(&generator.category)
                    .bind(&generator.domain)
                    .bind(&generator.description)
                    .bind(SqlJson(&generator.field_schema))
                    .bind(&generator.raw_generator_body)
                    .bind(SqlJson(&generator.docs_metadata))
                    .execute(&mut *tx)
                    .await?;
                    let existing = sqlx::query(
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
                        WHERE built_in = FALSE
                          AND name = $1
                          AND category = $2
                          AND domain = $3
                          AND description = $4
                          AND field_schema = $5
                          AND raw_generator_body = $6
                          AND docs_metadata = $7
                        ORDER BY created_at, id
                        LIMIT 1
                        FOR UPDATE
                        "#,
                    )
                    .bind(&generator.name)
                    .bind(&generator.category)
                    .bind(&generator.domain)
                    .bind(&generator.description)
                    .bind(SqlJson(&generator.field_schema))
                    .bind(&generator.raw_generator_body)
                    .bind(SqlJson(&generator.docs_metadata))
                    .fetch_optional(&mut *tx)
                    .await?
                    .map(patch_generator_from_row)
                    .transpose()?;
                    if let Some(existing) = existing {
                        tx.commit().await?;
                        return Ok(existing);
                    }
                }
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
                .fetch_optional(&mut *tx)
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
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
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
        let (operations, affected_sections) = validate_runtime_config_bulk_patch(&rendered)?;
        Ok(RuntimeConfigPatchGeneratorRenderView {
            generator_id: generator.id,
            name: generator.name,
            toml: rendered,
            patch: serde_json::to_value(&operations)
                .context("failed to serialize rendered patch operations")?,
            affected_sections,
            docs_metadata: generator.docs_metadata,
            generated_at: unix_now().to_string(),
        })
    }

    pub(crate) async fn delete_runtime_config_patch_generator(
        &self,
        generator_id: Uuid,
        reviewed_name: &str,
        operator: &AuthContext,
    ) -> Result<()> {
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let current = sqlx::query(
                    r#"
                    SELECT name, built_in
                    FROM runtime_config_patch_generators
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(generator_id)
                .fetch_optional(&mut *tx)
                .await?
                .with_context(|| "runtime_config_patch_generator_not_found")?;
                anyhow::ensure!(
                    !current.try_get::<bool, _>("built_in")?,
                    "runtime_config_patch_generator_builtin_immutable"
                );
                anyhow::ensure!(
                    current.try_get::<String, _>("name")? == reviewed_name.trim(),
                    "runtime_config_patch_generator_delete_review_stale"
                );
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
                .fetch_optional(&mut *tx)
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
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(())
            }
        }
    }

    async fn ensure_builtin_runtime_config_patch_generators(&self) -> Result<()> {
        match self {
            Self::Postgres(_) => Ok(()),
        }
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
        "result": "succeeded",
        "operator_id": operator.operator.id,
        "operator_username": &operator.operator.username,
        "operator_role": &operator.operator.role,
        "operator_session_id": operator.audit_session_id(),
        "origin_kind": "operator_request",
        "component": "runtime-config-generator-controller",
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
    validate_runtime_config_bulk_patch(&rendered)?;
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
