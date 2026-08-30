use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{BTreeSet, HashMap};
use uuid::Uuid;

use crate::{
    model::{
        AgentIdentityView, AuthContext, ClientKeyRevocationView, KeyLifecycleClientView,
        KeyLifecycleReportView, UpsertAgentIdentityRequest,
    },
    repository::Repository,
    repository_inventory::ensure_postgres_tags_in_order,
    repository_jobs::{
        finish_jobs_in_tx_and_reconcile_event_sources,
        mark_active_targets_agent_lost_for_client_in_tx,
        skip_unstarted_queued_targets_for_client_in_tx,
    },
};

fn increment_decimal_digits(digits: &str) -> String {
    let mut bytes = digits.as_bytes().to_vec();
    for digit in bytes.iter_mut().rev() {
        if *digit < b'9' {
            *digit += 1;
            return String::from_utf8(bytes).expect("decimal digits are valid UTF-8");
        }
        *digit = b'0';
    }
    let mut incremented = String::with_capacity(bytes.len() + 1);
    incremented.push('1');
    incremented.push_str(&String::from_utf8(bytes).expect("decimal digits are valid UTF-8"));
    incremented
}

impl Repository {
    pub(crate) async fn preflight_agent_identity_upsert(
        &self,
        request: &UpsertAgentIdentityRequest,
    ) -> Result<()> {
        let client_id = request
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("client_id_required")?;
        let public_key = decode_public_key_hex(&request.client_public_key_hex)?;
        let requested_display_name = request
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if !request.replace_existing_key || requested_display_name.is_some() {
            self.ensure_visible_display_name_available(
                requested_display_name.unwrap_or(client_id),
                request.replace_existing_key.then_some(client_id),
            )
            .await?;
        }

        if self
            .is_public_key_revoked(&public_key)
            .await
            .context("failed to check agent key revocation before identity import")?
        {
            anyhow::bail!("agent_identity_key_revoked");
        }

        self.ensure_agent_public_key_available(client_id, &public_key)
            .await?;

        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT
                        status,
                        public_key,
                        hidden_at IS NOT NULL AS hidden
                    FROM clients
                    WHERE id = $1
                    "#,
                )
                .bind(client_id)
                .fetch_optional(pool)
                .await?;

