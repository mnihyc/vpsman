-- Retained telemetry, ping series, projections, and retention policy.

SET LOCAL check_function_bodies = false;

-- Functions.

CREATE FUNCTION public.initialize_telemetry_projection_heads_for_client() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO telemetry_projection_heads (
        client_id, accepted_seq, projected_seq,
        accepted_at, projected_at
    ) VALUES (
        NEW.id, 0, 0,
        clock_timestamp(), clock_timestamp()
    ) ON CONFLICT (client_id) DO NOTHING;
    INSERT INTO telemetry_minute_materialization_heads (client_id)
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;
    INSERT INTO traffic_counter_minute_heads (client_id)
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;
    RETURN NEW;
END
$$;



CREATE FUNCTION public.initialize_telemetry_webhook_cursor_for_client() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO telemetry_webhook_cursors (client_id, last_sample_seq)
    VALUES (NEW.id, 0)
    ON CONFLICT (client_id) DO NOTHING;
    RETURN NEW;
END
$$;



-- An ordinal mask has one canonical least-significant-bit-first encoding.
-- A reader must reject the whole vector when its byte count differs or any
-- unused high bit in the final byte is non-zero.
CREATE FUNCTION public.telemetry_ordinal_admission_mask_is_exact(
    p_mask BYTEA,
    p_item_count BIGINT
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT CASE
        WHEN p_item_count < 0 THEN FALSE
        WHEN octet_length(p_mask)::BIGINT <>
             p_item_count / 8
                + CASE WHEN p_item_count % 8 = 0 THEN 0 ELSE 1 END
            THEN FALSE
        WHEN p_item_count % 8 = 0 THEN TRUE
        ELSE get_byte(p_mask, octet_length(p_mask) - 1)
            < (1 << (p_item_count % 8)::INTEGER)
    END
$$;



-- Agent network counters are unsigned 64-bit values while PostgreSQL stores
-- telemetry counters as signed BIGINT. Keep the established saturating wire
-- conversion at the one SQL boundary shared by every raw-payload reader.
CREATE FUNCTION public.telemetry_u64_counter_to_bigint(p_value TEXT)
RETURNS BIGINT
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT LEAST(
        p_value::NUMERIC,
        9223372036854775807::NUMERIC
    )::BIGINT
$$;



CREATE FUNCTION public.validate_telemetry_webhook_cursor_advance() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    accepted BIGINT;
BEGIN
    IF NEW.client_id IS DISTINCT FROM OLD.client_id
       OR NEW.last_sample_seq < OLD.last_sample_seq THEN
        RAISE EXCEPTION 'telemetry webhook cursor is immutable or regressed';
    END IF;
    SELECT head.accepted_seq INTO accepted
    FROM telemetry_projection_heads head
    WHERE head.client_id = NEW.client_id;
    IF accepted IS NULL OR NEW.last_sample_seq > accepted THEN
        RAISE EXCEPTION 'telemetry webhook cursor exceeds accepted sequence';
    END IF;
    RETURN NEW;
END
$$;



CREATE FUNCTION public.reject_updated_telemetry_projection_identity() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    RAISE EXCEPTION
        'telemetry projection sample identity is immutable';
END;
$$;



CREATE FUNCTION public.publish_telemetry_retention_effect() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    effect_name TEXT;
    should_publish BOOLEAN := FALSE;
BEGIN
    IF TG_NARGS <> 1 OR TG_TABLE_SCHEMA <> 'public' THEN
        RAISE EXCEPTION 'invalid telemetry retention effect trigger binding';
    END IF;
    effect_name := TG_ARGV[0];
    IF NOT (
        (effect_name = 'core_minute_frontier_advanced'
            AND TG_TABLE_NAME = 'telemetry_minute_materialization_heads'
            AND TG_OP = 'UPDATE')
        OR (effect_name = 'traffic_minute_frontier_advanced'
            AND TG_TABLE_NAME = 'traffic_counter_minute_heads'
            AND TG_OP = 'UPDATE')
        OR (effect_name = 'ping_facts_published'
            AND TG_TABLE_NAME = 'telemetry_ping_facts'
            AND TG_OP = ANY (ARRAY['INSERT'::text, 'UPDATE'::text]))
        OR (effect_name = 'ping_facts_deleted'
            AND TG_TABLE_NAME = 'telemetry_ping_facts'
            AND TG_OP = 'DELETE')
        OR (effect_name = 'ping_current_deleted'
            AND TG_TABLE_NAME = 'telemetry_ping_current'
            AND TG_OP = 'DELETE')
        OR (effect_name = 'ping_rollups_deleted'
            AND TG_TABLE_NAME = 'telemetry_ping_rollups'
            AND TG_OP = 'DELETE')
        OR (effect_name = 'telemetry_samples_deleted'
            AND TG_TABLE_NAME = 'telemetry_samples'
            AND TG_OP = 'DELETE')
        OR (effect_name = 'network_observation_history_published'
            AND TG_TABLE_NAME = 'network_observations'
            AND TG_OP = ANY (ARRAY['INSERT'::text, 'UPDATE'::text]))
        OR (effect_name = 'network_observation_history_deleted'
            AND TG_TABLE_NAME = ANY (ARRAY[
                'network_observations'::text,
                'network_observation_rollups'::text
            ])
            AND TG_OP = 'DELETE')
        OR (effect_name = 'network_observation_latest_deleted'
            AND TG_TABLE_NAME = 'network_observation_latest'
            AND TG_OP = 'DELETE')
        OR (effect_name = 'network_observation_series_deactivated'
            AND TG_TABLE_NAME = 'network_observation_series'
            AND TG_OP = 'UPDATE')
    ) THEN
        RAISE EXCEPTION 'unsupported telemetry retention effect trigger binding: %.%.% -> %',
            TG_TABLE_SCHEMA, TG_TABLE_NAME, TG_OP, effect_name;
    END IF;

    IF effect_name = 'network_observation_history_published' THEN
        SELECT EXISTS (
            SELECT 1
            FROM new_telemetry_retention_rows
            WHERE source = 'manual'
        ) INTO should_publish;
    ELSIF TG_OP = 'INSERT' THEN
        SELECT EXISTS (SELECT 1 FROM new_telemetry_retention_rows)
        INTO should_publish;
    ELSIF TG_OP = 'DELETE' THEN
        SELECT EXISTS (SELECT 1 FROM old_telemetry_retention_rows)
        INTO should_publish;
    ELSIF effect_name = ANY (ARRAY[
        'core_minute_frontier_advanced'::text,
        'traffic_minute_frontier_advanced'::text
    ]) THEN
        SELECT EXISTS (
            SELECT 1
            FROM old_telemetry_retention_rows old_row
            JOIN new_telemetry_retention_rows new_row USING (client_id)
            WHERE new_row.materialized_seq > old_row.materialized_seq
        ) INTO should_publish;
    ELSIF effect_name = 'network_observation_series_deactivated' THEN
        SELECT EXISTS (
            SELECT 1
            FROM old_telemetry_retention_rows old_row
            JOIN new_telemetry_retention_rows new_row USING (id)
            WHERE old_row.active AND NOT new_row.active
        ) INTO should_publish;
    ELSE
        SELECT EXISTS (SELECT 1 FROM new_telemetry_retention_rows)
        INTO should_publish;
    END IF;

    IF should_publish THEN
        PERFORM pg_notify(
            'vpsman_telemetry_retention',
            jsonb_build_object(
                'owner', 'history_retention',
                'effect', effect_name
            )::TEXT
        );
    END IF;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.enqueue_telemetry_history_due_events() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    earliest_coalesce_ready_at TIMESTAMPTZ;
BEGIN
    IF TG_NARGS <> 1
       OR TG_TABLE_SCHEMA <> 'public'
       OR TG_TABLE_NAME <> TG_ARGV[0]
       OR NOT (TG_ARGV[0] = ANY (ARRAY[
            'telemetry_rollups'::text,
            'telemetry_network_rates'::text,
            'telemetry_ping_rollups'::text,
            'system_metric_rollups'::text,
            'network_observation_rollups'::text
       ]))
       OR NOT (TG_OP = ANY (ARRAY['INSERT'::text, 'UPDATE'::text])) THEN
        RAISE EXCEPTION 'invalid telemetry history due-event trigger binding';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM new_telemetry_history_rows) THEN
        RETURN NULL;
    END IF;

    -- Producers append immutable eligibility evidence. They never contend on
    -- the unique due-span authority owned by retention. coalesce_ready_at is the
    -- producer-owned completeness boundary: every edge waits for its whole
    -- destination bucket. A genuinely late additive fragment is already past
    -- that boundary and remains immediately eligible. The coalescer then moves
    -- the complete evidence into that authority in a separate short transaction;
    -- an event committed after its DELETE snapshot remains as the next durable
    -- wake-up.
    WITH phases(
        source_bucket_secs, destination_bucket_secs, retain_days
    ) AS (
        VALUES
            (60, 300, 2),
            (300, 1800, 8),
            (1800, 3600, 31),
            (3600, 10800, 91),
            (10800, 21600, 181),
            (21600, 86400, 366)
    ), inserted AS (
        INSERT INTO public.telemetry_history_due_events (
            domain, source_bucket_secs, destination_bucket_secs,
            owner_identity, destination_start, coalesce_ready_at, due_at
        )
        SELECT DISTINCT
        TG_ARGV[0]::text AS domain,
        phase.source_bucket_secs,
        phase.destination_bucket_secs,
        CASE TG_ARGV[0]
            WHEN 'telemetry_rollups'
                THEN ARRAY[to_jsonb(row) ->> 'client_id']
            WHEN 'telemetry_network_rates'
                THEN ARRAY[
                    to_jsonb(row) ->> 'client_id',
                    to_jsonb(row) ->> 'interface'
                ]
            WHEN 'telemetry_ping_rollups'
                THEN ARRAY[to_jsonb(row) ->> 'series_id']
            WHEN 'system_metric_rollups'
                THEN ARRAY[to_jsonb(row) ->> 'metric']
            WHEN 'network_observation_rollups'
                THEN ARRAY[to_jsonb(row) ->> 'series_id']
            ELSE NULL
        END AS owner_identity,
        date_bin(
            make_interval(secs => phase.destination_bucket_secs),
            row.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS destination_start,
        date_bin(
            make_interval(secs => phase.destination_bucket_secs),
            row.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) + make_interval(secs => phase.destination_bucket_secs)
            AS coalesce_ready_at,
        date_bin(
            make_interval(secs => phase.destination_bucket_secs),
            row.bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) + make_interval(secs =>
            phase.destination_bucket_secs + phase.retain_days * 86400
        ) AS due_at
        FROM new_telemetry_history_rows row
        JOIN phases phase ON row.bucket_secs = phase.source_bucket_secs
        ORDER BY domain, owner_identity, source_bucket_secs,
                 destination_bucket_secs, destination_start, coalesce_ready_at
        RETURNING coalesce_ready_at
    )
    SELECT min(coalesce_ready_at)
    INTO earliest_coalesce_ready_at
    FROM inserted;

    -- One commit-scoped publication covers both consumers of this exact
    -- statement: the domain's terminal-retention frontier and, when a source
    -- tier has a successor, the durable due-event coalescer. Terminal-day rows
    -- have no successor event but can still move their prune frontier earlier.
    -- The transition table proves that at least one row was actually written;
    -- durable rollups/events remain authoritative after a lost notification.
    PERFORM pg_notify(
        'vpsman_telemetry_retention',
        jsonb_strip_nulls(jsonb_build_object(
            'owner', 'history_retention',
            'effect', 'ordinary_rollup_published',
            'domain', TG_ARGV[0],
            'ready_at_unix',
                EXTRACT(EPOCH FROM earliest_coalesce_ready_at)::BIGINT
        ))::TEXT
    );

    RETURN NULL;
END;
$$;



CREATE FUNCTION public.publish_telemetry_history_due_span() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    span_domain TEXT;
    span_source_bucket_secs INTEGER;
    span_destination_bucket_secs INTEGER;
    span_due_at TIMESTAMPTZ;
    phase_count BIGINT;
BEGIN
    IF TG_NARGS <> 0
       OR TG_TABLE_SCHEMA <> 'public'
       OR TG_TABLE_NAME <> 'telemetry_history_due_spans'
       OR NOT (TG_OP = ANY (ARRAY['INSERT'::text, 'UPDATE'::text])) THEN
        RAISE EXCEPTION 'invalid telemetry history due-span trigger binding';
    END IF;
    IF NOT EXISTS (SELECT 1 FROM new_telemetry_retention_rows) THEN
        RETURN NULL;
    END IF;
    SELECT
        min(domain),
        min(source_bucket_secs),
        min(destination_bucket_secs),
        min(due_at),
        count(DISTINCT ROW(domain, source_bucket_secs, destination_bucket_secs))
    INTO
        span_domain,
        span_source_bucket_secs,
        span_destination_bucket_secs,
        span_due_at,
        phase_count
    FROM new_telemetry_retention_rows;
    IF phase_count <> 1 THEN
        RAISE EXCEPTION 'one due-span statement published multiple retention phases';
    END IF;
    PERFORM pg_notify(
        'vpsman_telemetry_retention',
        jsonb_build_object(
            'owner', 'history_retention',
            'effect', 'due_span_published',
            'domain', span_domain,
            'source_bucket_secs', span_source_bucket_secs,
            'destination_bucket_secs', span_destination_bucket_secs,
            'due_at_unix', EXTRACT(EPOCH FROM span_due_at)::BIGINT
        )::TEXT
    );
    RETURN NULL;
END;
$$;



-- Tables.

CREATE TABLE public.history_retention_policies (
    domain text NOT NULL,
    retention_days integer NOT NULL,
    prune_limit integer NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    metadata_only boolean DEFAULT false NOT NULL,
    export_enabled boolean DEFAULT true NOT NULL,
    notes text,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT history_retention_policies_domain_check CHECK ((domain = ANY (ARRAY['audit_logs'::text, 'system_metric_rollups'::text, 'telemetry_rollups'::text, 'telemetry_network_rates'::text, 'telemetry_ping_rollups'::text, 'traffic_counter_rollups'::text, 'job_outputs'::text, 'network_observations'::text, 'client_status_history'::text, 'gateway_sessions'::text]))),
    CONSTRAINT history_retention_policies_bounded_domains_enabled_check CHECK ((enabled OR (domain <> ALL (ARRAY['system_metric_rollups'::text, 'telemetry_rollups'::text, 'telemetry_network_rates'::text, 'telemetry_ping_rollups'::text, 'traffic_counter_rollups'::text, 'network_observations'::text])))),
    CONSTRAINT history_retention_policies_notes_check CHECK (((notes IS NULL) OR (length(notes) <= 1000))),
    CONSTRAINT history_retention_policies_prune_limit_check CHECK (((prune_limit >= 1) AND (prune_limit <= 100000))),
    CONSTRAINT history_retention_policies_retention_days_check CHECK (((retention_days >= 1) AND (retention_days <= 3650))),
    CONSTRAINT history_retention_policies_traffic_rollup_min_days_check CHECK (((domain <> 'traffic_counter_rollups'::text) OR (retention_days >= 32))),
    CONSTRAINT history_retention_policies_pkey PRIMARY KEY (domain),
    CONSTRAINT history_retention_policies_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id)
);



CREATE TABLE public.traffic_history_retention_cursors (
    domain text NOT NULL,
    source_bucket_secs integer NOT NULL,
    destination_bucket_secs integer NOT NULL,
    traffic_client_id text,
    traffic_source_kind text,
    traffic_interface text,
    traffic_lane text,
    traffic_frontier_start timestamp with time zone,
    traffic_scan_after timestamp with time zone,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT traffic_history_retention_cursors_domain_check CHECK ((domain = 'traffic_counter_samples'::text)),
    CONSTRAINT traffic_history_retention_cursors_shape_check CHECK (
        (
            (source_bucket_secs = 0 AND destination_bucket_secs = 0)
            OR (source_bucket_secs = 0 AND destination_bucket_secs = -1)
            OR (source_bucket_secs = 3600 AND destination_bucket_secs = 10800)
            OR (source_bucket_secs = 10800 AND destination_bucket_secs = 21600)
            OR (source_bucket_secs = 21600 AND destination_bucket_secs = 86400)
        )
        AND (
            num_nonnulls(traffic_client_id, traffic_source_kind,
                traffic_interface, traffic_lane, traffic_scan_after) = 0
            OR (
                num_nonnulls(traffic_client_id, traffic_source_kind,
                    traffic_interface, traffic_lane, traffic_scan_after) = 5
                AND length(traffic_client_id) >= 1
                AND traffic_source_kind = ANY (ARRAY['host'::text, 'tunnel'::text])
                AND length(traffic_interface) BETWEEN 1 AND 128
                AND (
                    (source_bucket_secs = 0
                        AND destination_bucket_secs = -1
                        AND traffic_lane = ANY (
                            ARRAY['raw'::text, 'raw_deferred'::text]))
                    OR (source_bucket_secs = 0
                        AND destination_bucket_secs = 0
                        AND traffic_lane = ANY (ARRAY[
                            'prune_1h_live'::text,
                            'prune_1h_vnstat_import'::text,
                            'prune_3h_live'::text,
                            'prune_3h_vnstat_import'::text,
                            'prune_6h_live'::text,
                            'prune_6h_vnstat_import'::text,
                            'prune_1d_live'::text,
                            'prune_1d_vnstat_import'::text
                        ]))
                    OR (source_bucket_secs > 0
                        AND traffic_lane = ANY (
                            ARRAY['live'::text, 'vnstat_import'::text]))
                )
            )
        )),
    CONSTRAINT traffic_history_retention_cursors_frontier_check CHECK (
        (source_bucket_secs = 0
            AND destination_bucket_secs = -1
            AND ((traffic_client_id IS NULL AND traffic_frontier_start IS NULL)
                OR (traffic_client_id IS NOT NULL
                    AND traffic_frontier_start IS NOT NULL)))
        OR ((source_bucket_secs <> 0 OR destination_bucket_secs <> -1)
            AND traffic_frontier_start IS NULL)),
    CONSTRAINT traffic_history_retention_cursors_alignment_check CHECK (((traffic_scan_after IS NULL) OR (traffic_scan_after = date_trunc('minute'::text, traffic_scan_after))) AND ((traffic_frontier_start IS NULL) OR (traffic_frontier_start = date_trunc('minute'::text, traffic_frontier_start)))),
    CONSTRAINT traffic_history_retention_cursors_pkey PRIMARY KEY (domain, source_bucket_secs, destination_bucket_secs)
);



