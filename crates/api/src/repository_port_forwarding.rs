use std::{collections::HashMap, net::IpAddr};

use anyhow::{Context, Result};
use sqlx::{types::Json as SqlJson, Row};
use uuid::Uuid;
use vpsman_common::{
    port_forwarding_desired_hash, validate_port_forward_rule, validate_port_forwarding_config,
    AgentPortForwardingConfig, PortForwardMapping, PortForwardProtocol, PortForwardRule,
    PortForwardRuntimeSnapshot, PortForwardRuntimeStatus, MAX_PORT_FORWARD_RULES,
};

use crate::{
    internal_operator::persisted_actor_id,
    model::{AuditLogView, AuthContext},
    model_port_forwarding::{
        CreatePortForwardRuleRequest, PortForwardBulkAction, PortForwardBulkItem,
        PortForwardRuleRecord, PortForwardRuleView, PortForwardRuntimeRecord,
        UpdatePortForwardRuleRequest,
    },
    repository::{MemoryState, Repository},
    unix_now,
};

impl Repository {
    pub(crate) async fn list_port_forward_rules(&self) -> Result<Vec<PortForwardRuleView>> {
        let records = self.list_port_forward_rule_records(true).await?;
        let runtime = self.list_port_forward_runtime_records().await?;
        let mut expected_hashes = HashMap::new();
        for client_id in records
            .iter()
            .map(|record| record.client_id.clone())
            .collect::<std::collections::BTreeSet<_>>()
        {
            let config = config_from_records(
                &records
                    .iter()
                    .filter(|record| record.client_id == client_id)
                    .cloned()
                    .collect::<Vec<_>>(),
            )?;
            expected_hashes.insert(client_id, config.desired_hash);
        }
        let mut views = records
            .into_iter()
            .map(|record| {
                let runtime = runtime.get(&record.client_id);
                let expected_hash = expected_hashes
                    .get(&record.client_id)
                    .map(String::as_str)
                    .filter(|hash| !hash.is_empty());
                record_to_view(record, runtime, expected_hash)
            })
            .collect::<Vec<_>>();
        views.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.client_id.cmp(&right.client_id))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(views)
    }

    pub(crate) async fn get_port_forward_rule(
        &self,
        id: Uuid,
    ) -> Result<Option<PortForwardRuleView>> {
        Ok(self
            .list_port_forward_rules()
            .await?
            .into_iter()
            .find(|rule| rule.id == id))
    }

    pub(crate) async fn port_forwarding_config_for_client(
        &self,
        client_id: &str,
    ) -> Result<AgentPortForwardingConfig> {
        let records = self
            .list_port_forward_rule_records(false)
            .await?
            .into_iter()
            .filter(|record| record.client_id == client_id && record.enabled)
            .collect::<Vec<_>>();
        config_from_records(&records)
    }

    pub(crate) async fn create_port_forward_rule(
        &self,
        request: &CreatePortForwardRuleRequest,
        operator: &AuthContext,
    ) -> Result<PortForwardRuleView> {
        let now = unix_now().to_string();
        let candidate = PortForwardRuleRecord {
            id: Uuid::new_v4(),
            actor_id: persisted_actor_id(operator),
            client_id: request.client_id.clone(),
            name: request.name.trim().to_string(),
            protocol: request.protocol,
            target_ip: request.target_ip,
            mappings: request.mappings.clone(),
            masquerade: request.masquerade,
            enabled: request.enabled,
            revision: 1,
            created_at: now.clone(),
            updated_at: now,
            deleted_at: None,
            deleted_by: None,
            deleted_reason: None,
            removal_confirmed_at: None,
            forgotten_at: None,
            forgotten_by: None,
            forget_reason: None,
        };
        validate_record(&candidate)?;

        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.port_forward_lifecycle.lock().await;
                ensure_memory_port_forward_client_active(memory, &candidate.client_id).await?;
                {
                    let mut rules = memory.port_forward_rules.write().await;
                    ensure_candidate_valid(&candidate, &rules, None)?;
                    rules.push(candidate.clone());
                }
                memory.audits.write().await.push(port_forward_audit_view(
                    "network.port_forward_rule_created",
                    &candidate,
                    operator,
                ));
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_port_forward_client(&mut tx, &candidate.client_id).await?;
                ensure_postgres_port_forward_client_active(&mut tx, &candidate.client_id).await?;
                let existing =
                    select_postgres_port_forward_rules_for_client(&mut tx, &candidate.client_id)
                        .await?;
                ensure_candidate_valid(&candidate, &existing, None)?;
                sqlx::query(
                    r#"
                    INSERT INTO port_forward_rules (
                        id, actor_id, client_id, name, protocol, target_ip, mappings,
                        masquerade, enabled
                    )
                    VALUES ($1, $2, $3, $4, $5, $6::inet, $7, $8, $9)
                    "#,
                )
                .bind(candidate.id)
                .bind(candidate.actor_id)
                .bind(&candidate.client_id)
                .bind(&candidate.name)
                .bind(protocol_name(candidate.protocol))
                .bind(candidate.target_ip.to_string())
                .bind(SqlJson(&candidate.mappings))
                .bind(candidate.masquerade)
                .bind(candidate.enabled)
                .execute(&mut *tx)
                .await?;
                insert_port_forward_audit(
                    &mut tx,
                    "network.port_forward_rule_created",
                    &candidate,
                    operator,
                )
                .await?;
                tx.commit().await?;
            }
        }
        Ok(record_to_view(candidate, None, None))
    }

    pub(crate) async fn update_port_forward_rule(
        &self,
        id: Uuid,
        request: &UpdatePortForwardRuleRequest,
        operator: &AuthContext,
    ) -> Result<PortForwardRuleView> {
        let persisted = match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.port_forward_lifecycle.lock().await;
                let client_id = memory
                    .port_forward_rules
                    .read()
                    .await
                    .iter()
                    .find(|record| record.id == id && record.deleted_at.is_none())
                    .map(|record| record.client_id.clone())
                    .context("port_forward_rule_not_found")?;
                ensure_memory_port_forward_client_active(memory, &client_id).await?;
                let candidate = {
                    let mut rules = memory.port_forward_rules.write().await;
                    let index = rules
                        .iter()
                        .position(|record| record.id == id && record.deleted_at.is_none())
                        .context("port_forward_rule_not_found")?;
                    anyhow::ensure!(
                        rules[index].revision == request.expected_revision,
                        "port_forward_rule_snapshot_stale"
                    );
                    let mut candidate = rules[index].clone();
                    apply_update(&mut candidate, request, operator);
                    ensure_candidate_valid(&candidate, &rules, Some(id))?;
                    rules[index] = candidate.clone();
                    candidate
                };
                memory.audits.write().await.push(port_forward_audit_view(
                    "network.port_forward_rule_updated",
                    &candidate,
                    operator,
                ));
                candidate
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let client_id = select_postgres_port_forward_rule_client_id(&mut tx, id)
                    .await?
                    .context("port_forward_rule_not_found")?;
                lock_postgres_port_forward_client(&mut tx, &client_id).await?;
                ensure_postgres_port_forward_client_active(&mut tx, &client_id).await?;
                let current = select_postgres_port_forward_rule(&mut tx, id)
                    .await?
                    .context("port_forward_rule_not_found")?;
                anyhow::ensure!(current.deleted_at.is_none(), "port_forward_rule_not_found");
                anyhow::ensure!(
                    current.revision == request.expected_revision,
                    "port_forward_rule_snapshot_stale"
                );
                let existing =
                    select_postgres_port_forward_rules_for_client(&mut tx, &current.client_id)
                        .await?;
                let mut candidate = current;
                apply_update(&mut candidate, request, operator);
                ensure_candidate_valid(&candidate, &existing, Some(id))?;
                let result = sqlx::query(
                    r#"
                    UPDATE port_forward_rules
                    SET actor_id = $3,
                        name = $4,
                        protocol = $5,
                        target_ip = $6::inet,
                        mappings = $7,
                        masquerade = $8,
                        enabled = $9,
                        revision = revision + 1,
                        updated_at = now()
                    WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
                    "#,
                )
                .bind(id)
                .bind(request.expected_revision)
                .bind(persisted_actor_id(operator))
                .bind(&candidate.name)
                .bind(protocol_name(candidate.protocol))
                .bind(candidate.target_ip.to_string())
                .bind(SqlJson(&candidate.mappings))
                .bind(candidate.masquerade)
                .bind(candidate.enabled)
                .execute(&mut *tx)
                .await?;
                anyhow::ensure!(
                    result.rows_affected() == 1,
                    "port_forward_rule_snapshot_stale"
                );
                insert_port_forward_audit(
                    &mut tx,
                    "network.port_forward_rule_updated",
                    &candidate,
                    operator,
                )
                .await?;
                tx.commit().await?;
                candidate
            }
        };
        Ok(record_to_view(persisted, None, None))
    }

    pub(crate) async fn set_port_forward_rule_enabled(
        &self,
        id: Uuid,
        expected_revision: i64,
        enabled: bool,
        operator: &AuthContext,
    ) -> Result<PortForwardRuleView> {
        let current = self
            .get_port_forward_rule_record(id)
            .await?
            .context("port_forward_rule_not_found")?;
        let request = UpdatePortForwardRuleRequest {
            expected_revision,
            name: current.name,
            protocol: current.protocol,
            target_ip: current.target_ip,
            mappings: current.mappings,
            masquerade: current.masquerade,
            enabled,
            confirmed: true,
        };
        self.update_port_forward_rule(id, &request, operator).await
    }

    pub(crate) async fn delete_port_forward_rule(
        &self,
        id: Uuid,
        expected_revision: i64,
        reason: Option<&str>,
        operator: &AuthContext,
    ) -> Result<PortForwardRuleView> {
        let reason = normalize_reason(reason);
        let persisted = match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.port_forward_lifecycle.lock().await;
                let client_id = memory
                    .port_forward_rules
                    .read()
                    .await
                    .iter()
                    .find(|record| record.id == id && record.deleted_at.is_none())
                    .map(|record| record.client_id.clone())
                    .context("port_forward_rule_not_found")?;
                ensure_memory_port_forward_client_active(memory, &client_id).await?;
                let persisted = {
                    let mut rules = memory.port_forward_rules.write().await;
                    let record = rules
                        .iter_mut()
                        .find(|record| record.id == id && record.deleted_at.is_none())
                        .context("port_forward_rule_not_found")?;
                    anyhow::ensure!(
                        record.revision == expected_revision,
                        "port_forward_rule_snapshot_stale"
                    );
                    record.enabled = false;
                    record.revision += 1;
                    record.updated_at = unix_now().to_string();
                    record.deleted_at = Some(record.updated_at.clone());
                    record.deleted_by = persisted_actor_id(operator);
                    record.deleted_reason = reason;
                    record.removal_confirmed_at = None;
                    record.clone()
                };
                memory.audits.write().await.push(port_forward_audit_view(
                    "network.port_forward_rule_deleted",
                    &persisted,
                    operator,
                ));
                persisted
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let client_id = select_postgres_port_forward_rule_client_id(&mut tx, id)
                    .await?
                    .context("port_forward_rule_not_found")?;
                lock_postgres_port_forward_client(&mut tx, &client_id).await?;
                ensure_postgres_port_forward_client_active(&mut tx, &client_id).await?;
                let current = select_postgres_port_forward_rule(&mut tx, id)
                    .await?
                    .context("port_forward_rule_not_found")?;
                anyhow::ensure!(
                    current.revision == expected_revision,
                    "port_forward_rule_snapshot_stale"
                );
                let row = sqlx::query(
                    r#"
                    UPDATE port_forward_rules
                    SET enabled = FALSE,
                        revision = revision + 1,
                        deleted_at = now(),
                        deleted_by = $3,
                        deleted_reason = $4,
                        removal_confirmed_at = NULL,
                        updated_at = now()
                    WHERE id = $1 AND revision = $2 AND deleted_at IS NULL
                    RETURNING id, actor_id, client_id, name, protocol,
                        host(target_ip) AS target_ip, mappings, masquerade, enabled, revision,
                        created_at::text AS created_at, updated_at::text AS updated_at,
                        deleted_at::text AS deleted_at, deleted_by, deleted_reason,
                        removal_confirmed_at::text AS removal_confirmed_at,
                        forgotten_at::text AS forgotten_at, forgotten_by, forget_reason
                    "#,
                )
                .bind(id)
                .bind(expected_revision)
                .bind(persisted_actor_id(operator))
                .bind(reason)
                .fetch_optional(&mut *tx)
                .await?
                .context("port_forward_rule_snapshot_stale")?;
                let persisted = port_forward_record_from_row(&row)?;
                insert_port_forward_audit(
                    &mut tx,
                    "network.port_forward_rule_deleted",
                    &persisted,
                    operator,
                )
                .await?;
                tx.commit().await?;
                persisted
            }
        };
        Ok(record_to_view(persisted, None, None))
    }

    pub(crate) async fn forget_port_forward_rule(
        &self,
        id: Uuid,
        expected_revision: i64,
        reason: Option<&str>,
        operator: &AuthContext,
    ) -> Result<PortForwardRuleView> {
        let reason = normalize_reason(reason).context("port_forward_forget_reason_required")?;
        let persisted = match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.port_forward_lifecycle.lock().await;
                let client_id = memory
                    .port_forward_rules
                    .read()
                    .await
                    .iter()
                    .find(|record| record.id == id)
                    .map(|record| record.client_id.clone())
                    .context("port_forward_rule_not_found")?;
                ensure_memory_port_forward_client_active(memory, &client_id).await?;
                let persisted = {
                    let mut rules = memory.port_forward_rules.write().await;
                    let record = rules
                        .iter_mut()
                        .find(|record| record.id == id)
                        .context("port_forward_rule_not_found")?;
                    anyhow::ensure!(
                        record.revision == expected_revision,
                        "port_forward_rule_snapshot_stale"
                    );
                    anyhow::ensure!(
                        record.deleted_at.is_some()
                            && record.removal_confirmed_at.is_none()
                            && record.forgotten_at.is_none(),
                        "port_forward_rule_not_removal_pending"
                    );
                    record.revision += 1;
                    record.updated_at = unix_now().to_string();
                    record.forgotten_at = Some(record.updated_at.clone());
                    record.forgotten_by = persisted_actor_id(operator);
                    record.forget_reason = Some(reason);
                    record.clone()
                };
                memory.audits.write().await.push(port_forward_audit_view(
                    "network.port_forward_rule_forgotten",
                    &persisted,
                    operator,
                ));
                memory
                    .port_forward_runtime
                    .write()
                    .await
                    .remove(&persisted.client_id);
                persisted
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let client_id = select_postgres_port_forward_rule_client_id(&mut tx, id)
                    .await?
                    .context("port_forward_rule_not_found")?;
                lock_postgres_port_forward_client(&mut tx, &client_id).await?;
                ensure_postgres_port_forward_client_active(&mut tx, &client_id).await?;
                let current = select_postgres_port_forward_rule(&mut tx, id)
                    .await?
                    .context("port_forward_rule_not_found")?;
                anyhow::ensure!(
                    current.revision == expected_revision,
                    "port_forward_rule_snapshot_stale"
                );
                anyhow::ensure!(
                    current.deleted_at.is_some()
                        && current.removal_confirmed_at.is_none()
                        && current.forgotten_at.is_none(),
                    "port_forward_rule_not_removal_pending"
                );
                let row = sqlx::query(
                    r#"
                    UPDATE port_forward_rules
                    SET revision = revision + 1,
                        forgotten_at = now(),
                        forgotten_by = $3,
                        forget_reason = $4,
                        updated_at = now()
                    WHERE id = $1 AND revision = $2
                      AND deleted_at IS NOT NULL
                      AND removal_confirmed_at IS NULL
                      AND forgotten_at IS NULL
                    RETURNING id, actor_id, client_id, name, protocol,
                        host(target_ip) AS target_ip, mappings, masquerade, enabled, revision,
                        created_at::text AS created_at, updated_at::text AS updated_at,
                        deleted_at::text AS deleted_at, deleted_by, deleted_reason,
                        removal_confirmed_at::text AS removal_confirmed_at,
                        forgotten_at::text AS forgotten_at, forgotten_by, forget_reason
                    "#,
                )
                .bind(id)
                .bind(expected_revision)
                .bind(persisted_actor_id(operator))
                .bind(reason)
                .fetch_optional(&mut *tx)
                .await?
                .context("port_forward_rule_snapshot_stale")?;
                let persisted = port_forward_record_from_row(&row)?;
                sqlx::query("DELETE FROM port_forward_runtime_state WHERE client_id = $1")
                    .bind(&persisted.client_id)
                    .execute(&mut *tx)
                    .await?;
                insert_port_forward_audit(
                    &mut tx,
                    "network.port_forward_rule_forgotten",
                    &persisted,
                    operator,
                )
                .await?;
                tx.commit().await?;
                persisted
            }
        };
        Ok(record_to_view(persisted, None, None))
    }

    pub(crate) async fn bulk_mutate_port_forward_rules(
        &self,
        action: PortForwardBulkAction,
        items: &[PortForwardBulkItem],
        reason: Option<&str>,
        operator: &AuthContext,
    ) -> Result<Vec<PortForwardRuleView>> {
        anyhow::ensure!(!items.is_empty(), "port_forward_bulk_items_empty");
        anyhow::ensure!(
            items.len() <= MAX_PORT_FORWARD_RULES,
            "port_forward_bulk_too_many_items"
        );
        let mut ids = items.iter().map(|item| item.id).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        anyhow::ensure!(ids.len() == items.len(), "port_forward_bulk_duplicate_item");
        if matches!(action, PortForwardBulkAction::Reapply) {
            let records = self.list_port_forward_rule_records(true).await?;
            let selected = validate_bulk_snapshots(records, items)?;
            anyhow::ensure!(
                selected.iter().all(|record| record.deleted_at.is_none()),
                "port_forward_rule_not_active"
            );
            return Ok(selected
                .into_iter()
                .map(|record| record_to_view(record, None, None))
                .collect());
        }

        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.port_forward_lifecycle.lock().await;
                let selected =
                    validate_bulk_snapshots(memory.port_forward_rules.read().await.clone(), items)?;
                let mut client_ids = selected
                    .iter()
                    .map(|record| record.client_id.clone())
                    .collect::<Vec<_>>();
                client_ids.sort();
                client_ids.dedup();
                for client_id in &client_ids {
                    ensure_memory_port_forward_client_active(memory, client_id).await?;
                }
                let changed = {
                    let mut rules = memory.port_forward_rules.write().await;
                    let selected_ids = selected.iter().map(|record| record.id).collect::<Vec<_>>();
                    let now = unix_now().to_string();
                    let mut candidates = rules.clone();
                    for record in candidates
                        .iter_mut()
                        .filter(|record| selected_ids.contains(&record.id))
                    {
                        apply_bulk_action(record, action, &now, reason, operator)?;
                    }
                    validate_all_enabled_records(&candidates)?;
                    let changed = candidates
                        .iter()
                        .filter(|record| selected_ids.contains(&record.id))
                        .cloned()
                        .collect::<Vec<_>>();
                    *rules = candidates;
                    changed
                };
                let audit_action = bulk_audit_action(action);
                let mut audits = memory.audits.write().await;
                audits.extend(
                    changed
                        .iter()
                        .map(|record| port_forward_audit_view(audit_action, record, operator)),
                );
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let identity_rows =
                    sqlx::query("SELECT id, client_id FROM port_forward_rules WHERE id = ANY($1)")
                        .bind(&ids)
                        .fetch_all(&mut *tx)
                        .await?;
                anyhow::ensure!(
                    identity_rows.len() == ids.len(),
                    "port_forward_rule_not_found"
                );
                let mut client_ids = identity_rows
                    .iter()
                    .map(|row| row.try_get::<String, _>("client_id"))
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                client_ids.sort();
                client_ids.dedup();
                for client_id in &client_ids {
                    lock_postgres_port_forward_client(&mut tx, client_id).await?;
                    ensure_postgres_port_forward_client_active(&mut tx, client_id).await?;
                }
                let selected_rows = sqlx::query(
                    r#"
                    SELECT id, actor_id, client_id, name, protocol,
                        host(target_ip) AS target_ip, mappings, masquerade, enabled, revision,
                        created_at::text AS created_at, updated_at::text AS updated_at,
                        deleted_at::text AS deleted_at, deleted_by, deleted_reason,
                        removal_confirmed_at::text AS removal_confirmed_at,
                        forgotten_at::text AS forgotten_at, forgotten_by, forget_reason
                    FROM port_forward_rules
                    WHERE id = ANY($1)
                    FOR UPDATE
                    "#,
                )
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await?;
                let selected = selected_rows
                    .iter()
                    .map(port_forward_record_from_row)
                    .collect::<Result<Vec<_>>>()?;
                let selected = validate_bulk_snapshots(selected, items)?;
                let now = unix_now().to_string();
                let mut candidates = Vec::with_capacity(selected.len());
                for mut record in selected {
                    apply_bulk_action(&mut record, action, &now, reason, operator)?;
                    candidates.push(record);
                }
                for client_id in &client_ids {
                    let mut all =
                        select_postgres_port_forward_rules_for_client(&mut tx, client_id).await?;
                    for candidate in candidates
                        .iter()
                        .filter(|record| &record.client_id == client_id)
                    {
                        if let Some(slot) = all.iter_mut().find(|record| record.id == candidate.id)
                        {
                            *slot = candidate.clone();
                        }
                    }
                    validate_all_enabled_records(&all)?;
                }
                for candidate in &candidates {
                    let result = sqlx::query(
                        r#"
                        UPDATE port_forward_rules
                        SET actor_id = $3,
                            enabled = $4,
                            revision = revision + 1,
                            deleted_at = CASE WHEN $5 THEN now() ELSE NULL END,
                            deleted_by = $6,
                            deleted_reason = $7,
                            removal_confirmed_at = NULL,
                            updated_at = now()
                        WHERE id = $1 AND revision = $2
                        "#,
                    )
                    .bind(candidate.id)
                    .bind(candidate.revision - 1)
                    .bind(persisted_actor_id(operator))
                    .bind(candidate.enabled)
                    .bind(matches!(action, PortForwardBulkAction::Delete))
                    .bind(candidate.deleted_by)
                    .bind(&candidate.deleted_reason)
                    .execute(&mut *tx)
                    .await?;
                    anyhow::ensure!(
                        result.rows_affected() == 1,
                        "port_forward_rule_snapshot_stale"
                    );
                    insert_port_forward_audit(
                        &mut tx,
                        bulk_audit_action(action),
                        candidate,
                        operator,
                    )
                    .await?;
                }
                tx.commit().await?;
            }
        }
        let all = self.list_port_forward_rules().await?;
        Ok(all
            .into_iter()
            .filter(|rule| ids.contains(&rule.id))
            .collect())
    }

    pub(crate) async fn record_port_forward_runtime_snapshot(
        &self,
        client_id: &str,
        snapshot: &PortForwardRuntimeSnapshot,
    ) -> Result<()> {
        anyhow::ensure!(
            snapshot.rules.len() <= MAX_PORT_FORWARD_RULES,
            "port_forward_runtime_too_many_rules"
        );
        match self {
            Self::Memory(memory) => {
                let _lifecycle_guard = memory.port_forward_lifecycle.lock().await;
                if !memory_port_forward_client_active(memory, client_id).await {
                    return Ok(());
                }
                let snapshot = carry_forward_owned_table_evidence(
                    snapshot,
                    memory
                        .port_forward_runtime
                        .read()
                        .await
                        .get(client_id)
                        .map(|record| &record.snapshot),
                );
                let expected = config_from_records(
                    &memory
                        .port_forward_rules
                        .read()
                        .await
                        .iter()
                        .filter(|record| record.client_id == client_id && record.enabled)
                        .cloned()
                        .collect::<Vec<_>>(),
                )?;
                let confirms_desired = snapshot_confirms_desired(&snapshot, &expected);
                memory
                    .port_forward_runtime
                    .write()
                    .await
                    .insert(client_id.to_string(), PortForwardRuntimeRecord { snapshot });
                if confirms_desired {
                    let now = unix_now().to_string();
                    for rule in memory
                        .port_forward_rules
                        .write()
                        .await
                        .iter_mut()
                        .filter(|rule| {
                            rule.client_id == client_id
                                && rule.deleted_at.is_some()
                                && rule.removal_confirmed_at.is_none()
                                && rule.forgotten_at.is_none()
                        })
                    {
                        rule.removal_confirmed_at = Some(now.clone());
                        rule.updated_at = now.clone();
                    }
                }
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_port_forward_client(&mut tx, client_id).await?;
                if !postgres_port_forward_client_active(&mut tx, client_id).await? {
                    return Ok(());
                }
                let previous = sqlx::query_scalar::<_, SqlJson<PortForwardRuntimeSnapshot>>(
                    "SELECT snapshot FROM port_forward_runtime_state WHERE client_id = $1",
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let snapshot = carry_forward_owned_table_evidence(
                    snapshot,
                    previous.as_ref().map(|record| &record.0),
                );
                let expected = config_from_records(
                    &select_postgres_port_forward_rules_for_client(&mut tx, client_id)
                        .await?
                        .into_iter()
                        .filter(|record| record.enabled)
                        .collect::<Vec<_>>(),
                )?;
                let confirms_desired = snapshot_confirms_desired(&snapshot, &expected);
                sqlx::query(
                    r#"
                    INSERT INTO port_forward_runtime_state (client_id, snapshot, observed_at)
                    VALUES ($1, $2, to_timestamp($3::double precision))
                    ON CONFLICT (client_id) DO UPDATE
                    SET snapshot = EXCLUDED.snapshot,
                        observed_at = EXCLUDED.observed_at,
                        updated_at = now()
                    "#,
                )
                .bind(client_id)
                .bind(SqlJson(&snapshot))
                .bind(snapshot.observed_unix as f64)
                .execute(&mut *tx)
                .await?;
                if confirms_desired {
                    sqlx::query(
                        r#"
                        UPDATE port_forward_rules
                        SET removal_confirmed_at = now(), updated_at = now()
                        WHERE client_id = $1
                          AND deleted_at IS NOT NULL
                          AND removal_confirmed_at IS NULL
                          AND forgotten_at IS NULL
                        "#,
                    )
                    .bind(client_id)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn port_forwarding_blocks_agent_delete(
        &self,
        client_id: &str,
    ) -> Result<bool> {
        let records = self.list_port_forward_rule_records(true).await?;
        let desired_or_pending = records.iter().any(|record| {
            record.client_id == client_id
                && (record.enabled
                    || (record.deleted_at.is_some()
                        && record.removal_confirmed_at.is_none()
                        && record.forgotten_at.is_none()))
        });
        if desired_or_pending {
            return Ok(true);
        }
        let runtime = self.list_port_forward_runtime_records().await?;
        Ok(runtime.get(client_id).is_some_and(|runtime| {
            runtime.snapshot.owned_table_present == Some(true)
                || (runtime.snapshot.owned_table_present.is_none()
                    && matches!(
                        runtime.snapshot.status,
                        PortForwardRuntimeStatus::Applied | PortForwardRuntimeStatus::Drifted
                    )
                    && (!runtime.snapshot.rules.is_empty()
                        || runtime.snapshot.observed_hash.is_some()))
        }))
    }

    async fn get_port_forward_rule_record(
        &self,
        id: Uuid,
    ) -> Result<Option<PortForwardRuleRecord>> {
        Ok(self
            .list_port_forward_rule_records(true)
            .await?
            .into_iter()
            .find(|record| record.id == id))
    }

    async fn list_port_forward_rule_records(
        &self,
        include_removal_pending: bool,
    ) -> Result<Vec<PortForwardRuleRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .port_forward_rules
                .read()
                .await
                .iter()
                .filter(|record| {
                    record.deleted_at.is_none()
                        || (include_removal_pending
                            && record.removal_confirmed_at.is_none()
                            && record.forgotten_at.is_none())
                })
                .cloned()
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT id, actor_id, client_id, name, protocol,
                        host(target_ip) AS target_ip, mappings, masquerade, enabled, revision,
                        created_at::text AS created_at, updated_at::text AS updated_at,
                        deleted_at::text AS deleted_at, deleted_by, deleted_reason,
                        removal_confirmed_at::text AS removal_confirmed_at,
                        forgotten_at::text AS forgotten_at, forgotten_by, forget_reason
                    FROM port_forward_rules
                    WHERE deleted_at IS NULL
                       OR ($1 AND removal_confirmed_at IS NULL AND forgotten_at IS NULL)
                    ORDER BY updated_at DESC, id DESC
                    "#,
                )
                .bind(include_removal_pending)
                .fetch_all(pool)
                .await?;
                rows.iter()
                    .map(port_forward_record_from_row)
                    .collect::<Result<Vec<_>>>()
            }
        }
    }

    async fn list_port_forward_runtime_records(
        &self,
    ) -> Result<HashMap<String, PortForwardRuntimeRecord>> {
        match self {
            Self::Memory(memory) => Ok(memory.port_forward_runtime.read().await.clone()),
            Self::Postgres(pool) => {
                let rows =
                    sqlx::query("SELECT client_id, snapshot FROM port_forward_runtime_state")
                        .fetch_all(pool)
                        .await?;
                rows.into_iter()
                    .map(|row| {
                        let client_id: String = row.try_get("client_id")?;
                        let snapshot: SqlJson<PortForwardRuntimeSnapshot> =
                            row.try_get("snapshot")?;
                        Ok((
                            client_id,
                            PortForwardRuntimeRecord {
                                snapshot: snapshot.0,
                            },
                        ))
                    })
                    .collect::<std::result::Result<HashMap<_, _>, sqlx::Error>>()
                    .map_err(Into::into)
            }
        }
    }
}

