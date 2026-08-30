-- Commands, schedules, jobs, terminal sessions, and worker coordination.

SET LOCAL check_function_bodies = false;

-- Functions.

CREATE FUNCTION public.alert_jsonb_string_array_valid(value jsonb, max_items integer) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
    SELECT jsonb_typeof(value) = 'array'
       AND jsonb_array_length(value) BETWEEN 1 AND max_items
       AND NOT EXISTS (
           SELECT 1
           FROM jsonb_array_elements(value) AS item
           WHERE jsonb_typeof(item) <> 'string'
       );
$$;



CREATE FUNCTION public.alert_uuid_array_is_unique_bounded(value uuid[], max_items integer) RETURNS boolean
    LANGUAGE sql IMMUTABLE STRICT
    AS $$
    SELECT cardinality(value) <= max_items
       AND cardinality(value) = (
           SELECT count(DISTINCT item)::INTEGER FROM unnest(value) AS item
       );
$$;



CREATE FUNCTION public.job_target_effective_terminal_at(p_status text, p_completed_at timestamp with time zone, p_result_received_at timestamp with time zone, p_started_at timestamp with time zone, p_cancel_acked_at timestamp with time zone, p_cancel_sent_at timestamp with time zone, p_cancel_requested_at timestamp with time zone) RETURNS timestamp with time zone
    LANGUAGE sql IMMUTABLE PARALLEL SAFE
    AS $$
    SELECT CASE
        WHEN p_status IN ('control_timeout', 'agent_timeout', 'agent_lost') THEN
            COALESCE(p_completed_at, p_result_received_at, p_started_at)
        WHEN p_status = 'canceled' THEN
            COALESCE(
                p_completed_at,
                p_cancel_acked_at,
                p_cancel_sent_at,
                p_cancel_requested_at,
                p_started_at
            )
        ELSE NULL
    END;
$$;



CREATE FUNCTION public.apply_system_dashboard_target_metric_delta(
    p_client_id text,
    p_target_queued bigint,
    p_target_dispatching bigint,
    p_target_running bigint,
    p_total_dispatch_attempts bigint,
    p_retried_targets bigint,
    p_cancel_requested bigint,
    p_cancel_sent bigint,
    p_cancel_acked bigint,
    p_cancel_awaiting_ack bigint
) RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
    UPDATE public.system_dashboard_target_metrics
    SET target_queued = target_queued + p_target_queued,
        target_dispatching = target_dispatching + p_target_dispatching,
        target_running = target_running + p_target_running,
        total_dispatch_attempts = total_dispatch_attempts
            + p_total_dispatch_attempts,
        retried_targets = retried_targets + p_retried_targets,
        cancel_requested = cancel_requested + p_cancel_requested,
        cancel_sent = cancel_sent + p_cancel_sent,
        cancel_acked = cancel_acked + p_cancel_acked,
        cancel_awaiting_ack = cancel_awaiting_ack + p_cancel_awaiting_ack
    WHERE client_id = p_client_id;

    IF NOT FOUND THEN
        INSERT INTO public.system_dashboard_target_metrics AS current_metrics (
            client_id,
            target_queued,
            target_dispatching,
            target_running,
            total_dispatch_attempts,
            retried_targets,
            cancel_requested,
            cancel_sent,
            cancel_acked,
            cancel_awaiting_ack
        ) VALUES (
            p_client_id,
            p_target_queued,
            p_target_dispatching,
            p_target_running,
            p_total_dispatch_attempts,
            p_retried_targets,
            p_cancel_requested,
            p_cancel_sent,
            p_cancel_acked,
            p_cancel_awaiting_ack
        )
        ON CONFLICT (client_id) DO UPDATE SET
            target_queued = current_metrics.target_queued
                + EXCLUDED.target_queued,
            target_dispatching = current_metrics.target_dispatching
                + EXCLUDED.target_dispatching,
            target_running = current_metrics.target_running
                + EXCLUDED.target_running,
            total_dispatch_attempts = current_metrics.total_dispatch_attempts
                + EXCLUDED.total_dispatch_attempts,
            retried_targets = current_metrics.retried_targets
                + EXCLUDED.retried_targets,
            cancel_requested = current_metrics.cancel_requested
                + EXCLUDED.cancel_requested,
            cancel_sent = current_metrics.cancel_sent
                + EXCLUDED.cancel_sent,
            cancel_acked = current_metrics.cancel_acked
                + EXCLUDED.cancel_acked,
            cancel_awaiting_ack = current_metrics.cancel_awaiting_ack
                + EXCLUDED.cancel_awaiting_ack;
    END IF;

    DELETE FROM public.system_dashboard_target_metrics
    WHERE client_id = p_client_id
      AND target_queued = 0
      AND target_dispatching = 0
      AND target_running = 0
      AND total_dispatch_attempts = 0
      AND retried_targets = 0
      AND cancel_requested = 0
      AND cancel_sent = 0
      AND cancel_acked = 0
      AND cancel_awaiting_ack = 0;
END;
$$;



