CREATE TABLE agent_update_releases (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    channel TEXT NOT NULL,
    status TEXT NOT NULL,
    artifact_sha256_hex TEXT NOT NULL,
    artifact_signature_provided BOOLEAN NOT NULL DEFAULT TRUE,
    artifact_signature_sha256_hex TEXT,
    artifact_signing_key_sha256_hex TEXT NOT NULL,
    artifact_url_sha256_hex TEXT,
    artifact_object_key TEXT,
    artifact_download_path TEXT,
    size_bytes BIGINT,
    rollback_artifact_sha256_hex TEXT,
    rollback_artifact_signature_provided BOOLEAN NOT NULL DEFAULT FALSE,
    rollback_artifact_signature_sha256_hex TEXT,
    rollback_artifact_signing_key_sha256_hex TEXT,
    rollback_artifact_url_sha256_hex TEXT,
    rollback_artifact_object_key TEXT,
    rollback_artifact_download_path TEXT,
    rollback_size_bytes BIGINT,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (name, version, channel)
);

CREATE INDEX agent_update_releases_channel_created_idx
    ON agent_update_releases (channel, created_at DESC, id DESC);

CREATE INDEX agent_update_releases_artifact_idx
    ON agent_update_releases (
        artifact_sha256_hex,
        artifact_signing_key_sha256_hex,
        created_at DESC
    );

CREATE INDEX agent_update_releases_artifact_object_idx
    ON agent_update_releases (artifact_object_key)
    WHERE artifact_object_key IS NOT NULL;

CREATE INDEX agent_update_releases_rollback_artifact_idx
    ON agent_update_releases (
        rollback_artifact_sha256_hex,
        rollback_artifact_signing_key_sha256_hex,
        created_at DESC
    )
    WHERE rollback_artifact_sha256_hex IS NOT NULL;

CREATE INDEX agent_update_releases_rollback_object_idx
    ON agent_update_releases (rollback_artifact_object_key)
    WHERE rollback_artifact_object_key IS NOT NULL;

CREATE TABLE agent_update_rollout_policies (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    channel TEXT,
    canary_count INTEGER,
    automation_health_gate TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'tag', 'provider')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (canary_count IS NULL OR canary_count BETWEEN 0 AND 10000),
    CHECK (
        automation_health_gate IS NULL
        OR automation_health_gate IN ('heartbeat_verified', 'manual_after_canary', 'manual_only')
    )
);

CREATE INDEX agent_update_rollout_policies_match_idx
    ON agent_update_rollout_policies (
        enabled,
        channel,
        scope_kind,
        scope_value,
        priority DESC,
        updated_at DESC
    );

CREATE TABLE agent_update_rollouts (
    id UUID PRIMARY KEY,
    job_id UUID NOT NULL UNIQUE REFERENCES jobs(id) ON DELETE CASCADE,
    actor_id UUID REFERENCES operators(id),
    status TEXT NOT NULL,
    artifact_sha256_hex TEXT NOT NULL,
    artifact_signature_provided BOOLEAN NOT NULL DEFAULT FALSE,
    artifact_signing_key_sha256_hex TEXT,
    target_count INTEGER NOT NULL,
    canary_count INTEGER NOT NULL DEFAULT 0,
    activation_policy TEXT NOT NULL,
    heartbeat_timeout_secs INTEGER,
    automation_status TEXT NOT NULL DEFAULT 'unreconciled',
    automation_next_action TEXT,
    automation_blocker TEXT,
    automation_targets TEXT[] NOT NULL DEFAULT '{}',
    automation_updated_at TIMESTAMPTZ,
    automation_paused BOOLEAN NOT NULL DEFAULT FALSE,
    automation_pause_reason TEXT,
    automation_health_gate TEXT NOT NULL DEFAULT 'heartbeat_verified',
    automation_lease_owner TEXT,
    automation_lease_expires_at TIMESTAMPTZ,
    rollout_policy_id UUID REFERENCES agent_update_rollout_policies(id),
    rollout_policy_name TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX agent_update_rollouts_status_created_idx
    ON agent_update_rollouts (status, created_at DESC, id DESC);

CREATE INDEX agent_update_rollouts_automation_idx
    ON agent_update_rollouts (automation_status, automation_updated_at DESC, id DESC);

CREATE INDEX agent_update_rollouts_automation_lease_idx
    ON agent_update_rollouts (automation_lease_expires_at, automation_paused, id);

CREATE TABLE agent_update_rollout_targets (
    rollout_id UUID NOT NULL REFERENCES agent_update_rollouts(id) ON DELETE CASCADE,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    status TEXT NOT NULL,
    exit_code INTEGER,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (rollout_id, client_id)
);

CREATE INDEX agent_update_rollout_targets_client_idx
    ON agent_update_rollout_targets (client_id, updated_at DESC);