CREATE TABLE public.telemetry_history_due_spans (
    domain text NOT NULL,
    source_bucket_secs integer NOT NULL,
    destination_bucket_secs integer NOT NULL,
    -- Domain supplies the type of this exact natural owner: [client],
    -- [client, interface], [series], or [metric]. Keeping this identity in the
    -- work key prevents one claimed time span from expanding to a fleet scan.
    owner_identity text[] NOT NULL,
    destination_start timestamp with time zone NOT NULL,
    due_at timestamp with time zone NOT NULL,
    CONSTRAINT telemetry_history_due_spans_domain_tier_check CHECK (
        ((domain = ANY (ARRAY[
                'telemetry_rollups'::text,
                'telemetry_network_rates'::text,
                'telemetry_ping_rollups'::text,
                'system_metric_rollups'::text,
                'network_observation_rollups'::text
            ]))
            AND (source_bucket_secs = ANY (
                ARRAY[60, 300, 1800, 3600, 10800, 21600]))
            AND (destination_bucket_secs = CASE source_bucket_secs
                WHEN 60 THEN 300
                WHEN 300 THEN 1800
                WHEN 1800 THEN 3600
                WHEN 3600 THEN 10800
                WHEN 10800 THEN 21600
                WHEN 21600 THEN 86400
                ELSE NULL
            END))),
    CONSTRAINT telemetry_history_due_spans_destination_alignment_check CHECK (
        destination_start = date_bin(
            make_interval(secs => destination_bucket_secs),
            destination_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        )),
    CONSTRAINT telemetry_history_due_spans_owner_identity_check CHECK (
        array_ndims(owner_identity) = 1
        AND array_lower(owner_identity, 1) = 1
        AND array_position(owner_identity, NULL) IS NULL
        AND CASE domain
            WHEN 'telemetry_network_rates' THEN
                cardinality(owner_identity) = 2
                AND length(owner_identity[1]) >= 1
                AND length(owner_identity[2]) BETWEEN 1 AND 128
            WHEN 'telemetry_rollups' THEN
                cardinality(owner_identity) = 1
                AND length(owner_identity[1]) >= 1
            WHEN 'system_metric_rollups' THEN
                cardinality(owner_identity) = 1
                AND length(btrim(owner_identity[1])) BETWEEN 1 AND 128
            WHEN 'telemetry_ping_rollups' THEN
                cardinality(owner_identity) = 1
                AND owner_identity[1] ~ '^[1-9][0-9]{0,18}$'
                AND (length(owner_identity[1]) < 19
                    OR owner_identity[1] <= '9223372036854775807')
            WHEN 'network_observation_rollups' THEN
                cardinality(owner_identity) = 1
                AND owner_identity[1] ~ '^[1-9][0-9]{0,18}$'
                AND (length(owner_identity[1]) < 19
                    OR owner_identity[1] <= '9223372036854775807')
            ELSE FALSE
        END
    ),
    CONSTRAINT telemetry_history_due_spans_due_at_check CHECK (
        due_at = destination_start
            + make_interval(secs => destination_bucket_secs
                + CASE destination_bucket_secs
                WHEN 300 THEN 2 * 86400
                WHEN 1800 THEN 8 * 86400
                WHEN 3600 THEN 31 * 86400
                WHEN 10800 THEN 91 * 86400
                WHEN 21600 THEN 181 * 86400
                WHEN 86400 THEN 366 * 86400
                ELSE NULL
            END)),
    CONSTRAINT telemetry_history_due_spans_pkey PRIMARY KEY (
        domain, source_bucket_secs, destination_bucket_secs,
        destination_start, owner_identity
    )
);



CREATE TABLE public.telemetry_history_due_events (
    event_id bigint GENERATED ALWAYS AS IDENTITY NOT NULL,
    domain text NOT NULL,
    source_bucket_secs integer NOT NULL,
    destination_bucket_secs integer NOT NULL,
    owner_identity text[] NOT NULL,
    destination_start timestamp with time zone NOT NULL,
    coalesce_ready_at timestamp with time zone NOT NULL,
    due_at timestamp with time zone NOT NULL,
    CONSTRAINT telemetry_history_due_events_domain_tier_check CHECK (
        ((domain = ANY (ARRAY[
                'telemetry_rollups'::text,
                'telemetry_network_rates'::text,
                'telemetry_ping_rollups'::text,
                'system_metric_rollups'::text,
                'network_observation_rollups'::text
            ]))
            AND (source_bucket_secs = ANY (
                ARRAY[60, 300, 1800, 3600, 10800, 21600]))
            AND (destination_bucket_secs = CASE source_bucket_secs
                WHEN 60 THEN 300
                WHEN 300 THEN 1800
                WHEN 1800 THEN 3600
                WHEN 3600 THEN 10800
                WHEN 10800 THEN 21600
                WHEN 21600 THEN 86400
                ELSE NULL
            END))),
    CONSTRAINT telemetry_history_due_events_destination_alignment_check CHECK (
        destination_start = date_bin(
            make_interval(secs => destination_bucket_secs),
            destination_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        )),
    CONSTRAINT telemetry_history_due_events_owner_identity_check CHECK (
        array_ndims(owner_identity) = 1
        AND array_lower(owner_identity, 1) = 1
        AND array_position(owner_identity, NULL) IS NULL
        AND CASE domain
            WHEN 'telemetry_network_rates' THEN
                cardinality(owner_identity) = 2
                AND length(owner_identity[1]) >= 1
                AND length(owner_identity[2]) BETWEEN 1 AND 128
            WHEN 'telemetry_rollups' THEN
                cardinality(owner_identity) = 1
                AND length(owner_identity[1]) >= 1
            WHEN 'system_metric_rollups' THEN
                cardinality(owner_identity) = 1
                AND length(btrim(owner_identity[1])) BETWEEN 1 AND 128
            WHEN 'telemetry_ping_rollups' THEN
                cardinality(owner_identity) = 1
                AND owner_identity[1] ~ '^[1-9][0-9]{0,18}$'
                AND (length(owner_identity[1]) < 19
                    OR owner_identity[1] <= '9223372036854775807')
            WHEN 'network_observation_rollups' THEN
                cardinality(owner_identity) = 1
                AND owner_identity[1] ~ '^[1-9][0-9]{0,18}$'
                AND (length(owner_identity[1]) < 19
                    OR owner_identity[1] <= '9223372036854775807')
            ELSE FALSE
        END
    ),
    CONSTRAINT telemetry_history_due_events_coalesce_ready_at_check CHECK (
        coalesce_ready_at > destination_start
        AND coalesce_ready_at <= destination_start
            + make_interval(secs => destination_bucket_secs)
        AND coalesce_ready_at < due_at),
    CONSTRAINT telemetry_history_due_events_due_at_check CHECK (
        due_at = destination_start
            + make_interval(secs => destination_bucket_secs
                + CASE destination_bucket_secs
                WHEN 300 THEN 2 * 86400
                WHEN 1800 THEN 8 * 86400
                WHEN 3600 THEN 31 * 86400
                WHEN 10800 THEN 91 * 86400
                WHEN 21600 THEN 181 * 86400
                WHEN 86400 THEN 366 * 86400
                ELSE NULL
            END)),
    CONSTRAINT telemetry_history_due_events_pkey PRIMARY KEY (event_id)
);



CREATE TABLE public.ping_targets (
    id uuid NOT NULL,
    name text NOT NULL,
    host text NOT NULL,
    probe_kind text NOT NULL,
    port integer,
    enabled boolean DEFAULT true NOT NULL,
    selector_expression text DEFAULT '*'::text NOT NULL,
    generation bigint DEFAULT 1 NOT NULL,
    created_by uuid,
    updated_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ping_targets_check CHECK ((((probe_kind = 'icmp'::text) AND (port IS NULL)) OR ((probe_kind = 'tcp'::text) AND ((port >= 1) AND (port <= 65535))))),
    CONSTRAINT ping_targets_generation_check CHECK ((generation > 0)),
    CONSTRAINT ping_targets_host_check CHECK (((length(TRIM(BOTH FROM host)) >= 1) AND (length(TRIM(BOTH FROM host)) <= 253))),
    CONSTRAINT ping_targets_name_check CHECK (((length(TRIM(BOTH FROM name)) >= 1) AND (length(TRIM(BOTH FROM name)) <= 128))),
    CONSTRAINT ping_targets_probe_kind_check CHECK ((probe_kind = ANY (ARRAY['icmp'::text, 'tcp'::text]))),
    CONSTRAINT ping_targets_selector_expression_check CHECK (((length(TRIM(BOTH FROM selector_expression)) >= 1) AND (length(TRIM(BOTH FROM selector_expression)) <= 4096))),
    CONSTRAINT ping_targets_pkey PRIMARY KEY (id),
    CONSTRAINT ping_targets_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.operators(id),
    CONSTRAINT ping_targets_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id)
);