fn config_from_records(records: &[PortForwardRuleRecord]) -> Result<AgentPortForwardingConfig> {
    let mut rules = records
        .iter()
        .filter(|record| record.deleted_at.is_none() && record.enabled)
        .map(runtime_rule_from_record)
        .collect::<Vec<_>>();
    rules.sort_by_key(|rule| rule.id);
    let desired_hash = if rules.is_empty() {
        String::new()
    } else {
        port_forwarding_desired_hash(&rules)
    };
    let config = AgentPortForwardingConfig {
        desired_hash,
        rules,
        ..AgentPortForwardingConfig::default()
    };
    validate_port_forwarding_config(&config)
        .map_err(|error| anyhow::anyhow!("port_forward_desired_state_invalid:{error}"))?;
    Ok(config)
}

fn snapshot_confirms_desired(
    snapshot: &PortForwardRuntimeSnapshot,
    expected: &AgentPortForwardingConfig,
) -> bool {
    match snapshot.status {
        PortForwardRuntimeStatus::Applied => {
            snapshot.desired_hash.as_deref() == Some(expected.desired_hash.as_str())
                && !expected.rules.is_empty()
        }
        PortForwardRuntimeStatus::Absent => expected.rules.is_empty(),
        _ => false,
    }
}

fn carry_forward_owned_table_evidence(
    incoming: &PortForwardRuntimeSnapshot,
    previous: Option<&PortForwardRuntimeSnapshot>,
) -> PortForwardRuntimeSnapshot {
    let mut snapshot = incoming.clone();
    if snapshot.owned_table_present.is_none() {
        snapshot.owned_table_present = previous.and_then(|record| record.owned_table_present);
    }
    snapshot
}

