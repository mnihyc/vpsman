use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::{Context, Result};
use serde_json::json;
use sqlx::Row;
use uuid::Uuid;
use vpsman_common::{encode_json, payload_hash, JobCommand};

use crate::{
    job_request::validate_unsigned_command_envelope,
    model::{
        AgentUpdateActivationDelegationClaim, AgentUpdateActivationDelegationRequest,
        AgentUpdateActivationDelegationView, AgentUpdateRollbackDelegatedProofRecord,
        AgentUpdateRollbackDelegationClaim, AgentUpdateRollbackDelegationRequest,
        AgentUpdateRollbackDelegationView, AgentUpdateRolloutView, AuditLogView, AuthContext,
    },
    repository::Repository,
    repository_rollouts::{ROLLOUT_DELEGATED_ACTION_ACTIVATE, ROLLOUT_DELEGATED_ACTION_ROLLBACK},
};

fn agent_update_rollback_command_hash(rollback_sha256_hex: Option<&str>) -> Result<String> {
    let operation = JobCommand::AgentUpdateRollback {
        rollback_sha256_hex: rollback_sha256_hex.map(str::to_string),
    };
    Ok(payload_hash(&encode_json(&operation)?))
}

fn agent_update_activation_command_hash(
    staged_sha256_hex: &str,
    restart_agent: bool,
) -> Result<String> {
    let operation = JobCommand::AgentUpdateActivate {
        staged_sha256_hex: staged_sha256_hex.to_string(),
        restart_agent,
    };
    Ok(payload_hash(&encode_json(&operation)?))
}

struct RollbackDelegationMetadata<'a> {
    rollout_id: Uuid,
    operator: &'a AuthContext,
    command_hash: &'a str,
    clients: &'a [String],
    rollback_sha256_hex: Option<&'a str>,
    force_unprivileged: bool,
    proof_expires_unix_min: Option<i64>,
    proof_expires_unix_max: Option<i64>,
}

fn rollback_delegation_recorded_metadata(
    input: RollbackDelegationMetadata<'_>,
) -> serde_json::Value {
    json!({
        "rollout_id": input.rollout_id,
        "operator_id": input.operator.operator.id,
        "action": ROLLOUT_DELEGATED_ACTION_ROLLBACK,
        "payload_hash": input.command_hash,
        "target_count": input.clients.len(),
        "clients": input.clients,
        "rollback_sha256_hex": input.rollback_sha256_hex.map(str::to_ascii_lowercase),
        "force_unprivileged": input.force_unprivileged,
        "proof_expires_unix_min": input.proof_expires_unix_min,
        "proof_expires_unix_max": input.proof_expires_unix_max,
        "delegation": "scoped_exact_rollback_proof_escrow",
    })
}

struct ActivationDelegationMetadata<'a> {
    rollout_id: Uuid,
    operator: &'a AuthContext,
    command_hash: &'a str,
    clients: &'a [String],
    staged_sha256_hex: &'a str,
    restart_agent: bool,
    force_unprivileged: bool,
    proof_expires_unix_min: Option<i64>,
    proof_expires_unix_max: Option<i64>,
}

fn activation_delegation_recorded_metadata(
    input: ActivationDelegationMetadata<'_>,
) -> serde_json::Value {
    json!({
        "rollout_id": input.rollout_id,
        "operator_id": input.operator.operator.id,
        "action": ROLLOUT_DELEGATED_ACTION_ACTIVATE,
        "payload_hash": input.command_hash,
        "target_count": input.clients.len(),
        "clients": input.clients,
        "staged_sha256_hex": input.staged_sha256_hex.to_ascii_lowercase(),
        "restart_agent": input.restart_agent,
        "force_unprivileged": input.force_unprivileged,
        "proof_expires_unix_min": input.proof_expires_unix_min,
        "proof_expires_unix_max": input.proof_expires_unix_max,
        "delegation": "scoped_exact_activation_proof_escrow",
    })
}

fn rollback_delegation_status_metadata(
    rollout_id: Uuid,
    delegation_ids: &[Uuid],
    job_id: Option<Uuid>,
    status: &str,
    reason: Option<&str>,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout_id,
        "delegation_count": delegation_ids.len(),
        "delegation_ids": delegation_ids,
        "dispatch_job_id": job_id,
        "status": status,
        "reason": reason,
    })
}

fn activation_delegation_status_metadata(
    rollout_id: Uuid,
    delegation_ids: &[Uuid],
    job_id: Option<Uuid>,
    status: &str,
    reason: Option<&str>,
) -> serde_json::Value {
    json!({
        "rollout_id": rollout_id,
        "delegation_count": delegation_ids.len(),
        "delegation_ids": delegation_ids,
        "dispatch_job_id": job_id,
        "status": status,
        "reason": reason,
    })
}

fn delegated_proof_expired_metadata(
    records: &[AgentUpdateRollbackDelegatedProofRecord],
) -> serde_json::Value {
    let delegation_ids = records.iter().map(|record| record.id).collect::<Vec<_>>();
    let rollouts = records
        .iter()
        .map(|record| record.rollout_id)
        .collect::<BTreeSet<_>>();
    let actions = records
        .iter()
        .map(|record| record.action.as_str())
        .collect::<BTreeSet<_>>();
    json!({
        "delegation_count": records.len(),
        "delegation_ids": delegation_ids,
        "rollout_ids": rollouts,
        "actions": actions,
        "status": "expired",
        "reason": "proof_expires_unix elapsed before dispatch claim",
    })
}

fn build_rollback_delegation_summary(
    rollout_id: Uuid,
    rows: Vec<&AgentUpdateRollbackDelegatedProofRecord>,
    now_unix: i64,
) -> Option<AgentUpdateRollbackDelegationView> {
    if rows.is_empty() {
        return None;
    }
    let first = rows[0];
    let ready_count = rows
        .iter()
        .filter(|record| record.status == "ready" && record.proof_expires_unix >= now_unix)
        .count() as i32;
    let dispatching_count = rows
        .iter()
        .filter(|record| record.status == "dispatching")
        .count() as i32;
    let dispatched_count = rows
        .iter()
        .filter(|record| record.status == "dispatched")
        .count() as i32;
    let failed_count = rows
        .iter()
        .filter(|record| record.status == "failed")
        .count() as i32;
    let expired_count = rows
        .iter()
        .filter(|record| record.status == "expired" || record.proof_expires_unix < now_unix)
        .count() as i32;
    Some(AgentUpdateRollbackDelegationView {
        rollout_id,
        action: ROLLOUT_DELEGATED_ACTION_ROLLBACK.to_string(),
        rollback_sha256_hex: first.rollback_sha256_hex.clone(),
        force_unprivileged: first.force_unprivileged,
        payload_hash: first.payload_hash.clone(),
        target_count: rows.len() as i32,
        ready_count,
        dispatching_count,
        dispatched_count,
        expired_count,
        failed_count,
        proof_expires_unix_min: rows.iter().map(|record| record.proof_expires_unix).min(),
        proof_expires_unix_max: rows.iter().map(|record| record.proof_expires_unix).max(),
        dispatch_job_id: rows.iter().find_map(|record| record.dispatch_job_id),
        created_at: rows
            .iter()
            .map(|record| record.created_at.as_str())
            .min()
            .unwrap_or("0")
            .to_string(),
        updated_at: rows
            .iter()
            .map(|record| record.updated_at.as_str())
            .max()
            .unwrap_or("0")
            .to_string(),
    })
}

