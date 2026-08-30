-- Alert evidence, policies, lifecycle queues, webhooks, and notifications.

SET LOCAL check_function_bodies = false;

-- Functions.

CREATE FUNCTION public.advance_alert_policy_current_evidence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    evidence_scope_revision BIGINT;
BEGIN
    IF NEW.subject_client_id IS NULL
       OR NEW.fact_kind NOT IN ('metric', 'state') THEN
        RETURN NEW;
    END IF;
    evidence_scope_revision := COALESCE(
        NULLIF(NEW.subject_snapshot->>'scope_revision', '')::bigint,
        0
    );
    INSERT INTO alert_policy_effective_current_evidence (
        subject_client_id, source_kind, natural_key, fact_kind,
        evidence_id, observed_at, evidence_seq, updated_at
    ) SELECT
        NEW.subject_client_id, NEW.source_kind, NEW.natural_key, NEW.fact_kind,
        NEW.id, NEW.observed_at, NEW.evidence_seq, clock_timestamp()
    FROM clients current_subject
    WHERE current_subject.id=NEW.subject_client_id
    ON CONFLICT (subject_client_id, source_kind, natural_key) DO UPDATE SET
        fact_kind = EXCLUDED.fact_kind,
        evidence_id = EXCLUDED.evidence_id,
        observed_at = EXCLUDED.observed_at,
        evidence_seq = EXCLUDED.evidence_seq,
        updated_at = EXCLUDED.updated_at
    WHERE ROW(
        EXCLUDED.observed_at,
        EXCLUDED.evidence_seq
    ) > ROW(
        alert_policy_effective_current_evidence.observed_at,
        alert_policy_effective_current_evidence.evidence_seq
    );
    IF NEW.source_event_id LIKE 'scope:%' THEN
        RETURN NEW;
    END IF;
    INSERT INTO alert_policy_current_evidence (
        subject_client_id, source_kind, natural_key, fact_kind,
        evidence_id, observed_at, evidence_seq, updated_at
    ) SELECT
        NEW.subject_client_id, NEW.source_kind, NEW.natural_key, NEW.fact_kind,
        NEW.id, NEW.observed_at, NEW.evidence_seq, clock_timestamp()
    FROM clients current_subject
    WHERE current_subject.id=NEW.subject_client_id
      AND current_subject.policy_scope_revision=evidence_scope_revision
    ON CONFLICT (subject_client_id, source_kind, natural_key) DO UPDATE SET
        fact_kind = EXCLUDED.fact_kind,
        evidence_id = EXCLUDED.evidence_id,
        observed_at = EXCLUDED.observed_at,
        evidence_seq = EXCLUDED.evidence_seq,
        updated_at = EXCLUDED.updated_at
    WHERE ROW(
        EXCLUDED.observed_at,
        EXCLUDED.evidence_seq
    ) > ROW(
        alert_policy_current_evidence.observed_at,
        alert_policy_current_evidence.evidence_seq
    );

    -- A stale source snapshot never becomes actual-current. Requeue the
    -- committed revision with an idempotent max-CAS; a later client mutation
    -- independently queues its newer revision, so neither owner waits on the
    -- other and a materializer cannot clear the only repair identity.
    INSERT INTO alert_policy_scope_dirty_clients (
        client_id, target_revision, dirty_at
    ) SELECT
        current_subject.id,
        current_subject.policy_scope_revision,
        clock_timestamp()
    FROM clients current_subject
    WHERE current_subject.id=NEW.subject_client_id
      AND current_subject.policy_scope_revision<>evidence_scope_revision
    ON CONFLICT (client_id) DO UPDATE SET
        target_revision = GREATEST(
            alert_policy_scope_dirty_clients.target_revision,
            EXCLUDED.target_revision
        ),
        dirty_at = CASE
            WHEN EXCLUDED.target_revision
                    > alert_policy_scope_dirty_clients.target_revision
            THEN EXCLUDED.dirty_at
            ELSE alert_policy_scope_dirty_clients.dirty_at
        END;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.alert_policy_meta_condition_valid(value jsonb, allow_elapsed boolean) RETURNS boolean
    LANGUAGE plpgsql IMMUTABLE STRICT
    AS $_$
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
$_$;



CREATE FUNCTION public.bump_policy_scope_revision_for_assignment() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    affected_client_id TEXT;
BEGIN
    affected_client_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.client_id ELSE NEW.client_id END;
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



CREATE FUNCTION public.bump_policy_scope_revision_for_tag_name() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.name IS DISTINCT FROM OLD.name THEN
        UPDATE clients client
        SET policy_scope_revision = client.policy_scope_revision + 1
        FROM client_tags assignment
        WHERE assignment.tag_id = NEW.id AND client.id = assignment.client_id;
    END IF;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.enqueue_displaced_alert_policy_evidence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.evidence_id IS DISTINCT FROM NEW.evidence_id
       AND OLD.fact_kind IN ('metric', 'state') THEN
        INSERT INTO alert_policy_evidence_prune_candidates (
            evidence_id, source_kind, subject_client_id, natural_key
        ) VALUES (
            OLD.evidence_id, OLD.source_kind,
            OLD.subject_client_id, OLD.natural_key
        )
        ON CONFLICT (evidence_id) DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.enqueue_noncurrent_alert_policy_evidence() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.fact_kind IN ('metric', 'state')
       AND NOT EXISTS (
            SELECT 1 FROM alert_policy_current_evidence current_fact
            WHERE current_fact.evidence_id = NEW.id
       )
       AND NOT EXISTS (
            SELECT 1
            FROM alert_policy_effective_current_evidence effective_fact
            WHERE effective_fact.evidence_id = NEW.id
       ) THEN
        INSERT INTO alert_policy_evidence_prune_candidates (
            evidence_id, source_kind, subject_client_id, natural_key
        ) VALUES (
            NEW.id, NEW.source_kind, NEW.subject_client_id, NEW.natural_key
        )
        ON CONFLICT (evidence_id) DO NOTHING;
    END IF;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.queue_alert_policy_scope_revision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO alert_policy_scope_dirty_clients (
        client_id, target_revision, dirty_at
    ) VALUES (
        NEW.id, NEW.policy_scope_revision, clock_timestamp()
    )
    ON CONFLICT (client_id) DO UPDATE SET
        target_revision = GREATEST(
            alert_policy_scope_dirty_clients.target_revision,
            EXCLUDED.target_revision
        ),
        dirty_at = CASE
            WHEN EXCLUDED.target_revision
                    > alert_policy_scope_dirty_clients.target_revision
            THEN EXCLUDED.dirty_at
            ELSE alert_policy_scope_dirty_clients.dirty_at
        END;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.queue_last_seen_policy_scope_rebase() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    must_rebase BOOLEAN := FALSE;
BEGIN
    IF NEW.enabled
       AND lower(NEW.selector_expression) LIKE '%last_seen%' THEN
        IF TG_OP = 'INSERT' THEN
            must_rebase := TRUE;
        ELSE
            must_rebase := NOT OLD.enabled
                OR OLD.selector_expression IS DISTINCT
                    FROM NEW.selector_expression;
        END IF;
    END IF;
    IF NOT must_rebase THEN
        RETURN NEW;
    END IF;
    INSERT INTO alert_policy_scope_dirty_clients (
        client_id, target_revision, requires_revision_advance, dirty_at
    )
    SELECT client.id, client.policy_scope_revision, TRUE, clock_timestamp()
    FROM clients client
    ORDER BY client.id
    ON CONFLICT (client_id) DO UPDATE SET
        target_revision = GREATEST(
            alert_policy_scope_dirty_clients.target_revision,
            EXCLUDED.target_revision
        ),
        requires_revision_advance = TRUE,
        dirty_at = CASE
            WHEN NOT alert_policy_scope_dirty_clients.requires_revision_advance
                 OR EXCLUDED.target_revision
                    > alert_policy_scope_dirty_clients.target_revision
            THEN EXCLUDED.dirty_at
            ELSE alert_policy_scope_dirty_clients.dirty_at
        END;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.set_backup_request_terminal_at() RETURNS trigger
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



CREATE FUNCTION public.set_job_alert_terminal_at() RETURNS trigger
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



CREATE FUNCTION public.set_job_target_capability_alert_at() RETURNS trigger
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



CREATE FUNCTION public.stamp_client_operational_alert_boundaries() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'INSERT' THEN
        NEW.operational_alert_status_at := clock_timestamp();
        NEW.operational_alert_tunnel_boundary_at := clock_timestamp();
    ELSE
        IF OLD.status IS DISTINCT FROM NEW.status THEN
            NEW.operational_alert_status_at := clock_timestamp();
            NEW.operational_alert_tunnel_boundary_at := clock_timestamp();
        ELSIF OLD.process_incarnation_id IS DISTINCT FROM NEW.process_incarnation_id THEN
            NEW.operational_alert_tunnel_boundary_at := clock_timestamp();
        END IF;
    END IF;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.stamp_client_policy_scope_revision() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF ROW(
        NEW.display_name, NEW.status, NEW.registration_ip, NEW.last_ip,
        NEW.internal_build_number, NEW.stale_since,
        NEW.stale_reason, NEW.hidden_at
    ) IS DISTINCT FROM ROW(
        OLD.display_name, OLD.status, OLD.registration_ip, OLD.last_ip,
        OLD.internal_build_number, OLD.stale_since,
        OLD.stale_reason, OLD.hidden_at
    ) OR (
        NEW.last_seen_at IS DISTINCT FROM OLD.last_seen_at
        AND EXISTS (
            SELECT 1
            FROM policy_groups group_row
            WHERE group_row.enabled
              AND lower(group_row.selector_expression) LIKE '%last_seen%'
        )
    ) THEN
        NEW.policy_scope_revision := OLD.policy_scope_revision + 1;
    END IF;
    RETURN NEW;