async fn memory_port_forward_client_active(memory: &MemoryState, client_id: &str) -> bool {
    if memory.hidden_clients.read().await.contains(client_id) {
        return false;
    }
    memory
        .agents
        .read()
        .await
        .iter()
        .any(|agent| agent.id == client_id)
}

async fn ensure_memory_port_forward_client_active(
    memory: &MemoryState,
    client_id: &str,
) -> Result<()> {
    anyhow::ensure!(
        memory_port_forward_client_active(memory, client_id).await,
        "port_forward_client_inactive"
    );
    Ok(())
}

fn ensure_candidate_valid(
    candidate: &PortForwardRuleRecord,
    existing: &[PortForwardRuleRecord],
    replacing: Option<Uuid>,
) -> Result<()> {
    validate_record(candidate)?;
    let active_count = existing
        .iter()
        .filter(|record| record.deleted_at.is_none() && Some(record.id) != replacing)
        .count()
        + usize::from(candidate.deleted_at.is_none());
    anyhow::ensure!(
        active_count <= MAX_PORT_FORWARD_RULES,
        "port_forward_rule_limit_reached"
    );
    anyhow::ensure!(
        !existing.iter().any(|record| {
            record.deleted_at.is_none()
                && Some(record.id) != replacing
                && record.client_id == candidate.client_id
                && record.name == candidate.name
        }),
        "port_forward_rule_name_conflict"
    );
    let mut all = existing
        .iter()
        .filter(|record| record.deleted_at.is_none() && Some(record.id) != replacing)
        .cloned()
        .collect::<Vec<_>>();
    if candidate.deleted_at.is_none() {
        all.push(candidate.clone());
    }
    validate_all_enabled_records(&all)
}

