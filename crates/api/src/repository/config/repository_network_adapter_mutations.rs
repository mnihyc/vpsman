use super::*;
use crate::model::{
    NetworkAdapterAffectedResourceView, NetworkAdapterMutationResponse,
    NetworkAdapterPreviewResponse, RuntimeConfigDispatchView, UpdateNetworkAdapterMetadataRequest,
};

impl Repository {
    pub(crate) async fn preview_network_adapter_definition(
        &self,
        id: Uuid,
        candidate: &UpsertNetworkAdapterDefinitionRequest,
    ) -> Result<NetworkAdapterPreviewResponse> {
        validate_network_adapter_definition(candidate)?;
        let Self::Postgres(pool) = self;
        let mut tx = pool.begin().await?;
        let current = load_adapter(&mut tx, id, false).await?;
        let preview = preview_in_tx(&mut tx, &current, candidate).await?;
        tx.commit().await?;
        Ok(preview)
    }

    pub(crate) async fn update_network_adapter_metadata(
        &self,
        id: Uuid,
        request: &UpdateNetworkAdapterMetadataRequest,
        operator: &AuthContext,
    ) -> Result<NetworkAdapterDefinitionView> {
        validate_operator_name(&request.name, "network_adapter_name_invalid")?;
        anyhow::ensure!(
            request
                .description
                .as_deref()
                .is_none_or(|value| value.len() <= 4096),
            "network_adapter_description_invalid"
        );
        let Self::Postgres(pool) = self;
        let mut tx = pool.begin().await?;
        lock_postgres_definition_lifecycles_in_tx(&mut tx, &[format!("network-adapter:{id}")])
            .await?;
        let current = load_adapter(&mut tx, id, true).await?;
        anyhow::ensure!(
            current.updated_at == request.expected_updated_at,
            "network_adapter_review_stale"
        );
        let description = normalized_description(request.description.as_deref());
        if current.name == request.name.trim() && current.description == description {
            tx.commit().await?;
            return Ok(current);
        }
        // Deliberately omit definition: metadata cannot fire executable-change
        // propagation or accidentally overwrite commands from a stale editor.
        let row = sqlx::query(
            "UPDATE network_adapter_definitions SET name=$2, description=$3, updated_at=clock_timestamp() \
             WHERE id=$1 RETURNING id,adapter_kind,name,description,definition, \
             created_at::text AS created_at,updated_at::text AS updated_at,0::bigint AS port_forward_rule_count"
        ).bind(id).bind(request.name.trim()).bind(description)
            .fetch_one(&mut *tx).await.map_err(network_adapter_database_error)?;
        let mut saved = network_adapter_definition_from_row(row)?;
        saved.port_forward_rule_count = current.port_forward_rule_count;
        insert_configuration_audit_in_tx(
            &mut tx,
            "network_adapter_definition.metadata_updated",
            &format!("network_adapter_definition:{id}"),
            network_adapter_audit_metadata(&saved),
            operator,
        )
        .await?;
        tx.commit().await?;
        Ok(saved)
    }
}