END;
$$;



CREATE FUNCTION public.stamp_gateway_session_operational_alert_boundary() RETURNS trigger
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



-- Sequences.

CREATE SEQUENCE public.alert_lifecycle_event_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



CREATE SEQUENCE public.alert_policy_evidence_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;



-- Tables.

CREATE TABLE public.alert_policy_evidence (
    evidence_seq bigint DEFAULT nextval('public.alert_policy_evidence_seq'::regclass) NOT NULL,
    id uuid NOT NULL,
    source_kind text NOT NULL,
    source_event_id text NOT NULL,
    fact_kind text NOT NULL,
    natural_key text NOT NULL,
    confirmation_bucket_key text NOT NULL,
    subject_client_id text,
    target_kind text NOT NULL,
    target_id text NOT NULL,
    source_status text NOT NULL,
    completeness text NOT NULL,
    subject_snapshot jsonb NOT NULL,
    payload jsonb NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    state_started_at timestamp with time zone,
    causation_id uuid,
    schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    evaluation_pending boolean NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_evidence_completeness_check CHECK ((completeness = ANY (ARRAY['complete'::text, 'unknown'::text]))),
    CONSTRAINT alert_policy_evidence_fact_kind_check CHECK ((((source_kind = 'telemetry.combined'::text) AND (fact_kind = 'metric'::text)) OR ((source_kind = ANY (ARRAY['agent.status'::text, 'agent.access'::text, 'tunnel.adapter'::text, 'tunnel.traffic'::text])) AND (fact_kind = 'state'::text)) OR ((source_kind = ANY (ARRAY['job.terminal'::text, 'backup.failure'::text, 'job.capability'::text])) AND (fact_kind = 'occurrence'::text)))),
    CONSTRAINT alert_policy_evidence_identity_check CHECK ((((length(btrim(source_event_id)) >= 1) AND (length(btrim(source_event_id)) <= 512)) AND ((length(btrim(natural_key)) >= 1) AND (length(btrim(natural_key)) <= 512)) AND ((length(btrim(confirmation_bucket_key)) >= 1) AND (length(btrim(confirmation_bucket_key)) <= 512)) AND ((length(btrim(target_kind)) >= 1) AND (length(btrim(target_kind)) <= 64)) AND ((length(btrim(target_id)) >= 1) AND (length(btrim(target_id)) <= 512)) AND ((length(btrim(source_status)) >= 1) AND (length(btrim(source_status)) <= 256)))),
    CONSTRAINT alert_policy_evidence_lineage_check CHECK (public.alert_uuid_array_is_unique_bounded(schedule_lineage, 16)),
    CONSTRAINT alert_policy_evidence_payload_check CHECK (((jsonb_typeof(subject_snapshot) = 'object'::text) AND (jsonb_typeof(payload) = 'object'::text))),
    CONSTRAINT alert_policy_evidence_source_kind_check CHECK ((source_kind = ANY (ARRAY['telemetry.combined'::text, 'agent.status'::text, 'agent.access'::text, 'tunnel.adapter'::text, 'tunnel.traffic'::text, 'job.terminal'::text, 'backup.failure'::text, 'job.capability'::text]))),
    CONSTRAINT alert_policy_evidence_state_time_check CHECK ((((fact_kind = 'occurrence'::text) AND (state_started_at IS NULL)) OR ((fact_kind = ANY (ARRAY['metric'::text, 'state'::text])) AND (state_started_at IS NOT NULL)))),
    CONSTRAINT alert_policy_evidence_evidence_seq_key UNIQUE (evidence_seq),
    CONSTRAINT alert_policy_evidence_id_seq_key UNIQUE (id, evidence_seq),
    CONSTRAINT alert_policy_evidence_pkey PRIMARY KEY (id),
    CONSTRAINT alert_policy_evidence_source_event_key UNIQUE (source_kind, source_event_id)
);



