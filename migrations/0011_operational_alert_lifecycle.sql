-- Persist every non-policy Fleet alert as an explicit episode. Human triage
-- remains in fleet_alert_states and is deliberately not a lifecycle owner.

-- Establish the maintenance-window handoff in the same client-first order as
-- ingest/session writers before taking downstream DDL locks. EXCLUSIVE still
-- permits ordinary reads while draining SELECT FOR UPDATE and writes.
LOCK TABLE clients IN EXCLUSIVE MODE;

ALTER TABLE telemetry_tunnels
    ADD COLUMN telemetry_topology_identity_hash TEXT,
    ADD COLUMN telemetry_runtime_evidence_identity_hash TEXT;

-- Only rows that existed while this migration held the telemetry table lock
-- are eligible for the one-time NULL-identity backfill. Rows written after
-- the DDL handoff receive the FALSE default and cannot be classified as
-- pre-upgrade evidence.
ALTER TABLE telemetry_tunnels
    ADD COLUMN operational_alert_legacy_identity BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE telemetry_tunnels
SET operational_alert_legacy_identity = TRUE;

-- Pair legacy telemetry only with a plan that is itself unchanged across the
-- lifecycle handoff. PostgreSQL installs the trigger before releasing the
-- migration's table lock.
ALTER TABLE tunnel_plans
    ADD COLUMN operational_alert_legacy_runtime_identity BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN operational_alert_runtime_boundary_at TIMESTAMPTZ;

UPDATE tunnel_plans
SET operational_alert_legacy_runtime_identity = TRUE,
    operational_alert_runtime_boundary_at = updated_at;

ALTER TABLE tunnel_plans
    ALTER COLUMN operational_alert_runtime_boundary_at SET DEFAULT clock_timestamp(),
    ALTER COLUMN operational_alert_runtime_boundary_at SET NOT NULL;

CREATE FUNCTION invalidate_tunnel_plan_legacy_runtime_identity()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.plan IS DISTINCT FROM NEW.plan
       OR OLD.builtin_credentials IS DISTINCT FROM NEW.builtin_credentials
       OR OLD.enabled IS DISTINCT FROM NEW.enabled
       OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at THEN
        NEW.operational_alert_legacy_runtime_identity := FALSE;
        NEW.operational_alert_runtime_boundary_at := clock_timestamp();
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER tunnel_plans_legacy_runtime_identity_trigger
BEFORE UPDATE OF plan, builtin_credentials, enabled, deleted_at ON tunnel_plans
FOR EACH ROW EXECUTE FUNCTION invalidate_tunnel_plan_legacy_runtime_identity();

-- Authoritative client condition/session boundaries use DB wall-clock time and
-- a DDL-fenced legacy marker so the stopped-writer cutover has one exact
-- provenance boundary.
ALTER TABLE clients
    ADD COLUMN operational_alert_status_at TIMESTAMPTZ;

UPDATE clients client
SET operational_alert_status_at = COALESCE(
    (
        SELECT max(history.created_at)
        FROM client_status_history history
        WHERE history.client_id = client.id
          AND history.to_status = client.status
    ),
    client.last_seen_at,
    client.created_at
);

ALTER TABLE clients
    ALTER COLUMN operational_alert_status_at SET NOT NULL,
    ADD COLUMN operational_alert_legacy_status BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN operational_alert_tunnel_boundary_at TIMESTAMPTZ;

UPDATE clients
SET operational_alert_legacy_status = TRUE;

-- Existing active gateway sessions are authoritative tunnel-evidence
-- boundaries too. Seed the latest one before installing the transition
-- trigger so pre-session telemetry cannot be attributed to the active
-- runtime during the one-time legacy backfill.
UPDATE clients client
SET operational_alert_tunnel_boundary_at = GREATEST(
    client.operational_alert_status_at,
    COALESCE(
        (
            SELECT max(session.started_at)
            FROM gateway_sessions session
            WHERE session.client_id = client.id
              AND session.status = 'active'
        ),
        client.operational_alert_status_at
    )
);