CREATE FUNCTION public.maintain_system_dashboard_target_metrics() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    delta record;
BEGIN
    IF TG_OP = 'INSERT' THEN
        FOR delta IN
            WITH deltas AS (
                SELECT
                    client_id,
                    count(*) FILTER (
                        WHERE completed_at IS NULL AND status = 'queued'
                    )::bigint AS target_queued,
                    count(*) FILTER (
                        WHERE completed_at IS NULL AND status = 'dispatching'
                    )::bigint AS target_dispatching,
                    count(*) FILTER (
                        WHERE completed_at IS NULL AND status = 'running'
                    )::bigint AS target_running,
                    COALESCE(sum(dispatch_attempts), 0)::bigint
                        AS total_dispatch_attempts,
                    count(*) FILTER (WHERE dispatch_attempts > 1)::bigint
                        AS retried_targets,
                    count(*) FILTER (
                        WHERE cancel_requested_at IS NOT NULL
                    )::bigint AS cancel_requested,
                    count(*) FILTER (
                        WHERE cancel_sent_at IS NOT NULL
                    )::bigint AS cancel_sent,
                    count(*) FILTER (
                        WHERE cancel_acked_at IS NOT NULL
                    )::bigint AS cancel_acked,
                    count(*) FILTER (
                        WHERE cancel_sent_at IS NOT NULL
                          AND cancel_acked_at IS NULL
                          AND completed_at IS NULL
                    )::bigint AS cancel_awaiting_ack
                FROM new_system_dashboard_targets
                GROUP BY client_id
            )
            SELECT *
            FROM deltas
            WHERE target_queued <> 0
               OR target_dispatching <> 0
               OR target_running <> 0
               OR total_dispatch_attempts <> 0
               OR retried_targets <> 0
               OR cancel_requested <> 0
               OR cancel_sent <> 0
               OR cancel_acked <> 0
               OR cancel_awaiting_ack <> 0
            ORDER BY client_id COLLATE "C"
        LOOP
            PERFORM public.apply_system_dashboard_target_metric_delta(
                delta.client_id,
                delta.target_queued,
                delta.target_dispatching,
                delta.target_running,
                delta.total_dispatch_attempts,
                delta.retried_targets,
                delta.cancel_requested,
                delta.cancel_sent,
                delta.cancel_acked,
                delta.cancel_awaiting_ack
            );
        END LOOP;
    ELSIF TG_OP = 'DELETE' THEN
        FOR delta IN
            WITH deltas AS (
                SELECT
                    client_id,
                    -count(*) FILTER (
                        WHERE completed_at IS NULL AND status = 'queued'
                    )::bigint AS target_queued,
                    -count(*) FILTER (
                        WHERE completed_at IS NULL AND status = 'dispatching'
                    )::bigint AS target_dispatching,
                    -count(*) FILTER (
                        WHERE completed_at IS NULL AND status = 'running'
                    )::bigint AS target_running,
                    -COALESCE(sum(dispatch_attempts), 0)::bigint
                        AS total_dispatch_attempts,
                    -count(*) FILTER (
                        WHERE dispatch_attempts > 1
                    )::bigint AS retried_targets,
                    -count(*) FILTER (
                        WHERE cancel_requested_at IS NOT NULL
                    )::bigint AS cancel_requested,
                    -count(*) FILTER (
                        WHERE cancel_sent_at IS NOT NULL
                    )::bigint AS cancel_sent,
                    -count(*) FILTER (
                        WHERE cancel_acked_at IS NOT NULL
                    )::bigint AS cancel_acked,
                    -count(*) FILTER (
                        WHERE cancel_sent_at IS NOT NULL
                          AND cancel_acked_at IS NULL
                          AND completed_at IS NULL
                    )::bigint AS cancel_awaiting_ack
                FROM old_system_dashboard_targets
                GROUP BY client_id
            )
            SELECT *
            FROM deltas
            WHERE target_queued <> 0
               OR target_dispatching <> 0
               OR target_running <> 0
               OR total_dispatch_attempts <> 0
               OR retried_targets <> 0
               OR cancel_requested <> 0
               OR cancel_sent <> 0
               OR cancel_acked <> 0
               OR cancel_awaiting_ack <> 0
            ORDER BY client_id COLLATE "C"
        LOOP
            PERFORM public.apply_system_dashboard_target_metric_delta(
                delta.client_id,
                delta.target_queued,
                delta.target_dispatching,
                delta.target_running,
                delta.total_dispatch_attempts,
                delta.retried_targets,
                delta.cancel_requested,
                delta.cancel_sent,
                delta.cancel_acked,
                delta.cancel_awaiting_ack
            );
        END LOOP;
    ELSE
        FOR delta IN
            WITH changes AS (
                SELECT -1::bigint AS direction, old_row.*
                FROM old_system_dashboard_targets old_row
                UNION ALL
                SELECT 1::bigint AS direction, new_row.*
                FROM new_system_dashboard_targets new_row
            ),
            deltas AS (
                SELECT
                    client_id,
                    COALESCE(sum(direction) FILTER (
                        WHERE completed_at IS NULL AND status = 'queued'
                    ), 0)::bigint AS target_queued,
                    COALESCE(sum(direction) FILTER (
                        WHERE completed_at IS NULL AND status = 'dispatching'
                    ), 0)::bigint AS target_dispatching,
                    COALESCE(sum(direction) FILTER (
                        WHERE completed_at IS NULL AND status = 'running'
                    ), 0)::bigint AS target_running,
                    COALESCE(sum(
                        direction * dispatch_attempts::bigint
                    ), 0)::bigint AS total_dispatch_attempts,
                    COALESCE(sum(direction) FILTER (
                        WHERE dispatch_attempts > 1
                    ), 0)::bigint AS retried_targets,
                    COALESCE(sum(direction) FILTER (
                        WHERE cancel_requested_at IS NOT NULL
                    ), 0)::bigint AS cancel_requested,
                    COALESCE(sum(direction) FILTER (
                        WHERE cancel_sent_at IS NOT NULL
                    ), 0)::bigint AS cancel_sent,
                    COALESCE(sum(direction) FILTER (
                        WHERE cancel_acked_at IS NOT NULL
                    ), 0)::bigint AS cancel_acked,
                    COALESCE(sum(direction) FILTER (
                        WHERE cancel_sent_at IS NOT NULL
                          AND cancel_acked_at IS NULL
                          AND completed_at IS NULL
                    ), 0)::bigint AS cancel_awaiting_ack
                FROM changes
                GROUP BY client_id
            )
            SELECT *
            FROM deltas
            WHERE target_queued <> 0
               OR target_dispatching <> 0
               OR target_running <> 0
               OR total_dispatch_attempts <> 0
               OR retried_targets <> 0
               OR cancel_requested <> 0
               OR cancel_sent <> 0
               OR cancel_acked <> 0
               OR cancel_awaiting_ack <> 0
            ORDER BY client_id COLLATE "C"
        LOOP
            PERFORM public.apply_system_dashboard_target_metric_delta(
                delta.client_id,
                delta.target_queued,
                delta.target_dispatching,
                delta.target_running,
                delta.total_dispatch_attempts,
                delta.retried_targets,
                delta.cancel_requested,
                delta.cancel_sent,
                delta.cancel_acked,
                delta.cancel_awaiting_ack
            );
        END LOOP;
    END IF;
    RETURN NULL;
END;
$$;



-- Tables.

