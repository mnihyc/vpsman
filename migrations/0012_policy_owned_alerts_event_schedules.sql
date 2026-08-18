-- Unify every Fleet alert behind an Alert Policy-owned episode and add the
-- durable lifecycle-event boundary consumed independently by webhooks and
-- alert-event schedules. This migration is maintenance-gated: application
-- writers must be stopped while it runs.

CREATE FUNCTION alert_policy_meta_condition_valid(value JSONB, allow_elapsed BOOLEAN)
RETURNS BOOLEAN
LANGUAGE plpgsql
IMMUTABLE
STRICT
AS $$
DECLARE
    kind TEXT;
BEGIN
    IF jsonb_typeof(value) <> 'object' THEN
        RETURN FALSE;
    END IF;
    kind := value->>'kind';
    IF kind = 'immediate' THEN
        RETURN value = '{"kind":"immediate"}'::jsonb;
    ELSIF kind = 'sustained' THEN
        RETURN value - 'kind' - 'seconds' = '{}'::jsonb
           AND jsonb_typeof(value->'seconds') = 'number'
           AND (value->>'seconds') ~ '^[0-9]+$'
           AND (value->>'seconds')::BIGINT BETWEEN 1 AND 2592000;
    ELSIF kind = 'count' THEN
        RETURN value - 'kind' - 'confirmations' - 'within_seconds' = '{}'::jsonb
           AND jsonb_typeof(value->'confirmations') = 'number'
           AND jsonb_typeof(value->'within_seconds') = 'number'
           AND (value->>'confirmations') ~ '^[0-9]+$'
           AND (value->>'within_seconds') ~ '^[0-9]+$'
           AND (value->>'confirmations')::INTEGER BETWEEN 1 AND 1000
           AND (value->>'within_seconds')::BIGINT BETWEEN 1 AND 2592000;
    ELSIF kind = 'elapsed_since_trigger' THEN
        RETURN allow_elapsed
           AND value - 'kind' - 'seconds' = '{}'::jsonb
           AND jsonb_typeof(value->'seconds') = 'number'
           AND (value->>'seconds') ~ '^[0-9]+$'
           AND (value->>'seconds')::BIGINT BETWEEN 1 AND 31536000;
    END IF;
    RETURN FALSE;
EXCEPTION
    WHEN numeric_value_out_of_range OR invalid_text_representation THEN
        RETURN FALSE;
END;
$$;

CREATE FUNCTION alert_uuid_array_is_unique_bounded(value UUID[], max_items INTEGER)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
AS $$
    SELECT cardinality(value) <= max_items
       AND cardinality(value) = (
           SELECT count(DISTINCT item)::INTEGER FROM unnest(value) AS item
       );
$$;

CREATE FUNCTION alert_jsonb_string_array_valid(value JSONB, max_items INTEGER)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
AS $$
    SELECT jsonb_typeof(value) = 'array'
       AND jsonb_array_length(value) BETWEEN 1 AND max_items
       AND NOT EXISTS (
           SELECT 1
           FROM jsonb_array_elements(value) AS item
           WHERE jsonb_typeof(item) <> 'string'
       );
$$;

-- Source writers take the shared transaction advisory for their stream before
-- reserving a sequence value. Rule/schedule arm mutations take the matching
-- exclusive advisory, drain every in-flight writer, then snapshot MAX(seq).
-- This proves prospective arming without serializing unrelated source writers
-- on one counter-row lock. Evaluators anti-join durable receipts and therefore
-- never mistake sequence allocation order for commit order.
CREATE SEQUENCE alert_policy_evidence_seq START WITH 1 CACHE 1;
CREATE SEQUENCE alert_lifecycle_event_seq START WITH 1 CACHE 1;

CREATE TABLE alert_policy_lifecycle_meta (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    cutover_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    startup_reconciled_at TIMESTAMPTZ,
    legacy_condition_bootstrap_completed BOOLEAN NOT NULL DEFAULT FALSE,
    legacy_event_bootstrap_completed BOOLEAN NOT NULL DEFAULT FALSE,
    evidence_retention_days INTEGER NOT NULL DEFAULT 30,
    evidence_pruned_through_seq BIGINT NOT NULL DEFAULT 0,
    lifecycle_retention_cursor_seq BIGINT NOT NULL DEFAULT 0,
    CHECK (evidence_retention_days BETWEEN 1 AND 3650),
    CHECK (evidence_pruned_through_seq >= 0),
    CHECK (lifecycle_retention_cursor_seq >= 0)
);

INSERT INTO alert_policy_lifecycle_meta (singleton)
VALUES (TRUE)
ON CONFLICT (singleton) DO NOTHING;

-- A deployment that already ran the 0011 application bootstrap must not
-- replay its bounded retained occurrences. Direct 0010->0012 upgrades keep
-- this false so the post-0012 startup reconciler performs that one quiet,
-- bounded bootstrap itself.
UPDATE alert_policy_lifecycle_meta lifecycle
SET legacy_condition_bootstrap_completed = operational.backfill_completed,
    legacy_event_bootstrap_completed = operational.backfill_completed
FROM operational_alert_lifecycle_meta operational
WHERE lifecycle.singleton AND operational.singleton;

-- Scope snapshots carry an explicit DB-monotonic revision. Raw source time
-- remains the state boundary, while selector metadata changes (including a
-- tag/rule round trip back to an earlier value) still produce a new causal
-- revision instead of being ordered by an opaque payload hash.
ALTER TABLE clients
    ADD COLUMN policy_scope_revision BIGINT NOT NULL DEFAULT 1,
    ADD CONSTRAINT clients_policy_scope_revision_check CHECK (
        policy_scope_revision >= 1
    );

CREATE FUNCTION stamp_client_policy_scope_revision()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF ROW(
        NEW.display_name, NEW.status, NEW.registration_ip, NEW.last_ip,
        NEW.last_seen_at, NEW.internal_build_number, NEW.stale_since,
        NEW.stale_reason, NEW.hidden_at
    ) IS DISTINCT FROM ROW(
        OLD.display_name, OLD.status, OLD.registration_ip, OLD.last_ip,
        OLD.last_seen_at, OLD.internal_build_number, OLD.stale_since,
        OLD.stale_reason, OLD.hidden_at
    ) THEN
        PERFORM pg_advisory_xact_lock_shared(
            hashtext('vpsman.alert_policy_evidence_arm')::bigint
        );
        NEW.policy_scope_revision := OLD.policy_scope_revision + 1;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER clients_policy_scope_revision_update_trigger
BEFORE UPDATE OF display_name, status, registration_ip, last_ip, last_seen_at,
    internal_build_number, stale_since, stale_reason, hidden_at ON clients
FOR EACH ROW EXECUTE FUNCTION stamp_client_policy_scope_revision();

CREATE FUNCTION bump_policy_scope_revision_for_assignment()
RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    affected_client_id TEXT;
BEGIN
    affected_client_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.client_id ELSE NEW.client_id END;
    PERFORM pg_advisory_xact_lock_shared(
        hashtext('vpsman.alert_policy_evidence_arm')::bigint
    );
    UPDATE clients
    SET policy_scope_revision = policy_scope_revision + 1
    WHERE id = affected_client_id;
    IF TG_OP = 'UPDATE' AND OLD.client_id IS DISTINCT FROM NEW.client_id THEN
        UPDATE clients
        SET policy_scope_revision = policy_scope_revision + 1
        WHERE id = OLD.client_id;
    END IF;
    RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$;

CREATE TRIGGER client_tags_policy_scope_revision_trigger
AFTER INSERT OR UPDATE OR DELETE ON client_tags
FOR EACH ROW EXECUTE FUNCTION bump_policy_scope_revision_for_assignment();

CREATE TRIGGER vps_rule_values_policy_scope_revision_trigger
AFTER INSERT OR UPDATE OR DELETE ON vps_rule_values
FOR EACH ROW EXECUTE FUNCTION bump_policy_scope_revision_for_assignment();

CREATE FUNCTION bump_policy_scope_revision_for_tag_name()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.name IS DISTINCT FROM OLD.name THEN
        PERFORM pg_advisory_xact_lock_shared(
            hashtext('vpsman.alert_policy_evidence_arm')::bigint
        );
        UPDATE clients client
        SET policy_scope_revision = client.policy_scope_revision + 1
        FROM client_tags assignment
        WHERE assignment.tag_id = NEW.id AND client.id = assignment.client_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER tags_policy_scope_revision_update_trigger
AFTER UPDATE OF name ON tags
FOR EACH ROW EXECUTE FUNCTION bump_policy_scope_revision_for_tag_name();

-- Existing metric policies become the generic rule shape without changing
-- their meaning. A null meta condition is the canonical Immediate value.
ALTER TABLE policy_rules
    RENAME COLUMN condition_expression TO trigger_condition_expression;