CREATE FUNCTION stamp_client_operational_alert_boundaries()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.operational_alert_status_at := clock_timestamp();
        NEW.operational_alert_legacy_status := FALSE;
        NEW.operational_alert_tunnel_boundary_at := clock_timestamp();
    ELSE
        IF OLD.status IS DISTINCT FROM NEW.status THEN
            NEW.operational_alert_status_at := clock_timestamp();
            NEW.operational_alert_legacy_status := FALSE;
            NEW.operational_alert_tunnel_boundary_at := clock_timestamp();
        ELSIF OLD.process_incarnation_id IS DISTINCT FROM NEW.process_incarnation_id THEN
            NEW.operational_alert_tunnel_boundary_at := clock_timestamp();
        END IF;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER clients_operational_alert_boundaries_insert_trigger
BEFORE INSERT ON clients
FOR EACH ROW EXECUTE FUNCTION stamp_client_operational_alert_boundaries();

CREATE TRIGGER clients_operational_alert_boundaries_update_trigger
BEFORE UPDATE OF status, process_incarnation_id ON clients
FOR EACH ROW EXECUTE FUNCTION stamp_client_operational_alert_boundaries();

-- A new or reactivated active session is a runtime boundary even when client status
-- and process incarnation are unchanged. An idempotent replay of the same
-- already-active session deliberately does not advance the boundary.
CREATE FUNCTION stamp_gateway_session_operational_alert_boundary()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.status = 'active'
       AND (
           TG_OP = 'INSERT'
           OR OLD.status IS DISTINCT FROM 'active'
           OR OLD.client_id IS DISTINCT FROM NEW.client_id
       ) THEN
        UPDATE clients
        SET operational_alert_tunnel_boundary_at = clock_timestamp()
        WHERE id = NEW.client_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER gateway_sessions_operational_alert_boundary_trigger
AFTER INSERT OR UPDATE OF status, client_id ON gateway_sessions
FOR EACH ROW EXECUTE FUNCTION stamp_gateway_session_operational_alert_boundary();

ALTER TABLE telemetry_tunnels
    ADD CONSTRAINT telemetry_tunnels_topology_identity_hash_check CHECK (
        telemetry_topology_identity_hash IS NULL
        OR telemetry_topology_identity_hash ~ '^[0-9a-f]{64}$'
    ),
    ADD CONSTRAINT telemetry_tunnels_runtime_evidence_identity_hash_check CHECK (
        telemetry_runtime_evidence_identity_hash IS NULL
        OR telemetry_runtime_evidence_identity_hash ~ '^[0-9a-f]{64}$'
    );

-- Lifecycle cutover times are stamped at the actual transition; now() reflects
-- transaction start and is not an authoritative occurrence clock.
ALTER TABLE jobs
    ADD COLUMN alert_terminal_at TIMESTAMPTZ;

UPDATE jobs
SET alert_terminal_at = COALESCE(completed_at, created_at)
WHERE status IN (
    'partial_success', 'canceled', 'rejected', 'failed',
    'agent_timeout', 'control_timeout'
);

CREATE FUNCTION set_job_alert_terminal_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.status IN (
        'partial_success', 'canceled', 'rejected', 'failed',
        'agent_timeout', 'control_timeout'
    ) THEN
        IF TG_OP = 'INSERT'
           OR OLD.status NOT IN (
               'partial_success', 'canceled', 'rejected', 'failed',
               'agent_timeout', 'control_timeout'
           )
           OR OLD.alert_terminal_at IS NULL THEN
            NEW.alert_terminal_at := clock_timestamp();
        END IF;
    ELSE
        NEW.alert_terminal_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER jobs_alert_terminal_at_trigger
BEFORE INSERT OR UPDATE OF status, completed_at ON jobs
FOR EACH ROW EXECUTE FUNCTION set_job_alert_terminal_at();