CREATE TABLE public.alert_episodes (
    id uuid NOT NULL,
    public_id text NOT NULL,
    producer_kind text NOT NULL,
    natural_key text NOT NULL,
    record_kind text NOT NULL,
    trigger_generation bigint NOT NULL,
    trigger_severity text NOT NULL,
    trigger_category text NOT NULL,
    severity text NOT NULL,
    category text NOT NULL,
    target_kind text NOT NULL,
    target_id text NOT NULL,
    client_id text,
    title text NOT NULL,
    detail text NOT NULL,
    source_status text NOT NULL,
    evidence jsonb NOT NULL,
    lifecycle_state text NOT NULL,
    triggered_at timestamp with time zone NOT NULL,
    last_confirmed_at timestamp with time zone NOT NULL,
    resolved_at timestamp with time zone,
    resolution_reason text,
    resolution_note text,
    resolution_actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    policy_group_id uuid NOT NULL,
    policy_rule_id uuid NOT NULL,
    policy_rule_version integer NOT NULL,
    policy_rule_kind text NOT NULL,
    policy_group_name text NOT NULL,
    policy_rule_name text NOT NULL,
    policy_rule_system_seed_key text,
    trigger_evidence_id uuid,
    last_evidence_id uuid,
    causation_id uuid,
    schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    CONSTRAINT alert_episodes_category_check CHECK (((category = ANY (ARRAY['agent_status'::text, 'network'::text, 'backup'::text, 'agent_update'::text, 'job'::text, 'capability_degraded'::text, 'traffic'::text, 'resource'::text])) AND (trigger_category = ANY (ARRAY['agent_status'::text, 'network'::text, 'backup'::text, 'agent_update'::text, 'job'::text, 'capability_degraded'::text, 'traffic'::text, 'resource'::text])))),
    CONSTRAINT alert_episodes_lifecycle_check CHECK ((((lifecycle_state = ANY (ARRAY['triggered'::text, 'persisting'::text])) AND (last_confirmed_at >= triggered_at) AND (resolved_at IS NULL) AND (resolution_reason IS NULL) AND (resolution_note IS NULL) AND (resolution_actor_id IS NULL)) OR ((lifecycle_state = 'unknown'::text) AND (record_kind = 'condition'::text) AND (last_confirmed_at >= triggered_at) AND (resolved_at IS NULL) AND (resolution_reason IS NULL) AND (resolution_note IS NULL) AND (resolution_actor_id IS NULL)) OR ((lifecycle_state = 'resolved'::text) AND (last_confirmed_at >= triggered_at) AND (resolved_at IS NOT NULL) AND (resolved_at >= last_confirmed_at) AND (resolution_reason IS NOT NULL) AND (((record_kind = 'event'::text) AND (resolution_reason = 'operator_resolved'::text) AND (resolution_note IS NOT NULL) AND (resolution_actor_id IS NOT NULL)) OR ((record_kind = 'event'::text) AND (resolution_reason = ANY (ARRAY['policy_time_elapsed'::text, 'source_scope_exited'::text, 'policy_scope_exited'::text, 'policy_scope_changed'::text, 'policy_disabled'::text, 'policy_changed'::text, 'policy_deleted'::text])) AND (resolution_note IS NULL) AND (resolution_actor_id IS NULL)) OR ((record_kind = 'condition'::text) AND (resolution_reason = ANY (ARRAY['condition_recovered'::text, 'recovery_expression_matched'::text, 'source_scope_exited'::text, 'policy_scope_exited'::text, 'policy_scope_changed'::text, 'policy_disabled'::text, 'policy_changed'::text, 'policy_deleted'::text])) AND (resolution_note IS NULL) AND (resolution_actor_id IS NULL)))))),
    CONSTRAINT alert_episodes_lineage_check CHECK (public.alert_uuid_array_is_unique_bounded(schedule_lineage, 16)),
    CONSTRAINT alert_episodes_policy_provenance_check CHECK (((policy_rule_version >= 1) AND (policy_rule_kind = ANY (ARRAY['metric'::text, 'state'::text, 'occurrence'::text])))),
    CONSTRAINT alert_episodes_producer_check CHECK ((((length(btrim(producer_kind)) >= 1) AND (length(btrim(producer_kind)) <= 128)) AND (producer_kind ~ '^[a-z][a-z0-9_.-]*$'::text))),
    CONSTRAINT alert_episodes_resolution_reason_check CHECK (((resolution_reason IS NULL) OR (resolution_reason = ANY (ARRAY['condition_recovered'::text, 'recovery_expression_matched'::text, 'policy_time_elapsed'::text, 'source_scope_exited'::text, 'policy_scope_exited'::text, 'policy_scope_changed'::text, 'policy_disabled'::text, 'policy_changed'::text, 'policy_deleted'::text, 'operator_resolved'::text])))),
    CONSTRAINT alert_episodes_rule_record_kind_check CHECK ((((policy_rule_kind = ANY (ARRAY['metric'::text, 'state'::text])) AND (record_kind = 'condition'::text)) OR ((policy_rule_kind = 'occurrence'::text) AND (record_kind = 'event'::text)))),
    CONSTRAINT alert_episodes_detail_check CHECK (((length(btrim(detail)) >= 1) AND (length(btrim(detail)) <= 4096))),
    CONSTRAINT alert_episodes_evidence_object_check CHECK ((jsonb_typeof(evidence) = 'object'::text)),
    CONSTRAINT alert_episodes_generation_check CHECK ((trigger_generation >= 1)),
    CONSTRAINT alert_episodes_natural_key_check CHECK (((length(btrim(natural_key)) >= 1) AND (length(btrim(natural_key)) <= 512))),
    CONSTRAINT alert_episodes_public_id_check CHECK (((length(btrim(public_id)) >= 1) AND (length(btrim(public_id)) <= 192))),
    CONSTRAINT alert_episodes_record_kind_check CHECK ((record_kind = ANY (ARRAY['condition'::text, 'event'::text]))),
    CONSTRAINT alert_episodes_resolution_note_check CHECK (((resolution_note IS NULL) OR ((length(btrim(resolution_note)) >= 1) AND (length(btrim(resolution_note)) <= 1024)))),
    CONSTRAINT alert_episodes_severity_check CHECK ((severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))),
    CONSTRAINT alert_episodes_source_status_check CHECK (((length(btrim(source_status)) >= 1) AND (length(btrim(source_status)) <= 256))),
    CONSTRAINT alert_episodes_title_check CHECK (((length(btrim(title)) >= 1) AND (length(btrim(title)) <= 256))),
    CONSTRAINT alert_episodes_trigger_severity_check CHECK ((trigger_severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))),
    CONSTRAINT alert_episodes_pkey PRIMARY KEY (id),
    CONSTRAINT alert_episodes_public_id_key UNIQUE (public_id),
    CONSTRAINT alert_episodes_last_evidence_id_fkey FOREIGN KEY (last_evidence_id) REFERENCES public.alert_policy_evidence(id),
    CONSTRAINT alert_episodes_trigger_evidence_id_fkey FOREIGN KEY (trigger_evidence_id) REFERENCES public.alert_policy_evidence(id),
    CONSTRAINT alert_episodes_resolution_actor_id_fkey FOREIGN KEY (resolution_actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.alert_lifecycle_events (
    event_seq bigint DEFAULT nextval('public.alert_lifecycle_event_seq'::regclass) NOT NULL,
    id uuid NOT NULL,
    episode_id uuid NOT NULL,
    trigger_generation bigint NOT NULL,
    edge_kind text NOT NULL,
    event_id text NOT NULL,
    event_predicates text[] NOT NULL,
    subject_client_ids text[] DEFAULT ARRAY[]::text[] NOT NULL,
    payload jsonb NOT NULL,
    causation_id uuid,
    schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    occurred_at timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_lifecycle_events_event_id_check CHECK (((length(btrim(event_id)) >= 1) AND (length(btrim(event_id)) <= 256))),
    CONSTRAINT alert_lifecycle_events_kind_check CHECK ((edge_kind = ANY (ARRAY['alert.triggered'::text, 'alert.resolved'::text]))),
    CONSTRAINT alert_lifecycle_events_lineage_check CHECK (public.alert_uuid_array_is_unique_bounded(schedule_lineage, 16)),
    CONSTRAINT alert_lifecycle_events_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT alert_lifecycle_events_predicate_check CHECK (((cardinality(event_predicates) >= 1) AND (event_predicates @> ARRAY[edge_kind]) AND (event_predicates <@ ARRAY['alert.triggered'::text, 'alert.resolved'::text, 'alert.category:agent_status'::text, 'alert.category:network'::text, 'alert.category:backup'::text, 'alert.category:agent_update'::text, 'alert.category:job'::text, 'alert.category:capability_degraded'::text, 'alert.category:traffic'::text, 'alert.category:resource'::text, 'alert.severity:info'::text, 'alert.severity:warning'::text, 'alert.severity:critical'::text]))),
    CONSTRAINT alert_lifecycle_events_episode_edge_key UNIQUE (episode_id, trigger_generation, edge_kind),
    CONSTRAINT alert_lifecycle_events_id_key UNIQUE (id),
    CONSTRAINT alert_lifecycle_events_kind_event_id_key UNIQUE (edge_kind, event_id),
    CONSTRAINT alert_lifecycle_events_pkey PRIMARY KEY (event_seq),
    CONSTRAINT alert_lifecycle_events_episode_id_fkey FOREIGN KEY (episode_id) REFERENCES public.alert_episodes(id) ON DELETE RESTRICT
);



CREATE TABLE public.alert_lifecycle_consumer_receipts (
    consumer_kind text NOT NULL,
    event_seq bigint NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    claim_id uuid,
    output_id uuid,
    output_occurred_at timestamp with time zone,
    error text,
    attempt_count integer DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_lifecycle_consumer_receipts_consumer_check CHECK ((consumer_kind = ANY (ARRAY['webhook'::text, 'schedule'::text]))),
    CONSTRAINT alert_lifecycle_consumer_receipts_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'in_progress'::text, 'completed'::text, 'failed'::text]))),
    CONSTRAINT alert_lifecycle_consumer_receipts_claim_check CHECK ((((status = 'in_progress'::text) AND (claim_id IS NOT NULL)) OR ((status <> 'in_progress'::text) AND (claim_id IS NULL)))),
    CONSTRAINT alert_lifecycle_consumer_receipts_output_check CHECK ((((consumer_kind = 'webhook'::text) AND (status = 'completed'::text) AND (output_id IS NOT NULL) AND (output_occurred_at IS NOT NULL)) OR ((consumer_kind = 'webhook'::text) AND (status <> 'completed'::text) AND (output_id IS NULL) AND (output_occurred_at IS NULL)) OR ((consumer_kind = 'schedule'::text) AND (output_id IS NULL) AND (output_occurred_at IS NULL)))),
    CONSTRAINT alert_lifecycle_consumer_receipts_attempt_check CHECK ((attempt_count >= 0)),
    CONSTRAINT alert_lifecycle_consumer_receipts_pkey PRIMARY KEY (consumer_kind, event_seq),
    CONSTRAINT alert_lifecycle_consumer_receipts_webhook_output_key UNIQUE (output_id),
    CONSTRAINT alert_lifecycle_consumer_receipts_event_seq_fkey FOREIGN KEY (event_seq) REFERENCES public.alert_lifecycle_events(event_seq) ON DELETE RESTRICT
);