ALTER TABLE policy_rules
    ADD COLUMN rule_kind TEXT NOT NULL DEFAULT 'metric',
    ADD COLUMN evidence_source TEXT,
    ADD COLUMN correlation_mode TEXT NOT NULL DEFAULT 'natural_key',
    ADD COLUMN category TEXT,
    ADD COLUMN title_template TEXT,
    ADD COLUMN detail_template TEXT,
    ADD COLUMN trigger_meta_condition JSONB,
    ADD COLUMN resolve_condition_expression TEXT,
    ADD COLUMN resolve_meta_condition JSONB,
    ADD COLUMN system_seed_key TEXT,
    ADD COLUMN armed_after_evidence_seq BIGINT NOT NULL DEFAULT 0,
    ADD COLUMN armed_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp();

UPDATE policy_rules
SET evidence_source = 'telemetry.combined',
    category = CASE
        WHEN trigger_condition_expression ~ '(^|[^A-Za-z0-9_.])traffic[.]'
            THEN 'traffic'
        ELSE 'resource'
    END,
    title_template = CASE
        WHEN trigger_condition_expression ~ '(^|[^A-Za-z0-9_.])traffic[.]'
            THEN 'Traffic quota threshold reached'
        ELSE 'Resource policy threshold reached'
    END,
    detail_template = '{subject.display_name} matched policy condition {policy_rule.trigger_condition_expression}',
    trigger_meta_condition = CASE
        WHEN window_secs > 0
            THEN jsonb_build_object('kind', 'sustained', 'seconds', window_secs)
        ELSE NULL
    END,
    -- Existing condition rules recover as soon as their complete trigger
    -- expression becomes false. Null meta is canonical Immediate.
    resolve_condition_expression = NULL,
    resolve_meta_condition = NULL;

ALTER TABLE policy_rules
    ALTER COLUMN evidence_source SET NOT NULL,
    ALTER COLUMN category SET NOT NULL,
    ALTER COLUMN title_template SET NOT NULL,
    ALTER COLUMN detail_template SET NOT NULL,
    DROP CONSTRAINT IF EXISTS policy_rules_window_secs_check,
    DROP CONSTRAINT IF EXISTS policy_rules_condition_expression_check,
    DROP COLUMN window_secs,
    ADD CONSTRAINT policy_rules_rule_kind_check CHECK (
        rule_kind IN ('metric', 'state', 'occurrence')
    ),
    ADD CONSTRAINT policy_rules_evidence_source_check CHECK (
        evidence_source IN (
            'telemetry.combined', 'agent.status', 'agent.access',
            'tunnel.adapter', 'tunnel.traffic', 'job.terminal',
            'backup.failure', 'job.capability'
        )
    ),
    ADD CONSTRAINT policy_rules_source_kind_check CHECK (
        (evidence_source = 'telemetry.combined' AND rule_kind = 'metric')
        OR (
            evidence_source IN (
                'agent.status', 'agent.access', 'tunnel.adapter', 'tunnel.traffic'
            )
            AND rule_kind = 'state'
        )
        OR (
            evidence_source IN ('job.terminal', 'backup.failure', 'job.capability')
            AND rule_kind = 'occurrence'
        )
    ),
    ADD CONSTRAINT policy_rules_correlation_mode_check CHECK (
        correlation_mode IN ('natural_key', 'subject', 'global')
        AND (
            (rule_kind IN ('metric', 'state') AND correlation_mode = 'natural_key')
            OR (
                rule_kind = 'occurrence'
                AND (
                    (
                        COALESCE(trigger_meta_condition->>'kind', 'immediate') = 'count'
                        AND correlation_mode IN ('subject', 'global')
                    ) OR (
                        COALESCE(trigger_meta_condition->>'kind', 'immediate') = 'immediate'
                        AND correlation_mode = 'natural_key'
                    )
                )
            )
        )
    ),
    ADD CONSTRAINT policy_rules_category_check CHECK (
        category IN (
            'agent_status', 'network', 'backup', 'agent_update', 'job',
            'capability_degraded', 'traffic', 'resource'
        )
    ),
    ADD CONSTRAINT policy_rules_trigger_expression_check CHECK (
        length(btrim(trigger_condition_expression)) BETWEEN 1 AND 4096
    ),
    ADD CONSTRAINT policy_rules_resolve_expression_check CHECK (
        resolve_condition_expression IS NULL
        OR length(btrim(resolve_condition_expression)) BETWEEN 1 AND 4096
    ),
    ADD CONSTRAINT policy_rules_presentation_check CHECK (
        length(btrim(title_template)) BETWEEN 1 AND 256
        AND length(btrim(detail_template)) BETWEEN 1 AND 4096
    ),
    ADD CONSTRAINT policy_rules_system_seed_key_check CHECK (
        system_seed_key IS NULL
        OR (
            length(system_seed_key) BETWEEN 1 AND 128
            AND system_seed_key ~ '^[a-z][a-z0-9_.-]*$'
        )
    ),
    ADD CONSTRAINT policy_rules_armed_after_evidence_seq_check CHECK (
        armed_after_evidence_seq >= 0
    ),
    ADD CONSTRAINT policy_rules_trigger_meta_check CHECK (
        trigger_meta_condition IS NULL
        OR (
            alert_policy_meta_condition_valid(trigger_meta_condition, FALSE)
            AND trigger_meta_condition->>'kind' NOT IN ('immediate', 'elapsed_since_trigger')
            AND (
                rule_kind <> 'occurrence'
                OR trigger_meta_condition->>'kind' IN ('immediate', 'count')
            )
        )
    ),
    ADD CONSTRAINT policy_rules_occurrence_correlation_check CHECK (
        rule_kind <> 'occurrence'
        OR (
            COALESCE(trigger_meta_condition->>'kind', 'immediate') = 'count'
            AND correlation_mode IN ('subject', 'global')
            AND (evidence_source <> 'job.terminal' OR correlation_mode = 'global')
        )
        OR (
            COALESCE(trigger_meta_condition->>'kind', 'immediate') = 'immediate'
            AND correlation_mode = 'natural_key'
        )
    ),
    ADD CONSTRAINT policy_rules_resolve_phase_check CHECK (
        (
            rule_kind IN ('metric', 'state')
            AND (
                resolve_meta_condition IS NULL
                OR (
                    alert_policy_meta_condition_valid(resolve_meta_condition, FALSE)
                    AND resolve_meta_condition->>'kind' NOT IN ('immediate', 'elapsed_since_trigger')
                )
            )
        ) OR (
            rule_kind = 'occurrence'
            AND resolve_condition_expression IS NULL
            AND resolve_meta_condition IS NOT NULL
            AND alert_policy_meta_condition_valid(resolve_meta_condition, TRUE)
            AND resolve_meta_condition->>'kind' = 'elapsed_since_trigger'
        )
    );

CREATE UNIQUE INDEX policy_rules_system_seed_key_idx
    ON policy_rules (system_seed_key)
    WHERE system_seed_key IS NOT NULL;

