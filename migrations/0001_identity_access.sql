CREATE TABLE operators (
    id UUID PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    totp_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    role TEXT NOT NULL,
    scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
    totp_secret_ciphertext_hex TEXT,
    totp_secret_nonce_hex TEXT,
    totp_secret_salt_hex TEXT,
    preferences JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT operators_scopes_json_array CHECK (jsonb_typeof(scopes) = 'array'),
    CONSTRAINT operators_totp_secret_hex CHECK (
        (
            totp_secret_ciphertext_hex IS NULL
            AND totp_secret_nonce_hex IS NULL
            AND totp_secret_salt_hex IS NULL
        )
        OR (
            totp_secret_ciphertext_hex ~ '^[0-9a-f]+$'
            AND totp_secret_nonce_hex ~ '^[0-9a-f]{24}$'
            AND totp_secret_salt_hex ~ '^[0-9a-f]{32}$'
        )
    ),
    CONSTRAINT operators_preferences_json_object CHECK (jsonb_typeof(preferences) = 'object')
);

CREATE TABLE clients (
    id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    public_key BYTEA NOT NULL,
    status TEXT NOT NULL DEFAULT 'unknown',
    agent_version TEXT,
    os_release TEXT,
    arch TEXT,
    capabilities JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_seen_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE tags (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE client_tags (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    tag_id UUID NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    PRIMARY KEY (client_id, tag_id)
);

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

CREATE INDEX operator_sessions_access_token_hash_idx
    ON operator_sessions (access_token_hash);

CREATE INDEX operator_sessions_refresh_token_hash_idx
    ON operator_sessions (refresh_token_hash);

CREATE TABLE enrollment_tokens (
    id UUID PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    token_prefix TEXT NOT NULL,
    created_by UUID REFERENCES operators(id),
    default_tags JSONB NOT NULL DEFAULT '[]'::jsonb,
    default_display_name TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ,
    used_by_client_id TEXT REFERENCES clients(id),
    purpose TEXT NOT NULL DEFAULT 'provision',
    allowed_client_id TEXT REFERENCES clients(id),
    requires_existing_client BOOLEAN NOT NULL DEFAULT FALSE,
    preserve_existing_assignments BOOLEAN NOT NULL DEFAULT TRUE,
    expected_old_public_key_sha256_hex TEXT,
    unmanaged_update_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    unmanaged_update_version_url TEXT NOT NULL DEFAULT 'https://github.com/mnihyc/vpsman/releases/latest/download/version.json',
    unmanaged_update_interval_secs BIGINT NOT NULL DEFAULT 86400,
    unmanaged_update_jitter_secs BIGINT NOT NULL DEFAULT 86400,
    unmanaged_update_activate BOOLEAN NOT NULL DEFAULT TRUE,
    unmanaged_update_restart_agent BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX enrollment_tokens_unused_expires_idx
    ON enrollment_tokens (expires_at)
    WHERE used_at IS NULL;

CREATE INDEX enrollment_tokens_allowed_client_idx
    ON enrollment_tokens (allowed_client_id, created_at DESC);

CREATE TABLE gateway_sessions (
    id UUID PRIMARY KEY,
    gateway_id TEXT NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    noise_public_key_hex TEXT,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    end_reason TEXT
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

CREATE TABLE audit_logs (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    command_hash TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