CREATE TABLE public.alert_policy_current_evidence (
    subject_client_id text NOT NULL,
    source_kind text NOT NULL,
    natural_key text NOT NULL,
    fact_kind text NOT NULL,
    evidence_id uuid NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    evidence_seq bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_current_evidence_fact_kind_check CHECK ((fact_kind = ANY (ARRAY['metric'::text, 'state'::text]))),
    CONSTRAINT alert_policy_current_evidence_natural_key_check CHECK (((length(btrim(natural_key)) >= 1) AND (length(btrim(natural_key)) <= 512))),
    CONSTRAINT alert_policy_current_evidence_source_kind_check CHECK (((length(btrim(source_kind)) >= 1) AND (length(btrim(source_kind)) <= 64))),
    CONSTRAINT alert_policy_current_evidence_evidence_id_key UNIQUE (evidence_id),
    CONSTRAINT alert_policy_current_evidence_pkey PRIMARY KEY (subject_client_id, source_kind, natural_key),
    CONSTRAINT alert_policy_current_evidence_evidence_id_fkey FOREIGN KEY (evidence_id) REFERENCES public.alert_policy_evidence(id) ON DELETE RESTRICT,
    CONSTRAINT alert_policy_current_evidence_subject_client_id_fkey FOREIGN KEY (subject_client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_effective_current_evidence (
    subject_client_id text NOT NULL,
    source_kind text NOT NULL,
    natural_key text NOT NULL,
    fact_kind text NOT NULL,
    evidence_id uuid NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    evidence_seq bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_effective_current_evidence_fact_kind_check CHECK ((fact_kind = ANY (ARRAY['metric'::text, 'state'::text]))),
    CONSTRAINT alert_policy_effective_current_evidence_natural_key_check CHECK (((length(btrim(natural_key)) >= 1) AND (length(btrim(natural_key)) <= 512))),
    CONSTRAINT alert_policy_effective_current_evidence_source_kind_check CHECK (((length(btrim(source_kind)) >= 1) AND (length(btrim(source_kind)) <= 64))),
    CONSTRAINT alert_policy_effective_current_evidence_evidence_id_key UNIQUE (evidence_id),
    CONSTRAINT alert_policy_effective_current_evidence_pkey PRIMARY KEY (subject_client_id, source_kind, natural_key),
    CONSTRAINT alert_policy_effective_current_evidence_evidence_id_fkey FOREIGN KEY (evidence_id) REFERENCES public.alert_policy_evidence(id) ON DELETE RESTRICT,
    CONSTRAINT alert_policy_effective_current_evidence_subject_client_id_fkey FOREIGN KEY (subject_client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_evidence_prune_candidates (
    evidence_id uuid NOT NULL,
    source_kind text NOT NULL,
    subject_client_id text,
    natural_key text NOT NULL,
    enqueued_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    last_attempted_at timestamp with time zone,
    CONSTRAINT alert_policy_evidence_prune_candidates_natural_key_check CHECK (((length(btrim(natural_key)) >= 1) AND (length(btrim(natural_key)) <= 512))),
    CONSTRAINT alert_policy_evidence_prune_candidates_source_kind_check CHECK (((length(btrim(source_kind)) >= 1) AND (length(btrim(source_kind)) <= 64))),
    CONSTRAINT alert_policy_evidence_prune_candidates_pkey PRIMARY KEY (evidence_id),
    CONSTRAINT alert_policy_evidence_prune_candidates_evidence_id_fkey FOREIGN KEY (evidence_id) REFERENCES public.alert_policy_evidence(id) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_lifecycle_meta (
    singleton boolean DEFAULT true NOT NULL,
    evidence_retention_days integer DEFAULT 30 NOT NULL,
    evidence_pruned_through_seq bigint DEFAULT 0 NOT NULL,
    CONSTRAINT alert_policy_lifecycle_meta_evidence_pruned_through_seq_check CHECK ((evidence_pruned_through_seq >= 0)),
    CONSTRAINT alert_policy_lifecycle_meta_evidence_retention_days_check CHECK (((evidence_retention_days >= 1) AND (evidence_retention_days <= 3650))),
    CONSTRAINT alert_policy_lifecycle_meta_singleton_check CHECK (singleton),
    CONSTRAINT alert_policy_lifecycle_meta_pkey PRIMARY KEY (singleton)
);



-- Desired telemetry-policy definitions are public immediately, while their
-- first effective generation waits for one exact current sample per client to
-- become a non-triggering arm baseline.  This singleton is only the short
-- publication fence; fleet work belongs to the rows below.
CREATE TABLE public.alert_telemetry_policy_activation (
    singleton boolean DEFAULT true NOT NULL,
    generation bigint DEFAULT 0 NOT NULL,
    desired_enabled boolean DEFAULT false NOT NULL,
    seeded_generation bigint,
    effective_generation bigint,
    boundary_evidence_seq bigint DEFAULT 0 NOT NULL,
    requested_at timestamp with time zone,
    seeded_at timestamp with time zone,
    effective_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_telemetry_policy_activation_generation_check CHECK (
        generation >= 0
        AND boundary_evidence_seq >= 0
        AND (seeded_generation IS NULL
            OR seeded_generation BETWEEN 1 AND generation)
        AND (effective_generation IS NULL
            OR effective_generation BETWEEN 1 AND generation)
    ),
    CONSTRAINT alert_telemetry_policy_activation_state_check CHECK (
        (NOT desired_enabled
            AND seeded_generation IS NULL
            AND effective_generation IS NULL
            AND boundary_evidence_seq = 0
            AND requested_at IS NULL
            AND seeded_at IS NULL
            AND effective_at IS NULL)
        OR (desired_enabled
            AND generation >= 1
            AND requested_at IS NOT NULL
            AND (seeded_generation IS NULL
                OR (seeded_generation = generation AND seeded_at IS NOT NULL))
            AND (effective_generation IS NULL
                OR (effective_generation = generation
                    AND seeded_generation = generation
                    AND effective_at IS NOT NULL)))
    ),
    CONSTRAINT alert_telemetry_policy_activation_singleton_check CHECK (singleton),
    CONSTRAINT alert_telemetry_policy_activation_pkey PRIMARY KEY (singleton)
);



-- One row is the durable owner of one activation generation's latest accepted
-- sample for one client.  The target sample FK keeps that exact baseline alive
-- until the consumer's revision/token-fenced acknowledgement commits.
CREATE TABLE public.alert_telemetry_policy_activation_work (
    activation_generation bigint NOT NULL,
    client_id text NOT NULL,
    target_accepted_seq bigint NOT NULL,
    target_sample_id uuid NOT NULL,
    work_revision bigint DEFAULT 1 NOT NULL,
    claim_token uuid,
    claim_revision bigint,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_telemetry_policy_activation_work_target_check CHECK (
        activation_generation >= 1
        AND target_accepted_seq >= 1
        AND work_revision >= 1
    ),
    CONSTRAINT alert_telemetry_policy_activation_work_claim_check CHECK (
        (claim_token IS NULL AND claim_revision IS NULL)
        OR (claim_token IS NOT NULL AND claim_revision = work_revision)
    ),
    CONSTRAINT alert_telemetry_policy_activation_work_pkey PRIMARY KEY (
        activation_generation, client_id
    ),
    CONSTRAINT alert_telemetry_policy_activation_work_client_id_fkey
        FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT alert_telemetry_policy_activation_work_target_sample_id_fkey
        FOREIGN KEY (target_sample_id) REFERENCES public.telemetry_samples(id) ON DELETE RESTRICT
);



CREATE TABLE public.alert_policy_scope_dirty_clients (
    client_id text NOT NULL,
    target_revision bigint NOT NULL,
    requires_revision_advance boolean DEFAULT false NOT NULL,
    dirty_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_scope_dirty_clients_target_revision_check CHECK ((target_revision >= 1)),
    CONSTRAINT alert_policy_scope_dirty_clients_pkey PRIMARY KEY (client_id),
    CONSTRAINT alert_policy_scope_dirty_clients_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.fleet_alert_notification_channels (
    id uuid NOT NULL,
    name text NOT NULL,
    scope_kind text NOT NULL,
    scope_value text,
    min_severity text NOT NULL,
    categories jsonb DEFAULT '[]'::jsonb NOT NULL,
    operator_states jsonb DEFAULT '[]'::jsonb NOT NULL,
    delivery_kind text NOT NULL,
    target text NOT NULL,
    cooldown_secs bigint DEFAULT 3600 NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    notes text,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT fleet_alert_notification_channels_categories_check CHECK ((jsonb_typeof(categories) = 'array'::text)),
    CONSTRAINT fleet_alert_notification_channels_check CHECK ((((scope_kind = 'global'::text) AND (scope_value IS NULL)) OR ((scope_kind <> 'global'::text) AND (scope_value IS NOT NULL)))),
    CONSTRAINT fleet_alert_notification_channels_cooldown_secs_check CHECK (((cooldown_secs >= 0) AND (cooldown_secs <= 2592000))),
    CONSTRAINT fleet_alert_notification_channels_min_severity_check CHECK ((min_severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))),
    CONSTRAINT fleet_alert_notification_channels_operator_states_check CHECK ((jsonb_typeof(operator_states) = 'array'::text)),
    CONSTRAINT fleet_alert_notification_channels_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['global'::text, 'provider'::text, 'tag'::text, 'client'::text]))),
    CONSTRAINT fleet_alert_notification_channels_name_key UNIQUE (name),
    CONSTRAINT fleet_alert_notification_channels_pkey PRIMARY KEY (id),
    CONSTRAINT fleet_alert_notification_channels_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.fleet_alert_notification_deliveries (
    id uuid NOT NULL,
    channel_id uuid NOT NULL,
    channel_name text NOT NULL,
    alert_id text NOT NULL,
    alert_severity text NOT NULL,
    alert_category text NOT NULL,
    status text NOT NULL,
    delivery_kind text NOT NULL,
    target text NOT NULL,
    dedupe_key text NOT NULL,
    payload jsonb NOT NULL,
    error text,
    cooldown_until_unix bigint NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    eligibility_revision bigint DEFAULT 0 NOT NULL,
    delivery_lease_id uuid,
    delivery_lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone,
    last_attempt_at timestamp with time zone,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivered_at timestamp with time zone,
    CONSTRAINT fleet_alert_notification_deliveries_alert_severity_check CHECK ((alert_severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))),
    CONSTRAINT fleet_alert_notification_deliveries_cooldown_until_unix_check CHECK ((cooldown_until_unix >= 0)),
    CONSTRAINT fleet_alert_notification_deliveries_eligibility_revision_check CHECK ((eligibility_revision >= 0)),
    CONSTRAINT fleet_alert_notification_deliveries_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'in_progress'::text, 'failed'::text, 'permanently_failed'::text, 'canceled_disabled'::text, 'delivered'::text, 'matched_dry_run'::text]))),
    CONSTRAINT fleet_alert_notification_deliveries_pkey PRIMARY KEY (id),
    CONSTRAINT fleet_alert_notification_deliveries_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.fleet_alert_states (
    alert_id text NOT NULL,
    state text NOT NULL,
    muted_until_unix bigint,
    escalation_level integer DEFAULT 0 NOT NULL,
    reason text,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    CONSTRAINT fleet_alert_states_check CHECK ((((state = 'muted'::text) AND (muted_until_unix IS NOT NULL)) OR (state <> 'muted'::text))),
    CONSTRAINT fleet_alert_states_escalation_level_check CHECK ((escalation_level >= 0)),
    CONSTRAINT fleet_alert_states_revision_check CHECK ((revision >= 0)),
    CONSTRAINT fleet_alert_states_state_check CHECK ((state = ANY (ARRAY['open'::text, 'acknowledged'::text, 'muted'::text, 'escalated'::text]))),
    CONSTRAINT fleet_alert_states_pkey PRIMARY KEY (alert_id),
    CONSTRAINT fleet_alert_states_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.policy_groups (
    id uuid NOT NULL,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    selector_expression text NOT NULL,
    notes text,
    created_by uuid,
    updated_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT policy_groups_name_check CHECK (((length(TRIM(BOTH FROM name)) >= 1) AND (length(TRIM(BOTH FROM name)) <= 128))),
    CONSTRAINT policy_groups_notes_check CHECK (((notes IS NULL) OR (length(notes) <= 1024))),
    CONSTRAINT policy_groups_selector_expression_check CHECK (((length(TRIM(BOTH FROM selector_expression)) >= 1) AND (length(TRIM(BOTH FROM selector_expression)) <= 4096))),
    CONSTRAINT policy_groups_name_key UNIQUE (name),
    CONSTRAINT policy_groups_pkey PRIMARY KEY (id),
    CONSTRAINT policy_groups_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.operators(id),
    CONSTRAINT policy_groups_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id)
);