CREATE TABLE public.command_templates (
    id uuid NOT NULL,
    name text NOT NULL,
    scope_kind text NOT NULL,
    scope_value text,
    command_type text NOT NULL,
    display_group text,
    operation jsonb NOT NULL,
    defaults jsonb DEFAULT '{}'::jsonb NOT NULL,
    actor_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT command_templates_check CHECK ((((scope_kind = 'global'::text) AND (scope_value IS NULL)) OR ((scope_kind <> 'global'::text) AND (scope_value IS NOT NULL)))),
    CONSTRAINT command_templates_command_type_check CHECK ((command_type = ANY (ARRAY['shell_argv'::text, 'shell_pty'::text, 'shell_script'::text, 'terminal_open'::text, 'config_read'::text, 'runtime_config_sync'::text, 'agent_update'::text, 'agent_update_activate'::text, 'agent_update_rollback'::text, 'agent_update_check'::text, 'file_pull'::text, 'file_push'::text, 'file_push_chunked'::text, 'file_transfer_start'::text, 'file_transfer_chunk'::text, 'file_transfer_commit'::text, 'file_transfer_abort'::text, 'file_transfer_download_start'::text, 'file_transfer_download_chunk'::text, 'file_stat'::text, 'file_list_dir'::text, 'file_read_text'::text, 'file_mkdir'::text, 'file_write_text'::text, 'file_rename'::text, 'file_delete'::text, 'file_chmod'::text, 'file_chown'::text, 'file_copy'::text, 'file_download'::text, 'file_archive_tar'::text, 'user_sessions'::text, 'process_list'::text, 'process_start'::text, 'process_stop'::text, 'process_restart'::text, 'process_status'::text, 'process_logs'::text, 'backup'::text, 'restore'::text, 'restore_rollback'::text, 'network_status'::text, 'network_interfaces'::text, 'network_probe'::text, 'network_speed_test'::text]))),
    CONSTRAINT command_templates_defaults_check CHECK ((jsonb_typeof(defaults) = 'object'::text)),
    CONSTRAINT command_templates_display_group_check CHECK (((display_group IS NULL) OR ((length(display_group) >= 1) AND (length(display_group) <= 64)))),
    CONSTRAINT command_templates_operation_check CHECK ((jsonb_typeof(operation) = 'object'::text)),
    CONSTRAINT command_templates_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['global'::text, 'provider'::text, 'tag'::text, 'client'::text]))),
    CONSTRAINT command_templates_pkey PRIMARY KEY (id),
    CONSTRAINT command_templates_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.job_approvals (
    id uuid NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    job_id uuid NOT NULL,
    command_type text NOT NULL,
    selector_expression text NOT NULL,
    target_client_ids text[] NOT NULL,
    target_count integer DEFAULT 0 NOT NULL,
    privileged boolean DEFAULT true NOT NULL,
    destructive boolean DEFAULT false NOT NULL,
    force_unprivileged boolean DEFAULT false NOT NULL,
    max_timeout_secs bigint DEFAULT 30 NOT NULL,
    payload_hash text NOT NULL,
    request_fingerprint text NOT NULL,
    requester_id uuid,
    requester_username text NOT NULL,
    requester_role text NOT NULL,
    request_reason text,
    risk text DEFAULT 'standard'::text NOT NULL,
    job_request jsonb NOT NULL,
    decision_by uuid,
    decision_username text,
    decision_reason text,
    requested_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT job_approvals_check CHECK ((((status = 'pending'::text) AND (decided_at IS NULL)) OR ((status <> 'pending'::text) AND (decided_at IS NOT NULL)))),
    CONSTRAINT job_approvals_command_type_check CHECK ((length(TRIM(BOTH FROM command_type)) > 0)),
    CONSTRAINT job_approvals_max_timeout_secs_check CHECK ((max_timeout_secs > 0)),
    CONSTRAINT job_approvals_requester_role_check CHECK ((length(TRIM(BOTH FROM requester_role)) > 0)),
    CONSTRAINT job_approvals_requester_username_check CHECK ((length(TRIM(BOTH FROM requester_username)) > 0)),
    CONSTRAINT job_approvals_risk_check CHECK (((length(TRIM(BOTH FROM risk)) >= 1) AND (length(TRIM(BOTH FROM risk)) <= 64))),
    CONSTRAINT job_approvals_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'approved'::text, 'rejected'::text]))),
    CONSTRAINT job_approvals_target_count_check CHECK ((target_count >= 0)),
    CONSTRAINT job_approvals_pkey PRIMARY KEY (id),
    CONSTRAINT job_approvals_decision_by_fkey FOREIGN KEY (decision_by) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT job_approvals_requester_id_fkey FOREIGN KEY (requester_id) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.schedules (
    id uuid NOT NULL,
    actor_id uuid,
    name text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    operation jsonb,
    selector_expression text NOT NULL,
    target_client_ids text[] NOT NULL,
    cron_expr text DEFAULT '0 * * * *'::text,
    timezone text DEFAULT 'UTC'::text,
    next_run_at timestamp with time zone,
    last_run_at timestamp with time zone,
    deferred_until timestamp with time zone,
    catch_up_policy text DEFAULT 'skip_missed'::text,
    catch_up_limit integer DEFAULT 1,
    retry_delay_secs bigint DEFAULT 300,
    max_failures integer DEFAULT 3 NOT NULL,
    failure_count integer DEFAULT 0 NOT NULL,
    last_job_id uuid,
    last_job_status text,
    last_job_completed_at timestamp with time zone,
    last_job_error text,
    last_error text,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    trigger_kind text DEFAULT 'cron'::text NOT NULL,
    event_expression text,
    event_argv_template jsonb,
    definition_revision bigint DEFAULT 1 NOT NULL,
    event_armed_at timestamp with time zone,
    CONSTRAINT schedules_definition_revision_check CHECK ((definition_revision >= 1)),
    CONSTRAINT schedules_event_argv_template_check CHECK (((event_argv_template IS NULL) OR ((trigger_kind = 'event'::text) AND public.alert_jsonb_string_array_valid(event_argv_template, 128)))),
    CONSTRAINT schedules_failure_count_check CHECK ((failure_count >= 0)),
    CONSTRAINT schedules_max_failures_check CHECK (((max_failures >= 1) AND (max_failures <= 100))),
    CONSTRAINT schedules_retry_delay_secs_check CHECK (((retry_delay_secs >= 1) AND (retry_delay_secs <= 86400))),
    CONSTRAINT schedules_target_client_ids_limit CHECK (((cardinality(target_client_ids) >= 0) AND (cardinality(target_client_ids) <= 500))),
    CONSTRAINT schedules_trigger_kind_check CHECK ((trigger_kind = ANY (ARRAY['cron'::text, 'event'::text]))),
    CONSTRAINT schedules_trigger_shape_check CHECK ((((trigger_kind = 'cron'::text) AND (cron_expr IS NOT NULL) AND (length(btrim(cron_expr)) > 0) AND (timezone = 'UTC'::text) AND (next_run_at IS NOT NULL) AND (catch_up_policy = ANY (ARRAY['skip_missed'::text, 'run_once'::text, 'run_all_limited'::text])) AND ((catch_up_limit >= 1) AND (catch_up_limit <= 25)) AND ((retry_delay_secs >= 1) AND (retry_delay_secs <= 86400)) AND (operation IS NOT NULL) AND (event_expression IS NULL) AND (event_argv_template IS NULL) AND (event_armed_at IS NULL)) OR ((trigger_kind = 'event'::text) AND (cron_expr IS NULL) AND (timezone IS NULL) AND (next_run_at IS NULL) AND (catch_up_policy IS NULL) AND (catch_up_limit IS NULL) AND (retry_delay_secs IS NULL) AND (operation IS NULL) AND (event_expression IS NOT NULL) AND ((length(btrim(event_expression)) >= 1) AND (length(btrim(event_expression)) <= 4096)) AND (event_armed_at IS NOT NULL)))),
    CONSTRAINT schedules_pkey PRIMARY KEY (id),
    CONSTRAINT schedules_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id),
    CONSTRAINT schedules_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.jobs (
    id uuid NOT NULL,
    actor_id uuid,
    command_type text NOT NULL,
    privileged boolean DEFAULT false NOT NULL,
    status text NOT NULL,
    target_count integer DEFAULT 0 NOT NULL,
    payload_hash text NOT NULL,
    operation jsonb,
    source_schedule_id uuid,
    approval_id uuid,
    request_fingerprint text NOT NULL,
    max_timeout_secs bigint DEFAULT 30 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    alert_terminal_at timestamp with time zone,
    causation_id uuid,
    schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    resource_kind text GENERATED ALWAYS AS (
        CASE
            WHEN command_type = ANY (ARRAY[
                'file_transfer_start'::text,
                'file_transfer_chunk'::text,
                'file_transfer_commit'::text,
                'file_transfer_abort'::text,
                'file_transfer_download_start'::text,
                'file_transfer_download_chunk'::text
            ]) THEN 'file_transfer_session'::text
            ELSE NULL::text
        END
    ) STORED,
    resource_id uuid GENERATED ALWAYS AS (
        CASE
            WHEN command_type = ANY (ARRAY[
                'file_transfer_start'::text,
                'file_transfer_chunk'::text,
                'file_transfer_commit'::text,
                'file_transfer_abort'::text,
                'file_transfer_download_start'::text,
                'file_transfer_download_chunk'::text
            ]) THEN ((operation ->> 'session_id'::text))::uuid
            ELSE NULL::uuid
        END
    ) STORED,
    CONSTRAINT jobs_alert_terminal_at_check CHECK (((status = ANY (ARRAY['partial_success'::text, 'canceled'::text, 'rejected'::text, 'failed'::text, 'agent_timeout'::text, 'control_timeout'::text])) = (alert_terminal_at IS NOT NULL))),
    CONSTRAINT jobs_resource_identity_shape_check CHECK (((resource_kind IS NULL) = (resource_id IS NULL))),
    CONSTRAINT jobs_schedule_lineage_check CHECK (public.alert_uuid_array_is_unique_bounded(schedule_lineage, 16)),
    CONSTRAINT jobs_status_common_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'completed'::text, 'partial_success'::text, 'skipped'::text, 'rejected'::text, 'failed'::text, 'agent_timeout'::text, 'control_timeout'::text, 'canceled'::text]))),
    CONSTRAINT jobs_pkey PRIMARY KEY (id),
    CONSTRAINT jobs_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id),
    CONSTRAINT jobs_approval_id_fkey FOREIGN KEY (approval_id) REFERENCES public.job_approvals(id),
    CONSTRAINT jobs_source_schedule_id_fkey FOREIGN KEY (source_schedule_id) REFERENCES public.schedules(id)
);