CREATE TABLE public.ping_target_assignments (
    target_id uuid NOT NULL,
    client_id text NOT NULL,
    is_primary boolean DEFAULT false NOT NULL,
    assigned_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT ping_target_assignments_pkey PRIMARY KEY (target_id, client_id),
    CONSTRAINT ping_target_assignments_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT ping_target_assignments_target_id_fkey FOREIGN KEY (target_id) REFERENCES public.ping_targets(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_ingest_watermarks (
    client_id text NOT NULL,
    process_incarnation_id uuid NOT NULL,
    telemetry_seq bigint NOT NULL,
    reported_observed_unix bigint NOT NULL,
    accepted_at timestamp with time zone DEFAULT now() NOT NULL,
    gateway_session_id uuid NOT NULL,
    CONSTRAINT telemetry_ingest_watermarks_reported_observed_unix_check CHECK ((reported_observed_unix >= 0)),
    CONSTRAINT telemetry_ingest_watermarks_telemetry_seq_check CHECK ((telemetry_seq > 0)),
    CONSTRAINT telemetry_ingest_watermarks_pkey PRIMARY KEY (client_id),
    CONSTRAINT telemetry_ingest_watermarks_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



-- Per-stream traffic summaries are durable consumer state. Raw arrivals never
-- mutate this table; the traffic minute consumer updates it only after the
-- corresponding closed sample is durable.
CREATE TABLE public.traffic_counter_streams (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    source_revision bigint DEFAULT 0 NOT NULL,
    materialized_revision bigint DEFAULT 0 NOT NULL,
    usage_rx_bytes bigint DEFAULT 0 NOT NULL,
    usage_tx_bytes bigint DEFAULT 0 NOT NULL,
    usage_rx_reset_count bigint DEFAULT 0 NOT NULL,
    usage_tx_reset_count bigint DEFAULT 0 NOT NULL,
    usage_row_count bigint DEFAULT 0 NOT NULL,
    sample_edge_revision bigint DEFAULT 0 NOT NULL,
    latest_sample_observed_at timestamp with time zone,
    latest_sample_rx_bytes bigint,
    latest_sample_tx_bytes bigint,
    latest_sample_rx_counter_epoch bigint,
    latest_sample_tx_counter_epoch bigint,
    latest_sample_source text,
    latest_sample_effective_observed_at timestamp with time zone,
    latest_sample_count integer,
    latest_sample_rx_bytes_avg bigint,
    latest_sample_tx_bytes_avg bigint,
    latest_sample_updated_at timestamp with time zone,
    previous_sample_effective_observed_at timestamp with time zone,
    previous_sample_rx_bytes bigint,
    previous_sample_tx_bytes bigint,
    previous_sample_rx_counter_epoch bigint,
    previous_sample_tx_counter_epoch bigint,
    first_exact_observed_at timestamp with time zone,
    last_exact_observed_at timestamp with time zone,
    first_unpromoted_observed_at timestamp with time zone,
    promoted_boundary_safe boolean DEFAULT false NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT traffic_counter_streams_pkey PRIMARY KEY (client_id, source_kind, interface),
    CONSTRAINT traffic_counter_streams_client_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT traffic_counter_streams_source_kind_check CHECK (source_kind = ANY (ARRAY['host'::text, 'tunnel'::text])),
    CONSTRAINT traffic_counter_streams_interface_check CHECK (length(interface) BETWEEN 1 AND 128),
    CONSTRAINT traffic_counter_streams_revision_check CHECK (
        source_revision >= 0
        AND materialized_revision BETWEEN 0 AND source_revision
        AND sample_edge_revision BETWEEN 0 AND source_revision
    ),
    CONSTRAINT traffic_counter_streams_usage_check CHECK (
        usage_rx_bytes >= 0 AND usage_tx_bytes >= 0
        AND usage_rx_reset_count >= 0 AND usage_tx_reset_count >= 0
        AND usage_row_count >= 0
    ),
    CONSTRAINT traffic_counter_streams_sample_edge_check CHECK (
        num_nonnulls(
            latest_sample_observed_at, latest_sample_rx_bytes,
            latest_sample_tx_bytes, latest_sample_rx_counter_epoch,
            latest_sample_tx_counter_epoch, latest_sample_source,
            latest_sample_effective_observed_at, latest_sample_count,
            latest_sample_rx_bytes_avg, latest_sample_tx_bytes_avg,
            latest_sample_updated_at
        ) = ANY (ARRAY[0, 11])
        AND num_nonnulls(
            previous_sample_effective_observed_at,
            previous_sample_rx_bytes, previous_sample_tx_bytes,
            previous_sample_rx_counter_epoch,
            previous_sample_tx_counter_epoch
        ) = ANY (ARRAY[0, 5])
        AND (
            previous_sample_effective_observed_at IS NULL
            OR latest_sample_observed_at IS NOT NULL
        )
        AND (
            latest_sample_observed_at IS NULL
            OR sample_edge_revision > 0
        )
        AND COALESCE(latest_sample_rx_bytes >= 0, true)
        AND COALESCE(latest_sample_tx_bytes >= 0, true)
        AND COALESCE(latest_sample_rx_counter_epoch >= 0, true)
        AND COALESCE(latest_sample_tx_counter_epoch >= 0, true)
        AND COALESCE(latest_sample_count > 0, true)
        AND COALESCE(latest_sample_rx_bytes_avg >= 0, true)
        AND COALESCE(latest_sample_tx_bytes_avg >= 0, true)
        AND COALESCE(
            latest_sample_observed_at =
                date_trunc('minute', latest_sample_observed_at), true
        )
        AND COALESCE(
            latest_sample_effective_observed_at >= latest_sample_observed_at
            AND latest_sample_effective_observed_at <
                latest_sample_observed_at + interval '1 minute', true
        )
        AND COALESCE(previous_sample_rx_bytes >= 0, true)
        AND COALESCE(previous_sample_tx_bytes >= 0, true)
        AND COALESCE(previous_sample_rx_counter_epoch >= 0, true)
        AND COALESCE(previous_sample_tx_counter_epoch >= 0, true)
        AND COALESCE(
            previous_sample_effective_observed_at <
                latest_sample_effective_observed_at, true
        )
        AND ((first_exact_observed_at IS NULL) = (last_exact_observed_at IS NULL))
        AND COALESCE(first_exact_observed_at <= last_exact_observed_at, true)
        AND COALESCE(last_exact_observed_at <= latest_sample_observed_at, true)
        AND COALESCE(first_unpromoted_observed_at = date_trunc('minute', first_unpromoted_observed_at), true)
        AND COALESCE(first_unpromoted_observed_at <= latest_sample_observed_at, true)
    )
);



-- Exact traffic endpoints are also the sole closed 60-second network
-- aggregate. Import and direct-history writers may omit aggregate columns;
-- the normalization trigger in 0005 canonicalizes those rows to one sample.
CREATE TABLE public.traffic_counter_samples (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    rx_bytes bigint NOT NULL,
    tx_bytes bigint NOT NULL,
    rx_counter_epoch bigint DEFAULT 0 NOT NULL,
    tx_counter_epoch bigint DEFAULT 0 NOT NULL,
    sample_source text NOT NULL,
    inbound_promoted boolean DEFAULT false NOT NULL,
    sample_count integer NOT NULL,
    rx_bytes_sum numeric(39,0) NOT NULL,
    tx_bytes_sum numeric(39,0) NOT NULL,
    latest_observed_at timestamp with time zone NOT NULL,
    rx_usage_bytes bigint NOT NULL,
    tx_usage_bytes bigint NOT NULL,
    rx_valid_count integer DEFAULT 0 NOT NULL,
    tx_valid_count integer DEFAULT 0 NOT NULL,
    any_valid_count integer DEFAULT 0 NOT NULL,
    rx_reset_count integer NOT NULL,
    tx_reset_count integer NOT NULL,
    any_reset_count integer DEFAULT 0 NOT NULL,
    usage_authoritative boolean NOT NULL,
    updated_at timestamp with time zone NOT NULL,
    CONSTRAINT traffic_counter_samples_interface_check CHECK (length(interface) BETWEEN 1 AND 128),
    CONSTRAINT traffic_counter_samples_observed_at_check CHECK (observed_at = date_trunc('minute', observed_at)),
    CONSTRAINT traffic_counter_samples_values_check CHECK (
        rx_bytes >= 0 AND tx_bytes >= 0
        AND rx_counter_epoch >= 0 AND tx_counter_epoch >= 0
        AND sample_count > 0
        AND rx_bytes_sum >= 0 AND tx_bytes_sum >= 0
        AND rx_usage_bytes >= 0 AND tx_usage_bytes >= 0
        AND rx_valid_count >= 0 AND tx_valid_count >= 0
        AND any_valid_count >= 0
        AND rx_reset_count >= 0 AND tx_reset_count >= 0
        AND any_reset_count >= 0
    ),
    CONSTRAINT traffic_counter_samples_latest_check CHECK (
        latest_observed_at >= observed_at
        AND latest_observed_at < observed_at + interval '1 minute'
        AND latest_observed_at = date_trunc('second', latest_observed_at)
    ),
    CONSTRAINT traffic_counter_samples_source_kind_check CHECK (source_kind = ANY (ARRAY['host'::text, 'tunnel'::text])),
    CONSTRAINT traffic_counter_samples_pkey PRIMARY KEY (client_id, source_kind, interface, observed_at),
    CONSTRAINT traffic_counter_samples_client_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_network_rates (
    client_id text NOT NULL,
    interface text NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    bucket_secs integer NOT NULL,
    sample_count integer NOT NULL,
    rx_bytes_sum numeric(39,0) DEFAULT 0 NOT NULL,
    tx_bytes_sum numeric(39,0) DEFAULT 0 NOT NULL,
    rx_bytes_avg bigint NOT NULL,
    tx_bytes_avg bigint NOT NULL,
    rx_bytes_last bigint NOT NULL,
    tx_bytes_last bigint NOT NULL,
    rx_counter_epoch bigint DEFAULT 0 NOT NULL,
    tx_counter_epoch bigint DEFAULT 0 NOT NULL,
    latest_observed_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT telemetry_network_rates_bucket_secs_check CHECK (
        bucket_secs = ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])
    ),
    CONSTRAINT telemetry_network_rates_bucket_start_check CHECK (
        bucket_start = date_trunc('minute', bucket_start)
        AND mod(extract(epoch FROM bucket_start)::bigint, bucket_secs) = 0
    ),
    CONSTRAINT telemetry_network_rates_check CHECK (((rx_bytes_avg >= 0) AND (tx_bytes_avg >= 0))),
    CONSTRAINT telemetry_network_rates_check1 CHECK (((rx_bytes_last >= 0) AND (tx_bytes_last >= 0))),
    CONSTRAINT telemetry_network_rates_check2 CHECK (((rx_counter_epoch >= 0) AND (tx_counter_epoch >= 0))),
    CONSTRAINT telemetry_network_rates_check3 CHECK (((latest_observed_at >= bucket_start) AND (latest_observed_at < (bucket_start + make_interval(secs => (bucket_secs)::double precision))))),
    CONSTRAINT telemetry_network_rates_latest_observed_second_check CHECK (
        latest_observed_at = date_trunc('second', latest_observed_at)
    ),
    CONSTRAINT telemetry_network_rates_sample_count_check CHECK ((sample_count > 0)),
    CONSTRAINT telemetry_network_rates_pkey PRIMARY KEY (bucket_secs, bucket_start, client_id, interface),
    CONSTRAINT telemetry_network_rates_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
) PARTITION BY LIST (bucket_secs);



CREATE TABLE public.telemetry_network_rates_minute
PARTITION OF public.telemetry_network_rates
FOR VALUES IN (60);



CREATE TABLE public.telemetry_network_rates_coarse
PARTITION OF public.telemetry_network_rates
FOR VALUES IN (300, 1800, 3600, 10800, 21600, 86400);



CREATE TABLE public.telemetry_ping_series (
    id bigint GENERATED ALWAYS AS IDENTITY (
        SEQUENCE NAME public.telemetry_ping_series_id_seq
        START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1
    ) NOT NULL,
    client_id text NOT NULL,
    target_id uuid NOT NULL,
    generation bigint NOT NULL,
    CONSTRAINT telemetry_ping_series_generation_check CHECK ((generation > 0)),
    CONSTRAINT telemetry_ping_series_client_id_target_id_generation_key UNIQUE (client_id, target_id, generation),
    CONSTRAINT telemetry_ping_series_pkey PRIMARY KEY (id),
    CONSTRAINT telemetry_ping_series_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT telemetry_ping_series_target_id_fkey FOREIGN KEY (target_id) REFERENCES public.ping_targets(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_ping_current (
    series_id bigint NOT NULL,
    latest_status text NOT NULL,
    latency_avg_ms double precision,
    rolling_loss_ratio double precision NOT NULL,
    latest_reason text,
    latest_checked_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT telemetry_ping_current_latency_avg_ms_check CHECK (((latency_avg_ms IS NULL) OR ((latency_avg_ms >= (0)::double precision) AND (latency_avg_ms <= (3600000)::double precision)))),
    CONSTRAINT telemetry_ping_current_latest_reason_check CHECK (((latest_reason IS NULL) OR (length(latest_reason) <= 512))),
    CONSTRAINT telemetry_ping_current_latest_status_check CHECK ((latest_status = ANY (ARRAY['ok'::text, 'degraded'::text, 'down'::text, 'error'::text]))),
    CONSTRAINT telemetry_ping_current_rolling_loss_ratio_check CHECK (((rolling_loss_ratio >= (0)::double precision) AND (rolling_loss_ratio <= (1)::double precision))),
    CONSTRAINT telemetry_ping_current_pkey PRIMARY KEY (series_id),
    CONSTRAINT telemetry_ping_current_series_id_fkey FOREIGN KEY (series_id) REFERENCES public.telemetry_ping_series(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_ping_facts (
    series_id bigint NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    evidence_id uuid NOT NULL,
    source_checked_unix bigint NOT NULL,
    checked_unix bigint NOT NULL,
    status text NOT NULL,
    latency_avg_ms double precision,
    loss_ratio double precision NOT NULL,
    reason text,
    CONSTRAINT telemetry_ping_facts_check CHECK ((checked_unix <= ((EXTRACT(epoch FROM observed_at))::bigint + 300))),
    CONSTRAINT telemetry_ping_facts_check1 CHECK ((((EXTRACT(epoch FROM observed_at))::bigint - checked_unix) <= 3900)),
    CONSTRAINT telemetry_ping_facts_check2 CHECK ((((status = 'ok'::text) AND (latency_avg_ms IS NOT NULL) AND (loss_ratio = (0)::double precision)) OR ((status = 'degraded'::text) AND (latency_avg_ms IS NOT NULL) AND (loss_ratio > (0)::double precision) AND (loss_ratio < (1)::double precision)) OR ((status = ANY (ARRAY['down'::text, 'error'::text])) AND (latency_avg_ms IS NULL) AND (loss_ratio = (1)::double precision)))),
    CONSTRAINT telemetry_ping_facts_checked_unix_check CHECK ((checked_unix > 0)),
    CONSTRAINT telemetry_ping_facts_latency_avg_ms_check CHECK (((latency_avg_ms IS NULL) OR ((latency_avg_ms >= (0)::double precision) AND (latency_avg_ms <= (3600000)::double precision)))),
    CONSTRAINT telemetry_ping_facts_loss_ratio_check CHECK (((loss_ratio >= (0)::double precision) AND (loss_ratio <= (1)::double precision))),
    CONSTRAINT telemetry_ping_facts_reason_check CHECK (((reason IS NULL) OR (length(reason) <= 4096))),
    CONSTRAINT telemetry_ping_facts_source_checked_unix_check CHECK ((source_checked_unix > 0)),
    CONSTRAINT telemetry_ping_facts_status_check CHECK ((status = ANY (ARRAY['ok'::text, 'degraded'::text, 'down'::text, 'error'::text]))),
    CONSTRAINT telemetry_ping_facts_pkey PRIMARY KEY (series_id, source_checked_unix),
    CONSTRAINT telemetry_ping_facts_series_id_fkey FOREIGN KEY (series_id) REFERENCES public.telemetry_ping_series(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_ping_rollups (
    series_id bigint NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    bucket_secs integer NOT NULL,
    sample_count integer NOT NULL,
    success_count integer NOT NULL,
    latency_sum_ms double precision DEFAULT 0 NOT NULL,
    latency_avg_ms double precision,
    latency_min_ms double precision,
    latency_max_ms double precision,
    loss_ratio_avg double precision NOT NULL,
    loss_ratio_sum double precision DEFAULT 0 NOT NULL,
    loss_ratio_max double precision NOT NULL,
    latest_status text NOT NULL,
    latest_reason text,
    latest_checked_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT telemetry_ping_rollups_bucket_secs_check CHECK (
        bucket_secs = ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])
    ),
    CONSTRAINT telemetry_ping_rollups_bucket_start_check CHECK (
        bucket_start = date_trunc('minute', bucket_start)
        AND mod(extract(epoch FROM bucket_start)::bigint, bucket_secs) = 0
    ),
    CONSTRAINT telemetry_ping_rollups_check CHECK (((success_count >= 0) AND (success_count <= sample_count))),
    CONSTRAINT telemetry_ping_rollups_check1 CHECK (((latest_checked_at >= bucket_start) AND (latest_checked_at < (bucket_start + make_interval(secs => (bucket_secs)::double precision))))),
    CONSTRAINT telemetry_ping_rollups_latency_avg_ms_check CHECK (((latency_avg_ms IS NULL) OR (latency_avg_ms >= (0)::double precision))),
    CONSTRAINT telemetry_ping_rollups_latency_max_ms_check CHECK (((latency_max_ms IS NULL) OR (latency_max_ms >= (0)::double precision))),
    CONSTRAINT telemetry_ping_rollups_latency_min_ms_check CHECK (((latency_min_ms IS NULL) OR (latency_min_ms >= (0)::double precision))),
    CONSTRAINT telemetry_ping_rollups_latest_reason_check CHECK (((latest_reason IS NULL) OR (length(latest_reason) <= 512))),
    CONSTRAINT telemetry_ping_rollups_latest_status_check CHECK ((latest_status = ANY (ARRAY['ok'::text, 'degraded'::text, 'down'::text, 'error'::text]))),
    CONSTRAINT telemetry_ping_rollups_loss_ratio_avg_check CHECK (((loss_ratio_avg >= (0)::double precision) AND (loss_ratio_avg <= (1)::double precision))),
    CONSTRAINT telemetry_ping_rollups_loss_ratio_max_check CHECK (((loss_ratio_max >= (0)::double precision) AND (loss_ratio_max <= (1)::double precision))),
    CONSTRAINT telemetry_ping_rollups_sample_count_check CHECK ((sample_count > 0)),
    CONSTRAINT telemetry_ping_rollups_pkey PRIMARY KEY (bucket_secs, bucket_start, series_id),
    CONSTRAINT telemetry_ping_rollups_series_id_fkey FOREIGN KEY (series_id) REFERENCES public.telemetry_ping_series(id) ON DELETE CASCADE
);





CREATE TABLE public.telemetry_projection_heads (
    client_id text NOT NULL,
    accepted_seq bigint NOT NULL,
    projected_seq bigint NOT NULL,
    latest_projected_sample_id uuid,
    published_generation bigint DEFAULT 0 NOT NULL,
    -- Cursor of the normalized traffic-minute snapshot underlying the compact
    -- policy frontier.  It is read only through this row's client primary key;
    -- no independent lookup index is useful.
    policy_traffic_materialized_seq bigint DEFAULT 0 NOT NULL,
    -- Sorted per-stream counter/usage frontier for samples after the cursor
    -- above.  This is projection state, not a second normalized traffic owner.
    policy_traffic_frontier jsonb,
    accepted_at timestamp with time zone NOT NULL,
    projected_at timestamp with time zone,
    projection_retry_at timestamp with time zone,
    projection_attempts integer DEFAULT 0 NOT NULL,
    projection_error text,
    CONSTRAINT telemetry_projection_heads_attempts_nonnegative CHECK ((projection_attempts >= 0)),
    CONSTRAINT telemetry_projection_heads_cursor_order CHECK (((accepted_seq >= 0) AND (projected_seq >= 0) AND (projected_seq <= accepted_seq) AND (published_generation >= 0) AND (policy_traffic_materialized_seq >= 0) AND (policy_traffic_materialized_seq <= projected_seq))),
    CONSTRAINT telemetry_projection_heads_policy_traffic_frontier_check CHECK ((policy_traffic_frontier IS NULL) OR (jsonb_typeof(policy_traffic_frontier) = 'array'::text)),
    CONSTRAINT telemetry_projection_heads_pkey PRIMARY KEY (client_id),
    CONSTRAINT telemetry_projection_heads_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



-- The cursor is a consumer commit boundary: every projected raw row through
-- materialized_seq is present in all core minute owners, and no still-open
-- natural minute may be crossed.
CREATE TABLE public.telemetry_minute_materialization_heads (
    client_id text NOT NULL,
    materialized_seq bigint DEFAULT 0 NOT NULL,
    materialized_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT telemetry_minute_materialization_heads_seq_check CHECK (
        materialized_seq >= 0
    ),
    CONSTRAINT telemetry_minute_materialization_heads_stamp_check CHECK (
        (materialized_seq = 0) = (materialized_at IS NULL)
    ),
    CONSTRAINT telemetry_minute_materialization_heads_pkey PRIMARY KEY (client_id),
    CONSTRAINT telemetry_minute_materialization_heads_client_id_fkey
        FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



-- Traffic has an independent consumer cursor because counter reconstruction
-- may complete separately from resource and Ping materialization.
CREATE TABLE public.traffic_counter_minute_heads (
    client_id text NOT NULL,
    materialized_seq bigint DEFAULT 0 NOT NULL,
    materialized_at timestamp with time zone,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT traffic_counter_minute_heads_seq_check CHECK (
        materialized_seq >= 0
    ),
    CONSTRAINT traffic_counter_minute_heads_stamp_check CHECK (
        (materialized_seq = 0) = (materialized_at IS NULL)
    ),
    CONSTRAINT traffic_counter_minute_heads_pkey PRIMARY KEY (client_id),
    CONSTRAINT traffic_counter_minute_heads_client_id_fkey
        FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_webhook_cursors (
    client_id text NOT NULL,
    last_sample_seq bigint DEFAULT 0 NOT NULL,
    CONSTRAINT telemetry_webhook_cursors_last_sample_seq_check CHECK (last_sample_seq >= 0),
    CONSTRAINT telemetry_webhook_cursors_pkey PRIMARY KEY (client_id),
    CONSTRAINT telemetry_webhook_cursors_client_id_fkey FOREIGN KEY (client_id)
        REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.telemetry_rollups (
    client_id text NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    bucket_secs integer NOT NULL,
    sample_count integer NOT NULL,
    cpu_usage_sample_count integer DEFAULT 0 NOT NULL,
    cpu_usage_sum double precision DEFAULT 0 NOT NULL,
    cpu_usage_avg double precision,
    cpu_usage_max double precision,
    cpu_cores_max integer DEFAULT 0 NOT NULL,
    cpu_load_1_avg double precision NOT NULL,
    cpu_load_1_sum double precision DEFAULT 0 NOT NULL,
    cpu_load_1_max double precision NOT NULL,
    cpu_load_5_avg double precision DEFAULT 0 NOT NULL,
    cpu_load_5_sum double precision DEFAULT 0 NOT NULL,
    cpu_load_5_max double precision DEFAULT 0 NOT NULL,
    cpu_load_15_avg double precision DEFAULT 0 NOT NULL,
    cpu_load_15_sum double precision DEFAULT 0 NOT NULL,
    cpu_load_15_max double precision DEFAULT 0 NOT NULL,
    memory_total_bytes_max bigint NOT NULL,
    memory_available_bytes_avg bigint NOT NULL,
    memory_available_bytes_sum numeric(39,0) DEFAULT 0 NOT NULL,
    memory_available_bytes_min bigint NOT NULL,
    memory_used_ratio_avg double precision NOT NULL,
    memory_used_ratio_sum double precision DEFAULT 0 NOT NULL,
    memory_used_ratio_max double precision NOT NULL,
    swap_sample_count integer DEFAULT 0 NOT NULL,
    swap_total_bytes_max bigint,
    swap_available_bytes_avg bigint,
    swap_available_bytes_sum numeric(39,0) DEFAULT 0 NOT NULL,
    swap_available_bytes_min bigint,
    swap_used_ratio_avg double precision,
    swap_used_ratio_sum double precision DEFAULT 0 NOT NULL,
    swap_used_ratio_max double precision,
    disk_total_bytes_max bigint DEFAULT 0 NOT NULL,
    disk_available_bytes_avg bigint DEFAULT 0 NOT NULL,
    disk_available_bytes_sum numeric(39,0) DEFAULT 0 NOT NULL,
    disk_available_bytes_min bigint DEFAULT 0 NOT NULL,
    disk_used_ratio_avg double precision DEFAULT 0 NOT NULL,
    disk_used_ratio_sum double precision DEFAULT 0 NOT NULL,
    disk_used_ratio_max double precision DEFAULT 0 NOT NULL,
    connections_sample_count integer DEFAULT 0 NOT NULL,
    tcp_sockets_latest bigint,
    udp_sockets_latest bigint,
    connections_observed_at timestamp with time zone,
    latest_observed_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    disk_sample_count integer DEFAULT 0 NOT NULL,
    CONSTRAINT telemetry_rollups_bucket_secs_check CHECK (
        bucket_secs = ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])
    ),
    CONSTRAINT telemetry_rollups_bucket_start_check CHECK (
        bucket_start = date_trunc('minute', bucket_start)
        AND mod(extract(epoch FROM bucket_start)::bigint, bucket_secs) = 0
    ),
    CONSTRAINT telemetry_rollups_check CHECK (((cpu_usage_sample_count >= 0) AND (cpu_usage_sample_count <= sample_count))),
    CONSTRAINT telemetry_rollups_check1 CHECK (((swap_sample_count >= 0) AND (swap_sample_count <= sample_count))),
    CONSTRAINT telemetry_rollups_check10 CHECK (((disk_sample_count >= 0) AND (disk_sample_count <= sample_count))),
    CONSTRAINT telemetry_rollups_check2 CHECK (((((swap_sample_count = 0) AND (((swap_total_bytes_max IS NULL) AND (swap_available_bytes_avg IS NULL) AND (swap_available_bytes_min IS NULL)) OR ((swap_total_bytes_max = 0) AND (swap_available_bytes_avg = 0) AND (swap_available_bytes_min = 0))) AND (swap_used_ratio_avg IS NULL) AND (swap_used_ratio_max IS NULL)) OR ((swap_sample_count > 0) AND (swap_total_bytes_max > 0) AND (swap_available_bytes_avg IS NOT NULL) AND (swap_available_bytes_min IS NOT NULL) AND (swap_used_ratio_avg IS NOT NULL) AND (swap_used_ratio_max IS NOT NULL))) IS TRUE)),
    CONSTRAINT telemetry_rollups_check3 CHECK (((swap_total_bytes_max IS NULL) OR ((swap_total_bytes_max >= 0) AND (swap_available_bytes_avg >= 0) AND (swap_available_bytes_min >= 0) AND (swap_available_bytes_min <= swap_available_bytes_avg) AND (swap_available_bytes_avg <= swap_total_bytes_max)))),
    CONSTRAINT telemetry_rollups_check4 CHECK (((connections_sample_count >= 0) AND (connections_sample_count <= sample_count))),
    CONSTRAINT telemetry_rollups_check5 CHECK (((connections_sample_count = 0) = (connections_observed_at IS NULL))),
    CONSTRAINT telemetry_rollups_check6 CHECK (((connections_observed_at IS NULL) OR ((connections_observed_at >= bucket_start) AND (connections_observed_at < (bucket_start + make_interval(secs => (bucket_secs)::double precision)))))),
    CONSTRAINT telemetry_rollups_check7 CHECK (((latest_observed_at >= bucket_start) AND (latest_observed_at < (bucket_start + make_interval(secs => (bucket_secs)::double precision))))),
    CONSTRAINT telemetry_rollups_check8 CHECK (((connections_sample_count = 0) = (tcp_sockets_latest IS NULL))),
    CONSTRAINT telemetry_rollups_check9 CHECK (((tcp_sockets_latest IS NULL) = (udp_sockets_latest IS NULL))),
    CONSTRAINT telemetry_rollups_cpu_cores_max_check CHECK ((cpu_cores_max >= 0)),
    CONSTRAINT telemetry_rollups_cpu_usage_avg_check CHECK (((cpu_usage_avg IS NULL) OR ((cpu_usage_avg >= (0)::double precision) AND (cpu_usage_avg <= (1)::double precision)))),
    CONSTRAINT telemetry_rollups_cpu_usage_max_check CHECK (((cpu_usage_max IS NULL) OR ((cpu_usage_max >= (0)::double precision) AND (cpu_usage_max <= (1)::double precision)))),
    CONSTRAINT telemetry_rollups_disk_used_ratio_avg_check CHECK (((disk_used_ratio_avg >= (0)::double precision) AND (disk_used_ratio_avg <= (1)::double precision))),
    CONSTRAINT telemetry_rollups_disk_used_ratio_max_check CHECK (((disk_used_ratio_max >= (0)::double precision) AND (disk_used_ratio_max <= (1)::double precision))),
    CONSTRAINT telemetry_rollups_memory_used_ratio_avg_check CHECK (((memory_used_ratio_avg >= (0)::double precision) AND (memory_used_ratio_avg <= (1)::double precision))),
    CONSTRAINT telemetry_rollups_memory_used_ratio_max_check CHECK (((memory_used_ratio_max >= (0)::double precision) AND (memory_used_ratio_max <= (1)::double precision))),
    CONSTRAINT telemetry_rollups_sample_count_check CHECK ((sample_count > 0)),
    CONSTRAINT telemetry_rollups_swap_used_ratio_avg_check CHECK (((swap_used_ratio_avg IS NULL) OR ((swap_used_ratio_avg >= (0)::double precision) AND (swap_used_ratio_avg <= (1)::double precision)))),
    CONSTRAINT telemetry_rollups_swap_used_ratio_max_check CHECK (((swap_used_ratio_max IS NULL) OR ((swap_used_ratio_max >= (0)::double precision) AND (swap_used_ratio_max <= (1)::double precision)))),
    CONSTRAINT telemetry_rollups_tcp_sockets_latest_check CHECK (((tcp_sockets_latest IS NULL) OR (tcp_sockets_latest >= 0))),
    CONSTRAINT telemetry_rollups_udp_sockets_latest_check CHECK (((udp_sockets_latest IS NULL) OR (udp_sockets_latest >= 0))),
    CONSTRAINT telemetry_rollups_pkey PRIMARY KEY (bucket_secs, bucket_start, client_id),
    CONSTRAINT telemetry_rollups_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);


CREATE TABLE public.telemetry_samples (
    id uuid NOT NULL,
    client_id text NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    cpu_utilization_ratio double precision,
    cpu_cores integer NOT NULL,
    cpu_load_1 double precision NOT NULL,
    cpu_load_5 double precision NOT NULL,
    cpu_load_15 double precision NOT NULL,
    memory_total_bytes bigint NOT NULL,
    memory_available_bytes bigint NOT NULL,
    swap_total_bytes bigint,
    swap_available_bytes bigint,
    disk_total_bytes bigint,
    disk_available_bytes bigint,
    tcp_sockets bigint NOT NULL,
    udp_sockets bigint NOT NULL,
    payload jsonb NOT NULL,
    accepted_seq bigint NOT NULL,
    accepted_at timestamp with time zone NOT NULL,
    source_gateway_id text NOT NULL,
    source_gateway_session_id uuid NOT NULL,
    source_process_incarnation_id uuid NOT NULL,
    source_telemetry_seq bigint NOT NULL,
    reported_observed_unix bigint NOT NULL,
    ping_source_checked_unix bigint[] DEFAULT ARRAY[]::bigint[] NOT NULL,
    network_admission_mask bytea DEFAULT '\x'::bytea NOT NULL,
    tunnel_admission_mask bytea DEFAULT '\x'::bytea NOT NULL,
    CONSTRAINT telemetry_samples_check CHECK ((((swap_total_bytes IS NULL) AND (swap_available_bytes IS NULL)) OR ((swap_total_bytes >= 0) AND ((swap_available_bytes >= 0) AND (swap_available_bytes <= swap_total_bytes))))),
    CONSTRAINT telemetry_samples_cpu_cores_check CHECK ((cpu_cores >= 0)),
    CONSTRAINT telemetry_samples_cpu_load_15_check CHECK ((cpu_load_15 >= (0)::double precision)),
    CONSTRAINT telemetry_samples_cpu_load_1_check CHECK ((cpu_load_1 >= (0)::double precision)),
    CONSTRAINT telemetry_samples_cpu_load_5_check CHECK ((cpu_load_5 >= (0)::double precision)),
    CONSTRAINT telemetry_samples_cpu_utilization_ratio_check CHECK (((cpu_utilization_ratio IS NULL) OR ((cpu_utilization_ratio >= (0)::double precision) AND (cpu_utilization_ratio <= (1)::double precision)))),
    CONSTRAINT telemetry_samples_disk_available_bytes_check CHECK ((disk_available_bytes >= 0)),
    CONSTRAINT telemetry_samples_disk_total_bytes_check CHECK ((disk_total_bytes >= 0)),
    CONSTRAINT telemetry_samples_memory_available_bytes_check CHECK ((memory_available_bytes >= 0)),
    CONSTRAINT telemetry_samples_memory_total_bytes_check CHECK ((memory_total_bytes >= 0)),
    CONSTRAINT telemetry_samples_payload_check CHECK ((jsonb_typeof(payload) = 'object'::text)),
    CONSTRAINT telemetry_samples_tcp_sockets_check CHECK ((tcp_sockets >= 0)),
    CONSTRAINT telemetry_samples_udp_sockets_check CHECK ((udp_sockets >= 0)),
    CONSTRAINT telemetry_samples_ping_source_checked_nonnegative CHECK (((array_position(ping_source_checked_unix, NULL::bigint) IS NULL) AND (0 <= ALL (ping_source_checked_unix)))),
    CONSTRAINT telemetry_samples_pkey PRIMARY KEY (id),
    CONSTRAINT telemetry_samples_source_metadata_check CHECK (((accepted_seq > 0) AND ((length(btrim(source_gateway_id)) >= 1) AND (length(btrim(source_gateway_id)) <= 128)) AND (source_telemetry_seq >= 0) AND (reported_observed_unix >= 0))),
    CONSTRAINT telemetry_samples_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



-- This suffix is the resource-minute consumer ownership boundary. A raw row
-- appears here until and only until that consumer commits both derived minutes
-- and the matching cursor advance.
CREATE VIEW public.telemetry_projected_raw_core_suffix AS
SELECT sample.*
FROM public.telemetry_minute_materialization_heads minute
JOIN public.telemetry_projection_heads projection USING (client_id)
JOIN public.telemetry_samples sample
  ON sample.client_id = minute.client_id
 AND sample.accepted_seq > minute.materialized_seq
 AND sample.accepted_seq <= projection.projected_seq;
CREATE FUNCTION public.telemetry_projected_raw_resource_minutes_source(
    p_client_ids TEXT[]
)
RETURNS SETOF public.telemetry_rollups
LANGUAGE sql
STABLE
AS $$
-- The requested head relation is the explicit raw-suffix ownership boundary.
-- A non-NULL array reaches the client primary keys before touched-minute
-- discovery, independent of whether PostgreSQL uses a custom or generic plan.
WITH requested_clients AS MATERIALIZED (
    SELECT DISTINCT requested.client_id
    FROM unnest(p_client_ids) requested(client_id)
    WHERE p_client_ids IS NOT NULL
), requested_heads AS MATERIALIZED (
    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM public.telemetry_minute_materialization_heads minute
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NULL
    UNION ALL
    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM requested_clients requested
    JOIN public.telemetry_minute_materialization_heads minute USING (client_id)
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NOT NULL
), raw_samples AS NOT MATERIALIZED (
    -- NULL is the genuinely all-client relation: keep it setwise so the public
    -- wrapper does not turn a fleet read into one index probe per head.
    SELECT sample.*
    FROM requested_heads head
    JOIN public.telemetry_samples sample
      ON sample.client_id = head.client_id
     AND sample.accepted_seq > head.materialized_seq
     AND sample.accepted_seq <= head.projected_seq
    WHERE p_client_ids IS NULL

    UNION ALL

    -- Exact consumers must bind the owner before touching the journal.  The
    -- subquery boundary is intentional: a forced generic plan otherwise chose
    -- two fleet-wide sample scans even when only four owners were requested.
    SELECT sample.*
    FROM requested_heads head
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = head.client_id
          AND sample.accepted_seq > head.materialized_seq
          AND sample.accepted_seq <= head.projected_seq
        OFFSET 0
    ) sample
    WHERE p_client_ids IS NOT NULL
), touched AS NOT MATERIALIZED (
    SELECT DISTINCT sample.client_id,
           date_trunc('minute', sample.observed_at) AS bucket_start
    FROM raw_samples sample
), minute_samples AS NOT MATERIALIZED (
    SELECT sample.*, touched.bucket_start
    FROM touched
    JOIN requested_heads head USING (client_id)
    JOIN public.telemetry_samples sample
      ON sample.client_id = touched.client_id
     AND sample.observed_at >= touched.bucket_start
     AND sample.observed_at < touched.bucket_start + interval '1 minute'
     AND sample.accepted_seq <= head.projected_seq
    WHERE p_client_ids IS NULL

    UNION ALL

    SELECT sample.*, touched.bucket_start
    FROM touched
    JOIN requested_heads head USING (client_id)
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = touched.client_id
          AND sample.observed_at >= touched.bucket_start
          AND sample.observed_at < touched.bucket_start + interval '1 minute'
          AND sample.accepted_seq <= head.projected_seq
        OFFSET 0
    ) sample
    WHERE p_client_ids IS NOT NULL
), valued AS NOT MATERIALIZED (
    SELECT
        sample.*,
        CASE WHEN sample.memory_total_bytes > 0 THEN
            (sample.memory_total_bytes - sample.memory_available_bytes)::DOUBLE PRECISION
                / sample.memory_total_bytes::DOUBLE PRECISION
        ELSE 0::DOUBLE PRECISION END AS memory_used_ratio,
        CASE WHEN sample.swap_total_bytes > 0 THEN
            (sample.swap_total_bytes - sample.swap_available_bytes)::DOUBLE PRECISION
                / sample.swap_total_bytes::DOUBLE PRECISION
        END AS swap_used_ratio,
        CASE WHEN sample.disk_total_bytes > 0 THEN
            (sample.disk_total_bytes - sample.disk_available_bytes)::DOUBLE PRECISION
                / sample.disk_total_bytes::DOUBLE PRECISION
        ELSE 0::DOUBLE PRECISION END AS disk_used_ratio,
        jsonb_typeof(sample.payload -> 'connections') = 'object'
            AS connections_present
    FROM minute_samples sample
)
SELECT
    client_id,
    bucket_start,
    60::INTEGER AS bucket_secs,
    count(*)::INTEGER AS sample_count,
    count(cpu_utilization_ratio)::INTEGER AS cpu_usage_sample_count,
    COALESCE(sum(cpu_utilization_ratio), 0)::DOUBLE PRECISION AS cpu_usage_sum,
    avg(cpu_utilization_ratio)::DOUBLE PRECISION AS cpu_usage_avg,
    max(cpu_utilization_ratio)::DOUBLE PRECISION AS cpu_usage_max,
    max(cpu_cores)::INTEGER AS cpu_cores_max,
    avg(cpu_load_1)::DOUBLE PRECISION AS cpu_load_1_avg,
    sum(cpu_load_1)::DOUBLE PRECISION AS cpu_load_1_sum,
    max(cpu_load_1)::DOUBLE PRECISION AS cpu_load_1_max,
    avg(cpu_load_5)::DOUBLE PRECISION AS cpu_load_5_avg,
    sum(cpu_load_5)::DOUBLE PRECISION AS cpu_load_5_sum,
    max(cpu_load_5)::DOUBLE PRECISION AS cpu_load_5_max,
    avg(cpu_load_15)::DOUBLE PRECISION AS cpu_load_15_avg,
    sum(cpu_load_15)::DOUBLE PRECISION AS cpu_load_15_sum,
    max(cpu_load_15)::DOUBLE PRECISION AS cpu_load_15_max,
    max(memory_total_bytes)::BIGINT AS memory_total_bytes_max,
    round(avg(memory_available_bytes::NUMERIC))::BIGINT
        AS memory_available_bytes_avg,
    sum(memory_available_bytes::NUMERIC)::NUMERIC(39,0)
        AS memory_available_bytes_sum,
    min(memory_available_bytes)::BIGINT AS memory_available_bytes_min,
    avg(memory_used_ratio)::DOUBLE PRECISION AS memory_used_ratio_avg,
    sum(memory_used_ratio)::DOUBLE PRECISION AS memory_used_ratio_sum,
    max(memory_used_ratio)::DOUBLE PRECISION AS memory_used_ratio_max,
    count(*) FILTER (WHERE swap_total_bytes > 0)::INTEGER
        AS swap_sample_count,
    max(swap_total_bytes)::BIGINT AS swap_total_bytes_max,
    CASE
        WHEN count(*) FILTER (WHERE swap_total_bytes > 0) > 0
        THEN round(avg(swap_available_bytes::NUMERIC)
            FILTER (WHERE swap_total_bytes > 0))::BIGINT
        WHEN count(swap_total_bytes) > 0 THEN 0::BIGINT
    END AS swap_available_bytes_avg,
    COALESCE(sum(swap_available_bytes::NUMERIC)
        FILTER (WHERE swap_total_bytes > 0), 0)::NUMERIC(39,0)
        AS swap_available_bytes_sum,
    CASE
        WHEN count(*) FILTER (WHERE swap_total_bytes > 0) > 0
        THEN min(swap_available_bytes) FILTER (WHERE swap_total_bytes > 0)
        WHEN count(swap_total_bytes) > 0 THEN 0::BIGINT
    END AS swap_available_bytes_min,
    avg(swap_used_ratio)::DOUBLE PRECISION AS swap_used_ratio_avg,
    COALESCE(sum(swap_used_ratio), 0)::DOUBLE PRECISION AS swap_used_ratio_sum,
    max(swap_used_ratio)::DOUBLE PRECISION AS swap_used_ratio_max,
    COALESCE(max(disk_total_bytes) FILTER (WHERE disk_total_bytes > 0), 0)::BIGINT
        AS disk_total_bytes_max,
    COALESCE(round(avg(disk_available_bytes::NUMERIC)
        FILTER (WHERE disk_total_bytes > 0)), 0)::BIGINT
        AS disk_available_bytes_avg,
    COALESCE(sum(disk_available_bytes::NUMERIC)
        FILTER (WHERE disk_total_bytes > 0), 0)::NUMERIC(39,0)
        AS disk_available_bytes_sum,
    COALESCE(min(disk_available_bytes)
        FILTER (WHERE disk_total_bytes > 0), 0)::BIGINT
        AS disk_available_bytes_min,
    COALESCE(avg(disk_used_ratio)
        FILTER (WHERE disk_total_bytes > 0), 0)::DOUBLE PRECISION
        AS disk_used_ratio_avg,
    COALESCE(sum(disk_used_ratio)
        FILTER (WHERE disk_total_bytes > 0), 0)::DOUBLE PRECISION
        AS disk_used_ratio_sum,
    COALESCE(max(disk_used_ratio)
        FILTER (WHERE disk_total_bytes > 0), 0)::DOUBLE PRECISION
        AS disk_used_ratio_max,
    count(*) FILTER (WHERE connections_present)::INTEGER
        AS connections_sample_count,
    (array_agg(tcp_sockets ORDER BY observed_at DESC, accepted_seq DESC)
        FILTER (WHERE connections_present))[1] AS tcp_sockets_latest,
    (array_agg(udp_sockets ORDER BY observed_at DESC, accepted_seq DESC)
        FILTER (WHERE connections_present))[1] AS udp_sockets_latest,
    max(observed_at) FILTER (WHERE connections_present)
        AS connections_observed_at,
    max(observed_at) AS latest_observed_at,
    max(accepted_at) AS updated_at,
    count(*) FILTER (WHERE disk_total_bytes > 0)::INTEGER
        AS disk_sample_count
FROM valued
GROUP BY client_id, bucket_start
$$;
-- Exact consumers pass every independently known physical bound.  The NULL
-- defaults retain deliberate all-client setwise inspection; exact calls bind
-- each owner before retained or projected history is read.
CREATE FUNCTION public.telemetry_resource_points_source(
    p_client_ids TEXT[],
    p_min_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_max_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_bucket_secs INTEGER DEFAULT NULL,
    p_per_owner_limit BIGINT DEFAULT NULL
)
RETURNS SETOF public.telemetry_rollups
LANGUAGE sql
STABLE
AS $$
WITH requested_clients AS MATERIALIZED (
    SELECT DISTINCT requested.client_id
    FROM unnest(p_client_ids) requested(client_id)
    WHERE p_client_ids IS NOT NULL
), projected_suffix AS MATERIALIZED (
    SELECT suffix.*
    FROM public.telemetry_projected_raw_resource_minutes_source(p_client_ids)
        suffix
    WHERE suffix.bucket_start >= COALESCE(
              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
          )
      AND suffix.bucket_start <= COALESCE(
              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
          )
      AND (p_bucket_secs IS NULL OR suffix.bucket_secs = p_bucket_secs)
), all_client_points AS NOT MATERIALIZED (
    SELECT retained.*
    FROM public.telemetry_rollups retained
    WHERE p_client_ids IS NULL
      AND retained.bucket_start >= COALESCE(
              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
          )
      AND retained.bucket_start <= COALESCE(
              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
          )
      AND (p_bucket_secs IS NULL OR retained.bucket_secs = p_bucket_secs)
      AND NOT EXISTS (
          SELECT 1
          FROM projected_suffix suffix
          WHERE suffix.client_id = retained.client_id
            AND suffix.bucket_secs = retained.bucket_secs
            AND suffix.bucket_start = retained.bucket_start
      )

    UNION ALL

    SELECT suffix.*
    FROM projected_suffix suffix
    WHERE p_client_ids IS NULL
), exact_client_points AS NOT MATERIALIZED (
    SELECT point.*
    FROM requested_clients requested
    CROSS JOIN LATERAL (
        SELECT candidate.*
        FROM (
            (
                SELECT retained.*
                FROM public.telemetry_rollups retained
                WHERE retained.client_id = requested.client_id
                  AND retained.bucket_start >= COALESCE(
                          p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                      )
                  AND retained.bucket_start <= COALESCE(
                          p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                      )
                  AND (
                      p_bucket_secs IS NULL
                      OR retained.bucket_secs = p_bucket_secs
                  )
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix suffix
                      WHERE suffix.client_id = retained.client_id
                        AND suffix.bucket_secs = retained.bucket_secs
                        AND suffix.bucket_start = retained.bucket_start
                  )
                ORDER BY retained.bucket_start DESC,
                         retained.latest_observed_at DESC,
                         retained.bucket_secs ASC
                LIMIT p_per_owner_limit
                OFFSET 0
            )

            UNION ALL

            SELECT suffix.*
            FROM projected_suffix suffix
            WHERE suffix.client_id = requested.client_id
        ) candidate
        ORDER BY candidate.bucket_start DESC,
                 candidate.latest_observed_at DESC,
                 candidate.bucket_secs ASC
        LIMIT p_per_owner_limit
        OFFSET 0
    ) point
    WHERE p_client_ids IS NOT NULL
)
SELECT point.*
FROM all_client_points point
UNION ALL
SELECT point.*
FROM exact_client_points point
$$;
CREATE VIEW public.telemetry_projected_raw_ping_minutes AS
-- Ping follows the same read-owner rule as resource and network telemetry:
-- an exact series/range predicate must reach the open raw suffix before JSON
-- expansion. These CTEs are logical stages, not fleet-wide execution fences.
WITH expanded AS NOT MATERIALIZED (
    SELECT
        sample.id AS evidence_id,
        sample.client_id,
        sample.accepted_seq,
        sample.accepted_at,
        ping.ordinality,
        sample.ping_source_checked_unix[ping.ordinality] AS source_checked_unix,
        (ping.value ->> 'target_id')::UUID AS target_id,
        (ping.value ->> 'generation')::BIGINT AS generation,
        (ping.value ->> 'checked_unix')::BIGINT AS checked_unix,
        ping.value ->> 'status' AS status,
        (ping.value ->> 'latency_avg_ms')::DOUBLE PRECISION AS latency_avg_ms,
        (ping.value ->> 'loss_ratio')::DOUBLE PRECISION AS loss_ratio,
        ping.value ->> 'reason' AS reason
    FROM public.telemetry_projected_raw_core_suffix sample
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(sample.payload -> 'ping_results') = 'array'
            THEN sample.payload -> 'ping_results' ELSE '[]'::JSONB END
    ) WITH ORDINALITY ping(value, ordinality)
    WHERE ping.ordinality <= cardinality(sample.ping_source_checked_unix)
), raw_evidence AS NOT MATERIALIZED (
    SELECT
        series.id AS series_id,
        expanded.*
    FROM expanded
    JOIN public.telemetry_ping_series series
      ON series.client_id = expanded.client_id
     AND series.target_id = expanded.target_id
     AND series.generation = expanded.generation
    WHERE expanded.source_checked_unix > 0
      AND expanded.checked_unix > 0
), touched AS NOT MATERIALIZED (
    SELECT DISTINCT series_id, (checked_unix / 60) AS bucket_start_unix
    FROM raw_evidence
), evidence AS NOT MATERIALIZED (
    SELECT
        fact.series_id,
        fact.evidence_id,
        0::BIGINT AS accepted_seq,
        fact.observed_at AS accepted_at,
        0::BIGINT AS ordinality,
        fact.source_checked_unix,
        fact.checked_unix,
        fact.status,
        fact.latency_avg_ms,
        fact.loss_ratio,
        fact.reason,
        0::INTEGER AS source_priority
    FROM touched
    JOIN public.telemetry_ping_facts fact
      ON fact.series_id = touched.series_id
     AND fact.checked_unix >= touched.bucket_start_unix * 60
     AND fact.checked_unix < (touched.bucket_start_unix + 1) * 60
    UNION ALL
    SELECT
        raw.series_id,
        raw.evidence_id,
        raw.accepted_seq,
        raw.accepted_at,
        raw.ordinality,
        raw.source_checked_unix,
        raw.checked_unix,
        raw.status,
        raw.latency_avg_ms,
        raw.loss_ratio,
        raw.reason,
        1::INTEGER AS source_priority
    FROM raw_evidence raw
), canonical AS NOT MATERIALIZED (
    SELECT DISTINCT ON (series_id, source_checked_unix) *
    FROM evidence
    ORDER BY series_id, source_checked_unix,
             source_priority DESC, accepted_seq DESC, ordinality DESC
), grouped AS NOT MATERIALIZED (
    SELECT
        series_id,
        to_timestamp((checked_unix / 60) * 60) AS bucket_start,
        count(*)::INTEGER AS sample_count,
        count(latency_avg_ms)::INTEGER AS success_count,
        sum(COALESCE(latency_avg_ms, 0))::DOUBLE PRECISION AS latency_sum_ms,
        avg(latency_avg_ms)::DOUBLE PRECISION AS latency_avg_ms,
        min(latency_avg_ms)::DOUBLE PRECISION AS latency_min_ms,
        max(latency_avg_ms)::DOUBLE PRECISION AS latency_max_ms,
        avg(loss_ratio)::DOUBLE PRECISION AS loss_ratio_avg,
        sum(loss_ratio)::DOUBLE PRECISION AS loss_ratio_sum,
        max(loss_ratio)::DOUBLE PRECISION AS loss_ratio_max,
        (array_agg(status ORDER BY checked_unix DESC,
            source_checked_unix DESC, accepted_seq DESC))[1] AS latest_status,
        (array_agg(left(reason, 512) ORDER BY checked_unix DESC,
            source_checked_unix DESC, accepted_seq DESC))[1] AS latest_reason,
        to_timestamp(max(checked_unix)) AS latest_checked_at,
        max(accepted_at) AS updated_at
    FROM canonical
    GROUP BY series_id, (checked_unix / 60)
)
SELECT series_id, bucket_start, 60::INTEGER AS bucket_secs,
       sample_count, success_count, latency_sum_ms,
       latency_avg_ms, latency_min_ms, latency_max_ms,
       loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
       latest_status, latest_reason, latest_checked_at, updated_at
FROM grouped;



CREATE VIEW public.telemetry_ping_points AS
SELECT retained.*
FROM public.telemetry_ping_rollups retained
WHERE NOT EXISTS (
    SELECT 1
    FROM public.telemetry_projected_raw_ping_minutes suffix
    WHERE suffix.series_id = retained.series_id
      AND suffix.bucket_secs = retained.bucket_secs
      AND suffix.bucket_start = retained.bucket_start
)
UNION ALL
SELECT suffix.*
FROM public.telemetry_projected_raw_ping_minutes suffix;



CREATE FUNCTION public.telemetry_projected_raw_network_minutes_source(
    p_client_ids TEXT[]
)
RETURNS SETOF public.telemetry_network_rates
LANGUAGE sql
STABLE
AS $$
-- Resolve the exact consumer heads before discovering raw minutes.  The
-- all-client branch is reserved for the public relation wrapper; a non-NULL
-- request drives primary-key joins under both custom and generic plans.
WITH requested_clients AS MATERIALIZED (
    SELECT DISTINCT requested.client_id
    FROM unnest(p_client_ids) requested(client_id)
    WHERE p_client_ids IS NOT NULL
), requested_heads AS MATERIALIZED (
    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM public.traffic_counter_minute_heads minute
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NULL
    UNION ALL
    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM requested_clients requested
    JOIN public.traffic_counter_minute_heads minute USING (client_id)
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NOT NULL
), raw_samples AS NOT MATERIALIZED (
    -- Preserve a normal setwise scan for the NULL/all-client public relation.
    SELECT sample.*
    FROM requested_heads head
    JOIN public.telemetry_samples sample
      ON sample.client_id = head.client_id
     AND sample.accepted_seq > head.materialized_seq
     AND sample.accepted_seq <= head.projected_seq
    WHERE p_client_ids IS NULL

    UNION ALL

    -- Exact-owner calls use an explicit parameter-to-index boundary.  This is
    -- required by the measured forced-generic plan, not by a tiny-fixture plan:
    -- without it PostgreSQL scanned every unrelated journal row twice.
    SELECT sample.*
    FROM requested_heads head
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = head.client_id
          AND sample.accepted_seq > head.materialized_seq
          AND sample.accepted_seq <= head.projected_seq
        OFFSET 0
    ) sample
    WHERE p_client_ids IS NOT NULL
), raw_client_minutes AS MATERIALIZED (
    SELECT DISTINCT sample.client_id,
           date_trunc('minute', sample.observed_at) AS bucket_start
    FROM raw_samples sample
), minute_samples AS NOT MATERIALIZED (
    SELECT sample.*, raw_minute.bucket_start, head.materialized_seq
    FROM raw_client_minutes raw_minute
    JOIN requested_heads head USING (client_id)
    JOIN public.telemetry_samples sample
      ON sample.client_id = raw_minute.client_id
     AND sample.observed_at >= raw_minute.bucket_start
     AND sample.observed_at < raw_minute.bucket_start + interval '1 minute'
     AND sample.accepted_seq <= head.projected_seq
    WHERE p_client_ids IS NULL

    UNION ALL

    SELECT sample.*, raw_minute.bucket_start, head.materialized_seq
    FROM raw_client_minutes raw_minute
    JOIN requested_heads head USING (client_id)
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = raw_minute.client_id
          AND sample.observed_at >= raw_minute.bucket_start
          AND sample.observed_at < raw_minute.bucket_start + interval '1 minute'
          AND sample.accepted_seq <= head.projected_seq
        OFFSET 0
    ) sample
    WHERE p_client_ids IS NOT NULL
), expanded AS MATERIALIZED (
    -- Decode each projected payload once for every requested open client
    -- minute.  The raw marker preserves the original touched-interface rule:
    -- older projected samples contribute only when a still-unconsumed sample
    -- names the same interface in that natural minute.
    SELECT
        minute_sample.client_id,
        minute_sample.bucket_start,
        minute_sample.accepted_seq,
        minute_sample.accepted_at,
        minute_sample.observed_at,
        network.ordinality,
        network.value ->> 'interface' AS interface,
        public.telemetry_u64_counter_to_bigint(
            network.value ->> 'rx_bytes'
        ) AS rx_bytes,
        public.telemetry_u64_counter_to_bigint(
            network.value ->> 'tx_bytes'
        ) AS tx_bytes,
        minute_sample.accepted_seq > minute_sample.materialized_seq AS is_raw
    FROM minute_samples minute_sample
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(minute_sample.payload -> 'networks') = 'array'
            THEN minute_sample.payload -> 'networks' ELSE '[]'::JSONB END
    ) WITH ORDINALITY network(value, ordinality)
    WHERE CASE
          WHEN public.telemetry_ordinal_admission_mask_is_exact(
              minute_sample.network_admission_mask,
              CASE WHEN jsonb_typeof(minute_sample.payload -> 'networks') = 'array'
                  THEN jsonb_array_length(minute_sample.payload -> 'networks')::BIGINT
                  ELSE 0 END
          ) THEN get_bit(
              minute_sample.network_admission_mask,
              (network.ordinality - 1)::INTEGER
          ) = 1
          ELSE FALSE
      END
      AND octet_length(network.value ->> 'interface') BETWEEN 1 AND 128
), touched AS MATERIALIZED (
    SELECT DISTINCT client_id, interface, bucket_start
    FROM expanded
    WHERE is_raw
), touched_predecessors AS MATERIALIZED (
    -- Missing wall minutes do not own counter continuity.  Every touched raw
    -- coordinate resolves its latest durable coordinate after excluding rows
    -- shadowed by another touched raw minute.  Raw minutes with the same
    -- predecessor are therefore one stream even across a wall gap; an actual
    -- intervening durable/import observation is the only segment boundary.
    SELECT touched.*,
           predecessor.bucket_start AS durable_predecessor_bucket_start,
           predecessor.rx_bytes AS prior_rx_bytes,
           predecessor.tx_bytes AS prior_tx_bytes,
           COALESCE(predecessor.rx_counter_epoch, 0)
               AS prior_rx_counter_epoch,
           COALESCE(predecessor.tx_counter_epoch, 0)
               AS prior_tx_counter_epoch,
           predecessor.sample_source AS prior_sample_source
    FROM touched
    LEFT JOIN public.traffic_counter_streams stream
      ON stream.client_id = touched.client_id
     AND stream.source_kind = 'host'
     AND stream.interface = touched.interface
    LEFT JOIN LATERAL (
        SELECT candidate.bucket_start,
               candidate.rx_bytes, candidate.tx_bytes,
               candidate.rx_counter_epoch, candidate.tx_counter_epoch,
               candidate.sample_source
        FROM (
            -- The compact stream edge survives exact-minute promotion.  It is
            -- eligible only when its coordinate is not being recomputed by
            -- this raw suffix.
            SELECT date_trunc(
                       'minute', stream.latest_sample_observed_at
                   ) AS bucket_start,
                   stream.latest_sample_rx_bytes AS rx_bytes,
                   stream.latest_sample_tx_bytes AS tx_bytes,
                   stream.latest_sample_rx_counter_epoch AS rx_counter_epoch,
                   stream.latest_sample_tx_counter_epoch AS tx_counter_epoch,
                   stream.latest_sample_source AS sample_source,
                   1::INTEGER AS source_priority
            WHERE stream.latest_sample_observed_at < touched.bucket_start
              AND NOT EXISTS (
                  SELECT 1
                  FROM touched shadow
                  WHERE shadow.client_id = touched.client_id
                    AND shadow.interface = touched.interface
                    AND shadow.bucket_start = date_trunc(
                        'minute', stream.latest_sample_observed_at
                    )
              )

            UNION ALL

            SELECT sample.observed_at AS bucket_start,
                   sample.rx_bytes, sample.tx_bytes,
                   sample.rx_counter_epoch, sample.tx_counter_epoch,
                   sample.sample_source,
                   0::INTEGER AS source_priority
            FROM public.traffic_counter_samples sample
            WHERE sample.client_id = touched.client_id
              AND sample.source_kind = 'host'
              AND sample.interface = touched.interface
              AND sample.observed_at < touched.bucket_start
              AND NOT EXISTS (
                  SELECT 1
                  FROM touched shadow
                  WHERE shadow.client_id = sample.client_id
                    AND shadow.interface = sample.interface
                    AND shadow.bucket_start = sample.observed_at
              )
        ) candidate
        ORDER BY candidate.bucket_start DESC, candidate.source_priority DESC
        LIMIT 1
    ) predecessor ON TRUE
), source AS NOT MATERIALIZED (
    SELECT
        expanded.client_id,
        touched.interface,
        touched.bucket_start,
        touched.durable_predecessor_bucket_start,
        expanded.accepted_seq,
        expanded.accepted_at,
        expanded.observed_at,
        expanded.ordinality,
        expanded.rx_bytes,
        expanded.tx_bytes,
        touched.prior_rx_bytes,
        touched.prior_tx_bytes,
        touched.prior_rx_counter_epoch,
        touched.prior_tx_counter_epoch,
        touched.prior_sample_source
    FROM touched_predecessors touched
    JOIN expanded
      ON expanded.client_id = touched.client_id
     AND expanded.interface = touched.interface
     AND expanded.bucket_start = touched.bucket_start
), ordered AS NOT MATERIALIZED (
    SELECT source.*,
           lag(source.rx_bytes) OVER stream_order AS lag_rx_bytes,
           lag(source.tx_bytes) OVER stream_order AS lag_tx_bytes
    FROM source
    WINDOW stream_order AS (
        PARTITION BY source.client_id, source.interface,
                     source.durable_predecessor_bucket_start
        ORDER BY source.observed_at, source.accepted_seq, source.ordinality
    )
), epoch AS NOT MATERIALIZED (
    SELECT ordered.*,
           prior_rx_counter_epoch + sum(
               -- Projected envelopes are live agent observations.  The first
               -- live edge after imported evidence starts one new epoch even
               -- when its counter did not decrease, exactly as minute closure.
               CASE WHEN rx_bytes < COALESCE(lag_rx_bytes, prior_rx_bytes, rx_bytes)
                          OR (
                              lag_rx_bytes IS NULL
                              AND prior_sample_source LIKE 'vnstat_import:%'
                          )
                    THEN 1 ELSE 0 END
           ) OVER stream_order AS rx_counter_epoch,
           prior_tx_counter_epoch + sum(
               CASE WHEN tx_bytes < COALESCE(lag_tx_bytes, prior_tx_bytes, tx_bytes)
                          OR (
                              lag_tx_bytes IS NULL
                              AND prior_sample_source LIKE 'vnstat_import:%'
                          )
                    THEN 1 ELSE 0 END
           ) OVER stream_order AS tx_counter_epoch
    FROM ordered
    WINDOW stream_order AS (
        PARTITION BY client_id, interface,
                     durable_predecessor_bucket_start
        ORDER BY observed_at, accepted_seq, ordinality
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    )
)
SELECT
    client_id,
    interface,
    bucket_start,
    60::INTEGER AS bucket_secs,
    count(*)::INTEGER AS sample_count,
    sum(rx_bytes::NUMERIC)::NUMERIC(39,0) AS rx_bytes_sum,
    sum(tx_bytes::NUMERIC)::NUMERIC(39,0) AS tx_bytes_sum,
    round(avg(rx_bytes::NUMERIC))::BIGINT AS rx_bytes_avg,
    round(avg(tx_bytes::NUMERIC))::BIGINT AS tx_bytes_avg,
    (array_agg(rx_bytes ORDER BY observed_at DESC,
        accepted_seq DESC, ordinality DESC))[1] AS rx_bytes_last,
    (array_agg(tx_bytes ORDER BY observed_at DESC,
        accepted_seq DESC, ordinality DESC))[1] AS tx_bytes_last,
    (array_agg(rx_counter_epoch ORDER BY observed_at DESC,
        accepted_seq DESC, ordinality DESC))[1] AS rx_counter_epoch,
    (array_agg(tx_counter_epoch ORDER BY observed_at DESC,
        accepted_seq DESC, ordinality DESC))[1] AS tx_counter_epoch,
    max(observed_at) AS latest_observed_at,
    max(accepted_at) AS updated_at
FROM epoch
GROUP BY client_id, interface, bucket_start
$$;
-- Closed network points have two durable physical owners: retained rate tiers
-- and exact traffic minutes awaiting their day-one promotion. This source is
-- the storage-free logical boundary consumed by durable projections; it never
-- expands the projected raw journal.
CREATE FUNCTION public.telemetry_network_durable_points_source(
    p_client_ids TEXT[],
    p_min_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_max_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_bucket_secs INTEGER DEFAULT NULL,
    p_interfaces TEXT[] DEFAULT NULL,
    p_per_stream_limit BIGINT DEFAULT NULL
)
RETURNS SETOF public.telemetry_network_rates
LANGUAGE plpgsql
STABLE
AS $$
BEGIN
    -- A bounded history reader already knows every physical stream owner.  Its
    -- optional cap is therefore per exact (client, interface), never a fleet
    -- LIMIT.  Each physical branch may stop at the same K before the final
    -- merge: the K newest rows of a union cannot contain row K+1 from any one
    -- branch.  The outer limit remains the canonical durable-owner boundary.
    -- Callers overlay and cap the projected raw suffix separately so an open
    -- minute continues to replace, rather than compete with, its durable key.
    IF p_per_stream_limit IS NOT NULL THEN
        IF p_client_ids IS NULL OR p_interfaces IS NULL THEN
            RAISE EXCEPTION
                'a durable network per-stream limit requires exact client and interface owners';
        END IF;
        IF cardinality(p_client_ids) <> cardinality(p_interfaces) THEN
            RAISE EXCEPTION
                'durable network per-stream owner arrays must have equal cardinality';
        END IF;
        IF array_position(p_client_ids, NULL) IS NOT NULL
           OR array_position(p_interfaces, NULL) IS NOT NULL THEN
            RAISE EXCEPTION
                'durable network per-stream owner arrays cannot contain NULL';
        END IF;
        IF p_per_stream_limit < 1 THEN
            RETURN;
        END IF;

        RETURN QUERY
        WITH requested_streams AS MATERIALIZED (
            SELECT DISTINCT requested.client_id, requested.interface
            FROM unnest(p_client_ids, p_interfaces)
                requested(client_id, interface)
        )
        SELECT point.*
        FROM requested_streams stream
        CROSS JOIN LATERAL (
            SELECT candidate.*
            FROM (
                (
                    SELECT minute.*
                    FROM public.telemetry_network_rates_minute minute
                    WHERE (p_bucket_secs IS NULL OR p_bucket_secs = 60)
                      AND minute.client_id = stream.client_id
                      AND minute.interface = stream.interface
                      AND minute.bucket_start >= COALESCE(
                              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                          )
                      AND minute.bucket_start <= COALESCE(
                              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                          )
                      AND (
                          p_min_bucket_start IS NULL
                          OR minute.latest_observed_at >= p_min_bucket_start
                      )
                      AND (
                          p_max_bucket_start IS NULL
                          OR minute.latest_observed_at
                              < p_max_bucket_start + interval '1 minute'
                      )
                    -- For one non-overlapping stream, effective observation
                    -- order and bucket-start order are strictly equivalent.
                    -- This order reaches the existing per-stream index.
                    ORDER BY minute.latest_observed_at DESC,
                             minute.bucket_start DESC
                    LIMIT p_per_stream_limit
                    OFFSET 0
                )

                UNION ALL

                (
                    SELECT
                        sample.client_id,
                        sample.interface,
                        sample.observed_at AS bucket_start,
                        60::INTEGER AS bucket_secs,
                        sample.sample_count,
                        sample.rx_bytes_sum,
                        sample.tx_bytes_sum,
                        round(
                            sample.rx_bytes_sum / sample.sample_count::NUMERIC
                        )::BIGINT,
                        round(
                            sample.tx_bytes_sum / sample.sample_count::NUMERIC
                        )::BIGINT,
                        sample.rx_bytes,
                        sample.tx_bytes,
                        sample.rx_counter_epoch,
                        sample.tx_counter_epoch,
                        sample.latest_observed_at,
                        sample.updated_at
                    FROM public.traffic_counter_samples sample
                    WHERE (p_bucket_secs IS NULL OR p_bucket_secs = 60)
                      AND sample.client_id = stream.client_id
                      AND sample.source_kind = 'host'
                      AND sample.interface = stream.interface
                      AND NOT sample.inbound_promoted
                      AND sample.observed_at >= COALESCE(
                              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                          )
                      AND sample.observed_at <= COALESCE(
                              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                          )
                    ORDER BY sample.observed_at DESC
                    LIMIT p_per_stream_limit
                    OFFSET 0
                )

                UNION ALL

                (
                    SELECT coarse.*
                    FROM public.telemetry_network_rates_coarse coarse
                    WHERE (p_bucket_secs IS NULL
                           OR p_bucket_secs <> 60
                              AND coarse.bucket_secs = p_bucket_secs)
                      AND coarse.client_id = stream.client_id
                      AND coarse.interface = stream.interface
                      AND coarse.bucket_start >= COALESCE(
                              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                          )
                      AND coarse.bucket_start <= COALESCE(
                              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                          )
                      AND (
                          p_min_bucket_start IS NULL
                          OR coarse.latest_observed_at >= p_min_bucket_start
                      )
                      AND (
                          p_max_bucket_start IS NULL
                          OR coarse.latest_observed_at
                              < p_max_bucket_start + interval '1 day'
                      )
                    ORDER BY coarse.latest_observed_at DESC,
                             coarse.bucket_start DESC,
                             coarse.bucket_secs DESC
                    LIMIT p_per_stream_limit
                    OFFSET 0
                )
            ) candidate
            ORDER BY candidate.bucket_start DESC,
                     candidate.latest_observed_at DESC,
                     candidate.bucket_secs ASC
            LIMIT p_per_stream_limit
            OFFSET 0
        ) point;

        RETURN;
    END IF;

    -- Dashboard coordinate publication names one closed minute exactly.  Keep
    -- that shape separate from range/all-tier inspection so PostgreSQL receives
    -- the complete physical minute key under a generic plan.  In particular,
    -- the minute partition constraint alone is not an index condition: state
    -- bucket_secs = 60 and bucket_start equality explicitly before the known
    -- client/interface suffix of the primary key.
    IF p_bucket_secs = 60
       AND p_min_bucket_start IS NOT NULL
       AND p_max_bucket_start = p_min_bucket_start THEN
        IF p_client_ids IS NULL AND p_interfaces IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_minute retained
            WHERE retained.bucket_secs = 60
              AND retained.bucket_start = p_min_bucket_start;

            RETURN QUERY
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM public.traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
              AND NOT sample.inbound_promoted
              AND sample.observed_at = p_min_bucket_start;
        ELSIF p_client_ids IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_minute retained
            WHERE retained.bucket_secs = 60
              AND retained.bucket_start = p_min_bucket_start
              AND retained.interface = ANY(p_interfaces);

            RETURN QUERY
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM public.traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
              AND sample.interface = ANY(p_interfaces)
              AND NOT sample.inbound_promoted
              AND sample.observed_at = p_min_bucket_start;
        ELSIF p_interfaces IS NULL THEN
            RETURN QUERY
            WITH requested_clients AS MATERIALIZED (
                SELECT DISTINCT requested.client_id
                FROM unnest(p_client_ids) requested(client_id)
                WHERE requested.client_id IS NOT NULL
            )
            SELECT retained.*
            FROM requested_clients requested
            CROSS JOIN LATERAL (
                SELECT retained.*
                FROM public.telemetry_network_rates_minute retained
                WHERE retained.bucket_secs = 60
                  AND retained.bucket_start = p_min_bucket_start
                  AND retained.client_id = requested.client_id
                OFFSET 0
            ) retained;

            RETURN QUERY
            WITH requested_clients AS MATERIALIZED (
                SELECT DISTINCT requested.client_id
                FROM unnest(p_client_ids) requested(client_id)
                WHERE requested.client_id IS NOT NULL
            )
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM requested_clients requested
            CROSS JOIN LATERAL (
                SELECT sample.*
                FROM public.traffic_counter_samples sample
                WHERE sample.client_id = requested.client_id
                  AND sample.source_kind = 'host'
                  AND NOT sample.inbound_promoted
                  AND sample.observed_at = p_min_bucket_start
                OFFSET 0
            ) sample;
        ELSE
            RETURN QUERY
            WITH requested_clients AS MATERIALIZED (
                SELECT DISTINCT requested.client_id
                FROM unnest(p_client_ids) requested(client_id)
                WHERE requested.client_id IS NOT NULL
            ), requested_interfaces AS MATERIALIZED (
                SELECT DISTINCT requested.interface
                FROM unnest(p_interfaces) requested(interface)
                WHERE requested.interface IS NOT NULL
            )
            SELECT retained.*
            FROM requested_clients requested_client
            CROSS JOIN LATERAL (
                SELECT retained.*
                FROM public.telemetry_network_rates_minute retained
                WHERE retained.bucket_secs = 60
                  AND retained.bucket_start = p_min_bucket_start
                  AND retained.client_id = requested_client.client_id
                OFFSET 0
            ) retained
            JOIN requested_interfaces requested_interface
              ON requested_interface.interface = retained.interface;

            RETURN QUERY
            WITH requested_clients AS MATERIALIZED (
                SELECT DISTINCT requested.client_id
                FROM unnest(p_client_ids) requested(client_id)
                WHERE requested.client_id IS NOT NULL
            ), requested_interfaces AS MATERIALIZED (
                SELECT DISTINCT requested.interface
                FROM unnest(p_interfaces) requested(interface)
                WHERE requested.interface IS NOT NULL
            )
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM requested_clients requested_client
            CROSS JOIN requested_interfaces requested_interface
            CROSS JOIN LATERAL (
                SELECT sample.*
                FROM public.traffic_counter_samples sample
                WHERE sample.client_id = requested_client.client_id
                  AND sample.source_kind = 'host'
                  AND sample.interface = requested_interface.interface
                  AND sample.observed_at = p_min_bucket_start
                  AND NOT sample.inbound_promoted
                OFFSET 0
            ) sample;
        END IF;

        RETURN;
    END IF;

    -- The exact minute and retained coarse tiers have different physical
    -- owners. Branch before touching either relation so a parameterized
    -- dashboard coordinate cannot execute the logical all-tier union.
    -- Exact interface vectors take the complete physical key into each
    -- branch before PostgreSQL plans it; the generic NULL scope remains only
    -- for callers that intentionally request every interface.
    IF p_client_ids IS NULL AND p_interfaces IS NOT NULL THEN
        IF p_bucket_secs IS NULL OR p_bucket_secs = 60 THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_minute retained
            WHERE retained.interface = ANY(p_interfaces)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );

            RETURN QUERY
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM public.traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
              AND sample.interface = ANY(p_interfaces)
              AND NOT sample.inbound_promoted
              AND sample.observed_at >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND sample.observed_at <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;

        IF p_bucket_secs IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.interface = ANY(p_interfaces)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        ELSIF p_bucket_secs <> 60 THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.bucket_secs = p_bucket_secs
              AND retained.interface = ANY(p_interfaces)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;

        RETURN;
    END IF;

    IF p_client_ids IS NOT NULL AND p_interfaces IS NOT NULL THEN
        IF p_bucket_secs IS NULL OR p_bucket_secs = 60 THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_minute retained
            WHERE retained.client_id = ANY(p_client_ids)
              AND retained.interface = ANY(p_interfaces)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );

            RETURN QUERY
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM public.traffic_counter_samples sample
            WHERE sample.client_id = ANY(p_client_ids)
              AND sample.source_kind = 'host'
              AND sample.interface = ANY(p_interfaces)
              AND NOT sample.inbound_promoted
              AND sample.observed_at >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND sample.observed_at <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;

        IF p_bucket_secs IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.client_id = ANY(p_client_ids)
              AND retained.interface = ANY(p_interfaces)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        ELSIF p_bucket_secs <> 60 THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.bucket_secs = p_bucket_secs
              AND retained.client_id = ANY(p_client_ids)
              AND retained.interface = ANY(p_interfaces)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;

        RETURN;
    END IF;

    IF p_bucket_secs IS NULL OR p_bucket_secs = 60 THEN
        IF p_client_ids IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_minute retained
            WHERE retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );

            RETURN QUERY
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM public.traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
              AND NOT sample.inbound_promoted
              AND sample.observed_at >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND sample.observed_at <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        ELSE
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_minute retained
            WHERE retained.client_id = ANY(p_client_ids)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );

            RETURN QUERY
            SELECT
                sample.client_id,
                sample.interface,
                sample.observed_at AS bucket_start,
                60::INTEGER AS bucket_secs,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                round(
                    sample.rx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                round(
                    sample.tx_bytes_sum / sample.sample_count::NUMERIC
                )::BIGINT,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.latest_observed_at,
                sample.updated_at
            FROM public.traffic_counter_samples sample
            WHERE sample.client_id = ANY(p_client_ids)
              AND sample.source_kind = 'host'
              AND NOT sample.inbound_promoted
              AND sample.observed_at >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND sample.observed_at <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;
    END IF;

    IF p_bucket_secs IS NULL THEN
        IF p_client_ids IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        ELSE
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.client_id = ANY(p_client_ids)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;
    ELSIF p_bucket_secs <> 60 THEN
        IF p_client_ids IS NULL THEN
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.bucket_secs = p_bucket_secs
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        ELSE
            RETURN QUERY
            SELECT retained.*
            FROM public.telemetry_network_rates_coarse retained
            WHERE retained.bucket_secs = p_bucket_secs
              AND retained.client_id = ANY(p_client_ids)
              AND retained.bucket_start >= COALESCE(
                      p_min_bucket_start, '-infinity'::TIMESTAMPTZ
                  )
              AND retained.bucket_start <= COALESCE(
                      p_max_bucket_start, 'infinity'::TIMESTAMPTZ
                  );
        END IF;
    END IF;
END;
$$;
-- Effective history overlays the unpublished raw suffix on that
-- durable owner.  A raw natural minute shadows the same durable coordinate
-- until its consumer atomically publishes and advances the cursor.
CREATE FUNCTION public.telemetry_network_rate_points_source(
    p_client_ids TEXT[],
    p_min_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_max_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_bucket_secs INTEGER DEFAULT NULL,
    p_interfaces TEXT[] DEFAULT NULL
)
RETURNS SETOF public.telemetry_network_rates
LANGUAGE sql
STABLE
AS $$
WITH projected_suffix AS MATERIALIZED (
    SELECT suffix.*
    FROM public.telemetry_projected_raw_network_minutes_source(p_client_ids)
        suffix
    WHERE suffix.bucket_start >= COALESCE(
              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
          )
      AND suffix.bucket_start <= COALESCE(
              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
          )
      AND (p_bucket_secs IS NULL OR suffix.bucket_secs = p_bucket_secs)
      AND (p_interfaces IS NULL OR suffix.interface = ANY(p_interfaces))
), durable AS NOT MATERIALIZED (
    SELECT point.*
    FROM public.telemetry_network_durable_points_source(
        p_client_ids,
        p_min_bucket_start,
        p_max_bucket_start,
        p_bucket_secs,
        p_interfaces
    ) point
)
SELECT durable.*
FROM durable
WHERE NOT EXISTS (
    SELECT 1
    FROM projected_suffix suffix
    WHERE suffix.client_id = durable.client_id
      AND suffix.interface = durable.interface
      AND suffix.bucket_secs = durable.bucket_secs
      AND suffix.bucket_start = durable.bucket_start
)
UNION ALL
SELECT suffix.*
FROM projected_suffix suffix
$$;
-- Canonical current host-interface identity reads the revision-ready compact
-- stream owner plus only its strictly newer projected suffix.  It performs no
-- exact-sample or retained-history probe.
CREATE FUNCTION public.telemetry_network_current_identities_source(
    p_client_ids TEXT[]
)
RETURNS TABLE (
    client_id TEXT,
    interface TEXT
)
LANGUAGE sql
STABLE
AS $$
WITH requested_clients AS MATERIALIZED (
    SELECT DISTINCT requested.client_id
    FROM unnest(p_client_ids) requested(client_id)
    WHERE p_client_ids IS NOT NULL
), requested_heads AS MATERIALIZED (
    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM public.traffic_counter_minute_heads minute
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NULL

    UNION ALL

    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM requested_clients requested
    JOIN public.traffic_counter_minute_heads minute USING (client_id)
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NOT NULL
), raw_samples AS NOT MATERIALIZED (
    SELECT sample.*
    FROM requested_heads head
    JOIN public.telemetry_samples sample
      ON sample.client_id = head.client_id
     AND sample.accepted_seq > head.materialized_seq
     AND sample.accepted_seq <= head.projected_seq
    WHERE p_client_ids IS NULL

    UNION ALL

    SELECT sample.*
    FROM requested_heads head
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = head.client_id
          AND sample.accepted_seq > head.materialized_seq
          AND sample.accepted_seq <= head.projected_seq
        OFFSET 0
    ) sample
    WHERE p_client_ids IS NOT NULL
), network_payloads AS MATERIALIZED (
    SELECT sample.client_id, sample.observed_at,
           sample.network_admission_mask,
           CASE WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
               THEN sample.payload -> 'networks' ELSE '[]'::JSONB END
               AS networks,
           CASE WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
               THEN jsonb_array_length(sample.payload -> 'networks')::BIGINT
               ELSE 0 END AS network_count
    FROM raw_samples sample
), valid_network_payloads AS MATERIALIZED (
    SELECT sample.*
    FROM network_payloads sample
    WHERE public.telemetry_ordinal_admission_mask_is_exact(
        sample.network_admission_mask,
        sample.network_count
    )
), stream_registry AS MATERIALIZED (
    SELECT stream.*
    FROM public.traffic_counter_streams stream
    WHERE p_client_ids IS NULL
      AND stream.source_kind = 'host'

    UNION ALL

    SELECT stream.*
    FROM requested_clients requested
    CROSS JOIN LATERAL (
        SELECT stream.*
        FROM public.traffic_counter_streams stream
        WHERE stream.client_id = requested.client_id
          AND stream.source_kind = 'host'
        OFFSET 0
    ) stream
    WHERE p_client_ids IS NOT NULL
), ready_base AS MATERIALIZED (
    SELECT stream.*
    FROM stream_registry stream
    WHERE stream.sample_edge_revision = stream.source_revision
      AND stream.sample_edge_revision > 0
      AND stream.latest_sample_observed_at IS NOT NULL
), raw AS MATERIALIZED (
    SELECT sample.client_id,
           network.value ->> 'interface' AS interface,
           date_trunc('minute', sample.observed_at) AS bucket_start
    FROM valid_network_payloads sample
    CROSS JOIN LATERAL jsonb_array_elements(sample.networks)
        WITH ORDINALITY network(value, ordinality)
    WHERE get_bit(
              sample.network_admission_mask,
              (network.ordinality - 1)::INTEGER
          ) = 1
      AND octet_length(network.value ->> 'interface') BETWEEN 1 AND 128
), raw_streams AS MATERIALIZED (
    SELECT raw.client_id, raw.interface,
           min(raw.bucket_start) AS first_bucket_start
    FROM raw
    GROUP BY raw.client_id, raw.interface
), raw_stream_shape AS MATERIALIZED (
    SELECT raw.client_id, raw.interface,
           registry.client_id IS NULL
           OR (
               registry.sample_edge_revision = registry.source_revision
               AND registry.sample_edge_revision > 0
               AND (
                   registry.latest_sample_observed_at IS NULL
                   OR raw.first_bucket_start >
                       registry.latest_sample_observed_at
               )
           ) AS append_safe
    FROM raw_streams raw
    LEFT JOIN stream_registry registry
      ON registry.client_id = raw.client_id
     AND registry.interface = raw.interface
), raw_identities AS MATERIALIZED (
    SELECT shape.client_id, shape.interface
    FROM raw_stream_shape shape
    WHERE shape.append_safe
), durable_identities AS MATERIALIZED (
    SELECT stream.client_id, stream.interface
    FROM ready_base stream
    WHERE NOT EXISTS (
              SELECT 1
              FROM raw_identities raw
              WHERE raw.client_id = stream.client_id
                AND raw.interface = stream.interface
          )
)
SELECT durable.client_id, durable.interface
FROM durable_identities durable
UNION ALL
SELECT raw.client_id, raw.interface
FROM raw_identities raw;
$$;
-- Current network readers already resolve their complete client set.  Bind
-- that owner before reading the projected journal or durable stream edges so
-- a request never expands unrelated clients' network JSON/history. NULL
-- retains deliberate all-client inspection semantics.
CREATE FUNCTION public.telemetry_network_current_source(
    p_client_ids TEXT[]
)
RETURNS TABLE (
    client_id TEXT,
    interface TEXT,
    latest_bucket_start TIMESTAMPTZ,
    latest_bucket_secs INTEGER,
    latest_sample_count INTEGER,
    latest_observed_at TIMESTAMPTZ,
    latest_rx_bytes_avg BIGINT,
    latest_tx_bytes_avg BIGINT,
    latest_rx_bytes BIGINT,
    latest_tx_bytes BIGINT,
    latest_admitted_at_projection BOOLEAN,
    latest_rx_counter_epoch BIGINT,
    latest_tx_counter_epoch BIGINT,
    previous_observed_at TIMESTAMPTZ,
    previous_rx_bytes BIGINT,
    previous_tx_bytes BIGINT,
    previous_rx_counter_epoch BIGINT,
    previous_tx_counter_epoch BIGINT,
    rx_bytes_delta BIGINT,
    tx_bytes_delta BIGINT,
    rx_bps_avg DOUBLE PRECISION,
    tx_bps_avg DOUBLE PRECISION,
    transition_valid BOOLEAN,
    transition_admitted_at_projection BOOLEAN,
    updated_at TIMESTAMPTZ
)
LANGUAGE sql
STABLE
AS $$
WITH requested_clients AS MATERIALIZED (
    SELECT DISTINCT requested.client_id
    FROM unnest(p_client_ids) requested(client_id)
    WHERE p_client_ids IS NOT NULL
), requested_heads AS MATERIALIZED (
    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM public.traffic_counter_minute_heads minute
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NULL

    UNION ALL

    SELECT minute.client_id, minute.materialized_seq,
           projection.projected_seq
    FROM requested_clients requested
    JOIN public.traffic_counter_minute_heads minute USING (client_id)
    JOIN public.telemetry_projection_heads projection USING (client_id)
    WHERE p_client_ids IS NOT NULL
), stream_registry AS MATERIALIZED (
    SELECT stream.*
    FROM public.traffic_counter_streams stream
    WHERE p_client_ids IS NULL
      AND stream.source_kind = 'host'

    UNION ALL

    SELECT stream.*
    FROM requested_clients requested
    CROSS JOIN LATERAL (
        SELECT stream.*
        FROM public.traffic_counter_streams stream
        WHERE stream.client_id = requested.client_id
          AND stream.source_kind = 'host'
        OFFSET 0
    ) stream
    WHERE p_client_ids IS NOT NULL
), ready_base AS MATERIALIZED (
    SELECT stream.*
    FROM stream_registry stream
    WHERE stream.sample_edge_revision = stream.source_revision
      AND stream.sample_edge_revision > 0
      AND stream.latest_sample_observed_at IS NOT NULL
), raw_samples AS NOT MATERIALIZED (
    SELECT sample.*
    FROM requested_heads head
    JOIN public.telemetry_samples sample
      ON sample.client_id = head.client_id
     AND sample.accepted_seq > head.materialized_seq
     AND sample.accepted_seq <= head.projected_seq
    WHERE p_client_ids IS NULL

    UNION ALL

    SELECT sample.*
    FROM requested_heads head
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = head.client_id
          AND sample.accepted_seq > head.materialized_seq
          AND sample.accepted_seq <= head.projected_seq
        OFFSET 0
    ) sample
    WHERE p_client_ids IS NOT NULL
), network_payloads AS MATERIALIZED (
    SELECT sample.client_id, sample.observed_at, sample.accepted_seq,
           sample.accepted_at, sample.network_admission_mask,
           CASE WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
               THEN sample.payload -> 'networks' ELSE '[]'::JSONB END
               AS networks,
           CASE WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
               THEN jsonb_array_length(sample.payload -> 'networks')::BIGINT
               ELSE 0 END AS network_count
    FROM raw_samples sample
), valid_network_payloads AS MATERIALIZED (
    SELECT sample.*
    FROM network_payloads sample
    WHERE public.telemetry_ordinal_admission_mask_is_exact(
        sample.network_admission_mask,
        sample.network_count
    )
), raw AS MATERIALIZED (
    SELECT
        sample.client_id,
        network.value ->> 'interface' AS interface,
        date_trunc('minute', sample.observed_at) AS bucket_start,
        sample.observed_at,
        sample.accepted_seq,
        sample.accepted_at AS updated_at,
        network.ordinality,
        public.telemetry_u64_counter_to_bigint(
            network.value ->> 'rx_bytes'
        ) AS rx_bytes,
        public.telemetry_u64_counter_to_bigint(
            network.value ->> 'tx_bytes'
        ) AS tx_bytes,
        TRUE AS admitted_at_projection
    FROM valid_network_payloads sample
    CROSS JOIN LATERAL jsonb_array_elements(sample.networks)
        WITH ORDINALITY network(value, ordinality)
    WHERE get_bit(
              sample.network_admission_mask,
              (network.ordinality - 1)::INTEGER
          ) = 1
      AND octet_length(network.value ->> 'interface') BETWEEN 1 AND 128
), raw_streams AS MATERIALIZED (
    SELECT raw.client_id, raw.interface,
           min(raw.bucket_start) AS first_bucket_start
    FROM raw
    GROUP BY raw.client_id, raw.interface
), raw_stream_shape AS MATERIALIZED (
    SELECT raw.client_id, raw.interface,
           registry.client_id IS NULL
           OR (
               registry.sample_edge_revision = registry.source_revision
               AND registry.sample_edge_revision > 0
               AND (
                   registry.latest_sample_observed_at IS NULL
                   OR raw.first_bucket_start >
                       registry.latest_sample_observed_at
               )
           ) AS append_safe,
           registry.latest_sample_rx_counter_epoch AS anchor_rx_counter_epoch,
           registry.latest_sample_tx_counter_epoch AS anchor_tx_counter_epoch,
           registry.latest_sample_rx_bytes AS anchor_rx_bytes,
           registry.latest_sample_tx_bytes AS anchor_tx_bytes,
           registry.latest_sample_source AS anchor_sample_source
    FROM raw_streams raw
    LEFT JOIN stream_registry registry
      ON registry.client_id = raw.client_id
     AND registry.interface = raw.interface
), eligible_raw AS MATERIALIZED (
    SELECT raw.*,
           shape.anchor_rx_counter_epoch,
           shape.anchor_tx_counter_epoch,
           shape.anchor_rx_bytes,
           shape.anchor_tx_bytes,
           shape.anchor_sample_source
    FROM raw
    JOIN raw_stream_shape shape
      ON shape.client_id = raw.client_id
     AND shape.interface = raw.interface
    WHERE shape.append_safe
), raw_lagged AS (
    SELECT raw.*,
           lag(raw.rx_bytes) OVER stream_order AS lag_rx_bytes,
           lag(raw.tx_bytes) OVER stream_order AS lag_tx_bytes
    FROM eligible_raw raw
    WINDOW stream_order AS (
        PARTITION BY raw.client_id, raw.interface
        ORDER BY raw.observed_at, raw.accepted_seq, raw.ordinality
    )
), raw_epoch AS (
    SELECT
        raw_lagged.*,
        COALESCE(anchor_rx_counter_epoch, 0) + sum(
            CASE WHEN
                rx_bytes < COALESCE(lag_rx_bytes, anchor_rx_bytes, rx_bytes)
                OR (
                    lag_rx_bytes IS NULL
                    AND anchor_sample_source LIKE 'vnstat_import:%'
                )
                 THEN 1 ELSE 0 END
        ) OVER stream_order AS rx_counter_epoch,
        COALESCE(anchor_tx_counter_epoch, 0) + sum(
            CASE WHEN
                tx_bytes < COALESCE(lag_tx_bytes, anchor_tx_bytes, tx_bytes)
                OR (
                    lag_tx_bytes IS NULL
                    AND anchor_sample_source LIKE 'vnstat_import:%'
                )
                 THEN 1 ELSE 0 END
        ) OVER stream_order AS tx_counter_epoch
    FROM raw_lagged
    WINDOW stream_order AS (
        PARTITION BY client_id, interface
        ORDER BY observed_at, accepted_seq, ordinality
        ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW
    )
), observations AS (
    SELECT base.client_id, base.interface,
           base.previous_sample_effective_observed_at AS observed_at,
           0::BIGINT AS accepted_seq,
           0::BIGINT AS ordinality,
           base.latest_sample_updated_at AS updated_at,
           base.previous_sample_rx_bytes AS rx_bytes,
           base.previous_sample_tx_bytes AS tx_bytes,
           TRUE AS admitted_at_projection,
           base.previous_sample_rx_counter_epoch AS rx_counter_epoch,
           base.previous_sample_tx_counter_epoch AS tx_counter_epoch
    FROM ready_base base
    WHERE base.previous_sample_effective_observed_at IS NOT NULL

    UNION ALL

    SELECT base.client_id, base.interface,
           base.latest_sample_effective_observed_at AS observed_at,
           0::BIGINT AS accepted_seq,
           0::BIGINT AS ordinality,
           base.latest_sample_updated_at AS updated_at,
           base.latest_sample_rx_bytes AS rx_bytes,
           base.latest_sample_tx_bytes AS tx_bytes,
           TRUE AS admitted_at_projection,
           base.latest_sample_rx_counter_epoch AS rx_counter_epoch,
           base.latest_sample_tx_counter_epoch AS tx_counter_epoch
    FROM ready_base base

    UNION ALL

    SELECT raw.client_id, raw.interface, raw.observed_at, raw.accepted_seq,
           raw.ordinality, raw.updated_at, raw.rx_bytes, raw.tx_bytes,
           raw.admitted_at_projection, raw.rx_counter_epoch,
           raw.tx_counter_epoch
    FROM raw_epoch raw
), ranked AS (
    SELECT observations.*,
           row_number() OVER (
               PARTITION BY client_id, interface
               ORDER BY observed_at DESC, accepted_seq DESC, ordinality DESC
           ) AS recency
    FROM observations
), edges AS (
    SELECT
        client_id,
        interface,
        max(observed_at) FILTER (WHERE recency = 1) AS latest_observed_at,
        max(rx_bytes) FILTER (WHERE recency = 1) AS latest_rx_bytes,
        max(tx_bytes) FILTER (WHERE recency = 1) AS latest_tx_bytes,
        max(rx_counter_epoch) FILTER (WHERE recency = 1)
            AS latest_rx_counter_epoch,
        max(tx_counter_epoch) FILTER (WHERE recency = 1)
            AS latest_tx_counter_epoch,
        bool_or(admitted_at_projection) FILTER (WHERE recency = 1)
            AS latest_admitted_at_projection,
        max(updated_at) FILTER (WHERE recency = 1) AS updated_at,
        max(observed_at) FILTER (WHERE recency = 2) AS previous_observed_at,
        max(rx_bytes) FILTER (WHERE recency = 2) AS previous_rx_bytes,
        max(tx_bytes) FILTER (WHERE recency = 2) AS previous_tx_bytes,
        max(rx_counter_epoch) FILTER (WHERE recency = 2)
            AS previous_rx_counter_epoch,
        max(tx_counter_epoch) FILTER (WHERE recency = 2)
            AS previous_tx_counter_epoch,
        bool_or(admitted_at_projection) FILTER (WHERE recency = 2)
            AS previous_admitted_at_projection
    FROM ranked
    WHERE recency <= 2
    GROUP BY client_id, interface
), raw_minutes AS (
    SELECT
        client_id, interface, date_trunc('minute', observed_at) AS bucket_start,
        count(*)::INTEGER AS sample_count,
        round(avg(rx_bytes::NUMERIC))::BIGINT AS rx_bytes_avg,
        round(avg(tx_bytes::NUMERIC))::BIGINT AS tx_bytes_avg
    FROM raw_epoch
    GROUP BY client_id, interface, date_trunc('minute', observed_at)
), durable_minutes AS (
    SELECT base.client_id, base.interface,
           base.latest_sample_observed_at AS bucket_start,
           base.latest_sample_count AS sample_count,
           base.latest_sample_rx_bytes_avg AS rx_bytes_avg,
           base.latest_sample_tx_bytes_avg AS tx_bytes_avg
    FROM ready_base base
), minute_summary AS (
    SELECT * FROM raw_minutes
    UNION ALL
    SELECT * FROM durable_minutes
), current_state AS (
    SELECT edges.*,
           summary.sample_count AS latest_sample_count,
           summary.rx_bytes_avg AS latest_rx_bytes_avg,
           summary.tx_bytes_avg AS latest_tx_bytes_avg,
           edges.previous_observed_at IS NOT NULL
           AND edges.latest_observed_at > edges.previous_observed_at
           AND edges.latest_rx_counter_epoch = edges.previous_rx_counter_epoch
           AND edges.latest_tx_counter_epoch = edges.previous_tx_counter_epoch
           AND edges.latest_rx_bytes >= edges.previous_rx_bytes
           AND edges.latest_tx_bytes >= edges.previous_tx_bytes
               AS transition_valid
    FROM edges
    JOIN minute_summary summary
      ON summary.client_id = edges.client_id
     AND summary.interface = edges.interface
     AND summary.bucket_start = date_trunc('minute', edges.latest_observed_at)
)
SELECT
    client_id,
    interface,
    date_trunc('minute', latest_observed_at) AS latest_bucket_start,
    60::INTEGER AS latest_bucket_secs,
    latest_sample_count,
    latest_observed_at,
    latest_rx_bytes_avg,
    latest_tx_bytes_avg,
    latest_rx_bytes,
    latest_tx_bytes,
    latest_admitted_at_projection,
    latest_rx_counter_epoch,
    latest_tx_counter_epoch,
    previous_observed_at,
    previous_rx_bytes,
    previous_tx_bytes,
    previous_rx_counter_epoch,
    previous_tx_counter_epoch,
    CASE WHEN transition_valid
         THEN latest_rx_bytes - previous_rx_bytes ELSE 0 END AS rx_bytes_delta,
    CASE WHEN transition_valid
         THEN latest_tx_bytes - previous_tx_bytes ELSE 0 END AS tx_bytes_delta,
    CASE WHEN transition_valid THEN
        (latest_rx_bytes - previous_rx_bytes)::DOUBLE PRECISION * 8
        / EXTRACT(EPOCH FROM latest_observed_at - previous_observed_at)
        ELSE 0::DOUBLE PRECISION END AS rx_bps_avg,
    CASE WHEN transition_valid THEN
        (latest_tx_bytes - previous_tx_bytes)::DOUBLE PRECISION * 8
        / EXTRACT(EPOCH FROM latest_observed_at - previous_observed_at)
        ELSE 0::DOUBLE PRECISION END AS tx_bps_avg,
    transition_valid,
    transition_valid
        AND latest_admitted_at_projection
        AND COALESCE(previous_admitted_at_projection, FALSE)
        AS transition_admitted_at_projection,
    updated_at