CREATE TABLE public.policy_rules (
    id uuid NOT NULL,
    group_id uuid NOT NULL,
    rule_version integer DEFAULT 1 NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    traffic_selector text,
    trigger_condition_expression text NOT NULL,
    severity text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    rule_kind text DEFAULT 'metric'::text NOT NULL,
    evidence_source text NOT NULL,
    correlation_mode text DEFAULT 'natural_key'::text NOT NULL,
    category text NOT NULL,
    title_template text NOT NULL,
    detail_template text NOT NULL,
    trigger_meta_condition jsonb,
    resolve_condition_expression text,
    resolve_meta_condition jsonb,
    system_seed_key text,
    armed_after_evidence_seq bigint DEFAULT 0 NOT NULL,
    armed_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT policy_rules_armed_after_evidence_seq_check CHECK ((armed_after_evidence_seq >= 0)),
    CONSTRAINT policy_rules_category_check CHECK ((category = ANY (ARRAY['agent_status'::text, 'network'::text, 'backup'::text, 'agent_update'::text, 'job'::text, 'capability_degraded'::text, 'traffic'::text, 'resource'::text]))),
    CONSTRAINT policy_rules_correlation_mode_check CHECK (((correlation_mode = ANY (ARRAY['natural_key'::text, 'subject'::text, 'global'::text])) AND (((rule_kind = ANY (ARRAY['metric'::text, 'state'::text])) AND (correlation_mode = 'natural_key'::text)) OR ((rule_kind = 'occurrence'::text) AND (((COALESCE((trigger_meta_condition ->> 'kind'::text), 'immediate'::text) = 'count'::text) AND (correlation_mode = ANY (ARRAY['subject'::text, 'global'::text]))) OR ((COALESCE((trigger_meta_condition ->> 'kind'::text), 'immediate'::text) = 'immediate'::text) AND (correlation_mode = 'natural_key'::text))))))),
    CONSTRAINT policy_rules_evidence_source_check CHECK ((evidence_source = ANY (ARRAY['telemetry.combined'::text, 'agent.status'::text, 'agent.access'::text, 'tunnel.adapter'::text, 'tunnel.traffic'::text, 'job.terminal'::text, 'backup.failure'::text, 'job.capability'::text]))),
    CONSTRAINT policy_rules_name_check CHECK (((length(TRIM(BOTH FROM name)) >= 1) AND (length(TRIM(BOTH FROM name)) <= 128))),
    CONSTRAINT policy_rules_occurrence_correlation_check CHECK (((rule_kind <> 'occurrence'::text) OR ((COALESCE((trigger_meta_condition ->> 'kind'::text), 'immediate'::text) = 'count'::text) AND (correlation_mode = ANY (ARRAY['subject'::text, 'global'::text])) AND ((evidence_source <> 'job.terminal'::text) OR (correlation_mode = 'global'::text))) OR ((COALESCE((trigger_meta_condition ->> 'kind'::text), 'immediate'::text) = 'immediate'::text) AND (correlation_mode = 'natural_key'::text)))),
    CONSTRAINT policy_rules_presentation_check CHECK ((((length(btrim(title_template)) >= 1) AND (length(btrim(title_template)) <= 256)) AND ((length(btrim(detail_template)) >= 1) AND (length(btrim(detail_template)) <= 4096)))),
    CONSTRAINT policy_rules_resolve_expression_check CHECK (((resolve_condition_expression IS NULL) OR ((length(btrim(resolve_condition_expression)) >= 1) AND (length(btrim(resolve_condition_expression)) <= 4096)))),
    CONSTRAINT policy_rules_resolve_phase_check CHECK ((((rule_kind = ANY (ARRAY['metric'::text, 'state'::text])) AND ((resolve_meta_condition IS NULL) OR (public.alert_policy_meta_condition_valid(resolve_meta_condition, false) AND ((resolve_meta_condition ->> 'kind'::text) <> ALL (ARRAY['immediate'::text, 'elapsed_since_trigger'::text]))))) OR ((rule_kind = 'occurrence'::text) AND (resolve_condition_expression IS NULL) AND (resolve_meta_condition IS NOT NULL) AND public.alert_policy_meta_condition_valid(resolve_meta_condition, true) AND ((resolve_meta_condition ->> 'kind'::text) = 'elapsed_since_trigger'::text)))),
    CONSTRAINT policy_rules_rule_kind_check CHECK ((rule_kind = ANY (ARRAY['metric'::text, 'state'::text, 'occurrence'::text]))),
    CONSTRAINT policy_rules_severity_check CHECK ((severity = ANY (ARRAY['info'::text, 'warning'::text, 'critical'::text]))),
    CONSTRAINT policy_rules_source_kind_check CHECK ((((evidence_source = 'telemetry.combined'::text) AND (rule_kind = 'metric'::text)) OR ((evidence_source = ANY (ARRAY['agent.status'::text, 'agent.access'::text, 'tunnel.adapter'::text, 'tunnel.traffic'::text])) AND (rule_kind = 'state'::text)) OR ((evidence_source = ANY (ARRAY['job.terminal'::text, 'backup.failure'::text, 'job.capability'::text])) AND (rule_kind = 'occurrence'::text)))),
    CONSTRAINT policy_rules_system_seed_key_check CHECK (((system_seed_key IS NULL) OR (((length(system_seed_key) >= 1) AND (length(system_seed_key) <= 128)) AND (system_seed_key ~ '^[a-z][a-z0-9_.-]*$'::text)))),
    CONSTRAINT policy_rules_trigger_expression_check CHECK (((length(btrim(trigger_condition_expression)) >= 1) AND (length(btrim(trigger_condition_expression)) <= 4096))),
    CONSTRAINT policy_rules_trigger_meta_check CHECK (((trigger_meta_condition IS NULL) OR (public.alert_policy_meta_condition_valid(trigger_meta_condition, false) AND ((trigger_meta_condition ->> 'kind'::text) <> ALL (ARRAY['immediate'::text, 'elapsed_since_trigger'::text])) AND ((rule_kind <> 'occurrence'::text) OR ((trigger_meta_condition ->> 'kind'::text) = ANY (ARRAY['immediate'::text, 'count'::text])))))),
    CONSTRAINT policy_rules_pkey PRIMARY KEY (id),
    CONSTRAINT policy_rules_id_version_key UNIQUE (id, rule_version),
    CONSTRAINT policy_rules_group_id_fkey FOREIGN KEY (group_id) REFERENCES public.policy_groups(id) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_evidence_targets (
    evidence_id uuid NOT NULL,
    evidence_seq bigint NOT NULL,
    policy_rule_id uuid NOT NULL,
    rule_version integer NOT NULL,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_evidence_targets_pkey PRIMARY KEY (policy_rule_id, rule_version, evidence_seq),
    CONSTRAINT alert_policy_evidence_targets_evidence_fkey FOREIGN KEY (evidence_id, evidence_seq) REFERENCES public.alert_policy_evidence(id, evidence_seq) ON DELETE CASCADE,
    CONSTRAINT alert_policy_evidence_targets_rule_generation_fkey FOREIGN KEY (policy_rule_id, rule_version) REFERENCES public.policy_rules(id, rule_version) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_confirmations (
    policy_rule_id uuid NOT NULL,
    rule_version integer NOT NULL,
    confirmation_bucket_key text NOT NULL,
    phase text NOT NULL,
    evidence_id uuid NOT NULL,
    accepted_at timestamp with time zone NOT NULL,
    confirmation_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    confirmation_lineage_overflow boolean DEFAULT false NOT NULL,
    CONSTRAINT alert_policy_confirmations_lineage_check CHECK ((public.alert_uuid_array_is_unique_bounded(confirmation_lineage, 16) AND ((NOT confirmation_lineage_overflow) OR (cardinality(confirmation_lineage) = 16)))),
    CONSTRAINT alert_policy_confirmations_phase_check CHECK ((phase = ANY (ARRAY['trigger'::text, 'resolve'::text]))),
    CONSTRAINT alert_policy_confirmations_pkey PRIMARY KEY (policy_rule_id, rule_version, confirmation_bucket_key, phase, evidence_id),
    CONSTRAINT alert_policy_confirmations_evidence_id_fkey FOREIGN KEY (evidence_id) REFERENCES public.alert_policy_evidence(id) ON DELETE RESTRICT,
    CONSTRAINT alert_policy_confirmations_policy_rule_id_fkey FOREIGN KEY (policy_rule_id) REFERENCES public.policy_rules(id) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_evaluation_states (
    policy_rule_id uuid NOT NULL,
    rule_version integer NOT NULL,
    confirmation_bucket_key text NOT NULL,
    occurrence_cohort_id uuid,
    subject_client_id text,
    truth_state text NOT NULL,
    last_evidence_id uuid,
    last_evidence_seq bigint,
    last_evidence_source_event_id text,
    last_evidence_observed_at timestamp with time zone,
    trigger_confirmed_duration_secs bigint DEFAULT 0 NOT NULL,
    trigger_segment_started_at timestamp with time zone,
    resolve_confirmed_duration_secs bigint DEFAULT 0 NOT NULL,
    resolve_segment_started_at timestamp with time zone,
    trigger_generation bigint DEFAULT 0 NOT NULL,
    active_episode_id uuid,
    next_transition_at timestamp with time zone,
    last_evaluated_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_evaluation_states_durations_check CHECK (((trigger_confirmed_duration_secs >= 0) AND (resolve_confirmed_duration_secs >= 0))),
    CONSTRAINT alert_policy_evaluation_states_generation_check CHECK ((trigger_generation >= 0)),
    CONSTRAINT alert_policy_evaluation_states_truth_check CHECK ((truth_state = ANY (ARRAY['matched'::text, 'not_matched'::text, 'unknown'::text]))),
    CONSTRAINT alert_policy_evaluation_states_pkey PRIMARY KEY (policy_rule_id, rule_version, confirmation_bucket_key),
    CONSTRAINT alert_policy_evaluation_states_last_evidence_id_fkey FOREIGN KEY (last_evidence_id) REFERENCES public.alert_policy_evidence(id),
    CONSTRAINT alert_policy_evaluation_states_last_evidence_seq_fkey FOREIGN KEY (last_evidence_seq) REFERENCES public.alert_policy_evidence(evidence_seq),
    CONSTRAINT alert_policy_evaluation_states_policy_rule_id_fkey FOREIGN KEY (policy_rule_id) REFERENCES public.policy_rules(id) ON DELETE CASCADE
);



CREATE TABLE public.alert_policy_evidence_receipts (
    policy_rule_id uuid NOT NULL,
    rule_version integer NOT NULL,
    evidence_seq bigint NOT NULL,
    evidence_id uuid NOT NULL,
    natural_key text NOT NULL,
    confirmation_bucket_key text NOT NULL,
    result text NOT NULL,
    detail text,
    evaluated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT alert_policy_evidence_receipts_result_check CHECK ((result = ANY (ARRAY['matched'::text, 'not_matched'::text, 'unknown'::text, 'out_of_scope'::text, 'source_scope_exited'::text, 'pre_armed'::text, 'stale'::text, 'error'::text, 'lineage_overflow'::text]))),
    CONSTRAINT alert_policy_evidence_receipts_pkey PRIMARY KEY (policy_rule_id, rule_version, evidence_seq),
    CONSTRAINT alert_policy_evidence_receipts_evidence_id_fkey FOREIGN KEY (evidence_id) REFERENCES public.alert_policy_evidence(id) ON DELETE RESTRICT,
    CONSTRAINT alert_policy_evidence_receipts_evidence_seq_fkey FOREIGN KEY (evidence_seq) REFERENCES public.alert_policy_evidence(evidence_seq) ON DELETE RESTRICT,
    CONSTRAINT alert_policy_evidence_receipts_policy_rule_id_fkey FOREIGN KEY (policy_rule_id) REFERENCES public.policy_rules(id) ON DELETE CASCADE
);



CREATE TABLE public.schedule_event_receipts (
    id uuid NOT NULL,
    schedule_id uuid NOT NULL,
    definition_revision bigint NOT NULL,
    actor_id uuid,
    schedule_name text NOT NULL,
    event_seq bigint NOT NULL,
    event_kind text NOT NULL,
    event_id text NOT NULL,
    episode_id uuid NOT NULL,
    trigger_generation bigint NOT NULL,
    edge_ordinal integer NOT NULL,
    status text NOT NULL,
    status_reason text,
    source_occurred_at timestamp with time zone NOT NULL,
    source_payload_hash text NOT NULL,
    matched_subject_client_ids text[] NOT NULL,
    fixed_target_client_ids text[] NOT NULL,
    causation_id uuid NOT NULL,
    source_schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    dispatched_schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    rendered_operation jsonb,
    rendered_operation_hash text,
    job_id uuid,
    error text,
    created_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    dispatched_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT schedule_event_receipts_edge_check CHECK (((event_kind = ANY (ARRAY['alert.triggered'::text, 'alert.resolved'::text])) AND (edge_ordinal = ANY (ARRAY[1, 2])))),
    CONSTRAINT schedule_event_receipts_hash_check CHECK (((source_payload_hash ~ '^[0-9a-f]{64}$'::text) AND ((rendered_operation_hash IS NULL) OR (rendered_operation_hash ~ '^[0-9a-f]{64}$'::text)))),
    CONSTRAINT schedule_event_receipts_lineage_check CHECK ((public.alert_uuid_array_is_unique_bounded(source_schedule_lineage, 16) AND public.alert_uuid_array_is_unique_bounded(dispatched_schedule_lineage, 16))),
    CONSTRAINT schedule_event_receipts_result_check CHECK ((((status = 'dispatched'::text) AND (job_id IS NOT NULL) AND (dispatched_at IS NOT NULL)) OR ((status <> 'dispatched'::text) AND (job_id IS NULL) AND (dispatched_at IS NULL)))),
    CONSTRAINT schedule_event_receipts_schedule_name_check CHECK (((length(btrim(schedule_name)) >= 1) AND (length(btrim(schedule_name)) <= 256))),
    CONSTRAINT schedule_event_receipts_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'dispatched'::text, 'skipped'::text, 'superseded'::text, 'lineage_overflow'::text, 'failed'::text]))),
    CONSTRAINT schedule_event_receipts_job_id_key UNIQUE (job_id),
    CONSTRAINT schedule_event_receipts_pkey PRIMARY KEY (id),
    CONSTRAINT schedule_event_receipts_source_key UNIQUE (schedule_id, event_kind, event_id),
    CONSTRAINT schedule_event_receipts_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id) ON DELETE RESTRICT,
    CONSTRAINT schedule_event_receipts_episode_id_fkey FOREIGN KEY (episode_id) REFERENCES public.alert_episodes(id) ON DELETE RESTRICT,
    CONSTRAINT schedule_event_receipts_event_seq_fkey FOREIGN KEY (event_seq) REFERENCES public.alert_lifecycle_events(event_seq) ON DELETE RESTRICT,
    CONSTRAINT schedule_event_receipts_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE RESTRICT,
    CONSTRAINT schedule_event_receipts_schedule_id_fkey FOREIGN KEY (schedule_id) REFERENCES public.schedules(id) ON DELETE RESTRICT
);