                if let Some(row) = row {
                    let hidden: bool = row.try_get("hidden")?;
                    let status: String = row.try_get("status")?;
                    if hidden || status == "deleted" {
                        anyhow::bail!("agent_identity_deactivated");
                    }
                    let existing_key: Vec<u8> = row.try_get("public_key")?;
                    if request.replace_existing_key {
                        if existing_key.is_empty() {
                            anyhow::bail!("client_not_found_or_no_key");
                        }
                        anyhow::ensure!(existing_key != public_key, "agent_identity_key_unchanged");
                    } else {
                        anyhow::bail!("client_id_already_registered");
                    }
                } else if request.replace_existing_key {
                    anyhow::bail!("client_not_found_or_no_key");
                }
                Ok(())
            }
        }
    }

    pub(crate) async fn upsert_agent_identity(
        &self,
        request: &UpsertAgentIdentityRequest,
        operator: &AuthContext,
    ) -> Result<AgentIdentityView> {
        let client_id = match request.client_id.as_deref() {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => self.generate_auto_client_id().await?,
        };
        let public_key = decode_public_key_hex(&request.client_public_key_hex)?;
        let public_key_sha256_hex = public_key_sha256_hex(&public_key);
        let display_name = request
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(client_id.as_str())
            .to_string();
        if !request.replace_existing_key
            || request
                .display_name
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
        {
            self.ensure_visible_display_name_available(
                &display_name,
                request.replace_existing_key.then_some(client_id.as_str()),
            )
            .await?;
        }
        let tags = normalize_tags(&request.tags);
        let identity_payload_hash =
            vpsman_common::agent_identity_payload_hash(vpsman_common::AgentIdentityPayloadInput {
                client_id: &client_id,
                public_key: &public_key,
                display_name: request.display_name.as_deref(),
                tags: &tags,
                replace_existing_key: request.replace_existing_key,
            });

        if self
            .is_public_key_revoked(&public_key)
            .await
            .context("failed to check agent key revocation before identity import")?
        {
            anyhow::bail!("agent_identity_key_revoked");
        }

        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_client_lifecycles_in_tx(&mut tx, std::slice::from_ref(&client_id))
                    .await?;
                let existing = sqlx::query(
                    r#"
                    SELECT
                        id,
                        display_name,
                        status,
                        public_key,
                        process_incarnation_id,
                        hidden_at IS NOT NULL AS hidden
                    FROM clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(&client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let mut owned_key_hashes = vec![public_key_sha256_hex.clone()];
                if request.replace_existing_key {
                    if let Some(row) = existing.as_ref() {
                        let existing_key: Vec<u8> = row.try_get("public_key")?;
                        if !existing_key.is_empty() {
                            owned_key_hashes.push(
                                crate::repository_key_lifecycle::public_key_sha256_hex(
                                    &existing_key,
                                ),
                            );
                        }
                    }
                }
                lock_postgres_key_identities_in_tx(&mut tx, &owned_key_hashes).await?;
                let mut agent_lost_job_ids = Vec::new();
                if fetch_postgres_key_revocation(&mut tx, &public_key_sha256_hex)
                    .await?
                    .is_some()
                {
                    anyhow::bail!("agent_identity_key_revoked");
                }
                if sqlx::query(
                    r#"
                    SELECT 1
                    FROM clients
                    WHERE public_key = $1
                      AND id <> $2
                      AND octet_length(public_key) > 0
                    LIMIT 1
                    "#,
                )
                .bind(&public_key)
                .bind(&client_id)
                .fetch_optional(&mut *tx)
                .await?
                .is_some()
                {
                    anyhow::bail!("agent_identity_key_already_registered");
                }
                let creating_identity = existing.is_none();
                let prior_status = existing
                    .as_ref()
                    .map(|row| row.try_get::<String, _>("status"))
                    .transpose()?;

                if let Some(row) = existing.as_ref() {
                    let hidden: bool = row.try_get("hidden")?;
                    let status = prior_status
                        .as_deref()
                        .context("existing client status missing")?;
                    if hidden || status == "deleted" {
                        anyhow::bail!("agent_identity_deactivated");
                    }
                    let existing_key: Vec<u8> = row.try_get("public_key")?;
                    if request.replace_existing_key {
                        if existing_key.is_empty() {
                            anyhow::bail!("client_not_found_or_no_key");
                        }
                        anyhow::ensure!(existing_key != public_key, "agent_identity_key_unchanged");
                        sqlx::query(
                            r#"
                            INSERT INTO client_key_revocations (
                                id, client_id, public_key_sha256_hex, reason, revoked_by
                            )
                            VALUES ($1, $2, $3, 'client_key_replaced', $4)
                            ON CONFLICT (public_key_sha256_hex) DO NOTHING
                            "#,
                        )
                        .bind(Uuid::new_v4())
                        .bind(&client_id)
                        .bind(crate::repository_key_lifecycle::public_key_sha256_hex(
                            &existing_key,
                        ))
                        .bind(operator.operator.id)
                        .execute(&mut *tx)
                        .await?;
                        let old_process_incarnation_id: Option<Uuid> =
                            row.try_get("process_incarnation_id")?;
                        if let Some(old_process_incarnation_id) = old_process_incarnation_id {
                            agent_lost_job_ids = mark_active_targets_agent_lost_for_client_in_tx(
                                &mut tx,
                                &client_id,
                                old_process_incarnation_id,
                                None,
                                "client_key_replaced",
                                "client public key was replaced before final command output",
                            )
                            .await?;
                        }
                        sqlx::query(
                            r#"
                            UPDATE gateway_sessions
                            SET
                                status = 'ended',
                                last_seen_at = now(),
                                ended_at = COALESCE(ended_at, now()),
                                end_reason = COALESCE(end_reason, 'client_key_replaced')
                            WHERE client_id = $1 AND status = 'active'
                            "#,
                        )
                        .bind(&client_id)
                        .execute(&mut *tx)
                        .await?;
                    } else {
                        anyhow::bail!("client_id_already_registered");
                    }
                    sqlx::query(
                        r#"
                        UPDATE clients
                        SET display_name = CASE WHEN $2::text IS NULL THEN display_name ELSE $2 END,
                            public_key = $3,
                            status = CASE
                                WHEN $4 AND status <> 'suspended' THEN 'offline'
                                ELSE status
                            END,
                            process_incarnation_id = CASE WHEN $4 THEN NULL ELSE process_incarnation_id END,
                            stale_since = NULL,
                            stale_reason = NULL,
                            stale_build_number = NULL,
                            suspended_from_status = CASE
                                WHEN $4 AND status = 'suspended' THEN 'offline'
                                ELSE suspended_from_status
                            END
                        WHERE id = $1 AND hidden_at IS NULL
                        "#,
                    )
                    .bind(&client_id)
                    .bind(
                        request
                            .display_name
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty()),
                    )
                    .bind(&public_key)
                    .bind(request.replace_existing_key)
                    .execute(&mut *tx)
                    .await?;
                } else {
                    sqlx::query(
                        r#"
                        INSERT INTO clients (
                            id, display_name, public_key, status, internal_build_number, capabilities
                        )
                        VALUES ($1, $2, $3, 'never', 1, '{}'::jsonb)
                        "#,
                    )
                    .bind(&client_id)
                    .bind(&display_name)
                    .bind(&public_key)
                    .execute(&mut *tx)
                    .await?;
                }

                let tag_ids = ensure_postgres_tags_in_order(&mut tx, &tags).await?;
                for tag in &tags {
                    let tag_id = *tag_ids
                        .get(tag)
                        .with_context(|| format!("tag_order_insert_missing:{tag}"))?;
                    sqlx::query(
                        r#"
                        INSERT INTO client_tags (client_id, tag_id)
                        VALUES ($1, $2)
                        ON CONFLICT DO NOTHING
                        "#,
                    )
                    .bind(&client_id)
                    .bind(tag_id)
                    .execute(&mut *tx)
                    .await?;
                }
                let replacement_transition_from = replacement_transition_prior_status(
                    request.replace_existing_key,
                    prior_status.as_deref(),
                );
                if let Some(prior_status) = replacement_transition_from {
                    crate::repository_ingest::record_client_status_transition_in_tx(
                        &mut tx,
                        &client_id,
                        Some(prior_status),
                        "offline",
                        "client_key_replaced",
                        json!({
                            "operator_id": operator.operator.id,
                            "recovery_with_new_key": true,
                        }),
                        "operator_request",
                        "agent-identity-controller",
                    )
                    .await?;
                } else if !creating_identity && request.replace_existing_key {
                    // Replacing an offline or suspended client's key does not
                    // create another status edge, but it does invalidate the
                    // prior process/session boundary. Publish that tunnel
                    // evidence transition atomically with the replacement.
                    crate::repository_operational_alerts::mark_postgres_tunnel_alerts_unknown_for_clients_in_tx(
                        &mut tx,
                        std::slice::from_ref(&client_id),
                    )
                    .await?;
                }
                if creating_identity {
                    crate::repository_operational_alerts::reconcile_postgres_agent_alert_transition_in_tx(
                        &mut tx,
                        &client_id,
                        "never",
                    )
                    .await?;
                }
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &agent_lost_job_ids).await?;

                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (id, actor_id, action, target, command_hash, metadata)
                    VALUES ($1, $2, 'agent_identity.upserted', $3, $4, $5)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{}", client_id))
                .bind(&identity_payload_hash)
                .bind(json!({
                    "client_id": &client_id,
                    "requested_display_name": request.display_name.as_deref().map(str::trim),
                    "public_key_sha256_hex": public_key_sha256_hex,
                    "replace_existing_key": request.replace_existing_key,
                    "tags": tags,
                    "agent_lost_job_ids": agent_lost_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "agent-identity-controller",
                }))
                .execute(&mut *tx)
                .await?;
                let view = fetch_postgres_agent_identity(&mut tx, &client_id).await?;
                tx.commit().await?;
                Ok(view)
            }
        }
    }

    async fn generate_auto_client_id(&self) -> Result<String> {
        match self {
            Self::Postgres(pool) => {
                let max_numeric_digits = sqlx::query_scalar::<_, String>(
                    r#"
                    WITH numeric_client_ids AS (
                        SELECT COALESCE(
                            NULLIF(
                                regexp_replace(
                                    regexp_replace(id, '^v-', ''),
                                    '^0+',
                                    ''
                                ),
                                ''
                            ),
                            '0'
                        ) AS digits
                        FROM clients
                        WHERE id ~ '^(v-)?[0-9]+$'
                    )
                    SELECT digits
                    FROM numeric_client_ids
                    ORDER BY length(digits) DESC, digits DESC
                    LIMIT 1
                    "#,
                )
                .fetch_optional(pool)
                .await?
                .unwrap_or_else(|| "0".to_string());
                let next = increment_decimal_digits(&max_numeric_digits);
                Ok(format!("v-{next}"))
            }
        }
    }

    pub(crate) async fn revoke_current_client_key(
        &self,
        client_id: &str,
        reason: Option<&str>,
        operator: &AuthContext,
    ) -> Result<ClientKeyRevocationView> {
        let reason = normalized_reason(reason);
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                lock_postgres_client_lifecycles_in_tx(&mut tx, &[client_id.to_string()]).await?;
                let row = sqlx::query(
                    r#"
                    SELECT public_key, status, process_incarnation_id
                    FROM visible_clients
                    WHERE id = $1
                    FOR UPDATE
                    "#,
                )
                .bind(client_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    anyhow::bail!("client not found: {client_id}");
                };
                let current_public_key: Vec<u8> = row.try_get("public_key")?;
                if current_public_key.is_empty() {
                    anyhow::bail!("client public key missing for {client_id}");
                }
                let prior_status: String = row.try_get("status")?;
                let old_process_incarnation_id: Option<Uuid> =
                    row.try_get("process_incarnation_id")?;
                let public_key_sha256_hex = public_key_sha256_hex(&current_public_key);
                lock_postgres_key_identities_in_tx(
                    &mut tx,
                    std::slice::from_ref(&public_key_sha256_hex),
                )
                .await?;
                if let Some(existing) =
                    fetch_postgres_key_revocation(&mut tx, &public_key_sha256_hex).await?
                {
                    let agent_lost_job_ids =
                        if let Some(old_process_incarnation_id) = old_process_incarnation_id {
                            mark_active_targets_agent_lost_for_client_in_tx(
                                &mut tx,
                                client_id,
                                old_process_incarnation_id,
                                None,
                                "client_key_revoked",
                                "client key was revoked before final command output",
                            )
                            .await?
                        } else {
                            Vec::new()
                        };
                    let skipped_job_ids = skip_unstarted_queued_targets_for_client_in_tx(
                        &mut tx,
                        client_id,
                        "client_key_revoked",
                        "client_key_revoked: target skipped before dispatch",
                    )
                    .await?;
                    let mut affected_job_ids = agent_lost_job_ids.clone();
                    affected_job_ids.extend(skipped_job_ids.iter().copied());
                    finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &affected_job_ids)
                        .await?;
                    mark_postgres_agent_revoked(
                        &mut tx,
                        client_id,
                        reason.as_deref(),
                        &prior_status,
                    )
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO audit_logs (
                            id, actor_id, action, target, command_hash, metadata
                        )
                        VALUES ($1, $2, 'client_key.revoked', $3, NULL, $4)
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(operator.operator.id)
                    .bind(format!("client:{client_id}"))
                    .bind(json!({
                        "client_id": client_id,
                        "public_key_sha256_hex": existing.public_key_sha256_hex,
                        "reason": existing.reason,
                        "recovered_existing_revocation": true,
                        "agent_lost_job_ids": agent_lost_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                        "skipped_unstarted_job_ids": skipped_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                        "result": "succeeded",
                        "operator_id": operator.operator.id,
                        "operator_username": &operator.operator.username,
                        "operator_role": &operator.operator.role,
                        "operator_session_id": operator.audit_session_id(),
                        "origin_kind": "operator_request",
                        "component": "client-key-controller",
                    }))
                    .execute(&mut *tx)
                    .await?;
                    tx.commit().await?;
                    return Ok(existing);
                }

                let id = Uuid::new_v4();
                sqlx::query(
                    r#"
                    INSERT INTO client_key_revocations (
                        id, client_id, public_key_sha256_hex, reason, revoked_by
                    )
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(id)
                .bind(client_id)
                .bind(&public_key_sha256_hex)
                .bind(&reason)
                .bind(operator.operator.id)
                .execute(&mut *tx)
                .await?;
                let agent_lost_job_ids =
                    if let Some(old_process_incarnation_id) = old_process_incarnation_id {
                        mark_active_targets_agent_lost_for_client_in_tx(
                            &mut tx,
                            client_id,
                            old_process_incarnation_id,
                            None,
                            "client_key_revoked",
                            "client key was revoked before final command output",
                        )
                        .await?
                    } else {
                        Vec::new()
                    };
                let skipped_job_ids = skip_unstarted_queued_targets_for_client_in_tx(
                    &mut tx,
                    client_id,
                    "client_key_revoked",
                    "client_key_revoked: target skipped before dispatch",
                )
                .await?;
                let mut affected_job_ids = agent_lost_job_ids.clone();
                affected_job_ids.extend(skipped_job_ids.iter().copied());
                finish_jobs_in_tx_and_reconcile_event_sources(&mut tx, &affected_job_ids).await?;
                mark_postgres_agent_revoked(&mut tx, client_id, reason.as_deref(), &prior_status)
                    .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'client_key.revoked', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("client:{client_id}"))
                .bind(json!({
                    "client_id": client_id,
                    "public_key_sha256_hex": public_key_sha256_hex,
                    "reason": reason,
                    "agent_lost_job_ids": agent_lost_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "skipped_unstarted_job_ids": skipped_job_ids.iter().map(Uuid::to_string).collect::<Vec<_>>(),
                    "result": "succeeded",
                    "operator_id": operator.operator.id,
                    "operator_username": &operator.operator.username,
                    "operator_role": &operator.operator.role,
                    "operator_session_id": operator.audit_session_id(),
                    "origin_kind": "operator_request",
                    "component": "client-key-controller",
                }))
                .execute(&mut *tx)
                .await?;
                let record = fetch_postgres_key_revocation(&mut tx, &public_key_sha256_hex)
                    .await?
                    .context("inserted client key revocation was not readable")?;
                tx.commit().await?;
                Ok(record)
            }
        }
    }

    pub(crate) async fn list_client_key_revocations(
        &self,
        limit: i64,
    ) -> Result<Vec<ClientKeyRevocationView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        id,
                        client_id,
                        public_key_sha256_hex,
                        reason,
                        revoked_by,
                        EXTRACT(EPOCH FROM created_at)::bigint AS created_unix
                    FROM client_key_revocations
                    ORDER BY created_at DESC
                    LIMIT $1
                    "#,
                )
                .bind(limit)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(client_key_revocation_from_row)
                    .collect()
            }
        }
    }

    pub(crate) async fn key_lifecycle_report(&self) -> Result<KeyLifecycleReportView> {
        // This is a suggestion, not a reservation. Concurrent registrations may
        // legitimately conflict and retry with the next report value.
        let suggested_client_id = self.generate_auto_client_id().await?;
        match self {
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                    .execute(&mut *tx)
                    .await?;
                let client_rows = sqlx::query(
                    r#"
                    SELECT id, display_name, status, public_key
                    FROM visible_clients
                    ORDER BY display_name, id
                    "#,
                )
                .fetch_all(&mut *tx)
                .await?;
                let raw_clients = client_rows
                    .into_iter()
                    .map(|row| {
                        let client_id: String = row.try_get("id")?;
                        let display_name: String = row.try_get("display_name")?;
                        let status: String = row.try_get("status")?;
                        let public_key: Vec<u8> = row.try_get("public_key")?;
                        let fingerprint =
                            (!public_key.is_empty()).then(|| public_key_sha256_hex(&public_key));
                        Ok((client_id, display_name, status, fingerprint))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let current_key_pairs = raw_clients
                    .iter()
                    .filter_map(|(client_id, _, _, fingerprint)| {
                        fingerprint
                            .as_ref()
                            .map(|fingerprint| (client_id.clone(), fingerprint.clone()))
                    })
                    .collect::<Vec<_>>();
                let current_client_ids = current_key_pairs
                    .iter()
                    .map(|(client_id, _)| client_id.clone())
                    .collect::<Vec<_>>();
                let current_fingerprints = current_key_pairs
                    .iter()
                    .map(|(_, fingerprint)| fingerprint.clone())
                    .collect::<Vec<_>>();
                let latest_revocation_rows = if current_client_ids.is_empty() {
                    Vec::new()
                } else {
                    sqlx::query(
                        r#"
                            SELECT
                                current_keys.client_id,
                                revocation.reason,
                                EXTRACT(EPOCH FROM revocation.created_at)::bigint
                                    AS created_unix
                            FROM unnest($1::text[], $2::text[])
                                AS current_keys(client_id, fingerprint)
                            JOIN LATERAL (
                                SELECT reason, created_at
                                FROM client_key_revocations
                                WHERE client_id = current_keys.client_id
                                  AND public_key_sha256_hex = current_keys.fingerprint
                                ORDER BY created_at DESC, id DESC
                                LIMIT 1
                            ) revocation ON true
                            "#,
                    )
                    .bind(&current_client_ids)
                    .bind(&current_fingerprints)
                    .fetch_all(&mut *tx)
                    .await?
                };
                let latest_revocations = latest_revocation_rows
                    .into_iter()
                    .map(|row| {
                        Ok((
                            row.try_get::<String, _>("client_id")?,
                            (
                                row.try_get::<i64, _>("created_unix")?.to_string(),
                                row.try_get::<Option<String>, _>("reason")?,
                            ),
                        ))
                    })
                    .collect::<Result<HashMap<_, _>>>()?;
                let revocation_count = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*)::bigint FROM client_key_revocations",
                )
                .fetch_one(&mut *tx)
                .await?;
                tx.commit().await?;
                let mut current_key_revoked_count = 0usize;
                let clients = raw_clients
                    .into_iter()
                    .map(|(client_id, display_name, status, fingerprint)| {
                        let latest = latest_revocations.get(&client_id);
                        if latest.is_some() {
                            current_key_revoked_count += 1;
                        }
                        KeyLifecycleClientView {
                            client_id,
                            display_name,
                            status,
                            current_public_key_sha256_hex: fingerprint,
                            current_key_revoked: latest.is_some(),
                            latest_revoked_at: latest.map(|(created_at, _)| created_at.clone()),
                            latest_revocation_reason: latest.and_then(|(_, reason)| reason.clone()),
                        }
                    })
                    .collect::<Vec<_>>();
                let direct_identity_client_count = clients
                    .iter()
                    .filter(|client| client.current_public_key_sha256_hex.is_some())
                    .count();
                Ok(KeyLifecycleReportView {
                    suggested_client_id,
                    direct_identity_client_count,
                    current_key_revoked_count,
                    revocation_count: usize::try_from(revocation_count)
                        .context("client key revocation count is invalid")?,
                    clients,
                })
            }
        }
    }

    pub(crate) async fn is_public_key_revoked(&self, public_key: &[u8]) -> Result<bool> {
        let public_key_sha256_hex = public_key_sha256_hex(public_key);
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT 1
                    FROM client_key_revocations
                    WHERE public_key_sha256_hex = $1
                    LIMIT 1
                    "#,
                )
                .bind(public_key_sha256_hex)
                .fetch_optional(pool)
                .await?;
                Ok(row.is_some())
            }
        }
    }

    async fn ensure_agent_public_key_available(
        &self,
        client_id: &str,
        public_key: &[u8],
    ) -> Result<()> {
        let duplicate = match self {
            Self::Postgres(pool) => sqlx::query(
                r#"
                SELECT 1
                FROM clients
                WHERE public_key = $1
                  AND id <> $2
                  AND octet_length(public_key) > 0
                LIMIT 1
                "#,
            )
            .bind(public_key)
            .bind(client_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
        };
        anyhow::ensure!(!duplicate, "agent_identity_key_already_registered");
        Ok(())
    }

    pub(crate) async fn client_public_key_sha256_hex(
        &self,
        client_id: &str,
    ) -> Result<Option<String>> {
        match self {
            Self::Postgres(pool) => {
                let row = sqlx::query(
                    r#"
                    SELECT public_key
                    FROM visible_clients
                    WHERE id = $1
                    "#,
                )
                .bind(client_id)
                .fetch_optional(pool)
                .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let public_key: Vec<u8> = row.try_get("public_key")?;
                Ok((!public_key.is_empty()).then(|| public_key_sha256_hex(&public_key)))
            }
        }
    }
}