-- Enabled system defaults use stable all-subject scope. Raw evidence decides
-- whether a condition matches; scope never removes an offline/revoked subject.
INSERT INTO policy_groups (
    id, name, enabled, selector_expression, notes
) VALUES (
    'c1000000-0000-4000-8000-000000000001',
    'System operational evidence policies',
    TRUE,
    '*',
    'Enabled deterministic defaults for normalized operational evidence. Operators may edit or disable these policies after upgrade.'
)
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id, group_id, sort_order, name, enabled, traffic_selector,
    trigger_condition_expression, severity, rule_kind, evidence_source, correlation_mode,
    category, title_template, detail_template, trigger_meta_condition,
    resolve_condition_expression, resolve_meta_condition, system_seed_key
) VALUES
    (
        'd1000000-0000-4000-8000-000000000001',
        'c1000000-0000-4000-8000-000000000001', 0,
        'Agent never connected', TRUE, NULL,
        'evidence.status = never', 'warning', 'state', 'agent.status', 'natural_key',
        'agent_status', 'Agent is not online',
        '{subject.display_name} currently reports {evidence.status}',
        '{"kind":"sustained","seconds":600}', NULL,
        '{"kind":"sustained","seconds":60}', 'agent.never_connected'
    ),
    (
        'd1000000-0000-4000-8000-000000000002',
        'c1000000-0000-4000-8000-000000000001', 1,
        'Agent connectivity degraded', TRUE, NULL,
        'evidence.status in [disconnected, stale]', 'warning', 'state', 'agent.status', 'natural_key',
        'agent_status', 'Agent is not online',
        '{subject.display_name} currently reports {evidence.status}',
        '{"kind":"sustained","seconds":120}', NULL,
        '{"kind":"sustained","seconds":60}', 'agent.connectivity_degraded'
    ),
    (
        'd1000000-0000-4000-8000-000000000003',
        'c1000000-0000-4000-8000-000000000001', 2,
        'Agent offline', TRUE, NULL,
        'evidence.status = offline', 'critical', 'state', 'agent.status', 'natural_key',
        'agent_status', 'Agent is not online',
        '{subject.display_name} currently reports {evidence.status}',
        '{"kind":"sustained","seconds":120}', NULL,
        '{"kind":"sustained","seconds":60}', 'agent.offline'
    ),
    (
        'd1000000-0000-4000-8000-000000000004',
        'c1000000-0000-4000-8000-000000000001', 3,
        'Agent access revoked', TRUE, NULL,
        'evidence.status = revoked', 'critical', 'state', 'agent.access', 'natural_key',
        'agent_status', 'VPS access revoked',
        '{subject.display_name} cannot reconnect until an operator assigns a new key',
        NULL, NULL, NULL, 'agent.access_revoked'
    ),
    (
        'd1000000-0000-4000-8000-000000000005',
        'c1000000-0000-4000-8000-000000000001', 4,
        'Tunnel adapter failure', TRUE, NULL,
        'evidence.adapter.success = false', 'critical', 'state', 'tunnel.adapter', 'natural_key',
        'network', 'Tunnel adapter status failed',
        '{evidence.reason}',
        '{"kind":"sustained","seconds":120}', NULL,
        '{"kind":"sustained","seconds":60}', 'tunnel.adapter_failure'
    ),
    (
        'd1000000-0000-4000-8000-000000000006',
        'c1000000-0000-4000-8000-000000000001', 5,
        'Tunnel traffic degraded', TRUE, NULL,
        'evidence.traffic.status != ok', 'warning', 'state', 'tunnel.traffic', 'natural_key',
        'network', 'Tunnel interface counters are degraded',
        '{evidence.reason}',
        '{"kind":"sustained","seconds":120}', NULL,
        '{"kind":"sustained","seconds":60}', 'tunnel.traffic_degraded'
    ),
    (
        'd1000000-0000-4000-8000-000000000007',
        'c1000000-0000-4000-8000-000000000001', 6,
        'General job partial success', TRUE, NULL,
        'evidence.status = partial_success && !(evidence.command_type in ["*backup*", "*restore*", "*agent_update*"])',
        'warning', 'occurrence', 'job.terminal', 'natural_key', 'job',
        'Job requires operator attention', '{evidence.command_type} job {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.general_partial_success'
    ),
    (
        'd1000000-0000-4000-8000-000000000008',
        'c1000000-0000-4000-8000-000000000001', 7,
        'General job hard failure', TRUE, NULL,
        'evidence.status in [canceled, rejected, failed, agent_timeout, control_timeout] && !(evidence.command_type in ["*backup*", "*restore*", "*agent_update*"])',
        'critical', 'occurrence', 'job.terminal', 'natural_key', 'job',
        'Job requires operator attention', '{evidence.command_type} job {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.general_hard_failure'
    ),
    (
        'd1000000-0000-4000-8000-000000000009',
        'c1000000-0000-4000-8000-000000000001', 8,
        'Backup job partial success', TRUE, NULL,
        'evidence.status = partial_success && evidence.command_type in ["*backup*", "*restore*"]',
        'warning', 'occurrence', 'job.terminal', 'natural_key', 'backup',
        'Job requires operator attention', '{evidence.command_type} job {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.backup_partial_success'
    ),
    (
        'd1000000-0000-4000-8000-000000000010',
        'c1000000-0000-4000-8000-000000000001', 9,
        'Backup job hard failure', TRUE, NULL,
        'evidence.status in [canceled, rejected, failed, agent_timeout, control_timeout] && evidence.command_type in ["*backup*", "*restore*"]',
        'critical', 'occurrence', 'job.terminal', 'natural_key', 'backup',
        'Job requires operator attention', '{evidence.command_type} job {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.backup_hard_failure'
    ),
    (
        'd1000000-0000-4000-8000-000000000011',
        'c1000000-0000-4000-8000-000000000001', 10,
        'Agent update partial success', TRUE, NULL,
        'evidence.status = partial_success && evidence.command_type in ["*agent_update*"]',
        'warning', 'occurrence', 'job.terminal', 'natural_key', 'agent_update',
        'Job requires operator attention', '{evidence.command_type} job {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.agent_update_partial_success'
    ),
    (
        'd1000000-0000-4000-8000-000000000012',
        'c1000000-0000-4000-8000-000000000001', 11,
        'Agent update hard failure', TRUE, NULL,
        'evidence.status in [canceled, rejected, failed, agent_timeout, control_timeout] && evidence.command_type in ["*agent_update*"]',
        'critical', 'occurrence', 'job.terminal', 'natural_key', 'agent_update',
        'Job requires operator attention', '{evidence.command_type} job {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.agent_update_hard_failure'
    ),
    (
        'd1000000-0000-4000-8000-000000000013',
        'c1000000-0000-4000-8000-000000000001', 12,
        'Backup request failure', TRUE, NULL,
        'evidence.status = execution_failed', 'critical', 'occurrence', 'backup.failure', 'natural_key',
        'backup', 'Backup request failed', 'backup request {evidence.backup_request_id} is {evidence.status}',
        NULL, NULL, '{"kind":"elapsed_since_trigger","seconds":604800}',
        'backup.request_failure'
    ),
    (
        'd1000000-0000-4000-8000-000000000014',
        'c1000000-0000-4000-8000-000000000001', 13,
        'Capability-degraded target', TRUE, NULL,
        'evidence.status = skipped', 'warning', 'occurrence', 'job.capability', 'natural_key',
        'capability_degraded', 'Operation skipped because the agent lacks a required capability',
        '{evidence.hint}', NULL, NULL,
        '{"kind":"elapsed_since_trigger","seconds":604800}',
        'job.capability_degraded'
    ),
    (
        'd1000000-0000-4000-8000-000000000015',
        'c1000000-0000-4000-8000-000000000001', 14,
        'Retired legacy agent connectivity history', FALSE, NULL,
        'evidence.status in [never, disconnected, stale, offline]',
        'warning', 'state', 'agent.status', 'natural_key',
        'agent_status', 'Agent connectivity legacy history',
        'Migrated connectivity history whose original trigger state was not retained.',
        NULL, NULL, NULL, 'legacy.agent_connectivity_history'
    )
ON CONFLICT DO NOTHING;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM policy_groups
        WHERE id = 'c1000000-0000-4000-8000-000000000001'
          AND name = 'System operational evidence policies'
          AND enabled
          AND selector_expression = '*'
    ) OR (
        SELECT count(*) FROM policy_rules
        WHERE group_id = 'c1000000-0000-4000-8000-000000000001'
          AND system_seed_key IS NOT NULL
    ) <> 15 THEN
        RAISE EXCEPTION '0012 deterministic operational policy seed conflict';
    END IF;
END;
$$;

-- Raw facts are immutable, presentation-neutral evidence. Source adapters own
-- stable correlation and subject snapshots, but never alert severity/title.
CREATE TABLE alert_policy_evidence (
    evidence_seq BIGINT NOT NULL UNIQUE
        DEFAULT nextval('alert_policy_evidence_seq'),
    id UUID PRIMARY KEY,
    source_kind TEXT NOT NULL,
    source_event_id TEXT NOT NULL,
    fact_kind TEXT NOT NULL,
    natural_key TEXT NOT NULL,
    confirmation_bucket_key TEXT NOT NULL,
    subject_client_id TEXT,
    target_kind TEXT NOT NULL,
    target_id TEXT NOT NULL,
    source_status TEXT NOT NULL,
    completeness TEXT NOT NULL,
    subject_snapshot JSONB NOT NULL,
    payload JSONB NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL,
    state_started_at TIMESTAMPTZ,
    causation_id UUID,
    schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT alert_policy_evidence_source_event_key
        UNIQUE (source_kind, source_event_id),
    CONSTRAINT alert_policy_evidence_source_kind_check CHECK (
        source_kind IN (
            'telemetry.combined', 'agent.status', 'agent.access',
            'tunnel.adapter', 'tunnel.traffic', 'job.terminal',
            'backup.failure', 'job.capability'
        )
    ),
    CONSTRAINT alert_policy_evidence_fact_kind_check CHECK (
        (source_kind = 'telemetry.combined' AND fact_kind = 'metric')
        OR (
            source_kind IN (
                'agent.status', 'agent.access', 'tunnel.adapter', 'tunnel.traffic'
            )
            AND fact_kind = 'state'
        )
        OR (
            source_kind IN ('job.terminal', 'backup.failure', 'job.capability')
            AND fact_kind = 'occurrence'
        )
    ),
    CONSTRAINT alert_policy_evidence_identity_check CHECK (
        length(btrim(source_event_id)) BETWEEN 1 AND 512
        AND length(btrim(natural_key)) BETWEEN 1 AND 512
        AND length(btrim(confirmation_bucket_key)) BETWEEN 1 AND 512
        AND length(btrim(target_kind)) BETWEEN 1 AND 64
        AND length(btrim(target_id)) BETWEEN 1 AND 512
        AND length(btrim(source_status)) BETWEEN 1 AND 256
    ),
    CONSTRAINT alert_policy_evidence_completeness_check CHECK (
        completeness IN ('complete', 'unknown')
    ),
    CONSTRAINT alert_policy_evidence_payload_check CHECK (
        jsonb_typeof(subject_snapshot) = 'object'
        AND jsonb_typeof(payload) = 'object'
    ),
    CONSTRAINT alert_policy_evidence_state_time_check CHECK (
        (fact_kind = 'occurrence' AND state_started_at IS NULL)
        OR (fact_kind IN ('metric', 'state') AND state_started_at IS NOT NULL)
    ),
    CONSTRAINT alert_policy_evidence_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(schedule_lineage, 16)
    )
);