FROM current_state;
$$;
-- Indexes.

CREATE INDEX history_retention_policies_updated_idx ON public.history_retention_policies USING btree (updated_at DESC, domain);



CREATE INDEX telemetry_history_due_spans_due_idx ON public.telemetry_history_due_spans USING btree (domain, due_at, source_bucket_secs, destination_bucket_secs, destination_start);



CREATE INDEX telemetry_history_due_events_ready_idx ON public.telemetry_history_due_events USING btree (coalesce_ready_at, event_id);



-- The global ready index chooses one consumer owner.  Once chosen, this
-- independent index retrieves every ready event for that exact natural
-- coordinate without walking unrelated ready events.
CREATE INDEX telemetry_history_due_events_coordinate_ready_idx
ON public.telemetry_history_due_events USING btree (
    domain, source_bucket_secs, destination_bucket_secs,
    destination_start, owner_identity, coalesce_ready_at, event_id
);



CREATE INDEX ping_target_assignments_client_idx ON public.ping_target_assignments USING btree (client_id, target_id);



CREATE UNIQUE INDEX ping_target_assignments_one_primary_per_client_idx ON public.ping_target_assignments USING btree (client_id) WHERE is_primary;



CREATE UNIQUE INDEX ping_targets_name_unique_idx ON public.ping_targets USING btree (lower(name));



