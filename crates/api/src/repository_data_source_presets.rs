use std::collections::BTreeSet;

use anyhow::{Context, Result};
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::payload_hash;

use crate::{
    data_source_builtin_presets::builtin_presets,
    model::{
        AssignDataSourcePresetRequest, AssignDataSourcePresetResponse, AuditLogView, AuthContext,
        BulkResolveRequest, CloneDataSourcePresetRequest, CreateDataSourcePresetRequest,
        DataSourcePresetAssignmentView, DataSourcePresetDiffRequest, DataSourcePresetDiffView,
        DataSourcePresetTestView, DataSourcePresetView, TestDataSourcePresetRequest,
        UpdateDataSourcePresetRequest, UpdateDataSourcePresetResponse,
    },
    repository::Repository,
    repository_data_source_hot_config::render_data_source_preset_candidate,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_data_source_presets(
        &self,
        domain: Option<&str>,
    ) -> Result<Vec<DataSourcePresetView>> {
        self.ensure_builtin_data_source_presets().await?;
        match self {
            Self::Memory(memory) => {
                let assignments = memory.data_source_assignments.read().await;
                let mut presets = memory
                    .data_source_presets
                    .read()
                    .await
                    .iter()
                    .filter(|preset| domain.is_none_or(|domain| preset.domain == domain))
                    .cloned()
                    .map(|mut preset| {
                        preset.assigned_client_count = assignments
                            .iter()
                            .filter(|assignment| assignment.preset_id == preset.id)
                            .count() as i64;
                        preset
                    })
                    .collect::<Vec<_>>();
                presets.sort_by(|left, right| {
                    left.domain
                        .cmp(&right.domain)
                        .then_with(|| right.is_default.cmp(&left.is_default))
                        .then_with(|| left.scope.cmp(&right.scope))
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(presets)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        p.id,
                        p.domain,
                        p.name,
                        p.scope,
                        p.built_in,
                        p.is_default,
                        p.owner_client_id,
                        p.description,
                        p.definition,
                        p.created_at::text AS created_at,
                        p.updated_at::text AS updated_at,
                        count(a.client_id)::bigint AS assigned_client_count
                    FROM data_source_presets p
                    LEFT JOIN client_data_source_preset_assignments a ON a.preset_id = p.id
                    WHERE $1::TEXT IS NULL OR p.domain = $1
                    GROUP BY p.id
                    ORDER BY p.domain, p.is_default DESC, p.scope, p.name
                    "#,
                )
                .bind(domain)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(data_source_preset_from_row).collect()
            }
        }
    }

    pub(crate) async fn create_data_source_preset(
        &self,
        request: &CreateDataSourcePresetRequest,
        operator: &AuthContext,
    ) -> Result<DataSourcePresetView> {
        self.ensure_builtin_data_source_presets().await?;
        let now = unix_now().to_string();
        let scope = request.scope.trim();
        let owner = request.owner_client_id.as_deref().map(str::trim);
        let preset = match self {
            Self::Memory(memory) => {
                let mut presets = memory.data_source_presets.write().await;
                if let Some(existing) = presets.iter_mut().find(|preset| {
                    preset.domain == request.domain
                        && preset.name == request.name
                        && preset.scope == scope
                        && preset.owner_client_id.as_deref() == owner
                }) {
                    anyhow::ensure!(!existing.built_in, "data_source_preset_builtin_immutable");
                    existing.description = request.description.clone();
                    existing.definition = request.definition.clone();
                    existing.updated_at = now.clone();
                    existing.clone()
                } else {
                    let preset = DataSourcePresetView {
                        id: Uuid::new_v4(),
                        domain: request.domain.clone(),
                        name: request.name.clone(),
                        scope: scope.to_string(),
                        built_in: false,
                        is_default: false,
                        owner_client_id: owner.map(ToOwned::to_owned),
                        description: request.description.clone(),
                        definition: request.definition.clone(),
                        assigned_client_count: 0,
                        created_at: now.clone(),
                        updated_at: now.clone(),
                    };
                    presets.push(preset.clone());
                    preset
                }
            }
            Self::Postgres(pool) => {
                let existing_id = if let Some(owner) = owner {
                    sqlx::query(
                        r#"
                        SELECT id FROM data_source_presets
                        WHERE domain = $1 AND name = $2 AND owner_client_id = $3
                        "#,
                    )
                    .bind(&request.domain)
                    .bind(&request.name)
                    .bind(owner)
                    .fetch_optional(pool)
                    .await?
                    .map(|row| row.get::<Uuid, _>("id"))
                } else {
                    sqlx::query(
                        r#"
                        SELECT id FROM data_source_presets
                        WHERE domain = $1 AND name = $2 AND scope = $3 AND owner_client_id IS NULL
                        "#,
                    )
                    .bind(&request.domain)
                    .bind(&request.name)
                    .bind(scope)
                    .fetch_optional(pool)
                    .await?
                    .map(|row| row.get::<Uuid, _>("id"))
                };

                let row = if let Some(existing_id) = existing_id {
                    sqlx::query(
                        r#"
                        UPDATE data_source_presets
                        SET description = $2, definition = $3, updated_at = now()
                        WHERE id = $1 AND built_in = FALSE
                        RETURNING
                            id, domain, name, scope, built_in, is_default, owner_client_id,
                            description, definition, created_at::text AS created_at,
                            updated_at::text AS updated_at,
                            0::bigint AS assigned_client_count
                        "#,
                    )
                    .bind(existing_id)
                    .bind(&request.description)
                    .bind(sqlx::types::Json(&request.definition))
                    .fetch_one(pool)
                    .await?
                } else {
                    sqlx::query(
                        r#"
                        INSERT INTO data_source_presets (
                            id, domain, name, scope, built_in, is_default, owner_client_id,
                            description, definition
                        )
                        VALUES ($1, $2, $3, $4, FALSE, FALSE, $5, $6, $7)
                        RETURNING
                            id, domain, name, scope, built_in, is_default, owner_client_id,
                            description, definition, created_at::text AS created_at,
                            updated_at::text AS updated_at,
                            0::bigint AS assigned_client_count
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(&request.domain)
                    .bind(&request.name)
                    .bind(scope)
                    .bind(owner)
                    .bind(&request.description)
                    .bind(sqlx::types::Json(&request.definition))
                    .fetch_one(pool)
                    .await?
                };
                data_source_preset_from_row(row)?
            }
        };
        self.record_data_source_preset_audit(
            "data_source_preset.saved",
            &format!("data_source_preset:{}", preset.id),
            Some(&preset),
            &[],
            operator,
        )
        .await?;
        Ok(preset)
    }

    pub(crate) async fn clone_data_source_preset(
        &self,
        source_preset_id: Uuid,
        request: &CloneDataSourcePresetRequest,
        operator: &AuthContext,
    ) -> Result<DataSourcePresetView> {
        self.ensure_builtin_data_source_presets().await?;
        let source = self
            .data_source_preset_by_id(source_preset_id)
            .await?
            .with_context(|| format!("data_source_preset_not_found:{source_preset_id}"))?;
        let now = unix_now().to_string();
        let scope = request.scope.trim();
        let owner = request.owner_client_id.as_deref().map(str::trim);
        let description = request.description.clone().or(source.description.clone());
        let preset = match self {
            Self::Memory(memory) => {
                let mut presets = memory.data_source_presets.write().await;
                anyhow::ensure!(
                    !presets.iter().any(|preset| data_source_preset_key_matches(
                        preset,
                        &source.domain,
                        &request.name,
                        scope,
                        owner
                    )),
                    "data_source_preset_clone_target_exists"
                );
                let preset = DataSourcePresetView {
                    id: Uuid::new_v4(),
                    domain: source.domain.clone(),
                    name: request.name.clone(),
                    scope: scope.to_string(),
                    built_in: false,
                    is_default: false,
                    owner_client_id: owner.map(ToOwned::to_owned),
                    description,
                    definition: source.definition.clone(),
                    assigned_client_count: 0,
                    created_at: now.clone(),
                    updated_at: now,
                };
                presets.push(preset.clone());
                preset
            }
            Self::Postgres(pool) => {
                let exists = if let Some(owner) = owner {
                    sqlx::query(
                        r#"
                        SELECT id FROM data_source_presets
                        WHERE domain = $1 AND owner_client_id = $2 AND name = $3
                        "#,
                    )
                    .bind(&source.domain)
                    .bind(owner)
                    .bind(&request.name)
                    .fetch_optional(pool)
                    .await?
                    .is_some()
                } else {
                    sqlx::query(
                        r#"
                        SELECT id FROM data_source_presets
                        WHERE domain = $1 AND name = $2 AND scope = $3 AND owner_client_id IS NULL
                        "#,
                    )
                    .bind(&source.domain)
                    .bind(&request.name)
                    .bind(scope)
                    .fetch_optional(pool)
                    .await?
                    .is_some()
                };
                anyhow::ensure!(!exists, "data_source_preset_clone_target_exists");
                let row = sqlx::query(
                    r#"
                    INSERT INTO data_source_presets (
                        id, domain, name, scope, built_in, is_default, owner_client_id,
                        description, definition
                    )
                    VALUES ($1, $2, $3, $4, FALSE, FALSE, $5, $6, $7)
                    RETURNING
                        id, domain, name, scope, built_in, is_default, owner_client_id,
                        description, definition, created_at::text AS created_at,
                        updated_at::text AS updated_at,
                        0::bigint AS assigned_client_count
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(&source.domain)
                .bind(&request.name)
                .bind(scope)
                .bind(owner)
                .bind(&description)
                .bind(sqlx::types::Json(&source.definition))
                .fetch_one(pool)
                .await?;
                data_source_preset_from_row(row)?
            }
        };
        self.record_data_source_preset_audit(
            "data_source_preset.cloned",
            &format!("data_source_preset:{}", preset.id),
            Some(&preset),
            &[],
            operator,
        )
        .await?;
        Ok(preset)
    }

    pub(crate) async fn diff_data_source_preset(
        &self,
        preset_id: Uuid,
        request: &DataSourcePresetDiffRequest,
    ) -> Result<DataSourcePresetDiffView> {
        let preset = self
            .data_source_preset_by_id(preset_id)
            .await?
            .with_context(|| format!("data_source_preset_not_found:{preset_id}"))?;
        let candidate_description = if request.keep_description {
            preset.description.clone()
        } else {
            request.description.clone()
        };
        Ok(data_source_preset_diff(
            &preset,
            candidate_description,
            request.definition.clone(),
        ))
    }

    pub(crate) async fn test_data_source_preset(
        &self,
        preset_id: Uuid,
        request: &TestDataSourcePresetRequest,
    ) -> Result<DataSourcePresetTestView> {
        let preset = self
            .data_source_preset_by_id(preset_id)
            .await?
            .with_context(|| format!("data_source_preset_not_found:{preset_id}"))?;
        let mut candidate = preset.clone();
        candidate.definition = request.definition.clone();
        let (valid, renderable, error, sections, toml, unsupported_domains, render_notes) =
            match render_data_source_preset_candidate(&candidate) {
                Ok(rendered) => {
                    let renderable = rendered.unsupported_domains.is_empty();
                    (
                        true,
                        renderable,
                        None,
                        rendered.sections,
                        rendered.toml,
                        rendered.unsupported_domains,
                        rendered.render_notes,
                    )
                }
                Err(error) => (
                    false,
                    false,
                    Some(error.to_string()),
                    serde_json::json!({}),
                    String::new(),
                    Vec::new(),
                    Vec::new(),
                ),
            };
        Ok(DataSourcePresetTestView {
            preset_id: preset.id,
            domain: preset.domain,
            preset_name: preset.name,
            affected_client_count: preset.assigned_client_count,
            valid,
            renderable,
            error,
            sections,
            toml,
            unsupported_domains,
            render_notes,
            generated_at: unix_now().to_string(),
        })
    }

    pub(crate) async fn update_data_source_preset(
        &self,
        preset_id: Uuid,
        request: &UpdateDataSourcePresetRequest,
        operator: &AuthContext,
    ) -> Result<UpdateDataSourcePresetResponse> {
        self.ensure_builtin_data_source_presets().await?;
        let preset = self
            .data_source_preset_by_id(preset_id)
            .await?
            .with_context(|| format!("data_source_preset_not_found:{preset_id}"))?;
        anyhow::ensure!(!preset.built_in, "data_source_preset_builtin_immutable");
        let candidate_description = if request.keep_description {
            preset.description.clone()
        } else {
            request.description.clone()
        };
        let diff = data_source_preset_diff(
            &preset,
            candidate_description.clone(),
            request.definition.clone(),
        );
        let changed = diff.description_changed || diff.definition_changed;
        if !changed {
            return Ok(UpdateDataSourcePresetResponse {
                preset,
                affected_client_count: diff.affected_client_count,
                confirmation_required: false,
                diff,
            });
        }
        if diff.affected_client_count > 1 && !request.confirmed {
            return Ok(UpdateDataSourcePresetResponse {
                preset,
                affected_client_count: diff.affected_client_count,
                confirmation_required: true,
                diff,
            });
        }

        let updated = match self {
            Self::Memory(memory) => {
                let mut presets = memory.data_source_presets.write().await;
                let existing = presets
                    .iter_mut()
                    .find(|preset| preset.id == preset_id)
                    .with_context(|| format!("data_source_preset_not_found:{preset_id}"))?;
                anyhow::ensure!(!existing.built_in, "data_source_preset_builtin_immutable");
                existing.description = candidate_description.clone();
                existing.definition = request.definition.clone();
                existing.updated_at = unix_now().to_string();
                existing.assigned_client_count = diff.affected_client_count;
                existing.clone()
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE data_source_presets p
                    SET description = $2, definition = $3, updated_at = now()
                    WHERE p.id = $1 AND p.built_in = FALSE
                    RETURNING
                        p.id,
                        p.domain,
                        p.name,
                        p.scope,
                        p.built_in,
                        p.is_default,
                        p.owner_client_id,
                        p.description,
                        p.definition,
                        p.created_at::text AS created_at,
                        p.updated_at::text AS updated_at,
                        (
                            SELECT count(*)::bigint
                            FROM client_data_source_preset_assignments a
                            WHERE a.preset_id = p.id
                        ) AS assigned_client_count
                    "#,
                )
                .bind(preset_id)
                .bind(&candidate_description)
                .bind(sqlx::types::Json(&request.definition))
                .fetch_one(pool)
                .await?;
                data_source_preset_from_row(row)?
            }
        };
        self.record_data_source_preset_audit(
            "data_source_preset.updated",
            &format!("data_source_preset:{}", updated.id),
            Some(&updated),
            &[],
            operator,
        )
        .await?;
        Ok(UpdateDataSourcePresetResponse {
            preset: updated,
            affected_client_count: diff.affected_client_count,
            confirmation_required: false,
            diff,
        })
    }

    pub(crate) async fn list_data_source_assignments(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<DataSourcePresetAssignmentView>> {
        self.ensure_default_data_source_assignments().await?;
        match self {
            Self::Memory(memory) => {
                let mut assignments = memory
                    .data_source_assignments
                    .read()
                    .await
                    .iter()
                    .filter(|assignment| {
                        client_id.is_none_or(|client_id| assignment.client_id == client_id)
                            && domain.is_none_or(|domain| assignment.domain == domain)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                assignments.sort_by(|left, right| {
                    left.client_id
                        .cmp(&right.client_id)
                        .then_with(|| left.domain.cmp(&right.domain))
                });
                Ok(assignments)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        a.client_id,
                        a.domain,
                        a.preset_id,
                        p.name AS preset_name,
                        p.scope AS preset_scope,
                        a.assigned_at::text AS assigned_at
                    FROM client_data_source_preset_assignments a
                    JOIN data_source_presets p ON p.id = a.preset_id
                    WHERE ($1::TEXT IS NULL OR a.client_id = $1)
                      AND ($2::TEXT IS NULL OR a.domain = $2)
                    ORDER BY a.client_id, a.domain
                    "#,
                )
                .bind(client_id)
                .bind(domain)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(data_source_assignment_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn assign_data_source_preset(
        &self,
        request: &AssignDataSourcePresetRequest,
        operator: &AuthContext,
    ) -> Result<AssignDataSourcePresetResponse> {
        self.ensure_default_data_source_assignments().await?;
        let preset = self
            .data_source_preset_by_id(request.preset_id)
            .await?
            .with_context(|| format!("data_source_preset_not_found:{}", request.preset_id))?;
        anyhow::ensure!(
            preset.domain == request.domain,
            "data_source_preset_domain_mismatch"
        );

        let targets = self
            .resolve_bulk_targets(&BulkResolveRequest {
                selector_expression: request.selector_expression.clone(),
            })
            .await?
            .targets;
        anyhow::ensure!(
            !targets.is_empty(),
            "data_source_assignment_targets_required"
        );

        if preset.scope == "vps_local" {
            anyhow::ensure!(
                targets.len() == 1,
                "vps_local_preset_requires_single_target"
            );
            anyhow::ensure!(
                preset.owner_client_id.as_deref() == Some(targets[0].id.as_str()),
                "vps_local_preset_owner_mismatch"
            );
        }

        if targets.len() > 1 && !request.confirmed {
            let client_ids = targets
                .iter()
                .map(|target| target.id.clone())
                .collect::<Vec<_>>();
            let assignments = self
                .list_data_source_assignments_for_clients(&client_ids, Some(&request.domain))
                .await?;
            return Ok(AssignDataSourcePresetResponse {
                preset,
                target_count: targets.len(),
                confirmation_required: true,
                assignments,
            });
        }

        let client_ids = targets
            .iter()
            .map(|target| target.id.clone())
            .collect::<Vec<_>>();
        match self {
            Self::Memory(memory) => {
                let mut assignments = memory.data_source_assignments.write().await;
                for client_id in &client_ids {
                    let assigned_at = unix_now().to_string();
                    if let Some(existing) = assignments.iter_mut().find(|assignment| {
                        assignment.client_id == *client_id && assignment.domain == request.domain
                    }) {
                        existing.preset_id = preset.id;
                        existing.preset_name = preset.name.clone();
                        existing.preset_scope = preset.scope.clone();
                        existing.assigned_at = assigned_at;
                    } else {
                        assignments.push(DataSourcePresetAssignmentView {
                            client_id: client_id.clone(),
                            domain: request.domain.clone(),
                            preset_id: preset.id,
                            preset_name: preset.name.clone(),
                            preset_scope: preset.scope.clone(),
                            assigned_at,
                        });
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for client_id in &client_ids {
                    sqlx::query(
                        r#"
                        INSERT INTO client_data_source_preset_assignments (
                            client_id, domain, preset_id, assigned_at
                        )
                        VALUES ($1, $2, $3, now())
                        ON CONFLICT (client_id, domain) DO UPDATE SET
                            preset_id = EXCLUDED.preset_id,
                            assigned_at = now()
                        "#,
                    )
                    .bind(client_id)
                    .bind(&request.domain)
                    .bind(preset.id)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
        }

        let assignments = self
            .list_data_source_assignments_for_clients(&client_ids, Some(&request.domain))
            .await?;
        self.record_data_source_preset_audit(
            "data_source_preset.assigned",
            &format!("data_source_preset:{}", preset.id),
            Some(&preset),
            &client_ids,
            operator,
        )
        .await?;
        Ok(AssignDataSourcePresetResponse {
            preset,
            target_count: client_ids.len(),
            confirmation_required: false,
            assignments,
        })
    }

    async fn list_data_source_assignments_for_clients(
        &self,
        client_ids: &[String],
        domain: Option<&str>,
    ) -> Result<Vec<DataSourcePresetAssignmentView>> {
        let assignments = self.list_data_source_assignments(None, domain).await?;
        Ok(assignments
            .into_iter()
            .filter(|assignment| {
                client_ids
                    .iter()
                    .any(|client_id| client_id == &assignment.client_id)
            })
            .collect())
    }

    async fn ensure_builtin_data_source_presets(&self) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut presets = memory.data_source_presets.write().await;
                for built_in in builtin_presets() {
                    let id = Uuid::parse_str(built_in.id)?;
                    if presets.iter().any(|preset| preset.id == id) {
                        continue;
                    }
                    let now = unix_now().to_string();
                    presets.push(DataSourcePresetView {
                        id,
                        domain: built_in.domain.to_string(),
                        name: built_in.name.to_string(),
                        scope: "built_in".to_string(),
                        built_in: true,
                        is_default: built_in.is_default,
                        owner_client_id: None,
                        description: Some(built_in.description.to_string()),
                        definition: built_in.definition,
                        assigned_client_count: 0,
                        created_at: now.clone(),
                        updated_at: now,
                    });
                }
            }
            Self::Postgres(pool) => {
                for built_in in builtin_presets() {
                    sqlx::query(
                        r#"
                        INSERT INTO data_source_presets (
                            id, domain, name, scope, built_in, is_default,
                            description, definition
                        )
                        VALUES ($1, $2, $3, 'built_in', TRUE, $4, $5, $6)
                        ON CONFLICT (id) DO UPDATE SET
                            domain = EXCLUDED.domain,
                            name = EXCLUDED.name,
                            scope = EXCLUDED.scope,
                            built_in = TRUE,
                            is_default = EXCLUDED.is_default,
                            description = EXCLUDED.description,
                            definition = EXCLUDED.definition
                        "#,
                    )
                    .bind(Uuid::parse_str(built_in.id)?)
                    .bind(built_in.domain)
                    .bind(built_in.name)
                    .bind(built_in.is_default)
                    .bind(built_in.description)
                    .bind(sqlx::types::Json(&built_in.definition))
                    .execute(pool)
                    .await?;
                }
            }
        }
        Ok(())
    }

    async fn ensure_default_data_source_assignments(&self) -> Result<()> {
        self.ensure_builtin_data_source_presets().await?;
        match self {
            Self::Memory(memory) => {
                let agents = memory.agents.read().await.clone();
                let presets = memory.data_source_presets.read().await.clone();
                let defaults = presets
                    .iter()
                    .filter(|preset| preset.is_default)
                    .cloned()
                    .collect::<Vec<_>>();
                let mut assignments = memory.data_source_assignments.write().await;
                for agent in agents {
                    for preset in &defaults {
                        if assignments.iter().any(|assignment| {
                            assignment.client_id == agent.id && assignment.domain == preset.domain
                        }) {
                            continue;
                        }
                        assignments.push(DataSourcePresetAssignmentView {
                            client_id: agent.id.clone(),
                            domain: preset.domain.clone(),
                            preset_id: preset.id,
                            preset_name: preset.name.clone(),
                            preset_scope: preset.scope.clone(),
                            assigned_at: unix_now().to_string(),
                        });
                    }
                }
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO client_data_source_preset_assignments (client_id, domain, preset_id)
                    SELECT c.id, p.domain, p.id
                    FROM clients c
                    CROSS JOIN data_source_presets p
                    WHERE p.is_default
                    ON CONFLICT (client_id, domain) DO NOTHING
                    "#,
                )
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn data_source_preset_by_id(
        &self,
        preset_id: Uuid,
    ) -> Result<Option<DataSourcePresetView>> {
        Ok(self
            .list_data_source_presets(None)
            .await?
            .into_iter()
            .find(|preset| preset.id == preset_id))
    }

    async fn record_data_source_preset_audit(
        &self,
        action: &str,
        target: &str,
        preset: Option<&DataSourcePresetView>,
        client_ids: &[String],
        operator: &AuthContext,
    ) -> Result<()> {
        let metadata = serde_json::json!({
            "domain": preset.map(|preset| preset.domain.as_str()),
            "preset_id": preset.map(|preset| preset.id),
            "preset_name": preset.map(|preset| preset.name.as_str()),
            "preset_scope": preset.map(|preset| preset.scope.as_str()),
            "owner_client_id": preset.and_then(|preset| preset.owner_client_id.as_deref()),
            "target_clients": client_ids,
            "target_count": client_ids.len(),
        });
        let command_hash = Some(payload_hash(metadata.to_string().as_bytes()));
        match self {
            Self::Memory(memory) => {
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: action.to_string(),
                    target: target.to_string(),
                    command_hash,
                    metadata,
                    created_at: unix_now().to_string(),
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, $3, $4, $5, $6)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(action)
                .bind(target)
                .bind(&command_hash)
                .bind(sqlx::types::Json(&metadata))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}

fn data_source_preset_from_row(row: sqlx::postgres::PgRow) -> Result<DataSourcePresetView> {
    Ok(DataSourcePresetView {
        id: row.try_get("id")?,
        domain: row.try_get("domain")?,
        name: row.try_get("name")?,
        scope: row.try_get("scope")?,
        built_in: row.try_get("built_in")?,
        is_default: row.try_get("is_default")?,
        owner_client_id: row.try_get("owner_client_id")?,
        description: row.try_get("description")?,
        definition: row
            .try_get::<sqlx::types::Json<serde_json::Value>, _>("definition")?
            .0,
        assigned_client_count: row.try_get("assigned_client_count")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn data_source_assignment_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<DataSourcePresetAssignmentView> {
    Ok(DataSourcePresetAssignmentView {
        client_id: row.try_get("client_id")?,
        domain: row.try_get("domain")?,
        preset_id: row.try_get("preset_id")?,
        preset_name: row.try_get("preset_name")?,
        preset_scope: row.try_get("preset_scope")?,
        assigned_at: row.try_get("assigned_at")?,
    })
}

fn data_source_preset_key_matches(
    preset: &DataSourcePresetView,
    domain: &str,
    name: &str,
    scope: &str,
    owner_client_id: Option<&str>,
) -> bool {
    preset.domain == domain
        && preset.name == name
        && preset.scope == scope
        && preset.owner_client_id.as_deref() == owner_client_id
}

fn data_source_preset_diff(
    preset: &DataSourcePresetView,
    candidate_description: Option<String>,
    candidate_definition: serde_json::Value,
) -> DataSourcePresetDiffView {
    let changed_keys = changed_definition_keys(&preset.definition, &candidate_definition);
    let definition_changed = preset.definition != candidate_definition;
    let description_changed = preset.description != candidate_description;
    DataSourcePresetDiffView {
        preset_id: preset.id,
        domain: preset.domain.clone(),
        preset_name: preset.name.clone(),
        current_description: preset.description.clone(),
        candidate_description,
        current_definition: preset.definition.clone(),
        candidate_definition,
        description_changed,
        definition_changed,
        changed_keys,
        affected_client_count: preset.assigned_client_count,
    }
}

fn changed_definition_keys(
    current: &serde_json::Value,
    candidate: &serde_json::Value,
) -> Vec<String> {
    let (Some(current), Some(candidate)) = (current.as_object(), candidate.as_object()) else {
        return if current == candidate {
            Vec::new()
        } else {
            vec!["<definition>".to_string()]
        };
    };
    let mut keys = current
        .keys()
        .chain(candidate.keys())
        .collect::<BTreeSet<_>>();
    keys.retain(|key| current.get(*key) != candidate.get(*key));
    keys.into_iter().cloned().collect()
}
