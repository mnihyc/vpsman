ALTER TABLE job_outputs
    ADD COLUMN storage TEXT NOT NULL DEFAULT 'inline',
    ADD COLUMN object_key TEXT,
    ADD COLUMN data_sha256_hex TEXT,
    ADD COLUMN data_size_bytes BIGINT;

CREATE UNIQUE INDEX job_outputs_object_key_unique
    ON job_outputs(object_key)
    WHERE object_key IS NOT NULL;