CREATE TABLE public.schedule_event_dependencies (
    receipt_id uuid NOT NULL,
    prerequisite_job_id uuid NOT NULL,
    CONSTRAINT schedule_event_dependencies_pkey PRIMARY KEY (receipt_id, prerequisite_job_id),
    CONSTRAINT schedule_event_dependencies_prerequisite_job_id_fkey FOREIGN KEY (prerequisite_job_id) REFERENCES public.jobs(id) ON DELETE RESTRICT,
    CONSTRAINT schedule_event_dependencies_receipt_id_fkey FOREIGN KEY (receipt_id) REFERENCES public.schedule_event_receipts(id) ON DELETE CASCADE
);



CREATE TABLE public.webhook_events (
    id uuid NOT NULL,
    kind text NOT NULL,
    event_id text NOT NULL,
    event_predicates text[] DEFAULT ARRAY[]::text[] NOT NULL,
    subject_client_ids text[] DEFAULT ARRAY[]::text[] NOT NULL,
    payload jsonb NOT NULL,
    occurred_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    alert_lifecycle_event_seq bigint,
    causation_id uuid,
    schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    CONSTRAINT webhook_events_event_id_check CHECK (((length(TRIM(BOTH FROM event_id)) >= 1) AND (length(TRIM(BOTH FROM event_id)) <= 256))),
    CONSTRAINT webhook_events_kind_check CHECK (((length(TRIM(BOTH FROM kind)) >= 1) AND (length(TRIM(BOTH FROM kind)) <= 128))),
    CONSTRAINT webhook_events_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT webhook_events_schedule_lineage_check CHECK (public.alert_uuid_array_is_unique_bounded(schedule_lineage, 16)),
    CONSTRAINT webhook_events_pkey PRIMARY KEY (id),
    CONSTRAINT webhook_events_no_full_telemetry_outbox CHECK ((kind <> 'telemetry.rollup'::text)),
    CONSTRAINT webhook_events_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);