async fn mark_postgres_agent_revoked(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
    request_reason: Option<&str>,
    prior_status: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE clients
        SET
            status = 'revoked',
            process_incarnation_id = NULL,
            stale_since = NULL,
            stale_reason = NULL,
            stale_build_number = NULL,
            suspended_at = NULL,
            suspended_by = NULL,
            suspended_reason = NULL,
            suspended_from_status = NULL
        WHERE id = $1 AND hidden_at IS NULL
        "#,
    )
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE gateway_sessions
        SET
            status = 'ended',
            last_seen_at = now(),
            ended_at = COALESCE(ended_at, now()),
            end_reason = COALESCE(end_reason, 'client_key_revoked')
        WHERE client_id = $1 AND status = 'active'
        "#,
    )
    .bind(client_id)
    .execute(&mut **tx)
    .await?;
    if prior_status != "revoked" {
        crate::repository_ingest::record_client_status_transition_in_tx(
            tx,
            client_id,
            Some(prior_status),
            "revoked",
            "client_key_revoked",
            json!({
                "reason": request_reason,
                "frontend_visible": true,
                "access_deactivated": true,
                "recovery_allowed_with_new_key": true,
            }),
            "operator_request",
            "client-key-controller",
        )
        .await?;
    }
    Ok(())
}