CREATE TABLE public.job_rollouts (
    job_id uuid NOT NULL,
    status text DEFAULT 'running'::text NOT NULL,
    canary_client_ids text[] NOT NULL,
    batch_size integer NOT NULL,
    max_failures integer NOT NULL,
    pause_after_canary boolean DEFAULT true NOT NULL,
    batch_delay_secs bigint DEFAULT 0 NOT NULL,
    current_batch integer DEFAULT 0 NOT NULL,
    total_batches integer NOT NULL,
    failure_baseline integer DEFAULT 0 NOT NULL,
    pause_reason text,
    next_batch_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone,
    CONSTRAINT job_rollouts_batch_delay_check CHECK (((batch_delay_secs >= 0) AND (batch_delay_secs <= 86400))),
    CONSTRAINT job_rollouts_batch_index_check CHECK (((current_batch >= 0) AND (total_batches >= 1) AND (current_batch < total_batches))),
    CONSTRAINT job_rollouts_batch_size_check CHECK (((batch_size >= 1) AND (batch_size <= 100))),
    CONSTRAINT job_rollouts_canary_nonempty CHECK (((cardinality(canary_client_ids) >= 1) AND (cardinality(canary_client_ids) <= 25))),
    CONSTRAINT job_rollouts_failure_baseline_check CHECK ((failure_baseline >= 0)),
    CONSTRAINT job_rollouts_max_failures_check CHECK (((max_failures >= 0) AND (max_failures <= 100))),
    CONSTRAINT job_rollouts_status_check CHECK ((status = ANY (ARRAY['running'::text, 'paused'::text, 'completed'::text, 'aborted'::text]))),
    CONSTRAINT job_rollouts_terminal_shape_check CHECK ((((status = ANY (ARRAY['completed'::text, 'aborted'::text])) AND (completed_at IS NOT NULL)) OR ((status = ANY (ARRAY['running'::text, 'paused'::text])) AND (completed_at IS NULL)))),
    CONSTRAINT job_rollouts_pkey PRIMARY KEY (job_id),
    CONSTRAINT job_rollouts_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE
);