fn validate_all_enabled_records(records: &[PortForwardRuleRecord]) -> Result<()> {
    let mut clients = records
        .iter()
        .filter(|record| record.deleted_at.is_none() && record.enabled)
        .map(|record| record.client_id.clone())
        .collect::<Vec<_>>();
    clients.sort();
    clients.dedup();
    for client_id in clients {
        config_from_records(
            &records
                .iter()
                .filter(|record| record.client_id == client_id)
                .cloned()
                .collect::<Vec<_>>(),
        )?;
    }
    Ok(())
}

fn validate_record(record: &PortForwardRuleRecord) -> Result<()> {
    validate_port_forward_rule(&runtime_rule_from_record(record))
        .map_err(|error| anyhow::anyhow!("port_forward_rule_invalid:{error}"))
}

fn runtime_rule_from_record(record: &PortForwardRuleRecord) -> PortForwardRule {
    PortForwardRule {
        id: record.id,
        revision: record.revision,
        name: record.name.clone(),
        protocol: record.protocol,
        target_ip: record.target_ip,
        mappings: record.mappings.clone(),
        masquerade: record.masquerade,
    }
}

fn apply_update(
    record: &mut PortForwardRuleRecord,
    request: &UpdatePortForwardRuleRequest,
    operator: &AuthContext,
) {
    record.actor_id = persisted_actor_id(operator);
    record.name = request.name.trim().to_string();
    record.protocol = request.protocol;
    record.target_ip = request.target_ip;
    record.mappings = request.mappings.clone();
    record.masquerade = request.masquerade;
    record.enabled = request.enabled;
    record.revision += 1;
    record.updated_at = unix_now().to_string();
}

