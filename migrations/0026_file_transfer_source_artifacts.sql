CREATE TABLE file_transfer_source_artifacts (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    object_key TEXT NOT NULL,
    sha256_hex TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    created_by UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT file_transfer_source_artifacts_sha256_hex_check
        CHECK (sha256_hex ~ '^[0-9a-f]{64}$'),
    CONSTRAINT file_transfer_source_artifacts_size_check
        CHECK (size_bytes >= 0)
);

CREATE INDEX file_transfer_source_artifacts_created_idx
    ON file_transfer_source_artifacts (created_at DESC, id DESC);

CREATE INDEX file_transfer_source_artifacts_hash_idx
    ON file_transfer_source_artifacts (sha256_hex, size_bytes);