CREATE TABLE public.job_targets (
    job_id uuid NOT NULL,
    client_id text NOT NULL,
    status text NOT NULL,
    message text,
    exit_code integer,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    dispatch_attempts integer DEFAULT 0 NOT NULL,
    dispatch_lease_until timestamp with time zone,
    process_incarnation_id uuid,
    delivered_at timestamp with time zone,
    acked_at timestamp with time zone,
    deadline_at timestamp with time zone,
    cancel_requested_at timestamp with time zone,
    cancel_sent_at timestamp with time zone,
    cancel_acked_at timestamp with time zone,
    result_received_at timestamp with time zone,
    last_dispatch_error text,
    capability_degraded_reason text,
    capability_degraded_hint text,
    capability_alert_at timestamp with time zone,
    CONSTRAINT job_targets_capability_alert_at_check CHECK ((((status = 'skipped'::text) AND (capability_degraded_reason IS NOT NULL) AND (capability_degraded_hint IS NOT NULL)) = (capability_alert_at IS NOT NULL))),
    CONSTRAINT job_targets_capability_degraded_hint_check CHECK (((capability_degraded_hint IS NULL) OR ((length(TRIM(BOTH FROM capability_degraded_hint)) >= 1) AND (length(TRIM(BOTH FROM capability_degraded_hint)) <= 2048)))),
    CONSTRAINT job_targets_capability_degraded_pair_check CHECK (((capability_degraded_reason IS NULL) = (capability_degraded_hint IS NULL))),
    CONSTRAINT job_targets_capability_degraded_reason_check CHECK (((capability_degraded_reason IS NULL) OR ((length(TRIM(BOTH FROM capability_degraded_reason)) >= 1) AND (length(TRIM(BOTH FROM capability_degraded_reason)) <= 256)))),
    CONSTRAINT job_targets_status_common_check CHECK ((status = ANY (ARRAY['queued'::text, 'dispatching'::text, 'running'::text, 'completed'::text, 'skipped'::text, 'rejected'::text, 'failed'::text, 'agent_lost'::text, 'agent_timeout'::text, 'control_timeout'::text, 'canceled'::text]))),
    CONSTRAINT job_targets_pkey PRIMARY KEY (job_id, client_id),
    CONSTRAINT job_targets_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE
);



CREATE TABLE public.file_transfer_session_owners (
    client_id text NOT NULL,
    session_id uuid NOT NULL,
    job_id uuid NOT NULL,
    owner_token uuid DEFAULT gen_random_uuid() NOT NULL,
    acquired_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT file_transfer_session_owners_pkey PRIMARY KEY (client_id, session_id),
    CONSTRAINT file_transfer_session_owners_target_unique UNIQUE (job_id, client_id),
    CONSTRAINT file_transfer_session_owners_job_id_client_id_fkey FOREIGN KEY (job_id, client_id) REFERENCES public.job_targets(job_id, client_id) ON DELETE CASCADE
);



CREATE TABLE public.system_dashboard_target_metrics (
    client_id text NOT NULL,
    target_queued bigint DEFAULT 0 NOT NULL,
    target_dispatching bigint DEFAULT 0 NOT NULL,
    target_running bigint DEFAULT 0 NOT NULL,
    total_dispatch_attempts bigint DEFAULT 0 NOT NULL,
    retried_targets bigint DEFAULT 0 NOT NULL,
    cancel_requested bigint DEFAULT 0 NOT NULL,
    cancel_sent bigint DEFAULT 0 NOT NULL,
    cancel_acked bigint DEFAULT 0 NOT NULL,
    cancel_awaiting_ack bigint DEFAULT 0 NOT NULL,
    CONSTRAINT system_dashboard_target_metrics_nonnegative_check CHECK (
        target_queued >= 0
        AND target_dispatching >= 0
        AND target_running >= 0
        AND total_dispatch_attempts >= 0
        AND retried_targets >= 0
        AND cancel_requested >= 0
        AND cancel_sent >= 0
        AND cancel_acked >= 0
        AND cancel_awaiting_ack >= 0
    ),
    CONSTRAINT system_dashboard_target_metrics_pkey PRIMARY KEY (client_id)
);



CREATE TABLE public.job_outputs (
    job_id uuid NOT NULL,
    client_id text NOT NULL,
    seq integer NOT NULL,
    stream text NOT NULL,
    data bytea NOT NULL,
    exit_code integer,
    done boolean DEFAULT false NOT NULL,
    storage text DEFAULT 'inline'::text NOT NULL,
    object_key text,
    data_sha256_hex text,
    data_size_bytes bigint,
    received_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT job_outputs_pkey PRIMARY KEY (job_id, client_id, seq),
    CONSTRAINT job_outputs_job_id_client_id_fkey FOREIGN KEY (job_id, client_id) REFERENCES public.job_targets(job_id, client_id) ON DELETE CASCADE
);



CREATE TABLE public.job_output_projection_work (
    job_id uuid NOT NULL,
    client_id text NOT NULL,
    seq integer NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT job_output_projection_work_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT job_output_projection_work_lease_shape_check CHECK ((lease_id IS NULL) = (lease_until IS NULL)),
    CONSTRAINT job_output_projection_work_seq_check CHECK ((seq >= 0)),
    CONSTRAINT job_output_projection_work_pkey PRIMARY KEY (job_id, client_id, seq),
    CONSTRAINT job_output_projection_work_output_fkey FOREIGN KEY (job_id, client_id, seq) REFERENCES public.job_outputs(job_id, client_id, seq) ON DELETE CASCADE
);



CREATE TABLE public.network_traffic_import_finalizations (
    job_id uuid NOT NULL,
    client_id text NOT NULL,
    final_seq integer NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT network_traffic_import_finalizations_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT network_traffic_import_finalizations_final_seq_check CHECK ((final_seq >= 0)),
    CONSTRAINT network_traffic_import_finalizations_lease_shape_check CHECK ((lease_id IS NULL) = (lease_until IS NULL)),
    CONSTRAINT network_traffic_import_finalizations_pkey PRIMARY KEY (job_id, client_id),
    CONSTRAINT network_traffic_import_finalizations_job_id_client_id_fkey FOREIGN KEY (job_id, client_id) REFERENCES public.job_targets(job_id, client_id) ON DELETE CASCADE
);



CREATE TABLE public.job_rollout_targets (
    job_id uuid NOT NULL,
    client_id text NOT NULL,
    batch_index integer NOT NULL,
    CONSTRAINT job_rollout_targets_batch_index_check CHECK ((batch_index >= 0)),
    CONSTRAINT job_rollout_targets_pkey PRIMARY KEY (job_id, client_id),
    CONSTRAINT job_rollout_targets_job_id_client_id_fkey FOREIGN KEY (job_id, client_id) REFERENCES public.job_targets(job_id, client_id) ON DELETE CASCADE
);