async fn fetch_postgres_agent_identity(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_id: &str,
) -> Result<AgentIdentityView> {
    let row = sqlx::query(
        r#"
        SELECT
            c.id,
            c.display_name,
            c.status,
            c.public_key,
            COALESCE(array_remove(array_agg(t.name ORDER BY t.display_order, t.created_at, t.name), NULL), ARRAY[]::TEXT[]) AS tags
        FROM visible_clients c
        LEFT JOIN client_tags ct ON ct.client_id = c.id
        LEFT JOIN tags t ON t.id = ct.tag_id
        WHERE c.id = $1
        GROUP BY c.id, c.display_name, c.status, c.public_key
        "#,
    )
    .bind(client_id)
    .fetch_one(&mut **tx)
    .await?;
    let public_key: Vec<u8> = row.try_get("public_key")?;
    Ok(AgentIdentityView {
        client_id: row.try_get("id")?,
        display_name: row.try_get("display_name")?,
        status: row.try_get("status")?,
        current_public_key_sha256_hex: public_key_sha256_hex(&public_key),
        tags: row.try_get("tags")?,
    })
}

pub(crate) async fn lock_postgres_client_lifecycles_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<()> {
    // This advisory identity exists even before (or after) the client row, so
    // registration, deletion, key mutation, and dispatch can all use the same
    // exact owner. Canonical ordering prevents multi-client callers from
    // deadlocking without serializing unrelated clients.
    let ordered = client_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !ordered.is_empty() {
        sqlx::query(
            r#"
            SELECT pg_advisory_xact_lock(
                hashtextextended('vpsman:client-lifecycle:' || client.client_id, 0)
            )
            FROM unnest($1::text[]) WITH ORDINALITY AS client(client_id, lock_order)
            ORDER BY client.lock_order
            "#,
        )
        .bind(ordered)
        .fetch_all(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) async fn lock_postgres_key_identities_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    public_key_sha256_hexes: &[String],
) -> Result<()> {
    let ordered = public_key_sha256_hexes
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !ordered.is_empty() {
        sqlx::query(
            r#"
            SELECT pg_advisory_xact_lock(
                hashtextextended('vpsman:key-lifecycle:' || key.public_key_sha256_hex, 0)
            )
            FROM unnest($1::text[]) WITH ORDINALITY
                AS key(public_key_sha256_hex, lock_order)
            ORDER BY key.lock_order
            "#,
        )
        .bind(ordered)
        .fetch_all(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) async fn lock_postgres_definition_lifecycles_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition_identities: &[String],
) -> Result<()> {
    let ordered = definition_identities
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if !ordered.is_empty() {
        sqlx::query(
            r#"
            SELECT pg_advisory_xact_lock(
                hashtextextended('vpsman:definition-lifecycle:' || definition.identity, 0)
            )
            FROM unnest($1::text[]) WITH ORDINALITY AS definition(identity, lock_order)
            ORDER BY definition.lock_order
            "#,
        )
        .bind(ordered)
        .fetch_all(&mut **tx)
        .await?;
    }
    Ok(())
}

pub(crate) async fn lock_postgres_definitions_and_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    definition_identities: &[String],
    client_ids: &[String],
) -> Result<()> {
    // Every shared-definition writer uses the same namespace order: exact
    // definitions first, then exact clients. Each namespace is sorted/deduped.
    lock_postgres_definition_lifecycles_in_tx(tx, definition_identities).await?;
    lock_postgres_client_lifecycles_in_tx(tx, client_ids).await
}

pub(crate) async fn try_lock_postgres_client_lifecycles_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
) -> Result<Vec<String>> {
    let ordered = client_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if ordered.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar::<_, String>(
        r#"
        SELECT client.client_id
        FROM unnest($1::text[]) WITH ORDINALITY AS client(client_id, lock_order)
        WHERE pg_try_advisory_xact_lock(
            hashtextextended('vpsman:client-lifecycle:' || client.client_id, 0)
        )
        ORDER BY client.lock_order
        "#,
    )
    .bind(ordered)
    .fetch_all(&mut **tx)
    .await
    .map_err(Into::into)
}

