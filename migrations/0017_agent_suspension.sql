-- Suspension is an inventory lifecycle state, not deletion or monitoring
-- history removal.  Nullable metadata columns keep this upgrade metadata-only
-- for every existing client row; the consistency constraints scan only the
-- small client/status-history catalogs and do not rewrite telemetry tables.
ALTER TABLE clients
    ADD COLUMN suspended_at TIMESTAMPTZ,
    ADD COLUMN suspended_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    ADD COLUMN suspended_reason TEXT,
    ADD COLUMN suspended_from_status TEXT;

ALTER TABLE clients
    DROP CONSTRAINT clients_status_check,
    ADD CONSTRAINT clients_status_check CHECK (
        status IN (
            'never', 'online', 'disconnected', 'offline', 'stale',
            'suspended', 'revoked', 'deleted'
        )
    ) NOT VALID,
    ADD CONSTRAINT clients_suspended_reason_check CHECK (
        suspended_reason IS NULL
        OR length(btrim(suspended_reason)) BETWEEN 1 AND 240
    ) NOT VALID,
    ADD CONSTRAINT clients_suspension_state_check CHECK (
        (
            status = 'suspended'
            AND suspended_at IS NOT NULL
            AND suspended_from_status IN ('never', 'disconnected', 'offline', 'stale')
        )
        OR (
            status <> 'suspended'
            AND suspended_at IS NULL
            AND suspended_by IS NULL
            AND suspended_reason IS NULL
            AND suspended_from_status IS NULL
        )
    ) NOT VALID;

ALTER TABLE clients VALIDATE CONSTRAINT clients_status_check;
ALTER TABLE clients VALIDATE CONSTRAINT clients_suspended_reason_check;
ALTER TABLE clients VALIDATE CONSTRAINT clients_suspension_state_check;

ALTER TABLE client_status_history
    DROP CONSTRAINT client_status_history_from_check,
    DROP CONSTRAINT client_status_history_to_check,
    ADD CONSTRAINT client_status_history_from_check CHECK (
        from_status IS NULL
        OR from_status IN (
            'never', 'online', 'disconnected', 'offline', 'stale',
            'suspended', 'revoked', 'deleted'
        )
    ) NOT VALID,
    ADD CONSTRAINT client_status_history_to_check CHECK (
        to_status IN (
            'never', 'online', 'disconnected', 'offline', 'stale',
            'suspended', 'revoked', 'deleted'
        )
    ) NOT VALID;

ALTER TABLE client_status_history
    VALIDATE CONSTRAINT client_status_history_from_check;
ALTER TABLE client_status_history
    VALIDATE CONSTRAINT client_status_history_to_check;

CREATE OR REPLACE VIEW visible_clients AS
SELECT
    id,
    display_name,
    public_key,
    status,
    agent_version,
    internal_build_number,
    process_incarnation_id,
    os_release,
    arch,
    cpu_model,
    kernel_release,
    virtualization,
    system_reported_at,
    capabilities,
    registration_ip,
    last_ip,
    last_seen_at,
    stale_since,
    stale_reason,
    stale_build_number,
    hidden_at,
    hidden_by,
    hidden_reason,
    created_at,
    suspended_at,
    suspended_by,
    suspended_reason,
    suspended_from_status
FROM clients
WHERE hidden_at IS NULL;

COMMENT ON COLUMN clients.suspended_at IS
    'Current operator-approved monitoring/dispatch suspension boundary.';
COMMENT ON COLUMN clients.suspended_by IS
    'Operator who initiated the current suspension; retained history owns past actors.';
COMMENT ON COLUMN clients.suspended_reason IS
    'Optional operator reason for the current suspension.';
COMMENT ON COLUMN clients.suspended_from_status IS
    'Non-online lifecycle state restored by manual unsuspend.';