CREATE INDEX ping_targets_updated_idx ON public.ping_targets USING btree (updated_at DESC, name);



CREATE INDEX telemetry_network_rates_coarse_client_effective_idx ON public.telemetry_network_rates_coarse USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC, bucket_secs DESC);



CREATE INDEX telemetry_network_rates_coarse_effective_global_idx ON public.telemetry_network_rates_coarse USING btree (latest_observed_at DESC, client_id, interface, bucket_start DESC, bucket_secs DESC);



CREATE INDEX telemetry_network_rates_coarse_retention_idx ON public.telemetry_network_rates_coarse USING btree (bucket_start);



CREATE INDEX telemetry_network_rates_minute_client_effective_idx ON public.telemetry_network_rates_minute USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC);



CREATE INDEX telemetry_network_rates_minute_effective_global_idx ON public.telemetry_network_rates_minute USING btree (latest_observed_at DESC, client_id, interface, bucket_start DESC);



CREATE INDEX telemetry_network_rates_minute_retention_idx ON public.telemetry_network_rates_minute USING btree (bucket_start);



CREATE INDEX telemetry_ping_current_latest_idx ON public.telemetry_ping_current USING btree (latest_checked_at DESC, series_id);



CREATE INDEX telemetry_ping_facts_retention_idx ON public.telemetry_ping_facts USING btree (observed_at);



