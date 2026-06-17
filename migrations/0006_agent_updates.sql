CREATE TABLE agent_update_releases (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    channel TEXT NOT NULL,
    status TEXT NOT NULL,
    artifact_sha256_hex TEXT NOT NULL,
    artifact_url_sha256_hex TEXT NOT NULL,
    size_bytes BIGINT,
    rollback_artifact_sha256_hex TEXT,
    rollback_artifact_url_sha256_hex TEXT,
    rollback_size_bytes BIGINT,
    notes TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (name, version, channel),
    CONSTRAINT agent_update_releases_status_check
        CHECK (status IN ('published_external'))
);

CREATE INDEX agent_update_releases_channel_created_idx
    ON agent_update_releases (channel, created_at DESC, id DESC);

CREATE INDEX agent_update_releases_artifact_idx
    ON agent_update_releases (
        artifact_sha256_hex,
        created_at DESC
    );

CREATE INDEX agent_update_releases_rollback_artifact_idx
    ON agent_update_releases (
        rollback_artifact_sha256_hex,
        created_at DESC
    )
    WHERE rollback_artifact_sha256_hex IS NOT NULL;