fn build_rollback_delegation_summaries(
    rollout_id: Uuid,
    rows: Vec<&AgentUpdateRollbackDelegatedProofRecord>,
    now_unix: i64,
) -> Vec<AgentUpdateRollbackDelegationView> {
    let mut grouped: BTreeMap<&str, Vec<&AgentUpdateRollbackDelegatedProofRecord>> =
        BTreeMap::new();
    for record in rows {
        grouped
            .entry(record.payload_hash.as_str())
            .or_default()
            .push(record);
    }
    let mut summaries = grouped
        .into_values()
        .filter_map(|records| build_rollback_delegation_summary(rollout_id, records, now_unix))
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.payload_hash.cmp(&right.payload_hash))
    });
    summaries
}

fn build_activation_delegation_summary(
    rollout_id: Uuid,
    rows: Vec<&AgentUpdateRollbackDelegatedProofRecord>,
    now_unix: i64,
) -> Option<AgentUpdateActivationDelegationView> {
    if rows.is_empty() {
        return None;
    }
    let first = rows[0];
    let staged_sha256_hex = first.staged_sha256_hex.clone()?;
    let ready_count = rows
        .iter()
        .filter(|record| record.status == "ready" && record.proof_expires_unix >= now_unix)
        .count() as i32;
    let dispatching_count = rows
        .iter()
        .filter(|record| record.status == "dispatching")
        .count() as i32;
    let dispatched_count = rows
        .iter()
        .filter(|record| record.status == "dispatched")
        .count() as i32;
    let failed_count = rows
        .iter()
        .filter(|record| record.status == "failed")
        .count() as i32;
    let expired_count = rows
        .iter()
        .filter(|record| record.status == "expired" || record.proof_expires_unix < now_unix)
        .count() as i32;
    Some(AgentUpdateActivationDelegationView {
        rollout_id,
        action: ROLLOUT_DELEGATED_ACTION_ACTIVATE.to_string(),
        staged_sha256_hex,
        restart_agent: first.restart_agent,
        force_unprivileged: first.force_unprivileged,
        payload_hash: first.payload_hash.clone(),
        target_count: rows.len() as i32,
        ready_count,
        dispatching_count,
        dispatched_count,
        expired_count,
        failed_count,
        proof_expires_unix_min: rows.iter().map(|record| record.proof_expires_unix).min(),
        proof_expires_unix_max: rows.iter().map(|record| record.proof_expires_unix).max(),
        dispatch_job_id: rows.iter().find_map(|record| record.dispatch_job_id),
        created_at: rows
            .iter()
            .map(|record| record.created_at.as_str())
            .min()
            .unwrap_or("0")
            .to_string(),
        updated_at: rows
            .iter()
            .map(|record| record.updated_at.as_str())
            .max()
            .unwrap_or("0")
            .to_string(),
    })
}

fn build_activation_delegation_summaries(
    rollout_id: Uuid,
    rows: Vec<&AgentUpdateRollbackDelegatedProofRecord>,
    now_unix: i64,
) -> Vec<AgentUpdateActivationDelegationView> {
    let mut grouped: BTreeMap<&str, Vec<&AgentUpdateRollbackDelegatedProofRecord>> =
        BTreeMap::new();
    for record in rows {
        grouped
            .entry(record.payload_hash.as_str())
            .or_default()
            .push(record);
    }
    let mut summaries = grouped
        .into_values()
        .filter_map(|records| build_activation_delegation_summary(rollout_id, records, now_unix))
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.payload_hash.cmp(&right.payload_hash))
    });
    summaries
}

type RollbackDelegationClaimKey = (Uuid, Option<Uuid>, Option<String>, bool, String);
type ActivationDelegationClaimKey = (Uuid, Option<Uuid>, String, bool, bool, String);

fn group_rollback_delegation_claims(
    records: Vec<AgentUpdateRollbackDelegatedProofRecord>,
) -> Vec<AgentUpdateRollbackDelegationClaim> {
    let mut grouped: HashMap<RollbackDelegationClaimKey, AgentUpdateRollbackDelegationClaim> =
        HashMap::new();
    for record in records {
        let key = (
            record.rollout_id,
            record.actor_id,
            record.rollback_sha256_hex.clone(),
            record.force_unprivileged,
            record.payload_hash.clone(),
        );
        let claim = grouped
            .entry(key)
            .or_insert_with(|| AgentUpdateRollbackDelegationClaim {
                delegation_ids: Vec::new(),
                rollout_id: record.rollout_id,
                actor_id: record.actor_id,
                rollback_sha256_hex: record.rollback_sha256_hex.clone(),
                force_unprivileged: record.force_unprivileged,
                payload_hash: record.payload_hash.clone(),
                clients: Vec::new(),
                envelopes: HashMap::new(),
            });
        claim.delegation_ids.push(record.id);
        claim.clients.push(record.client_id.clone());
        claim.envelopes.insert(record.client_id, record.envelope);
    }
    let mut claims = grouped.into_values().collect::<Vec<_>>();
    for claim in &mut claims {
        claim.delegation_ids.sort();
        claim.clients.sort();
    }
    claims.sort_by(|left, right| {
        left.rollout_id
            .cmp(&right.rollout_id)
            .then_with(|| left.payload_hash.cmp(&right.payload_hash))
    });
    claims
}