fn apply_bulk_action(
    record: &mut PortForwardRuleRecord,
    action: PortForwardBulkAction,
    now: &str,
    reason: Option<&str>,
    operator: &AuthContext,
) -> Result<()> {
    anyhow::ensure!(record.deleted_at.is_none(), "port_forward_rule_not_active");
    record.actor_id = persisted_actor_id(operator);
    record.revision += 1;
    record.updated_at = now.to_string();
    match action {
        PortForwardBulkAction::Enable => record.enabled = true,
        PortForwardBulkAction::Disable => record.enabled = false,
        PortForwardBulkAction::Delete => {
            record.enabled = false;
            record.deleted_at = Some(now.to_string());
            record.deleted_by = persisted_actor_id(operator);
            record.deleted_reason = normalize_reason(reason);
            record.removal_confirmed_at = None;
        }
        PortForwardBulkAction::Reapply => unreachable!("reapply does not mutate records"),
    }
    Ok(())
}

fn validate_bulk_snapshots(
    records: Vec<PortForwardRuleRecord>,
    items: &[PortForwardBulkItem],
) -> Result<Vec<PortForwardRuleRecord>> {
    let mut selected = Vec::with_capacity(items.len());
    for item in items {
        let record = records
            .iter()
            .find(|record| record.id == item.id)
            .context("port_forward_rule_not_found")?;
        anyhow::ensure!(
            record.revision == item.expected_revision,
            "port_forward_rule_snapshot_stale"
        );
        selected.push(record.clone());
    }
    Ok(selected)
}