CREATE TABLE public.webhook_rule_deliveries (
    id uuid NOT NULL,
    rule_id uuid NOT NULL,
    rule_name text NOT NULL,
    event_kind text NOT NULL,
    event_id text NOT NULL,
    status text NOT NULL,
    target text NOT NULL,
    dedupe_key text NOT NULL,
    payload jsonb NOT NULL,
    matched_vps jsonb NOT NULL,
    message text NOT NULL,
    error text,
    cooldown_until_unix bigint NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    eligibility_revision bigint DEFAULT 0 NOT NULL,
    delivery_lease_id uuid,
    delivery_lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone,
    last_attempt_at timestamp with time zone,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivered_at timestamp with time zone,
    CONSTRAINT webhook_rule_deliveries_cooldown_until_unix_check CHECK ((cooldown_until_unix >= 0)),
    CONSTRAINT webhook_rule_deliveries_eligibility_revision_check CHECK ((eligibility_revision >= 0)),
    CONSTRAINT webhook_rule_deliveries_event_id_check CHECK (((length(TRIM(BOTH FROM event_id)) >= 1) AND (length(TRIM(BOTH FROM event_id)) <= 256))),
    CONSTRAINT webhook_rule_deliveries_event_kind_check CHECK (((length(TRIM(BOTH FROM event_kind)) >= 1) AND (length(TRIM(BOTH FROM event_kind)) <= 128))),
    CONSTRAINT webhook_rule_deliveries_matched_vps_check CHECK ((jsonb_typeof(matched_vps) = 'array'::text)),
    CONSTRAINT webhook_rule_deliveries_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT webhook_rule_deliveries_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'in_progress'::text, 'failed'::text, 'permanently_failed'::text, 'canceled_disabled'::text, 'delivered'::text, 'matched_dry_run'::text]))),
    CONSTRAINT webhook_rule_deliveries_target_check CHECK (((length(TRIM(BOTH FROM target)) >= 1) AND (length(TRIM(BOTH FROM target)) <= 512))),
    CONSTRAINT webhook_rule_deliveries_pkey PRIMARY KEY (id),
    CONSTRAINT webhook_rule_deliveries_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.webhook_rules (
    id uuid NOT NULL,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    expression text NOT NULL,
    target text NOT NULL,
    body_template text DEFAULT ''::text NOT NULL,
    signing_secret text,
    cooldown_secs bigint DEFAULT 300 NOT NULL,
    notes text,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT webhook_rules_body_template_check CHECK ((length(body_template) <= 4096)),
    CONSTRAINT webhook_rules_cooldown_secs_check CHECK (((cooldown_secs >= 0) AND (cooldown_secs <= 2592000))),
    CONSTRAINT webhook_rules_expression_check CHECK (((length(TRIM(BOTH FROM expression)) >= 1) AND (length(TRIM(BOTH FROM expression)) <= 4096))),
    CONSTRAINT webhook_rules_name_check CHECK (((length(TRIM(BOTH FROM name)) >= 1) AND (length(TRIM(BOTH FROM name)) <= 128))),
    CONSTRAINT webhook_rules_notes_check CHECK (((notes IS NULL) OR (length(notes) <= 1024))),
    CONSTRAINT webhook_rules_signing_secret_check CHECK (((signing_secret IS NULL) OR (length(signing_secret) <= 1024))),
    CONSTRAINT webhook_rules_target_check CHECK (((length(TRIM(BOTH FROM target)) >= 1) AND (length(TRIM(BOTH FROM target)) <= 512))),
    CONSTRAINT webhook_rules_name_key UNIQUE (name),
    CONSTRAINT webhook_rules_pkey PRIMARY KEY (id),
    CONSTRAINT webhook_rules_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



-- Indexes.

CREATE UNIQUE INDEX alert_episodes_identity_idx ON public.alert_episodes USING btree (policy_rule_id, policy_rule_version, natural_key, trigger_generation);



CREATE INDEX alert_episodes_last_evidence_idx ON public.alert_episodes USING btree (last_evidence_id) WHERE (last_evidence_id IS NOT NULL);



CREATE UNIQUE INDEX alert_episodes_one_current_idx ON public.alert_episodes USING btree (policy_rule_id, natural_key) WHERE (resolved_at IS NULL);



CREATE INDEX alert_episodes_trigger_evidence_idx ON public.alert_episodes USING btree (trigger_evidence_id) WHERE (trigger_evidence_id IS NOT NULL);



CREATE INDEX alert_lifecycle_events_episode_idx ON public.alert_lifecycle_events USING btree (episode_id, trigger_generation, event_seq);



CREATE INDEX alert_lifecycle_consumer_receipts_claim_idx ON public.alert_lifecycle_consumer_receipts USING btree (consumer_kind, status, event_seq);



CREATE INDEX alert_policy_confirmations_evidence_idx ON public.alert_policy_confirmations USING btree (evidence_id);



CREATE INDEX alert_policy_confirmations_window_idx ON public.alert_policy_confirmations USING btree (policy_rule_id, rule_version, confirmation_bucket_key, phase, accepted_at DESC, evidence_id);



CREATE INDEX alert_policy_effective_current_evidence_source_idx ON public.alert_policy_effective_current_evidence USING btree (source_kind, natural_key, subject_client_id) INCLUDE (evidence_id, evidence_seq);



CREATE INDEX alert_policy_evaluation_states_due_idx ON public.alert_policy_evaluation_states USING btree (next_transition_at, policy_rule_id, confirmation_bucket_key) WHERE (next_transition_at IS NOT NULL);



CREATE INDEX alert_policy_evaluation_states_active_episode_idx ON public.alert_policy_evaluation_states USING btree (active_episode_id) WHERE (active_episode_id IS NOT NULL);



CREATE INDEX alert_policy_evaluation_states_last_evidence_id_idx ON public.alert_policy_evaluation_states USING btree (last_evidence_id) WHERE (last_evidence_id IS NOT NULL);



CREATE INDEX alert_policy_evaluation_states_last_evidence_seq_idx ON public.alert_policy_evaluation_states USING btree (last_evidence_seq) WHERE (last_evidence_seq IS NOT NULL);



CREATE INDEX alert_policy_evidence_prune_candidates_retry_idx ON public.alert_policy_evidence_prune_candidates USING btree (last_attempted_at NULLS FIRST, enqueued_at, evidence_id);



CREATE INDEX alert_policy_evidence_pending_idx ON public.alert_policy_evidence USING btree (evidence_seq) INCLUDE (id, source_kind, natural_key, subject_client_id) WHERE evaluation_pending;



CREATE INDEX alert_policy_evidence_targets_evidence_idx ON public.alert_policy_evidence_targets USING btree (evidence_seq, policy_rule_id, rule_version);



CREATE INDEX alert_policy_evidence_receipts_evidence_id_idx ON public.alert_policy_evidence_receipts USING btree (evidence_id);



CREATE INDEX alert_policy_evidence_receipts_evidence_idx ON public.alert_policy_evidence_receipts USING btree (evidence_seq, policy_rule_id);



CREATE INDEX alert_policy_evidence_receipts_retention_idx ON public.alert_policy_evidence_receipts USING btree (evaluated_at, evidence_seq);



CREATE INDEX alert_policy_evidence_retention_candidates_idx ON public.alert_policy_evidence USING btree (created_at, evidence_seq);



CREATE INDEX alert_policy_evidence_source_latest_idx ON public.alert_policy_evidence USING btree (source_kind, natural_key, observed_at DESC, evidence_seq DESC);



CREATE INDEX alert_policy_evidence_subject_latest_idx ON public.alert_policy_evidence USING btree (subject_client_id, observed_at DESC, id DESC) WHERE (subject_client_id IS NOT NULL);



CREATE INDEX alert_policy_scope_dirty_clients_age_idx ON public.alert_policy_scope_dirty_clients USING btree (dirty_at, client_id);



CREATE INDEX fleet_alert_notification_channels_match_idx ON public.fleet_alert_notification_channels USING btree (enabled, scope_kind, scope_value, min_severity, delivery_kind, updated_at DESC);



CREATE INDEX fleet_alert_notification_deliveries_alert_idx ON public.fleet_alert_notification_deliveries USING btree (alert_id, created_at DESC);



CREATE INDEX fleet_alert_notification_deliveries_attempt_idx ON public.fleet_alert_notification_deliveries USING btree (status, next_attempt_at, created_at);



CREATE INDEX fleet_alert_notification_deliveries_channel_created_idx ON public.fleet_alert_notification_deliveries USING btree (channel_id, created_at DESC, alert_id);



CREATE INDEX fleet_alert_notification_deliveries_created_idx ON public.fleet_alert_notification_deliveries USING btree (created_at DESC, alert_id);



CREATE INDEX fleet_alert_notification_deliveries_dedupe_idx ON public.fleet_alert_notification_deliveries USING btree (dedupe_key, cooldown_until_unix DESC);



CREATE INDEX fleet_alert_notification_deliveries_lease_idx ON public.fleet_alert_notification_deliveries USING btree (status, delivery_lease_until, next_attempt_at, created_at);



CREATE INDEX fleet_alert_notification_deliveries_status_idx ON public.fleet_alert_notification_deliveries USING btree (status, created_at DESC);



CREATE INDEX fleet_alert_states_state_idx ON public.fleet_alert_states USING btree (state, updated_at DESC);



CREATE INDEX fleet_alert_states_updated_idx ON public.fleet_alert_states USING btree (updated_at DESC, alert_id);



CREATE INDEX alert_episodes_current_client_priority_idx ON public.alert_episodes USING btree (client_id, (
CASE record_kind
    WHEN 'condition'::text THEN 0
    ELSE 1
END), (
CASE
    WHEN (lifecycle_state = ANY (ARRAY['triggered'::text, 'persisting'::text])) THEN 0
    ELSE 1
END), (
CASE severity
    WHEN 'critical'::text THEN 0
    WHEN 'warning'::text THEN 1
    WHEN 'info'::text THEN 2
    ELSE 3
END), triggered_at DESC, id DESC) WHERE (resolved_at IS NULL);



CREATE INDEX alert_episodes_current_priority_idx ON public.alert_episodes USING btree ((
CASE record_kind
    WHEN 'condition'::text THEN 0
    ELSE 1
END), (
CASE
    WHEN (lifecycle_state = ANY (ARRAY['triggered'::text, 'persisting'::text])) THEN 0
    ELSE 1
END), (
CASE severity
    WHEN 'critical'::text THEN 0
    WHEN 'warning'::text THEN 1
    WHEN 'info'::text THEN 2
    ELSE 3
END), triggered_at DESC, id DESC) WHERE (resolved_at IS NULL);



CREATE INDEX alert_episodes_history_client_idx ON public.alert_episodes USING btree (client_id, triggered_at DESC, id DESC);



CREATE INDEX alert_episodes_history_idx ON public.alert_episodes USING btree (triggered_at DESC, id DESC);



CREATE INDEX alert_episodes_resolved_retention_idx ON public.alert_episodes USING btree (resolved_at DESC, id DESC) WHERE (lifecycle_state = 'resolved'::text);



CREATE INDEX alert_episodes_unresolved_event_client_cursor_idx ON public.alert_episodes USING btree (client_id, triggered_at DESC, id DESC) WHERE ((record_kind = 'event'::text) AND (resolved_at IS NULL));



CREATE INDEX alert_episodes_unresolved_event_cursor_idx ON public.alert_episodes USING btree (triggered_at DESC, id DESC) WHERE ((record_kind = 'event'::text) AND (resolved_at IS NULL));



CREATE INDEX policy_groups_enabled_idx ON public.policy_groups USING btree (enabled, updated_at DESC, name);



CREATE INDEX policy_groups_enabled_last_seen_scope_idx ON public.policy_groups USING btree (id) WHERE (enabled AND (lower(selector_expression) ~~ '%last_seen%'::text));



CREATE INDEX policy_rules_group_idx ON public.policy_rules USING btree (group_id, sort_order, created_at);



CREATE UNIQUE INDEX policy_rules_system_seed_key_idx ON public.policy_rules USING btree (system_seed_key) WHERE (system_seed_key IS NOT NULL);



CREATE INDEX schedule_event_receipts_episode_idx ON public.schedule_event_receipts USING btree (episode_id, trigger_generation, edge_ordinal, status);



CREATE INDEX schedule_event_receipts_event_idx ON public.schedule_event_receipts USING btree (event_seq, id);



CREATE INDEX schedule_event_receipts_pending_idx ON public.schedule_event_receipts USING btree (event_seq, schedule_id) WHERE (status = 'pending'::text);



CREATE INDEX webhook_events_lifecycle_seq_idx ON public.webhook_events USING btree (alert_lifecycle_event_seq) WHERE (alert_lifecycle_event_seq IS NOT NULL);



CREATE INDEX webhook_events_kind_idx ON public.webhook_events USING btree (kind, event_id, occurred_at DESC);



CREATE INDEX webhook_events_unprocessed_idx ON public.webhook_events USING btree (occurred_at, id) WHERE (processed_at IS NULL);



CREATE INDEX webhook_events_processed_retention_idx ON public.webhook_events USING btree (occurred_at, id) WHERE (processed_at IS NOT NULL);



CREATE INDEX webhook_rule_deliveries_attempt_idx ON public.webhook_rule_deliveries USING btree (status, next_attempt_at, created_at) WHERE (status = ANY (ARRAY['queued'::text, 'failed'::text]));



CREATE INDEX webhook_rule_deliveries_created_idx ON public.webhook_rule_deliveries USING btree (created_at DESC, id DESC);



CREATE INDEX webhook_rule_deliveries_rule_cooldown_idx ON public.webhook_rule_deliveries USING btree (rule_id, cooldown_until_unix DESC);



CREATE INDEX webhook_rule_deliveries_event_idx ON public.webhook_rule_deliveries USING btree (event_kind, event_id, created_at DESC);



CREATE INDEX webhook_rule_deliveries_event_kind_created_idx ON public.webhook_rule_deliveries USING btree (event_kind, created_at DESC, id DESC);



CREATE INDEX webhook_rule_deliveries_lease_idx ON public.webhook_rule_deliveries USING btree (status, delivery_lease_until, next_attempt_at, created_at) WHERE (status = 'in_progress'::text);



CREATE UNIQUE INDEX webhook_rule_deliveries_rule_event_unique_idx ON public.webhook_rule_deliveries USING btree (rule_id, event_id);



CREATE INDEX webhook_rule_deliveries_rule_idx ON public.webhook_rule_deliveries USING btree (rule_id, created_at DESC);



CREATE INDEX webhook_rule_deliveries_status_idx ON public.webhook_rule_deliveries USING btree (status, created_at DESC);



CREATE INDEX webhook_rules_enabled_idx ON public.webhook_rules USING btree (enabled, updated_at DESC, name);



-- Triggers.

CREATE TRIGGER alert_policy_actual_current_displaced AFTER UPDATE OF evidence_id ON public.alert_policy_current_evidence FOR EACH ROW WHEN ((old.evidence_id IS DISTINCT FROM new.evidence_id)) EXECUTE FUNCTION public.enqueue_displaced_alert_policy_evidence();



CREATE TRIGGER alert_policy_current_evidence_after_insert AFTER INSERT ON public.alert_policy_evidence FOR EACH ROW EXECUTE FUNCTION public.advance_alert_policy_current_evidence();



CREATE TRIGGER alert_policy_effective_current_displaced AFTER UPDATE OF evidence_id ON public.alert_policy_effective_current_evidence FOR EACH ROW WHEN ((old.evidence_id IS DISTINCT FROM new.evidence_id)) EXECUTE FUNCTION public.enqueue_displaced_alert_policy_evidence();



CREATE TRIGGER backup_requests_terminal_at_trigger BEFORE INSERT OR UPDATE OF status ON public.backup_requests FOR EACH ROW EXECUTE FUNCTION public.set_backup_request_terminal_at();



CREATE TRIGGER client_tags_policy_scope_revision_trigger AFTER INSERT OR DELETE OR UPDATE ON public.client_tags FOR EACH ROW EXECUTE FUNCTION public.bump_policy_scope_revision_for_assignment();



CREATE TRIGGER clients_alert_policy_scope_dirty AFTER UPDATE ON public.clients FOR EACH ROW WHEN ((old.policy_scope_revision IS DISTINCT FROM new.policy_scope_revision)) EXECUTE FUNCTION public.queue_alert_policy_scope_revision();



CREATE TRIGGER clients_operational_alert_boundaries_insert_trigger BEFORE INSERT ON public.clients FOR EACH ROW EXECUTE FUNCTION public.stamp_client_operational_alert_boundaries();



CREATE TRIGGER clients_operational_alert_boundaries_update_trigger BEFORE UPDATE OF status, process_incarnation_id ON public.clients FOR EACH ROW EXECUTE FUNCTION public.stamp_client_operational_alert_boundaries();



CREATE TRIGGER clients_policy_scope_revision_update_trigger BEFORE UPDATE OF display_name, status, registration_ip, last_ip, last_seen_at, internal_build_number, stale_since, stale_reason, hidden_at ON public.clients FOR EACH ROW EXECUTE FUNCTION public.stamp_client_policy_scope_revision();



CREATE TRIGGER gateway_sessions_operational_alert_boundary_trigger AFTER INSERT OR UPDATE OF status, client_id ON public.gateway_sessions FOR EACH ROW EXECUTE FUNCTION public.stamp_gateway_session_operational_alert_boundary();



CREATE TRIGGER job_targets_capability_alert_at_trigger BEFORE INSERT OR UPDATE OF status, capability_degraded_reason, capability_degraded_hint ON public.job_targets FOR EACH ROW EXECUTE FUNCTION public.set_job_target_capability_alert_at();



CREATE TRIGGER jobs_alert_terminal_at_trigger BEFORE INSERT OR UPDATE OF status, completed_at ON public.jobs FOR EACH ROW EXECUTE FUNCTION public.set_job_alert_terminal_at();



CREATE TRIGGER policy_groups_last_seen_scope_rebase AFTER INSERT OR UPDATE ON public.policy_groups FOR EACH ROW EXECUTE FUNCTION public.queue_last_seen_policy_scope_rebase();



CREATE TRIGGER tags_policy_scope_revision_update_trigger AFTER UPDATE OF name ON public.tags FOR EACH ROW EXECUTE FUNCTION public.bump_policy_scope_revision_for_tag_name();



CREATE TRIGGER vps_rule_values_policy_scope_revision_trigger AFTER INSERT OR DELETE OR UPDATE ON public.vps_rule_values FOR EACH ROW EXECUTE FUNCTION public.bump_policy_scope_revision_for_assignment();



CREATE TRIGGER zz_alert_policy_noncurrent_evidence_after_insert AFTER INSERT ON public.alert_policy_evidence FOR EACH ROW EXECUTE FUNCTION public.enqueue_noncurrent_alert_policy_evidence();
