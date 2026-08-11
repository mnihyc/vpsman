CREATE TABLE operators (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    totp_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    status TEXT NOT NULL DEFAULT 'active',
    role TEXT NOT NULL,
    scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
    totp_secret_ciphertext_hex TEXT,
    totp_secret_nonce_hex TEXT,
    totp_secret_salt_hex TEXT,
    totp_last_accepted_step BIGINT,
    preferences JSONB NOT NULL DEFAULT '{}'::jsonb,
    session_refresh_ttl_secs BIGINT NOT NULL DEFAULT 31536000,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,
    CONSTRAINT operators_scopes_json_array CHECK (jsonb_typeof(scopes) = 'array'),
    CONSTRAINT operators_status_check CHECK (status IN ('active', 'disabled', 'deleted')),
    CONSTRAINT operators_session_refresh_ttl_check CHECK (
        session_refresh_ttl_secs BETWEEN 86400 AND 315360000
    ),
    CONSTRAINT operators_totp_secret_hex CHECK (
        (
            totp_secret_ciphertext_hex IS NULL
            AND totp_secret_nonce_hex IS NULL
            AND totp_secret_salt_hex IS NULL
        )
        OR (
            totp_secret_ciphertext_hex IS NOT NULL
            AND totp_secret_nonce_hex IS NOT NULL
            AND totp_secret_salt_hex IS NOT NULL
            AND totp_secret_ciphertext_hex ~ '^[0-9a-f]+$'
            AND totp_secret_nonce_hex ~ '^[0-9a-f]{24}$'
            AND totp_secret_salt_hex ~ '^[0-9a-f]{32}$'
        )
    ),
    CONSTRAINT operators_totp_state_check CHECK (
        (
            totp_enabled
            AND totp_secret_ciphertext_hex IS NOT NULL
            AND totp_last_accepted_step IS NOT NULL
            AND totp_last_accepted_step >= 0
        )
        OR (
            NOT totp_enabled
            AND totp_last_accepted_step IS NULL
        )
    ),
    CONSTRAINT operators_preferences_json_object CHECK (jsonb_typeof(preferences) = 'object')
);

CREATE TABLE clients (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    public_key BYTEA NOT NULL,
    status TEXT NOT NULL DEFAULT 'never',
    agent_version TEXT,
    internal_build_number BIGINT NOT NULL DEFAULT 1,
    process_incarnation_id UUID,
    os_release TEXT,
    arch TEXT,
    cpu_model TEXT,
    kernel_release TEXT,
    virtualization TEXT,
    system_reported_at TIMESTAMPTZ,
    capabilities JSONB NOT NULL DEFAULT '{}'::jsonb,
    registration_ip INET,
    last_ip INET,
    last_seen_at TIMESTAMPTZ,
    stale_since TIMESTAMPTZ,
    stale_reason TEXT,
    stale_build_number BIGINT,
    hidden_at TIMESTAMPTZ,
    hidden_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    hidden_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT clients_status_check CHECK (status IN ('never', 'online', 'disconnected', 'offline', 'stale', 'revoked', 'deleted')),
    CONSTRAINT clients_deleted_visibility_check CHECK (
        (status = 'deleted') = (hidden_at IS NOT NULL)
    ),
    CONSTRAINT clients_internal_build_number_check CHECK (internal_build_number >= 1),
    CONSTRAINT clients_stale_build_number_check CHECK (stale_build_number IS NULL OR stale_build_number >= 1)
);

CREATE INDEX clients_visible_status_idx
    ON clients (status, last_seen_at DESC)
    WHERE hidden_at IS NULL;

CREATE UNIQUE INDEX clients_visible_display_name_key_idx
    ON clients (lower(btrim(display_name)))
    WHERE hidden_at IS NULL;

CREATE INDEX clients_visible_last_ip_idx
    ON clients (last_ip)
    WHERE hidden_at IS NULL;

CREATE UNIQUE INDEX clients_public_key_unique_idx
    ON clients (public_key)
    WHERE octet_length(public_key) > 0;

-- Operational code reads this projection. Lifecycle, identity, and historical
-- evidence code deliberately reads the clients table so tombstones remain
-- authoritative and auditable.
CREATE VIEW visible_clients AS
SELECT
    id,
    display_name,
    public_key,
    status,
    agent_version,
    internal_build_number,
    process_incarnation_id,
    os_release,
    arch,
    cpu_model,
    kernel_release,
    virtualization,
    system_reported_at,
    capabilities,
    registration_ip,
    last_ip,
    last_seen_at,
    stale_since,
    stale_reason,
    stale_build_number,
    hidden_at,
    hidden_by,
    hidden_reason,
    created_at
