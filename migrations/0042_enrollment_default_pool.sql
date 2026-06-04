ALTER TABLE enrollment_tokens
    ADD COLUMN default_pool_id UUID REFERENCES resource_pools(id),
    ADD COLUMN default_display_name TEXT;

CREATE INDEX enrollment_tokens_default_pool_idx
    ON enrollment_tokens (default_pool_id, created_at DESC)
    WHERE default_pool_id IS NOT NULL;