fn record_to_view(
    record: PortForwardRuleRecord,
    runtime: Option<&PortForwardRuntimeRecord>,
    expected_hash: Option<&str>,
) -> PortForwardRuleView {
    let desired_status = if record.deleted_at.is_some() {
        "removal_pending"
    } else if record.enabled {
        "enabled"
    } else {
        "disabled"
    };
    let rule_runtime = runtime.and_then(|runtime| {
        runtime
            .snapshot
            .rules
            .iter()
            .find(|stat| stat.rule_id == record.id && stat.revision == record.revision)
    });
    let runtime_is_current =
        runtime.is_some_and(|runtime| runtime.snapshot.desired_hash.as_deref() == expected_hash);
    let forwarding_enabled = runtime.and_then(|runtime| {
        if record.target_ip.is_ipv4() {
            runtime.snapshot.ipv4_forwarding_enabled
        } else {
            runtime.snapshot.ipv6_forwarding_enabled
        }
    });
    let mut runtime_status = if record.deleted_at.is_some() {
        "removal_pending".to_string()
    } else if runtime.is_none() || !runtime_is_current {
        "pending".to_string()
    } else if !record.enabled
        && runtime.is_some_and(|runtime| {
            matches!(
                runtime.snapshot.status,
                PortForwardRuntimeStatus::Applied | PortForwardRuntimeStatus::Absent
            )
        })
    {
        "disabled".to_string()
    } else {
        runtime
            .map(|runtime| runtime_status_name(runtime.snapshot.status).to_string())
            .unwrap_or_else(|| "unknown".to_string())
    };
    if runtime_status == "applied" && forwarding_enabled == Some(false) {
        runtime_status = "applied_warning".to_string();
    }
    PortForwardRuleView {
        id: record.id,
        client_id: record.client_id,
        name: record.name,
        protocol: record.protocol,
        target_ip: record.target_ip,
        mappings: record.mappings,
        masquerade: record.masquerade,
        enabled: record.enabled,
        revision: record.revision,
        desired_status: desired_status.to_string(),
        runtime_status,
        nat_matches: rule_runtime
            .map(|stat| stat.nat_matches)
            .unwrap_or_default(),
        desired_hash: expected_hash
            .filter(|hash| !hash.is_empty())
            .map(str::to_string),
        agent_desired_hash: runtime.and_then(|runtime| runtime.snapshot.desired_hash.clone()),
        observed_hash: runtime.and_then(|runtime| runtime.snapshot.observed_hash.clone()),
        nft_version: runtime.and_then(|runtime| runtime.snapshot.nft_version.clone()),
        forwarding_enabled,
        runtime_observed_unix: runtime
            .map(|runtime| runtime.snapshot.observed_unix)
            .filter(|observed| *observed > 0),
        runtime_error_code: runtime
            .filter(|_| runtime_is_current)
            .and_then(|runtime| runtime.snapshot.error_code.clone()),
        runtime_error: runtime
            .filter(|_| runtime_is_current)
            .and_then(|runtime| runtime.snapshot.error_message.clone()),
        created_at: record.created_at,
        updated_at: record.updated_at,
        deleted_at: record.deleted_at,
        removal_confirmed_at: record.removal_confirmed_at,
        forgotten_at: record.forgotten_at,
    }
}