CREATE INDEX telemetry_ping_facts_series_checked_idx ON public.telemetry_ping_facts USING btree (series_id, checked_unix);



CREATE INDEX telemetry_ping_rollups_current_range_idx ON public.telemetry_ping_rollups USING btree (series_id, bucket_start DESC) INCLUDE (bucket_secs, sample_count, loss_ratio_avg);



CREATE INDEX telemetry_ping_rollups_retention_idx ON public.telemetry_ping_rollups USING btree (bucket_start);



CREATE INDEX telemetry_projection_heads_visible_pending_idx ON public.telemetry_projection_heads USING btree (COALESCE(projected_at, '-infinity'::timestamp with time zone), client_id) WHERE (projected_seq < accepted_seq);



CREATE INDEX telemetry_projection_heads_accepted_order_idx ON public.telemetry_projection_heads USING btree (accepted_at, client_id);



CREATE INDEX telemetry_rollups_client_latest_point_idx ON public.telemetry_rollups USING btree (client_id, bucket_start DESC, latest_observed_at DESC, bucket_secs ASC);



CREATE INDEX telemetry_rollups_retention_idx ON public.telemetry_rollups USING btree (bucket_start);



CREATE INDEX telemetry_samples_client_latest_idx ON public.telemetry_samples USING btree (client_id, observed_at DESC, accepted_seq DESC);