fn group_activation_delegation_claims(
    records: Vec<AgentUpdateRollbackDelegatedProofRecord>,
) -> Vec<AgentUpdateActivationDelegationClaim> {
    let mut grouped: HashMap<ActivationDelegationClaimKey, AgentUpdateActivationDelegationClaim> =
        HashMap::new();
    for record in records {
        let Some(staged_sha256_hex) = record.staged_sha256_hex.clone() else {
            continue;
        };
        let key = (
            record.rollout_id,
            record.actor_id,
            staged_sha256_hex.clone(),
            record.restart_agent,
            record.force_unprivileged,
            record.payload_hash.clone(),
        );
        let claim = grouped
            .entry(key)
            .or_insert_with(|| AgentUpdateActivationDelegationClaim {
                delegation_ids: Vec::new(),
                rollout_id: record.rollout_id,
                actor_id: record.actor_id,
                staged_sha256_hex,
                restart_agent: record.restart_agent,
                force_unprivileged: record.force_unprivileged,
                payload_hash: record.payload_hash.clone(),
                clients: Vec::new(),
                envelopes: HashMap::new(),
            });
        claim.delegation_ids.push(record.id);
        claim.clients.push(record.client_id.clone());
        claim.envelopes.insert(record.client_id, record.envelope);
    }
    let mut claims = grouped.into_values().collect::<Vec<_>>();
    for claim in &mut claims {
        claim.delegation_ids.sort();
        claim.clients.sort();
    }
    claims.sort_by(|left, right| {
        left.rollout_id
            .cmp(&right.rollout_id)
            .then_with(|| left.payload_hash.cmp(&right.payload_hash))
    });
    claims
}