ALTER TABLE jobs
    ADD CONSTRAINT jobs_alert_terminal_at_check CHECK (
        (status IN (
            'partial_success', 'canceled', 'rejected', 'failed',
            'agent_timeout', 'control_timeout'
        )) = (alert_terminal_at IS NOT NULL)
    );

CREATE INDEX jobs_alert_terminal_at_idx
    ON jobs (alert_terminal_at DESC, id DESC)
    WHERE alert_terminal_at IS NOT NULL;

ALTER TABLE job_targets
    ADD COLUMN capability_alert_at TIMESTAMPTZ;

UPDATE job_targets target
SET capability_alert_at = COALESCE(
    target.completed_at,
    target.started_at,
    job.created_at
)
FROM jobs job
WHERE job.id = target.job_id
  AND target.status = 'skipped'
  AND target.capability_degraded_reason IS NOT NULL
  AND target.capability_degraded_hint IS NOT NULL;

CREATE FUNCTION set_job_target_capability_alert_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.status = 'skipped'
       AND NEW.capability_degraded_reason IS NOT NULL
       AND NEW.capability_degraded_hint IS NOT NULL THEN
        IF TG_OP = 'INSERT'
           OR OLD.status IS DISTINCT FROM NEW.status
           OR OLD.capability_degraded_reason IS DISTINCT FROM NEW.capability_degraded_reason
           OR OLD.capability_degraded_hint IS DISTINCT FROM NEW.capability_degraded_hint
           OR OLD.capability_alert_at IS NULL THEN
            NEW.capability_alert_at := clock_timestamp();
        END IF;
    ELSE
        NEW.capability_alert_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER job_targets_capability_alert_at_trigger
BEFORE INSERT OR UPDATE OF status, capability_degraded_reason, capability_degraded_hint
ON job_targets
FOR EACH ROW EXECUTE FUNCTION set_job_target_capability_alert_at();

ALTER TABLE job_targets
    ADD CONSTRAINT job_targets_capability_alert_at_check CHECK (
        (
            status = 'skipped'
            AND capability_degraded_reason IS NOT NULL
            AND capability_degraded_hint IS NOT NULL
        ) = (capability_alert_at IS NOT NULL)
    );

CREATE INDEX job_targets_capability_alert_at_idx
    ON job_targets (capability_alert_at DESC, job_id DESC, client_id ASC)
    WHERE capability_alert_at IS NOT NULL;

-- The request creation time is not the failure occurrence time. Persist the
-- authoritative terminal transition so post-cutover source repair classifies
-- the occurrence against the event-source cutoff exactly.
ALTER TABLE backup_requests
    ADD COLUMN terminal_at TIMESTAMPTZ;

UPDATE backup_requests request
SET terminal_at = COALESCE(
    (
        SELECT max(audit.created_at)
        FROM audit_logs audit
        WHERE audit.target = 'backup_request:' || request.id::text
          AND audit.action IN ('backup.execution_failed', 'backup.execution_canceled')
    ),
    request.created_at
)
WHERE request.status IN ('execution_failed', 'execution_canceled');

CREATE FUNCTION set_backup_request_terminal_at()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.status IN ('execution_failed', 'execution_canceled') THEN
        IF TG_OP = 'INSERT' OR OLD.status IS DISTINCT FROM NEW.status THEN
            NEW.terminal_at := clock_timestamp();
        END IF;
    ELSE
        NEW.terminal_at := NULL;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER backup_requests_terminal_at_trigger
BEFORE INSERT OR UPDATE OF status ON backup_requests
FOR EACH ROW EXECUTE FUNCTION set_backup_request_terminal_at();

ALTER TABLE backup_requests
    ADD CONSTRAINT backup_requests_terminal_at_check CHECK (
        (status IN ('execution_failed', 'execution_canceled'))
        = (terminal_at IS NOT NULL)
    );

CREATE INDEX backup_requests_failed_terminal_idx
    ON backup_requests (terminal_at DESC, id DESC)
    WHERE status = 'execution_failed';

