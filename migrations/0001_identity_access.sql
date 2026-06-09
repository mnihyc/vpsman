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
    status TEXT NOT NULL DEFAULT 'offline',
    agent_version TEXT,
    internal_build_number BIGINT NOT NULL DEFAULT 1,
    os_release TEXT,
    arch TEXT,
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
    CONSTRAINT clients_status_check CHECK (status IN ('online', 'offline', 'stale', 'revoked', 'deleted')),
    CONSTRAINT clients_internal_build_number_check CHECK (internal_build_number >= 1),
    CONSTRAINT clients_stale_build_number_check CHECK (stale_build_number IS NULL OR stale_build_number >= 1)
);

CREATE INDEX clients_visible_status_idx
    ON clients (status, last_seen_at DESC)
    WHERE hidden_at IS NULL;

CREATE INDEX clients_visible_last_ip_idx
    ON clients (last_ip)
    WHERE hidden_at IS NULL;

CREATE TABLE client_status_history (
    id UUID PRIMARY KEY,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    from_status TEXT,
    to_status TEXT NOT NULL,
    reason TEXT,
    metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT client_status_history_from_check
        CHECK (from_status IS NULL OR from_status IN ('online', 'offline', 'stale', 'revoked', 'deleted')),
    CONSTRAINT client_status_history_to_check
        CHECK (to_status IN ('online', 'offline', 'stale', 'revoked', 'deleted')),
    CONSTRAINT client_status_history_metadata_object CHECK (jsonb_typeof(metadata) = 'object')
);

CREATE INDEX client_status_history_client_created_idx
    ON client_status_history (client_id, created_at DESC);

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
