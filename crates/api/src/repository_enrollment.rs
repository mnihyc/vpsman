use anyhow::{Context, Result};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{
    model::{
        AuthContext, ClaimEnrollmentRequest, ClaimEnrollmentResponse, CreateEnrollmentTokenRequest,
        CreateEnrollmentTokenResponse, EnrollmentTokenRecord, EnrollmentTokenView,
        ResourcePoolView,
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
    TokenClientMismatch,
    ExistingClientRequiresReenrollmentToken,
    ReenrollmentClientMissing,
    ReenrollmentClientKeyChanged,
}

pub(crate) struct EnrollmentClaimContext {
    pub(crate) fallback_display_name: String,
}

impl Repository {
    pub(crate) async fn create_enrollment_token(
        &self,
        request: &CreateEnrollmentTokenRequest,
        operator: &AuthContext,
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
        let default_pool = self
            .resolve_enrollment_default_pool(request.default_pool_name.as_deref())
            .await?;
        let default_pool_id = default_pool.as_ref().map(|pool| pool.id);
        let default_pool_name = default_pool.as_ref().map(|pool| pool.name.clone());
        let default_display_name =
            normalize_optional_display_name(request.default_display_name.as_deref());
        let requires_existing_client = purpose == ENROLLMENT_PURPOSE_REBUILD_REENROLLMENT;
        let preserve_existing_assignments = request.preserve_existing_assignments.unwrap_or(true);
        let expected_old_public_key_sha256_hex = if requires_existing_client {
            let Some(client_id) = allowed_client_id.as_deref() else {
                anyhow::bail!("re-enrollment token requires allowed client id");
            };
            let Some(fingerprint) = self.client_public_key_sha256_hex(client_id).await? else {
                anyhow::bail!("re-enrollment client does not exist");
            };
            Some(fingerprint)
        } else {
            None
        };

        match self {
            Self::Memory(memory) => {
                memory
                    .enrollment_tokens
                    .write()
                    .await
                    .push(EnrollmentTokenRecord {
                        id,
                        token_hash,
                        token_prefix: token_prefix.clone(),
                        purpose: purpose.to_string(),
                        allowed_client_id: allowed_client_id.clone(),
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
                        default_pool_id,
                        default_display_name: default_display_name.clone(),
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
                            "allowed_client_id": allowed_client_id.clone(),
                            "requires_existing_client": requires_existing_client,
                            "preserve_existing_assignments": preserve_existing_assignments,
                            "expected_old_public_key_sha256_hex": expected_old_public_key_sha256_hex.clone(),
                            "default_tags": default_tags.clone(),
                            "default_pool_name": default_pool_name.clone(),
                            "default_display_name": default_display_name.clone(),
                            "expires_unix": expires_unix,
                        }),
                        created_at: now.to_string(),
                    });
            }
            Self::Postgres(pool) => {
                let mut tx = pool.begin().await?;
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
                        default_pool_id,
                        default_display_name
                    )
                    VALUES (
                        $1, $2, $3, $4, $5, to_timestamp($6::double precision),
                        $7, $8, $9, $10, $11, $12, $13
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
                .bind(&allowed_client_id)
                .bind(requires_existing_client)
                .bind(preserve_existing_assignments)
                .bind(&expected_old_public_key_sha256_hex)
                .bind(default_pool_id)
                .bind(&default_display_name)
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
                    "allowed_client_id": allowed_client_id.clone(),
                    "requires_existing_client": requires_existing_client,
                    "preserve_existing_assignments": preserve_existing_assignments,
                    "expected_old_public_key_sha256_hex": expected_old_public_key_sha256_hex.clone(),
                    "default_tags": default_tags.clone(),
                    "default_pool_name": default_pool_name.clone(),
                    "default_display_name": default_display_name.clone(),
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
            allowed_client_id,
            requires_existing_client,
            preserve_existing_assignments,
            expected_old_public_key_sha256_hex,
            expires_at: expires_unix.to_string(),
            default_tags,
            default_pool_name,
            default_display_name,
        })
    }

    pub(crate) async fn list_enrollment_tokens(&self) -> Result<Vec<EnrollmentTokenView>> {
        match self {
            Self::Memory(memory) => {
                let pools = memory.pools.read().await.clone();
                Ok(memory
                    .enrollment_tokens
                    .read()
                    .await
                    .iter()
                    .map(|record| enrollment_token_record_view(record, &pools))
                    .collect())
            }
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
                        enrollment_tokens.default_pool_id,
                        resource_pools.name AS default_pool_name,
                        enrollment_tokens.default_display_name
                    FROM enrollment_tokens
                    LEFT JOIN resource_pools ON resource_pools.id = enrollment_tokens.default_pool_id
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
                            default_pool_name: row.try_get("default_pool_name")?,
                            default_display_name: row.try_get("default_display_name")?,
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
            fallback_display_name: request.client_id.clone(),
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
                match memory_claim_policy(memory, token, request).await? {
                    EnrollmentClaimPolicy::Allowed => {}
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
                let policy = EnrollmentTokenPolicy::from_record(token);
                let display_name = match policy.default_display_name.clone() {
                    Some(display_name) => display_name,
                    None => {
                        existing_memory_display_name(
                            memory,
                            &request.client_id,
                            &context.fallback_display_name,
                        )
                        .await
                    }
                };
                token.used_at_unix = Some(now);
                token.used_by_client_id = Some(request.client_id.clone());
                drop(tokens);

                upsert_memory_enrolled_client(
                    memory,
                    request,
                    &display_name,
                    &tags,
                    policy.default_pool_id,
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
                        target: format!("client:{}", request.client_id),
                        command_hash: None,
                        metadata: json!({
                            "client_id": request.client_id,
                            "purpose": policy.purpose,
                            "allowed_client_id": policy.allowed_client_id,
                            "requires_existing_client": policy.requires_existing_client,
                            "preserve_existing_assignments": policy.preserve_existing_assignments,
                            "expected_old_public_key_sha256_hex": policy.expected_old_public_key_sha256_hex,
                            "new_public_key_sha256_hex": public_key_sha256_hex(&public_key),
                            "public_key_bytes": public_key.len(),
                            "tags": tags,
                            "default_pool_id": policy.default_pool_id,
                            "display_name": display_name,
                        }),
                        created_at: now.to_string(),
                    });
                Ok(EnrollmentClaimOutcome::Accepted(Box::new(claim_response(
                    settings,
                    request,
                    display_name,
                    tags,
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
                        default_pool_id,
                        default_display_name,
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
                    default_pool_id: row.try_get("default_pool_id")?,
                    default_display_name: row.try_get("default_display_name")?,
                };
                let existing =
                    current_postgres_client_identity(&mut tx, &request.client_id).await?;
                let display_name = policy
                    .default_display_name
                    .clone()
                    .or_else(|| {
                        existing
                            .as_ref()
                            .map(|identity| identity.display_name.clone())
                    })
                    .unwrap_or_else(|| context.fallback_display_name.clone());
                let existing_key = existing
                    .as_ref()
                    .map(|identity| identity.public_key.as_slice());
                match enforce_claim_policy(&policy, request, existing_key) {
                    EnrollmentClaimPolicy::Allowed => {}
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
                        id, display_name, public_key, status, agent_version, os_release, arch, pool_id
                    )
                    VALUES ($1, $2, $3, 'enrolled', '', '', '', $4)
                    ON CONFLICT (id) DO UPDATE SET
                        display_name = EXCLUDED.display_name,
                        public_key = EXCLUDED.public_key,
                        status = 'enrolled',
                        pool_id = COALESCE(EXCLUDED.pool_id, clients.pool_id)
                    "#,
                )
                .bind(&request.client_id)
                .bind(&display_name)
                .bind(&public_key)
                .bind(policy.default_pool_id)
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
                    .bind(&request.client_id)
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
                .bind(&request.client_id)
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
                .bind(format!("client:{}", request.client_id))
                .bind(json!({
                    "token_id": token_id,
                    "client_id": request.client_id,
                    "purpose": policy.purpose,
                    "allowed_client_id": policy.allowed_client_id,
                    "requires_existing_client": policy.requires_existing_client,
                    "preserve_existing_assignments": policy.preserve_existing_assignments,
                    "expected_old_public_key_sha256_hex": policy.expected_old_public_key_sha256_hex,
                    "new_public_key_sha256_hex": public_key_sha256_hex(&public_key),
                    "public_key_bytes": public_key.len(),
                    "tags": tags,
                    "default_pool_id": policy.default_pool_id,
                    "display_name": display_name,
                }))
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                Ok(EnrollmentClaimOutcome::Accepted(Box::new(claim_response(
                    settings,
                    request,
                    display_name,
                    tags,
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
                .map(|key| public_key_sha256_hex(key))),
            Self::Postgres(pool) => {
                let row = sqlx::query("SELECT public_key FROM clients WHERE id = $1")
                    .bind(client_id)
                    .fetch_optional(pool)
                    .await?;
                row.map(|row| {
                    let public_key: Vec<u8> = row.try_get("public_key")?;
                    Ok(public_key_sha256_hex(&public_key))
                })
                .transpose()
            }
        }
    }

    async fn resolve_enrollment_default_pool(
        &self,
        default_pool_name: Option<&str>,
    ) -> Result<Option<ResourcePoolView>> {
        let Some(default_pool_name) = default_pool_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        Ok(Some(self.pool_by_name(default_pool_name).await?))
    }
}

fn enrollment_token_record_view(
    record: &EnrollmentTokenRecord,
    pools: &[ResourcePoolView],
) -> EnrollmentTokenView {
    EnrollmentTokenView {
        id: record.id,
        token_prefix: record.token_prefix.clone(),
        purpose: record.purpose.clone(),
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
        default_pool_name: record
            .default_pool_id
            .and_then(|pool_id| pools.iter().find(|pool| pool.id == pool_id))
            .map(|pool| pool.name.clone()),
        default_display_name: record.default_display_name.clone(),
    }
}

#[derive(Clone, Debug)]
struct EnrollmentTokenPolicy {
    purpose: String,
    allowed_client_id: Option<String>,
    requires_existing_client: bool,
    preserve_existing_assignments: bool,
    expected_old_public_key_sha256_hex: Option<String>,
    default_pool_id: Option<Uuid>,
    default_display_name: Option<String>,
}

impl EnrollmentTokenPolicy {
    fn from_record(record: &EnrollmentTokenRecord) -> Self {
        Self {
            purpose: record.purpose.clone(),
            allowed_client_id: record.allowed_client_id.clone(),
            requires_existing_client: record.requires_existing_client,
            preserve_existing_assignments: record.preserve_existing_assignments,
            expected_old_public_key_sha256_hex: record.expected_old_public_key_sha256_hex.clone(),
            default_pool_id: record.default_pool_id,
            default_display_name: record.default_display_name.clone(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnrollmentClaimPolicy {
    Allowed,
    TokenClientMismatch,
    ExistingClientRequiresReenrollmentToken,
    ReenrollmentClientMissing,
    ReenrollmentClientKeyChanged,
}

async fn memory_claim_policy(
    memory: &crate::repository::MemoryState,
    token: &EnrollmentTokenRecord,
    request: &ClaimEnrollmentRequest,
) -> Result<EnrollmentClaimPolicy> {
    let policy = EnrollmentTokenPolicy::from_record(token);
    let existing_public_key = memory
        .client_public_keys
        .read()
        .await
        .get(&request.client_id)
        .cloned();
    Ok(enforce_claim_policy(
        &policy,
        request,
        existing_public_key.as_deref(),
    ))
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
    request: &ClaimEnrollmentRequest,
    existing_public_key: Option<&[u8]>,
) -> EnrollmentClaimPolicy {
    if policy
        .allowed_client_id
        .as_deref()
        .is_some_and(|client_id| client_id != request.client_id)
    {
        return EnrollmentClaimPolicy::TokenClientMismatch;
    }

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

async fn upsert_memory_enrolled_client(
    memory: &crate::repository::MemoryState,
    request: &ClaimEnrollmentRequest,
    display_name: &str,
    tags: &[String],
    default_pool_id: Option<Uuid>,
) {
    if let Ok(public_key) = decode_public_key(&request.client_public_key_hex) {
        memory
            .client_public_keys
            .write()
            .await
            .insert(request.client_id.clone(), public_key);
    }
    let mut agents = memory.agents.write().await;
    if let Some(agent) = agents
        .iter_mut()
        .find(|agent| agent.id == request.client_id)
    {
        agent.display_name = display_name.to_string();
        agent.status = "enrolled".to_string();
        for tag in tags {
            if !agent.tags.iter().any(|existing| existing == tag) {
                agent.tags.push(tag.clone());
            }
        }
        agent.tags.sort();
        if let Some(default_pool_id) = default_pool_id {
            memory
                .agent_pools
                .write()
                .await
                .insert(request.client_id.clone(), default_pool_id);
        }
        return;
    }
    agents.push(crate::model::AgentView {
        id: request.client_id.clone(),
        display_name: display_name.to_string(),
        status: "enrolled".to_string(),
        tags: tags.to_vec(),
        capabilities: Default::default(),
    });
    drop(agents);
    if let Some(default_pool_id) = default_pool_id {
        memory
            .agent_pools
            .write()
            .await
            .insert(request.client_id.clone(), default_pool_id);
    }
}

fn claim_response(
    settings: &EnrollmentSettings,
    request: &ClaimEnrollmentRequest,
    display_name: String,
    tags: Vec<String>,
) -> ClaimEnrollmentResponse {
    ClaimEnrollmentResponse {
        client_id: request.client_id.clone(),
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