FROM clients
WHERE hidden_at IS NULL;

COMMENT ON VIEW visible_clients IS
    'Live operational VPS projection; use clients explicitly for lifecycle and historical evidence.';

CREATE TABLE client_status_history (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    from_status TEXT,
    to_status TEXT NOT NULL,
    reason TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT client_status_history_from_check
        CHECK (from_status IS NULL OR from_status IN ('never', 'online', 'disconnected', 'offline', 'stale', 'revoked', 'deleted')),
    CONSTRAINT client_status_history_to_check
        CHECK (to_status IN ('never', 'online', 'disconnected', 'offline', 'stale', 'revoked', 'deleted')),
    CONSTRAINT client_status_history_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX client_status_history_client_created_idx
    ON client_status_history (client_id, created_at DESC);

CREATE TABLE tags (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    display_order BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE client_tags (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    tag_id UUID NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (client_id, tag_id)
);

CREATE INDEX client_tags_tag_id_client_id_idx
    ON client_tags (tag_id, client_id);

CREATE TABLE operator_sessions (
    id UUID PRIMARY KEY,
    operator_id UUID NOT NULL REFERENCES operators(id) ON DELETE CASCADE,
    access_token_hash TEXT NOT NULL UNIQUE,
    refresh_token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    refresh_expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX operator_sessions_operator_id_idx
    ON operator_sessions (operator_id);

CREATE TABLE operator_auth_throttle (
    scope_kind TEXT NOT NULL,
    scope_key TEXT NOT NULL,
    failed_attempts BIGINT NOT NULL DEFAULT 0,
    window_started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_until TIMESTAMPTZ,
    last_failed_at TIMESTAMPTZ,
    last_failure_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (scope_kind, scope_key),
    CONSTRAINT operator_auth_throttle_scope_kind_check
        CHECK (scope_kind IN ('username', 'username_ip', 'ip')),
    CONSTRAINT operator_auth_throttle_failed_attempts_check
        CHECK (failed_attempts >= 0)
);

CREATE INDEX operator_auth_throttle_locked_idx
    ON operator_auth_throttle (locked_until)
    WHERE locked_until IS NOT NULL;

CREATE TABLE gateway_sessions (
    id UUID PRIMARY KEY,
    gateway_id TEXT NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    noise_public_key_hex TEXT,
    remote_ip INET,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    end_reason TEXT,
    CONSTRAINT gateway_sessions_status_check
        CHECK (status IN ('active', 'ended', 'expired'))
);

CREATE INDEX gateway_sessions_client_status_idx
    ON gateway_sessions (client_id, status, last_seen_at DESC);

CREATE INDEX gateway_sessions_gateway_seen_idx
    ON gateway_sessions (gateway_id, last_seen_at DESC, id DESC);

CREATE TABLE client_key_revocations (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    public_key_sha256_hex TEXT NOT NULL,
    reason TEXT,
    revoked_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (client_id, public_key_sha256_hex),
    CONSTRAINT client_key_revocations_sha256_hex_valid
        CHECK (public_key_sha256_hex ~ '^[0-9a-f]{64}$')
);

CREATE INDEX client_key_revocations_client_created_idx
    ON client_key_revocations (client_id, created_at DESC);

CREATE UNIQUE INDEX client_key_revocations_public_key_unique_idx
    ON client_key_revocations (public_key_sha256_hex);

CREATE TABLE audit_logs (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    command_hash TEXT,
    metadata JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT audit_logs_canonical_metadata CHECK (
        jsonb_typeof(metadata) = 'object'
        AND metadata ?& ARRAY['result', 'origin_kind', 'component']
        AND jsonb_typeof(metadata -> 'result') = 'string'
        AND jsonb_typeof(metadata -> 'origin_kind') = 'string'
        AND jsonb_typeof(metadata -> 'component') = 'string'
        AND btrim(metadata ->> 'result') <> ''
        AND btrim(metadata ->> 'origin_kind') <> ''
        AND btrim(metadata ->> 'component') <> ''
    )
);

CREATE INDEX audit_logs_created_idx
    ON audit_logs (created_at DESC, id DESC);