pub(crate) async fn require_visible_postgres_clients_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    client_ids: &[String],
    error_code: &str,
) -> Result<()> {
    let expected = client_ids.iter().cloned().collect::<BTreeSet<_>>();
    if expected.is_empty() {
        return Ok(());
    }
    lock_postgres_client_lifecycles_in_tx(tx, &expected.iter().cloned().collect::<Vec<_>>())
        .await?;
    let visible = sqlx::query_scalar::<_, String>(
        r#"
        SELECT id
        FROM visible_clients
        WHERE id = ANY($1::text[])
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .bind(expected.iter().cloned().collect::<Vec<_>>())
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .collect::<BTreeSet<_>>();
    anyhow::ensure!(visible == expected, error_code.to_string());
    Ok(())
}

async fn fetch_postgres_key_revocation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    public_key_sha256_hex: &str,
) -> Result<Option<ClientKeyRevocationView>> {
    let row = sqlx::query(
        r#"
        SELECT
            id,
            client_id,
            public_key_sha256_hex,
            reason,
            revoked_by,
            EXTRACT(EPOCH FROM created_at)::bigint AS created_unix
        FROM client_key_revocations
        WHERE public_key_sha256_hex = $1
        "#,
    )
    .bind(public_key_sha256_hex)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(client_key_revocation_from_row).transpose()
}