CREATE TABLE operational_alert_lifecycle_meta (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    event_source_cutoff_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    condition_client_cursor TEXT,
    backfill_completed BOOLEAN NOT NULL DEFAULT FALSE,
    completed_at TIMESTAMPTZ,
    CONSTRAINT operational_alert_lifecycle_meta_completion_check CHECK (
        backfill_completed = (completed_at IS NOT NULL)
    )
);

INSERT INTO operational_alert_lifecycle_meta (singleton, backfill_completed)
VALUES (TRUE, FALSE)
ON CONFLICT (singleton) DO NOTHING;

CREATE TABLE operational_alert_episodes (
    id UUID PRIMARY KEY,
    public_id TEXT NOT NULL UNIQUE,
    producer_kind TEXT NOT NULL,
    natural_key TEXT NOT NULL,
    record_kind TEXT NOT NULL,
    trigger_generation BIGINT NOT NULL,
    trigger_severity TEXT NOT NULL,
    trigger_category TEXT NOT NULL,
    severity TEXT NOT NULL,
    category TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    client_id TEXT,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    source_status TEXT NOT NULL,
    evidence JSONB NOT NULL,
    lifecycle_state TEXT NOT NULL,
    triggered_at TIMESTAMPTZ NOT NULL,
    last_confirmed_at TIMESTAMPTZ,
    resolved_at TIMESTAMPTZ,
    resolution_reason TEXT,
    resolution_note TEXT,
    resolution_actor_id UUID REFERENCES operators(id),
    backfilled BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT operational_alert_episode_identity_key
        UNIQUE (producer_kind, natural_key, trigger_generation),
    CONSTRAINT operational_alert_episode_producer_check CHECK (
        producer_kind IN (
            'agent_status',
            'agent_access',
            'tunnel_adapter',
            'tunnel_traffic',
            'job',
            'backup_request',
            'capability_degraded'
        )
    ),
    CONSTRAINT operational_alert_episode_record_kind_check
        CHECK (record_kind IN ('condition', 'event')),
    CONSTRAINT operational_alert_episode_generation_check
        CHECK (trigger_generation >= 1),
    CONSTRAINT operational_alert_episode_public_id_check CHECK (
        length(btrim(public_id)) BETWEEN 1 AND 192
    ),
    CONSTRAINT operational_alert_episode_natural_key_check CHECK (
        length(btrim(natural_key)) BETWEEN 1 AND 512
    ),
    CONSTRAINT operational_alert_episode_source_status_check CHECK (
        length(btrim(source_status)) BETWEEN 1 AND 256
    ),
    CONSTRAINT operational_alert_episode_title_check CHECK (
        length(btrim(title)) BETWEEN 1 AND 256
    ),
    CONSTRAINT operational_alert_episode_detail_check CHECK (
        length(btrim(detail)) BETWEEN 1 AND 4096
    ),
    CONSTRAINT operational_alert_episode_resolution_reason_check CHECK (
        resolution_reason IS NULL
        OR resolution_reason IN (
            'condition_recovered',
            'source_scope_exited',
            'operator_resolved'
        )
    ),
    CONSTRAINT operational_alert_episode_resolution_note_check CHECK (
        resolution_note IS NULL
        OR length(btrim(resolution_note)) BETWEEN 1 AND 1024
    ),
    CONSTRAINT operational_alert_episode_severity_check
        CHECK (severity IN ('info', 'warning', 'critical')),
    CONSTRAINT operational_alert_episode_trigger_severity_check
        CHECK (trigger_severity IN ('info', 'warning', 'critical')),
    CONSTRAINT operational_alert_episode_category_check CHECK (
        category IN (
            'agent_status',
            'network',
            'backup',
            'agent_update',
            'job',
            'capability_degraded'
        )
    ),
    CONSTRAINT operational_alert_episode_trigger_category_check CHECK (
        trigger_category IN (
            'agent_status',
            'network',
            'backup',
            'agent_update',
            'job',
            'capability_degraded'
        )
    ),
    CONSTRAINT operational_alert_episode_evidence_object_check
        CHECK (jsonb_typeof(evidence) = 'object'),
    CONSTRAINT operational_alert_episode_lifecycle_check CHECK (
        (
            lifecycle_state IN ('triggered', 'persisting')
            AND last_confirmed_at IS NOT NULL
            AND last_confirmed_at >= triggered_at
            AND resolved_at IS NULL
            AND resolution_reason IS NULL
            AND resolution_note IS NULL
            AND resolution_actor_id IS NULL
        ) OR (
            lifecycle_state = 'unknown'
            AND record_kind = 'condition'
            AND last_confirmed_at IS NOT NULL
            AND last_confirmed_at >= triggered_at
            AND resolved_at IS NULL
            AND resolution_reason IS NULL
            AND resolution_note IS NULL
            AND resolution_actor_id IS NULL
        ) OR (
            lifecycle_state = 'resolved'
            AND last_confirmed_at IS NOT NULL
            AND resolved_at IS NOT NULL
            AND last_confirmed_at >= triggered_at
            AND resolved_at >= last_confirmed_at
            AND resolution_reason IS NOT NULL
            AND (
                (
                    record_kind = 'event'
                    AND resolution_reason = 'operator_resolved'
                    AND resolution_note IS NOT NULL
                    AND resolution_actor_id IS NOT NULL
                ) OR (
                    record_kind = 'condition'
                    AND resolution_reason IN ('condition_recovered', 'source_scope_exited')
                    AND resolution_note IS NULL
                    AND resolution_actor_id IS NULL
                )
            )
        )
    )
);

