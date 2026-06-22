use std::collections::BTreeSet;

use anyhow::{Context, Result};
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::payload_hash;

use crate::{
    model::{
        AssignSourceTemplateRequest, AssignSourceTemplateResponse, AuditLogView, AuthContext,
        CloneSourceTemplateRequest, CreateSourceTemplateRequest, SourceTemplateAssignmentView,
        SourceTemplateDiffRequest, SourceTemplateDiffView, SourceTemplateTestView,
        SourceTemplateView, TestSourceTemplateRequest, UpdateSourceTemplateRequest,
        UpdateSourceTemplateResponse,
    },
    repository::Repository,
    repository_source_config_patch::render_source_template_candidate,
    source_template_builtins::builtin_source_templates,
    unix_now,
};

impl Repository {
    pub(crate) async fn list_source_templates(
        &self,
        domain: Option<&str>,
    ) -> Result<Vec<SourceTemplateView>> {
        self.ensure_builtin_source_templates().await?;
        match self {
            Self::Memory(memory) => {
                let assignments = self
                    .effective_source_template_assignments(None, domain)
                    .await?;
                let mut templates = memory
                    .source_templates
                    .read()
                    .await
                    .iter()
                    .filter(|template| domain.is_none_or(|domain| template.domain == domain))
                    .cloned()
                    .map(|mut template| {
                        template.assigned_client_count = assignments
                            .iter()
                            .filter(|assignment| assignment.template_id == template.id)
                            .count()
                            as i64;
                        template
                    })
                    .collect::<Vec<_>>();
                templates.sort_by(|left, right| {
                    left.domain
                        .cmp(&right.domain)
                        .then_with(|| right.is_default.cmp(&left.is_default))
                        .then_with(|| left.scope.cmp(&right.scope))
                        .then_with(|| left.name.cmp(&right.name))
                });
                Ok(templates)
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH visible_clients AS (
                        SELECT id
                        FROM clients
                        WHERE hidden_at IS NULL
                          AND status NOT IN ('deleted', 'revoked')
                    ),
                    effective_assignments AS (
                        SELECT a.client_id, a.domain, a.template_id
                        FROM client_source_template_assignments a
                        JOIN visible_clients c ON c.id = a.client_id
                        UNION ALL
                        SELECT c.id AS client_id, p.domain, p.id AS template_id
                        FROM visible_clients c
                        CROSS JOIN source_templates p
                        WHERE p.is_default
                          AND NOT EXISTS (
                              SELECT 1
                              FROM client_source_template_assignments a
                              WHERE a.client_id = c.id
                                AND a.domain = p.domain
                          )
                    )
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
                        count(e.client_id)::bigint AS assigned_client_count
                    FROM source_templates p
                    LEFT JOIN effective_assignments e ON e.template_id = p.id
                    WHERE $1::TEXT IS NULL OR p.domain = $1
                    GROUP BY p.id
                    ORDER BY p.domain, p.is_default DESC, p.scope, p.name
                    "#,
                )
                .bind(domain)
                .fetch_all(pool)
                .await?;
                rows.into_iter().map(source_template_from_row).collect()
            }
        }
    }

    pub(crate) async fn create_source_template(
        &self,
        request: &CreateSourceTemplateRequest,
        operator: &AuthContext,
    ) -> Result<SourceTemplateView> {
        self.ensure_builtin_source_templates().await?;
        let now = unix_now().to_string();
        let scope = request.scope.trim();
        let owner = request.owner_client_id.as_deref().map(str::trim);
        let template = match self {
            Self::Memory(memory) => {
                let mut templates = memory.source_templates.write().await;
                if templates.iter().any(|template| {
                    template.domain == request.domain
                        && template.name == request.name
                        && template.scope == scope
                        && template.owner_client_id.as_deref() == owner
                }) {
                    anyhow::bail!("source_template_duplicate");
                } else {
                    let template = SourceTemplateView {
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
                    templates.push(template.clone());
                    template
                }
            }
            Self::Postgres(pool) => {
                let existing_id = if let Some(owner) = owner {
                    sqlx::query(
                        r#"
                        SELECT id FROM source_templates
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
                        SELECT id FROM source_templates
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

                anyhow::ensure!(existing_id.is_none(), "source_template_duplicate");
                let row = sqlx::query(
                    r#"
                    INSERT INTO source_templates (
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
                .await?;
                source_template_from_row(row)?
            }
        };
        self.record_source_template_audit(
            "source_template.saved",
            &format!("source_template:{}", template.id),
            Some(&template),
            &[],
            operator,
        )
        .await?;
        Ok(template)
    }

    pub(crate) async fn clone_source_template(
        &self,
        source_template_id: Uuid,
        request: &CloneSourceTemplateRequest,
        operator: &AuthContext,
    ) -> Result<SourceTemplateView> {
        self.ensure_builtin_source_templates().await?;
        let source = self
            .source_template_by_id(source_template_id)
            .await?
            .with_context(|| format!("source_template_not_found:{source_template_id}"))?;
        let now = unix_now().to_string();
        let scope = request.scope.trim();
        let owner = request.owner_client_id.as_deref().map(str::trim);
        let description = request.description.clone().or(source.description.clone());
        let template = match self {
            Self::Memory(memory) => {
                let mut templates = memory.source_templates.write().await;
                anyhow::ensure!(
                    !templates.iter().any(|template| source_template_key_matches(
                        template,
                        &source.domain,
                        &request.name,
                        scope,
                        owner
                    )),
                    "source_template_clone_target_exists"
                );
                let template = SourceTemplateView {
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
                templates.push(template.clone());
                template
            }
            Self::Postgres(pool) => {
                let exists = if let Some(owner) = owner {
                    sqlx::query(
                        r#"
                        SELECT id FROM source_templates
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
                        SELECT id FROM source_templates
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
                anyhow::ensure!(!exists, "source_template_clone_target_exists");
                let row = sqlx::query(
                    r#"
                    INSERT INTO source_templates (
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
                source_template_from_row(row)?
            }
        };
        self.record_source_template_audit(
            "source_template.cloned",
            &format!("source_template:{}", template.id),
            Some(&template),
            &[],
            operator,
        )
        .await?;
        Ok(template)
    }

    pub(crate) async fn diff_source_template(
        &self,
        template_id: Uuid,
        request: &SourceTemplateDiffRequest,
    ) -> Result<SourceTemplateDiffView> {
        let template = self
            .source_template_by_id(template_id)
            .await?
            .with_context(|| format!("source_template_not_found:{template_id}"))?;
        let candidate_description = if request.keep_description {
            template.description.clone()
        } else {
            request.description.clone()
        };
        Ok(source_template_diff(
            &template,
            candidate_description,
            request.definition.clone(),
        ))
    }

    pub(crate) async fn test_source_template(
        &self,
        template_id: Uuid,
        request: &TestSourceTemplateRequest,
    ) -> Result<SourceTemplateTestView> {
        let template = self
            .source_template_by_id(template_id)
            .await?
            .with_context(|| format!("source_template_not_found:{template_id}"))?;
        let mut candidate = template.clone();
        candidate.definition = request.definition.clone();
        let (valid, renderable, error, sections, toml, unsupported_domains, render_notes) =
            match render_source_template_candidate(&candidate) {
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
        Ok(SourceTemplateTestView {
            template_id: template.id,
            domain: template.domain,
            template_name: template.name,
            affected_client_count: template.assigned_client_count,
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

    pub(crate) async fn update_source_template(
        &self,
        template_id: Uuid,
        request: &UpdateSourceTemplateRequest,
        operator: &AuthContext,
    ) -> Result<UpdateSourceTemplateResponse> {
        self.ensure_builtin_source_templates().await?;
        let template = self
            .source_template_by_id(template_id)
            .await?
            .with_context(|| format!("source_template_not_found:{template_id}"))?;
        anyhow::ensure!(!template.built_in, "source_template_builtin_immutable");
        let candidate_description = if request.keep_description {
            template.description.clone()
        } else {
            request.description.clone()
        };
        let diff = source_template_diff(
            &template,
            candidate_description.clone(),
            request.definition.clone(),
        );
        let changed = diff.description_changed || diff.definition_changed;
        if !changed {
            return Ok(UpdateSourceTemplateResponse {
                template,
                affected_client_count: diff.affected_client_count,
                confirmation_required: false,
                diff,
            });
        }
        if !request.confirmed {
            return Ok(UpdateSourceTemplateResponse {
                template,
                affected_client_count: diff.affected_client_count,
                confirmation_required: true,
                diff,
            });
        }

        let updated = match self {
            Self::Memory(memory) => {
                let mut templates = memory.source_templates.write().await;
                let existing = templates
                    .iter_mut()
                    .find(|template| template.id == template_id)
                    .with_context(|| format!("source_template_not_found:{template_id}"))?;
                anyhow::ensure!(!existing.built_in, "source_template_builtin_immutable");
                existing.description = candidate_description.clone();
                existing.definition = request.definition.clone();
                existing.updated_at = unix_now().to_string();
                existing.assigned_client_count = diff.affected_client_count;
                existing.clone()
            }
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    UPDATE source_templates p
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
                            FROM client_source_template_assignments a
                            WHERE a.template_id = p.id
                        ) AS assigned_client_count
                    "#,
                )
                .bind(template_id)
                .bind(&candidate_description)
                .bind(sqlx::types::Json(&request.definition))
                .fetch_one(pool)
                .await?;
                source_template_from_row(row)?
            }
        };
        self.record_source_template_audit(
            "source_template.updated",
            &format!("source_template:{}", updated.id),
            Some(&updated),
            &[],
            operator,
        )
        .await?;
        Ok(UpdateSourceTemplateResponse {
            template: updated,
            affected_client_count: diff.affected_client_count,
            confirmation_required: false,
            diff,
        })
    }

    pub(crate) async fn list_source_template_assignments(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<SourceTemplateAssignmentView>> {
        self.ensure_builtin_source_templates().await?;
        self.effective_source_template_assignments(client_id, domain)
            .await
    }

    async fn effective_source_template_assignments(
        &self,
        client_id: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Vec<SourceTemplateAssignmentView>> {
        match self {
            Self::Memory(memory) => {
                let hidden = memory.hidden_clients.read().await;
                let visible_clients = memory
                    .agents
                    .read()
                    .await
                    .iter()
                    .filter(|agent| {
                        !hidden.contains(&agent.id)
                            && agent.status != "deleted"
                            && agent.status != "revoked"
                            && client_id.is_none_or(|client_id| agent.id == client_id)
                    })
                    .map(|agent| agent.id.clone())
                    .collect::<BTreeSet<_>>();
                let templates = memory.source_templates.read().await.clone();
                let explicit = memory
                    .source_template_assignments
                    .read()
                    .await
                    .iter()
                    .filter(|assignment| {
                        visible_clients.contains(&assignment.client_id)
                            && domain.is_none_or(|domain| assignment.domain == domain)
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                let mut assignments = explicit.clone();
                for client_id in &visible_clients {
                    for template in templates
                        .iter()
                        .filter(|template| template.is_default)
                        .filter(|template| domain.is_none_or(|domain| template.domain == domain))
                    {
                        if explicit.iter().any(|assignment| {
                            assignment.client_id == *client_id
                                && assignment.domain == template.domain
                        }) {
                            continue;
                        }
                        assignments.push(SourceTemplateAssignmentView {
                            client_id: client_id.clone(),
                            domain: template.domain.clone(),
                            template_id: template.id,
                            template_name: template.name.clone(),
                            template_scope: template.scope.clone(),
                            assigned_at: template.created_at.clone(),
                        });
                    }
                }
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
                    WITH visible_clients AS (
                        SELECT id
                        FROM clients
                        WHERE hidden_at IS NULL
                          AND status NOT IN ('deleted', 'revoked')
                          AND ($1::TEXT IS NULL OR id = $1)
                    ),
                    explicit AS (
                        SELECT
                            a.client_id,
                            a.domain,
                            a.template_id,
                            p.name AS template_name,
                            p.scope AS template_scope,
                            a.assigned_at::text AS assigned_at
                        FROM client_source_template_assignments a
                        JOIN visible_clients c ON c.id = a.client_id
                        JOIN source_templates p ON p.id = a.template_id
                        WHERE $2::TEXT IS NULL OR a.domain = $2
                    ),
                    effective_defaults AS (
                        SELECT
                            c.id AS client_id,
                            p.domain,
                            p.id AS template_id,
                            p.name AS template_name,
                            p.scope AS template_scope,
                            p.created_at::text AS assigned_at
                        FROM visible_clients c
                        CROSS JOIN source_templates p
                        WHERE p.is_default
                          AND ($2::TEXT IS NULL OR p.domain = $2)
                          AND NOT EXISTS (
                              SELECT 1
                              FROM client_source_template_assignments a
                              WHERE a.client_id = c.id
                                AND a.domain = p.domain
                          )
                    )
                    SELECT
                        client_id,
                        domain,
                        template_id,
                        template_name,
                        template_scope,
                        assigned_at
                    FROM explicit
                    UNION ALL
                    SELECT
                        client_id,
                        domain,
                        template_id,
                        template_name,
                        template_scope,
                        assigned_at
                    FROM effective_defaults
                    ORDER BY client_id, domain
                    "#,
                )
                .bind(client_id)
                .bind(domain)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(source_template_assignment_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn assign_source_template(
        &self,
        request: &AssignSourceTemplateRequest,
        operator: &AuthContext,
    ) -> Result<AssignSourceTemplateResponse> {
        self.ensure_builtin_source_templates().await?;
        let template = self
            .source_template_by_id(request.template_id)
            .await?
            .with_context(|| format!("source_template_not_found:{}", request.template_id))?;
        anyhow::ensure!(
            template.domain == request.domain,
            "source_template_domain_mismatch"
        );

        let targets = self.fixed_target_agents(&request.target_client_ids).await?;
        anyhow::ensure!(
            !targets.is_empty(),
            "source_template_assignment_targets_required"
        );

        if template.scope == "vps_local" {
            anyhow::ensure!(
                targets.len() == 1,
                "vps_local_template_requires_single_target"
            );
            anyhow::ensure!(
                template.owner_client_id.as_deref() == Some(targets[0].id.as_str()),
                "vps_local_template_owner_mismatch"
            );
        }

        if !request.confirmed {
            let client_ids = targets
                .iter()
                .map(|target| target.id.clone())
                .collect::<Vec<_>>();
            let assignments = self
                .list_source_template_assignments_for_clients(&client_ids, Some(&request.domain))
                .await?;
            return Ok(AssignSourceTemplateResponse {
                template,
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
                let mut assignments = memory.source_template_assignments.write().await;
                for client_id in &client_ids {
                    let assigned_at = unix_now().to_string();
                    if let Some(existing) = assignments.iter_mut().find(|assignment| {
                        assignment.client_id == *client_id && assignment.domain == request.domain
                    }) {
                        existing.template_id = template.id;
                        existing.template_name = template.name.clone();
                        existing.template_scope = template.scope.clone();
                        existing.assigned_at = assigned_at;
                    } else {
                        assignments.push(SourceTemplateAssignmentView {
                            client_id: client_id.clone(),
                            domain: request.domain.clone(),
                            template_id: template.id,
                            template_name: template.name.clone(),
                            template_scope: template.scope.clone(),
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
                        INSERT INTO client_source_template_assignments (
                            client_id, domain, template_id, assigned_at
                        )
                        VALUES ($1, $2, $3, now())
                        ON CONFLICT (client_id, domain) DO UPDATE SET
                            template_id = EXCLUDED.template_id,
                            assigned_at = now()
                        "#,
                    )
                    .bind(client_id)
                    .bind(&request.domain)
                    .bind(template.id)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
        }

        let assignments = self
            .list_source_template_assignments_for_clients(&client_ids, Some(&request.domain))
            .await?;
        self.record_source_template_audit(
            "source_template.assigned",
            &format!("source_template:{}", template.id),
            Some(&template),
            &client_ids,
            operator,
        )
        .await?;
        Ok(AssignSourceTemplateResponse {
            template,
            target_count: client_ids.len(),
            confirmation_required: false,
            assignments,
        })
    }

    async fn list_source_template_assignments_for_clients(
        &self,
        client_ids: &[String],
        domain: Option<&str>,
    ) -> Result<Vec<SourceTemplateAssignmentView>> {
        let assignments = self.list_source_template_assignments(None, domain).await?;
        Ok(assignments
            .into_iter()
            .filter(|assignment| {
                client_ids
                    .iter()
                    .any(|client_id| client_id == &assignment.client_id)
            })
            .collect())
    }

    async fn ensure_builtin_source_templates(&self) -> Result<()> {
        match self {
            Self::Memory(memory) => {
                let mut templates = memory.source_templates.write().await;
                for built_in in builtin_source_templates() {
                    let id = Uuid::parse_str(built_in.id)?;
                    if templates.iter().any(|template| template.id == id) {
                        continue;
                    }
                    let now = unix_now().to_string();
                    templates.push(SourceTemplateView {
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
                for built_in in builtin_source_templates() {
                    sqlx::query(
                        r#"
                        INSERT INTO source_templates (
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

    async fn source_template_by_id(&self, template_id: Uuid) -> Result<Option<SourceTemplateView>> {
        Ok(self
            .list_source_templates(None)
            .await?
            .into_iter()
            .find(|template| template.id == template_id))
    }

    async fn record_source_template_audit(
        &self,
        action: &str,
        target: &str,
        template: Option<&SourceTemplateView>,
        client_ids: &[String],
        operator: &AuthContext,
    ) -> Result<()> {
        let metadata = serde_json::json!({
            "domain": template.map(|template| template.domain.as_str()),
            "template_id": template.map(|template| template.id),
            "template_name": template.map(|template| template.name.as_str()),
            "template_scope": template.map(|template| template.scope.as_str()),
            "owner_client_id": template.and_then(|template| template.owner_client_id.as_deref()),
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

fn source_template_from_row(row: sqlx::postgres::PgRow) -> Result<SourceTemplateView> {
    Ok(SourceTemplateView {
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

fn source_template_assignment_from_row(
    row: sqlx::postgres::PgRow,
) -> Result<SourceTemplateAssignmentView> {
    Ok(SourceTemplateAssignmentView {
        client_id: row.try_get("client_id")?,
        domain: row.try_get("domain")?,
        template_id: row.try_get("template_id")?,
        template_name: row.try_get("template_name")?,
        template_scope: row.try_get("template_scope")?,
        assigned_at: row.try_get("assigned_at")?,
    })
}

fn source_template_key_matches(
    template: &SourceTemplateView,
    domain: &str,
    name: &str,
    scope: &str,
    owner_client_id: Option<&str>,
) -> bool {
    template.domain == domain
        && template.name == name
        && template.scope == scope
        && template.owner_client_id.as_deref() == owner_client_id
}

fn source_template_diff(
    template: &SourceTemplateView,
    candidate_description: Option<String>,
    candidate_definition: serde_json::Value,
) -> SourceTemplateDiffView {
    let changed_keys = changed_definition_keys(&template.definition, &candidate_definition);
    let definition_changed = template.definition != candidate_definition;
    let description_changed = template.description != candidate_description;
    SourceTemplateDiffView {
        template_id: template.id,
        domain: template.domain.clone(),
        template_name: template.name.clone(),
        current_description: template.description.clone(),
        candidate_description,
        current_definition: template.definition.clone(),
        candidate_definition,
        description_changed,
        definition_changed,
        changed_keys,
        affected_client_count: template.assigned_client_count,
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