CREATE INDEX alert_policy_evidence_source_latest_idx
    ON alert_policy_evidence (
        source_kind, natural_key, observed_at DESC, evidence_seq DESC
    );

CREATE INDEX alert_policy_evidence_created_idx
    ON alert_policy_evidence (evidence_seq ASC);

CREATE INDEX alert_policy_evidence_retention_candidates_idx
    ON alert_policy_evidence (created_at ASC, evidence_seq ASC);

CREATE INDEX alert_policy_evidence_subject_latest_idx
    ON alert_policy_evidence (subject_client_id, observed_at DESC, id DESC)
    WHERE subject_client_id IS NOT NULL;

-- Per-rule/correlation gate state records only accepted evidence IDs/revisions;
-- periodic evaluator ticks cannot manufacture count confirmations.
CREATE TABLE alert_policy_evaluation_states (
    policy_rule_id UUID NOT NULL REFERENCES policy_rules(id) ON DELETE CASCADE,
    rule_version INTEGER NOT NULL,
    confirmation_bucket_key TEXT NOT NULL,
    occurrence_cohort_id UUID,
    subject_client_id TEXT,
    truth_state TEXT NOT NULL,
    last_evidence_id UUID REFERENCES alert_policy_evidence(id),
    last_evidence_seq BIGINT REFERENCES alert_policy_evidence(evidence_seq),
    last_evidence_source_event_id TEXT,
    last_evidence_observed_at TIMESTAMPTZ,
    trigger_confirmed_duration_secs BIGINT NOT NULL DEFAULT 0,
    trigger_segment_started_at TIMESTAMPTZ,
    resolve_confirmed_duration_secs BIGINT NOT NULL DEFAULT 0,
    resolve_segment_started_at TIMESTAMPTZ,
    trigger_generation BIGINT NOT NULL DEFAULT 0,
    active_episode_id UUID,
    first_post_upgrade_evaluated_at TIMESTAMPTZ,
    next_transition_at TIMESTAMPTZ,
    last_evaluated_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (policy_rule_id, rule_version, confirmation_bucket_key),
    CONSTRAINT alert_policy_evaluation_states_truth_check CHECK (
        truth_state IN ('matched', 'not_matched', 'unknown')
    ),
    CONSTRAINT alert_policy_evaluation_states_durations_check CHECK (
        trigger_confirmed_duration_secs >= 0
        AND resolve_confirmed_duration_secs >= 0
    ),
    CONSTRAINT alert_policy_evaluation_states_generation_check CHECK (
        trigger_generation >= 0
    )
);

CREATE INDEX alert_policy_evaluation_states_due_idx
    ON alert_policy_evaluation_states (
        next_transition_at ASC, policy_rule_id, confirmation_bucket_key
    )
    WHERE next_transition_at IS NOT NULL;

CREATE INDEX alert_policy_evaluation_states_last_evidence_id_idx
    ON alert_policy_evaluation_states (last_evidence_id)
    WHERE last_evidence_id IS NOT NULL;

CREATE INDEX alert_policy_evaluation_states_last_evidence_seq_idx
    ON alert_policy_evaluation_states (last_evidence_seq)
    WHERE last_evidence_seq IS NOT NULL;

CREATE TABLE alert_policy_evidence_receipts (
    policy_rule_id UUID NOT NULL REFERENCES policy_rules(id) ON DELETE CASCADE,
    rule_version INTEGER NOT NULL,
    evidence_seq BIGINT NOT NULL REFERENCES alert_policy_evidence(evidence_seq) ON DELETE RESTRICT,
    evidence_id UUID NOT NULL REFERENCES alert_policy_evidence(id) ON DELETE RESTRICT,
    natural_key TEXT NOT NULL,
    confirmation_bucket_key TEXT NOT NULL,
    result TEXT NOT NULL,
    detail TEXT,
    evaluated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (policy_rule_id, rule_version, evidence_seq),
    CONSTRAINT alert_policy_evidence_receipts_result_check CHECK (
        result IN (
            'matched', 'not_matched', 'unknown', 'out_of_scope',
            'source_scope_exited', 'pre_armed', 'stale', 'error',
            'lineage_overflow'
        )
    )
);

CREATE INDEX alert_policy_evidence_receipts_evidence_idx
    ON alert_policy_evidence_receipts (evidence_seq, policy_rule_id);

CREATE INDEX alert_policy_evidence_receipts_evidence_id_idx
    ON alert_policy_evidence_receipts (evidence_id);

CREATE INDEX alert_policy_evidence_receipts_retention_idx
    ON alert_policy_evidence_receipts (evaluated_at ASC, evidence_seq ASC);

CREATE TABLE alert_policy_confirmations (
    policy_rule_id UUID NOT NULL REFERENCES policy_rules(id) ON DELETE CASCADE,
    rule_version INTEGER NOT NULL,
    confirmation_bucket_key TEXT NOT NULL,
    phase TEXT NOT NULL,
    evidence_id UUID NOT NULL REFERENCES alert_policy_evidence(id) ON DELETE RESTRICT,
    accepted_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (
        policy_rule_id, rule_version, confirmation_bucket_key, phase, evidence_id
    ),
    CONSTRAINT alert_policy_confirmations_phase_check CHECK (
        phase IN ('trigger', 'resolve')
    )
);

CREATE INDEX alert_policy_confirmations_window_idx
    ON alert_policy_confirmations (
        policy_rule_id, rule_version, confirmation_bucket_key,
        phase, accepted_at DESC, evidence_id
    );

CREATE INDEX alert_policy_confirmations_evidence_idx
    ON alert_policy_confirmations (evidence_id);

-- Evolve the operational owner in place so its public IDs and triage keys do
-- not move. Policy rows are copied into this same owner below.
ALTER TABLE operational_alert_episodes RENAME TO alert_episodes;

ALTER TABLE alert_episodes
    DROP CONSTRAINT operational_alert_episode_identity_key,
    DROP CONSTRAINT operational_alert_episode_producer_check,
    DROP CONSTRAINT operational_alert_episode_category_check,
    DROP CONSTRAINT operational_alert_episode_trigger_category_check,
    DROP CONSTRAINT operational_alert_episode_resolution_reason_check,
    DROP CONSTRAINT operational_alert_episode_lifecycle_check,
    ADD COLUMN policy_group_id UUID,
    ADD COLUMN policy_rule_id UUID,
    ADD COLUMN policy_rule_version INTEGER,
    ADD COLUMN policy_rule_kind TEXT,
    ADD COLUMN policy_group_name TEXT,
    ADD COLUMN policy_rule_name TEXT,
    ADD COLUMN policy_rule_system_seed_key TEXT,
    ADD COLUMN trigger_evidence_id UUID REFERENCES alert_policy_evidence(id),
    ADD COLUMN last_evidence_id UUID REFERENCES alert_policy_evidence(id),
    ADD COLUMN first_post_upgrade_evaluated_at TIMESTAMPTZ,
    ADD COLUMN causation_id UUID,
    ADD COLUMN schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[];

DROP INDEX operational_alert_episodes_one_current_idx;
DROP INDEX operational_alert_episodes_event_source_once_idx;

