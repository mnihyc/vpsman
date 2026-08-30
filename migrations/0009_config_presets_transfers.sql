-- Configuration presets, runtime patches, and file transfers.

-- Runtime configuration has two independently monotonic authorities. Desired
-- revisions fence composition work; apply versions are allocated only when a
-- consumer owns that exact desired revision.
CREATE SEQUENCE public.runtime_config_desired_revision_seq AS bigint;
CREATE SEQUENCE public.runtime_config_apply_version_seq AS bigint;

-- Tables.

CREATE TABLE public.client_runtime_config_apply_state (
    client_id text NOT NULL,
    applied_version bigint,
    applied_content_hash text,
    applied_config jsonb,
    applied_job_id uuid,
    applied_at timestamp with time zone,
    pending_version bigint,
    pending_content_hash text,
    pending_config jsonb,
    pending_job_id uuid,
    pending_reason text,
    pending_status text,
    pending_error text,
    pending_updated_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_runtime_config_apply_state_applied_config_check CHECK (((applied_config IS NULL) OR (jsonb_typeof(applied_config) = 'object'::text))),
    CONSTRAINT client_runtime_config_apply_state_error_check CHECK (((pending_error IS NULL) OR (octet_length(pending_error) <= 4096))),
    CONSTRAINT client_runtime_config_apply_state_hash_check CHECK ((((applied_content_hash IS NULL) OR (octet_length(applied_content_hash) <= 128)) AND ((pending_content_hash IS NULL) OR (octet_length(pending_content_hash) <= 128)))),
    CONSTRAINT client_runtime_config_apply_state_pending_config_check CHECK (((pending_config IS NULL) OR (jsonb_typeof(pending_config) = 'object'::text))),
    CONSTRAINT client_runtime_config_apply_state_pending_status_check CHECK (((pending_status IS NULL) OR (pending_status = ANY (ARRAY['queued'::text, 'failed'::text])))),
    CONSTRAINT client_runtime_config_apply_state_reason_check CHECK (((pending_reason IS NULL) OR (octet_length(pending_reason) <= 4096))),
    CONSTRAINT client_runtime_config_apply_state_pkey PRIMARY KEY (client_id),
    CONSTRAINT client_runtime_config_apply_state_applied_job_id_fkey FOREIGN KEY (applied_job_id) REFERENCES public.jobs(id) ON DELETE SET NULL,
    CONSTRAINT client_runtime_config_apply_state_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT client_runtime_config_apply_state_pending_job_id_fkey FOREIGN KEY (pending_job_id) REFERENCES public.jobs(id) ON DELETE SET NULL
);



CREATE TABLE public.client_runtime_config_owners (
    client_id text NOT NULL,
    source_revision bigint DEFAULT 0 NOT NULL,
    reconciled_revision bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_runtime_config_owners_pkey PRIMARY KEY (client_id),
    CONSTRAINT client_runtime_config_owners_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT client_runtime_config_owners_revision_check CHECK (
        source_revision >= 0
        AND reconciled_revision >= 0
        AND reconciled_revision <= source_revision
    )
);



CREATE TABLE public.client_runtime_config_reconcile_work (
    client_id text NOT NULL,
    desired_revision bigint NOT NULL,
    reason text NOT NULL,
    requested_by uuid,
    claim_token uuid,
    claim_revision bigint,
    apply_version bigint,
    lease_until timestamp with time zone,
    attempt_count integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_runtime_config_reconcile_work_pkey PRIMARY KEY (client_id),
    CONSTRAINT client_runtime_config_reconcile_work_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.client_runtime_config_owners(client_id) ON DELETE CASCADE,
    CONSTRAINT client_runtime_config_reconcile_work_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT client_runtime_config_reconcile_work_revision_check CHECK (desired_revision > 0),
    CONSTRAINT client_runtime_config_reconcile_work_attempt_check CHECK (attempt_count >= 0),
    CONSTRAINT client_runtime_config_reconcile_work_reason_check CHECK ((octet_length(reason) > 0) AND (octet_length(reason) <= 4096)),
    CONSTRAINT client_runtime_config_reconcile_work_error_check CHECK ((last_error IS NULL) OR (octet_length(last_error) <= 4096)),
    CONSTRAINT client_runtime_config_reconcile_work_claim_check CHECK (
        (claim_token IS NULL AND claim_revision IS NULL AND apply_version IS NULL AND lease_until IS NULL)
        OR
        (claim_token IS NOT NULL AND claim_revision = desired_revision AND apply_version > 0 AND lease_until IS NOT NULL)
    )
);



