ALTER TABLE jobs
    ADD COLUMN IF NOT EXISTS idempotency_key TEXT,
    ADD COLUMN IF NOT EXISTS reconnect_policy JSONB NOT NULL DEFAULT '{"duplicate_delivery":"ignore_completed","resume_outputs":true,"cancel_on_disconnect":false}'::jsonb;

CREATE UNIQUE INDEX IF NOT EXISTS jobs_actor_idempotency_key_idx
    ON jobs(actor_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE TABLE command_templates (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    command_type TEXT NOT NULL,
    operation JSONB NOT NULL,
    defaults JSONB NOT NULL DEFAULT '{}',
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'provider', 'tag', 'client')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (jsonb_typeof(operation) = 'object'),
    CHECK (jsonb_typeof(defaults) = 'object')
);

CREATE UNIQUE INDEX command_templates_global_name_idx
    ON command_templates(name)
    WHERE scope_kind = 'global';

CREATE UNIQUE INDEX command_templates_scoped_name_idx
    ON command_templates(scope_kind, scope_value, name)
    WHERE scope_kind <> 'global';

CREATE INDEX command_templates_lookup_idx
    ON command_templates(scope_kind, scope_value, command_type, updated_at DESC);