ALTER TABLE alert_episodes
    ADD CONSTRAINT alert_episodes_policy_provenance_check CHECK (
        (
            policy_group_id IS NULL
            AND policy_rule_id IS NULL
            AND policy_rule_version IS NULL
            AND policy_rule_kind IS NULL
        )
        OR (
            policy_group_id IS NOT NULL
            AND policy_rule_id IS NOT NULL
            AND policy_rule_version IS NOT NULL
            AND policy_rule_version >= 1
            AND policy_rule_kind IN ('metric', 'state', 'occurrence')
            AND policy_group_name IS NOT NULL
            AND policy_rule_name IS NOT NULL
        )
    ),
    ADD CONSTRAINT alert_episodes_rule_record_kind_check CHECK (
        policy_rule_kind IS NULL
        OR (policy_rule_kind IN ('metric', 'state') AND record_kind = 'condition')
        OR (policy_rule_kind = 'occurrence' AND record_kind = 'event')
    ),
    ADD CONSTRAINT alert_episodes_producer_check CHECK (
        length(btrim(producer_kind)) BETWEEN 1 AND 128
        AND producer_kind ~ '^[a-z][a-z0-9_.-]*$'
    ),
    ADD CONSTRAINT alert_episodes_category_check CHECK (
        category IN (
            'agent_status', 'network', 'backup', 'agent_update', 'job',
            'capability_degraded', 'traffic', 'resource'
        )
        AND trigger_category IN (
            'agent_status', 'network', 'backup', 'agent_update', 'job',
            'capability_degraded', 'traffic', 'resource'
        )
    ),
    ADD CONSTRAINT alert_episodes_resolution_reason_check CHECK (
        resolution_reason IS NULL
        OR resolution_reason IN (
            'condition_recovered',
            'recovery_expression_matched',
            'policy_time_elapsed',
            'source_scope_exited',
            'policy_scope_exited',
            'policy_scope_changed',
            'policy_disabled',
            'policy_changed',
            'policy_deleted',
            'operator_resolved'
        )
    ),
    ADD CONSTRAINT alert_episodes_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(schedule_lineage, 16)
    ),
    ADD CONSTRAINT alert_episodes_lifecycle_check CHECK (
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
            AND (last_confirmed_at IS NULL OR last_confirmed_at >= triggered_at)
            AND resolved_at IS NULL
            AND resolution_reason IS NULL
            AND resolution_note IS NULL
            AND resolution_actor_id IS NULL
        ) OR (
            lifecycle_state = 'resolved'
            AND last_confirmed_at IS NOT NULL
            AND last_confirmed_at >= triggered_at
            AND resolved_at IS NOT NULL
            AND resolved_at >= last_confirmed_at
            AND resolution_reason IS NOT NULL
            AND (
                (
                    record_kind = 'event'
                    AND resolution_reason = 'operator_resolved'
                    AND resolution_note IS NOT NULL
                    AND resolution_actor_id IS NOT NULL
                ) OR (
                    record_kind = 'event'
                    AND
                    resolution_reason IN (
                        'policy_time_elapsed',
                        'source_scope_exited',
                        'policy_scope_exited',
                        'policy_scope_changed',
                        'policy_disabled',
                        'policy_changed',
                        'policy_deleted'
                    )
                    AND resolution_note IS NULL
                    AND resolution_actor_id IS NULL
                ) OR (
                    record_kind = 'condition'
                    AND resolution_reason IN (
                        'condition_recovered',
                        'recovery_expression_matched',
                        'source_scope_exited',
                        'policy_scope_exited',
                        'policy_scope_changed',
                        'policy_disabled',
                        'policy_changed',
                        'policy_deleted'
                    )
                    AND resolution_note IS NULL
                    AND resolution_actor_id IS NULL
                )
            )
        )
    );

-- Metric-policy history enters the same owner quietly. The policy public ID
-- format and all lifecycle timestamps remain unchanged.
INSERT INTO alert_episodes (
    id, public_id, producer_kind, natural_key, record_kind,
    trigger_generation, trigger_severity, trigger_category, severity, category,
    target_kind, target_id, client_id, title, detail, source_status, evidence,
    lifecycle_state, triggered_at, last_confirmed_at, resolved_at,
    resolution_reason, resolution_note, resolution_actor_id, backfilled,
    policy_group_id, policy_rule_id, policy_rule_version, policy_rule_kind,
    policy_group_name, policy_rule_name, policy_rule_system_seed_key,
    first_post_upgrade_evaluated_at, created_at, updated_at
)
SELECT
    alert.id,
    'policy-alert:' || alert.id::text,
    COALESCE(rule.evidence_source, 'telemetry.combined'),
    alert.client_id,
    'condition',
    alert.trigger_generation,
    alert.severity,
    alert.category,
    alert.severity,
    alert.category,
    'agent',
    alert.client_id,
    alert.client_id,
    alert.title,
    alert.detail,
    'policy_condition',
    alert.payload
        || jsonb_build_object(
            'legacy_policy_alert', TRUE,
            'actual_value', alert.actual_value,
            'threshold_value', alert.threshold_value,
            'legacy_resolution_reason', alert.resolution_reason
        ),
    alert.lifecycle_state,
    alert.created_at,
    alert.last_confirmed_at,
    alert.resolved_at,
    alert.resolution_reason,
    NULL,
    NULL,
    alert.lifecycle_state = 'unknown',
    alert.policy_group_id,
    alert.policy_rule_id,
    CASE
        WHEN alert.payload#>>'{rule,rule_version}' ~ '^[0-9]+$'
            THEN (alert.payload#>>'{rule,rule_version}')::INTEGER
        ELSE COALESCE(rule.rule_version, 1)
    END,
    COALESCE(rule.rule_kind, 'metric'),
    COALESCE(group_row.name, 'Retired policy group'),
    COALESCE(rule.name, 'Retired policy rule'),
    rule.system_seed_key,
    NULL,
    alert.created_at,
    GREATEST(
        alert.created_at,
        alert.observed_at,
        COALESCE(alert.resolved_at, alert.created_at)
    )
FROM policy_alerts alert
LEFT JOIN policy_rules rule ON rule.id = alert.policy_rule_id
LEFT JOIN policy_groups group_row ON group_row.id = alert.policy_group_id
;

-- Backfill operational episodes onto deterministic starter rules below. This
-- changes provenance only; it never creates an edge or mutates lifecycle time.
WITH mapped AS (
    SELECT
        episode.id,
        CASE
            WHEN episode.producer_kind = 'agent_status' AND episode.source_status = 'never'
                THEN 'd1000000-0000-4000-8000-000000000001'::UUID
            WHEN episode.producer_kind = 'agent_status' AND episode.source_status IN ('disconnected', 'stale')
                THEN 'd1000000-0000-4000-8000-000000000002'::UUID
            WHEN episode.producer_kind = 'agent_status' AND episode.source_status = 'offline'
                THEN 'd1000000-0000-4000-8000-000000000003'::UUID
            -- Resolution evidence overwrote source_status in 0011. Critical
            -- connectivity episodes were the offline variant; warning history
            -- is otherwise ambiguous between never/disconnected/stale and is
            -- retained under a disabled migration-only rule without parsing
            -- mutable presentation text.
            WHEN episode.producer_kind = 'agent_status'
                 AND episode.lifecycle_state = 'resolved'
                 AND episode.trigger_severity = 'critical'
                THEN 'd1000000-0000-4000-8000-000000000003'::UUID
            WHEN episode.producer_kind = 'agent_status'
                THEN 'd1000000-0000-4000-8000-000000000015'::UUID
            WHEN episode.producer_kind = 'agent_access'
                THEN 'd1000000-0000-4000-8000-000000000004'::UUID
            WHEN episode.producer_kind = 'tunnel_adapter'
                THEN 'd1000000-0000-4000-8000-000000000005'::UUID
            WHEN episode.producer_kind = 'tunnel_traffic'
                THEN 'd1000000-0000-4000-8000-000000000006'::UUID
            WHEN episode.producer_kind = 'job' AND episode.category = 'job' AND episode.severity = 'warning'
                THEN 'd1000000-0000-4000-8000-000000000007'::UUID
            WHEN episode.producer_kind = 'job' AND episode.category = 'job'
                THEN 'd1000000-0000-4000-8000-000000000008'::UUID
            WHEN episode.producer_kind = 'job' AND episode.category = 'backup' AND episode.severity = 'warning'
                THEN 'd1000000-0000-4000-8000-000000000009'::UUID
            WHEN episode.producer_kind = 'job' AND episode.category = 'backup'
                THEN 'd1000000-0000-4000-8000-000000000010'::UUID
            WHEN episode.producer_kind = 'job' AND episode.category = 'agent_update' AND episode.severity = 'warning'
                THEN 'd1000000-0000-4000-8000-000000000011'::UUID
            WHEN episode.producer_kind = 'job' AND episode.category = 'agent_update'
                THEN 'd1000000-0000-4000-8000-000000000012'::UUID
            WHEN episode.producer_kind = 'backup_request'
                THEN 'd1000000-0000-4000-8000-000000000013'::UUID
            WHEN episode.producer_kind = 'capability_degraded'
                THEN 'd1000000-0000-4000-8000-000000000014'::UUID
            ELSE NULL
        END AS rule_id
    FROM alert_episodes episode
    WHERE episode.policy_rule_id IS NULL
)
UPDATE alert_episodes episode
SET policy_group_id = rule.group_id,
    policy_rule_id = rule.id,
    policy_rule_version = rule.rule_version,
    policy_rule_kind = rule.rule_kind,
    policy_group_name = group_row.name,
    policy_rule_name = rule.name,
    policy_rule_system_seed_key = rule.system_seed_key,
    producer_kind = rule.evidence_source,
    natural_key = episode.natural_key
FROM mapped
JOIN policy_rules rule ON rule.id = mapped.rule_id
JOIN policy_groups group_row ON group_row.id = rule.group_id
WHERE episode.id = mapped.id;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM alert_episodes
        WHERE policy_group_id IS NULL
           OR policy_rule_id IS NULL
           OR policy_rule_version IS NULL
           OR policy_rule_kind IS NULL
           OR policy_group_name IS NULL
           OR policy_rule_name IS NULL
    ) THEN
        RAISE EXCEPTION '0012 unmapped alert episode policy provenance';
    END IF;