CREATE UNIQUE INDEX operational_alert_episodes_one_current_idx
    ON operational_alert_episodes (producer_kind, natural_key)
    WHERE resolved_at IS NULL;

CREATE UNIQUE INDEX operational_alert_episodes_event_source_once_idx
    ON operational_alert_episodes (producer_kind, natural_key)
    WHERE record_kind = 'event';

CREATE INDEX operational_alert_episodes_current_priority_idx
    ON operational_alert_episodes (
        (CASE record_kind WHEN 'condition' THEN 0 ELSE 1 END),
        (CASE
            WHEN lifecycle_state IN ('triggered', 'persisting') THEN 0
            ELSE 1
        END),
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        triggered_at DESC,
        id DESC
    )
    WHERE resolved_at IS NULL;

CREATE INDEX operational_alert_episodes_current_client_priority_idx
    ON operational_alert_episodes (
        client_id,
        (CASE record_kind WHEN 'condition' THEN 0 ELSE 1 END),
        (CASE
            WHEN lifecycle_state IN ('triggered', 'persisting') THEN 0
            ELSE 1
        END),
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        triggered_at DESC,
        id DESC
    )
    WHERE resolved_at IS NULL;

CREATE INDEX operational_alert_episodes_history_idx
    ON operational_alert_episodes (triggered_at DESC, id DESC);

CREATE INDEX operational_alert_episodes_history_client_idx
    ON operational_alert_episodes (client_id, triggered_at DESC, id DESC);

CREATE INDEX operational_alert_episodes_unresolved_event_cursor_idx
    ON operational_alert_episodes (triggered_at DESC, id DESC)
    WHERE record_kind = 'event' AND resolved_at IS NULL;

CREATE INDEX operational_alert_episodes_unresolved_event_client_cursor_idx
    ON operational_alert_episodes (client_id, triggered_at DESC, id DESC)
    WHERE record_kind = 'event' AND resolved_at IS NULL;

-- These rows were written by the webhook worker as if a machine incident were
-- human triage. They have no FleetAlert owner and retention later forged an
-- acknowledgement. Remove exactly that invalid namespace and preserve every
-- real operator-owned triage row.
DELETE FROM fleet_alert_states
WHERE alert_id LIKE 'webhook\_delivery:%' ESCAPE '\';

-- Operational episodes are append-only in this migration. Unresolved episodes
-- are never retention candidates. No resolved-history or triage pruning is
-- performed until a separately declared owner/policy is implemented.
