use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;
use vpsman_common::AgentUpdateConfig;

use crate::{
    model::{
        AuthContext, ClaimEnrollmentRequest, ClaimEnrollmentResponse, CreateEnrollmentTokenRequest,
        CreateEnrollmentTokenResponse, EnrollmentTokenRecord, EnrollmentTokenView,
    },
    repository::Repository,
    security::{generate_token, token_hash},
    state::EnrollmentSettings,
    util::unix_now,
};

const DEFAULT_ENROLLMENT_TTL_SECS: u64 = 30 * 60;
const MAX_ENROLLMENT_TTL_SECS: u64 = 24 * 60 * 60;
pub(crate) const ENROLLMENT_PURPOSE_PROVISION: &str = "provision";
pub(crate) const ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT: &str = "rebuild_reenrollment";

#[derive(Debug)]
pub(crate) enum EnrollmentClaimOutcome {
    Accepted(Box<ClaimEnrollmentResponse>),
    InvalidToken,
    ExpiredToken,
    UsedToken,
    ProvisionClientIdSupplied,
    TokenClientMismatch,
    ExistingClientRequiresReenrollmentToken,
    ReenrollmentClientMissing,
    ReenrollmentClientKeyChanged,
}

pub(crate) struct EnrollmentClaimContext {
    pub(crate) fallback_display_name: String,
}

impl Repository {
    #[cfg(test)]
    pub(crate) async fn create_enrollment_token(
        &self,
        request: &CreateEnrollmentTokenRequest,
        operator: &AuthContext,
    ) -> Result<CreateEnrollmentTokenResponse> {
        self.create_enrollment_token_with_update(request, operator, &AgentUpdateConfig::default())
            .await
    }