fn runtime_status_name(status: PortForwardRuntimeStatus) -> &'static str {
    match status {
        PortForwardRuntimeStatus::Absent => "absent",
        PortForwardRuntimeStatus::Applied => "applied",
        PortForwardRuntimeStatus::Drifted => "drifted",
        PortForwardRuntimeStatus::Unsupported => "unsupported",
        PortForwardRuntimeStatus::Failed => "failed",
        PortForwardRuntimeStatus::Unknown => "unknown",
    }
}

fn protocol_name(protocol: PortForwardProtocol) -> &'static str {
    match protocol {
        PortForwardProtocol::Tcp => "tcp",
        PortForwardProtocol::Udp => "udp",
        PortForwardProtocol::Both => "both",
    }
}

fn parse_protocol(value: &str) -> Result<PortForwardProtocol> {
    match value {
        "tcp" => Ok(PortForwardProtocol::Tcp),
        "udp" => Ok(PortForwardProtocol::Udp),
        "both" => Ok(PortForwardProtocol::Both),
        _ => anyhow::bail!("invalid persisted port-forward protocol"),
    }
}

fn normalize_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(512).collect())
}

fn bulk_audit_action(action: PortForwardBulkAction) -> &'static str {
    match action {
        PortForwardBulkAction::Enable => "network.port_forward_rule_bulk_enabled",
        PortForwardBulkAction::Disable => "network.port_forward_rule_bulk_disabled",
        PortForwardBulkAction::Delete => "network.port_forward_rule_bulk_deleted",
        PortForwardBulkAction::Reapply => "network.port_forward_rule_bulk_reapplied",
    }
}

fn port_forward_audit_view(
    action: &str,
    record: &PortForwardRuleRecord,
    operator: &AuthContext,
) -> AuditLogView {
    AuditLogView {
        id: Uuid::new_v4(),
        actor_id: persisted_actor_id(operator),
        action: action.to_string(),
        target: format!("port_forward_rule:{}", record.id),
        command_hash: None,
        metadata: port_forward_audit_metadata(record, operator),
        created_at: unix_now().to_string(),
    }
}

