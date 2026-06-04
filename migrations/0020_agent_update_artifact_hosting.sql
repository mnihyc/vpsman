ALTER TABLE agent_update_releases
    ADD COLUMN artifact_object_key TEXT,
    ADD COLUMN artifact_download_path TEXT;

CREATE INDEX agent_update_releases_artifact_object_idx
    ON agent_update_releases (artifact_object_key)
    WHERE artifact_object_key IS NOT NULL;