END;
$$;

ALTER TABLE alert_episodes
    ALTER COLUMN policy_group_id SET NOT NULL,
    ALTER COLUMN policy_rule_id SET NOT NULL,
    ALTER COLUMN policy_rule_version SET NOT NULL,
    ALTER COLUMN policy_rule_kind SET NOT NULL,
    ALTER COLUMN policy_group_name SET NOT NULL,
    ALTER COLUMN policy_rule_name SET NOT NULL;

CREATE INDEX alert_episodes_trigger_evidence_idx
    ON alert_episodes (trigger_evidence_id)
    WHERE trigger_evidence_id IS NOT NULL;

CREATE INDEX alert_episodes_last_evidence_idx
    ON alert_episodes (last_evidence_id)
    WHERE last_evidence_id IS NOT NULL;

CREATE UNIQUE INDEX alert_episodes_identity_idx
    ON alert_episodes (
        policy_rule_id, policy_rule_version, natural_key, trigger_generation
    );

CREATE UNIQUE INDEX alert_episodes_one_current_idx
    ON alert_episodes (policy_rule_id, natural_key)
    -- Pre-0010 policy history can contain several conservative Unknown rows
    -- that were never confirmed. They remain history-only and must neither
    -- collide with one another nor block the one confirmed current episode.
    WHERE resolved_at IS NULL AND last_confirmed_at IS NOT NULL;

-- Every migrated episode receives one consumed source fact. Occurrence source
-- IDs retain the old natural source key; condition IDs use the immutable
-- public episode ID because the next authoritative state revision is distinct.
INSERT INTO alert_policy_evidence (
    id, source_kind, source_event_id, fact_kind, natural_key, confirmation_bucket_key,
    subject_client_id, target_kind, target_id, source_status, completeness,
    subject_snapshot, payload, observed_at, state_started_at,
    causation_id, schedule_lineage, created_at
)
SELECT
    episode.id,
    episode.producer_kind,
    CASE
        WHEN episode.record_kind = 'event'
            THEN episode.natural_key
        ELSE episode.public_id || ':migration'
    END,
    episode.policy_rule_kind,
    episode.natural_key,
    CASE COALESCE(rule.correlation_mode, 'natural_key')
        WHEN 'natural_key' THEN 'natural:' || episode.natural_key
        WHEN 'subject' THEN 'subject:' || episode.client_id
        ELSE 'global:' || episode.producer_kind
    END,
    episode.client_id,
    episode.target_kind,
    episode.target_id,
    episode.source_status,
    CASE episode.lifecycle_state WHEN 'unknown' THEN 'unknown' ELSE 'complete' END,
    CASE
        WHEN jsonb_typeof(episode.evidence->'subject') = 'object'
            THEN episode.evidence->'subject'
        ELSE jsonb_strip_nulls(jsonb_build_object('client_id', episode.client_id))
    END,
    episode.evidence || jsonb_build_object(
        'migration_backfill', TRUE,
        'legacy_public_id', episode.public_id,
        'policy', jsonb_build_object(
            'id', episode.policy_group_id,
            'name', episode.policy_group_name
        ),
        'policy_rule', jsonb_strip_nulls(jsonb_build_object(
            'id', episode.policy_rule_id,
            'name', episode.policy_rule_name,
            'rule_version', episode.policy_rule_version,
            'rule_kind', episode.policy_rule_kind,
            'system_seed_key', episode.policy_rule_system_seed_key
        ))
    ),
    COALESCE(episode.last_confirmed_at, episode.triggered_at),
    CASE WHEN episode.record_kind = 'condition' THEN episode.triggered_at ELSE NULL END,
    episode.causation_id,
    episode.schedule_lineage,
    episode.created_at
FROM alert_episodes episode
LEFT JOIN policy_rules rule ON rule.id = episode.policy_rule_id;

UPDATE alert_episodes episode
SET trigger_evidence_id = episode.id,
    last_evidence_id = episode.id;

INSERT INTO alert_policy_evidence_receipts (
    policy_rule_id, rule_version, evidence_seq, evidence_id,
    natural_key, confirmation_bucket_key, result, detail, evaluated_at
)
SELECT
    episode.policy_rule_id,
    episode.policy_rule_version,
    evidence.evidence_seq,
    evidence.id,
    episode.natural_key,
    evidence.confirmation_bucket_key,
    CASE episode.lifecycle_state WHEN 'unknown' THEN 'unknown' ELSE 'matched' END,
    '0012 quiet lifecycle-owner migration',
    meta.cutover_at
FROM alert_episodes episode
JOIN alert_policy_evidence evidence ON evidence.id = episode.id
JOIN policy_rules live_rule ON live_rule.id = episode.policy_rule_id
CROSS JOIN alert_policy_lifecycle_meta meta;

UPDATE policy_rules rule
SET armed_after_evidence_seq = COALESCE(
        (SELECT max(evidence_seq) FROM alert_policy_evidence),
        0
    ),
    armed_at = meta.cutover_at
FROM alert_policy_lifecycle_meta meta;

-- Preserve pending metric dwell/generation state before retiring the old
-- state table. A currently active episode wins as the durable lifecycle owner.
INSERT INTO alert_policy_evaluation_states (
    policy_rule_id, rule_version, confirmation_bucket_key, occurrence_cohort_id,
    subject_client_id, truth_state,
    last_evidence_id, last_evidence_seq, last_evidence_source_event_id,
    last_evidence_observed_at,
    trigger_confirmed_duration_secs, trigger_segment_started_at,
    resolve_confirmed_duration_secs, resolve_segment_started_at,
    trigger_generation, active_episode_id, first_post_upgrade_evaluated_at,
    next_transition_at, last_evaluated_at, updated_at
)
SELECT
    state.policy_rule_id,
    state.rule_version,
    'natural:' || state.client_id,
    NULL,
    state.client_id,
    CASE
        WHEN state.incomplete THEN 'unknown'
        WHEN state.condition_true THEN 'matched'
        ELSE 'not_matched'
    END,
    episode.id,
    evidence.evidence_seq,
    evidence.source_event_id,
    episode.last_confirmed_at,
    CASE
        WHEN episode.id IS NOT NULL
         AND episode.resolved_at IS NULL
         AND episode.last_confirmed_at IS NOT NULL THEN 0
        WHEN state.condition_true AND NOT state.incomplete
         AND state.first_true_at IS NOT NULL
            THEN GREATEST(
                EXTRACT(EPOCH FROM (state.last_evaluated_at - state.first_true_at))::BIGINT,
                0
            )
        ELSE 0
    END,
    -- Migration preserves already-confirmed dwell but pauses it. Only a fresh
    -- authoritative complete fact may start the next known segment; the
    -- cutover itself and timer ticks never manufacture evidence time.
    NULL,
    0,
    NULL,
    state.trigger_generation,
    CASE
        WHEN episode.resolved_at IS NULL AND episode.last_confirmed_at IS NOT NULL
            THEN episode.id
        ELSE NULL
    END,
    NULL,
    NULL,
    state.last_evaluated_at,
    state.updated_at
FROM policy_rule_states state
JOIN policy_rules rule ON rule.id = state.policy_rule_id
LEFT JOIN LATERAL (
    SELECT candidate.*
    FROM alert_episodes candidate
    WHERE candidate.policy_rule_id = state.policy_rule_id
      AND candidate.client_id = state.client_id
      AND candidate.trigger_generation = state.trigger_generation
    ORDER BY candidate.triggered_at DESC, candidate.id DESC
    LIMIT 1

) episode ON TRUE
LEFT JOIN alert_policy_evidence evidence ON evidence.id = episode.id;