CREATE UNIQUE INDEX telemetry_samples_client_accepted_seq_idx ON public.telemetry_samples USING btree (client_id, accepted_seq);



CREATE INDEX telemetry_samples_retention_idx ON public.telemetry_samples USING btree (observed_at);



CREATE INDEX telemetry_minute_materialization_heads_cursor_idx
ON public.telemetry_minute_materialization_heads (materialized_seq, client_id);



CREATE INDEX traffic_counter_minute_heads_cursor_idx
ON public.traffic_counter_minute_heads (materialized_seq, client_id);



-- Triggers.

CREATE TRIGGER telemetry_minute_materialization_heads_retention_update
AFTER UPDATE ON public.telemetry_minute_materialization_heads
REFERENCING OLD TABLE AS old_telemetry_retention_rows
            NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'core_minute_frontier_advanced'
);



CREATE TRIGGER traffic_counter_minute_heads_retention_update
AFTER UPDATE ON public.traffic_counter_minute_heads
REFERENCING OLD TABLE AS old_telemetry_retention_rows
            NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'traffic_minute_frontier_advanced'
);



CREATE TRIGGER telemetry_ping_facts_retention_insert
AFTER INSERT ON public.telemetry_ping_facts
REFERENCING NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'ping_facts_published'
);



CREATE TRIGGER telemetry_ping_facts_retention_update
AFTER UPDATE ON public.telemetry_ping_facts
REFERENCING NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'ping_facts_published'
);