pub(super) async fn update_reviewed_adapter(
    repo: &Repository,
    id: Uuid,
    candidate: &UpsertNetworkAdapterDefinitionRequest,
    operator: &AuthContext,
    review_hash: &str,
) -> Result<NetworkAdapterMutationResponse> {
    validate_network_adapter_definition(candidate)?;
    anyhow::ensure!(
        !review_hash.trim().is_empty(),
        "network_adapter_review_required"
    );
    let Repository::Postgres(pool) = repo;
    let mut tx = pool.begin().await?;
    lock_postgres_definition_lifecycles_in_tx(&mut tx, &[format!("network-adapter:{id}")]).await?;
    // Referencing writers take FOR SHARE on this definition before binding it.
    // Once locked, no newly accepted binding can escape this reviewed snapshot.
    let current = load_adapter(&mut tx, id, true).await?;
    lock_adapter_bindings(&mut tx, &current).await?;
    let preview = preview_in_tx(&mut tx, &current, candidate).await?;
    anyhow::ensure!(
        review_hash == preview.review_hash,
        "network_adapter_review_stale"
    );
    if preview.change_kind == "unchanged" {
        tx.commit().await?;
        return Ok(NetworkAdapterMutationResponse {
            definition: current,
            sync: Vec::new(),
        });
    }
    let row = sqlx::query(
        "UPDATE network_adapter_definitions SET name=$2, description=$3, definition=$4, updated_at=clock_timestamp() \
         WHERE id=$1 RETURNING id,adapter_kind,name,description,definition, \
         created_at::text AS created_at,updated_at::text AS updated_at,0::bigint AS port_forward_rule_count"
    ).bind(id).bind(candidate.name.trim()).bind(normalized_description(candidate.description.as_deref()))
        .bind(sqlx::types::Json(&candidate.definition)).fetch_one(&mut *tx).await
        .map_err(network_adapter_database_error)?;
    let mut saved = network_adapter_definition_from_row(row)?;
    saved.port_forward_rule_count = current.port_forward_rule_count;
    let mut audit = network_adapter_audit_metadata(&saved);
    audit["review_hash"] = serde_json::json!(preview.review_hash);
    audit["change_kind"] = serde_json::json!(preview.change_kind);
    audit["affected_resources"] = serde_json::to_value(&preview.affected_resources)?;
    audit["target_client_ids"] = serde_json::json!(preview.target_client_ids);
    insert_configuration_audit_in_tx(
        &mut tx,
        "network_adapter_definition.updated",
        &format!("network_adapter_definition:{id}"),
        audit,
        operator,
    )
    .await?;
    tx.commit().await?;
    // The source trigger owns enqueueing in the same transaction. This is a
    // queued handoff, never a claim that remote cleanup/apply already completed.
    let sync = if preview.change_kind == "commands" && saved.adapter_kind != "routing_cost" {
        preview
            .target_client_ids
            .into_iter()
            .map(|client_id| RuntimeConfigDispatchView {
                client_id,
                status: "queued".to_string(),
                job_id: None,
                error: None,
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(NetworkAdapterMutationResponse {
        definition: saved,
        sync,
    })
}

async fn load_adapter(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    exclusive: bool,
) -> Result<NetworkAdapterDefinitionView> {
    let lock = if exclusive { "FOR UPDATE" } else { "FOR SHARE" };
    let query = format!(
        "SELECT id,adapter_kind,name,description,definition,created_at::text AS created_at,updated_at::text AS updated_at, \
         (SELECT count(*) FROM (SELECT rule.id FROM port_forward_rules rule WHERE rule.adapter_definition_id=adapter.id \
          AND (rule.deleted_at IS NULL OR (rule.removal_confirmed_at IS NULL AND rule.forgotten_at IS NULL)) \
          UNION SELECT owner.rule_id FROM port_forward_adapter_owners owner WHERE owner.adapter_definition_id=adapter.id) refs) AS port_forward_rule_count \
         FROM network_adapter_definitions adapter WHERE id=$1 {lock}"
    );
    let row = sqlx::query(&query)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .context("network_adapter_definition_not_found")?;
    network_adapter_definition_from_row(row)
}

async fn preview_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    current: &NetworkAdapterDefinitionView,
    candidate: &UpsertNetworkAdapterDefinitionRequest,
) -> Result<NetworkAdapterPreviewResponse> {
    anyhow::ensure!(
        current.adapter_kind == candidate.adapter_kind,
        "network_adapter_definition_kind_immutable"
    );
    let command_change = current.definition != candidate.definition;
    anyhow::ensure!(
        !command_change
            || (current.name == candidate.name.trim()
                && current.description == normalized_description(candidate.description.as_deref())),
        "network_adapter_review_stale"
    );
    let change_kind = if command_change {
        "commands"
    } else if current.name != candidate.name.trim()
        || current.description != normalized_description(candidate.description.as_deref())
    {
        "metadata"
    } else {
        "unchanged"
    };
    let mut affected = Vec::new();
    let mut snapshots = Vec::new();
    let id_text = current.id.to_string();
    if current.adapter_kind == "runtime_tunnel" || current.adapter_kind == "routing_cost" {
        let field = if current.adapter_kind == "runtime_tunnel" {
            "runtime_control"
        } else {
            "ospf"
        };
        let rows = sqlx::query(
            "SELECT id,name,revision,enabled,left_client_id,right_client_id,plan, \
             left_ospf_job_id,right_ospf_job_id FROM tunnel_plans \
             WHERE deleted_at IS NULL AND (plan->$2->>'left_adapter_template_id'=$1 \
             OR plan->$2->>'right_adapter_template_id'=$1) ORDER BY id",
        )
        .bind(&id_text)
        .bind(field)
        .fetch_all(&mut **tx)
        .await?;
        for row in rows {
            let plan: sqlx::types::Json<Value> = row.try_get("plan")?;
            if command_change && field == "runtime_control" {
                let parsed: vpsman_common::TunnelPlan = serde_json::from_value(plan.0.clone())?;
                anyhow::ensure!(
                    parsed.runtime_control.traffic_limit.is_default()
                        || candidate.definition.get("traffic_limit_command").is_some(),
                    "network_adapter_traffic_limit_required_by_binding"
                );
            }
            let mut clients = Vec::new();
            for (side, column) in [
                ("left_adapter_template_id", "left_client_id"),
                ("right_adapter_template_id", "right_client_id"),
            ] {
                if plan.0[field][side].as_str() == Some(id_text.as_str()) {
                    clients.push(row.try_get::<String, _>(column)?);
                }
            }
            clients.sort();
            clients.dedup();
            let resource = NetworkAdapterAffectedResourceView {
                kind: current.adapter_kind.clone(),
                resource_id: row.try_get("id")?,
                resource_name: row.try_get("name")?,
                client_ids: clients,
                enabled: row.try_get("enabled")?,
                cleanup_pending: false,
            };
            snapshots.push(serde_json::json!({"resource": &resource, "revision": row.try_get::<i64,_>("revision")?,
                "plan": plan.0, "left_ospf_job_id": row.try_get::<Option<Uuid>,_>("left_ospf_job_id")?,
                "right_ospf_job_id": row.try_get::<Option<Uuid>,_>("right_ospf_job_id")?}));
            affected.push(resource);
        }
    } else {
        let rows = sqlx::query(
            "SELECT rule.id,rule.name,rule.client_id,rule.enabled,rule.deleted_at IS NOT NULL AS deleted, \
             rule.adapter_definition_id, to_jsonb(rule) AS snapshot, \
             EXISTS(SELECT 1 FROM port_forward_adapter_owners owner WHERE owner.rule_id=rule.id AND owner.adapter_definition_id=$1) AS owned \
             FROM port_forward_rules rule WHERE \
             (rule.adapter_definition_id=$1 AND (rule.deleted_at IS NULL OR (rule.removal_confirmed_at IS NULL AND rule.forgotten_at IS NULL))) \
             OR EXISTS(SELECT 1 FROM port_forward_adapter_owners owner WHERE owner.rule_id=rule.id AND owner.adapter_definition_id=$1) ORDER BY rule.id"
        ).bind(current.id).fetch_all(&mut **tx).await?;
        for row in rows {
            let enabled = row.try_get::<bool, _>("enabled")?
                && !row.try_get::<bool, _>("deleted")?
                && row.try_get::<Option<Uuid>, _>("adapter_definition_id")? == Some(current.id);
            let owned: bool = row.try_get("owned")?;
            let resource = NetworkAdapterAffectedResourceView {
                kind: current.adapter_kind.clone(),
                resource_id: row.try_get("id")?,
                resource_name: row.try_get("name")?,
                client_ids: vec![row.try_get("client_id")?],
                enabled,
                cleanup_pending: owned && !enabled,
            };
            let snapshot: sqlx::types::Json<Value> = row.try_get("snapshot")?;
            snapshots
                .push(serde_json::json!({"resource": &resource,"rule":snapshot.0,"owned":owned}));
            affected.push(resource);
        }
    }
    let targets = affected
        .iter()
        .filter(|resource| resource.enabled || resource.cleanup_pending)
        .flat_map(|resource| resource.client_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    let review_hash = payload_hash(&serde_json::to_vec(&serde_json::json!({
        "adapter": {"id": current.id,"kind":current.adapter_kind,"name":current.name,
            "description":current.description,"definition":current.definition,"updated_at":current.updated_at},
        "candidate": {"kind":candidate.adapter_kind,"name":candidate.name.trim(),
            "description":normalized_description(candidate.description.as_deref()),"definition":candidate.definition},
        "bindings":snapshots,
    }))?);
    Ok(NetworkAdapterPreviewResponse {
        review_hash,
        change_kind: change_kind.to_string(),
        affected_resources: affected,
        target_client_ids: targets.into_iter().collect(),
    })
}

async fn lock_adapter_bindings(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    current: &NetworkAdapterDefinitionView,
) -> Result<()> {
    // Existing bindings can change without binding a new adapter. Exact row
    // locks fence those writers too. Do not wait while holding the adapter:
    // some existing writers acquire their resource before its adapter share.
    let result = if current.adapter_kind == "port_forward" {
        sqlx::query(
            "SELECT id FROM port_forward_rules rule WHERE adapter_definition_id=$1 \
             OR EXISTS(SELECT 1 FROM port_forward_adapter_owners owner WHERE owner.rule_id=rule.id AND owner.adapter_definition_id=$1) \
             ORDER BY id FOR UPDATE OF rule NOWAIT"
        ).bind(current.id).fetch_all(&mut **tx).await
    } else {
        let field = if current.adapter_kind == "runtime_tunnel" {
            "runtime_control"
        } else {
            "ospf"
        };
        sqlx::query(
            "SELECT id FROM tunnel_plans WHERE deleted_at IS NULL AND \
             (plan->$2->>'left_adapter_template_id'=$1 OR plan->$2->>'right_adapter_template_id'=$1) \
             ORDER BY id FOR UPDATE NOWAIT"
        ).bind(current.id.to_string()).bind(field).fetch_all(&mut **tx).await
    };
    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("55P03") => {
            anyhow::bail!("network_adapter_review_stale")
        }
        Err(error) => Err(error.into()),
    }
}