fn port_forward_audit_metadata(
    record: &PortForwardRuleRecord,
    operator: &AuthContext,
) -> serde_json::Value {
    serde_json::json!({
        "rule_id": record.id,
        "client_id": &record.client_id,
        "name": &record.name,
        "protocol": protocol_name(record.protocol),
        "target_ip": record.target_ip,
        "mappings": &record.mappings,
        "masquerade": record.masquerade,
        "enabled": record.enabled,
        "revision": record.revision,
        "deleted_at": &record.deleted_at,
        "deleted_reason": &record.deleted_reason,
        "forgotten_at": &record.forgotten_at,
        "forget_reason": &record.forget_reason,
        "operator_username": &operator.operator.username,
        "session_id": operator.session_id,
    })
}

async fn insert_port_forward_audit(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    action: &str,
    record: &PortForwardRuleRecord,
    operator: &AuthContext,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
        VALUES ($1, $2, $3, $4, NULL, $5)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(persisted_actor_id(operator))
    .bind(action)
    .bind(format!("port_forward_rule:{}", record.id))
    .bind(port_forward_audit_metadata(record, operator))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn port_forward_record_from_row(row: &sqlx::postgres::PgRow) -> Result<PortForwardRuleRecord> {
    let target_ip: String = row.try_get("target_ip")?;
    let mappings: SqlJson<Vec<PortForwardMapping>> = row.try_get("mappings")?;
    Ok(PortForwardRuleRecord {
        id: row.try_get("id")?,
        actor_id: row.try_get("actor_id")?,
        client_id: row.try_get("client_id")?,
        name: row.try_get("name")?,
        protocol: parse_protocol(row.try_get("protocol")?)?,
        target_ip: target_ip
            .parse::<IpAddr>()
            .context("invalid persisted target IP")?,
        mappings: mappings.0,
        masquerade: row.try_get("masquerade")?,
        enabled: row.try_get("enabled")?,
        revision: row.try_get("revision")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        deleted_at: row.try_get("deleted_at")?,
        deleted_by: row.try_get("deleted_by")?,
        deleted_reason: row.try_get("deleted_reason")?,
        removal_confirmed_at: row.try_get("removal_confirmed_at")?,
        forgotten_at: row.try_get("forgotten_at")?,
        forgotten_by: row.try_get("forgotten_by")?,
        forget_reason: row.try_get("forget_reason")?,
    })
}

pub(crate) async fn lock_postgres_port_forward_client(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(client_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn postgres_port_forwarding_blocks_agent_delete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<bool> {
    let desired_or_pending = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM port_forward_rules
            WHERE client_id = $1
              AND (
                    (deleted_at IS NULL AND enabled)
                 OR (deleted_at IS NOT NULL AND removal_confirmed_at IS NULL AND forgotten_at IS NULL)
              )
        )
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    if desired_or_pending {
        return Ok(true);
    }
    let snapshot = sqlx::query_scalar::<_, SqlJson<PortForwardRuntimeSnapshot>>(
        "SELECT snapshot FROM port_forward_runtime_state WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(snapshot.is_some_and(|snapshot| {
        snapshot.0.owned_table_present == Some(true)
            || (snapshot.0.owned_table_present.is_none()
                && matches!(
                    snapshot.0.status,
                    PortForwardRuntimeStatus::Applied | PortForwardRuntimeStatus::Drifted
                )
                && (!snapshot.0.rules.is_empty() || snapshot.0.observed_hash.is_some()))
    }))
}

pub(crate) async fn archive_postgres_port_forwarding_for_agent_delete(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    operator_id: Uuid,
    reason: Option<&str>,
) -> Result<u64> {
    let deleted_reason = reason
        .map(|reason| format!("vps_deleted: {reason}"))
        .unwrap_or_else(|| "vps_deleted".to_string());
    let result = sqlx::query(
        r#"
        UPDATE port_forward_rules
        SET enabled = FALSE,
            revision = revision + 1,
            deleted_at = now(),
            deleted_by = $2,
            deleted_reason = $3,
            removal_confirmed_at = now(),
            updated_at = now()
        WHERE client_id = $1 AND deleted_at IS NULL AND enabled = FALSE
        "#,
    )
    .bind(client_id)
    .bind(operator_id)
    .bind(deleted_reason)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM port_forward_runtime_state WHERE client_id = $1")
        .bind(client_id)
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected())
}

async fn postgres_port_forward_client_active(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM clients WHERE id = $1 AND hidden_at IS NULL)",
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?)
}

async fn ensure_postgres_port_forward_client_active(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<()> {
    anyhow::ensure!(
        postgres_port_forward_client_active(tx, client_id).await?,
        "port_forward_client_inactive"
    );
    Ok(())
}

async fn select_postgres_port_forward_rule_client_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT client_id FROM port_forward_rules WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await?,
    )
}

async fn select_postgres_port_forward_rules_for_client(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<Vec<PortForwardRuleRecord>> {
    let rows = sqlx::query(
        r#"
        SELECT id, actor_id, client_id, name, protocol,
            host(target_ip) AS target_ip, mappings, masquerade, enabled, revision,
            created_at::text AS created_at, updated_at::text AS updated_at,
            deleted_at::text AS deleted_at, deleted_by, deleted_reason,
            removal_confirmed_at::text AS removal_confirmed_at,
            forgotten_at::text AS forgotten_at, forgotten_by, forget_reason
        FROM port_forward_rules
        WHERE client_id = $1 AND deleted_at IS NULL
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(client_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.iter()
        .map(port_forward_record_from_row)
        .collect::<Result<Vec<_>>>()
}

async fn select_postgres_port_forward_rule(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<Option<PortForwardRuleRecord>> {
    let row = sqlx::query(
        r#"
        SELECT id, actor_id, client_id, name, protocol,
            host(target_ip) AS target_ip, mappings, masquerade, enabled, revision,
            created_at::text AS created_at, updated_at::text AS updated_at,
            deleted_at::text AS deleted_at, deleted_by, deleted_reason,
            removal_confirmed_at::text AS removal_confirmed_at,
            forgotten_at::text AS forgotten_at, forgotten_by, forget_reason
        FROM port_forward_rules
        WHERE id = $1
        FOR UPDATE
        "#,
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(port_forward_record_from_row).transpose()
}