CREATE TABLE public.job_terminal_events (
    id uuid NOT NULL,
    event_kind text NOT NULL,
    job_id uuid NOT NULL,
    client_id text,
    status text NOT NULL,
    outcome jsonb,
    processing_status text DEFAULT 'queued'::text NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    processed_at timestamp with time zone,
    CONSTRAINT job_terminal_events_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT job_terminal_events_kind_check CHECK ((event_kind = ANY (ARRAY['target_terminalized'::text, 'job_terminalized'::text]))),
    CONSTRAINT job_terminal_events_owner_shape_check CHECK ((((processing_status = 'processing'::text) AND (lease_id IS NOT NULL) AND (lease_until IS NOT NULL)) OR ((processing_status <> 'processing'::text) AND (lease_id IS NULL) AND (lease_until IS NULL)))),
    CONSTRAINT job_terminal_events_processing_status_check CHECK ((processing_status = ANY (ARRAY['queued'::text, 'processing'::text, 'processed'::text, 'failed'::text]))),
    CONSTRAINT job_terminal_events_status_not_empty CHECK ((length(TRIM(BOTH FROM status)) > 0)),
    CONSTRAINT job_terminal_events_target_shape_check CHECK ((((event_kind = 'target_terminalized'::text) AND (client_id IS NOT NULL) AND (jsonb_typeof(outcome) = 'object'::text)) OR ((event_kind = 'job_terminalized'::text) AND (client_id IS NULL) AND (outcome IS NULL)))),
    CONSTRAINT job_terminal_events_pkey PRIMARY KEY (id),
    CONSTRAINT job_terminal_events_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE
);



CREATE TABLE public.job_terminal_enrichment_work (
    event_id uuid NOT NULL,
    job_id uuid NOT NULL,
    client_id text NOT NULL,
    status text NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT job_terminal_enrichment_work_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT job_terminal_enrichment_work_lease_shape_check CHECK ((lease_id IS NULL) = (lease_until IS NULL)),
    CONSTRAINT job_terminal_enrichment_work_status_not_empty CHECK ((length(TRIM(BOTH FROM status)) > 0)),
    CONSTRAINT job_terminal_enrichment_work_pkey PRIMARY KEY (event_id),
    CONSTRAINT job_terminal_enrichment_work_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.job_terminal_events(id) ON DELETE CASCADE
);



CREATE TABLE public.server_artifacts (
    id uuid NOT NULL,
    domain text NOT NULL,
    object_key text NOT NULL,
    sha256_hex text NOT NULL,
    size_bytes bigint NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    reservation_token uuid,
    job_id uuid,
    client_id text,
    stream text,
    seq integer,
    backup_request_id uuid,
    backup_artifact_id uuid,
    release_id uuid,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    tombstoned_at timestamp with time zone,
    deleted_at timestamp with time zone,
    CONSTRAINT server_artifacts_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT server_artifacts_reservation_shape_check CHECK (((status = 'creating'::text) = (reservation_token IS NOT NULL))),
    CONSTRAINT server_artifacts_status_check CHECK ((status = ANY (ARRAY['creating'::text, 'active'::text, 'deleting'::text, 'delete_failed'::text, 'tombstoned'::text, 'deleted'::text]))),
    CONSTRAINT server_artifacts_object_key_key UNIQUE (object_key),
    CONSTRAINT server_artifacts_pkey PRIMARY KEY (id)
);



CREATE TABLE public.server_artifact_deletion_intents (
    artifact_id uuid NOT NULL,
    object_key text NOT NULL,
    sha256_hex text NOT NULL,
    size_bytes bigint NOT NULL,
    source_kind text NOT NULL,
    source_id uuid NOT NULL,
    source_revision bigint NOT NULL,
    source_identity jsonb NOT NULL,
    attempt_count integer DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT server_artifact_deletion_intents_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT server_artifact_deletion_intents_lease_shape_check CHECK (((lease_id IS NULL) = (lease_until IS NULL))),
    CONSTRAINT server_artifact_deletion_intents_size_bytes_check CHECK ((size_bytes >= 0)),
    CONSTRAINT server_artifact_deletion_intents_source_identity_object CHECK ((jsonb_typeof(source_identity) = 'object'::text)),
    CONSTRAINT server_artifact_deletion_intents_source_kind_check CHECK ((source_kind = ANY (ARRAY['backup_policy'::text, 'manual_cleanup'::text, 'history_retention'::text]))),
    CONSTRAINT server_artifact_deletion_intents_source_revision_check CHECK ((source_revision >= 1)),
    CONSTRAINT server_artifact_deletion_intents_object_key_key UNIQUE (object_key),
    CONSTRAINT server_artifact_deletion_intents_pkey PRIMARY KEY (artifact_id),
    CONSTRAINT server_artifact_deletion_intents_artifact_id_fkey FOREIGN KEY (artifact_id) REFERENCES public.server_artifacts(id) ON DELETE CASCADE
);