CREATE TRIGGER telemetry_ping_facts_retention_delete
AFTER DELETE ON public.telemetry_ping_facts
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'ping_facts_deleted'
);



CREATE TRIGGER telemetry_ping_current_retention_delete
AFTER DELETE ON public.telemetry_ping_current
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'ping_current_deleted'
);



CREATE TRIGGER telemetry_ping_rollups_retention_delete
AFTER DELETE ON public.telemetry_ping_rollups
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'ping_rollups_deleted'
);



CREATE TRIGGER telemetry_samples_retention_delete
AFTER DELETE ON public.telemetry_samples
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'telemetry_samples_deleted'
);



CREATE TRIGGER telemetry_history_due_spans_retention_insert
AFTER INSERT ON public.telemetry_history_due_spans
REFERENCING NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_history_due_span();



CREATE TRIGGER telemetry_history_due_spans_retention_update
AFTER UPDATE ON public.telemetry_history_due_spans
REFERENCING NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_history_due_span();



CREATE TRIGGER telemetry_network_rates_due_events_insert
AFTER INSERT ON public.telemetry_network_rates
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'telemetry_network_rates'
);



CREATE TRIGGER telemetry_network_rates_due_events_update
AFTER UPDATE ON public.telemetry_network_rates
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'telemetry_network_rates'
);



CREATE TRIGGER telemetry_ping_rollups_due_events_insert
AFTER INSERT ON public.telemetry_ping_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'telemetry_ping_rollups'
);



CREATE TRIGGER telemetry_ping_rollups_due_events_update
AFTER UPDATE ON public.telemetry_ping_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'telemetry_ping_rollups'
);



CREATE TRIGGER telemetry_rollups_due_events_insert
AFTER INSERT ON public.telemetry_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'telemetry_rollups'
);



CREATE TRIGGER telemetry_rollups_due_events_update
AFTER UPDATE ON public.telemetry_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'telemetry_rollups'
);



CREATE TRIGGER clients_telemetry_projection_heads_initialize AFTER INSERT ON public.clients FOR EACH ROW EXECUTE FUNCTION public.initialize_telemetry_projection_heads_for_client();



CREATE TRIGGER clients_telemetry_webhook_cursors_initialize AFTER INSERT ON public.clients FOR EACH ROW EXECUTE FUNCTION public.initialize_telemetry_webhook_cursor_for_client();



CREATE TRIGGER telemetry_webhook_cursors_before_advance BEFORE UPDATE OF last_sample_seq ON public.telemetry_webhook_cursors FOR EACH ROW WHEN (old.last_sample_seq IS DISTINCT FROM new.last_sample_seq) EXECUTE FUNCTION public.validate_telemetry_webhook_cursor_advance();



CREATE TRIGGER telemetry_samples_before_projection_identity_update BEFORE UPDATE OF id, client_id, observed_at, accepted_seq ON public.telemetry_samples FOR EACH ROW WHEN (((old.id IS DISTINCT FROM new.id) OR (old.client_id IS DISTINCT FROM new.client_id) OR (old.observed_at IS DISTINCT FROM new.observed_at) OR (old.accepted_seq IS DISTINCT FROM new.accepted_seq))) EXECUTE FUNCTION public.reject_updated_telemetry_projection_identity();