WITH ranked_episode_state AS (
    SELECT
        episode.*,
        evidence.confirmation_bucket_key,
        evidence.evidence_seq,
        evidence.source_event_id,
        row_number() OVER (
            PARTITION BY episode.policy_rule_id, episode.policy_rule_version,
                         evidence.confirmation_bucket_key
            ORDER BY
                (episode.resolved_at IS NULL) DESC,
                episode.trigger_generation DESC,
                episode.triggered_at DESC,
                episode.id DESC
        ) AS state_rank
    FROM alert_episodes episode
    JOIN alert_policy_evidence evidence ON evidence.id = episode.id
    JOIN policy_rules rule
      ON rule.id = episode.policy_rule_id
     AND rule.rule_version = episode.policy_rule_version
)
INSERT INTO alert_policy_evaluation_states (
    policy_rule_id, rule_version, confirmation_bucket_key, occurrence_cohort_id,
    subject_client_id, truth_state,
    last_evidence_id, last_evidence_seq, last_evidence_source_event_id,
    last_evidence_observed_at,
    trigger_confirmed_duration_secs, trigger_segment_started_at,
    resolve_confirmed_duration_secs, resolve_segment_started_at,
    trigger_generation, active_episode_id, first_post_upgrade_evaluated_at,
    next_transition_at, last_evaluated_at, updated_at
)
SELECT
    episode.policy_rule_id,
    episode.policy_rule_version,
    episode.confirmation_bucket_key,
    CASE WHEN episode.record_kind = 'event' THEN episode.id ELSE NULL END,
    episode.client_id,
    CASE episode.lifecycle_state
        WHEN 'unknown' THEN 'unknown'
        WHEN 'resolved' THEN 'not_matched'
        ELSE 'matched'
    END,
    episode.id,
    episode.evidence_seq,
    episode.source_event_id,
    episode.last_confirmed_at,
    0,
    NULL,
    0,
    NULL,
    episode.trigger_generation,
    CASE
        WHEN episode.resolved_at IS NULL AND episode.last_confirmed_at IS NOT NULL
            THEN episode.id
        ELSE NULL
    END,
    NULL,
    CASE
        WHEN episode.record_kind = 'event'
         AND episode.resolved_at IS NULL
         AND episode.last_confirmed_at IS NOT NULL
            THEN episode.triggered_at + interval '7 days'
        ELSE NULL
    END,
    meta.cutover_at,
    meta.cutover_at
FROM ranked_episode_state episode
CROSS JOIN alert_policy_lifecycle_meta meta
WHERE episode.state_rank = 1
ON CONFLICT (policy_rule_id, rule_version, confirmation_bucket_key) DO NOTHING;

-- Occurrence defaults expire old unresolved migration history quietly. The
-- migration transition is evidence, not an edge, and therefore cannot fire a
-- webhook or schedule. Younger occurrences keep their original due time.
UPDATE alert_episodes episode
SET lifecycle_state = 'resolved',
    resolved_at = meta.cutover_at,
    resolution_reason = 'policy_time_elapsed',
    evidence = episode.evidence || jsonb_build_object(
        'migration_time_expiry', TRUE,
        'migration_time_expiry_at', meta.cutover_at
    ),
    updated_at = meta.cutover_at
FROM alert_policy_lifecycle_meta meta
WHERE episode.record_kind = 'event'
  AND episode.resolved_at IS NULL
  AND episode.triggered_at <= meta.cutover_at - interval '7 days';

UPDATE alert_policy_evaluation_states state
SET truth_state = 'not_matched',
    active_episode_id = NULL,
    next_transition_at = NULL,
    updated_at = meta.cutover_at
FROM alert_episodes episode
CROSS JOIN alert_policy_lifecycle_meta meta
WHERE state.active_episode_id = episode.id
  AND episode.resolution_reason = 'policy_time_elapsed'
  AND episode.evidence @> '{"migration_time_expiry":true}'::jsonb;

-- Lifecycle edges are a dedicated durable outbox. Webhooks and schedules own
-- independent receipts/cursors and never share webhook_events.processed_at.
CREATE TABLE alert_lifecycle_events (
    event_seq BIGINT PRIMARY KEY
        DEFAULT nextval('alert_lifecycle_event_seq'),
    id UUID NOT NULL UNIQUE,
    episode_id UUID NOT NULL REFERENCES alert_episodes(id) ON DELETE RESTRICT,
    trigger_generation BIGINT NOT NULL,
    edge_kind TEXT NOT NULL,
    event_id TEXT NOT NULL,
    event_predicates TEXT[] NOT NULL,
    subject_client_ids TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    payload JSONB NOT NULL,
    causation_id UUID,
    schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    occurred_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT alert_lifecycle_events_episode_edge_key
        UNIQUE (episode_id, trigger_generation, edge_kind),
    CONSTRAINT alert_lifecycle_events_kind_event_id_key
        UNIQUE (edge_kind, event_id),
    CONSTRAINT alert_lifecycle_events_kind_check CHECK (
        edge_kind IN ('alert.triggered', 'alert.resolved')
    ),
    CONSTRAINT alert_lifecycle_events_event_id_check CHECK (
        length(btrim(event_id)) BETWEEN 1 AND 256
    ),
    CONSTRAINT alert_lifecycle_events_payload_check CHECK (
        jsonb_typeof(payload) = 'object'
    ),
    CONSTRAINT alert_lifecycle_events_predicate_check CHECK (
        cardinality(event_predicates) >= 1
        AND event_predicates @> ARRAY[edge_kind]::TEXT[]
        AND event_predicates <@ ARRAY[
            'alert.triggered', 'alert.resolved',
            'alert.category:agent_status', 'alert.category:network',
            'alert.category:backup', 'alert.category:agent_update',
            'alert.category:job', 'alert.category:capability_degraded',
            'alert.category:traffic', 'alert.category:resource',
            'alert.severity:info', 'alert.severity:warning',
            'alert.severity:critical'
        ]::TEXT[]
    ),
    CONSTRAINT alert_lifecycle_events_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(schedule_lineage, 16)
    )
);

CREATE INDEX alert_lifecycle_events_consumer_idx
    ON alert_lifecycle_events (event_seq ASC);

CREATE INDEX alert_lifecycle_events_episode_idx
    ON alert_lifecycle_events (episode_id, trigger_generation, event_seq ASC);

CREATE TABLE alert_lifecycle_consumer_cursors (
    consumer_kind TEXT PRIMARY KEY,
    last_event_seq BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (consumer_kind IN ('webhook', 'schedule')),
    CHECK (last_event_seq >= 0)
);

INSERT INTO alert_lifecycle_consumer_cursors (consumer_kind)
VALUES ('webhook'), ('schedule')
ON CONFLICT (consumer_kind) DO NOTHING;

CREATE TABLE alert_lifecycle_webhook_receipts (
    event_seq BIGINT PRIMARY KEY
        REFERENCES alert_lifecycle_events(event_seq) ON DELETE RESTRICT,
    webhook_event_id UUID,
    webhook_event_occurred_at TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'pending',
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    UNIQUE (webhook_event_id),
    CHECK (status IN ('pending', 'projected', 'failed')),
    CHECK (
        (status = 'projected' AND webhook_event_id IS NOT NULL AND webhook_event_occurred_at IS NOT NULL)
        OR (status <> 'projected' AND webhook_event_id IS NULL AND webhook_event_occurred_at IS NULL)
    )
);

-- Application code performs a guarded AST rewrite of webhook expressions and
-- conditional body-template expressions before retiring legacy alert aliases.
CREATE TABLE alert_expression_migration_meta (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    completed_at TIMESTAMPTZ,
    rewritten_rule_count BIGINT,
    rewritten_template_count BIGINT,
    CHECK (
        (completed_at IS NULL AND rewritten_rule_count IS NULL AND rewritten_template_count IS NULL)
        OR (
            completed_at IS NOT NULL
            AND rewritten_rule_count IS NOT NULL
            AND rewritten_rule_count >= 0
            AND rewritten_template_count IS NOT NULL
            AND rewritten_template_count >= 0
        )
    )
);

INSERT INTO alert_expression_migration_meta (singleton)
VALUES (TRUE)
ON CONFLICT (singleton) DO NOTHING;

-- Rewriting is intentionally one-way at runtime, so retain the exact prior
-- bytes and both sides' hashes for every changed persisted webhook rule. This
-- makes the maintenance-gated canonicalization independently diagnosable and
-- recoverable without reintroducing compatibility parsing.
CREATE TABLE alert_expression_migration_audit (
    rule_id UUID PRIMARY KEY,
    prior_expression TEXT NOT NULL,
    rewritten_expression TEXT NOT NULL,
    prior_body_template TEXT NOT NULL,
    rewritten_body_template TEXT NOT NULL,
    prior_expression_sha256 TEXT NOT NULL,
    rewritten_expression_sha256 TEXT NOT NULL,
    prior_body_template_sha256 TEXT NOT NULL,
    rewritten_body_template_sha256 TEXT NOT NULL,
    rewritten_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (
        prior_expression_sha256 ~ '^[0-9a-f]{64}$'
        AND rewritten_expression_sha256 ~ '^[0-9a-f]{64}$'
        AND prior_body_template_sha256 ~ '^[0-9a-f]{64}$'
        AND rewritten_body_template_sha256 ~ '^[0-9a-f]{64}$'
    ),
    CHECK (
        prior_expression IS DISTINCT FROM rewritten_expression
        OR prior_body_template IS DISTINCT FROM rewritten_body_template
    )
);

