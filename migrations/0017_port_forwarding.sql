CREATE TABLE port_forward_rules (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id) ON DELETE SET NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE RESTRICT,
    name TEXT NOT NULL,
    protocol TEXT NOT NULL,
    target_ip INET NOT NULL,
    mappings JSONB NOT NULL,
    masquerade BOOLEAN NOT NULL DEFAULT TRUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    revision BIGINT NOT NULL DEFAULT 1 CHECK (revision >= 1),
    deleted_at TIMESTAMPTZ,
    deleted_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    deleted_reason TEXT,
    removal_confirmed_at TIMESTAMPTZ,
    forgotten_at TIMESTAMPTZ,
    forgotten_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    forget_reason TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT port_forward_rules_name_check
        CHECK (length(btrim(name)) BETWEEN 1 AND 128),
    CONSTRAINT port_forward_rules_protocol_check
        CHECK (protocol IN ('tcp', 'udp', 'both')),
    CONSTRAINT port_forward_rules_mappings_array_check
        CHECK (jsonb_typeof(mappings) = 'array')
);

CREATE UNIQUE INDEX port_forward_rules_active_name_idx
    ON port_forward_rules (client_id, name)
    WHERE deleted_at IS NULL;

CREATE INDEX port_forward_rules_client_state_idx
    ON port_forward_rules (client_id, enabled, updated_at DESC)
    WHERE forgotten_at IS NULL;

CREATE INDEX port_forward_rules_removal_pending_idx
    ON port_forward_rules (client_id, deleted_at)
    WHERE deleted_at IS NOT NULL
      AND removal_confirmed_at IS NULL
      AND forgotten_at IS NULL;

CREATE TABLE port_forward_runtime_state (
    client_id TEXT PRIMARY KEY REFERENCES clients(id) ON DELETE CASCADE,
    snapshot JSONB NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