CREATE TABLE public.client_runtime_config_overrides (
    client_id text NOT NULL,
    toml text NOT NULL,
    reason text DEFAULT ''::text NOT NULL,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_runtime_config_overrides_reason_check CHECK ((octet_length(reason) <= 4096)),
    CONSTRAINT client_runtime_config_overrides_toml_check CHECK (((octet_length(toml) > 0) AND (octet_length(toml) <= 4194304))),
    CONSTRAINT client_runtime_config_overrides_pkey PRIMARY KEY (client_id),
    CONSTRAINT client_runtime_config_overrides_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT client_runtime_config_overrides_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.configuration_presets (
    id uuid NOT NULL,
    behavior text NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    is_default boolean DEFAULT false NOT NULL,
    description text,
    definition jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT configuration_presets_behavior_check CHECK ((behavior = ANY (ARRAY['host_metrics'::text, 'latency_probe'::text, 'ospf_update_command'::text, 'process_inventory'::text, 'user_sessions'::text, 'command_execution'::text]))),
    CONSTRAINT configuration_presets_default_kind_check CHECK (((NOT is_default) OR (kind = 'system'::text))),
    CONSTRAINT configuration_presets_definition_object_check CHECK ((jsonb_typeof(definition) = 'object'::text)),
    CONSTRAINT configuration_presets_description_check CHECK (((description IS NULL) OR (octet_length(description) <= 4096))),
    CONSTRAINT configuration_presets_kind_check CHECK ((kind = ANY (ARRAY['system'::text, 'custom'::text]))),
    CONSTRAINT configuration_presets_name_check CHECK (((length(name) > 0) AND (name = btrim(name)) AND (octet_length(name) <= 256) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT configuration_presets_id_behavior_key UNIQUE (id, behavior),
    CONSTRAINT configuration_presets_pkey PRIMARY KEY (id)
);



CREATE TABLE public.client_configuration_preset_overrides (
    client_id text NOT NULL,
    behavior text NOT NULL,
    preset_id uuid NOT NULL,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_configuration_preset_overrides_pkey PRIMARY KEY (client_id, behavior),
    CONSTRAINT client_configuration_preset_overrides_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT client_configuration_preset_overrides_preset_id_behavior_fkey FOREIGN KEY (preset_id, behavior) REFERENCES public.configuration_presets(id, behavior) ON DELETE RESTRICT,
    CONSTRAINT client_configuration_preset_overrides_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.file_transfer_sessions (
    session_id uuid NOT NULL,
    client_id text NOT NULL,
    direction text NOT NULL,
    status text NOT NULL,
    path text NOT NULL,
    size_bytes bigint,
    progress_bytes bigint DEFAULT 0 NOT NULL,
    progress_ratio double precision,
    sha256_hex text,
    chunk_size_bytes bigint,
    last_chunk_size_bytes bigint,
    last_chunk_sha256_hex text,
    rate_limit_kbps bigint,
    resumed boolean,
    last_event text NOT NULL,
    last_job_id uuid NOT NULL,
    last_command_type text NOT NULL,
    last_seq integer NOT NULL,
    observed_at timestamp with time zone DEFAULT now() NOT NULL,
    handoff_available boolean DEFAULT false NOT NULL,
    handoff_object_key text,
    handoff_download_path text,
    CONSTRAINT file_transfer_sessions_direction_check CHECK ((direction = ANY (ARRAY['upload'::text, 'download'::text]))),
    CONSTRAINT file_transfer_sessions_last_command_type_check CHECK ((last_command_type = ANY (ARRAY['file_transfer_start'::text, 'file_transfer_chunk'::text, 'file_transfer_commit'::text, 'file_transfer_abort'::text, 'file_transfer_download_start'::text, 'file_transfer_download_chunk'::text]))),
    CONSTRAINT file_transfer_sessions_last_event_check CHECK ((last_event = ANY (ARRAY['file_transfer_start'::text, 'file_transfer_chunk_ack'::text, 'file_transfer_commit'::text, 'file_transfer_abort'::text, 'file_transfer_download_start'::text, 'file_transfer_download_chunk'::text]))),
    CONSTRAINT file_transfer_sessions_status_check CHECK ((status = ANY (ARRAY['started'::text, 'transferring'::text, 'completed'::text, 'aborted'::text, 'unknown'::text]))),
    CONSTRAINT file_transfer_sessions_pkey PRIMARY KEY (client_id, session_id),
    CONSTRAINT file_transfer_sessions_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT file_transfer_sessions_last_job_id_fkey FOREIGN KEY (last_job_id) REFERENCES public.jobs(id) ON DELETE CASCADE
);



CREATE TABLE public.file_transfer_source_artifacts (
    id uuid NOT NULL,
    name text NOT NULL,
    object_key text NOT NULL,
    sha256_hex text NOT NULL,
    size_bytes bigint NOT NULL,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT file_transfer_source_artifacts_sha256_hex_check CHECK ((sha256_hex ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT file_transfer_source_artifacts_size_check CHECK ((size_bytes >= 0)),
    CONSTRAINT file_transfer_source_artifacts_pkey PRIMARY KEY (id),
    CONSTRAINT file_transfer_source_artifacts_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.operators(id)
);



CREATE TABLE public.runtime_config_patch_generators (
    id uuid NOT NULL,
    name text NOT NULL,
    category text NOT NULL,
    domain text NOT NULL,
    description text NOT NULL,
    field_schema jsonb DEFAULT '{}'::jsonb NOT NULL,
    raw_generator_body text NOT NULL,
    docs_metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    built_in boolean DEFAULT false NOT NULL,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT runtime_config_patch_generators_body_check CHECK (((length(TRIM(BOTH FROM raw_generator_body)) > 0) AND (octet_length(raw_generator_body) <= 16384))),
    CONSTRAINT runtime_config_patch_generators_category_check CHECK (((length(TRIM(BOTH FROM category)) > 0) AND (octet_length(category) <= 4096))),
    CONSTRAINT runtime_config_patch_generators_description_check CHECK (((length(TRIM(BOTH FROM description)) > 0) AND (octet_length(description) <= 4096))),
    CONSTRAINT runtime_config_patch_generators_docs_object CHECK ((jsonb_typeof(docs_metadata) = 'object'::text)),
    CONSTRAINT runtime_config_patch_generators_domain_check CHECK (((length(TRIM(BOTH FROM domain)) > 0) AND (octet_length(domain) <= 4096))),
    CONSTRAINT runtime_config_patch_generators_name_check CHECK (((length(TRIM(BOTH FROM name)) > 0) AND (octet_length(name) <= 4096))),
    CONSTRAINT runtime_config_patch_generators_schema_object CHECK ((jsonb_typeof(field_schema) = 'object'::text)),
    CONSTRAINT runtime_config_patch_generators_pkey PRIMARY KEY (id),
    CONSTRAINT runtime_config_patch_generators_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id) ON DELETE SET NULL
);



-- Indexes.

CREATE INDEX client_configuration_preset_overrides_preset_idx ON public.client_configuration_preset_overrides USING btree (preset_id);



CREATE INDEX client_runtime_config_apply_state_pending_job_idx ON public.client_runtime_config_apply_state USING btree (pending_job_id) WHERE (pending_job_id IS NOT NULL);



CREATE INDEX client_runtime_config_reconcile_work_due_idx ON public.client_runtime_config_reconcile_work USING btree (next_attempt_at, lease_until, updated_at, client_id);



CREATE UNIQUE INDEX configuration_presets_default_idx ON public.configuration_presets USING btree (behavior) WHERE is_default;



CREATE UNIQUE INDEX configuration_presets_name_idx ON public.configuration_presets USING btree (behavior, lower(name));



CREATE INDEX file_transfer_sessions_observed_idx ON public.file_transfer_sessions USING btree (observed_at DESC, client_id, session_id);



CREATE INDEX file_transfer_source_artifacts_created_idx ON public.file_transfer_source_artifacts USING btree (created_at DESC, id DESC);



CREATE INDEX file_transfer_source_artifacts_hash_idx ON public.file_transfer_source_artifacts USING btree (sha256_hex, size_bytes);



CREATE UNIQUE INDEX file_transfer_source_artifacts_object_key_unique ON public.file_transfer_source_artifacts USING btree (object_key);



-- Durable runtime-configuration producers. Every source trigger supplies only
-- the clients whose composed document can change. The upsert supersedes an
-- in-flight claim instead of waiting for it; the old consumer consequently
-- fails its revision fence before it can create a job.

CREATE FUNCTION public.enqueue_runtime_config_reconcile(
    p_client_ids text[],
    p_reason text,
    p_requested_by uuid DEFAULT NULL
) RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
    WITH requested AS (
        SELECT client.id
        FROM public.visible_clients client
        JOIN (
            SELECT DISTINCT btrim(candidate) AS client_id
            FROM unnest(COALESCE(p_client_ids, ARRAY[]::text[])) candidate
            WHERE btrim(candidate) <> ''
        ) selected ON selected.client_id = client.id
    ), owned AS (
        INSERT INTO public.client_runtime_config_owners (
            client_id, source_revision, updated_at
        )
        SELECT
            requested.id,
            nextval('public.runtime_config_desired_revision_seq'),
            now()
        FROM requested
        ORDER BY requested.id COLLATE "C"
        ON CONFLICT (client_id) DO UPDATE SET
            source_revision = EXCLUDED.source_revision,
            updated_at = now()
        RETURNING client_id, source_revision
    )
    INSERT INTO public.client_runtime_config_reconcile_work (
        client_id,
        desired_revision,
        reason,
        requested_by,
        claim_token,
        claim_revision,
        apply_version,
        lease_until,
        attempt_count,
        next_attempt_at,
        last_error,
        updated_at
    )
    SELECT
        owner.client_id,
        owner.source_revision,
        left(COALESCE(NULLIF(btrim(p_reason), ''), 'runtime_config_source_changed'), 4096),
        p_requested_by,
        NULL,
        NULL,
        NULL,
        NULL,
        0,
        now(),
        NULL,
        now()
    FROM owned owner
    ORDER BY owner.client_id COLLATE "C"
    ON CONFLICT (client_id) DO UPDATE SET
        desired_revision = EXCLUDED.desired_revision,
        reason = EXCLUDED.reason,
        requested_by = EXCLUDED.requested_by,
        claim_token = NULL,
        claim_revision = NULL,
        apply_version = NULL,
        lease_until = NULL,
        attempt_count = 0,
        next_attempt_at = now(),
        last_error = NULL,
        updated_at = now();
    IF FOUND THEN
        PERFORM pg_notify('runtime_config_reconcile', 'ready');
    END IF;
END;
$$;



CREATE FUNCTION public.produce_runtime_config_override_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    source_client_id text;
    source_reason text;
    source_actor uuid;
BEGIN
    IF TG_OP = 'DELETE' THEN
        source_client_id := OLD.client_id;
        source_reason := OLD.reason;
        source_actor := OLD.updated_by;
    ELSE
        source_client_id := NEW.client_id;
        source_reason := NEW.reason;
        source_actor := NEW.updated_by;
    END IF;
    PERFORM public.enqueue_runtime_config_reconcile(
        ARRAY[source_client_id],
        COALESCE(NULLIF(source_reason, ''), 'operator_runtime_config_override'),
        source_actor
    );
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_configuration_source_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    source_client_id text;
    source_actor uuid;
BEGIN
    IF TG_OP = 'DELETE' THEN
        source_client_id := OLD.client_id;
        source_actor := OLD.updated_by;
    ELSE
        source_client_id := NEW.client_id;
        source_actor := NEW.updated_by;
    END IF;
    PERFORM public.enqueue_runtime_config_reconcile(
        ARRAY[source_client_id],
        'configuration_source_override_applied',
        source_actor
    );
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_configuration_preset_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    affected text[];
    old_id uuid;
    new_id uuid;
    old_behavior text;
    new_behavior text;
    old_default boolean := FALSE;
    new_default boolean := FALSE;
BEGIN
    IF TG_OP <> 'INSERT' THEN
        old_id := OLD.id;
        old_behavior := OLD.behavior;
        old_default := OLD.is_default;
    END IF;
    IF TG_OP <> 'DELETE' THEN
        new_id := NEW.id;
        new_behavior := NEW.behavior;
        new_default := NEW.is_default;
    END IF;
    SELECT array_agg(DISTINCT source.client_id ORDER BY source.client_id)
    INTO affected
    FROM (
        SELECT selected.client_id
        FROM public.client_configuration_preset_overrides selected
        WHERE selected.preset_id IN (old_id, new_id)
        UNION
        SELECT client.id
        FROM public.visible_clients client
        WHERE (old_default OR new_default)
          AND NOT EXISTS (
              SELECT 1
              FROM public.client_configuration_preset_overrides selected
              WHERE selected.client_id = client.id
                AND selected.behavior IN (old_behavior, new_behavior)
          )
    ) source;
    PERFORM public.enqueue_runtime_config_reconcile(
        affected,
        'configuration_preset_updated',
        NULL
    );
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_ping_assignment_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    old_client_id text;
    new_client_id text;
BEGIN
    IF TG_OP <> 'INSERT' THEN
        old_client_id := OLD.client_id;
    END IF;
    IF TG_OP <> 'DELETE' THEN
        new_client_id := NEW.client_id;
    END IF;
    PERFORM public.enqueue_runtime_config_reconcile(
        ARRAY[old_client_id, new_client_id],
        'ping_targets_updated',
        NULL
    );
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_ping_target_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    affected text[];
    affected_target_id uuid;
    source_actor uuid;
BEGIN
    IF TG_OP = 'DELETE' THEN
        affected_target_id := OLD.id;
        source_actor := OLD.updated_by;
    ELSE
        affected_target_id := NEW.id;
        source_actor := NEW.updated_by;
    END IF;
    SELECT array_agg(DISTINCT assignment.client_id ORDER BY assignment.client_id)
    INTO affected
    FROM public.ping_target_assignments assignment
    WHERE assignment.target_id = affected_target_id;
    PERFORM public.enqueue_runtime_config_reconcile(
        affected,
        'ping_targets_updated',
        source_actor
    );
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_tunnel_plan_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    affected text[] := ARRAY[]::text[];
    source_actor uuid;
BEGIN
    IF TG_OP <> 'INSERT' AND OLD.enabled AND OLD.deleted_at IS NULL THEN
        affected := affected || ARRAY[OLD.left_client_id, OLD.right_client_id];
        source_actor := OLD.actor_id;
    END IF;
    IF TG_OP <> 'DELETE' AND NEW.enabled AND NEW.deleted_at IS NULL THEN
        affected := affected || ARRAY[NEW.left_client_id, NEW.right_client_id];
        source_actor := COALESCE(NEW.actor_id, source_actor);
    END IF;
    IF cardinality(affected) > 0 THEN
        PERFORM public.enqueue_runtime_config_reconcile(
            affected,
            'tunnel_plan_updated',
            source_actor
        );
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_network_adapter_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    affected text[];
BEGIN
    SELECT array_agg(DISTINCT endpoint.client_id ORDER BY endpoint.client_id)
    INTO affected
    FROM (
        SELECT plan.left_client_id AS client_id
        FROM public.tunnel_plans plan
        WHERE plan.enabled
          AND plan.deleted_at IS NULL
          AND plan.plan #>> '{runtime_control,left_adapter_definition_id}' IN (OLD.id::text, NEW.id::text)
        UNION
        SELECT plan.right_client_id AS client_id
        FROM public.tunnel_plans plan
        WHERE plan.enabled
          AND plan.deleted_at IS NULL
          AND plan.plan #>> '{runtime_control,right_adapter_definition_id}' IN (OLD.id::text, NEW.id::text)
    ) endpoint;
    PERFORM public.enqueue_runtime_config_reconcile(
        affected,
        'network_adapter_definition_updated',
        NULL
    );
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.produce_port_forward_reconcile() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    affected text[] := ARRAY[]::text[];
    source_actor uuid;
BEGIN
    IF TG_OP <> 'INSERT' AND OLD.enabled AND OLD.deleted_at IS NULL THEN
        affected := affected || ARRAY[OLD.client_id];
        source_actor := OLD.actor_id;
    END IF;
    IF TG_OP <> 'DELETE' AND NEW.enabled AND NEW.deleted_at IS NULL THEN
        affected := affected || ARRAY[NEW.client_id];
        source_actor := COALESCE(NEW.actor_id, source_actor);
    END IF;
    IF cardinality(affected) > 0 THEN
        PERFORM public.enqueue_runtime_config_reconcile(
            affected,
            'port_forward_rule_updated',
            source_actor
        );
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;



CREATE FUNCTION public.maintain_runtime_config_work_for_client_lifecycle() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF NEW.hidden_at IS NOT NULL OR NEW.status = 'deleted' THEN
        DELETE FROM public.client_runtime_config_owners
        WHERE client_id = NEW.id;
    ELSIF NEW.process_incarnation_id IS NOT NULL
       AND NEW.status NOT IN ('never', 'suspended', 'revoked', 'deleted')
       AND (
           OLD.process_incarnation_id IS DISTINCT FROM NEW.process_incarnation_id
           OR OLD.status IN ('never', 'suspended', 'revoked')
       ) THEN
        UPDATE public.client_runtime_config_reconcile_work work
        SET next_attempt_at = LEAST(work.next_attempt_at, now()),
            updated_at = now()
        WHERE work.client_id = NEW.id;
        IF FOUND THEN
            PERFORM pg_notify('runtime_config_reconcile', 'ready');
        END IF;
    END IF;
    RETURN NEW;
END;
$$;



CREATE TRIGGER client_runtime_config_overrides_reconcile
BEFORE INSERT OR UPDATE OR DELETE ON public.client_runtime_config_overrides
FOR EACH ROW EXECUTE FUNCTION public.produce_runtime_config_override_reconcile();

CREATE TRIGGER client_configuration_preset_overrides_reconcile
BEFORE INSERT OR UPDATE OR DELETE ON public.client_configuration_preset_overrides
FOR EACH ROW EXECUTE FUNCTION public.produce_configuration_source_reconcile();

CREATE TRIGGER configuration_presets_reconcile
BEFORE INSERT OR DELETE OR UPDATE OF behavior, is_default, definition ON public.configuration_presets
FOR EACH ROW EXECUTE FUNCTION public.produce_configuration_preset_reconcile();

CREATE TRIGGER ping_target_assignments_reconcile
BEFORE INSERT OR UPDATE OR DELETE ON public.ping_target_assignments
FOR EACH ROW EXECUTE FUNCTION public.produce_ping_assignment_reconcile();

CREATE TRIGGER ping_targets_reconcile
BEFORE DELETE OR UPDATE OF name, host, probe_kind, port, enabled, generation ON public.ping_targets
FOR EACH ROW EXECUTE FUNCTION public.produce_ping_target_reconcile();

CREATE TRIGGER tunnel_plans_runtime_config_reconcile
BEFORE INSERT OR DELETE OR UPDATE OF enabled, left_client_id, right_client_id, plan, builtin_credentials, deleted_at ON public.tunnel_plans
FOR EACH ROW EXECUTE FUNCTION public.produce_tunnel_plan_reconcile();

CREATE TRIGGER network_adapter_definitions_runtime_config_reconcile
BEFORE UPDATE OF definition ON public.network_adapter_definitions
FOR EACH ROW EXECUTE FUNCTION public.produce_network_adapter_reconcile();

CREATE TRIGGER port_forward_rules_runtime_config_reconcile
BEFORE INSERT OR DELETE OR UPDATE OF client_id, name, protocol, target_ip, target_hostname, mappings, masquerade, enabled, revision, deleted_at ON public.port_forward_rules
FOR EACH ROW EXECUTE FUNCTION public.produce_port_forward_reconcile();

CREATE TRIGGER clients_runtime_config_work_lifecycle
AFTER UPDATE OF status, hidden_at, process_incarnation_id ON public.clients
FOR EACH ROW EXECUTE FUNCTION public.maintain_runtime_config_work_for_client_lifecycle();
