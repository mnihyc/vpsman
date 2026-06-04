ALTER TABLE enrollment_tokens
    ADD COLUMN unmanaged_update_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN unmanaged_update_version_url TEXT NOT NULL DEFAULT 'https://github.com/mnihyc/vpsman/releases/latest/download/version.json',
    ADD COLUMN unmanaged_update_interval_secs BIGINT NOT NULL DEFAULT 86400,
    ADD COLUMN unmanaged_update_jitter_secs BIGINT NOT NULL DEFAULT 86400,
    ADD COLUMN unmanaged_update_activate BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN unmanaged_update_restart_agent BOOLEAN NOT NULL DEFAULT TRUE;