fn client_key_revocation_from_row(row: sqlx::postgres::PgRow) -> Result<ClientKeyRevocationView> {
    Ok(ClientKeyRevocationView {
        id: row.try_get("id")?,
        client_id: row.try_get("client_id")?,
        public_key_sha256_hex: row.try_get("public_key_sha256_hex")?,
        reason: row.try_get("reason")?,
        revoked_by: row.try_get("revoked_by")?,
        created_at: row.try_get::<i64, _>("created_unix")?.to_string(),
    })
}

#[cfg(test)]
fn latest_current_revocation<'a>(
    revocations: &'a [ClientKeyRevocationView],
    client_id: &str,
    public_key_sha256_hex: Option<&str>,
) -> Option<&'a ClientKeyRevocationView> {
    let public_key_sha256_hex = public_key_sha256_hex?;
    revocations.iter().find(|record| {
        record.client_id == client_id && record.public_key_sha256_hex == public_key_sha256_hex
    })
}

fn normalized_reason(reason: Option<&str>) -> Option<String> {
    reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(1024).collect())
}

fn replacement_transition_prior_status(
    replace_existing_key: bool,
    prior_status: Option<&str>,
) -> Option<&str> {
    replace_existing_key
        .then_some(prior_status)
        .flatten()
        .filter(|status| !matches!(*status, "offline" | "suspended"))
}

