ALTER TABLE agent_update_releases
    ADD COLUMN rollback_artifact_sha256_hex TEXT,
    ADD COLUMN rollback_artifact_signature_provided BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN rollback_artifact_signature_sha256_hex TEXT,
    ADD COLUMN rollback_artifact_signing_key_sha256_hex TEXT,
    ADD COLUMN rollback_artifact_url_sha256_hex TEXT,
    ADD COLUMN rollback_artifact_object_key TEXT,
    ADD COLUMN rollback_artifact_download_path TEXT,
    ADD COLUMN rollback_size_bytes BIGINT;

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