impl Repository {
    async fn agent_update_delegation_summaries(
        &self,
        rollout_id: Uuid,
    ) -> Result<(
        Vec<AgentUpdateActivationDelegationView>,
        Vec<AgentUpdateRollbackDelegationView>,
    )> {
        let now_unix = crate::unix_now() as i64;
        match self {
            Self::Memory(memory) => {
                let delegations = memory.agent_update_rollback_delegations.read().await;
                let activation_rows = delegations
                    .iter()
                    .filter(|record| {
                        record.rollout_id == rollout_id
                            && record.action == ROLLOUT_DELEGATED_ACTION_ACTIVATE
                    })
                    .collect::<Vec<_>>();
                let rollback_rows = delegations
                    .iter()
                    .filter(|record| {
                        record.rollout_id == rollout_id
                            && record.action == ROLLOUT_DELEGATED_ACTION_ROLLBACK
                    })
                    .collect::<Vec<_>>();
                Ok((
                    build_activation_delegation_summaries(rollout_id, activation_rows, now_unix),
                    build_rollback_delegation_summaries(rollout_id, rollback_rows, now_unix),
                ))
            }
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        rollout_id,
                        client_id,
                        action,
                        payload_hash,
                        rollback_sha256_hex,
                        staged_sha256_hex,
                        restart_agent,
                        force_unprivileged,
                        envelope,
                        proof_expires_unix,
                        status,
                        dispatch_job_id,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM agent_update_rollout_delegated_proofs
                    WHERE rollout_id = $1
                    ORDER BY action, payload_hash, client_id
                    "#,
                )
                .bind(rollout_id)
                .fetch_all(pool)
                .await?;
                let records = rows
                    .into_iter()
                    .map(|row| {
                        let envelope: sqlx::types::Json<vpsman_common::CommandEnvelope> =
                            row.try_get("envelope")?;
                        Ok(AgentUpdateRollbackDelegatedProofRecord {
                            id: row.try_get("id")?,
                            rollout_id: row.try_get("rollout_id")?,
                            client_id: row.try_get("client_id")?,
                            action: row.try_get("action")?,
                            payload_hash: row.try_get("payload_hash")?,
                            rollback_sha256_hex: row.try_get("rollback_sha256_hex")?,
                            staged_sha256_hex: row.try_get("staged_sha256_hex")?,
                            restart_agent: row.try_get("restart_agent")?,
                            force_unprivileged: row.try_get("force_unprivileged")?,
                            envelope: envelope.0,
                            proof_expires_unix: row.try_get("proof_expires_unix")?,
                            status: row.try_get("status")?,
                            dispatch_job_id: row.try_get("dispatch_job_id")?,
                            actor_id: row.try_get("actor_id")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let activation_rows = records
                    .iter()
                    .filter(|record| record.action == ROLLOUT_DELEGATED_ACTION_ACTIVATE)
                    .collect::<Vec<_>>();
                let rollback_rows = records
                    .iter()
                    .filter(|record| record.action == ROLLOUT_DELEGATED_ACTION_ROLLBACK)
                    .collect::<Vec<_>>();
                Ok((
                    build_activation_delegation_summaries(rollout_id, activation_rows, now_unix),
                    build_rollback_delegation_summaries(rollout_id, rollback_rows, now_unix),
                ))
            }
        }
    }

    pub(crate) async fn attach_agent_update_delegation_summaries(
        &self,
        rollout: &mut AgentUpdateRolloutView,
    ) -> Result<()> {
        let (activation, rollback) = self.agent_update_delegation_summaries(rollout.id).await?;
        rollout.activation_delegations = activation;
        rollout.rollback_delegations = rollback;
        Ok(())
    }

    pub(crate) async fn expire_agent_update_delegated_proofs(&self, limit: i64) -> Result<i64> {
        let now_unix = crate::unix_now() as i64;
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut expired = Vec::new();
                for record in memory
                    .agent_update_rollback_delegations
                    .write()
                    .await
                    .iter_mut()
                {
                    if expired.len() >= limit.clamp(1, 500) as usize {
                        break;
                    }
                    if record.status == "ready" && record.proof_expires_unix < now_unix {
                        record.status = "expired".to_string();
                        record.updated_at = now.clone();
                        expired.push(record.clone());
                    }
                }
                if !expired.is_empty() {
                    memory.audits.write().await.push(AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: None,
                        action: "agent_update.delegated_proof_expired".to_string(),
                        target: "agent_update_rollout_delegations".to_string(),
                        command_hash: None,
                        metadata: delegated_proof_expired_metadata(&expired),
                        created_at: now,
                    });
                }
                Ok(expired.len() as i64)
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT id
                        FROM agent_update_rollout_delegated_proofs
                        WHERE status = 'ready'
                          AND proof_expires_unix < $2
                        ORDER BY proof_expires_unix, updated_at, id
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    )
                    UPDATE agent_update_rollout_delegated_proofs proof
                    SET status = 'expired',
                        updated_at = now()
                    FROM candidates
                    WHERE proof.id = candidates.id
                    RETURNING
                        proof.id,
                        proof.rollout_id,
                        proof.client_id,
                        proof.action,
                        proof.payload_hash,
                        proof.rollback_sha256_hex,
                        proof.staged_sha256_hex,
                        proof.restart_agent,
                        proof.force_unprivileged,
                        proof.envelope,
                        proof.proof_expires_unix,
                        proof.status,
                        proof.dispatch_job_id,
                        proof.actor_id,
                        proof.created_at::text AS created_at,
                        proof.updated_at::text AS updated_at
                    "#,
                )
                .bind(limit.clamp(1, 500))
                .bind(now_unix)
                .fetch_all(&mut *tx)
                .await?;
                let expired = rows
                    .into_iter()
                    .map(|row| {
                        let envelope: sqlx::types::Json<vpsman_common::CommandEnvelope> =
                            row.try_get("envelope")?;
                        Ok(AgentUpdateRollbackDelegatedProofRecord {
                            id: row.try_get("id")?,
                            rollout_id: row.try_get("rollout_id")?,
                            client_id: row.try_get("client_id")?,
                            action: row.try_get("action")?,
                            payload_hash: row.try_get("payload_hash")?,
                            rollback_sha256_hex: row.try_get("rollback_sha256_hex")?,
                            staged_sha256_hex: row.try_get("staged_sha256_hex")?,
                            restart_agent: row.try_get("restart_agent")?,
                            force_unprivileged: row.try_get("force_unprivileged")?,
                            envelope: envelope.0,
                            proof_expires_unix: row.try_get("proof_expires_unix")?,
                            status: row.try_get("status")?,
                            dispatch_job_id: row.try_get("dispatch_job_id")?,
                            actor_id: row.try_get("actor_id")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if !expired.is_empty() {
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, NULL, $2, $3, NULL, $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind("agent_update.delegated_proof_expired")
                    .bind("agent_update_rollout_delegations")
                    .bind(delegated_proof_expired_metadata(&expired))
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(expired.len() as i64)
            }
        }
    }

    pub(crate) async fn record_agent_update_rollback_delegation(
        &self,
        rollout_id: Uuid,
        request: &AgentUpdateRollbackDelegationRequest,
        operator: &AuthContext,
    ) -> Result<AgentUpdateRollbackDelegationView> {
        let rollout = self
            .find_agent_update_rollout(rollout_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("agent_update_rollout_not_found"))?;
        let command_hash =
            agent_update_rollback_command_hash(request.rollback_sha256_hex.as_deref())?;
        let target_clients = rollout
            .targets
            .iter()
            .map(|target| target.client_id.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut records = Vec::new();
        let mut clients = Vec::new();
        for (client_id, envelope) in &request.envelopes {
            anyhow::ensure!(
                target_clients.contains(client_id),
                "agent_update_rollout_delegation_target_not_in_rollout"
            );
            anyhow::ensure!(
                envelope.server_signature.is_empty(),
                "agent_update_rollout_delegation_must_be_unsigned"
            );
            validate_unsigned_command_envelope(envelope, client_id, &command_hash)
                .with_context(|| format!("invalid delegated rollback envelope for {client_id}"))?;
            let proof_expires_unix = envelope
                .proof
                .as_ref()
                .map(|proof| proof.expires_unix as i64)
                .context("delegated rollback envelope missing proof")?;
            records.push(AgentUpdateRollbackDelegatedProofRecord {
                id: Uuid::new_v4(),
                rollout_id,
                client_id: client_id.clone(),
                action: ROLLOUT_DELEGATED_ACTION_ROLLBACK.to_string(),
                payload_hash: command_hash.clone(),
                rollback_sha256_hex: request.rollback_sha256_hex.clone(),
                staged_sha256_hex: None,
                restart_agent: false,
                force_unprivileged: request.force_unprivileged,
                envelope: envelope.clone(),
                proof_expires_unix,
                status: "ready".to_string(),
                dispatch_job_id: None,
                actor_id: Some(operator.operator.id),
                created_at: crate::unix_now().to_string(),
                updated_at: crate::unix_now().to_string(),
            });
            clients.push(client_id.clone());
        }
        clients.sort();
        let proof_expires_unix_min = records.iter().map(|record| record.proof_expires_unix).min();
        let proof_expires_unix_max = records.iter().map(|record| record.proof_expires_unix).max();
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut delegations = memory.agent_update_rollback_delegations.write().await;
                for record in records {
                    delegations.retain(|existing| {
                        !(existing.rollout_id == rollout_id
                            && existing.client_id == record.client_id
                            && existing.action == ROLLOUT_DELEGATED_ACTION_ROLLBACK
                            && existing.payload_hash == command_hash)
                    });
                    delegations.push(record);
                }
                drop(delegations);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "agent_update.rollback_delegation_recorded".to_string(),
                    target: format!("agent_update_rollout:{rollout_id}"),
                    command_hash: Some(command_hash.clone()),
                    metadata: rollback_delegation_recorded_metadata(RollbackDelegationMetadata {
                        rollout_id,
                        operator,
                        command_hash: &command_hash,
                        clients: &clients,
                        rollback_sha256_hex: request.rollback_sha256_hex.as_deref(),
                        force_unprivileged: request.force_unprivileged,
                        proof_expires_unix_min,
                        proof_expires_unix_max,
                    }),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for record in &records {
                    sqlx::query(
                        r#"
                        INSERT INTO agent_update_rollout_delegated_proofs (
                            id, rollout_id, client_id, action, payload_hash,
                            rollback_sha256_hex, staged_sha256_hex, restart_agent,
                            force_unprivileged, envelope, proof_expires_unix, status,
                            dispatch_job_id, actor_id
                        )
                        VALUES ($1, $2, $3, $4, $5, $6, NULL, false, $7, $8, $9, 'ready', NULL, $10)
                        ON CONFLICT (rollout_id, client_id, action, payload_hash)
                        DO UPDATE SET
                            rollback_sha256_hex = EXCLUDED.rollback_sha256_hex,
                            staged_sha256_hex = NULL,
                            restart_agent = false,
                            force_unprivileged = EXCLUDED.force_unprivileged,
                            envelope = EXCLUDED.envelope,
                            proof_expires_unix = EXCLUDED.proof_expires_unix,
                            status = 'ready',
                            dispatch_job_id = NULL,
                            actor_id = EXCLUDED.actor_id,
                            updated_at = now()
                        "#,
                    )
                    .bind(record.id)
                    .bind(record.rollout_id)
                    .bind(&record.client_id)
                    .bind(&record.action)
                    .bind(&record.payload_hash)
                    .bind(&record.rollback_sha256_hex)
                    .bind(record.force_unprivileged)
                    .bind(sqlx::types::Json(&record.envelope))
                    .bind(record.proof_expires_unix)
                    .bind(record.actor_id)
                    .execute(&mut *tx)
                    .await?;
                }
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
                .bind("agent_update.rollback_delegation_recorded")
                .bind(format!("agent_update_rollout:{rollout_id}"))
                .bind(&command_hash)
                .bind(rollback_delegation_recorded_metadata(
                    RollbackDelegationMetadata {
                        rollout_id,
                        operator,
                        command_hash: &command_hash,
                        clients: &clients,
                        rollback_sha256_hex: request.rollback_sha256_hex.as_deref(),
                        force_unprivileged: request.force_unprivileged,
                        proof_expires_unix_min,
                        proof_expires_unix_max,
                    },
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        self.agent_update_rollback_delegation_summary(rollout_id, &command_hash)
            .await?
            .ok_or_else(|| anyhow::anyhow!("agent_update_rollout_delegation_not_found"))
    }

    pub(crate) async fn agent_update_rollback_delegation_summary(
        &self,
        rollout_id: Uuid,
        command_hash: &str,
    ) -> Result<Option<AgentUpdateRollbackDelegationView>> {
        match self {
            Self::Memory(memory) => {
                let now_unix = crate::unix_now() as i64;
                let delegations = memory.agent_update_rollback_delegations.read().await;
                let rows = delegations
                    .iter()
                    .filter(|record| {
                        record.rollout_id == rollout_id
                            && record.action == ROLLOUT_DELEGATED_ACTION_ROLLBACK
                            && record.payload_hash == command_hash
                    })
                    .collect::<Vec<_>>();
                Ok(build_rollback_delegation_summary(
                    rollout_id, rows, now_unix,
                ))
            }
            Self::Postgres(pool) => {
                let now_unix = crate::unix_now() as i64;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        rollout_id,
                        client_id,
                        action,
                        payload_hash,
                        rollback_sha256_hex,
                        staged_sha256_hex,
                        restart_agent,
                        force_unprivileged,
                        envelope,
                        proof_expires_unix,
                        status,
                        dispatch_job_id,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM agent_update_rollout_delegated_proofs
                    WHERE rollout_id = $1
                      AND action = $2
                      AND payload_hash = $3
                    ORDER BY client_id
                    "#,
                )
                .bind(rollout_id)
                .bind(ROLLOUT_DELEGATED_ACTION_ROLLBACK)
                .bind(command_hash)
                .fetch_all(pool)
                .await?;
                let records = rows
                    .into_iter()
                    .map(|row| {
                        let envelope: sqlx::types::Json<vpsman_common::CommandEnvelope> =
                            row.try_get("envelope")?;
                        Ok(AgentUpdateRollbackDelegatedProofRecord {
                            id: row.try_get("id")?,
                            rollout_id: row.try_get("rollout_id")?,
                            client_id: row.try_get("client_id")?,
                            action: row.try_get("action")?,
                            payload_hash: row.try_get("payload_hash")?,
                            rollback_sha256_hex: row.try_get("rollback_sha256_hex")?,
                            staged_sha256_hex: row.try_get("staged_sha256_hex")?,
                            restart_agent: row.try_get("restart_agent")?,
                            force_unprivileged: row.try_get("force_unprivileged")?,
                            envelope: envelope.0,
                            proof_expires_unix: row.try_get("proof_expires_unix")?,
                            status: row.try_get("status")?,
                            dispatch_job_id: row.try_get("dispatch_job_id")?,
                            actor_id: row.try_get("actor_id")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(build_rollback_delegation_summary(
                    rollout_id,
                    records.iter().collect(),
                    now_unix,
                ))
            }
        }
    }

    pub(crate) async fn record_agent_update_activation_delegation(
        &self,
        rollout_id: Uuid,
        request: &AgentUpdateActivationDelegationRequest,
        operator: &AuthContext,
    ) -> Result<AgentUpdateActivationDelegationView> {
        let rollout = self
            .find_agent_update_rollout(rollout_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("agent_update_rollout_not_found"))?;
        let staged_sha256_hex = rollout.artifact_sha256_hex.to_ascii_lowercase();
        let command_hash =
            agent_update_activation_command_hash(&staged_sha256_hex, request.restart_agent)?;
        let target_clients = rollout
            .targets
            .iter()
            .map(|target| target.client_id.clone())
            .collect::<std::collections::HashSet<_>>();
        let mut records = Vec::new();
        let mut clients = Vec::new();
        for (client_id, envelope) in &request.envelopes {
            anyhow::ensure!(
                target_clients.contains(client_id),
                "agent_update_rollout_delegation_target_not_in_rollout"
            );
            anyhow::ensure!(
                envelope.server_signature.is_empty(),
                "agent_update_rollout_delegation_must_be_unsigned"
            );
            validate_unsigned_command_envelope(envelope, client_id, &command_hash).with_context(
                || format!("invalid delegated activation envelope for {client_id}"),
            )?;
            let proof_expires_unix = envelope
                .proof
                .as_ref()
                .map(|proof| proof.expires_unix as i64)
                .context("delegated activation envelope missing proof")?;
            let now = crate::unix_now().to_string();
            records.push(AgentUpdateRollbackDelegatedProofRecord {
                id: Uuid::new_v4(),
                rollout_id,
                client_id: client_id.clone(),
                action: ROLLOUT_DELEGATED_ACTION_ACTIVATE.to_string(),
                payload_hash: command_hash.clone(),
                rollback_sha256_hex: None,
                staged_sha256_hex: Some(staged_sha256_hex.clone()),
                restart_agent: request.restart_agent,
                force_unprivileged: request.force_unprivileged,
                envelope: envelope.clone(),
                proof_expires_unix,
                status: "ready".to_string(),
                dispatch_job_id: None,
                actor_id: Some(operator.operator.id),
                created_at: now.clone(),
                updated_at: now,
            });
            clients.push(client_id.clone());
        }
        clients.sort();
        let proof_expires_unix_min = records.iter().map(|record| record.proof_expires_unix).min();
        let proof_expires_unix_max = records.iter().map(|record| record.proof_expires_unix).max();
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let mut delegations = memory.agent_update_rollback_delegations.write().await;
                for record in records {
                    delegations.retain(|existing| {
                        !(existing.rollout_id == rollout_id
                            && existing.client_id == record.client_id
                            && existing.action == ROLLOUT_DELEGATED_ACTION_ACTIVATE
                            && existing.payload_hash == command_hash)
                    });
                    delegations.push(record);
                }
                drop(delegations);
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: Some(operator.operator.id),
                    action: "agent_update.activation_delegation_recorded".to_string(),
                    target: format!("agent_update_rollout:{rollout_id}"),
                    command_hash: Some(command_hash.clone()),
                    metadata: activation_delegation_recorded_metadata(
                        ActivationDelegationMetadata {
                            rollout_id,
                            operator,
                            command_hash: &command_hash,
                            clients: &clients,
                            staged_sha256_hex: &staged_sha256_hex,
                            restart_agent: request.restart_agent,
                            force_unprivileged: request.force_unprivileged,
                            proof_expires_unix_min,
                            proof_expires_unix_max,
                        },
                    ),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                for record in &records {
                    sqlx::query(
                        r#"
                        INSERT INTO agent_update_rollout_delegated_proofs (
                            id, rollout_id, client_id, action, payload_hash,
                            rollback_sha256_hex, staged_sha256_hex, restart_agent,
                            force_unprivileged, envelope, proof_expires_unix, status,
                            dispatch_job_id, actor_id
                        )
                        VALUES ($1, $2, $3, $4, $5, NULL, $6, $7, $8, $9, $10, 'ready', NULL, $11)
                        ON CONFLICT (rollout_id, client_id, action, payload_hash)
                        DO UPDATE SET
                            rollback_sha256_hex = NULL,
                            staged_sha256_hex = EXCLUDED.staged_sha256_hex,
                            restart_agent = EXCLUDED.restart_agent,
                            force_unprivileged = EXCLUDED.force_unprivileged,
                            envelope = EXCLUDED.envelope,
                            proof_expires_unix = EXCLUDED.proof_expires_unix,
                            status = 'ready',
                            dispatch_job_id = NULL,
                            actor_id = EXCLUDED.actor_id,
                            updated_at = now()
                        "#,
                    )
                    .bind(record.id)
                    .bind(record.rollout_id)
                    .bind(&record.client_id)
                    .bind(&record.action)
                    .bind(&record.payload_hash)
                    .bind(&record.staged_sha256_hex)
                    .bind(record.restart_agent)
                    .bind(record.force_unprivileged)
                    .bind(sqlx::types::Json(&record.envelope))
                    .bind(record.proof_expires_unix)
                    .bind(record.actor_id)
                    .execute(&mut *tx)
                    .await?;
                }
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
                .bind("agent_update.activation_delegation_recorded")
                .bind(format!("agent_update_rollout:{rollout_id}"))
                .bind(&command_hash)
                .bind(activation_delegation_recorded_metadata(
                    ActivationDelegationMetadata {
                        rollout_id,
                        operator,
                        command_hash: &command_hash,
                        clients: &clients,
                        staged_sha256_hex: &staged_sha256_hex,
                        restart_agent: request.restart_agent,
                        force_unprivileged: request.force_unprivileged,
                        proof_expires_unix_min,
                        proof_expires_unix_max,
                    },
                ))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }
        self.agent_update_activation_delegation_summary(rollout_id, &command_hash)
            .await?
            .ok_or_else(|| anyhow::anyhow!("agent_update_rollout_delegation_not_found"))
    }

    pub(crate) async fn agent_update_activation_delegation_summary(
        &self,
        rollout_id: Uuid,
        command_hash: &str,
    ) -> Result<Option<AgentUpdateActivationDelegationView>> {
        match self {
            Self::Memory(memory) => {
                let now_unix = crate::unix_now() as i64;
                let delegations = memory.agent_update_rollback_delegations.read().await;
                let rows = delegations
                    .iter()
                    .filter(|record| {
                        record.rollout_id == rollout_id
                            && record.action == ROLLOUT_DELEGATED_ACTION_ACTIVATE
                            && record.payload_hash == command_hash
                    })
                    .collect::<Vec<_>>();
                Ok(build_activation_delegation_summary(
                    rollout_id, rows, now_unix,
                ))
            }
            Self::Postgres(pool) => {
                let now_unix = crate::unix_now() as i64;
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        rollout_id,
                        client_id,
                        action,
                        payload_hash,
                        rollback_sha256_hex,
                        staged_sha256_hex,
                        restart_agent,
                        force_unprivileged,
                        envelope,
                        proof_expires_unix,
                        status,
                        dispatch_job_id,
                        actor_id,
                        created_at::text AS created_at,
                        updated_at::text AS updated_at
                    FROM agent_update_rollout_delegated_proofs
                    WHERE rollout_id = $1
                      AND action = $2
                      AND payload_hash = $3
                    ORDER BY client_id
                    "#,
                )
                .bind(rollout_id)
                .bind(ROLLOUT_DELEGATED_ACTION_ACTIVATE)
                .bind(command_hash)
                .fetch_all(pool)
                .await?;
                let records = rows
                    .into_iter()
                    .map(|row| {
                        let envelope: sqlx::types::Json<vpsman_common::CommandEnvelope> =
                            row.try_get("envelope")?;
                        Ok(AgentUpdateRollbackDelegatedProofRecord {
                            id: row.try_get("id")?,
                            rollout_id: row.try_get("rollout_id")?,
                            client_id: row.try_get("client_id")?,
                            action: row.try_get("action")?,
                            payload_hash: row.try_get("payload_hash")?,
                            rollback_sha256_hex: row.try_get("rollback_sha256_hex")?,
                            staged_sha256_hex: row.try_get("staged_sha256_hex")?,
                            restart_agent: row.try_get("restart_agent")?,
                            force_unprivileged: row.try_get("force_unprivileged")?,
                            envelope: envelope.0,
                            proof_expires_unix: row.try_get("proof_expires_unix")?,
                            status: row.try_get("status")?,
                            dispatch_job_id: row.try_get("dispatch_job_id")?,
                            actor_id: row.try_get("actor_id")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(build_activation_delegation_summary(
                    rollout_id,
                    records.iter().collect(),
                    now_unix,
                ))
            }
        }
    }

    pub(crate) async fn claim_ready_agent_update_rollback_delegations(
        &self,
        limit: i64,
    ) -> Result<Vec<AgentUpdateRollbackDelegationClaim>> {
        let now_unix = crate::unix_now() as i64;
        match self {
            Self::Memory(memory) => {
                let rollouts = memory.agent_update_rollouts.read().await;
                let mut claimed = Vec::new();
                let mut delegations = memory.agent_update_rollback_delegations.write().await;
                for record in delegations.iter_mut() {
                    if claimed.len() >= limit.clamp(1, 200) as usize {
                        break;
                    }
                    if record.status != "ready"
                        || record.action != ROLLOUT_DELEGATED_ACTION_ROLLBACK
                        || record.proof_expires_unix < now_unix
                    {
                        continue;
                    }
                    let target_needs_rollback = rollouts
                        .iter()
                        .find(|rollout| rollout.id == record.rollout_id)
                        .and_then(|rollout| {
                            rollout
                                .targets
                                .iter()
                                .find(|target| target.client_id == record.client_id)
                        })
                        .is_some_and(|target| {
                            matches!(
                                target.status.as_str(),
                                "heartbeat_timeout" | "activation_failed"
                            )
                        });
                    if !target_needs_rollback {
                        continue;
                    }
                    record.status = "dispatching".to_string();
                    record.updated_at = crate::unix_now().to_string();
                    claimed.push(record.clone());
                }
                Ok(group_rollback_delegation_claims(claimed))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT proof.id
                        FROM agent_update_rollout_delegated_proofs proof
                        JOIN agent_update_rollout_targets target
                          ON target.rollout_id = proof.rollout_id
                         AND target.client_id = proof.client_id
                        WHERE proof.status = 'ready'
                          AND proof.action = $2
                          AND proof.proof_expires_unix >= $3
                          AND target.status IN ('heartbeat_timeout', 'activation_failed')
                        ORDER BY proof.updated_at, proof.id
                        LIMIT $1
                        FOR UPDATE SKIP LOCKED
                    )
                    UPDATE agent_update_rollout_delegated_proofs proof
                    SET status = 'dispatching',
                        updated_at = now()
                    FROM candidates
                    WHERE proof.id = candidates.id
                    RETURNING
                        proof.id,
                        proof.rollout_id,
                        proof.client_id,
                        proof.action,
                        proof.payload_hash,
                        proof.rollback_sha256_hex,
                        proof.staged_sha256_hex,
                        proof.restart_agent,
                        proof.force_unprivileged,
                        proof.envelope,
                        proof.proof_expires_unix,
                        proof.status,
                        proof.dispatch_job_id,
                        proof.actor_id,
                        proof.created_at::text AS created_at,
                        proof.updated_at::text AS updated_at
                    "#,
                )
                .bind(limit.clamp(1, 200))
                .bind(ROLLOUT_DELEGATED_ACTION_ROLLBACK)
                .bind(now_unix)
                .fetch_all(&mut *tx)
                .await?;
                tx.commit().await?;
                let records = rows
                    .into_iter()
                    .map(|row| {
                        let envelope: sqlx::types::Json<vpsman_common::CommandEnvelope> =
                            row.try_get("envelope")?;
                        Ok(AgentUpdateRollbackDelegatedProofRecord {
                            id: row.try_get("id")?,
                            rollout_id: row.try_get("rollout_id")?,
                            client_id: row.try_get("client_id")?,
                            action: row.try_get("action")?,
                            payload_hash: row.try_get("payload_hash")?,
                            rollback_sha256_hex: row.try_get("rollback_sha256_hex")?,
                            staged_sha256_hex: row.try_get("staged_sha256_hex")?,
                            restart_agent: row.try_get("restart_agent")?,
                            force_unprivileged: row.try_get("force_unprivileged")?,
                            envelope: envelope.0,
                            proof_expires_unix: row.try_get("proof_expires_unix")?,
                            status: row.try_get("status")?,
                            dispatch_job_id: row.try_get("dispatch_job_id")?,
                            actor_id: row.try_get("actor_id")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(group_rollback_delegation_claims(records))
            }
        }
    }

    pub(crate) async fn claim_ready_agent_update_activation_delegations(
        &self,
        limit: i64,
    ) -> Result<Vec<AgentUpdateActivationDelegationClaim>> {
        let now_unix = crate::unix_now() as i64;
        match self {
            Self::Memory(memory) => {
                let rollouts = memory.agent_update_rollouts.read().await;
                let mut claimed = Vec::new();
                let mut delegations = memory.agent_update_rollback_delegations.write().await;
                for record in delegations.iter_mut() {
                    if claimed.len() >= limit.clamp(1, 200) as usize {
                        break;
                    }
                    if record.status != "ready"
                        || record.action != ROLLOUT_DELEGATED_ACTION_ACTIVATE
                        || record.proof_expires_unix < now_unix
                    {
                        continue;
                    }
                    let rollout = rollouts
                        .iter()
                        .find(|rollout| rollout.id == record.rollout_id);
                    let Some(rollout) = rollout else {
                        continue;
                    };
                    if rollout.automation_next_action.as_deref() != Some("operator_activate_batch")
                        || !rollout
                            .automation_targets
                            .iter()
                            .any(|client_id| client_id == &record.client_id)
                        || record.staged_sha256_hex.as_deref()
                            != Some(rollout.artifact_sha256_hex.as_str())
                    {
                        continue;
                    }
                    let target_is_completed = rollout
                        .targets
                        .iter()
                        .find(|target| target.client_id == record.client_id)
                        .is_some_and(|target| target.status == "completed");
                    if !target_is_completed {
                        continue;
                    }
                    record.status = "dispatching".to_string();
                    record.updated_at = crate::unix_now().to_string();
                    claimed.push(record.clone());
                }
                Ok(group_activation_delegation_claims(claimed))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let rows = sqlx::query(
                    r#"
                    WITH candidates AS (
                        SELECT proof.id
                        FROM agent_update_rollout_delegated_proofs proof
                        JOIN agent_update_rollouts rollout
                          ON rollout.id = proof.rollout_id
                        JOIN agent_update_rollout_targets target
                          ON target.rollout_id = proof.rollout_id
                         AND target.client_id = proof.client_id
                        WHERE proof.status = 'ready'
                          AND proof.action = $2
                          AND proof.proof_expires_unix >= $3
                          AND target.status = 'completed'
                          AND rollout.automation_next_action = 'operator_activate_batch'
                          AND proof.client_id = ANY(rollout.automation_targets)
                          AND proof.staged_sha256_hex = rollout.artifact_sha256_hex
                        ORDER BY proof.updated_at, proof.id
                        LIMIT $1
                        FOR UPDATE OF proof SKIP LOCKED
                    )
                    UPDATE agent_update_rollout_delegated_proofs proof
                    SET status = 'dispatching',
                        updated_at = now()
                    FROM candidates
                    WHERE proof.id = candidates.id
                    RETURNING
                        proof.id,
                        proof.rollout_id,
                        proof.client_id,
                        proof.action,
                        proof.payload_hash,
                        proof.rollback_sha256_hex,
                        proof.staged_sha256_hex,
                        proof.restart_agent,
                        proof.force_unprivileged,
                        proof.envelope,
                        proof.proof_expires_unix,
                        proof.status,
                        proof.dispatch_job_id,
                        proof.actor_id,
                        proof.created_at::text AS created_at,
                        proof.updated_at::text AS updated_at
                    "#,
                )
                .bind(limit.clamp(1, 200))
                .bind(ROLLOUT_DELEGATED_ACTION_ACTIVATE)
                .bind(now_unix)
                .fetch_all(&mut *tx)
                .await?;
                tx.commit().await?;
                let records = rows
                    .into_iter()
                    .map(|row| {
                        let envelope: sqlx::types::Json<vpsman_common::CommandEnvelope> =
                            row.try_get("envelope")?;
                        Ok(AgentUpdateRollbackDelegatedProofRecord {
                            id: row.try_get("id")?,
                            rollout_id: row.try_get("rollout_id")?,
                            client_id: row.try_get("client_id")?,
                            action: row.try_get("action")?,
                            payload_hash: row.try_get("payload_hash")?,
                            rollback_sha256_hex: row.try_get("rollback_sha256_hex")?,
                            staged_sha256_hex: row.try_get("staged_sha256_hex")?,
                            restart_agent: row.try_get("restart_agent")?,
                            force_unprivileged: row.try_get("force_unprivileged")?,
                            envelope: envelope.0,
                            proof_expires_unix: row.try_get("proof_expires_unix")?,
                            status: row.try_get("status")?,
                            dispatch_job_id: row.try_get("dispatch_job_id")?,
                            actor_id: row.try_get("actor_id")?,
                            created_at: row.try_get("created_at")?,
                            updated_at: row.try_get("updated_at")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(group_activation_delegation_claims(records))
            }
        }
    }

    pub(crate) async fn mark_agent_update_rollback_delegations_dispatched(
        &self,
        rollout_id: Uuid,
        delegation_ids: &[Uuid],
        job_id: Uuid,
    ) -> Result<()> {
        self.mark_agent_update_rollback_delegations_status(
            rollout_id,
            delegation_ids,
            "dispatched",
            Some(job_id),
            None,
        )
        .await
    }

    pub(crate) async fn mark_agent_update_rollback_delegations_failed(
        &self,
        rollout_id: Uuid,
        delegation_ids: &[Uuid],
        reason: &str,
    ) -> Result<()> {
        self.mark_agent_update_rollback_delegations_status(
            rollout_id,
            delegation_ids,
            "failed",
            None,
            Some(reason),
        )
        .await
    }

    pub(crate) async fn mark_agent_update_activation_delegations_dispatched(
        &self,
        rollout_id: Uuid,
        delegation_ids: &[Uuid],
        job_id: Uuid,
    ) -> Result<()> {
        self.mark_agent_update_activation_delegations_status(
            rollout_id,
            delegation_ids,
            "dispatched",
            Some(job_id),
            None,
        )
        .await
    }

    pub(crate) async fn mark_agent_update_activation_delegations_failed(
        &self,
        rollout_id: Uuid,
        delegation_ids: &[Uuid],
        reason: &str,
    ) -> Result<()> {
        self.mark_agent_update_activation_delegations_status(
            rollout_id,
            delegation_ids,
            "failed",
            None,
            Some(reason),
        )
        .await
    }

    async fn mark_agent_update_activation_delegations_status(
        &self,
        rollout_id: Uuid,
        delegation_ids: &[Uuid],
        status: &str,
        job_id: Option<Uuid>,
        reason: Option<&str>,
    ) -> Result<()> {
        if delegation_ids.is_empty() {
            return Ok(());
        }
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let id_set = delegation_ids
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>();
                for record in memory
                    .agent_update_rollback_delegations
                    .write()
                    .await
                    .iter_mut()
                {
                    if id_set.contains(&record.id) {
                        record.status = status.to_string();
                        record.dispatch_job_id = job_id;
                        record.updated_at = now.clone();
                    }
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "agent_update.activation_delegation_status".to_string(),
                    target: format!("agent_update_rollout:{rollout_id}"),
                    command_hash: None,
                    metadata: activation_delegation_status_metadata(
                        rollout_id,
                        delegation_ids,
                        job_id,
                        status,
                        reason,
                    ),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_delegated_proofs
                    SET status = $2,
                        dispatch_job_id = COALESCE($3, dispatch_job_id),
                        updated_at = now()
                    WHERE id = ANY($1::UUID[])
                    "#,
                )
                .bind(delegation_ids)
                .bind(status)
                .bind(job_id)
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.activation_delegation_status")
                .bind(format!("agent_update_rollout:{rollout_id}"))
                .bind(activation_delegation_status_metadata(
                    rollout_id,
                    delegation_ids,
                    job_id,
                    status,
                    reason,
                ))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }

    async fn mark_agent_update_rollback_delegations_status(
        &self,
        rollout_id: Uuid,
        delegation_ids: &[Uuid],
        status: &str,
        job_id: Option<Uuid>,
        reason: Option<&str>,
    ) -> Result<()> {
        if delegation_ids.is_empty() {
            return Ok(());
        }
        match self {
            Self::Memory(memory) => {
                let now = crate::unix_now().to_string();
                let id_set = delegation_ids
                    .iter()
                    .copied()
                    .collect::<std::collections::HashSet<_>>();
                for record in memory
                    .agent_update_rollback_delegations
                    .write()
                    .await
                    .iter_mut()
                {
                    if id_set.contains(&record.id) {
                        record.status = status.to_string();
                        record.dispatch_job_id = job_id;
                        record.updated_at = now.clone();
                    }
                }
                memory.audits.write().await.push(AuditLogView {
                    id: Uuid::new_v4(),
                    actor_id: None,
                    action: "agent_update.rollback_delegation_status".to_string(),
                    target: format!("agent_update_rollout:{rollout_id}"),
                    command_hash: None,
                    metadata: rollback_delegation_status_metadata(
                        rollout_id,
                        delegation_ids,
                        job_id,
                        status,
                        reason,
                    ),
                    created_at: now,
                });
            }
            Self::Postgres(pool) => {
                sqlx::query(
                    r#"
                    UPDATE agent_update_rollout_delegated_proofs
                    SET status = $2,
                        dispatch_job_id = COALESCE($3, dispatch_job_id),
                        updated_at = now()
                    WHERE id = ANY($1::UUID[])
                    "#,
                )
                .bind(delegation_ids)
                .bind(status)
                .bind(job_id)
                .execute(pool)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, $2, $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind("agent_update.rollback_delegation_status")
                .bind(format!("agent_update_rollout:{rollout_id}"))
                .bind(rollback_delegation_status_metadata(
                    rollout_id,
                    delegation_ids,
                    job_id,
                    status,
                    reason,
                ))
                .execute(pool)
                .await?;
            }
        }
        Ok(())
    }
}