-- Schedule trigger discrimination. Existing schedules remain byte-for-byte
-- cron schedules. Event arming is sequence-based; timestamps are display-only.
ALTER TABLE schedules
    ADD COLUMN trigger_kind TEXT NOT NULL DEFAULT 'cron',
    ADD COLUMN event_expression TEXT,
    ADD COLUMN event_argv_template JSONB,
    ADD COLUMN definition_revision BIGINT NOT NULL DEFAULT 1,
    ADD COLUMN event_armed_at TIMESTAMPTZ,
    ADD COLUMN armed_after_event_seq BIGINT,
    ALTER COLUMN operation DROP NOT NULL,
    ALTER COLUMN cron_expr DROP NOT NULL,
    ALTER COLUMN timezone DROP NOT NULL,
    ALTER COLUMN next_run_at DROP NOT NULL,
    ALTER COLUMN catch_up_policy DROP NOT NULL,
    ALTER COLUMN catch_up_limit DROP NOT NULL,
    ALTER COLUMN retry_delay_secs DROP NOT NULL;

DROP INDEX schedules_due_idx;
DROP INDEX schedules_policy_due_idx;

ALTER TABLE schedules
    DROP CONSTRAINT schedules_catch_up_policy_check,
    DROP CONSTRAINT schedules_catch_up_limit_check,
    DROP CONSTRAINT schedules_timezone_utc,
    DROP CONSTRAINT schedules_cron_expr_not_empty,
    ADD CONSTRAINT schedules_trigger_kind_check CHECK (
        trigger_kind IN ('cron', 'event')
    ),
    ADD CONSTRAINT schedules_definition_revision_check CHECK (
        definition_revision >= 1
    ),
    ADD CONSTRAINT schedules_trigger_shape_check CHECK (
        (
            trigger_kind = 'cron'
            AND cron_expr IS NOT NULL
            AND length(btrim(cron_expr)) > 0
            AND timezone = 'UTC'
            AND next_run_at IS NOT NULL
            AND catch_up_policy IN ('skip_missed', 'run_once', 'run_all_limited')
            AND catch_up_limit BETWEEN 1 AND 25
            AND retry_delay_secs BETWEEN 1 AND 86400
            AND operation IS NOT NULL
            AND event_expression IS NULL
            AND event_argv_template IS NULL
            AND event_armed_at IS NULL
            AND armed_after_event_seq IS NULL
        ) OR (
            trigger_kind = 'event'
            AND cron_expr IS NULL
            AND timezone IS NULL
            AND next_run_at IS NULL
            AND catch_up_policy IS NULL
            AND catch_up_limit IS NULL
            AND retry_delay_secs IS NULL
            AND operation IS NULL
            AND event_expression IS NOT NULL
            AND length(btrim(event_expression)) BETWEEN 1 AND 4096
            AND event_armed_at IS NOT NULL
            AND armed_after_event_seq IS NOT NULL
            AND armed_after_event_seq >= 0
        )
    ),
    ADD CONSTRAINT schedules_event_argv_template_check CHECK (
        event_argv_template IS NULL
        OR (
            trigger_kind = 'event'
            AND alert_jsonb_string_array_valid(event_argv_template, 128)
        )
    );

CREATE INDEX schedules_due_idx
    ON schedules (enabled, next_run_at, deferred_until)
    WHERE deleted_at IS NULL AND trigger_kind = 'cron';

CREATE INDEX schedules_policy_due_idx
    ON schedules (enabled, next_run_at, catch_up_policy)
    WHERE deleted_at IS NULL AND trigger_kind = 'cron';

CREATE INDEX schedules_event_enabled_idx
    ON schedules (armed_after_event_seq, id)
    WHERE deleted_at IS NULL AND enabled AND trigger_kind = 'event';

CREATE TABLE schedule_event_receipts (
    id UUID PRIMARY KEY,
    schedule_id UUID NOT NULL REFERENCES schedules(id) ON DELETE RESTRICT,
    definition_revision BIGINT NOT NULL,
    actor_id UUID REFERENCES operators(id) ON DELETE RESTRICT,
    schedule_name TEXT NOT NULL,
    event_seq BIGINT NOT NULL REFERENCES alert_lifecycle_events(event_seq) ON DELETE RESTRICT,
    event_kind TEXT NOT NULL,
    event_id TEXT NOT NULL,
    episode_id UUID NOT NULL REFERENCES alert_episodes(id) ON DELETE RESTRICT,
    trigger_generation BIGINT NOT NULL,
    edge_ordinal INTEGER NOT NULL,
    status TEXT NOT NULL,
    status_reason TEXT,
    source_occurred_at TIMESTAMPTZ NOT NULL,
    source_payload_hash TEXT NOT NULL,
    matched_subject_client_ids TEXT[] NOT NULL,
    fixed_target_client_ids TEXT[] NOT NULL,
    causation_id UUID NOT NULL,
    source_schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    dispatched_schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    rendered_operation JSONB,
    rendered_operation_hash TEXT,
    job_id UUID UNIQUE REFERENCES jobs(id) ON DELETE RESTRICT,
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    dispatched_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CONSTRAINT schedule_event_receipts_source_key
        UNIQUE (schedule_id, event_kind, event_id),
    CONSTRAINT schedule_event_receipts_schedule_name_check CHECK (
        length(btrim(schedule_name)) BETWEEN 1 AND 256
    ),
    CONSTRAINT schedule_event_receipts_status_check CHECK (
        status IN (
            'pending', 'dispatched', 'skipped', 'superseded',
            'lineage_overflow', 'failed'
        )
    ),
    CONSTRAINT schedule_event_receipts_edge_check CHECK (
        event_kind IN ('alert.triggered', 'alert.resolved')
        AND edge_ordinal IN (1, 2)
    ),
    CONSTRAINT schedule_event_receipts_hash_check CHECK (
        source_payload_hash ~ '^[0-9a-f]{64}$'
        AND (rendered_operation_hash IS NULL OR rendered_operation_hash ~ '^[0-9a-f]{64}$')
    ),
    CONSTRAINT schedule_event_receipts_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(source_schedule_lineage, 16)
        AND alert_uuid_array_is_unique_bounded(dispatched_schedule_lineage, 16)
    ),
    CONSTRAINT schedule_event_receipts_result_check CHECK (
        (status = 'dispatched' AND job_id IS NOT NULL AND dispatched_at IS NOT NULL)
        OR (status <> 'dispatched' AND job_id IS NULL AND dispatched_at IS NULL)
    )
);

CREATE INDEX schedule_event_receipts_pending_idx
    ON schedule_event_receipts (event_seq ASC, schedule_id)
    WHERE status = 'pending';

CREATE INDEX schedule_event_receipts_episode_idx
    ON schedule_event_receipts (episode_id, trigger_generation, edge_ordinal, status);

CREATE INDEX schedule_event_receipts_event_idx
    ON schedule_event_receipts (event_seq, id);

CREATE TABLE schedule_event_dependencies (
    receipt_id UUID NOT NULL REFERENCES schedule_event_receipts(id) ON DELETE CASCADE,
    prerequisite_job_id UUID NOT NULL REFERENCES jobs(id) ON DELETE RESTRICT,
    PRIMARY KEY (receipt_id, prerequisite_job_id)
);

ALTER TABLE jobs
    ADD COLUMN causation_id UUID,
    ADD COLUMN schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    ADD CONSTRAINT jobs_schedule_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(schedule_lineage, 16)
    );

ALTER TABLE backup_requests
    ADD COLUMN causation_id UUID,
    ADD COLUMN schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    ADD CONSTRAINT backup_requests_schedule_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(schedule_lineage, 16)
    );

ALTER TABLE webhook_events
    ADD COLUMN alert_lifecycle_event_seq BIGINT,
    ADD COLUMN causation_id UUID,
    ADD COLUMN schedule_lineage UUID[] NOT NULL DEFAULT ARRAY[]::UUID[],
    ADD CONSTRAINT webhook_events_schedule_lineage_check CHECK (
        alert_uuid_array_is_unique_bounded(schedule_lineage, 16)
    );

-- PostgreSQL cannot enforce a global unique index that omits the occurrence
-- partition key. The non-partitioned lifecycle receipt above is the one-to-one
-- ownership fence; this index serves lookup/pruning on the projected log.
CREATE INDEX webhook_events_lifecycle_seq_idx
    ON webhook_events (alert_lifecycle_event_seq)
    WHERE alert_lifecycle_event_seq IS NOT NULL;

-- The legacy policy table is no longer a lifecycle owner after its rows have
-- been copied. Its former state table is likewise superseded by generic gate
-- state. No compatibility view is retained.
DROP TABLE policy_alerts;
DROP TABLE policy_rule_states;