fn decode_public_key_hex(value: &str) -> Result<Vec<u8>> {
    let public_key = hex::decode(value.trim()).context("invalid agent public key hex")?;
    anyhow::ensure!(public_key.len() == 32, "agent public key must be 32 bytes");
    Ok(public_key)
}

fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();
    for tag in tags {
        let trimmed = tag.trim();
        if trimmed.is_empty() || normalized.iter().any(|existing| existing == trimmed) {
            continue;
        }
        normalized.push(trimmed.to_string());
    }
    normalized
}

pub(crate) fn agent_identity_payload_hash(request: &UpsertAgentIdentityRequest) -> Result<String> {
    let client_id = request
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("client_id_required")?;
    let public_key = decode_public_key_hex(&request.client_public_key_hex)?;
    Ok(vpsman_common::agent_identity_payload_hash(
        vpsman_common::AgentIdentityPayloadInput {
            client_id,
            public_key: &public_key,
            display_name: request.display_name.as_deref(),
            tags: &request.tags,
            replace_existing_key: request.replace_existing_key,
        },
    ))
}

pub(crate) fn public_key_sha256_hex(public_key: &[u8]) -> String {
    hex::encode(Sha256::digest(public_key))
}

#[cfg(test)]
#[path = "tests_repository_key_lifecycle.rs"]
mod tests;