CREATE TABLE public.server_jobs (
    id uuid NOT NULL,
    job_type text NOT NULL,
    status text NOT NULL,
    expression text,
    preview_hash text,
    matched_count bigint DEFAULT 0 NOT NULL,
    matched_bytes bigint DEFAULT 0 NOT NULL,
    deleted_count bigint DEFAULT 0 NOT NULL,
    deleted_bytes bigint DEFAULT 0 NOT NULL,
    error text,
    attempt_count integer DEFAULT 0 NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    created_by uuid,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    started_at timestamp with time zone,
    completed_at timestamp with time zone,
    canceled_at timestamp with time zone,
    CONSTRAINT server_jobs_attempt_count_check CHECK ((attempt_count >= 0)),
    CONSTRAINT server_jobs_lease_shape_check CHECK ((((status = 'running'::text) AND (lease_id IS NOT NULL) AND (lease_until IS NOT NULL)) OR ((status <> 'running'::text) AND (lease_id IS NULL) AND (lease_until IS NULL)))),
    CONSTRAINT server_jobs_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT server_jobs_status_check CHECK ((status = ANY (ARRAY['queued'::text, 'running'::text, 'completed'::text, 'failed'::text, 'canceled'::text]))),
    CONSTRAINT server_jobs_type_check CHECK ((job_type = 'artifact_cleanup'::text)),
    CONSTRAINT server_jobs_pkey PRIMARY KEY (id),
    CONSTRAINT server_jobs_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.server_job_artifact_cleanup_targets (
    server_job_id uuid NOT NULL,
    artifact_id uuid NOT NULL,
    domain text NOT NULL,
    object_key text NOT NULL,
    sha256_hex text NOT NULL,
    size_bytes bigint NOT NULL,
    status_at_review text NOT NULL,
    outcome text DEFAULT 'pending'::text NOT NULL,
    outcome_reason text,
    processed_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT server_job_artifact_cleanup_targets_status_check CHECK ((status_at_review = ANY (ARRAY['creating'::text, 'active'::text, 'deleting'::text, 'delete_failed'::text, 'tombstoned'::text, 'deleted'::text]))),
    CONSTRAINT server_job_artifact_cleanup_targets_outcome_check CHECK ((outcome = ANY (ARRAY['pending'::text, 'deleted'::text, 'tombstoned'::text, 'skipped'::text]))),
    CONSTRAINT server_job_artifact_cleanup_targets_outcome_shape_check CHECK (((outcome = 'pending'::text) = (processed_at IS NULL))),
    CONSTRAINT server_job_artifact_cleanup_targets_pkey PRIMARY KEY (server_job_id, artifact_id),
    CONSTRAINT server_job_artifact_cleanup_targets_server_job_id_fkey FOREIGN KEY (server_job_id) REFERENCES public.server_jobs(id) ON DELETE CASCADE
);



CREATE TABLE public.terminal_output_chunks (
    client_id text NOT NULL,
    session_id uuid NOT NULL,
    terminal_seq bigint NOT NULL,
    job_id uuid NOT NULL,
    data bytea NOT NULL,
    size_bytes bigint NOT NULL,
    sha256_hex text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT terminal_output_chunks_size_bytes_check CHECK ((size_bytes >= 0)),
    CONSTRAINT terminal_output_chunks_terminal_seq_check CHECK ((terminal_seq > 0)),
    CONSTRAINT terminal_output_chunks_pkey PRIMARY KEY (client_id, session_id, terminal_seq),
    CONSTRAINT terminal_output_chunks_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT terminal_output_chunks_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE
);



CREATE TABLE public.terminal_sessions (
    session_id uuid NOT NULL,
    client_id text NOT NULL,
    job_id uuid NOT NULL,
    state text NOT NULL,
    last_status text NOT NULL,
    argv jsonb DEFAULT '[]'::jsonb NOT NULL,
    cwd text,
    cols bigint,
    rows bigint,
    idle_timeout_secs bigint,
    flow_window_bytes bigint,
    output_first_seq bigint,
    output_next_seq bigint,
    output_retained_first_seq bigint,
    output_retained_bytes bigint,
    output_dropped_bytes bigint,
    output_dropped_chunks bigint,
    output_replay_truncated boolean DEFAULT false NOT NULL,
    last_input_seq bigint DEFAULT 0 NOT NULL,
    close_reason text,
    last_event text NOT NULL,
    last_job_output_seq integer,
    opened_at timestamp with time zone,
    observed_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT terminal_sessions_argv_array CHECK ((jsonb_typeof(argv) = 'array'::text)),
    CONSTRAINT terminal_sessions_last_event_check CHECK ((last_event = ANY (ARRAY['terminal_open'::text, 'terminal_input'::text, 'terminal_resize'::text, 'terminal_close'::text, 'terminal_stream'::text]))),
    CONSTRAINT terminal_sessions_last_input_seq_check CHECK ((last_input_seq >= 0)),
    CONSTRAINT terminal_sessions_last_job_output_seq_check CHECK (((last_job_output_seq IS NULL) OR (last_job_output_seq >= 0))),
    CONSTRAINT terminal_sessions_last_status_check CHECK ((last_status = ANY (ARRAY['opening'::text, 'opened'::text, 'attached'::text, 'rejected'::text, 'failed'::text, 'accepted'::text, 'resized'::text, 'closed'::text, 'missing'::text, 'streaming'::text, 'exited'::text, 'idle_timeout'::text, 'disconnected_timeout'::text, 'lifecycle_disconnected'::text]))),
    CONSTRAINT terminal_sessions_state_check CHECK ((state = ANY (ARRAY['opening'::text, 'open'::text, 'closed'::text, 'missing'::text, 'rejected'::text, 'failed'::text, 'exited'::text]))),
    CONSTRAINT terminal_sessions_job_id_key UNIQUE (job_id),
    CONSTRAINT terminal_sessions_pkey PRIMARY KEY (client_id, session_id),
    CONSTRAINT terminal_sessions_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT terminal_sessions_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE
);



CREATE FUNCTION public.enqueue_job_output_projection_work() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO public.job_output_projection_work AS work (
        job_id, client_id, seq
    ) VALUES (
        NEW.job_id, NEW.client_id, NEW.seq
    )
    ON CONFLICT (job_id, client_id, seq) DO UPDATE
    SET next_attempt_at = LEAST(work.next_attempt_at, now()),
        last_error = NULL,
        updated_at = now();

    PERFORM pg_notify('vpsman_job_output_projection', '');
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.release_file_transfer_session_owner_on_target_terminal() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.completed_at IS NULL AND NEW.completed_at IS NOT NULL THEN
        DELETE FROM public.file_transfer_session_owners owner
        WHERE owner.job_id = NEW.job_id
          AND owner.client_id = NEW.client_id;
    END IF;
    RETURN NULL;
END;
$$;



-- Indexes.

CREATE INDEX command_templates_display_group_idx ON public.command_templates USING btree (scope_kind, scope_value, display_group, updated_at DESC) WHERE (display_group IS NOT NULL);



CREATE UNIQUE INDEX command_templates_global_name_idx ON public.command_templates USING btree (name) WHERE (scope_kind = 'global'::text);



CREATE INDEX command_templates_lookup_idx ON public.command_templates USING btree (scope_kind, scope_value, command_type, updated_at DESC);



CREATE UNIQUE INDEX command_templates_scoped_name_idx ON public.command_templates USING btree (scope_kind, scope_value, name) WHERE (scope_kind <> 'global'::text);



CREATE INDEX job_approvals_job_idx ON public.job_approvals USING btree (job_id);



CREATE INDEX job_approvals_requester_idx ON public.job_approvals USING btree (requester_username, requested_at DESC);



CREATE INDEX job_approvals_status_requested_idx ON public.job_approvals USING btree (status, requested_at DESC, id DESC);



CREATE INDEX job_outputs_created_idx ON public.job_outputs USING btree (created_at, job_id, client_id, seq);



CREATE INDEX job_output_projection_work_due_idx ON public.job_output_projection_work USING btree (next_attempt_at, lease_until, created_at, job_id, client_id, seq);



CREATE UNIQUE INDEX job_outputs_object_key_unique ON public.job_outputs USING btree (object_key) WHERE (object_key IS NOT NULL);



CREATE INDEX network_traffic_import_finalizations_due_idx ON public.network_traffic_import_finalizations USING btree (next_attempt_at, lease_until, created_at, job_id, client_id);



CREATE INDEX job_rollout_targets_batch_idx ON public.job_rollout_targets USING btree (job_id, batch_index, client_id);



CREATE INDEX job_rollouts_active_idx ON public.job_rollouts USING btree (status, next_batch_at, updated_at, job_id) WHERE (completed_at IS NULL);



CREATE INDEX job_targets_active_client_idx ON public.job_targets USING btree (client_id, job_id) WHERE ((completed_at IS NULL) AND (status = ANY (ARRAY['queued'::text, 'dispatching'::text, 'running'::text])));



CREATE INDEX job_targets_active_status_idx ON public.job_targets USING btree (status, job_id, client_id) WHERE (completed_at IS NULL);



CREATE INDEX job_targets_capability_degraded_idx ON public.job_targets USING btree (COALESCE(completed_at, started_at) DESC, job_id DESC, client_id) WHERE (capability_degraded_reason IS NOT NULL);



CREATE INDEX job_targets_client_capability_degraded_idx ON public.job_targets USING btree (client_id, COALESCE(completed_at, started_at) DESC, job_id DESC) WHERE (capability_degraded_reason IS NOT NULL);



CREATE INDEX job_targets_deadline_due_idx ON public.job_targets USING btree (deadline_at, job_id, client_id) WHERE ((completed_at IS NULL) AND (status = ANY (ARRAY['dispatching'::text, 'running'::text])));



CREATE INDEX job_targets_dispatch_due_idx ON public.job_targets USING btree (status, dispatch_lease_until, job_id, client_id) WHERE ((completed_at IS NULL) AND (status = ANY (ARRAY['queued'::text, 'dispatching'::text])));



CREATE INDEX job_targets_recent_effective_terminal_idx ON public.job_targets USING btree (status, public.job_target_effective_terminal_at(status, completed_at, result_received_at, started_at, cancel_acked_at, cancel_sent_at, cancel_requested_at) DESC) WHERE (status = ANY (ARRAY['control_timeout'::text, 'agent_timeout'::text, 'agent_lost'::text, 'canceled'::text]));



CREATE UNIQUE INDEX job_terminal_events_job_unique_idx ON public.job_terminal_events USING btree (job_id) WHERE (event_kind = 'job_terminalized'::text);



CREATE INDEX job_terminal_events_processing_idx ON public.job_terminal_events USING btree (processing_status, next_attempt_at, created_at, id) WHERE (processing_status = ANY (ARRAY['queued'::text, 'failed'::text, 'processing'::text]));



CREATE UNIQUE INDEX job_terminal_events_target_unique_idx ON public.job_terminal_events USING btree (job_id, client_id) WHERE (event_kind = 'target_terminalized'::text);



CREATE INDEX job_terminal_enrichment_work_due_idx ON public.job_terminal_enrichment_work USING btree (next_attempt_at, lease_until, created_at, event_id);



CREATE INDEX job_terminal_enrichment_work_job_idx ON public.job_terminal_enrichment_work USING btree (job_id);



CREATE INDEX jobs_active_dashboard_idx ON public.jobs USING btree (created_at DESC, id DESC) WHERE (status = ANY (ARRAY['queued'::text, 'running'::text]));



CREATE INDEX jobs_active_status_idx ON public.jobs USING btree (status, id) WHERE (completed_at IS NULL);



CREATE UNIQUE INDEX jobs_approval_id_idx ON public.jobs USING btree (approval_id) WHERE (approval_id IS NOT NULL);



CREATE INDEX jobs_created_idx ON public.jobs USING btree (created_at DESC, id DESC);



CREATE INDEX jobs_file_transfer_download_resource_idx ON public.jobs USING btree (resource_id, id) WHERE ((resource_kind = 'file_transfer_session'::text) AND (command_type = 'file_transfer_download_chunk'::text));



CREATE INDEX jobs_scheduled_source_idx ON public.jobs USING btree (status, source_schedule_id) WHERE (source_schedule_id IS NOT NULL);



CREATE INDEX schedules_due_idx ON public.schedules USING btree (enabled, next_run_at, deferred_until) WHERE ((deleted_at IS NULL) AND (trigger_kind = 'cron'::text));



CREATE INDEX schedules_event_enabled_idx ON public.schedules USING btree (event_armed_at, id) WHERE ((deleted_at IS NULL) AND enabled AND (trigger_kind = 'event'::text));



CREATE INDEX schedules_visible_name_idx ON public.schedules USING btree (name, id) WHERE (deleted_at IS NULL);



CREATE INDEX server_artifacts_domain_status_idx ON public.server_artifacts USING btree (domain, status, created_at DESC);



CREATE INDEX server_artifacts_job_idx ON public.server_artifacts USING btree (job_id, client_id, seq) WHERE (job_id IS NOT NULL);



CREATE INDEX server_artifact_deletion_intents_due_idx ON public.server_artifact_deletion_intents USING btree (next_attempt_at, lease_until, created_at, artifact_id);



CREATE INDEX server_job_artifact_cleanup_targets_job_idx ON public.server_job_artifact_cleanup_targets USING btree (server_job_id, created_at, artifact_id);



CREATE INDEX server_jobs_status_created_idx ON public.server_jobs USING btree (status, next_attempt_at, lease_until, created_at);



CREATE INDEX terminal_sessions_observed_idx ON public.terminal_sessions USING btree (observed_at DESC, client_id, session_id);



-- Triggers.

CREATE TRIGGER job_outputs_projection_work_after_insert AFTER INSERT ON public.job_outputs FOR EACH ROW EXECUTE FUNCTION public.enqueue_job_output_projection_work();



CREATE TRIGGER job_targets_system_dashboard_metrics_after_delete AFTER DELETE ON public.job_targets REFERENCING OLD TABLE AS old_system_dashboard_targets FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_system_dashboard_target_metrics();



CREATE TRIGGER job_targets_system_dashboard_metrics_after_insert AFTER INSERT ON public.job_targets REFERENCING NEW TABLE AS new_system_dashboard_targets FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_system_dashboard_target_metrics();



CREATE TRIGGER job_targets_system_dashboard_metrics_after_update AFTER UPDATE ON public.job_targets REFERENCING OLD TABLE AS old_system_dashboard_targets NEW TABLE AS new_system_dashboard_targets FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_system_dashboard_target_metrics();



CREATE TRIGGER job_targets_file_transfer_owner_after_terminal AFTER UPDATE OF completed_at ON public.job_targets FOR EACH ROW EXECUTE FUNCTION public.release_file_transfer_session_owner_on_target_terminal();