    pub(crate) async fn create_enrollment_token_with_update(
        &self,
        request: &CreateEnrollmentTokenRequest,
        operator: &AuthContext,
        default_update: &AgentUpdateConfig,
    ) -> Result<CreateEnrollmentTokenResponse> {
        let token = generate_token();
        let token_hash = token_hash(&token);
        let token_prefix = token.chars().take(12).collect::<String>();
        let id = Uuid::new_v4();
        let now = unix_now();
        let ttl_secs = request
            .ttl_secs
            .unwrap_or(DEFAULT_ENROLLMENT_TTL_SECS)
            .clamp(60, MAX_ENROLLMENT_TTL_SECS);
        let expires_unix = now.saturating_add(ttl_secs);
        let default_tags = normalize_tags(&request.default_tags);
        let purpose = normalize_enrollment_purpose(request.purpose.as_deref());
        let allowed_client_id = normalize_optional_client_id(request.allowed_client_id.as_deref());
        let default_display_name =
            normalize_optional_display_name(request.default_display_name.as_deref());
        let update = enrollment_update_config(request, default_update);
        let requires_existing_client = purpose == ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT;
        let preserve_existing_assignments = request.preserve_existing_assignments.unwrap_or(true);
        let mut expected_old_public_key_sha256_hex = None;
        let assigned_client_id = if requires_existing_client {
            let Some(client_id) = allowed_client_id.as_deref() else {
                anyhow::bail!("re-enrollment token requires allowed client id");
            };
            let Some(fingerprint) = self.client_public_key_sha256_hex(client_id).await? else {
                anyhow::bail!("re-enrollment client does not exist");
            };
            expected_old_public_key_sha256_hex = Some(fingerprint);
            client_id.to_string()
        } else {
            if allowed_client_id.is_some() {
                anyhow::bail!("provision enrollment token cannot accept client-supplied id");
            }
            self.generate_unique_provision_client_id().await?
        };
        let token_client_id = Some(assigned_client_id.clone());

        match self {
            Self::Memory(memory) => {
                if !requires_existing_client {
                    upsert_memory_unenrolled_client(
                        memory,
                        &assigned_client_id,
                        default_display_name.as_deref(),
                        &default_tags,
                    )
                    .await;
                }
                memory
                    .enrollment_tokens
                    .write()
                    .await
                    .push(EnrollmentTokenRecord {
                        id,
                        token_hash,
                        token_prefix: token_prefix.clone(),
                        purpose: purpose.to_string(),
                        allowed_client_id: token_client_id.clone(),
                        requires_existing_client,
                        preserve_existing_assignments,
                        expected_old_public_key_sha256_hex: expected_old_public_key_sha256_hex
                            .clone(),
                        created_by: Some(operator.operator.id),
                        created_at_unix: now,
                        expires_unix,
                        used_at_unix: None,
                        used_by_client_id: None,
                        default_tags: default_tags.clone(),
                        default_display_name: default_display_name.clone(),
                        unmanaged_update_enabled: update.unmanaged_enabled,
                        unmanaged_update_version_url: update.unmanaged_version_url.clone(),
                        unmanaged_update_interval_secs: update.unmanaged_interval_secs as i64,
                        unmanaged_update_jitter_secs: update.unmanaged_jitter_secs as i64,
                        unmanaged_update_activate: update.unmanaged_activate,
                        unmanaged_update_restart_agent: update.unmanaged_restart_agent,
                    });
                memory
                    .audits
                    .write()
                    .await
                    .push(crate::model::AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: Some(operator.operator.id),
                        action: "enrollment.token_created".to_string(),
                        target: format!("enrollment_token:{id}"),
                        command_hash: None,
                        metadata: json!({
                            "token_id": id,
                            "token_prefix": token_prefix.clone(),
                            "purpose": purpose,
                            "assigned_client_id": assigned_client_id.clone(),
                            "allowed_client_id": token_client_id.clone(),
                            "requires_existing_client": requires_existing_client,
                            "preserve_existing_assignments": preserve_existing_assignments,
                            "expected_old_public_key_sha256_hex": expected_old_public_key_sha256_hex.clone(),
                            "default_tags": default_tags.clone(),
                            "default_display_name": default_display_name.clone(),
                            "created_placeholder_client": !requires_existing_client,
                            "unmanaged_update_enabled": update.unmanaged_enabled,
                            "unmanaged_update_version_url": update.unmanaged_version_url.clone(),
                            "unmanaged_update_interval_secs": update.unmanaged_interval_secs,
                            "unmanaged_update_jitter_secs": update.unmanaged_jitter_secs,
                            "unmanaged_update_activate": update.unmanaged_activate,
                            "unmanaged_update_restart_agent": update.unmanaged_restart_agent,
                            "expires_unix": expires_unix,
                        }),
                        created_at: now.to_string(),
                    });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                if !requires_existing_client {
                    insert_postgres_unenrolled_client(
                        &mut tx,
                        &assigned_client_id,
                        default_display_name.as_deref(),
                        &default_tags,
                    )
                    .await?;
                }
                sqlx::query(
                    r#"
                    INSERT INTO enrollment_tokens (
                        id,
                        token_hash,
                        token_prefix,
                        created_by,
                        default_tags,
                        expires_at,
                        purpose,
                        allowed_client_id,
                        requires_existing_client,
                        preserve_existing_assignments,
                        expected_old_public_key_sha256_hex,
                        default_display_name,
                        unmanaged_update_enabled,
                        unmanaged_update_version_url,
                        unmanaged_update_interval_secs,
                        unmanaged_update_jitter_secs,
                        unmanaged_update_activate,
                        unmanaged_update_restart_agent
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, to_timestamp($6::double precision),
                        $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18
                    )
                    "#,
                )
                .bind(id)
                .bind(&token_hash)
                .bind(&token_prefix)
                .bind(operator.operator.id)
                .bind(sqlx::types::Json(&default_tags))
                .bind(expires_unix as f64)
                .bind(purpose)
                .bind(&token_client_id)
                .bind(requires_existing_client)
                .bind(preserve_existing_assignments)
                .bind(&expected_old_public_key_sha256_hex)
                .bind(&default_display_name)
                .bind(update.unmanaged_enabled)
                .bind(&update.unmanaged_version_url)
                .bind(update.unmanaged_interval_secs as i64)
                .bind(update.unmanaged_jitter_secs as i64)
                .bind(update.unmanaged_activate)
                .bind(update.unmanaged_restart_agent)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, $2, 'enrollment.token_created', $3, NULL, $4)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(operator.operator.id)
                .bind(format!("enrollment_token:{id}"))
                .bind(json!({
                    "token_id": id,
                    "token_prefix": token_prefix.clone(),
                    "purpose": purpose,
                    "assigned_client_id": assigned_client_id.clone(),
                    "allowed_client_id": token_client_id.clone(),
                    "requires_existing_client": requires_existing_client,
                    "preserve_existing_assignments": preserve_existing_assignments,
                    "expected_old_public_key_sha256_hex": expected_old_public_key_sha256_hex.clone(),
                    "default_tags": default_tags.clone(),
                    "default_display_name": default_display_name.clone(),
                    "created_placeholder_client": !requires_existing_client,
                    "unmanaged_update_enabled": update.unmanaged_enabled,
                    "unmanaged_update_version_url": update.unmanaged_version_url.clone(),
                    "unmanaged_update_interval_secs": update.unmanaged_interval_secs,
                    "unmanaged_update_jitter_secs": update.unmanaged_jitter_secs,
                    "unmanaged_update_activate": update.unmanaged_activate,
                    "unmanaged_update_restart_agent": update.unmanaged_restart_agent,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
            }
        }

        Ok(CreateEnrollmentTokenResponse {
            id,
            token,
            token_prefix,
            purpose: purpose.to_string(),
            assigned_client_id: token_client_id.clone(),
            allowed_client_id: token_client_id,
            requires_existing_client,
            preserve_existing_assignments,
            expected_old_public_key_sha256_hex,
            expires_at: expires_unix.to_string(),
            default_tags,
            default_display_name,
            unmanaged_update_enabled: update.unmanaged_enabled,
            unmanaged_update_version_url: update.unmanaged_version_url,
            unmanaged_update_interval_secs: update.unmanaged_interval_secs as i64,
            unmanaged_update_jitter_secs: update.unmanaged_jitter_secs as i64,
            unmanaged_update_activate: update.unmanaged_activate,
            unmanaged_update_restart_agent: update.unmanaged_restart_agent,
        })
    }

    pub(crate) async fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenView>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .enrollment_tokens
                .read()
                .await
                .iter()
                .map(enrollment_token_record_view)
                .collect()),
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    SELECT
                        enrollment_tokens.id,
                        enrollment_tokens.token_prefix,
                        enrollment_tokens.purpose,
                        enrollment_tokens.allowed_client_id,
                        enrollment_tokens.requires_existing_client,
                        enrollment_tokens.preserve_existing_assignments,
                        enrollment_tokens.expected_old_public_key_sha256_hex,
                        enrollment_tokens.created_by,
                        EXTRACT(EPOCH FROM enrollment_tokens.created_at)::bigint AS created_unix,
                        EXTRACT(EPOCH FROM enrollment_tokens.expires_at)::bigint AS expires_unix,
                        EXTRACT(EPOCH FROM enrollment_tokens.used_at)::bigint AS used_unix,
                        enrollment_tokens.used_by_client_id,
                        enrollment_tokens.default_tags,
                        enrollment_tokens.default_display_name,
                        enrollment_tokens.unmanaged_update_enabled,
                        enrollment_tokens.unmanaged_update_version_url,
                        enrollment_tokens.unmanaged_update_interval_secs,
                        enrollment_tokens.unmanaged_update_jitter_secs,
                        enrollment_tokens.unmanaged_update_activate,
                        enrollment_tokens.unmanaged_update_restart_agent
                    FROM enrollment_tokens
                    ORDER BY enrollment_tokens.created_at DESC
                    LIMIT 200
                    "#,
                )
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let default_tags: sqlx::types::Json<Vec<String>> =
                            row.try_get("default_tags")?;
                        let used_unix: Option<i64> = row.try_get("used_unix")?;
                        Ok(EnrollmentTokenView {
                            id: row.try_get("id")?,
                            token_prefix: row.try_get("token_prefix")?,
                            purpose: row.try_get("purpose")?,
                            assigned_client_id: row.try_get("allowed_client_id")?,
                            allowed_client_id: row.try_get("allowed_client_id")?,
                            requires_existing_client: row.try_get("requires_existing_client")?,
                            preserve_existing_assignments: row
                                .try_get("preserve_existing_assignments")?,
                            expected_old_public_key_sha256_hex: row
                                .try_get("expected_old_public_key_sha256_hex")?,
                            created_by: row.try_get("created_by")?,
                            created_at: row.try_get::<i64, _>("created_unix")?.to_string(),
                            expires_at: row.try_get::<i64, _>("expires_unix")?.to_string(),
                            used_at: used_unix.map(|value| value.to_string()),
                            used_by_client_id: row.try_get("used_by_client_id")?,
                            default_tags: default_tags.0,
                            default_display_name: row.try_get("default_display_name")?,
                            unmanaged_update_enabled: row.try_get("unmanaged_update_enabled")?,
                            unmanaged_update_version_url: row
                                .try_get("unmanaged_update_version_url")?,
                            unmanaged_update_interval_secs: row
                                .try_get("unmanaged_update_interval_secs")?,
                            unmanaged_update_jitter_secs: row
                                .try_get("unmanaged_update_jitter_secs")?,
                            unmanaged_update_activate: row.try_get("unmanaged_update_activate")?,
                            unmanaged_update_restart_agent: row
                                .try_get("unmanaged_update_restart_agent")?,
                        })
                    })
                    .collect()
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn claim_enrollment(
        &self,
        settings: &EnrollmentSettings,
        request: &ClaimEnrollmentRequest,
    ) -> Result<EnrollmentClaimOutcome> {
        let context = EnrollmentClaimContext {
            fallback_display_name: request
                .client_id
                .clone()
                .unwrap_or_else(|| "pending-enrollment".to_string()),
        };
        self.claim_enrollment_with_context(settings, &context, request)
            .await
    }

    pub(crate) async fn claim_enrollment_with_context(
        &self,
        settings: &EnrollmentSettings,
        context: &EnrollmentClaimContext,
        request: &ClaimEnrollmentRequest,
    ) -> Result<EnrollmentClaimOutcome> {
        let token_hash = token_hash(request.token.trim());
        let public_key = decode_public_key(&request.client_public_key_hex)?;
        match self {
            Self::Memory(memory) => {
                let now = unix_now();
                let mut tokens = memory.enrollment_tokens.write().await;
                let Some(token) = tokens
                    .iter_mut()
                    .find(|token| token.token_hash == token_hash)
                else {
                    return Ok(EnrollmentClaimOutcome::InvalidToken);
                };
                if token.used_at_unix.is_some() {
                    return Ok(EnrollmentClaimOutcome::UsedToken);
                }
                if token.expires_unix <= now {
                    return Ok(EnrollmentClaimOutcome::ExpiredToken);
                }
                let tags = enrollment_claim_tags(settings, &token.default_tags);
                let policy = EnrollmentTokenPolicy::from_record(token);
                let effective_client_id = match claim_client_id(&policy, request) {
                    Ok(client_id) => client_id,
                    Err(EnrollmentClaimPolicy::ProvisionClientIdSupplied) => {
                        return Ok(EnrollmentClaimOutcome::ProvisionClientIdSupplied);
                    }
                    Err(EnrollmentClaimPolicy::TokenClientMismatch) => {
                        return Ok(EnrollmentClaimOutcome::TokenClientMismatch);
                    }
                    Err(EnrollmentClaimPolicy::ReenrollmentClientMissing) => {
                        return Ok(EnrollmentClaimOutcome::ReenrollmentClientMissing);
                    }
                    Err(other) => {
                        anyhow::bail!("unexpected enrollment claim id policy: {other:?}");
                    }
                };
                let existing_public_key = memory
                    .client_public_keys
                    .read()
                    .await
                    .get(&effective_client_id)
                    .cloned();
                match enforce_claim_policy(&policy, existing_public_key.as_deref()) {
                    EnrollmentClaimPolicy::Allowed => {}
                    EnrollmentClaimPolicy::ProvisionClientIdSupplied => {
                        return Ok(EnrollmentClaimOutcome::ProvisionClientIdSupplied);
                    }
                    EnrollmentClaimPolicy::TokenClientMismatch => {
                        return Ok(EnrollmentClaimOutcome::TokenClientMismatch);
                    }
                    EnrollmentClaimPolicy::ExistingClientRequiresReenrollmentToken => {
                        return Ok(EnrollmentClaimOutcome::ExistingClientRequiresReenrollmentToken);
                    }
                    EnrollmentClaimPolicy::ReenrollmentClientMissing => {
                        return Ok(EnrollmentClaimOutcome::ReenrollmentClientMissing);
                    }
                    EnrollmentClaimPolicy::ReenrollmentClientKeyChanged => {
                        return Ok(EnrollmentClaimOutcome::ReenrollmentClientKeyChanged);
                    }
                }
                let display_name = match policy.default_display_name.clone() {
                    Some(display_name) => display_name,
                    None => {
                        existing_memory_display_name(
                            memory,
                            &effective_client_id,
                            &context.fallback_display_name,
                        )
                        .await
                    }
                };
                token.used_at_unix = Some(now);
                token.used_by_client_id = Some(effective_client_id.clone());
                drop(tokens);

                upsert_memory_enrolled_client(
                    memory,
                    &effective_client_id,
                    request,
                    &display_name,
                    &tags,
                )
                .await;
                memory
                    .audits
                    .write()
                    .await
                    .push(crate::model::AuditLogView {
                        id: Uuid::new_v4(),
                        actor_id: None,
                        action: "enrollment.claimed".to_string(),
                        target: format!("client:{effective_client_id}"),
                        command_hash: None,
                        metadata: json!({
                            "client_id": &effective_client_id,
                            "purpose": policy.purpose,
                            "allowed_client_id": policy.allowed_client_id,
                            "requires_existing_client": policy.requires_existing_client,
                            "preserve_existing_assignments": policy.preserve_existing_assignments,
                            "expected_old_public_key_sha256_hex": policy.expected_old_public_key_sha256_hex,
                            "new_public_key_sha256_hex": public_key_sha256_hex(&public_key),
                            "public_key_bytes": public_key.len(),
                            "tags": tags,
                            "display_name": display_name,
                            "unmanaged_update_enabled": policy.unmanaged_update_enabled,
                            "unmanaged_update_version_url": policy.unmanaged_update_version_url,
                            "unmanaged_update_interval_secs": policy.unmanaged_update_interval_secs,
                            "unmanaged_update_jitter_secs": policy.unmanaged_update_jitter_secs,
                            "unmanaged_update_activate": policy.unmanaged_update_activate,
                            "unmanaged_update_restart_agent": policy.unmanaged_update_restart_agent,
                        }),
                        created_at: now.to_string(),
                    });
                Ok(EnrollmentClaimOutcome::Accepted(Box::new(claim_response(
                    settings,
                    &effective_client_id,
                    display_name,
                    tags,
                    policy.update_config(&settings.update),
                ))))
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                let row = sqlx::query(
                    r#"
                    SELECT
                        id,
                        default_tags,
                        purpose,
                        allowed_client_id,
                        requires_existing_client,
                        preserve_existing_assignments,
                        expected_old_public_key_sha256_hex,
                        default_display_name,
                        unmanaged_update_enabled,
                        unmanaged_update_version_url,
                        unmanaged_update_interval_secs,
                        unmanaged_update_jitter_secs,
                        unmanaged_update_activate,
                        unmanaged_update_restart_agent,
                        EXTRACT(EPOCH FROM expires_at)::bigint AS expires_unix,
                        used_at IS NOT NULL AS used
                    FROM enrollment_tokens
                    WHERE token_hash = $1
                    FOR UPDATE
                    "#,
                )
                .bind(&token_hash)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(row) = row else {
                    return Ok(EnrollmentClaimOutcome::InvalidToken);
                };
                let used: bool = row.try_get("used")?;
                if used {
                    return Ok(EnrollmentClaimOutcome::UsedToken);
                }
                let expires_unix: i64 = row.try_get("expires_unix")?;
                if expires_unix <= unix_now() as i64 {
                    return Ok(EnrollmentClaimOutcome::ExpiredToken);
                }
                let token_id: Uuid = row.try_get("id")?;
                let default_tags: sqlx::types::Json<Vec<String>> = row.try_get("default_tags")?;
                let tags = enrollment_claim_tags(settings, &default_tags.0);
                let policy = EnrollmentTokenPolicy {
                    purpose: row.try_get("purpose")?,
                    allowed_client_id: row.try_get("allowed_client_id")?,
                    requires_existing_client: row.try_get("requires_existing_client")?,
                    preserve_existing_assignments: row.try_get("preserve_existing_assignments")?,
                    expected_old_public_key_sha256_hex: row
                        .try_get("expected_old_public_key_sha256_hex")?,
                    default_display_name: row.try_get("default_display_name")?,
                    unmanaged_update_enabled: row.try_get("unmanaged_update_enabled")?,
                    unmanaged_update_version_url: row.try_get("unmanaged_update_version_url")?,
                    unmanaged_update_interval_secs: row
                        .try_get("unmanaged_update_interval_secs")?,
                    unmanaged_update_jitter_secs: row.try_get("unmanaged_update_jitter_secs")?,
                    unmanaged_update_activate: row.try_get("unmanaged_update_activate")?,
                    unmanaged_update_restart_agent: row
                        .try_get("unmanaged_update_restart_agent")?,
                };
                let effective_client_id = match claim_client_id(&policy, request) {
                    Ok(client_id) => client_id,
                    Err(EnrollmentClaimPolicy::ProvisionClientIdSupplied) => {
                        return Ok(EnrollmentClaimOutcome::ProvisionClientIdSupplied);
                    }
                    Err(EnrollmentClaimPolicy::TokenClientMismatch) => {
                        return Ok(EnrollmentClaimOutcome::TokenClientMismatch);
                    }
                    Err(EnrollmentClaimPolicy::ReenrollmentClientMissing) => {
                        return Ok(EnrollmentClaimOutcome::ReenrollmentClientMissing);
                    }
                    Err(other) => {
                        anyhow::bail!("unexpected enrollment claim id policy: {other:?}");
                    }
                };
                let existing =
                    current_postgres_client_identity(&mut tx, &effective_client_id).await?;
                let display_name = policy
                    .default_display_name
                    .clone()
                    .or_else(|| {
                        existing
                            .as_ref()
                            .map(|identity| identity.display_name.clone())
                    })
                    .unwrap_or_else(|| context.fallback_display_name.clone());
                let existing_key = existing.as_ref().and_then(|identity| {
                    (!identity.public_key.is_empty()).then_some(identity.public_key.as_slice())
                });
                match enforce_claim_policy(&policy, existing_key) {
                    EnrollmentClaimPolicy::Allowed => {}
                    EnrollmentClaimPolicy::ProvisionClientIdSupplied => {
                        return Ok(EnrollmentClaimOutcome::ProvisionClientIdSupplied);
                    }
                    EnrollmentClaimPolicy::TokenClientMismatch => {
                        return Ok(EnrollmentClaimOutcome::TokenClientMismatch);
                    }
                    EnrollmentClaimPolicy::ExistingClientRequiresReenrollmentToken => {
                        return Ok(EnrollmentClaimOutcome::ExistingClientRequiresReenrollmentToken);
                    }
                    EnrollmentClaimPolicy::ReenrollmentClientMissing => {
                        return Ok(EnrollmentClaimOutcome::ReenrollmentClientMissing);
                    }
                    EnrollmentClaimPolicy::ReenrollmentClientKeyChanged => {
                        return Ok(EnrollmentClaimOutcome::ReenrollmentClientKeyChanged);
                    }
                }
                sqlx::query(
                    r#"
                    INSERT INTO clients (
                        id, display_name, public_key, status, agent_version, os_release, arch
                    )
                    VALUES ($1, $2, $3, 'enrolled', '', '', '')
                    ON CONFLICT (id) DO UPDATE SET
                        display_name = EXCLUDED.display_name,
                        public_key = EXCLUDED.public_key,
                        status = 'enrolled'
                    "#,
                )
                .bind(&effective_client_id)
                .bind(&display_name)
                .bind(&public_key)
                .execute(&mut *tx)
                .await?;
                for tag in &tags {
                    sqlx::query(
                        r#"
                        INSERT INTO tags (id, name)
                        VALUES ($1, $2)
                        ON CONFLICT (name) DO NOTHING
                        "#,
                    )
                    .bind(Uuid::new_v4())
                    .bind(tag)
                    .execute(&mut *tx)
                    .await?;
                    sqlx::query(
                        r#"
                        INSERT INTO client_tags (client_id, tag_id)
                        SELECT $1, id FROM tags WHERE name = $2
                        ON CONFLICT DO NOTHING
                        "#,
                    )
                    .bind(&effective_client_id)
                    .bind(tag)
                    .execute(&mut *tx)
                    .await?;
                }
                sqlx::query(
                    r#"
                    UPDATE enrollment_tokens
                    SET used_at = now(), used_by_client_id = $2
                    WHERE id = $1
                    "#,
                )
                .bind(token_id)
                .bind(&effective_client_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    r#"
                    INSERT INTO audit_logs (
                        id, actor_id, action, target, command_hash, metadata
                    )
                    VALUES ($1, NULL, 'enrollment.claimed', $2, NULL, $3)
                    "#,
                )
                .bind(Uuid::new_v4())
                .bind(format!("client:{effective_client_id}"))
                .bind(json!({
                    "token_id": token_id,
                    "client_id": &effective_client_id,
                    "purpose": policy.purpose,
                    "allowed_client_id": policy.allowed_client_id,
                    "requires_existing_client": policy.requires_existing_client,
                    "preserve_existing_assignments": policy.preserve_existing_assignments,
                    "expected_old_public_key_sha256_hex": policy.expected_old_public_key_sha256_hex,
                    "new_public_key_sha256_hex": public_key_sha256_hex(&public_key),
                    "public_key_bytes": public_key.len(),
                    "tags": tags,
                    "display_name": display_name,
                    "unmanaged_update_enabled": policy.unmanaged_update_enabled,
                    "unmanaged_update_version_url": policy.unmanaged_update_version_url,
                    "unmanaged_update_interval_secs": policy.unmanaged_update_interval_secs,
                    "unmanaged_update_jitter_secs": policy.unmanaged_update_jitter_secs,
                    "unmanaged_update_activate": policy.unmanaged_update_activate,
                    "unmanaged_update_restart_agent": policy.unmanaged_update_restart_agent,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(EnrollmentClaimOutcome::Accepted(Box::new(claim_response(
                    settings,
                    &effective_client_id,
                    display_name,
                    tags,
                    policy.update_config(&settings.update),
                ))))
            }
        }
    }

    pub(crate) async fn client_public_key_sha256_hex(
        &self,
        client_id: &str,
    ) -> Result<Option<String>> {
        match self {
            Self::Memory(memory) => Ok(memory
                .client_public_keys
                .read()
                .await
                .get(client_id)
                .and_then(|key| (!key.is_empty()).then(|| public_key_sha256_hex(key)))),
            Self::Postgres(pool) => {
                let row = sqlx::query("SELECT public_key FROM clients WHERE id = $1")
                    .bind(client_id)
                    .fetch_optional(pool)
                    .await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let public_key: Vec<u8> = row.try_get("public_key")?;
                if public_key.is_empty() {
                    return Ok(None);
                }
                Ok(Some(public_key_sha256_hex(&public_key)))
            }
        }
    }

    async fn generate_unique_provision_client_id(&self) -> Result<String> {
        for _ in 0..16 {
            let candidate = Uuid::new_v4().to_string();
            if !self.client_id_exists(&candidate).await? {
                return Ok(candidate);
            }
        }
        anyhow::bail!("failed to generate unique provision client id")
    }

    async fn client_id_exists(&self, client_id: &str) -> Result<bool> {
        match self {
            Self::Memory(memory) => Ok(memory
                .agents
                .read()
                .await
                .iter()
                .any(|agent| agent.id == client_id)),
            Self::Postgres(pool) => {
                let row = sqlx::query("SELECT 1 FROM clients WHERE id = $1 LIMIT 1")
                    .bind(client_id)
                    .fetch_optional(pool)
                    .await?;
                Ok(row.is_some())
            }
        }
    }
}

fn enrollment_token_record_view(record: &EnrollmentTokenRecord) -> EnrollmentTokenView {
    EnrollmentTokenView {
        id: record.id,
        token_prefix: record.token_prefix.clone(),
        purpose: record.purpose.clone(),
        assigned_client_id: record.allowed_client_id.clone(),
        allowed_client_id: record.allowed_client_id.clone(),
        requires_existing_client: record.requires_existing_client,
        preserve_existing_assignments: record.preserve_existing_assignments,
        expected_old_public_key_sha256_hex: record.expected_old_public_key_sha256_hex.clone(),
        created_by: record.created_by,
        created_at: record.created_at_unix.to_string(),
        expires_at: record.expires_unix.to_string(),
        used_at: record.used_at_unix.map(|value| value.to_string()),
        used_by_client_id: record.used_by_client_id.clone(),
        default_tags: record.default_tags.clone(),
        default_display_name: record.default_display_name.clone(),
        unmanaged_update_enabled: record.unmanaged_update_enabled,
        unmanaged_update_version_url: record.unmanaged_update_version_url.clone(),
        unmanaged_update_interval_secs: record.unmanaged_update_interval_secs,
        unmanaged_update_jitter_secs: record.unmanaged_update_jitter_secs,
        unmanaged_update_activate: record.unmanaged_update_activate,
        unmanaged_update_restart_agent: record.unmanaged_update_restart_agent,
    }
}

#[derive(Clone, Debug)]
struct EnrollmentTokenPolicy {
    purpose: String,
    allowed_client_id: Option<String>,
    requires_existing_client: bool,
    preserve_existing_assignments: bool,
    expected_old_public_key_sha256_hex: Option<String>,
    default_display_name: Option<String>,
    unmanaged_update_enabled: bool,
    unmanaged_update_version_url: String,
    unmanaged_update_interval_secs: i64,
    unmanaged_update_jitter_secs: i64,
    unmanaged_update_activate: bool,
    unmanaged_update_restart_agent: bool,
}

impl EnrollmentTokenPolicy {
    fn from_record(record: &EnrollmentTokenRecord) -> Self {
        Self {
            purpose: record.purpose.clone(),
            allowed_client_id: record.allowed_client_id.clone(),
            requires_existing_client: record.requires_existing_client,
            preserve_existing_assignments: record.preserve_existing_assignments,
            expected_old_public_key_sha256_hex: record.expected_old_public_key_sha256_hex.clone(),
            default_display_name: record.default_display_name.clone(),
            unmanaged_update_enabled: record.unmanaged_update_enabled,
            unmanaged_update_version_url: record.unmanaged_update_version_url.clone(),
            unmanaged_update_interval_secs: record.unmanaged_update_interval_secs,
            unmanaged_update_jitter_secs: record.unmanaged_update_jitter_secs,
            unmanaged_update_activate: record.unmanaged_update_activate,
            unmanaged_update_restart_agent: record.unmanaged_update_restart_agent,
        }
    }

    fn update_config(&self, default_update: &AgentUpdateConfig) -> AgentUpdateConfig {
        AgentUpdateConfig {
            trusted_artifact_signing_key_hex: default_update
                .trusted_artifact_signing_key_hex
                .clone(),
            unmanaged_enabled: self.unmanaged_update_enabled,
            unmanaged_version_url: self.unmanaged_update_version_url.clone(),
            unmanaged_interval_secs: self.unmanaged_update_interval_secs.max(300) as u64,
            unmanaged_jitter_secs: self.unmanaged_update_jitter_secs.max(0) as u64,
            unmanaged_activate: self.unmanaged_update_activate,
            unmanaged_restart_agent: self.unmanaged_update_restart_agent,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnrollmentClaimPolicy {
    Allowed,
    ProvisionClientIdSupplied,
    TokenClientMismatch,
    ExistingClientRequiresReenrollmentToken,
    ReenrollmentClientMissing,
    ReenrollmentClientKeyChanged,
}

async fn upsert_memory_unenrolled_client(
    memory: &crate::repository::MemoryState,
    client_id: &str,
    default_display_name: Option<&str>,
    tags: &[String],
) {
    let display_name = default_display_name.unwrap_or(client_id);
    let mut agents = memory.agents.write().await;
    if let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) {
        agent.display_name = display_name.to_string();
        agent.status = "unenrolled".to_string();
        for tag in tags {
            if !agent.tags.iter().any(|existing| existing == tag) {
                agent.tags.push(tag.clone());
            }
        }
        agent.tags.sort();
    } else {
        agents.push(crate::model::AgentView {
            id: client_id.to_string(),
            display_name: display_name.to_string(),
            status: "unenrolled".to_string(),
            tags: tags.to_vec(),
            capabilities: Default::default(),
        });
    }
    drop(agents);
}

async fn insert_postgres_unenrolled_client(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
    default_display_name: Option<&str>,
    tags: &[String],
) -> Result<()> {
    let display_name = default_display_name.unwrap_or(client_id);
    sqlx::query(
        r#"
        INSERT INTO clients (
            id, display_name, public_key, status, agent_version, os_release, arch
        )
        VALUES ($1, $2, $3, 'unenrolled', '', '', '')
        "#,
    )
    .bind(client_id)
    .bind(display_name)
    .bind(Vec::<u8>::new())
    .execute(&mut **tx)
    .await?;
    for tag in tags {
        sqlx::query(
            r#"
            INSERT INTO tags (id, name)
            VALUES ($1, $2)
            ON CONFLICT (name) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tag)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO client_tags (client_id, tag_id)
            SELECT $1, id FROM tags WHERE name = $2
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(client_id)
        .bind(tag)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

struct PostgresClientIdentity {
    public_key: Vec<u8>,
    display_name: String,
}

async fn current_postgres_client_identity(
    tx: &mut Transaction<'_, Postgres>,
    client_id: &str,
) -> Result<Option<PostgresClientIdentity>> {
    let row = sqlx::query("SELECT public_key, display_name FROM clients WHERE id = $1")
        .bind(client_id)
        .fetch_optional(&mut **tx)
        .await?;
    row.map(|row| {
        Ok(PostgresClientIdentity {
            public_key: row.try_get("public_key")?,
            display_name: row.try_get("display_name")?,
        })
    })
    .transpose()
}

fn enforce_claim_policy(
    policy: &EnrollmentTokenPolicy,
    existing_public_key: Option<&[u8]>,
) -> EnrollmentClaimPolicy {
    let existing_public_key_sha256_hex = existing_public_key.map(public_key_sha256_hex);
    if existing_public_key_sha256_hex.is_some()
        && policy.purpose != ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT
    {
        return EnrollmentClaimPolicy::ExistingClientRequiresReenrollmentToken;
    }
    if policy.requires_existing_client && existing_public_key_sha256_hex.is_none() {
        return EnrollmentClaimPolicy::ReenrollmentClientMissing;
    }
    if let (Some(expected), Some(current)) = (
        policy.expected_old_public_key_sha256_hex.as_deref(),
        existing_public_key_sha256_hex.as_deref(),
    ) {
        if expected != current {
            return EnrollmentClaimPolicy::ReenrollmentClientKeyChanged;
        }
    }
    EnrollmentClaimPolicy::Allowed
}

fn claim_client_id(
    policy: &EnrollmentTokenPolicy,
    request: &ClaimEnrollmentRequest,
) -> std::result::Result<String, EnrollmentClaimPolicy> {
    let Some(token_client_id) = policy.allowed_client_id.as_deref() else {
        return Err(EnrollmentClaimPolicy::ReenrollmentClientMissing);
    };
    if policy.purpose == ENROLLMENT_PURPOSE_PROVISION {
        if request.client_id.as_deref().is_some() {
            return Err(EnrollmentClaimPolicy::ProvisionClientIdSupplied);
        }
        return Ok(token_client_id.to_string());
    }
    if request
        .client_id
        .as_deref()
        .is_some_and(|client_id| client_id != token_client_id)
    {
        return Err(EnrollmentClaimPolicy::TokenClientMismatch);
    }
    Ok(token_client_id.to_string())
}

async fn upsert_memory_enrolled_client(
    memory: &crate::repository::MemoryState,
    client_id: &str,
    request: &ClaimEnrollmentRequest,
    display_name: &str,
    tags: &[String],
) {
    if let Ok(public_key) = decode_public_key(&request.client_public_key_hex) {
        memory
            .client_public_keys
            .write()
            .await
            .insert(client_id.to_string(), public_key);
    }
    let mut agents = memory.agents.write().await;
    if let Some(agent) = agents.iter_mut().find(|agent| agent.id == client_id) {
        agent.display_name = display_name.to_string();
        agent.status = "enrolled".to_string();
        for tag in tags {
            if !agent.tags.iter().any(|existing| existing == tag) {
                agent.tags.push(tag.clone());
            }
        }
        agent.tags.sort();
        return;
    }
    agents.push(crate::model::AgentView {
        id: client_id.to_string(),
        display_name: display_name.to_string(),
        status: "enrolled".to_string(),
        tags: tags.to_vec(),
        capabilities: Default::default(),
    });
    drop(agents);
}

fn claim_response(
    settings: &EnrollmentSettings,
    client_id: &str,
    display_name: String,
    tags: Vec<String>,
    update: AgentUpdateConfig,
) -> ClaimEnrollmentResponse {
    ClaimEnrollmentResponse {
        client_id: client_id.to_string(),
        display_name,
        tcp_endpoints: settings.tcp_endpoints.clone(),
        discovery_url: settings.discovery_url.clone(),
        noise_mode: settings.noise_mode,
        gateway_server_public_key_hex: settings.gateway_server_public_key_hex.clone(),
        server_ed25519_public_key_hex: settings.server_ed25519_public_key_hex.clone(),
        discovery_trusted_server_ed25519_public_keys_hex: settings
            .discovery_trusted_server_ed25519_public_keys_hex
            .clone(),
        telemetry_light_secs: settings.telemetry_light_secs,
        telemetry_full_secs: settings.telemetry_full_secs,
        tags,
        update,
    }
}

pub(crate) fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut tags = tags
        .iter()
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    tags
}

fn enrollment_claim_tags(settings: &EnrollmentSettings, token_tags: &[String]) -> Vec<String> {
    let mut tags = token_tags.to_vec();
    if let Some(country_tag) = settings.default_country_tag.as_ref() {
        tags.push(country_tag.clone());
    }
    tags = normalize_tags(&tags);
    tags
}

async fn existing_memory_display_name(
    memory: &crate::repository::MemoryState,
    client_id: &str,
    fallback: &str,
) -> String {
    memory
        .agents
        .read()
        .await
        .iter()
        .find(|agent| agent.id == client_id)
        .map(|agent| agent.display_name.clone())
        .unwrap_or_else(|| fallback.to_string())
}

fn normalize_optional_display_name(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn enrollment_update_config(
    request: &CreateEnrollmentTokenRequest,
    default_update: &AgentUpdateConfig,
) -> AgentUpdateConfig {
    AgentUpdateConfig {
        trusted_artifact_signing_key_hex: default_update.trusted_artifact_signing_key_hex.clone(),
        unmanaged_enabled: request
            .unmanaged_update_enabled
            .unwrap_or(default_update.unmanaged_enabled),
        unmanaged_version_url: request
            .unmanaged_update_version_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| default_update.unmanaged_version_url.clone()),
        unmanaged_interval_secs: request
            .unmanaged_update_interval_secs
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(default_update.unmanaged_interval_secs),
        unmanaged_jitter_secs: request
            .unmanaged_update_jitter_secs
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(default_update.unmanaged_jitter_secs),
        unmanaged_activate: request
            .unmanaged_update_activate
            .unwrap_or(default_update.unmanaged_activate),
        unmanaged_restart_agent: request
            .unmanaged_update_restart_agent
            .unwrap_or(default_update.unmanaged_restart_agent),
    }
}

fn decode_public_key(value: &str) -> Result<Vec<u8>> {
    let public_key = hex::decode(value.trim()).context("client public key is not valid hex")?;
    anyhow::ensure!(public_key.len() == 32, "client public key must be 32 bytes");
    Ok(public_key)
}

pub(crate) fn normalize_enrollment_purpose(purpose: Option<&str>) -> &'static str {
    match purpose.unwrap_or(ENROLLMENT_PURPOSE_PROVISION).trim() {
        ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT => ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT,
        _ => ENROLLMENT_PURPOSE_PROVISION,
    }
}

pub(crate) fn normalize_optional_client_id(client_id: Option<&str>) -> Option<String> {
    client_id
        .map(str::trim)
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_string)
}

pub(crate) fn public_key_sha256_hex(public_key: &[u8]) -> String {
    hex::encode(Sha256::digest(public_key))
}
