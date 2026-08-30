-- Traffic samples, counters, usage summaries, and quota state.

SET LOCAL check_function_bodies = false;

-- Functions.

-- Direct-history and import callers own a single endpoint when they omit the
-- physical-observation aggregate. Live minute closure supplies every field
-- explicitly, so this trigger never guesses over a real aggregate.
CREATE FUNCTION public.normalize_traffic_counter_sample_aggregate()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    NEW.sample_count := COALESCE(NEW.sample_count, 1);
    NEW.rx_bytes_sum := COALESCE(NEW.rx_bytes_sum, NEW.rx_bytes::numeric);
    NEW.tx_bytes_sum := COALESCE(NEW.tx_bytes_sum, NEW.tx_bytes::numeric);
    NEW.latest_observed_at := COALESCE(NEW.latest_observed_at, NEW.observed_at);
    NEW.rx_usage_bytes := COALESCE(NEW.rx_usage_bytes, 0);
    NEW.tx_usage_bytes := COALESCE(NEW.tx_usage_bytes, 0);
    NEW.rx_valid_count := COALESCE(NEW.rx_valid_count, 0);
    NEW.tx_valid_count := COALESCE(NEW.tx_valid_count, 0);
    NEW.any_valid_count := COALESCE(NEW.any_valid_count, 0);
    NEW.rx_reset_count := COALESCE(NEW.rx_reset_count, 0);
    NEW.tx_reset_count := COALESCE(NEW.tx_reset_count, 0);
    NEW.any_reset_count := COALESCE(NEW.any_reset_count, 0);
    NEW.usage_authoritative := COALESCE(NEW.usage_authoritative, FALSE);
    NEW.updated_at := COALESCE(NEW.updated_at, clock_timestamp());

    -- Existing one-observation import/history rows remain canonical when a
    -- caller corrects only their endpoint columns. Multi-observation live rows
    -- must always carry their explicitly recomputed sums.
    IF TG_OP = 'UPDATE' AND OLD.sample_count = 1 THEN
        IF NEW.rx_bytes IS DISTINCT FROM OLD.rx_bytes
           AND NEW.rx_bytes_sum IS NOT DISTINCT FROM OLD.rx_bytes_sum THEN
            NEW.rx_bytes_sum := NEW.rx_bytes::numeric;
        END IF;
        IF NEW.tx_bytes IS DISTINCT FROM OLD.tx_bytes
           AND NEW.tx_bytes_sum IS NOT DISTINCT FROM OLD.tx_bytes_sum THEN
            NEW.tx_bytes_sum := NEW.tx_bytes::numeric;
        END IF;
    END IF;
    RETURN NEW;
END;
$$;



-- The traffic scheduler may run in more than one worker process. Durable rows
-- are its authority; these commit-scoped notifications only make another
-- replica re-check a cached proof after publication. A bulk statement emits
-- at most one raw hint and one hint per supported rollup width.
CREATE FUNCTION public.publish_traffic_counter_samples_retention_effect()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    should_publish BOOLEAN := FALSE;
BEGIN
    IF TG_NARGS <> 0
       OR TG_TABLE_SCHEMA <> 'public'
       OR TG_TABLE_NAME <> 'traffic_counter_samples'
       OR NOT (TG_OP = ANY (ARRAY['INSERT'::text, 'UPDATE'::text])) THEN
        RAISE EXCEPTION
            'invalid traffic-counter sample retention trigger binding';
    END IF;

    IF TG_OP = 'INSERT' THEN
        SELECT EXISTS (
            SELECT 1
            FROM new_traffic_retention_rows new_row
            WHERE NOT new_row.inbound_promoted
        ) INTO should_publish;
    ELSE
        -- Every corrected value is consumed by RawPromotion while the row is
        -- unpromoted. Whole-row equality suppresses only a true no-op;
        -- promoted-only updates cannot recreate raw-promotion work.
        SELECT EXISTS (
            SELECT 1
            FROM new_traffic_retention_rows new_row
            WHERE NOT new_row.inbound_promoted
              AND NOT EXISTS (
                  SELECT 1
                  FROM old_traffic_retention_rows old_row
                  WHERE old_row.client_id = new_row.client_id
                    AND old_row.source_kind = new_row.source_kind
                    AND old_row.interface = new_row.interface
                    AND old_row.observed_at = new_row.observed_at
                    AND to_jsonb(old_row) = to_jsonb(new_row)
              )
        ) INTO should_publish;
    END IF;

    IF should_publish THEN
        PERFORM pg_notify(
            'vpsman_telemetry_retention',
            jsonb_build_object(
                'owner', 'history_retention',
                'effect', 'traffic_samples_published'
            )::TEXT
        );
    END IF;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.publish_traffic_counter_rollups_retention_effect()
RETURNS trigger
LANGUAGE plpgsql
AS $$
DECLARE
    published_bucket_secs INTEGER;
BEGIN
    IF TG_NARGS <> 0
       OR TG_TABLE_SCHEMA <> 'public'
       OR TG_TABLE_NAME <> 'traffic_counter_rollups'
       OR NOT (TG_OP = ANY (ARRAY['INSERT'::text, 'UPDATE'::text])) THEN
        RAISE EXCEPTION
            'invalid traffic-counter rollup retention trigger binding';
    END IF;

    IF TG_OP = 'INSERT' THEN
        FOR published_bucket_secs IN
            SELECT DISTINCT new_row.bucket_secs
            FROM new_traffic_retention_rows new_row
            WHERE new_row.bucket_secs = ANY (ARRAY[3600, 10800, 21600, 86400])
            ORDER BY new_row.bucket_secs
        LOOP
            PERFORM pg_notify(
                'vpsman_telemetry_retention',
                jsonb_build_object(
                    'owner', 'history_retention',
                    'effect', 'traffic_rollup_published',
                    'bucket_secs', published_bucket_secs
                )::TEXT
            );
        END LOOP;
    ELSE
        -- Same-key counter corrections change the next tier even though their
        -- time deadline is unchanged. Compare the complete durable row so
        -- only an actual no-op is silent, and coalesce bulk corrections by
        -- physical width.
        FOR published_bucket_secs IN
            SELECT DISTINCT new_row.bucket_secs
            FROM new_traffic_retention_rows new_row
            WHERE new_row.bucket_secs = ANY (ARRAY[3600, 10800, 21600, 86400])
              AND NOT EXISTS (
                  SELECT 1
                  FROM old_traffic_retention_rows old_row
                  WHERE old_row.client_id = new_row.client_id
                    AND old_row.source_kind = new_row.source_kind
                    AND old_row.interface = new_row.interface
                    AND old_row.origin_kind = new_row.origin_kind
                    AND old_row.bucket_secs = new_row.bucket_secs
                    AND old_row.bucket_start = new_row.bucket_start
                    AND to_jsonb(old_row) = to_jsonb(new_row)
              )
            ORDER BY new_row.bucket_secs
        LOOP
            PERFORM pg_notify(
                'vpsman_telemetry_retention',
                jsonb_build_object(
                    'owner', 'history_retention',
                    'effect', 'traffic_rollup_published',
                    'bucket_secs', published_bucket_secs
                )::TEXT
            );
        END LOOP;
    END IF;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.add_traffic_counter_hourly_usage_totals_after_insert() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF current_setting(
        'vpsman.traffic_hourly_derivations_prepublished', true
    ) = 'on' THEN
        RETURN NULL;
    END IF;

    WITH added AS (
        SELECT
            client_id,
            source_kind,
            interface,
            COALESCE(SUM(rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(SUM(tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(SUM(rx_reset_count), 0)::bigint AS rx_reset_count,
            COALESCE(SUM(tx_reset_count), 0)::bigint AS tx_reset_count,
            COUNT(*)::bigint AS row_count
        FROM new_traffic_counter_hourly_usage
        GROUP BY client_id, source_kind, interface
    )
    UPDATE traffic_counter_streams stream
    SET
        usage_rx_bytes = stream.usage_rx_bytes + added.rx_bytes,
        usage_tx_bytes = stream.usage_tx_bytes + added.tx_bytes,
        usage_rx_reset_count =
            stream.usage_rx_reset_count + added.rx_reset_count,
        usage_tx_reset_count =
            stream.usage_tx_reset_count + added.tx_reset_count,
        usage_row_count = stream.usage_row_count + added.row_count,
        updated_at = now()
    FROM added
    WHERE stream.client_id = added.client_id
      AND stream.source_kind = added.source_kind
      AND stream.interface = added.interface;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.traffic_counter_completed_hour_usage(target_client_id text, target_source_kind text, target_interface text, range_start timestamp with time zone, range_end timestamp with time zone) RETURNS TABLE(rx_bytes bigint, tx_bytes bigint, rx_reset_count bigint, tx_reset_count bigint)
    LANGUAGE sql STABLE
    AS $$
WITH frontier AS MATERIALIZED (
    SELECT GREATEST(
        range_start,
        LEAST(
            range_end,
            date_bin(
                interval '1 hour',
                COALESCE(stream.first_unpromoted_observed_at, range_end),
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            )
        )
    ) AS exact_start
    FROM traffic_counter_streams stream
    WHERE stream.client_id = target_client_id
      AND stream.source_kind = target_source_kind
      AND stream.interface = target_interface
), retained AS (
    SELECT
        COALESCE(sum(rollup.rx_bytes), 0)::bigint AS rx_bytes,
        COALESCE(sum(rollup.tx_bytes), 0)::bigint AS tx_bytes,
        COALESCE(sum(rollup.rx_reset_count), 0)::bigint AS rx_reset_count,
        COALESCE(sum(rollup.tx_reset_count), 0)::bigint AS tx_reset_count
    FROM frontier
    LEFT JOIN traffic_counter_rollups rollup
      ON rollup.client_id = target_client_id
     AND rollup.source_kind = target_source_kind
     AND rollup.interface = target_interface
     AND rollup.bucket_secs = 3600
     AND rollup.bucket_start >= range_start
     AND rollup.bucket_start < frontier.exact_start
), exact AS (
    SELECT
        COALESCE(sum(hourly.rx_bytes), 0)::bigint AS rx_bytes,
        COALESCE(sum(hourly.tx_bytes), 0)::bigint AS tx_bytes,
        COALESCE(sum(hourly.rx_reset_count), 0)::bigint AS rx_reset_count,
        COALESCE(sum(hourly.tx_reset_count), 0)::bigint AS tx_reset_count
    FROM frontier
    LEFT JOIN traffic_counter_hourly_usage hourly
      ON hourly.client_id = target_client_id
     AND hourly.source_kind = target_source_kind
     AND hourly.interface = target_interface
     AND hourly.bucket_start >= frontier.exact_start
     AND hourly.bucket_start < range_end
)
SELECT
    retained.rx_bytes + exact.rx_bytes,
    retained.tx_bytes + exact.tx_bytes,
    retained.rx_reset_count + exact.rx_reset_count,
    retained.tx_reset_count + exact.tx_reset_count
FROM retained CROSS JOIN exact
$$;



CREATE FUNCTION public.traffic_counter_billing_context(target_client_id text, as_of timestamp with time zone) RETURNS TABLE(reset_day integer, reset_hour integer, cycle_start timestamp with time zone, completed_through timestamp with time zone)
    LANGUAGE sql STABLE
    AS $$
WITH raw AS (
    SELECT
        rule.value_json->>'day' AS reset_day_text,
        rule.value_json->>'hour' AS reset_hour_text
    FROM (VALUES (1)) anchor(ordinal)
    LEFT JOIN vps_rule_values rule
      ON rule.client_id = target_client_id
     AND rule.key = 'traffic.reset_day'
), normalized AS (
    SELECT
        CASE
            WHEN COALESCE(reset_day_text, '1') ~ '^-?[0-9]+$'
             AND COALESCE(reset_day_text, '1')::integer IN (
                    -1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10,
                    11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
                    21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31
                 )
            THEN COALESCE(reset_day_text, '1')::integer
            ELSE 1
        END AS reset_day,
        CASE
            WHEN COALESCE(reset_hour_text, '0') ~ '^[0-9]+$'
             AND COALESCE(reset_hour_text, '0')::integer BETWEEN 0 AND 23
            THEN COALESCE(reset_hour_text, '0')::integer
            ELSE 0
        END AS reset_hour
    FROM raw
), bounded AS (
    SELECT
        normalized.*,
        date_bin(
            interval '1 hour', as_of,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS completed_through
    FROM normalized
)
SELECT
    reset_day,
    reset_hour,
    CASE
        WHEN reset_day = -1 THEN NULL
        ELSE traffic_counter_cycle_start_utc(reset_day, reset_hour, as_of)
    END AS cycle_start,
    completed_through
FROM bounded
$$;



CREATE FUNCTION public.apply_traffic_counter_active_cycle_usage_deltas(changed_client_ids text[], changed_source_kinds text[], changed_interfaces text[], changed_bucket_starts timestamp with time zone[], changed_rx_bytes bigint[], changed_tx_bytes bigint[], changed_rx_reset_counts bigint[], changed_tx_reset_counts bigint[], as_of timestamp with time zone DEFAULT statement_timestamp()) RETURNS void
    LANGUAGE plpgsql
    AS $_$
DECLARE
    item_count INTEGER := COALESCE(array_length(changed_client_ids, 1), 0);
BEGIN
    IF item_count = 0 THEN
        RETURN;
    END IF;
    IF current_setting(
        'vpsman.traffic_explicit_hourly_reconstruction', true
    ) = 'on' THEN
        RETURN;
    END IF;
    IF array_length(changed_source_kinds, 1) IS DISTINCT FROM item_count
       OR array_length(changed_interfaces, 1) IS DISTINCT FROM item_count
       OR array_length(changed_bucket_starts, 1) IS DISTINCT FROM item_count
       OR array_length(changed_rx_bytes, 1) IS DISTINCT FROM item_count
       OR array_length(changed_tx_bytes, 1) IS DISTINCT FROM item_count
       OR array_length(changed_rx_reset_counts, 1) IS DISTINCT FROM item_count
       OR array_length(changed_tx_reset_counts, 1) IS DISTINCT FROM item_count
    THEN
        RAISE EXCEPTION 'traffic active-cycle delta arrays must have equal lengths';
    END IF;

    -- Every hourly/rollup trigger reaches the active prefix through this one
    -- boundary. Own only its changed stream rows in the same canonical order
    -- as minute publication and reset reconstruction before inspecting or
    -- mutating active state.
    PERFORM 1
    FROM traffic_counter_streams stream
    JOIN (
        SELECT DISTINCT
            changed.client_id, changed.source_kind, changed.interface
        FROM unnest(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS changed(client_id, source_kind, interface)
    ) changed
      ON changed.client_id = stream.client_id
     AND changed.source_kind = stream.source_kind
     AND changed.interface = stream.interface
    ORDER BY stream.client_id, stream.source_kind, stream.interface
    FOR UPDATE OF stream;

    -- The stream and active-cycle prefix are independent authorities. A delta
    -- statement may stage one stream revision, while the active prefix changes
    -- only when its completed-hour values or lifecycle boundary changes.
    -- Missing/damaged prefixes are never reconstructed incidentally. The only
    -- local initializations are a genuinely new stream or a clean UTC
    -- billing-cycle rollover.
    IF EXISTS (
        WITH changed_streams AS MATERIALIZED (
            SELECT DISTINCT client_id, source_kind, interface
            FROM unnest(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces
            ) AS changed(client_id, source_kind, interface)
        ), billing AS MATERIALIZED (
            SELECT changed.client_id, context.*
            FROM (
                SELECT DISTINCT client_id FROM changed_streams
            ) changed
            CROSS JOIN LATERAL traffic_counter_billing_context(
                changed.client_id, as_of
            ) context
        ), authority AS (
            SELECT
                billing.cycle_start,
                billing.completed_through,
                stream.source_revision,
                stream.materialized_revision,
                stream.sample_edge_revision,
                stream.promoted_boundary_safe,
                stream.usage_row_count,
                stream.latest_sample_observed_at,
                active.client_id AS active_client_id,
                active.cycle_start AS active_cycle_start,
                active.completed_through AS active_completed_through,
                active.source_revision AS active_source_revision,
                active.materialized_revision AS active_materialized_revision
            FROM changed_streams changed
            JOIN billing ON billing.client_id = changed.client_id
            JOIN traffic_counter_streams stream
              ON stream.client_id = changed.client_id
             AND stream.source_kind = changed.source_kind
             AND stream.interface = changed.interface
            LEFT JOIN traffic_counter_active_cycle_usage active
              ON active.client_id = changed.client_id
             AND active.source_kind = changed.source_kind
             AND active.interface = changed.interface
            WHERE billing.reset_day <> -1
        )
        SELECT 1
        FROM authority
        WHERE NOT (
            -- Ready prefix in this cycle. The stream may be in either half of
            -- the one-revision exact-repair publication.
            (
                active_client_id IS NOT NULL
                AND active_cycle_start = cycle_start
                AND active_completed_through <= completed_through
                AND active_source_revision = active_materialized_revision
                AND source_revision BETWEEN materialized_revision
                    AND materialized_revision + 1
                AND sample_edge_revision = materialized_revision
                AND promoted_boundary_safe
                AND latest_sample_observed_at <
                    active_completed_through + interval '1 hour'
            )
            OR
            -- First bounded publication for a genuinely empty stream.
            (
                active_client_id IS NULL
                AND source_revision = 1
                AND materialized_revision = 0
                AND sample_edge_revision = 0
                AND usage_row_count = 0
                AND latest_sample_observed_at IS NULL
            )
            OR
            -- Clean owner advancing into a new UTC billing cycle.
            (
                active_client_id IS NOT NULL
                AND active_cycle_start < cycle_start
                AND active_completed_through <= completed_through
                AND active_source_revision = active_materialized_revision
                AND source_revision BETWEEN materialized_revision
                    AND materialized_revision + 1
                AND sample_edge_revision = materialized_revision
                AND promoted_boundary_safe
                AND latest_sample_observed_at < cycle_start
            )
        )
    ) THEN
        RAISE EXCEPTION
            'traffic active-cycle delta encountered an unready authority'
            USING ERRCODE = 'PZ030';
    END IF;

    WITH raw_deltas AS MATERIALIZED (
        SELECT *
        FROM unnest(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_bucket_starts,
            changed_rx_bytes,
            changed_tx_bytes,
            changed_rx_reset_counts,
            changed_tx_reset_counts
        ) AS delta(
            client_id, source_kind, interface, bucket_start,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count
        )
    ), changed_streams AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM raw_deltas
    ), billing AS MATERIALIZED (
        SELECT changed.client_id, context.*
        FROM (
            SELECT DISTINCT client_id FROM changed_streams
        ) changed
        CROSS JOIN LATERAL traffic_counter_billing_context(
            changed.client_id, as_of
        ) context
    ), contexts AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            billing.cycle_start,
            billing.completed_through,
            stream.source_revision AS stream_source_revision,
            stream.materialized_revision AS stream_materialized_revision,
            stream.sample_edge_revision AS stream_sample_edge_revision,
            stream.promoted_boundary_safe,
            stream.usage_row_count AS stream_usage_row_count,
            stream.latest_sample_observed_at
        FROM changed_streams changed
        JOIN billing ON billing.client_id = changed.client_id
        JOIN traffic_counter_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
        WHERE billing.reset_day <> -1
    ), prior AS MATERIALIZED (
        SELECT
            context.*,
            active.cycle_start AS prior_cycle_start,
            active.completed_through AS prior_completed_through,
            active.rx_bytes AS prior_rx_bytes,
            active.tx_bytes AS prior_tx_bytes,
            active.rx_reset_count AS prior_rx_reset_count,
            active.tx_reset_count AS prior_tx_reset_count,
            active.source_revision AS prior_source_revision,
            active.materialized_revision AS prior_materialized_revision,
            COALESCE(active.source_revision + 1, 1) AS active_next_revision,
            active.client_id IS NOT NULL
                AND active.cycle_start = context.cycle_start
                AND active.completed_through <= context.completed_through
                AND active.source_revision = active.materialized_revision
                AND context.stream_source_revision BETWEEN
                    context.stream_materialized_revision
                    AND context.stream_materialized_revision + 1
                AND context.stream_sample_edge_revision =
                    context.stream_materialized_revision
                AND context.promoted_boundary_safe
                AND context.latest_sample_observed_at <
                    active.completed_through + interval '1 hour'
                AS incrementally_ready,
            active.client_id IS NULL
                AND context.stream_source_revision = 1
                AND context.stream_materialized_revision = 0
                AND context.stream_sample_edge_revision = 0
                AND context.stream_usage_row_count = 0
                AND context.latest_sample_observed_at IS NULL
                AS new_stream_initializable,
            (
                active.client_id IS NULL
                AND context.stream_source_revision = 1
                AND context.stream_materialized_revision = 0
                AND context.stream_sample_edge_revision = 0
                AND context.stream_usage_row_count = 0
                AND context.latest_sample_observed_at IS NULL
            ) OR (
                active.client_id IS NOT NULL
                AND active.cycle_start < context.cycle_start
                AND active.completed_through <= context.completed_through
                AND active.source_revision = active.materialized_revision
                AND context.stream_source_revision BETWEEN
                    context.stream_materialized_revision
                    AND context.stream_materialized_revision + 1
                AND context.stream_sample_edge_revision =
                    context.stream_materialized_revision
                AND context.promoted_boundary_safe
                AND context.latest_sample_observed_at < context.cycle_start
            ) AS lifecycle_initializable
        FROM contexts context
        LEFT JOIN traffic_counter_active_cycle_usage active
          ON active.client_id = context.client_id
         AND active.source_kind = context.source_kind
         AND active.interface = context.interface
    ), initial_totals AS MATERIALIZED (
        SELECT
            prior.client_id,
            prior.source_kind,
            prior.interface,
            prior.cycle_start,
            prior.completed_through,
            COALESCE(sum(delta.rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(sum(delta.tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(sum(delta.rx_reset_count), 0)::bigint
                AS rx_reset_count,
            COALESCE(sum(delta.tx_reset_count), 0)::bigint
                AS tx_reset_count,
            prior.active_next_revision
        FROM prior
        LEFT JOIN raw_deltas delta
          ON delta.client_id = prior.client_id
         AND delta.source_kind = prior.source_kind
         AND delta.interface = prior.interface
         AND delta.bucket_start >= prior.cycle_start
         AND delta.bucket_start < prior.completed_through
        WHERE prior.new_stream_initializable
        GROUP BY
            prior.client_id,
            prior.source_kind,
            prior.interface,
            prior.cycle_start,
            prior.completed_through,
            prior.active_next_revision
        UNION ALL
        SELECT
            prior.client_id,
            prior.source_kind,
            prior.interface,
            prior.cycle_start,
            prior.completed_through,
            usage.rx_bytes,
            usage.tx_bytes,
            usage.rx_reset_count,
            usage.tx_reset_count,
            prior.active_next_revision
        FROM prior
        CROSS JOIN LATERAL traffic_counter_completed_hour_usage(
            prior.client_id,
            prior.source_kind,
            prior.interface,
            prior.cycle_start,
            prior.completed_through
        ) usage
        WHERE prior.lifecycle_initializable
          AND NOT prior.new_stream_initializable
    ), initialized AS (
        INSERT INTO traffic_counter_active_cycle_usage (
            client_id, source_kind, interface,
            cycle_start, completed_through,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
            source_revision, materialized_revision, updated_at
        )
        SELECT
            client_id, source_kind, interface,
            cycle_start, completed_through,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
            active_next_revision, active_next_revision, clock_timestamp()
        FROM initial_totals
        ORDER BY client_id, source_kind, interface
        ON CONFLICT (client_id, source_kind, interface) DO UPDATE SET
            cycle_start = EXCLUDED.cycle_start,
            completed_through = EXCLUDED.completed_through,
            rx_bytes = EXCLUDED.rx_bytes,
            tx_bytes = EXCLUDED.tx_bytes,
            rx_reset_count = EXCLUDED.rx_reset_count,
            tx_reset_count = EXCLUDED.tx_reset_count,
            source_revision = EXCLUDED.source_revision,
            materialized_revision = EXCLUDED.materialized_revision,
            updated_at = EXCLUDED.updated_at
        RETURNING client_id, source_kind, interface
    ), closed_gap AS MATERIALIZED (
        SELECT
            prior.client_id,
            prior.source_kind,
            prior.interface,
            COALESCE(sum(hourly.rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(sum(hourly.tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(sum(hourly.rx_reset_count), 0)::bigint
                AS rx_reset_count,
            COALESCE(sum(hourly.tx_reset_count), 0)::bigint
                AS tx_reset_count
        FROM prior
        LEFT JOIN traffic_counter_hourly_usage hourly
          ON hourly.client_id = prior.client_id
         AND hourly.source_kind = prior.source_kind
         AND hourly.interface = prior.interface
         AND hourly.bucket_start >= prior.prior_completed_through
         AND hourly.bucket_start < prior.completed_through
        WHERE prior.incrementally_ready
        GROUP BY prior.client_id, prior.source_kind, prior.interface
    ), prior_range_deltas AS MATERIALIZED (
        SELECT
            prior.client_id,
            prior.source_kind,
            prior.interface,
            COALESCE(sum(delta.rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(sum(delta.tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(sum(delta.rx_reset_count), 0)::bigint
                AS rx_reset_count,
            COALESCE(sum(delta.tx_reset_count), 0)::bigint
                AS tx_reset_count
        FROM prior
        LEFT JOIN raw_deltas delta
          ON delta.client_id = prior.client_id
         AND delta.source_kind = prior.source_kind
         AND delta.interface = prior.interface
         AND delta.bucket_start >= prior.cycle_start
         AND delta.bucket_start < prior.prior_completed_through
        WHERE prior.incrementally_ready
        GROUP BY prior.client_id, prior.source_kind, prior.interface
    )
    UPDATE traffic_counter_active_cycle_usage active
    SET
        completed_through = prior.completed_through,
        rx_bytes = prior.prior_rx_bytes
            + gap.rx_bytes + delta.rx_bytes,
        tx_bytes = prior.prior_tx_bytes
            + gap.tx_bytes + delta.tx_bytes,
        rx_reset_count = prior.prior_rx_reset_count
            + gap.rx_reset_count + delta.rx_reset_count,
        tx_reset_count = prior.prior_tx_reset_count
            + gap.tx_reset_count + delta.tx_reset_count,
        source_revision = prior.prior_source_revision + 1,
        materialized_revision = prior.prior_source_revision + 1,
        updated_at = clock_timestamp()
    FROM prior
    JOIN closed_gap gap
      ON gap.client_id = prior.client_id
     AND gap.source_kind = prior.source_kind
     AND gap.interface = prior.interface
    JOIN prior_range_deltas delta
      ON delta.client_id = prior.client_id
     AND delta.source_kind = prior.source_kind
     AND delta.interface = prior.interface
    WHERE prior.incrementally_ready
      AND active.client_id = prior.client_id
      AND active.source_kind = prior.source_kind
      AND active.interface = prior.interface
      AND active.source_revision = prior.prior_source_revision
      AND active.materialized_revision = prior.prior_materialized_revision
      AND (
          active.completed_through IS DISTINCT FROM prior.completed_through
          OR active.rx_bytes IS DISTINCT FROM
                prior.prior_rx_bytes + gap.rx_bytes + delta.rx_bytes
          OR active.tx_bytes IS DISTINCT FROM
                prior.prior_tx_bytes + gap.tx_bytes + delta.tx_bytes
          OR active.rx_reset_count IS DISTINCT FROM
                prior.prior_rx_reset_count
                    + gap.rx_reset_count + delta.rx_reset_count
          OR active.tx_reset_count IS DISTINCT FROM
                prior.prior_tx_reset_count
                    + gap.tx_reset_count + delta.tx_reset_count
      );
END;
$_$;



CREATE FUNCTION public.apply_traffic_counter_rollup_summary_deltas(changed_client_ids text[], changed_source_kinds text[], changed_interfaces text[], changed_origin_kinds text[], changed_bucket_secs integer[], changed_rx_bytes bigint[], changed_tx_bytes bigint[], changed_rx_reset_counts bigint[], changed_tx_reset_counts bigint[], changed_row_counts bigint[]) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
    changed_count INTEGER := cardinality(changed_client_ids);
BEGIN
    IF COALESCE(changed_count, 0) = 0 THEN
        RETURN;
    END IF;
    IF cardinality(changed_source_kinds) IS DISTINCT FROM changed_count
       OR cardinality(changed_interfaces) IS DISTINCT FROM changed_count
       OR cardinality(changed_origin_kinds) IS DISTINCT FROM changed_count
       OR cardinality(changed_bucket_secs) IS DISTINCT FROM changed_count
       OR cardinality(changed_rx_bytes) IS DISTINCT FROM changed_count
       OR cardinality(changed_tx_bytes) IS DISTINCT FROM changed_count
       OR cardinality(changed_rx_reset_counts) IS DISTINCT FROM changed_count
       OR cardinality(changed_tx_reset_counts) IS DISTINCT FROM changed_count
       OR cardinality(changed_row_counts) IS DISTINCT FROM changed_count THEN
        RAISE EXCEPTION 'traffic rollup summary arrays must have equal lengths';
    END IF;
    IF EXISTS (
        SELECT 1
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_origin_kinds,
            changed_bucket_secs,
            changed_rx_bytes,
            changed_tx_bytes,
            changed_rx_reset_counts,
            changed_tx_reset_counts,
            changed_row_counts
        ) AS changed(
            client_id, source_kind, interface, origin_kind, bucket_secs,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count, row_count
        )
        GROUP BY
            changed.client_id, changed.source_kind, changed.interface,
            changed.origin_kind, changed.bucket_secs
        HAVING count(*) <> 1
    ) THEN
        RAISE EXCEPTION 'traffic rollup summary tiers must be unique';
    END IF;

    -- Cascading client deletion removes both summary tables. Do not recreate
    -- an authority whose owner is already absent in this transaction. The
    -- ordered source is also the lock order for multi-stream reimports.
    INSERT INTO traffic_counter_rollup_summary_streams (
        client_id, source_kind, interface,
        source_revision, materialized_revision,
        rollup_row_count, tier_count, updated_at
    )
    SELECT DISTINCT
        changed.client_id, changed.source_kind, changed.interface,
        1, 0, 0, 0, now()
    FROM UNNEST(
        changed_client_ids,
        changed_source_kinds,
        changed_interfaces
    ) AS changed(client_id, source_kind, interface)
    JOIN clients client ON client.id = changed.client_id
    ORDER BY changed.client_id, changed.source_kind, changed.interface
    ON CONFLICT (client_id, source_kind, interface) DO UPDATE SET
        source_revision =
            traffic_counter_rollup_summary_streams.source_revision + 1,
        updated_at = now();

    -- Reject arithmetic underflow, incomplete final deletion, or a negative
    -- replacement before constraints can obscure which authority was bad.
    IF EXISTS (
        WITH changed AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_origin_kinds,
                changed_bucket_secs,
                changed_rx_bytes,
                changed_tx_bytes,
                changed_rx_reset_counts,
                changed_tx_reset_counts,
                changed_row_counts
            ) AS item(
                client_id, source_kind, interface, origin_kind, bucket_secs,
                rx_bytes, tx_bytes, rx_reset_count, tx_reset_count, row_count
            )
        )
        SELECT 1
        FROM changed
        JOIN clients client ON client.id = changed.client_id
        JOIN traffic_counter_rollup_tier_summaries summary
          ON summary.client_id = changed.client_id
         AND summary.source_kind = changed.source_kind
         AND summary.interface = changed.interface
         AND summary.origin_kind = changed.origin_kind
         AND summary.bucket_secs = changed.bucket_secs
        WHERE summary.rollup_row_count + changed.row_count < 0
           OR summary.rx_bytes + changed.rx_bytes < 0
           OR summary.tx_bytes + changed.tx_bytes < 0
           OR summary.rx_reset_count + changed.rx_reset_count < 0
           OR summary.tx_reset_count + changed.tx_reset_count < 0
           OR (
                summary.rollup_row_count + changed.row_count = 0
                AND (
                    summary.rx_bytes + changed.rx_bytes <> 0
                    OR summary.tx_bytes + changed.tx_bytes <> 0
                    OR summary.rx_reset_count + changed.rx_reset_count <> 0
                    OR summary.tx_reset_count + changed.tx_reset_count <> 0
                )
           )
        LIMIT 1
    ) THEN
        RAISE EXCEPTION 'traffic rollup summary delta would be invalid';
    END IF;
    IF EXISTS (
        WITH changed AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_origin_kinds,
                changed_bucket_secs,
                changed_rx_bytes,
                changed_tx_bytes,
                changed_rx_reset_counts,
                changed_tx_reset_counts,
                changed_row_counts
            ) AS item(
                client_id, source_kind, interface, origin_kind, bucket_secs,
                rx_bytes, tx_bytes, rx_reset_count, tx_reset_count, row_count
            )
        )
        SELECT 1
        FROM changed
        JOIN clients client ON client.id = changed.client_id
        LEFT JOIN traffic_counter_rollup_tier_summaries summary
          ON summary.client_id = changed.client_id
         AND summary.source_kind = changed.source_kind
         AND summary.interface = changed.interface
         AND summary.origin_kind = changed.origin_kind
         AND summary.bucket_secs = changed.bucket_secs
        WHERE summary.client_id IS NULL
          AND (
                changed.row_count <= 0
                OR changed.rx_bytes < 0
                OR changed.tx_bytes < 0
                OR changed.rx_reset_count < 0
                OR changed.tx_reset_count < 0
          )
        LIMIT 1
    ) THEN
        RAISE EXCEPTION 'traffic rollup summary delta has no prior tier';
    END IF;

    -- Remove tiers whose last rows disappeared before applying positive-row
    -- updates, because the table deliberately forbids a zero-row tier.
    WITH changed AS MATERIALIZED (
        SELECT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_origin_kinds,
            changed_bucket_secs,
            changed_rx_bytes,
            changed_tx_bytes,
            changed_rx_reset_counts,
            changed_tx_reset_counts,
            changed_row_counts
        ) AS item(
            client_id, source_kind, interface, origin_kind, bucket_secs,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count, row_count
        )
    )
    DELETE FROM traffic_counter_rollup_tier_summaries summary
    USING changed
    WHERE summary.client_id = changed.client_id
      AND summary.source_kind = changed.source_kind
      AND summary.interface = changed.interface
      AND summary.origin_kind = changed.origin_kind
      AND summary.bucket_secs = changed.bucket_secs
      AND summary.rollup_row_count + changed.row_count = 0;

    WITH changed AS MATERIALIZED (
        SELECT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_origin_kinds,
            changed_bucket_secs,
            changed_rx_bytes,
            changed_tx_bytes,
            changed_rx_reset_counts,
            changed_tx_reset_counts,
            changed_row_counts
        ) AS item(
            client_id, source_kind, interface, origin_kind, bucket_secs,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count, row_count
        )
    )
    UPDATE traffic_counter_rollup_tier_summaries summary
    SET rx_bytes = summary.rx_bytes + changed.rx_bytes,
        tx_bytes = summary.tx_bytes + changed.tx_bytes,
        rx_reset_count = summary.rx_reset_count + changed.rx_reset_count,
        tx_reset_count = summary.tx_reset_count + changed.tx_reset_count,
        rollup_row_count = summary.rollup_row_count + changed.row_count,
        updated_at = now()
    FROM changed
    WHERE summary.client_id = changed.client_id
      AND summary.source_kind = changed.source_kind
      AND summary.interface = changed.interface
      AND summary.origin_kind = changed.origin_kind
      AND summary.bucket_secs = changed.bucket_secs
      AND summary.rollup_row_count + changed.row_count > 0;

    WITH changed AS MATERIALIZED (
        SELECT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_origin_kinds,
            changed_bucket_secs,
            changed_rx_bytes,
            changed_tx_bytes,
            changed_rx_reset_counts,
            changed_tx_reset_counts,
            changed_row_counts
        ) AS item(
            client_id, source_kind, interface, origin_kind, bucket_secs,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count, row_count
        )
    )
    INSERT INTO traffic_counter_rollup_tier_summaries (
        client_id, source_kind, interface, origin_kind, bucket_secs,
        first_bucket_start, latest_bucket_start, last_bucket_end,
        rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
        rollup_row_count, materialized_revision, updated_at
    )
    SELECT
        changed.client_id, changed.source_kind, changed.interface,
        changed.origin_kind, changed.bucket_secs,
        first_row.bucket_start, last_row.bucket_start,
        last_row.bucket_start
            + make_interval(secs => changed.bucket_secs),
        changed.rx_bytes, changed.tx_bytes,
        changed.rx_reset_count, changed.tx_reset_count,
        changed.row_count, stream.source_revision, now()
    FROM changed
    JOIN clients client ON client.id = changed.client_id
    JOIN traffic_counter_rollup_summary_streams stream
      ON stream.client_id = changed.client_id
     AND stream.source_kind = changed.source_kind
     AND stream.interface = changed.interface
    CROSS JOIN LATERAL (
        SELECT rollup.bucket_start
        FROM traffic_counter_rollups rollup
        WHERE rollup.client_id = changed.client_id
          AND rollup.source_kind = changed.source_kind
          AND rollup.interface = changed.interface
          AND rollup.origin_kind = changed.origin_kind
          AND rollup.bucket_secs = changed.bucket_secs
        ORDER BY rollup.bucket_start
        LIMIT 1
    ) first_row
    CROSS JOIN LATERAL (
        SELECT rollup.bucket_start
        FROM traffic_counter_rollups rollup
        WHERE rollup.client_id = changed.client_id
          AND rollup.source_kind = changed.source_kind
          AND rollup.interface = changed.interface
          AND rollup.origin_kind = changed.origin_kind
          AND rollup.bucket_secs = changed.bucket_secs
        ORDER BY rollup.bucket_start DESC
        LIMIT 1
    ) last_row
    LEFT JOIN traffic_counter_rollup_tier_summaries existing
      ON existing.client_id = changed.client_id
     AND existing.source_kind = changed.source_kind
     AND existing.interface = changed.interface
     AND existing.origin_kind = changed.origin_kind
     AND existing.bucket_secs = changed.bucket_secs
    WHERE existing.client_id IS NULL;

    -- Boundary probes are two LIMIT 1 primary-key walks per changed tier,
    -- independent of the number or age of retained rollups.
    WITH changed AS MATERIALIZED (
        SELECT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_origin_kinds,
            changed_bucket_secs
        ) AS item(
            client_id, source_kind, interface, origin_kind, bucket_secs
        )
    ), boundaries AS MATERIALIZED (
        SELECT
            changed.*,
            first_row.bucket_start AS first_bucket_start,
            last_row.bucket_start AS latest_bucket_start
        FROM changed
        CROSS JOIN LATERAL (
            SELECT rollup.bucket_start
            FROM traffic_counter_rollups rollup
            WHERE rollup.client_id = changed.client_id
              AND rollup.source_kind = changed.source_kind
              AND rollup.interface = changed.interface
              AND rollup.origin_kind = changed.origin_kind
              AND rollup.bucket_secs = changed.bucket_secs
            ORDER BY rollup.bucket_start
            LIMIT 1
        ) first_row
        CROSS JOIN LATERAL (
            SELECT rollup.bucket_start
            FROM traffic_counter_rollups rollup
            WHERE rollup.client_id = changed.client_id
              AND rollup.source_kind = changed.source_kind
              AND rollup.interface = changed.interface
              AND rollup.origin_kind = changed.origin_kind
              AND rollup.bucket_secs = changed.bucket_secs
            ORDER BY rollup.bucket_start DESC
            LIMIT 1
        ) last_row
    )
    UPDATE traffic_counter_rollup_tier_summaries summary
    SET first_bucket_start = boundaries.first_bucket_start,
        latest_bucket_start = boundaries.latest_bucket_start,
        last_bucket_end = boundaries.latest_bucket_start
            + make_interval(secs => boundaries.bucket_secs),
        updated_at = now()
    FROM boundaries
    WHERE summary.client_id = boundaries.client_id
      AND summary.source_kind = boundaries.source_kind
      AND summary.interface = boundaries.interface
      AND summary.origin_kind = boundaries.origin_kind
      AND summary.bucket_secs = boundaries.bucket_secs;

    -- A surviving base tier and its bounded authority must remain 1:1. This
    -- catches unsupported direct summary tampering without a history scan.
    IF EXISTS (
        WITH changed AS MATERIALIZED (
            SELECT *
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_origin_kinds,
                changed_bucket_secs
            ) AS item(
                client_id, source_kind, interface, origin_kind, bucket_secs
            )
        )
        SELECT 1
        FROM changed
        JOIN clients client ON client.id = changed.client_id
        LEFT JOIN traffic_counter_rollup_tier_summaries summary
          ON summary.client_id = changed.client_id
         AND summary.source_kind = changed.source_kind
         AND summary.interface = changed.interface
         AND summary.origin_kind = changed.origin_kind
         AND summary.bucket_secs = changed.bucket_secs
        WHERE (summary.client_id IS NULL) IS DISTINCT FROM NOT EXISTS (
            SELECT 1
            FROM traffic_counter_rollups rollup
            WHERE rollup.client_id = changed.client_id
              AND rollup.source_kind = changed.source_kind
              AND rollup.interface = changed.interface
              AND rollup.origin_kind = changed.origin_kind
              AND rollup.bucket_secs = changed.bucket_secs
            LIMIT 1
        )
        LIMIT 1
    ) THEN
        RAISE EXCEPTION 'traffic rollup tier authority disagrees with base rows';
    END IF;

    -- Advancing one stream revision makes every one of its at-most-eight tier
    -- rows part of the same exact materialization fence.
    WITH changed_streams AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS item(client_id, source_kind, interface)
    )
    UPDATE traffic_counter_rollup_tier_summaries summary
    SET materialized_revision = stream.source_revision,
        updated_at = now()
    FROM changed_streams changed
    JOIN traffic_counter_rollup_summary_streams stream
      ON stream.client_id = changed.client_id
     AND stream.source_kind = changed.source_kind
     AND stream.interface = changed.interface
    WHERE summary.client_id = changed.client_id
      AND summary.source_kind = changed.source_kind
      AND summary.interface = changed.interface;

    WITH changed_streams AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS item(client_id, source_kind, interface)
    ), summary_counts AS (
        SELECT
            changed.client_id, changed.source_kind, changed.interface,
            COALESCE(sum(summary.rollup_row_count), 0)::BIGINT
                AS rollup_row_count,
            count(summary.bucket_secs)::INTEGER AS tier_count
        FROM changed_streams changed
        JOIN traffic_counter_rollup_summary_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
        LEFT JOIN traffic_counter_rollup_tier_summaries summary
          ON summary.client_id = changed.client_id
         AND summary.source_kind = changed.source_kind
         AND summary.interface = changed.interface
        GROUP BY changed.client_id, changed.source_kind, changed.interface
    )
    UPDATE traffic_counter_rollup_summary_streams stream
    SET materialized_revision = stream.source_revision,
        rollup_row_count = summary.rollup_row_count,
        tier_count = summary.tier_count,
        updated_at = now()
    FROM summary_counts summary
    WHERE stream.client_id = summary.client_id
      AND stream.source_kind = summary.source_kind
      AND stream.interface = summary.interface;

    -- No base rollup stream needs an empty authority. Removing it bounds the
    -- registry under interface churn; a concurrent insert has already queued
    -- behind this deterministically locked row and will create it again.
    WITH changed_streams AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS item(client_id, source_kind, interface)
    )
    DELETE FROM traffic_counter_rollup_summary_streams stream
    USING changed_streams changed
    WHERE stream.client_id = changed.client_id
      AND stream.source_kind = changed.source_kind
      AND stream.interface = changed.interface
      AND stream.rollup_row_count = 0
      AND stream.tier_count = 0;
END;
$$;



CREATE FUNCTION public.maintain_traffic_counter_active_cycle_after_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    PERFORM apply_traffic_counter_active_cycle_usage_deltas(
        array_agg(row.client_id ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.source_kind ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.interface ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.bucket_start ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.rx_bytes::bigint ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.tx_bytes::bigint ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.rx_reset_count::bigint ORDER BY row.client_id,
                  row.source_kind, row.interface, row.bucket_start),
        array_agg(-row.tx_reset_count::bigint ORDER BY row.client_id,
                  row.source_kind, row.interface, row.bucket_start)
    )
    FROM old_traffic_counter_hourly_usage row;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.maintain_traffic_counter_active_cycle_after_insert() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- The proven live path publishes the hourly row and stream totals itself.
    -- A rollover also closes the prior hour in the active prefix, while a
    -- same-hour append deliberately leaves that independent owner unchanged.
    -- Do not execute the generic delta graph a second time.
    IF current_setting(
        'vpsman.traffic_hourly_derivations_prepublished', true
    ) = 'on' THEN
        RETURN NULL;
    END IF;

    PERFORM apply_traffic_counter_active_cycle_usage_deltas(
        array_agg(row.client_id ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.source_kind ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.interface ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.bucket_start ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.rx_bytes::bigint ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.tx_bytes::bigint ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.rx_reset_count::bigint ORDER BY row.client_id,
                  row.source_kind, row.interface, row.bucket_start),
        array_agg(row.tx_reset_count::bigint ORDER BY row.client_id,
                  row.source_kind, row.interface, row.bucket_start)
    )
    FROM new_traffic_counter_hourly_usage row;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.maintain_traffic_counter_active_cycle_after_update() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    -- The authenticated newest-sample replacement publishes the stream totals
    -- and open hourly row itself.  It cannot change a completed-hour prefix,
    -- so the independent active owner is deliberately left untouched and the
    -- generic delta graph must not run a second time.
    IF current_setting(
        'vpsman.traffic_hourly_derivations_prepublished', true
    ) = 'on' THEN
        RETURN NULL;
    END IF;

    PERFORM apply_traffic_counter_active_cycle_usage_deltas(
        array_agg(row.client_id ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.source_kind ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.interface ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.bucket_start ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.rx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.tx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.rx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign),
        array_agg(row.tx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start, row.sign)
    )
    FROM (
        SELECT
            old_row.client_id,
            old_row.source_kind,
            old_row.interface,
            old_row.bucket_start,
            -old_row.rx_bytes::bigint AS rx_bytes,
            -old_row.tx_bytes::bigint AS tx_bytes,
            -old_row.rx_reset_count::bigint AS rx_reset_count,
            -old_row.tx_reset_count::bigint AS tx_reset_count,
            0 AS sign
        FROM old_traffic_counter_hourly_usage old_row
        UNION ALL
        SELECT
            new_row.client_id,
            new_row.source_kind,
            new_row.interface,
            new_row.bucket_start,
            new_row.rx_bytes::bigint,
            new_row.tx_bytes::bigint,
            new_row.rx_reset_count::bigint,
            new_row.tx_reset_count::bigint,
            1 AS sign
        FROM new_traffic_counter_hourly_usage new_row
    ) row;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_active_cycle_after_rule_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    changed_clients TEXT[];
BEGIN
    SELECT array_agg(DISTINCT changed.client_id ORDER BY changed.client_id)
    INTO changed_clients
    FROM (
        SELECT OLD.client_id
        WHERE TG_OP IN ('UPDATE', 'DELETE')
          AND OLD.key = 'traffic.reset_day'
        UNION ALL
        SELECT NEW.client_id
        WHERE TG_OP IN ('INSERT', 'UPDATE')
          AND NEW.key = 'traffic.reset_day'
    ) changed;

    IF COALESCE(array_length(changed_clients, 1), 0) = 0 THEN
        RETURN NULL;
    END IF;

    -- A rule write owns only the durable request. Reconstructing retained
    -- hourly usage belongs to the worker consumer and must never extend this
    -- producer transaction or its row locks.
    INSERT INTO traffic_counter_active_cycle_rebuild_work (
        client_id,
        requested_revision,
        materialized_revision,
        requested_at,
        next_attempt_at,
        updated_at
    )
    -- A client cascade deletes its rules after the parent is no longer live;
    -- that teardown must not manufacture worker-owned reconstruction work.
    SELECT changed.client_id, 1, 0, clock_timestamp(), now(), now()
    FROM unnest(changed_clients) AS changed(client_id)
    JOIN clients client ON client.id = changed.client_id
    ON CONFLICT (client_id) DO UPDATE SET
        requested_revision =
            traffic_counter_active_cycle_rebuild_work.requested_revision + 1,
        requested_at = EXCLUDED.requested_at,
        next_attempt_at = LEAST(
            traffic_counter_active_cycle_rebuild_work.next_attempt_at,
            EXCLUDED.next_attempt_at
        ),
        updated_at = EXCLUDED.updated_at;

    PERFORM pg_notify('vpsman_traffic_active_cycle_rebuild', 'ready');
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_active_cycle_usage(target_client_ids text[], as_of timestamp with time zone DEFAULT statement_timestamp()) RETURNS void
    LANGUAGE plpgsql
    AS $_$
BEGIN
    IF COALESCE(array_length(target_client_ids, 1), 0) = 0 THEN
        RETURN;
    END IF;

    -- The minute publisher, explicit imports, and reset-rule reconstruction
    -- may all publish the same stream summaries. Own exactly the targeted
    -- stream rows in their shared canonical order before inspecting readiness
    -- or replacing the active-cycle projection. There is deliberately no
    -- client-wide or fleet-wide lock here.
    PERFORM 1
    FROM traffic_counter_streams stream
    JOIN (
        SELECT DISTINCT target.client_id
        FROM unnest(target_client_ids) AS target(client_id)
        JOIN clients client ON client.id = target.client_id
    ) target ON target.client_id = stream.client_id
    ORDER BY stream.client_id, stream.source_kind, stream.interface
    FOR UPDATE OF stream;

    -- This function owns only the active-cycle cache. It may consume a fully
    -- published hourly/edge stream, but it must never bless or repair either
    -- upstream authority as a side effect.
    IF EXISTS (
        WITH target_clients AS MATERIALIZED (
            SELECT DISTINCT target.client_id
            FROM unnest(target_client_ids) AS target(client_id)
            JOIN clients client ON client.id = target.client_id
        )
        SELECT 1
        FROM target_clients target
        JOIN traffic_counter_streams stream
          ON stream.client_id = target.client_id
        WHERE stream.source_revision <> stream.materialized_revision
           OR stream.sample_edge_revision <> stream.materialized_revision
           OR NOT stream.promoted_boundary_safe
    ) THEN
        RAISE EXCEPTION
            'traffic active-cycle refresh encountered an unready stream authority'
            USING ERRCODE = 'PZ030';
    END IF;

    -- A no-reset client uses the independent retained-history summary and
    -- therefore owns no monthly-cycle prefix row.
    WITH target_clients AS MATERIALIZED (
        SELECT DISTINCT target.client_id
        FROM unnest(target_client_ids) AS target(client_id)
        JOIN clients client ON client.id = target.client_id
    ), billing AS MATERIALIZED (
        SELECT target.client_id, context.*
        FROM target_clients target
        CROSS JOIN LATERAL traffic_counter_billing_context(
            target.client_id, as_of
        ) context
    )
    DELETE FROM traffic_counter_active_cycle_usage active
    USING billing
    WHERE active.client_id = billing.client_id
      AND billing.reset_day = -1;

    WITH target_clients AS MATERIALIZED (
        SELECT DISTINCT target.client_id
        FROM unnest(target_client_ids) AS target(client_id)
        JOIN clients client ON client.id = target.client_id
    ), cycles AS MATERIALIZED (
        SELECT target.client_id, context.cycle_start,
               context.completed_through
        FROM target_clients target
        CROSS JOIN LATERAL traffic_counter_billing_context(
            target.client_id, as_of
        ) context
        WHERE context.reset_day <> -1
    ), rebuilt AS (
        SELECT
            stream.client_id,
            stream.source_kind,
            stream.interface,
            cycle.cycle_start,
            cycle.completed_through,
            usage.rx_bytes,
            usage.tx_bytes,
            usage.rx_reset_count,
            usage.tx_reset_count
        FROM cycles cycle
        JOIN traffic_counter_streams stream
          ON stream.client_id = cycle.client_id
        CROSS JOIN LATERAL traffic_counter_completed_hour_usage(
            stream.client_id,
            stream.source_kind,
            stream.interface,
            cycle.cycle_start,
            cycle.completed_through
        ) usage
    )
    INSERT INTO traffic_counter_active_cycle_usage (
        client_id, source_kind, interface,
        cycle_start, completed_through,
        rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
        source_revision, materialized_revision, updated_at
    )
    SELECT
        client_id, source_kind, interface,
            cycle_start, completed_through,
            rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
            1, 1, clock_timestamp()
    FROM rebuilt
    ORDER BY client_id, source_kind, interface
    ON CONFLICT (client_id, source_kind, interface) DO UPDATE SET
        cycle_start = EXCLUDED.cycle_start,
        completed_through = EXCLUDED.completed_through,
        rx_bytes = EXCLUDED.rx_bytes,
        tx_bytes = EXCLUDED.tx_bytes,
        rx_reset_count = EXCLUDED.rx_reset_count,
        tx_reset_count = EXCLUDED.tx_reset_count,
        source_revision = traffic_counter_active_cycle_usage.source_revision + 1,
        materialized_revision =
            traffic_counter_active_cycle_usage.source_revision + 1,
        updated_at = EXCLUDED.updated_at;
END;
$_$;



CREATE FUNCTION public.refresh_traffic_counter_hourly_usage(changed_client_ids text[], changed_source_kinds text[], changed_interfaces text[], changed_observed_at timestamp with time zone[], rebuild_entire_streams boolean DEFAULT false) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
    dirty_client_ids TEXT[];
    dirty_source_kinds TEXT[];
    dirty_interfaces TEXT[];
    dirty_observed_at TIMESTAMPTZ[];
    clean_client_ids TEXT[];
    clean_source_kinds TEXT[];
    clean_interfaces TEXT[];
    clean_observed_at TIMESTAMPTZ[];
BEGIN
    IF COALESCE(array_length(changed_client_ids, 1), 0) = 0 THEN
        RETURN;
    END IF;
    IF array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_source_kinds, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_interfaces, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_observed_at, 1) THEN
        RAISE EXCEPTION 'traffic hourly refresh arrays must have equal lengths';
    END IF;

    -- Whole-stream reconstruction is an explicit repair/import operation. If
    -- its marker is absent, seed the cumulative ledger before the exact core
    -- deletes it so the DELETE delta cannot underflow. Ordinary arrivals and
    -- retention are never allowed to enter this retained-history path.
    IF rebuild_entire_streams AND EXISTS (
        SELECT 1
        FROM (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces
            ) AS item(client_id, source_kind, interface)
        ) changed
        JOIN clients ON clients.id = changed.client_id
        LEFT JOIN traffic_counter_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
        WHERE stream.client_id IS NULL
    ) THEN
    WITH changed AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS item(client_id, source_kind, interface)
    ), missing AS MATERIALIZED (
        SELECT changed.*
        FROM changed
        JOIN clients ON clients.id = changed.client_id
        LEFT JOIN traffic_counter_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
        WHERE stream.client_id IS NULL
    ), usage AS (
        SELECT
            missing.client_id,
            missing.source_kind,
            missing.interface,
            COALESCE(SUM(hourly.rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(SUM(hourly.tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(SUM(hourly.rx_reset_count), 0)::bigint
                AS rx_reset_count,
            COALESCE(SUM(hourly.tx_reset_count), 0)::bigint
                AS tx_reset_count,
            COUNT(hourly.bucket_start)::bigint AS row_count
        FROM missing
        LEFT JOIN traffic_counter_hourly_usage hourly
          ON hourly.client_id = missing.client_id
         AND hourly.source_kind = missing.source_kind
         AND hourly.interface = missing.interface
        GROUP BY missing.client_id, missing.source_kind, missing.interface
    )
    INSERT INTO traffic_counter_streams (
        client_id,
        source_kind,
        interface,
        source_revision,
        materialized_revision,
        usage_rx_bytes,
        usage_tx_bytes,
        usage_rx_reset_count,
        usage_tx_reset_count,
        usage_row_count,
        updated_at
    )
    SELECT
        usage.client_id,
        usage.source_kind,
        usage.interface,
        1,
        0,
        usage.rx_bytes,
        usage.tx_bytes,
        usage.rx_reset_count,
        usage.tx_reset_count,
        usage.row_count,
        now()
    FROM usage
    ORDER BY usage.client_id, usage.source_kind, usage.interface
    ON CONFLICT (client_id, source_kind, interface) DO NOTHING;
    END IF;

    IF rebuild_entire_streams THEN
        PERFORM refresh_traffic_counter_hourly_usage_exact_core(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at,
            TRUE
        );
        RETURN;
    END IF;

    -- A genuinely new stream has neither a marker nor derived hourly rows.
    -- Establish its empty revision fence; the bounded changed-hour core below
    -- publishes revision one. A missing marker beside an existing ledger is
    -- corruption, not permission to scan and reconstruct retained history.
    INSERT INTO traffic_counter_streams (
        client_id, source_kind, interface,
        source_revision, materialized_revision, updated_at
    )
    SELECT DISTINCT
        changed.client_id, changed.source_kind, changed.interface,
        0, 0, now()
    FROM UNNEST(
        changed_client_ids,
        changed_source_kinds,
        changed_interfaces
    ) AS changed(client_id, source_kind, interface)
    JOIN clients client ON client.id = changed.client_id
    WHERE NOT EXISTS (
        SELECT 1
        FROM traffic_counter_hourly_usage hourly
        WHERE hourly.client_id = changed.client_id
          AND hourly.source_kind = changed.source_kind
          AND hourly.interface = changed.interface
    )
    ORDER BY changed.client_id, changed.source_kind, changed.interface
    ON CONFLICT (client_id, source_kind, interface) DO NOTHING;

    WITH classified AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            changed.observed_at,
            changed.ordinality,
            stream.client_id IS NULL
                OR stream.source_revision <> stream.materialized_revision
                OR (
                    NOT (
                        stream.source_revision = 0
                        AND stream.materialized_revision = 0
                        AND stream.sample_edge_revision = 0
                        AND stream.usage_row_count = 0
                        AND stream.latest_sample_observed_at IS NULL
                    )
                    AND (
                        stream.sample_edge_revision <>
                            stream.materialized_revision
                        OR NOT stream.promoted_boundary_safe
                    )
                )
                AS dirty
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) WITH ORDINALITY AS changed(
            client_id, source_kind, interface, observed_at, ordinality
        )
        JOIN clients client ON client.id = changed.client_id
        LEFT JOIN traffic_counter_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
    )
    SELECT
        array_agg(client_id ORDER BY ordinality) FILTER (WHERE dirty),
        array_agg(source_kind ORDER BY ordinality) FILTER (WHERE dirty),
        array_agg(interface ORDER BY ordinality) FILTER (WHERE dirty),
        array_agg(observed_at ORDER BY ordinality) FILTER (WHERE dirty),
        array_agg(client_id ORDER BY ordinality) FILTER (WHERE NOT dirty),
        array_agg(source_kind ORDER BY ordinality) FILTER (WHERE NOT dirty),
        array_agg(interface ORDER BY ordinality) FILTER (WHERE NOT dirty),
        array_agg(observed_at ORDER BY ordinality) FILTER (WHERE NOT dirty)
    INTO
        dirty_client_ids,
        dirty_source_kinds,
        dirty_interfaces,
        dirty_observed_at,
        clean_client_ids,
        clean_source_kinds,
        clean_interfaces,
        clean_observed_at
    FROM classified;

    IF COALESCE(array_length(dirty_client_ids, 1), 0) > 0 THEN
        RAISE EXCEPTION
            'traffic hourly refresh encountered a missing or dirty stream authority'
            USING ERRCODE = 'PZ029';
    END IF;
    IF COALESCE(array_length(clean_client_ids, 1), 0) > 0 THEN
        PERFORM refresh_traffic_counter_hourly_usage_exact_core(
            clean_client_ids,
            clean_source_kinds,
            clean_interfaces,
            clean_observed_at,
            FALSE
        );
    END IF;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_hourly_usage_after_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
BEGIN
    IF current_setting(
        'vpsman.traffic_retention_hourly_delete_managed', true
    ) = 'on' THEN
        RETURN NULL;
    END IF;

    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(source_kind ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(interface ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(observed_at ORDER BY client_id, source_kind, interface, observed_at)
    INTO client_ids, source_kinds, interfaces, observed_values
    FROM old_traffic_counter_samples;
    PERFORM refresh_traffic_counter_hourly_usage(
        client_ids, source_kinds, interfaces, observed_values
    );
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_hourly_usage_after_insert() RETURNS trigger
    LANGUAGE plpgsql
    SET jit TO 'off'
    AS $_$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
    changed_count BIGINT;
    changed_stream_count BIGINT;
    fast_count BIGINT;
    exact_count BIGINT;
    same_hour_count BIGINT;
    rollover_count BIGINT;
    updated_stream_count BIGINT;
    updated_same_hour_count BIGINT;
    inserted_rollover_count BIGINT;
    expected_active_count BIGINT;
    updated_active_count BIGINT;
    owner_client_count BIGINT;
    fast_path BOOLEAN := FALSE;
BEGIN
    IF NOT EXISTS (SELECT 1 FROM new_traffic_counter_samples) THEN
        RETURN NULL;
    END IF;

    SELECT count(DISTINCT client_id)::bigint
    INTO owner_client_count
    FROM new_traffic_counter_samples;

    -- Supported one-client writers and the setwise live-minute publisher both
    -- hold their exact stream owners. Classify each statement stream against its
    -- client-specific billing context: a single ordinary row may use the direct
    -- same-hour/rollover owner, while duplicate, first, sparse, or historical
    -- rows keep their exact changed-hour owner without pulling ready peers into
    -- that work. Multi-client import/history statements have no live marker and
    -- remain wholly exact; a lost fast fence is never reinterpreted as repair.
    IF owner_client_count = 1
       OR current_setting(
            'vpsman.traffic_live_minute_publication', true
          ) = 'on' THEN
        -- Suppress generic hourly side effects only while the direct fast rows
        -- publish them. The marker is cleared before any exact-owner call.
        PERFORM set_config(
            'vpsman.traffic_hourly_derivations_prepublished', 'on', TRUE
        );
        WITH owners AS MATERIALIZED (
            SELECT DISTINCT sample.client_id
            FROM new_traffic_counter_samples sample
        ), billing AS MATERIALIZED (
            SELECT
                owner.client_id,
                context.reset_day,
                context.reset_hour,
                context.cycle_start,
                context.completed_through,
                CASE
                    WHEN context.reset_day = -1 THEN NULL
                    ELSE traffic_counter_cycle_start_utc(
                        context.reset_day,
                        context.reset_hour,
                        context.completed_through - interval '1 second'
                    )
                END AS previous_cycle_start
            FROM owners owner
            CROSS JOIN LATERAL traffic_counter_billing_context(
                owner.client_id, statement_timestamp()
            ) context
        ), transitions AS MATERIALIZED (
            SELECT
                sample.client_id,
                sample.source_kind,
                sample.interface,
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source,
                sample.inbound_promoted,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                sample.latest_observed_at AS sample_effective_observed_at,
                sample.updated_at AS sample_updated_at,
                sample.rx_usage_bytes,
                sample.tx_usage_bytes,
                sample.rx_reset_count,
                sample.tx_reset_count,
                sample.usage_authoritative,
                stream.client_id AS stream_client_id,
                stream.source_revision AS stream_source_revision,
                stream.materialized_revision AS stream_materialized_revision,
                stream.sample_edge_revision AS stream_sample_edge_revision,
                stream.latest_sample_observed_at AS previous_observed_at,
                stream.latest_sample_rx_bytes AS previous_rx_bytes,
                stream.latest_sample_tx_bytes AS previous_tx_bytes,
                stream.latest_sample_rx_counter_epoch
                    AS previous_rx_counter_epoch,
                stream.latest_sample_tx_counter_epoch
                    AS previous_tx_counter_epoch,
                stream.latest_sample_source AS previous_sample_source,
                stream.latest_sample_effective_observed_at
                    AS previous_effective_observed_at,
                stream.latest_sample_count AS previous_sample_count,
                stream.latest_sample_rx_bytes_avg AS previous_rx_bytes_avg,
                stream.latest_sample_tx_bytes_avg AS previous_tx_bytes_avg,
                stream.latest_sample_updated_at AS previous_sample_updated_at,
                stream.first_exact_observed_at,
                stream.last_exact_observed_at,
                stream.first_unpromoted_observed_at,
                stream.promoted_boundary_safe,
                previous_hour.rx_bytes AS previous_hour_rx_bytes,
                previous_hour.tx_bytes AS previous_hour_tx_bytes,
                previous_hour.rx_reset_count::bigint
                    AS previous_hour_rx_reset_count,
                previous_hour.tx_reset_count::bigint
                    AS previous_hour_tx_reset_count,
                previous_hour.latest_observed_at
                    AS previous_hour_latest_observed_at,
                target_hour.client_id AS target_hour_client_id,
                active.cycle_start AS active_cycle_start,
                active.completed_through AS active_completed_through,
                active.source_revision AS active_source_revision,
                active.materialized_revision AS active_materialized_revision,
                billing.reset_day,
                billing.reset_hour,
                billing.cycle_start,
                billing.completed_through,
                billing.previous_cycle_start,
                buckets.bucket_start,
                buckets.previous_bucket_start,
                buckets.bucket_start = buckets.previous_bucket_start
                    AS same_hour,
                buckets.bucket_start =
                    buckets.previous_bucket_start + interval '1 hour'
                    AS hour_rollover,
                CASE
                    WHEN sample.usage_authoritative
                    THEN sample.rx_usage_bytes
                    WHEN sample.rx_counter_epoch =
                            stream.latest_sample_rx_counter_epoch
                     AND sample.rx_bytes >= stream.latest_sample_rx_bytes
                    THEN sample.rx_bytes - stream.latest_sample_rx_bytes
                    ELSE 0
                END AS delta_rx_bytes,
                CASE
                    WHEN sample.usage_authoritative
                    THEN sample.tx_usage_bytes
                    WHEN sample.tx_counter_epoch =
                            stream.latest_sample_tx_counter_epoch
                     AND sample.tx_bytes >= stream.latest_sample_tx_bytes
                    THEN sample.tx_bytes - stream.latest_sample_tx_bytes
                    ELSE 0
                END AS delta_tx_bytes,
                CASE
                    WHEN sample.usage_authoritative
                    THEN sample.rx_reset_count::bigint
                    WHEN stream.latest_sample_rx_counter_epoch IS NOT NULL
                     AND sample.rx_counter_epoch <>
                            stream.latest_sample_rx_counter_epoch
                     AND NOT (
                        stream.latest_sample_source LIKE 'vnstat_import:%'
                        AND sample.sample_source NOT LIKE 'vnstat_import:%'
                     )
                    THEN 1::bigint ELSE 0::bigint
                END AS delta_rx_reset_count,
                CASE
                    WHEN sample.usage_authoritative
                    THEN sample.tx_reset_count::bigint
                    WHEN stream.latest_sample_tx_counter_epoch IS NOT NULL
                     AND sample.tx_counter_epoch <>
                            stream.latest_sample_tx_counter_epoch
                     AND NOT (
                        stream.latest_sample_source LIKE 'vnstat_import:%'
                        AND sample.sample_source NOT LIKE 'vnstat_import:%'
                     )
                    THEN 1::bigint ELSE 0::bigint
                END AS delta_tx_reset_count
            FROM new_traffic_counter_samples sample
            JOIN billing ON billing.client_id = sample.client_id
            LEFT JOIN traffic_counter_streams stream
              ON stream.client_id = sample.client_id
             AND stream.source_kind = sample.source_kind
             AND stream.interface = sample.interface
            CROSS JOIN LATERAL (
                SELECT
                    date_bin(
                        interval '1 hour', sample.observed_at,
                        TIMESTAMPTZ '1970-01-01 00:00:00+00'
                    ) AS bucket_start,
                    date_bin(
                        interval '1 hour', stream.latest_sample_observed_at,
                        TIMESTAMPTZ '1970-01-01 00:00:00+00'
                    ) AS previous_bucket_start
            ) buckets
            LEFT JOIN traffic_counter_hourly_usage previous_hour
              ON previous_hour.client_id = stream.client_id
             AND previous_hour.source_kind = stream.source_kind
             AND previous_hour.interface = stream.interface
             AND previous_hour.bucket_start = buckets.previous_bucket_start
            LEFT JOIN traffic_counter_hourly_usage target_hour
              ON target_hour.client_id = stream.client_id
             AND target_hour.source_kind = stream.source_kind
             AND target_hour.interface = stream.interface
             AND target_hour.bucket_start = buckets.bucket_start
             AND buckets.bucket_start <> buckets.previous_bucket_start
            LEFT JOIN traffic_counter_active_cycle_usage active
              ON active.client_id = sample.client_id
             AND active.source_kind = sample.source_kind
             AND active.interface = sample.interface
        ), shaped AS MATERIALIZED (
            SELECT
                transitions.*,
                count(*) OVER (
                    PARTITION BY client_id, source_kind, interface
                )::bigint AS statement_stream_rows,
                COALESCE(
                    reset_day <> -1
                    AND hour_rollover
                    AND active_source_revision = active_materialized_revision
                    AND active_cycle_start = previous_cycle_start
                    AND active_completed_through = previous_bucket_start,
                    FALSE
                ) AS rollover_active_behind,
                COALESCE(
                    reset_day <> -1
                    AND hour_rollover
                    AND active_source_revision = active_materialized_revision
                    AND active_cycle_start = cycle_start
                    AND active_completed_through = bucket_start,
                    FALSE
                ) AS rollover_active_current
            FROM transitions
        ), classified AS MATERIALIZED (
            SELECT
                shaped.*,
                COALESCE(
                    statement_stream_rows = 1
                    AND stream_client_id IS NOT NULL
                    AND stream_source_revision = stream_materialized_revision
                    AND stream_sample_edge_revision =
                        stream_materialized_revision
                    AND promoted_boundary_safe
                    AND observed_at >= completed_through
                    AND observed_at < completed_through + interval '1 hour'
                    AND NOT inbound_promoted
                    AND sample_source NOT LIKE 'vnstat_import:%'
                    AND first_exact_observed_at IS NOT NULL
                    AND first_unpromoted_observed_at IS NOT NULL
                    AND last_exact_observed_at = previous_observed_at
                    AND observed_at > previous_observed_at
                    AND previous_hour_latest_observed_at =
                        previous_observed_at
                    AND (same_hour OR hour_rollover)
                    AND (NOT hour_rollover OR target_hour_client_id IS NULL)
                    AND (
                        reset_day = -1
                        OR (
                            same_hour
                            AND active_source_revision =
                                active_materialized_revision
                            AND active_cycle_start = cycle_start
                            AND active_completed_through = bucket_start
                        )
                        OR rollover_active_behind
                        OR rollover_active_current
                    ),
                    FALSE
                ) AS fast_eligible
            FROM shaped
        ), gate AS MATERIALIZED (
            SELECT
                count(*)::bigint AS changed_count,
                count(DISTINCT (client_id, source_kind, interface))::bigint
                    AS changed_stream_count,
                count(*) FILTER (WHERE fast_eligible)::bigint AS fast_count,
                count(*) FILTER (WHERE NOT fast_eligible)::bigint
                    AS exact_count,
                array_agg(
                    client_id
                    ORDER BY client_id, source_kind, interface, observed_at
                ) FILTER (WHERE NOT fast_eligible) AS exact_client_ids,
                array_agg(
                    source_kind
                    ORDER BY client_id, source_kind, interface, observed_at
                ) FILTER (WHERE NOT fast_eligible) AS exact_source_kinds,
                array_agg(
                    interface
                    ORDER BY client_id, source_kind, interface, observed_at
                ) FILTER (WHERE NOT fast_eligible) AS exact_interfaces,
                array_agg(
                    observed_at
                    ORDER BY client_id, source_kind, interface, observed_at
                ) FILTER (WHERE NOT fast_eligible) AS exact_observed_values
            FROM classified
        ), fast AS MATERIALIZED (
            SELECT classified.*
            FROM classified
            WHERE fast_eligible
        ), updated_streams AS (
            UPDATE traffic_counter_streams stream
            SET
                source_revision = stream.source_revision + 1,
                materialized_revision = stream.materialized_revision + 1,
                sample_edge_revision = stream.sample_edge_revision + 1,
                latest_sample_observed_at = changed.observed_at,
                latest_sample_rx_bytes = changed.rx_bytes,
                latest_sample_tx_bytes = changed.tx_bytes,
                latest_sample_rx_counter_epoch = changed.rx_counter_epoch,
                latest_sample_tx_counter_epoch = changed.tx_counter_epoch,
                latest_sample_source = changed.sample_source,
                latest_sample_effective_observed_at =
                    changed.sample_effective_observed_at,
                latest_sample_count = changed.sample_count,
                latest_sample_rx_bytes_avg = round(
                    changed.rx_bytes_sum / changed.sample_count::numeric
                )::bigint,
                latest_sample_tx_bytes_avg = round(
                    changed.tx_bytes_sum / changed.sample_count::numeric
                )::bigint,
                latest_sample_updated_at = changed.sample_updated_at,
                previous_sample_effective_observed_at =
                    changed.previous_effective_observed_at,
                previous_sample_rx_bytes = changed.previous_rx_bytes,
                previous_sample_tx_bytes = changed.previous_tx_bytes,
                previous_sample_rx_counter_epoch =
                    changed.previous_rx_counter_epoch,
                previous_sample_tx_counter_epoch =
                    changed.previous_tx_counter_epoch,
                last_exact_observed_at = changed.observed_at,
                usage_rx_bytes = stream.usage_rx_bytes
                    + changed.delta_rx_bytes,
                usage_tx_bytes = stream.usage_tx_bytes
                    + changed.delta_tx_bytes,
                usage_rx_reset_count = stream.usage_rx_reset_count
                    + changed.delta_rx_reset_count,
                usage_tx_reset_count = stream.usage_tx_reset_count
                    + changed.delta_tx_reset_count,
                usage_row_count = stream.usage_row_count
                    + CASE WHEN changed.hour_rollover THEN 1 ELSE 0 END,
                updated_at = clock_timestamp()
            FROM fast changed
            WHERE stream.client_id = changed.client_id
              AND stream.source_kind = changed.source_kind
              AND stream.interface = changed.interface
              AND stream.source_revision =
                    changed.stream_source_revision
              AND stream.materialized_revision =
                    changed.stream_materialized_revision
              AND stream.sample_edge_revision =
                    changed.stream_sample_edge_revision
              AND stream.latest_sample_observed_at =
                    changed.previous_observed_at
              AND stream.latest_sample_rx_bytes = changed.previous_rx_bytes
              AND stream.latest_sample_tx_bytes = changed.previous_tx_bytes
              AND stream.latest_sample_rx_counter_epoch =
                    changed.previous_rx_counter_epoch
              AND stream.latest_sample_tx_counter_epoch =
                    changed.previous_tx_counter_epoch
              AND stream.latest_sample_source = changed.previous_sample_source
              AND stream.latest_sample_effective_observed_at =
                    changed.previous_effective_observed_at
              AND stream.latest_sample_count = changed.previous_sample_count
              AND stream.latest_sample_rx_bytes_avg =
                    changed.previous_rx_bytes_avg
              AND stream.latest_sample_tx_bytes_avg =
                    changed.previous_tx_bytes_avg
              AND stream.latest_sample_updated_at =
                    changed.previous_sample_updated_at
              AND stream.last_exact_observed_at =
                    changed.previous_observed_at
            RETURNING stream.client_id, stream.source_kind, stream.interface
        ), updated_same_hours AS (
            UPDATE traffic_counter_hourly_usage usage
            SET
                rx_bytes = usage.rx_bytes + changed.delta_rx_bytes,
                tx_bytes = usage.tx_bytes + changed.delta_tx_bytes,
                rx_reset_count = usage.rx_reset_count
                    + changed.delta_rx_reset_count::integer,
                tx_reset_count = usage.tx_reset_count
                    + changed.delta_tx_reset_count::integer,
                sample_count = usage.sample_count + 1,
                latest_observed_at = changed.observed_at,
                updated_at = clock_timestamp()
            FROM fast changed
            JOIN updated_streams updated
              ON updated.client_id = changed.client_id
             AND updated.source_kind = changed.source_kind
             AND updated.interface = changed.interface
            WHERE changed.same_hour
              AND usage.client_id = changed.client_id
              AND usage.source_kind = changed.source_kind
              AND usage.interface = changed.interface
              AND usage.bucket_start = changed.bucket_start
              AND usage.latest_observed_at = changed.previous_observed_at
            RETURNING 1
        ), inserted_rollover_hours AS (
            INSERT INTO traffic_counter_hourly_usage (
                client_id, source_kind, interface, bucket_start,
                rx_bytes, tx_bytes, rx_reset_count, tx_reset_count,
                sample_count, first_observed_at, latest_observed_at, updated_at
            )
            SELECT
                changed.client_id,
                changed.source_kind,
                changed.interface,
                changed.bucket_start,
                changed.delta_rx_bytes,
                changed.delta_tx_bytes,
                changed.delta_rx_reset_count::integer,
                changed.delta_tx_reset_count::integer,
                1,
                changed.observed_at,
                changed.observed_at,
                clock_timestamp()
            FROM fast changed
            JOIN updated_streams updated
              ON updated.client_id = changed.client_id
             AND updated.source_kind = changed.source_kind
             AND updated.interface = changed.interface
            WHERE changed.hour_rollover
            ORDER BY changed.client_id, changed.source_kind, changed.interface
            ON CONFLICT (client_id, source_kind, interface, bucket_start)
                DO NOTHING
            RETURNING client_id, source_kind, interface
        ), updated_active AS (
            UPDATE traffic_counter_active_cycle_usage active
            SET
                cycle_start = changed.cycle_start,
                completed_through = changed.bucket_start,
                rx_bytes = CASE
                    WHEN active.cycle_start <> changed.cycle_start
                    THEN CASE
                        WHEN changed.previous_bucket_start >= changed.cycle_start
                        THEN changed.previous_hour_rx_bytes ELSE 0
                    END
                    ELSE active.rx_bytes + changed.previous_hour_rx_bytes
                END,
                tx_bytes = CASE
                    WHEN active.cycle_start <> changed.cycle_start
                    THEN CASE
                        WHEN changed.previous_bucket_start >= changed.cycle_start
                        THEN changed.previous_hour_tx_bytes ELSE 0
                    END
                    ELSE active.tx_bytes + changed.previous_hour_tx_bytes
                END,
                rx_reset_count = CASE
                    WHEN active.cycle_start <> changed.cycle_start
                    THEN CASE
                        WHEN changed.previous_bucket_start >= changed.cycle_start
                        THEN changed.previous_hour_rx_reset_count ELSE 0
                    END
                    ELSE active.rx_reset_count
                        + changed.previous_hour_rx_reset_count
                END,
                tx_reset_count = CASE
                    WHEN active.cycle_start <> changed.cycle_start
                    THEN CASE
                        WHEN changed.previous_bucket_start >= changed.cycle_start
                        THEN changed.previous_hour_tx_reset_count ELSE 0
                    END
                    ELSE active.tx_reset_count
                        + changed.previous_hour_tx_reset_count
                END,
                source_revision = active.source_revision + 1,
                materialized_revision = active.source_revision + 1,
                updated_at = clock_timestamp()
            FROM fast changed
            JOIN updated_streams updated
              ON updated.client_id = changed.client_id
             AND updated.source_kind = changed.source_kind
             AND updated.interface = changed.interface
            JOIN inserted_rollover_hours inserted
              ON inserted.client_id = changed.client_id
             AND inserted.source_kind = changed.source_kind
             AND inserted.interface = changed.interface
            WHERE changed.reset_day <> -1
              AND changed.hour_rollover
              AND changed.rollover_active_behind
              AND active.client_id = changed.client_id
              AND active.source_kind = changed.source_kind
              AND active.interface = changed.interface
              AND active.source_revision = changed.active_source_revision
              AND active.materialized_revision =
                    changed.active_materialized_revision
            RETURNING 1
        )
        SELECT
            gate.changed_count,
            gate.changed_stream_count,
            gate.fast_count,
            gate.exact_count,
            gate.exact_client_ids,
            gate.exact_source_kinds,
            gate.exact_interfaces,
            gate.exact_observed_values,
            (SELECT count(*)::bigint FROM fast WHERE same_hour),
            (SELECT count(*)::bigint FROM fast WHERE hour_rollover),
            (SELECT count(*)::bigint FROM updated_streams),
            (SELECT count(*)::bigint FROM updated_same_hours),
            (SELECT count(*)::bigint FROM inserted_rollover_hours),
            (SELECT count(*)::bigint FROM fast
             WHERE rollover_active_behind),
            (SELECT count(*)::bigint FROM updated_active)
        INTO
            changed_count,
            changed_stream_count,
            fast_count,
            exact_count,
            client_ids,
            source_kinds,
            interfaces,
            observed_values,
            same_hour_count,
            rollover_count,
            updated_stream_count,
            updated_same_hour_count,
            inserted_rollover_count,
            expected_active_count,
            updated_active_count
        FROM gate;
        PERFORM set_config(
            'vpsman.traffic_hourly_derivations_prepublished', 'off', TRUE
        );

        fast_path := changed_count = fast_count + exact_count
            AND fast_count <= changed_stream_count
            AND COALESCE(cardinality(client_ids), 0) = exact_count
            AND COALESCE(cardinality(source_kinds), 0) = exact_count
            AND COALESCE(cardinality(interfaces), 0) = exact_count
            AND COALESCE(cardinality(observed_values), 0) = exact_count
            AND same_hour_count + rollover_count = fast_count
            AND updated_stream_count = fast_count
            AND updated_same_hour_count = same_hour_count
            AND inserted_rollover_count = rollover_count
            AND updated_active_count = expected_active_count;
        IF NOT fast_path THEN
            RAISE EXCEPTION 'traffic live insert lost an authority fence'
                USING ERRCODE = 'PZ028';
        END IF;

        -- The exact wrapper still owns every legitimate first/sparse/history
        -- stream and rejects damaged authorities. Fast peers are already fully
        -- published and are never included in its ordered arrays.
        IF exact_count > 0 THEN
            PERFORM refresh_traffic_counter_hourly_usage(
                client_ids, source_kinds, interfaces, observed_values
            );
        ELSE
            -- Only an all-fast statement can suppress the later sample-edge
            -- trigger. In a mixed statement that trigger recognizes these fast
            -- rows as already refreshed and publishes only the exact peers.
            PERFORM set_config(
                'vpsman.traffic_sample_edges_prepublished', 'on', TRUE
            );
        END IF;
        RETURN NULL;
    END IF;

    -- First samples, imports, historical rows, sparse gaps, and multi-row
    -- mutations use the exact changed-hour owner. This is semantic routing
    -- decided before live DML, never recovery from a failed live fence.
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(source_kind ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(interface ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(observed_at ORDER BY client_id, source_kind, interface, observed_at)
    INTO client_ids, source_kinds, interfaces, observed_values
    FROM new_traffic_counter_samples;
    PERFORM refresh_traffic_counter_hourly_usage(
        client_ids, source_kinds, interfaces, observed_values
    );
    RETURN NULL;
END;
$_$;



CREATE FUNCTION public.refresh_traffic_counter_hourly_usage_after_update() RETURNS trigger
    LANGUAGE plpgsql
    SET jit TO 'off'
    AS $_$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
    old_rx_values BIGINT[];
    old_tx_values BIGINT[];
    old_rx_counter_epoch_values BIGINT[];
    old_tx_counter_epoch_values BIGINT[];
    old_sample_source_values TEXT[];
    old_effective_observed_values TIMESTAMPTZ[];
    old_sample_count_values INTEGER[];
    old_rx_bytes_avg_values BIGINT[];
    old_tx_bytes_avg_values BIGINT[];
    old_sample_updated_values TIMESTAMPTZ[];
    new_rx_values BIGINT[];
    new_tx_values BIGINT[];
    new_rx_counter_epoch_values BIGINT[];
    new_tx_counter_epoch_values BIGINT[];
    new_sample_source_values TEXT[];
    new_effective_observed_values TIMESTAMPTZ[];
    new_sample_count_values INTEGER[];
    new_rx_bytes_avg_values BIGINT[];
    new_tx_bytes_avg_values BIGINT[];
    new_sample_updated_values TIMESTAMPTZ[];
    fast_bucket_starts TIMESTAMPTZ[];
    fast_rx_bytes BIGINT[];
    fast_tx_bytes BIGINT[];
    changed_count BIGINT;
    changed_stream_count BIGINT;
    updated_stream_count BIGINT;
    updated_hour_count BIGINT;
    owner_client_id TEXT;
    owner_client_count BIGINT;
    fast_path BOOLEAN := FALSE;
    lineage_only BOOLEAN := FALSE;
BEGIN
    -- INSERT .. ON CONFLICT runs the statement-level UPDATE trigger even when
    -- no row conflicted.  This return is material at telemetry frequency.
    IF NOT EXISTS (SELECT 1 FROM old_traffic_counter_samples)
       AND NOT EXISTS (SELECT 1 FROM new_traffic_counter_samples) THEN
        RETURN NULL;
    END IF;

    SELECT min(client_id), count(DISTINCT client_id)::bigint
    INTO owner_client_id, owner_client_count
    FROM (
        SELECT client_id FROM old_traffic_counter_samples
        UNION
        SELECT client_id FROM new_traffic_counter_samples
    ) owners;

    WITH paired AS MATERIALIZED (
        SELECT
            COALESCE(old_sample.client_id, new_sample.client_id) AS client_id,
            COALESCE(old_sample.source_kind, new_sample.source_kind)
                AS source_kind,
            COALESCE(old_sample.interface, new_sample.interface) AS interface,
            COALESCE(old_sample.observed_at, new_sample.observed_at)
                AS observed_at,
            old_sample.client_id AS old_client_id,
            old_sample.source_kind AS old_source_kind,
            old_sample.interface AS old_interface,
            old_sample.observed_at AS old_observed_at,
            old_sample.rx_bytes AS old_rx_bytes,
            old_sample.tx_bytes AS old_tx_bytes,
            old_sample.rx_counter_epoch AS old_rx_counter_epoch,
            old_sample.tx_counter_epoch AS old_tx_counter_epoch,
            old_sample.sample_source AS old_sample_source,
            old_sample.inbound_promoted AS old_inbound_promoted,
            old_sample.latest_observed_at AS old_effective_observed_at,
            old_sample.sample_count AS old_sample_count,
            round(
                old_sample.rx_bytes_sum / old_sample.sample_count::numeric
            )::bigint AS old_rx_bytes_avg,
            round(
                old_sample.tx_bytes_sum / old_sample.sample_count::numeric
            )::bigint AS old_tx_bytes_avg,
            old_sample.updated_at AS old_sample_updated_at,
            new_sample.client_id AS new_client_id,
            new_sample.source_kind AS new_source_kind,
            new_sample.interface AS new_interface,
            new_sample.observed_at AS new_observed_at,
            new_sample.rx_bytes AS new_rx_bytes,
            new_sample.tx_bytes AS new_tx_bytes,
            new_sample.rx_counter_epoch AS new_rx_counter_epoch,
            new_sample.tx_counter_epoch AS new_tx_counter_epoch,
            new_sample.sample_source AS new_sample_source,
            new_sample.inbound_promoted AS new_inbound_promoted,
            new_sample.latest_observed_at AS new_effective_observed_at,
            new_sample.sample_count AS new_sample_count,
            round(
                new_sample.rx_bytes_sum / new_sample.sample_count::numeric
            )::bigint AS new_rx_bytes_avg,
            round(
                new_sample.tx_bytes_sum / new_sample.sample_count::numeric
            )::bigint AS new_tx_bytes_avg,
            new_sample.updated_at AS new_sample_updated_at
        FROM old_traffic_counter_samples old_sample
        FULL OUTER JOIN new_traffic_counter_samples new_sample
          ON new_sample.client_id = old_sample.client_id
         AND new_sample.source_kind = old_sample.source_kind
         AND new_sample.interface = old_sample.interface
         AND new_sample.observed_at = old_sample.observed_at
        WHERE COALESCE(old_sample.client_id, new_sample.client_id) =
            owner_client_id
    ), prepared AS MATERIALIZED (
        SELECT
            paired.*,
            COALESCE(
                paired.old_client_id IS NOT NULL
                AND paired.new_client_id IS NOT NULL
                AND paired.old_client_id = paired.new_client_id
                AND paired.old_source_kind = paired.new_source_kind
                AND paired.old_interface = paired.new_interface
                AND paired.old_observed_at = paired.new_observed_at
                AND NOT paired.old_inbound_promoted
                AND NOT paired.new_inbound_promoted
                AND paired.old_sample_source NOT LIKE 'vnstat_import:%'
                AND paired.new_sample_source NOT LIKE 'vnstat_import:%'
                -- Only the monotonic, same-lineage replacement can be
                -- expressed as an exact new-minus-old delta without reading
                -- retained predecessors. Counter/source transitions route to
                -- the explicit reconstruction owner below.
                AND paired.new_sample_source = paired.old_sample_source
                AND paired.new_rx_counter_epoch =
                    paired.old_rx_counter_epoch
                AND paired.new_tx_counter_epoch =
                    paired.old_tx_counter_epoch
                AND paired.new_rx_bytes >= paired.old_rx_bytes
                AND paired.new_tx_bytes >= paired.old_tx_bytes
                AND paired.new_observed_at >= date_bin(
                    interval '1 hour', statement_timestamp(),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                )
                AND paired.new_observed_at < date_bin(
                    interval '1 hour', statement_timestamp(),
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) + interval '1 hour'
                AND stream.source_revision = stream.materialized_revision
                AND stream.sample_edge_revision = stream.materialized_revision
                AND stream.promoted_boundary_safe
                AND stream.first_exact_observed_at IS NOT NULL
                AND stream.first_unpromoted_observed_at IS NOT NULL
                AND stream.latest_sample_observed_at = paired.old_observed_at
                AND stream.latest_sample_rx_bytes = paired.old_rx_bytes
                AND stream.latest_sample_tx_bytes = paired.old_tx_bytes
                AND stream.latest_sample_rx_counter_epoch =
                    paired.old_rx_counter_epoch
                AND stream.latest_sample_tx_counter_epoch =
                    paired.old_tx_counter_epoch
                AND stream.latest_sample_source = paired.old_sample_source
                AND stream.latest_sample_effective_observed_at =
                    paired.old_effective_observed_at
                AND stream.latest_sample_count = paired.old_sample_count
                AND stream.latest_sample_rx_bytes_avg = paired.old_rx_bytes_avg
                AND stream.latest_sample_tx_bytes_avg = paired.old_tx_bytes_avg
                AND stream.latest_sample_updated_at =
                    paired.old_sample_updated_at
                AND stream.last_exact_observed_at = paired.old_observed_at
                AND usage.latest_observed_at = paired.old_observed_at,
                FALSE
            ) AS eligible
        FROM paired
        LEFT JOIN traffic_counter_streams stream
          ON stream.client_id = owner_client_id
         AND stream.client_id = paired.client_id
         AND stream.source_kind = paired.source_kind
         AND stream.interface = paired.interface
        LEFT JOIN traffic_counter_hourly_usage usage
          ON usage.client_id = owner_client_id
         AND usage.client_id = paired.new_client_id
         AND usage.source_kind = paired.new_source_kind
         AND usage.interface = paired.new_interface
         AND usage.bucket_start = date_bin(
                interval '1 hour', paired.new_observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
             )
    )
    SELECT
        count(*)::bigint,
        count(DISTINCT (client_id, source_kind, interface))::bigint,
        COALESCE(bool_and(eligible), FALSE),
        array_agg(client_id ORDER BY client_id, source_kind, interface),
        array_agg(source_kind ORDER BY client_id, source_kind, interface),
        array_agg(interface ORDER BY client_id, source_kind, interface),
        array_agg(new_observed_at ORDER BY client_id, source_kind, interface),
        array_agg(old_rx_bytes ORDER BY client_id, source_kind, interface),
        array_agg(old_tx_bytes ORDER BY client_id, source_kind, interface),
        array_agg(old_rx_counter_epoch
            ORDER BY client_id, source_kind, interface),
        array_agg(old_tx_counter_epoch
            ORDER BY client_id, source_kind, interface),
        array_agg(old_sample_source
            ORDER BY client_id, source_kind, interface),
        array_agg(old_effective_observed_at
            ORDER BY client_id, source_kind, interface),
        array_agg(old_sample_count
            ORDER BY client_id, source_kind, interface),
        array_agg(old_rx_bytes_avg
            ORDER BY client_id, source_kind, interface),
        array_agg(old_tx_bytes_avg
            ORDER BY client_id, source_kind, interface),
        array_agg(old_sample_updated_at
            ORDER BY client_id, source_kind, interface),
        array_agg(new_rx_bytes ORDER BY client_id, source_kind, interface),
        array_agg(new_tx_bytes ORDER BY client_id, source_kind, interface),
        array_agg(new_rx_counter_epoch
            ORDER BY client_id, source_kind, interface),
        array_agg(new_tx_counter_epoch
            ORDER BY client_id, source_kind, interface),
        array_agg(new_sample_source
            ORDER BY client_id, source_kind, interface),
        array_agg(new_effective_observed_at
            ORDER BY client_id, source_kind, interface),
        array_agg(new_sample_count
            ORDER BY client_id, source_kind, interface),
        array_agg(new_rx_bytes_avg
            ORDER BY client_id, source_kind, interface),
        array_agg(new_tx_bytes_avg
            ORDER BY client_id, source_kind, interface),
        array_agg(new_sample_updated_at
            ORDER BY client_id, source_kind, interface),
        array_agg(date_bin(
            interval '1 hour', new_observed_at,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) ORDER BY client_id, source_kind, interface),
        array_agg(
            new_rx_bytes - old_rx_bytes
            ORDER BY client_id, source_kind, interface
        ),
        array_agg(
            new_tx_bytes - old_tx_bytes
            ORDER BY client_id, source_kind, interface
        )
    INTO
        changed_count,
        changed_stream_count,
        fast_path,
        client_ids,
        source_kinds,
        interfaces,
        observed_values,
        old_rx_values,
        old_tx_values,
        old_rx_counter_epoch_values,
        old_tx_counter_epoch_values,
        old_sample_source_values,
        old_effective_observed_values,
        old_sample_count_values,
        old_rx_bytes_avg_values,
        old_tx_bytes_avg_values,
        old_sample_updated_values,
        new_rx_values,
        new_tx_values,
        new_rx_counter_epoch_values,
        new_tx_counter_epoch_values,
        new_sample_source_values,
        new_effective_observed_values,
        new_sample_count_values,
        new_rx_bytes_avg_values,
        new_tx_bytes_avg_values,
        new_sample_updated_values,
        fast_bucket_starts,
        fast_rx_bytes,
        fast_tx_bytes
    FROM prepared;

    fast_path := fast_path
        AND owner_client_count = 1
        AND changed_count = changed_stream_count;
    IF fast_path THEN
        -- The strict ordinary replacement has no reset/source transition, so
        -- its exact contribution change is simply new minus old. Publish the
        -- stream and open-hour owners from one materialized transition set;
        -- completed-cycle state is outside this same-coordinate scope.
        PERFORM set_config(
            'vpsman.traffic_hourly_derivations_prepublished', 'on', TRUE
        );
        WITH changed AS MATERIALIZED (
            SELECT *
            FROM unnest(
                client_ids, source_kinds, interfaces,
                fast_bucket_starts, observed_values,
                old_rx_values, old_tx_values,
                old_rx_counter_epoch_values,
                old_tx_counter_epoch_values,
                old_sample_source_values,
                old_effective_observed_values,
                old_sample_count_values,
                old_rx_bytes_avg_values,
                old_tx_bytes_avg_values,
                old_sample_updated_values,
                new_rx_values, new_tx_values,
                new_rx_counter_epoch_values,
                new_tx_counter_epoch_values,
                new_sample_source_values,
                new_effective_observed_values,
                new_sample_count_values,
                new_rx_bytes_avg_values,
                new_tx_bytes_avg_values,
                new_sample_updated_values,
                fast_rx_bytes, fast_tx_bytes
            ) AS row(
                client_id, source_kind, interface, bucket_start,
                observed_at, old_rx_bytes, old_tx_bytes,
                old_rx_counter_epoch, old_tx_counter_epoch,
                old_sample_source, old_effective_observed_at, old_sample_count,
                old_rx_bytes_avg, old_tx_bytes_avg, old_sample_updated_at,
                new_rx_bytes, new_tx_bytes,
                new_rx_counter_epoch, new_tx_counter_epoch,
                new_sample_source,
                new_effective_observed_at, new_sample_count,
                new_rx_bytes_avg, new_tx_bytes_avg, new_sample_updated_at,
                rx_bytes, tx_bytes
            )
        ), updated_streams AS (
            UPDATE traffic_counter_streams stream
            SET
                source_revision = stream.source_revision + 1,
                materialized_revision = stream.materialized_revision + 1,
                sample_edge_revision = stream.sample_edge_revision + 1,
                latest_sample_rx_bytes = changed.new_rx_bytes,
                latest_sample_tx_bytes = changed.new_tx_bytes,
                latest_sample_rx_counter_epoch =
                    changed.new_rx_counter_epoch,
                latest_sample_tx_counter_epoch =
                    changed.new_tx_counter_epoch,
                latest_sample_source = changed.new_sample_source,
                latest_sample_effective_observed_at =
                    changed.new_effective_observed_at,
                latest_sample_count = changed.new_sample_count,
                latest_sample_rx_bytes_avg = changed.new_rx_bytes_avg,
                latest_sample_tx_bytes_avg = changed.new_tx_bytes_avg,
                latest_sample_updated_at = changed.new_sample_updated_at,
                usage_rx_bytes = stream.usage_rx_bytes + changed.rx_bytes,
                usage_tx_bytes = stream.usage_tx_bytes + changed.tx_bytes,
                updated_at = clock_timestamp()
            FROM changed
            WHERE stream.client_id = changed.client_id
              AND stream.source_kind = changed.source_kind
              AND stream.interface = changed.interface
              AND stream.source_revision = stream.materialized_revision
              AND stream.sample_edge_revision = stream.materialized_revision
              AND stream.latest_sample_observed_at = changed.observed_at
              AND stream.latest_sample_rx_bytes = changed.old_rx_bytes
              AND stream.latest_sample_tx_bytes = changed.old_tx_bytes
              AND stream.latest_sample_rx_counter_epoch =
                    changed.old_rx_counter_epoch
              AND stream.latest_sample_tx_counter_epoch =
                    changed.old_tx_counter_epoch
              AND stream.latest_sample_source = changed.old_sample_source
              AND stream.latest_sample_effective_observed_at =
                    changed.old_effective_observed_at
              AND stream.latest_sample_count = changed.old_sample_count
              AND stream.latest_sample_rx_bytes_avg = changed.old_rx_bytes_avg
              AND stream.latest_sample_tx_bytes_avg = changed.old_tx_bytes_avg
              AND stream.latest_sample_updated_at = changed.old_sample_updated_at
              AND stream.last_exact_observed_at = changed.observed_at
            RETURNING stream.client_id, stream.source_kind, stream.interface
        ), updated_hours AS (
            UPDATE traffic_counter_hourly_usage usage
            SET
                rx_bytes = usage.rx_bytes + changed.rx_bytes,
                tx_bytes = usage.tx_bytes + changed.tx_bytes,
                updated_at = clock_timestamp()
            FROM changed
            JOIN updated_streams updated
              ON updated.client_id = changed.client_id
             AND updated.source_kind = changed.source_kind
             AND updated.interface = changed.interface
            WHERE usage.client_id = changed.client_id
              AND usage.source_kind = changed.source_kind
              AND usage.interface = changed.interface
              AND usage.bucket_start = changed.bucket_start
              AND usage.latest_observed_at = changed.observed_at
            RETURNING usage.client_id, usage.source_kind, usage.interface
        )
        SELECT
            (SELECT count(*)::bigint FROM updated_streams),
            (SELECT count(*)::bigint FROM updated_hours)
        INTO updated_stream_count, updated_hour_count;
        PERFORM set_config(
            'vpsman.traffic_hourly_derivations_prepublished', 'off', TRUE
        );
        IF updated_stream_count IS DISTINCT FROM changed_stream_count
           OR updated_hour_count IS DISTINCT FROM changed_stream_count THEN
            RAISE EXCEPTION 'traffic live update lost an authority fence'
                USING ERRCODE = 'PZ028';
        END IF;
        PERFORM set_config(
            'vpsman.traffic_sample_edges_prepublished', 'on', TRUE
        );
        RETURN NULL;
    END IF;

    IF current_setting('vpsman.traffic_import_same_shape_update', true) = 'on'
       AND EXISTS (SELECT 1 FROM new_traffic_counter_samples) THEN
        SELECT NOT EXISTS (
            SELECT 1
            FROM old_traffic_counter_samples old_sample
            FULL OUTER JOIN new_traffic_counter_samples new_sample
              ON new_sample.client_id = old_sample.client_id
             AND new_sample.source_kind = old_sample.source_kind
             AND new_sample.interface = old_sample.interface
             AND new_sample.observed_at = old_sample.observed_at
            WHERE old_sample.client_id IS NULL
               OR new_sample.client_id IS NULL
               OR NOT starts_with(old_sample.sample_source, 'vnstat_import:')
               OR NOT starts_with(new_sample.sample_source, 'vnstat_import:')
               OR old_sample.rx_bytes IS DISTINCT FROM new_sample.rx_bytes
               OR old_sample.tx_bytes IS DISTINCT FROM new_sample.tx_bytes
               OR old_sample.rx_counter_epoch IS DISTINCT FROM
                    new_sample.rx_counter_epoch
               OR old_sample.tx_counter_epoch IS DISTINCT FROM
                    new_sample.tx_counter_epoch
               OR old_sample.inbound_promoted IS DISTINCT FROM
                    new_sample.inbound_promoted
               OR starts_with(old_sample.sample_source, 'vnstat_import:')
                    IS DISTINCT FROM
                    starts_with(new_sample.sample_source, 'vnstat_import:')
        ) INTO lineage_only;

        IF lineage_only THEN
            BEGIN
                WITH changed_streams AS MATERIALIZED (
                    SELECT DISTINCT client_id, source_kind, interface
                    FROM new_traffic_counter_samples
                )
                UPDATE traffic_counter_streams streams
                SET
                    source_revision = streams.source_revision + 1,
                    materialized_revision = streams.source_revision + 1,
                    -- The proven same-shape rewrite changes only the vnStat
                    -- lineage token. Counter edges and the import/live class
                    -- are unchanged, so the edge owner publishes the same
                    -- semantic head at the new stream revision.
                    sample_edge_revision = streams.source_revision + 1,
                    updated_at = now()
                FROM changed_streams changed
                WHERE streams.client_id = changed.client_id
                  AND streams.source_kind = changed.source_kind
                  AND streams.interface = changed.interface
                  AND streams.source_revision = streams.materialized_revision;
                GET DIAGNOSTICS updated_stream_count = ROW_COUNT;

                SELECT count(*)::bigint
                INTO changed_stream_count
                FROM (
                    SELECT DISTINCT client_id, source_kind, interface
                    FROM new_traffic_counter_samples
                ) changed;
                IF updated_stream_count IS DISTINCT FROM changed_stream_count THEN
                    RAISE EXCEPTION
                        'traffic import same-shape update encountered a missing or dirty hourly marker'
                        USING ERRCODE = 'PZ001';
                END IF;
            EXCEPTION
                WHEN SQLSTATE 'PZ001' THEN
                    lineage_only := FALSE;
            END;
            IF lineage_only THEN
                RETURN NULL;
            END IF;
        END IF;
    END IF;

    WITH changed AS (
        SELECT client_id, source_kind, interface, observed_at
        FROM old_traffic_counter_samples
        UNION
        SELECT client_id, source_kind, interface, observed_at
        FROM new_traffic_counter_samples
    )
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface,
                  observed_at),
        array_agg(source_kind ORDER BY client_id, source_kind, interface,
                  observed_at),
        array_agg(interface ORDER BY client_id, source_kind, interface,
                  observed_at),
        array_agg(observed_at ORDER BY client_id, source_kind, interface,
                  observed_at)
    INTO client_ids, source_kinds, interfaces, observed_values
    FROM changed;
    PERFORM refresh_traffic_counter_hourly_usage(
        client_ids, source_kinds, interfaces, observed_values
    );
    RETURN NULL;
END;
$_$;



CREATE FUNCTION public.refresh_traffic_counter_hourly_usage_exact_core(changed_client_ids text[], changed_source_kinds text[], changed_interfaces text[], changed_observed_at timestamp with time zone[], rebuild_entire_streams boolean DEFAULT false) RETURNS void
    LANGUAGE plpgsql
    AS $$
DECLARE
    coverage_requires_rebuild BOOLEAN;
BEGIN
    IF COALESCE(array_length(changed_client_ids, 1), 0) = 0 THEN
        RETURN;
    END IF;
    IF array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_source_kinds, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_interfaces, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_observed_at, 1) THEN
        RAISE EXCEPTION 'traffic hourly refresh arrays must have equal lengths';
    END IF;

    -- Client deletion cascades through raw samples and both derived tables.
    -- The raw AFTER DELETE trigger must not recreate a coverage row for an
    -- identity that is already absent in this transaction.
    IF NOT EXISTS (
        SELECT 1
        FROM UNNEST(changed_client_ids) AS changed(client_id)
        JOIN clients ON clients.id = changed.client_id
    ) THEN
        RETURN;
    END IF;

    -- The ordinary wrapper creates an empty fence only for a genuinely new
    -- stream. Existing streams must bring both the hourly and sample-edge
    -- owners ready; any other mismatch is damaged authority and must fail
    -- instead of silently turning an arrival into a history scan.
    WITH changed_streams AS (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    )
    SELECT COALESCE(bool_or(
        streams.client_id IS NULL
        OR streams.source_revision <> streams.materialized_revision
        OR (
            NOT (
                streams.source_revision = 0
                AND streams.materialized_revision = 0
                AND streams.sample_edge_revision = 0
                AND streams.usage_row_count = 0
                AND streams.latest_sample_observed_at IS NULL
            )
            AND (
                streams.sample_edge_revision <>
                    streams.materialized_revision
                OR NOT streams.promoted_boundary_safe
            )
        )
    ), FALSE)
    INTO coverage_requires_rebuild
    FROM changed_streams changed
    JOIN clients ON clients.id = changed.client_id
    LEFT JOIN traffic_counter_streams streams
      ON streams.client_id = changed.client_id
     AND streams.source_kind = changed.source_kind
     AND streams.interface = changed.interface;

    IF coverage_requires_rebuild AND NOT rebuild_entire_streams THEN
        RAISE EXCEPTION
            'traffic hourly core encountered an unready stream authority'
            USING ERRCODE = 'PZ029';
    END IF;

    INSERT INTO traffic_counter_streams (
        client_id,
        source_kind,
        interface,
        source_revision,
        materialized_revision,
        updated_at
    )
    SELECT DISTINCT
        changed.client_id,
        changed.source_kind,
        changed.interface,
        1,
        0,
        now()
    FROM UNNEST(
        changed_client_ids,
        changed_source_kinds,
        changed_interfaces,
        changed_observed_at
    ) AS changed(client_id, source_kind, interface, observed_at)
    JOIN clients ON clients.id = changed.client_id
    ON CONFLICT (client_id, source_kind, interface) DO UPDATE SET
        source_revision =
            traffic_counter_streams.source_revision + 1,
        updated_at = now();

    -- Large imports and whole-stream epoch rewrites use one exact-key ordered
    -- scan per changed stream. The LATERAL boundary keeps each window local to
    -- one primary-key range, while the narrow projection avoids retaining a
    -- second full-row copy of the stream.
    IF rebuild_entire_streams THEN
        PERFORM set_config(
            'vpsman.traffic_explicit_hourly_reconstruction', 'on', TRUE
        );
        WITH changed_streams AS (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_observed_at
            ) AS item(client_id, source_kind, interface, observed_at)
        )
        DELETE FROM traffic_counter_hourly_usage usage
        USING changed_streams changed
        WHERE usage.client_id = changed.client_id
          AND usage.source_kind = changed.source_kind
          AND usage.interface = changed.interface;

        WITH changed_streams AS MATERIALIZED (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_observed_at
            ) AS item(client_id, source_kind, interface, observed_at)
        )
        INSERT INTO traffic_counter_hourly_usage (
            client_id,
            source_kind,
            interface,
            bucket_start,
            rx_bytes,
            tx_bytes,
            rx_reset_count,
            tx_reset_count,
            sample_count,
            first_observed_at,
            latest_observed_at,
            updated_at
        )
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            hourly.bucket_start,
            hourly.rx_bytes,
            hourly.tx_bytes,
            hourly.rx_reset_count,
            hourly.tx_reset_count,
            hourly.sample_count,
            hourly.first_observed_at,
            hourly.latest_observed_at,
            now()
        FROM changed_streams changed
        CROSS JOIN LATERAL (
            WITH sequenced AS (
                SELECT
                    sample.observed_at,
                    sample.rx_bytes,
                    sample.tx_bytes,
                    sample.rx_counter_epoch,
                    sample.tx_counter_epoch,
                    sample.sample_source,
                    sample.rx_usage_bytes,
                    sample.tx_usage_bytes,
                    sample.rx_reset_count,
                    sample.tx_reset_count,
                    sample.usage_authoritative,
                    LAG(sample.rx_bytes) OVER ordered
                        AS previous_rx_bytes,
                    LAG(sample.tx_bytes) OVER ordered
                        AS previous_tx_bytes,
                    LAG(sample.rx_counter_epoch) OVER ordered
                        AS previous_rx_counter_epoch,
                    LAG(sample.tx_counter_epoch) OVER ordered
                        AS previous_tx_counter_epoch,
                    LAG(sample.sample_source) OVER ordered
                        AS previous_sample_source
                FROM traffic_counter_samples sample
                WHERE sample.client_id = changed.client_id
                  AND sample.source_kind = changed.source_kind
                  AND sample.interface = changed.interface
                  AND sample.observed_at >= '-infinity'::timestamptz
                  AND sample.observed_at <= 'infinity'::timestamptz
                WINDOW ordered AS (ORDER BY sample.observed_at)
            )
            SELECT
                date_bin(
                    interval '1 hour',
                    observed_at,
                    TIMESTAMPTZ '1970-01-01 00:00:00+00'
                ) AS bucket_start,
                COALESCE(SUM(CASE
                    WHEN usage_authoritative THEN rx_usage_bytes
                    WHEN rx_counter_epoch = previous_rx_counter_epoch
                     AND rx_bytes >= previous_rx_bytes
                    THEN rx_bytes - previous_rx_bytes ELSE 0 END
                ), 0)::bigint AS rx_bytes,
                COALESCE(SUM(CASE
                    WHEN usage_authoritative THEN tx_usage_bytes
                    WHEN tx_counter_epoch = previous_tx_counter_epoch
                     AND tx_bytes >= previous_tx_bytes
                    THEN tx_bytes - previous_tx_bytes ELSE 0 END
                ), 0)::bigint AS tx_bytes,
                COALESCE(SUM(CASE
                    WHEN usage_authoritative THEN rx_reset_count
                    WHEN previous_rx_counter_epoch IS NOT NULL
                     AND rx_counter_epoch <> previous_rx_counter_epoch
                     AND NOT (
                         previous_sample_source LIKE 'vnstat_import:%'
                         AND sample_source NOT LIKE 'vnstat_import:%'
                     ) THEN 1 ELSE 0 END
                ), 0)::integer AS rx_reset_count,
                COALESCE(SUM(CASE
                    WHEN usage_authoritative THEN tx_reset_count
                    WHEN previous_tx_counter_epoch IS NOT NULL
                     AND tx_counter_epoch <> previous_tx_counter_epoch
                     AND NOT (
                         previous_sample_source LIKE 'vnstat_import:%'
                         AND sample_source NOT LIKE 'vnstat_import:%'
                     ) THEN 1 ELSE 0 END
                ), 0)::integer AS tx_reset_count,
                COUNT(*)::integer AS sample_count,
                MIN(observed_at) AS first_observed_at,
                MAX(observed_at) AS latest_observed_at
            FROM sequenced
            GROUP BY date_bin(
                interval '1 hour',
                observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            )
        ) hourly;

        UPDATE traffic_counter_streams streams
        SET
            materialized_revision = streams.source_revision,
            updated_at = now()
        FROM (
            SELECT DISTINCT client_id, source_kind, interface
            FROM UNNEST(
                changed_client_ids,
                changed_source_kinds,
                changed_interfaces,
                changed_observed_at
            ) AS item(client_id, source_kind, interface, observed_at)
        ) changed
        WHERE streams.client_id = changed.client_id
          AND streams.source_kind = changed.source_kind
          AND streams.interface = changed.interface;
        PERFORM set_config(
            'vpsman.traffic_explicit_hourly_reconstruction', 'off', TRUE
        );
        -- Publish the changed streams' exact sample edges before rebuilding
        -- the active prefix. This leaves one explicit repair owner and makes
        -- the active reconstruction observe the same bounded stream head that
        -- every reader will validate after commit.
        PERFORM refresh_traffic_counter_sample_edges(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        );
        PERFORM refresh_traffic_counter_active_cycle_usage(
            ARRAY(
                SELECT DISTINCT client_id
                FROM UNNEST(changed_client_ids) AS changed(client_id)
                ORDER BY client_id
            )
        );
        RETURN;
    END IF;

    -- Updating a sample changes the transition attributed to that sample and
    -- to its immediate successor. Rebuild the hours containing both. For a
    -- multi-row import/update, DISTINCT collapses repeated work per hour.
    WITH changed AS MATERIALIZED (
        SELECT DISTINCT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    ), changed_with_next AS MATERIALIZED (
        SELECT
            changed.*,
            LEAD(observed_at) OVER (
                PARTITION BY client_id, source_kind, interface
                ORDER BY observed_at
            ) AS next_changed_at
        FROM changed
    ), affected AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                changed.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        UNION
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                successor.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        JOIN LATERAL (
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
              AND sample.observed_at > changed.observed_at
            ORDER BY sample.observed_at ASC
            LIMIT 1
        ) successor ON TRUE
        -- Samples are minute-aligned. Consecutive changed minutes are each
        -- other's successor and their hours are already present above, so a
        -- large telemetry import needs only one boundary lookup per gap.
        WHERE changed.next_changed_at IS NULL
           OR changed.next_changed_at > changed.observed_at + interval '1 minute'
    )
    DELETE FROM traffic_counter_hourly_usage usage
    USING affected
    WHERE usage.client_id = affected.client_id
      AND usage.source_kind = affected.source_kind
      AND usage.interface = affected.interface
      AND usage.bucket_start = affected.bucket_start;

    WITH changed AS MATERIALIZED (
        SELECT DISTINCT *
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    ), changed_with_next AS MATERIALIZED (
        SELECT
            changed.*,
            LEAD(observed_at) OVER (
                PARTITION BY client_id, source_kind, interface
                ORDER BY observed_at
            ) AS next_changed_at
        FROM changed
    ), affected AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                changed.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        UNION
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            date_bin(
                interval '1 hour',
                successor.observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ) AS bucket_start
        FROM changed_with_next changed
        JOIN LATERAL (
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
              AND sample.observed_at > changed.observed_at
            ORDER BY sample.observed_at ASC
            LIMIT 1
        ) successor ON TRUE
        WHERE changed.next_changed_at IS NULL
           OR changed.next_changed_at > changed.observed_at + interval '1 minute'
    ), selected AS MATERIALIZED (
        SELECT
            affected.client_id,
            affected.source_kind,
            affected.interface,
            affected.bucket_start,
            sample.observed_at,
            sample.rx_bytes,
            sample.tx_bytes,
            sample.rx_counter_epoch,
            sample.tx_counter_epoch,
            sample.sample_source,
            sample.rx_usage_bytes,
            sample.tx_usage_bytes,
            sample.rx_reset_count,
            sample.tx_reset_count,
            sample.usage_authoritative
        FROM affected
        JOIN LATERAL (
            (
                SELECT
                    sample.observed_at,
                    sample.rx_bytes,
                    sample.tx_bytes,
                    sample.rx_counter_epoch,
                    sample.tx_counter_epoch,
                    sample.sample_source,
                    sample.rx_usage_bytes,
                    sample.tx_usage_bytes,
                    sample.rx_reset_count,
                    sample.tx_reset_count,
                    sample.usage_authoritative
                FROM traffic_counter_samples sample
                WHERE sample.client_id = affected.client_id
                  AND sample.source_kind = affected.source_kind
                  AND sample.interface = affected.interface
                  AND sample.observed_at < affected.bucket_start
                ORDER BY sample.observed_at DESC
                LIMIT 1
            )
            UNION ALL
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source,
                sample.rx_usage_bytes,
                sample.tx_usage_bytes,
                sample.rx_reset_count,
                sample.tx_reset_count,
                sample.usage_authoritative
            FROM traffic_counter_samples sample
            WHERE sample.client_id = affected.client_id
              AND sample.source_kind = affected.source_kind
              AND sample.interface = affected.interface
              AND sample.observed_at >= affected.bucket_start
              AND sample.observed_at < affected.bucket_start + interval '1 hour'
        ) sample ON TRUE
    ), sequenced AS (
        SELECT
            selected.*,
            LAG(rx_bytes) OVER stream AS previous_rx_bytes,
            LAG(tx_bytes) OVER stream AS previous_tx_bytes,
            LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
            LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
            LAG(sample_source) OVER stream AS previous_sample_source
        FROM selected
        WINDOW stream AS (
            PARTITION BY client_id, source_kind, interface, bucket_start
            ORDER BY observed_at
        )
    )
    INSERT INTO traffic_counter_hourly_usage (
        client_id,
        source_kind,
        interface,
        bucket_start,
        rx_bytes,
        tx_bytes,
        rx_reset_count,
        tx_reset_count,
        sample_count,
        first_observed_at,
        latest_observed_at,
        updated_at
    )
    SELECT
        client_id,
        source_kind,
        interface,
        bucket_start,
        COALESCE(SUM(
            CASE
                WHEN usage_authoritative THEN rx_usage_bytes
                WHEN rx_counter_epoch = previous_rx_counter_epoch
                 AND rx_bytes >= previous_rx_bytes
                THEN rx_bytes - previous_rx_bytes
                ELSE 0
            END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::bigint,
        COALESCE(SUM(
            CASE
                WHEN usage_authoritative THEN tx_usage_bytes
                WHEN tx_counter_epoch = previous_tx_counter_epoch
                 AND tx_bytes >= previous_tx_bytes
                THEN tx_bytes - previous_tx_bytes
                ELSE 0
            END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::bigint,
        COALESCE(SUM(CASE
            WHEN usage_authoritative THEN rx_reset_count
            WHEN previous_rx_counter_epoch IS NOT NULL
             AND rx_counter_epoch <> previous_rx_counter_epoch
             AND NOT (
                 previous_sample_source LIKE 'vnstat_import:%'
                 AND sample_source NOT LIKE 'vnstat_import:%'
             ) THEN 1 ELSE 0 END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::integer,
        COALESCE(SUM(CASE
            WHEN usage_authoritative THEN tx_reset_count
            WHEN previous_tx_counter_epoch IS NOT NULL
             AND tx_counter_epoch <> previous_tx_counter_epoch
             AND NOT (
                 previous_sample_source LIKE 'vnstat_import:%'
                 AND sample_source NOT LIKE 'vnstat_import:%'
             ) THEN 1 ELSE 0 END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::integer,
        COUNT(*) FILTER (WHERE observed_at >= bucket_start)::integer,
        MIN(observed_at) FILTER (WHERE observed_at >= bucket_start),
        MAX(observed_at) FILTER (WHERE observed_at >= bucket_start),
        now()
    FROM sequenced
    GROUP BY client_id, source_kind, interface, bucket_start
    HAVING COUNT(*) FILTER (WHERE observed_at >= bucket_start) > 0;

    UPDATE traffic_counter_streams streams
    SET
        materialized_revision = streams.source_revision,
        updated_at = now()
    FROM (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces,
            changed_observed_at
        ) AS item(client_id, source_kind, interface, observed_at)
    ) changed
    WHERE streams.client_id = changed.client_id
      AND streams.source_kind = changed.source_kind
      AND streams.interface = changed.interface;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_rollup_summaries_after_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    origin_kinds TEXT[];
    bucket_sizes INTEGER[];
    rx_byte_deltas BIGINT[];
    tx_byte_deltas BIGINT[];
    rx_reset_deltas BIGINT[];
    tx_reset_deltas BIGINT[];
    row_count_deltas BIGINT[];
BEGIN
    PERFORM apply_traffic_counter_active_cycle_usage_deltas(
        array_agg(row.client_id ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.source_kind ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.interface ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.bucket_start ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.rx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.tx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.rx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(-row.tx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start)
    )
    FROM (
        SELECT
            client_id, source_kind, interface, bucket_start,
            sum(rx_bytes)::bigint AS rx_bytes,
            sum(tx_bytes)::bigint AS tx_bytes,
            sum(rx_reset_count)::bigint AS rx_reset_count,
            sum(tx_reset_count)::bigint AS tx_reset_count
        FROM old_traffic_counter_rollups
        WHERE bucket_secs = 3600
        GROUP BY client_id, source_kind, interface, bucket_start
    ) row;

    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(source_kind ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(interface ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(origin_kind ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(bucket_secs ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(rx_bytes ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(tx_bytes ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(rx_reset_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(tx_reset_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(row_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs)
    INTO client_ids, source_kinds, interfaces, origin_kinds, bucket_sizes,
         rx_byte_deltas, tx_byte_deltas, rx_reset_deltas, tx_reset_deltas,
         row_count_deltas
    FROM (
        SELECT
            client_id, source_kind, interface, origin_kind, bucket_secs,
            -sum(rx_bytes)::BIGINT AS rx_bytes,
            -sum(tx_bytes)::BIGINT AS tx_bytes,
            -sum(rx_reset_count)::BIGINT AS rx_reset_count,
            -sum(tx_reset_count)::BIGINT AS tx_reset_count,
            -count(*)::BIGINT AS row_count
        FROM old_traffic_counter_rollups
        GROUP BY client_id, source_kind, interface, origin_kind, bucket_secs
    ) changed;
    PERFORM apply_traffic_counter_rollup_summary_deltas(
        client_ids, source_kinds, interfaces, origin_kinds, bucket_sizes,
        rx_byte_deltas, tx_byte_deltas, rx_reset_deltas, tx_reset_deltas,
        row_count_deltas
    );
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_rollup_summaries_after_insert() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    origin_kinds TEXT[];
    bucket_sizes INTEGER[];
    rx_byte_deltas BIGINT[];
    tx_byte_deltas BIGINT[];
    rx_reset_deltas BIGINT[];
    tx_reset_deltas BIGINT[];
    row_count_deltas BIGINT[];
BEGIN
    PERFORM apply_traffic_counter_active_cycle_usage_deltas(
        array_agg(row.client_id ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.source_kind ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.interface ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.bucket_start ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.rx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.tx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.rx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.tx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start)
    )
    FROM (
        SELECT
            client_id, source_kind, interface, bucket_start,
            sum(rx_bytes)::bigint AS rx_bytes,
            sum(tx_bytes)::bigint AS tx_bytes,
            sum(rx_reset_count)::bigint AS rx_reset_count,
            sum(tx_reset_count)::bigint AS tx_reset_count
        FROM new_traffic_counter_rollups
        WHERE bucket_secs = 3600
        GROUP BY client_id, source_kind, interface, bucket_start
    ) row;

    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(source_kind ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(interface ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(origin_kind ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(bucket_secs ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(rx_bytes ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(tx_bytes ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(rx_reset_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(tx_reset_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(row_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs)
    INTO client_ids, source_kinds, interfaces, origin_kinds, bucket_sizes,
         rx_byte_deltas, tx_byte_deltas, rx_reset_deltas, tx_reset_deltas,
         row_count_deltas
    FROM (
        SELECT
            client_id, source_kind, interface, origin_kind, bucket_secs,
            sum(rx_bytes)::BIGINT AS rx_bytes,
            sum(tx_bytes)::BIGINT AS tx_bytes,
            sum(rx_reset_count)::BIGINT AS rx_reset_count,
            sum(tx_reset_count)::BIGINT AS tx_reset_count,
            count(*)::BIGINT AS row_count
        FROM new_traffic_counter_rollups
        GROUP BY client_id, source_kind, interface, origin_kind, bucket_secs
    ) changed;
    PERFORM apply_traffic_counter_rollup_summary_deltas(
        client_ids, source_kinds, interfaces, origin_kinds, bucket_sizes,
        rx_byte_deltas, tx_byte_deltas, rx_reset_deltas, tx_reset_deltas,
        row_count_deltas
    );
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_rollup_summaries_after_update() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    origin_kinds TEXT[];
    bucket_sizes INTEGER[];
    rx_byte_deltas BIGINT[];
    tx_byte_deltas BIGINT[];
    rx_reset_deltas BIGINT[];
    tx_reset_deltas BIGINT[];
    row_count_deltas BIGINT[];
BEGIN
    PERFORM apply_traffic_counter_active_cycle_usage_deltas(
        array_agg(row.client_id ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.source_kind ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.interface ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.bucket_start ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.rx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.tx_bytes ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.rx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start),
        array_agg(row.tx_reset_count ORDER BY row.client_id, row.source_kind,
                  row.interface, row.bucket_start)
    )
    FROM (
        SELECT
            client_id, source_kind, interface, bucket_start,
            sum(rx_bytes)::bigint AS rx_bytes,
            sum(tx_bytes)::bigint AS tx_bytes,
            sum(rx_reset_count)::bigint AS rx_reset_count,
            sum(tx_reset_count)::bigint AS tx_reset_count
        FROM (
            SELECT
                client_id, source_kind, interface, bucket_start,
                rx_bytes::numeric AS rx_bytes,
                tx_bytes::numeric AS tx_bytes,
                rx_reset_count::numeric AS rx_reset_count,
                tx_reset_count::numeric AS tx_reset_count
            FROM new_traffic_counter_rollups
            WHERE bucket_secs = 3600
            UNION ALL
            SELECT
                client_id, source_kind, interface, bucket_start,
                -rx_bytes::numeric,
                -tx_bytes::numeric,
                -rx_reset_count::numeric,
                -tx_reset_count::numeric
            FROM old_traffic_counter_rollups
            WHERE bucket_secs = 3600
        ) delta
        GROUP BY client_id, source_kind, interface, bucket_start
        HAVING sum(rx_bytes) <> 0
            OR sum(tx_bytes) <> 0
            OR sum(rx_reset_count) <> 0
            OR sum(tx_reset_count) <> 0
    ) row;

    WITH deltas AS (
        SELECT
            client_id, source_kind, interface, origin_kind, bucket_secs,
            rx_bytes::NUMERIC AS rx_bytes,
            tx_bytes::NUMERIC AS tx_bytes,
            rx_reset_count::NUMERIC AS rx_reset_count,
            tx_reset_count::NUMERIC AS tx_reset_count,
            1::BIGINT AS row_count
        FROM new_traffic_counter_rollups
        UNION ALL
        SELECT
            client_id, source_kind, interface, origin_kind, bucket_secs,
            -rx_bytes::NUMERIC, -tx_bytes::NUMERIC,
            -rx_reset_count::NUMERIC, -tx_reset_count::NUMERIC,
            -1::BIGINT
        FROM old_traffic_counter_rollups
    ), changed AS (
        SELECT
            client_id, source_kind, interface, origin_kind, bucket_secs,
            sum(rx_bytes)::BIGINT AS rx_bytes,
            sum(tx_bytes)::BIGINT AS tx_bytes,
            sum(rx_reset_count)::BIGINT AS rx_reset_count,
            sum(tx_reset_count)::BIGINT AS tx_reset_count,
            sum(row_count)::BIGINT AS row_count
        FROM deltas
        GROUP BY client_id, source_kind, interface, origin_kind, bucket_secs
    )
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(source_kind ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(interface ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(origin_kind ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(bucket_secs ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(rx_bytes ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(tx_bytes ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(rx_reset_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(tx_reset_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs),
        array_agg(row_count ORDER BY client_id, source_kind, interface,
                  origin_kind, bucket_secs)
    INTO client_ids, source_kinds, interfaces, origin_kinds, bucket_sizes,
         rx_byte_deltas, tx_byte_deltas, rx_reset_deltas, tx_reset_deltas,
         row_count_deltas
    FROM changed;
    PERFORM apply_traffic_counter_rollup_summary_deltas(
        client_ids, source_kinds, interfaces, origin_kinds, bucket_sizes,
        rx_byte_deltas, tx_byte_deltas, rx_reset_deltas, tx_reset_deltas,
        row_count_deltas
    );
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_sample_edges(changed_client_ids text[], changed_source_kinds text[], changed_interfaces text[]) RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF COALESCE(array_length(changed_client_ids, 1), 0) = 0 THEN
        RETURN;
    END IF;
    IF array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_source_kinds, 1)
       OR array_length(changed_client_ids, 1)
            IS DISTINCT FROM array_length(changed_interfaces, 1) THEN
        RAISE EXCEPTION 'traffic sample edge arrays must have equal lengths';
    END IF;

    WITH changed AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS item(client_id, source_kind, interface)
    )
    INSERT INTO traffic_counter_promoted_boundaries (
        client_id, source_kind, interface, observed_at
    )
    SELECT
        changed.client_id,
        changed.source_kind,
        changed.interface,
        sample.observed_at
    FROM changed
    JOIN LATERAL (
        SELECT raw.observed_at, raw.inbound_promoted
        FROM traffic_counter_samples raw
        WHERE raw.client_id = changed.client_id
          AND raw.source_kind = changed.source_kind
          AND raw.interface = changed.interface
        ORDER BY raw.observed_at
        LIMIT 2
    ) sample ON sample.inbound_promoted
    ON CONFLICT DO NOTHING;

    WITH changed AS MATERIALIZED (
        SELECT DISTINCT client_id, source_kind, interface
        FROM UNNEST(
            changed_client_ids,
            changed_source_kinds,
            changed_interfaces
        ) AS item(client_id, source_kind, interface)
    ), edges AS MATERIALIZED (
        SELECT
            changed.client_id,
            changed.source_kind,
            changed.interface,
            latest.observed_at AS latest_sample_observed_at,
            latest.rx_bytes AS latest_sample_rx_bytes,
            latest.tx_bytes AS latest_sample_tx_bytes,
            latest.rx_counter_epoch AS latest_sample_rx_counter_epoch,
            latest.tx_counter_epoch AS latest_sample_tx_counter_epoch,
            latest.sample_source AS latest_sample_source,
            latest.latest_observed_at
                AS latest_sample_effective_observed_at,
            latest.sample_count AS latest_sample_count,
            round(latest.rx_bytes_sum / latest.sample_count::numeric)::bigint
                AS latest_sample_rx_bytes_avg,
            round(latest.tx_bytes_sum / latest.sample_count::numeric)::bigint
                AS latest_sample_tx_bytes_avg,
            latest.updated_at AS latest_sample_updated_at,
            previous.latest_observed_at
                AS previous_sample_effective_observed_at,
            previous.rx_bytes AS previous_sample_rx_bytes,
            previous.tx_bytes AS previous_sample_tx_bytes,
            previous.rx_counter_epoch
                AS previous_sample_rx_counter_epoch,
            previous.tx_counter_epoch
                AS previous_sample_tx_counter_epoch,
            first_unpromoted.observed_at AS first_unpromoted_observed_at,
            CASE
                WHEN promoted.promoted_count = 0
                 AND first_rows.raw_promoted_count = 0
                 AND NOT COALESCE(latest.inbound_promoted, FALSE)
                THEN first_rows.first_sample_at
                WHEN promoted.promoted_count = 1
                 AND first_rows.raw_promoted_count = 1
                 AND first_rows.first_inbound_promoted
                 AND promoted.first_promoted_at = first_rows.first_sample_at
                 AND first_rows.sample_count = 2
                 AND NOT COALESCE(latest.inbound_promoted, FALSE)
                THEN first_rows.second_sample_at
            END AS first_exact_observed_at,
            CASE
                WHEN (
                    (
                        promoted.promoted_count = 0
                        AND first_rows.raw_promoted_count = 0
                    )
                    OR (
                        promoted.promoted_count = 1
                        AND first_rows.raw_promoted_count = 1
                        AND first_rows.first_inbound_promoted
                        AND promoted.first_promoted_at =
                            first_rows.first_sample_at
                    )
                )
                 AND NOT COALESCE(latest.inbound_promoted, FALSE)
                THEN latest.observed_at
            END AS last_exact_observed_at,
            (
                promoted.promoted_count = 0
                AND first_rows.raw_promoted_count = 0
            )
                OR (
                    promoted.promoted_count = 1
                    AND first_rows.raw_promoted_count = 1
                    AND first_rows.first_inbound_promoted
                    AND promoted.first_promoted_at = first_rows.first_sample_at
                ) AS promoted_boundary_safe
        FROM changed
        LEFT JOIN LATERAL (
            -- This is the exact raw-retention frontier even when a preserved
            -- conflict precedes a later promoted boundary. `first_exact` is
            -- intentionally NULL for that unsafe presentation shape, so it
            -- cannot serve as the retention authority.
            SELECT sample.observed_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
              AND NOT sample.inbound_promoted
            ORDER BY sample.observed_at
            LIMIT 1
        ) first_unpromoted ON TRUE
        LEFT JOIN LATERAL (
            SELECT
                sample.observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch,
                sample.sample_source,
                sample.inbound_promoted,
                sample.sample_count,
                sample.rx_bytes_sum,
                sample.tx_bytes_sum,
                sample.latest_observed_at,
                sample.updated_at
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) latest ON TRUE
        LEFT JOIN LATERAL (
            SELECT
                sample.latest_observed_at,
                sample.rx_bytes,
                sample.tx_bytes,
                sample.rx_counter_epoch,
                sample.tx_counter_epoch
            FROM traffic_counter_samples sample
            WHERE sample.client_id = changed.client_id
              AND sample.source_kind = changed.source_kind
              AND sample.interface = changed.interface
              AND sample.observed_at < latest.observed_at
            ORDER BY sample.observed_at DESC
            LIMIT 1
        ) previous ON TRUE
        LEFT JOIN LATERAL (
            SELECT
                count(*)::integer AS sample_count,
                min(first_two.observed_at) AS first_sample_at,
                max(first_two.observed_at) AS second_sample_at,
                count(*) FILTER (
                    WHERE first_two.inbound_promoted
                )::integer AS raw_promoted_count,
                COALESCE((array_agg(
                    first_two.inbound_promoted
                    ORDER BY first_two.observed_at
                ))[1], FALSE) AS first_inbound_promoted
            FROM (
                SELECT sample.observed_at, sample.inbound_promoted
                FROM traffic_counter_samples sample
                WHERE sample.client_id = changed.client_id
                  AND sample.source_kind = changed.source_kind
                  AND sample.interface = changed.interface
                ORDER BY sample.observed_at
                LIMIT 2
            ) first_two
        ) first_rows ON TRUE
        LEFT JOIN LATERAL (
            SELECT
                count(*)::integer AS promoted_count,
                min(first_two_promoted.observed_at) AS first_promoted_at
            FROM (
                SELECT promoted.observed_at
                FROM traffic_counter_promoted_boundaries promoted
                WHERE promoted.client_id = changed.client_id
                  AND promoted.source_kind = changed.source_kind
                  AND promoted.interface = changed.interface
                ORDER BY promoted.observed_at
                LIMIT 2
            ) first_two_promoted
        ) promoted ON TRUE
    )
    UPDATE traffic_counter_streams stream
    SET
        sample_edge_revision = stream.source_revision,
        latest_sample_observed_at = edges.latest_sample_observed_at,
        latest_sample_rx_bytes = edges.latest_sample_rx_bytes,
        latest_sample_tx_bytes = edges.latest_sample_tx_bytes,
        latest_sample_rx_counter_epoch =
            edges.latest_sample_rx_counter_epoch,
        latest_sample_tx_counter_epoch =
            edges.latest_sample_tx_counter_epoch,
        latest_sample_source = edges.latest_sample_source,
        latest_sample_effective_observed_at =
            edges.latest_sample_effective_observed_at,
        latest_sample_count = edges.latest_sample_count,
        latest_sample_rx_bytes_avg = edges.latest_sample_rx_bytes_avg,
        latest_sample_tx_bytes_avg = edges.latest_sample_tx_bytes_avg,
        latest_sample_updated_at = edges.latest_sample_updated_at,
        previous_sample_effective_observed_at =
            edges.previous_sample_effective_observed_at,
        previous_sample_rx_bytes = edges.previous_sample_rx_bytes,
        previous_sample_tx_bytes = edges.previous_sample_tx_bytes,
        previous_sample_rx_counter_epoch =
            edges.previous_sample_rx_counter_epoch,
        previous_sample_tx_counter_epoch =
            edges.previous_sample_tx_counter_epoch,
        first_exact_observed_at = edges.first_exact_observed_at,
        last_exact_observed_at = edges.last_exact_observed_at,
        first_unpromoted_observed_at = edges.first_unpromoted_observed_at,
        promoted_boundary_safe = edges.promoted_boundary_safe,
        updated_at = now()
    FROM edges
    WHERE stream.client_id = edges.client_id
      AND stream.source_kind = edges.source_kind
      AND stream.interface = edges.interface;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_sample_edges_after_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
BEGIN
    DELETE FROM traffic_counter_promoted_boundaries boundary
    USING old_traffic_counter_sample_edges sample
    WHERE sample.inbound_promoted
      AND boundary.client_id = sample.client_id
      AND boundary.source_kind = sample.source_kind
      AND boundary.interface = sample.interface
      AND boundary.observed_at = sample.observed_at;

    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface),
        array_agg(source_kind ORDER BY client_id, source_kind, interface),
        array_agg(interface ORDER BY client_id, source_kind, interface)
    INTO client_ids, source_kinds, interfaces
    FROM (
        SELECT DISTINCT client_id, source_kind, interface
        FROM old_traffic_counter_sample_edges
    ) changed;
    PERFORM refresh_traffic_counter_sample_edges(
        client_ids, source_kinds, interfaces
    );
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_sample_edges_after_insert() RETURNS trigger
    LANGUAGE plpgsql
    SET jit TO 'off'
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
BEGIN
    IF NOT EXISTS (SELECT 1 FROM new_traffic_counter_sample_edges) THEN
        RETURN NULL;
    END IF;
    INSERT INTO traffic_counter_promoted_boundaries (
        client_id, source_kind, interface, observed_at
    )
    SELECT DISTINCT
        sample.client_id,
        sample.source_kind,
        sample.interface,
        sample.observed_at
    FROM new_traffic_counter_sample_edges sample
    WHERE sample.inbound_promoted
    ON CONFLICT DO NOTHING;

    IF current_setting(
        'vpsman.traffic_sample_edges_prepublished', true
    ) = 'on' THEN
        PERFORM set_config(
            'vpsman.traffic_sample_edges_prepublished', 'off', TRUE
        );
        RETURN NULL;
    END IF;

    WITH changed AS MATERIALIZED (
        SELECT
            sample.client_id,
            sample.source_kind,
            sample.interface,
            count(*)::bigint AS row_count,
            min(sample.observed_at) AS first_observed_at,
            max(sample.observed_at) AS latest_observed_at,
            (array_agg(sample.rx_bytes ORDER BY sample.observed_at DESC))[1]
                AS latest_rx_bytes,
            (array_agg(sample.tx_bytes ORDER BY sample.observed_at DESC))[1]
                AS latest_tx_bytes,
            (array_agg(
                sample.rx_counter_epoch ORDER BY sample.observed_at DESC
            ))[1] AS latest_rx_counter_epoch,
            (array_agg(
                sample.tx_counter_epoch ORDER BY sample.observed_at DESC
            ))[1] AS latest_tx_counter_epoch,
            (array_agg(
                sample.sample_source ORDER BY sample.observed_at DESC
            ))[1] AS latest_sample_source,
            (array_agg(
                sample.latest_observed_at ORDER BY sample.observed_at DESC
            ))[1] AS latest_effective_observed_at,
            (array_agg(
                sample.sample_count ORDER BY sample.observed_at DESC
            ))[1] AS latest_sample_count,
            (array_agg(
                round(sample.rx_bytes_sum / sample.sample_count::numeric)::bigint
                ORDER BY sample.observed_at DESC
            ))[1] AS latest_rx_bytes_avg,
            (array_agg(
                round(sample.tx_bytes_sum / sample.sample_count::numeric)::bigint
                ORDER BY sample.observed_at DESC
            ))[1] AS latest_tx_bytes_avg,
            (array_agg(
                sample.updated_at ORDER BY sample.observed_at DESC
            ))[1] AS latest_sample_updated_at,
            bool_and(
                NOT sample.inbound_promoted
                AND sample.sample_source NOT LIKE 'vnstat_import:%'
            ) AS ordinary_live
        FROM new_traffic_counter_sample_edges sample
        GROUP BY sample.client_id, sample.source_kind, sample.interface
    ), already_refreshed AS MATERIALIZED (
        SELECT changed.client_id, changed.source_kind, changed.interface
        FROM changed
        JOIN traffic_counter_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
        WHERE changed.row_count = 1
          AND changed.ordinary_live
          AND stream.source_revision = stream.materialized_revision
          AND stream.sample_edge_revision = stream.materialized_revision
          AND stream.promoted_boundary_safe
          AND stream.first_exact_observed_at IS NOT NULL
          AND stream.first_unpromoted_observed_at IS NOT NULL
          AND stream.latest_sample_observed_at = changed.latest_observed_at
          AND stream.latest_sample_rx_bytes = changed.latest_rx_bytes
          AND stream.latest_sample_tx_bytes = changed.latest_tx_bytes
          AND stream.latest_sample_rx_counter_epoch =
                changed.latest_rx_counter_epoch
          AND stream.latest_sample_tx_counter_epoch =
                changed.latest_tx_counter_epoch
          AND stream.latest_sample_source = changed.latest_sample_source
          AND stream.latest_sample_effective_observed_at =
                changed.latest_effective_observed_at
          AND stream.latest_sample_count = changed.latest_sample_count
          AND stream.latest_sample_rx_bytes_avg = changed.latest_rx_bytes_avg
          AND stream.latest_sample_tx_bytes_avg = changed.latest_tx_bytes_avg
          AND stream.latest_sample_updated_at = changed.latest_sample_updated_at
          AND stream.last_exact_observed_at = changed.latest_observed_at
    ), initially_refreshed AS (
        UPDATE traffic_counter_streams stream
        SET
            sample_edge_revision = stream.materialized_revision,
            latest_sample_observed_at = changed.latest_observed_at,
            latest_sample_rx_bytes = changed.latest_rx_bytes,
            latest_sample_tx_bytes = changed.latest_tx_bytes,
            latest_sample_rx_counter_epoch = changed.latest_rx_counter_epoch,
            latest_sample_tx_counter_epoch = changed.latest_tx_counter_epoch,
            latest_sample_source = changed.latest_sample_source,
            latest_sample_effective_observed_at =
                changed.latest_effective_observed_at,
            latest_sample_count = changed.latest_sample_count,
            latest_sample_rx_bytes_avg = changed.latest_rx_bytes_avg,
            latest_sample_tx_bytes_avg = changed.latest_tx_bytes_avg,
            latest_sample_updated_at = changed.latest_sample_updated_at,
            first_exact_observed_at = changed.first_observed_at,
            last_exact_observed_at = changed.latest_observed_at,
            first_unpromoted_observed_at = changed.first_observed_at,
            promoted_boundary_safe = TRUE,
            updated_at = now()
        FROM changed
        WHERE stream.client_id = changed.client_id
          AND stream.source_kind = changed.source_kind
          AND stream.interface = changed.interface
          AND changed.row_count = 1
          AND changed.ordinary_live
          AND stream.source_revision = 1
          AND stream.materialized_revision = 1
          AND stream.sample_edge_revision = 0
          AND stream.usage_row_count = 1
          AND stream.latest_sample_observed_at IS NULL
          AND stream.latest_sample_rx_bytes IS NULL
          AND stream.latest_sample_tx_bytes IS NULL
          AND stream.latest_sample_rx_counter_epoch IS NULL
          AND stream.latest_sample_tx_counter_epoch IS NULL
          AND stream.latest_sample_source IS NULL
          AND stream.latest_sample_effective_observed_at IS NULL
          AND stream.latest_sample_count IS NULL
          AND stream.latest_sample_rx_bytes_avg IS NULL
          AND stream.latest_sample_tx_bytes_avg IS NULL
          AND stream.latest_sample_updated_at IS NULL
          AND stream.previous_sample_effective_observed_at IS NULL
          AND stream.previous_sample_rx_bytes IS NULL
          AND stream.previous_sample_tx_bytes IS NULL
          AND stream.previous_sample_rx_counter_epoch IS NULL
          AND stream.previous_sample_tx_counter_epoch IS NULL
          AND stream.first_exact_observed_at IS NULL
          AND stream.last_exact_observed_at IS NULL
          AND stream.first_unpromoted_observed_at IS NULL
        RETURNING stream.client_id, stream.source_kind, stream.interface
    ), directly_refreshed AS (
        UPDATE traffic_counter_streams stream
        SET
            sample_edge_revision = stream.materialized_revision,
            latest_sample_observed_at = changed.latest_observed_at,
            latest_sample_rx_bytes = changed.latest_rx_bytes,
            latest_sample_tx_bytes = changed.latest_tx_bytes,
            latest_sample_rx_counter_epoch = changed.latest_rx_counter_epoch,
            latest_sample_tx_counter_epoch = changed.latest_tx_counter_epoch,
            latest_sample_source = changed.latest_sample_source,
            latest_sample_effective_observed_at =
                changed.latest_effective_observed_at,
            latest_sample_count = changed.latest_sample_count,
            latest_sample_rx_bytes_avg = changed.latest_rx_bytes_avg,
            latest_sample_tx_bytes_avg = changed.latest_tx_bytes_avg,
            latest_sample_updated_at = changed.latest_sample_updated_at,
            previous_sample_effective_observed_at =
                stream.latest_sample_effective_observed_at,
            previous_sample_rx_bytes = stream.latest_sample_rx_bytes,
            previous_sample_tx_bytes = stream.latest_sample_tx_bytes,
            previous_sample_rx_counter_epoch =
                stream.latest_sample_rx_counter_epoch,
            previous_sample_tx_counter_epoch =
                stream.latest_sample_tx_counter_epoch,
            first_exact_observed_at = COALESCE(
                stream.first_exact_observed_at,
                changed.first_observed_at
            ),
            last_exact_observed_at = changed.latest_observed_at,
            promoted_boundary_safe = TRUE,
            updated_at = now()
        FROM changed
        WHERE stream.client_id = changed.client_id
          AND stream.source_kind = changed.source_kind
          AND stream.interface = changed.interface
          AND changed.ordinary_live
          AND changed.row_count = 1
          AND NOT EXISTS (
                SELECT 1
                FROM already_refreshed ready
                WHERE ready.client_id = changed.client_id
                  AND ready.source_kind = changed.source_kind
                  AND ready.interface = changed.interface
          )
          AND NOT EXISTS (
                SELECT 1
                FROM initially_refreshed initialized
                WHERE initialized.client_id = changed.client_id
                  AND initialized.source_kind = changed.source_kind
                  AND initialized.interface = changed.interface
          )
          AND stream.source_revision = stream.materialized_revision
          AND stream.sample_edge_revision =
                stream.materialized_revision - 1
          AND stream.promoted_boundary_safe
          AND stream.first_exact_observed_at IS NOT NULL
          AND stream.first_unpromoted_observed_at IS NOT NULL
          AND stream.last_exact_observed_at =
                stream.latest_sample_observed_at
          AND changed.first_observed_at >
                stream.latest_sample_observed_at
        RETURNING stream.client_id, stream.source_kind, stream.interface
    ), fallback AS (
        SELECT changed.client_id, changed.source_kind, changed.interface
        FROM changed
        WHERE NOT EXISTS (
            SELECT 1
            FROM already_refreshed ready
            WHERE ready.client_id = changed.client_id
              AND ready.source_kind = changed.source_kind
              AND ready.interface = changed.interface
        )
          AND NOT EXISTS (
            SELECT 1
            FROM directly_refreshed refreshed
            WHERE refreshed.client_id = changed.client_id
              AND refreshed.source_kind = changed.source_kind
              AND refreshed.interface = changed.interface
        )
          AND NOT EXISTS (
            SELECT 1
            FROM initially_refreshed initialized
            WHERE initialized.client_id = changed.client_id
              AND initialized.source_kind = changed.source_kind
              AND initialized.interface = changed.interface
        )
    )
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface),
        array_agg(source_kind ORDER BY client_id, source_kind, interface),
        array_agg(interface ORDER BY client_id, source_kind, interface)
    INTO client_ids, source_kinds, interfaces
    FROM fallback;
    IF COALESCE(array_length(client_ids, 1), 0) > 0 THEN
        PERFORM refresh_traffic_counter_sample_edges(
            client_ids, source_kinds, interfaces
        );
    END IF;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.refresh_traffic_counter_sample_edges_after_update() RETURNS trigger
    LANGUAGE plpgsql
    SET jit TO 'off'
    AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
BEGIN
    IF NOT EXISTS (SELECT 1 FROM old_traffic_counter_sample_edges)
       AND NOT EXISTS (SELECT 1 FROM new_traffic_counter_sample_edges) THEN
        RETURN NULL;
    END IF;
    DELETE FROM traffic_counter_promoted_boundaries boundary
    USING old_traffic_counter_sample_edges sample
    WHERE sample.inbound_promoted
      AND boundary.client_id = sample.client_id
      AND boundary.source_kind = sample.source_kind
      AND boundary.interface = sample.interface
      AND boundary.observed_at = sample.observed_at;

    INSERT INTO traffic_counter_promoted_boundaries (
        client_id, source_kind, interface, observed_at
    )
    SELECT DISTINCT
        sample.client_id,
        sample.source_kind,
        sample.interface,
        sample.observed_at
    FROM new_traffic_counter_sample_edges sample
    WHERE sample.inbound_promoted
    ON CONFLICT DO NOTHING;

    IF current_setting(
        'vpsman.traffic_sample_edges_prepublished', true
    ) = 'on' THEN
        PERFORM set_config(
            'vpsman.traffic_sample_edges_prepublished', 'off', TRUE
        );
        RETURN NULL;
    END IF;

    WITH old_rows AS MATERIALIZED (
        SELECT
            sample.client_id,
            sample.source_kind,
            sample.interface,
            count(*)::bigint AS row_count,
            min(sample.observed_at) AS observed_at,
            min(sample.rx_bytes) AS rx_bytes,
            min(sample.tx_bytes) AS tx_bytes,
            min(sample.rx_counter_epoch) AS rx_counter_epoch,
            min(sample.tx_counter_epoch) AS tx_counter_epoch,
            min(sample.sample_source) AS sample_source,
            min(sample.latest_observed_at) AS effective_observed_at,
            min(sample.sample_count) AS sample_count,
            min(round(
                sample.rx_bytes_sum / sample.sample_count::numeric
            )::bigint) AS rx_bytes_avg,
            min(round(
                sample.tx_bytes_sum / sample.sample_count::numeric
            )::bigint) AS tx_bytes_avg,
            min(sample.updated_at) AS sample_updated_at,
            bool_and(
                NOT sample.inbound_promoted
                AND sample.sample_source NOT LIKE 'vnstat_import:%'
            ) AS ordinary_live
        FROM old_traffic_counter_sample_edges sample
        GROUP BY sample.client_id, sample.source_kind, sample.interface
    ), new_rows AS MATERIALIZED (
        SELECT
            sample.client_id,
            sample.source_kind,
            sample.interface,
            count(*)::bigint AS row_count,
            min(sample.observed_at) AS observed_at,
            min(sample.rx_bytes) AS rx_bytes,
            min(sample.tx_bytes) AS tx_bytes,
            min(sample.rx_counter_epoch) AS rx_counter_epoch,
            min(sample.tx_counter_epoch) AS tx_counter_epoch,
            min(sample.sample_source) AS sample_source,
            min(sample.latest_observed_at) AS effective_observed_at,
            min(sample.sample_count) AS sample_count,
            min(round(
                sample.rx_bytes_sum / sample.sample_count::numeric
            )::bigint) AS rx_bytes_avg,
            min(round(
                sample.tx_bytes_sum / sample.sample_count::numeric
            )::bigint) AS tx_bytes_avg,
            min(sample.updated_at) AS sample_updated_at,
            bool_and(
                NOT sample.inbound_promoted
                AND sample.sample_source NOT LIKE 'vnstat_import:%'
            ) AS ordinary_live
        FROM new_traffic_counter_sample_edges sample
        GROUP BY sample.client_id, sample.source_kind, sample.interface
    ), changed AS MATERIALIZED (
        SELECT
            COALESCE(old_group.client_id, new_group.client_id) AS client_id,
            COALESCE(old_group.source_kind, new_group.source_kind)
                AS source_kind,
            COALESCE(old_group.interface, new_group.interface) AS interface,
            old_group.row_count AS old_row_count,
            new_group.row_count AS new_row_count,
            old_group.observed_at AS old_observed_at,
            new_group.observed_at AS new_observed_at,
            old_group.rx_bytes AS old_rx_bytes,
            old_group.tx_bytes AS old_tx_bytes,
            new_group.rx_bytes AS new_rx_bytes,
            new_group.tx_bytes AS new_tx_bytes,
            old_group.rx_counter_epoch AS old_rx_counter_epoch,
            old_group.tx_counter_epoch AS old_tx_counter_epoch,
            old_group.sample_source AS old_sample_source,
            new_group.rx_counter_epoch AS new_rx_counter_epoch,
            new_group.tx_counter_epoch AS new_tx_counter_epoch,
            new_group.sample_source AS new_sample_source,
            old_group.effective_observed_at AS old_effective_observed_at,
            new_group.effective_observed_at AS new_effective_observed_at,
            old_group.sample_count AS old_sample_count,
            new_group.sample_count AS new_sample_count,
            old_group.rx_bytes_avg AS old_rx_bytes_avg,
            new_group.rx_bytes_avg AS new_rx_bytes_avg,
            old_group.tx_bytes_avg AS old_tx_bytes_avg,
            new_group.tx_bytes_avg AS new_tx_bytes_avg,
            old_group.sample_updated_at AS old_sample_updated_at,
            new_group.sample_updated_at AS new_sample_updated_at,
            COALESCE(old_group.ordinary_live, FALSE) AS old_ordinary_live,
            COALESCE(new_group.ordinary_live, FALSE) AS new_ordinary_live
        FROM old_rows old_group
        FULL OUTER JOIN new_rows new_group
          ON new_group.client_id = old_group.client_id
         AND new_group.source_kind = old_group.source_kind
         AND new_group.interface = old_group.interface
    ), already_refreshed AS MATERIALIZED (
        SELECT changed.client_id, changed.source_kind, changed.interface
        FROM changed
        JOIN traffic_counter_streams stream
          ON stream.client_id = changed.client_id
         AND stream.source_kind = changed.source_kind
         AND stream.interface = changed.interface
        WHERE changed.old_row_count = 1
          AND changed.new_row_count = 1
          AND changed.old_ordinary_live
          AND changed.new_ordinary_live
          AND changed.old_observed_at = changed.new_observed_at
          AND stream.source_revision = stream.materialized_revision
          AND stream.sample_edge_revision = stream.materialized_revision
          AND stream.promoted_boundary_safe
          AND stream.first_exact_observed_at IS NOT NULL
          AND stream.first_unpromoted_observed_at IS NOT NULL
          AND stream.latest_sample_observed_at = changed.new_observed_at
          AND stream.last_exact_observed_at = changed.new_observed_at
          AND stream.latest_sample_rx_bytes = changed.new_rx_bytes
          AND stream.latest_sample_tx_bytes = changed.new_tx_bytes
          AND stream.latest_sample_rx_counter_epoch =
                changed.new_rx_counter_epoch
          AND stream.latest_sample_tx_counter_epoch =
                changed.new_tx_counter_epoch
          AND stream.latest_sample_source = changed.new_sample_source
          AND stream.latest_sample_effective_observed_at =
                changed.new_effective_observed_at
          AND stream.latest_sample_count = changed.new_sample_count
          AND stream.latest_sample_rx_bytes_avg = changed.new_rx_bytes_avg
          AND stream.latest_sample_tx_bytes_avg = changed.new_tx_bytes_avg
          AND stream.latest_sample_updated_at = changed.new_sample_updated_at
    ), directly_refreshed AS (
        UPDATE traffic_counter_streams stream
        SET
            sample_edge_revision = stream.materialized_revision,
            latest_sample_rx_bytes = changed.new_rx_bytes,
            latest_sample_tx_bytes = changed.new_tx_bytes,
            latest_sample_rx_counter_epoch = changed.new_rx_counter_epoch,
            latest_sample_tx_counter_epoch = changed.new_tx_counter_epoch,
            latest_sample_source = changed.new_sample_source,
            latest_sample_effective_observed_at =
                changed.new_effective_observed_at,
            latest_sample_count = changed.new_sample_count,
            latest_sample_rx_bytes_avg = changed.new_rx_bytes_avg,
            latest_sample_tx_bytes_avg = changed.new_tx_bytes_avg,
            latest_sample_updated_at = changed.new_sample_updated_at,
            updated_at = now()
        FROM changed
        WHERE stream.client_id = changed.client_id
          AND stream.source_kind = changed.source_kind
          AND stream.interface = changed.interface
          AND NOT EXISTS (
                SELECT 1
                FROM already_refreshed ready
                WHERE ready.client_id = changed.client_id
                  AND ready.source_kind = changed.source_kind
                  AND ready.interface = changed.interface
          )
          AND changed.old_row_count = 1
          AND changed.new_row_count = 1
          AND changed.old_ordinary_live
          AND changed.new_ordinary_live
          AND changed.old_observed_at = changed.new_observed_at
          AND stream.source_revision = stream.materialized_revision
          AND stream.sample_edge_revision =
                stream.materialized_revision - 1
          AND stream.promoted_boundary_safe
          AND stream.first_exact_observed_at IS NOT NULL
          AND stream.first_unpromoted_observed_at IS NOT NULL
          AND stream.latest_sample_observed_at = changed.old_observed_at
          AND stream.last_exact_observed_at = changed.old_observed_at
          AND stream.latest_sample_rx_bytes = changed.old_rx_bytes
          AND stream.latest_sample_tx_bytes = changed.old_tx_bytes
          AND stream.latest_sample_rx_counter_epoch =
                changed.old_rx_counter_epoch
          AND stream.latest_sample_tx_counter_epoch =
                changed.old_tx_counter_epoch
          AND stream.latest_sample_source = changed.old_sample_source
          AND stream.latest_sample_effective_observed_at =
                changed.old_effective_observed_at
          AND stream.latest_sample_count = changed.old_sample_count
          AND stream.latest_sample_rx_bytes_avg = changed.old_rx_bytes_avg
          AND stream.latest_sample_tx_bytes_avg = changed.old_tx_bytes_avg
          AND stream.latest_sample_updated_at = changed.old_sample_updated_at
        RETURNING stream.client_id, stream.source_kind, stream.interface
    ), fallback AS (
        SELECT changed.client_id, changed.source_kind, changed.interface
        FROM changed
        WHERE NOT EXISTS (
            SELECT 1
            FROM already_refreshed ready
            WHERE ready.client_id = changed.client_id
              AND ready.source_kind = changed.source_kind
              AND ready.interface = changed.interface
        )
          AND NOT EXISTS (
            SELECT 1
            FROM directly_refreshed refreshed
            WHERE refreshed.client_id = changed.client_id
              AND refreshed.source_kind = changed.source_kind
              AND refreshed.interface = changed.interface
        )
    )
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface),
        array_agg(source_kind ORDER BY client_id, source_kind, interface),
        array_agg(interface ORDER BY client_id, source_kind, interface)
    INTO client_ids, source_kinds, interfaces
    FROM fallback;
    IF COALESCE(array_length(client_ids, 1), 0) > 0 THEN
        PERFORM refresh_traffic_counter_sample_edges(
            client_ids, source_kinds, interfaces
        );
    END IF;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.replace_traffic_counter_hourly_usage_totals_after_update() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF current_setting(
        'vpsman.traffic_hourly_derivations_prepublished', true
    ) = 'on' THEN
        RETURN NULL;
    END IF;

    WITH deltas AS (
        SELECT
            row.client_id,
            row.source_kind,
            row.interface,
            SUM(row.rx_bytes)::bigint AS rx_bytes,
            SUM(row.tx_bytes)::bigint AS tx_bytes,
            SUM(row.rx_reset_count)::bigint AS rx_reset_count,
            SUM(row.tx_reset_count)::bigint AS tx_reset_count,
            SUM(row.row_count)::bigint AS row_count
        FROM (
            SELECT
                old_row.client_id,
                old_row.source_kind,
                old_row.interface,
                -old_row.rx_bytes::bigint AS rx_bytes,
                -old_row.tx_bytes::bigint AS tx_bytes,
                -old_row.rx_reset_count::bigint AS rx_reset_count,
                -old_row.tx_reset_count::bigint AS tx_reset_count,
                -1::bigint AS row_count
            FROM old_traffic_counter_hourly_usage old_row
            UNION ALL
            SELECT
                new_row.client_id,
                new_row.source_kind,
                new_row.interface,
                new_row.rx_bytes::bigint,
                new_row.tx_bytes::bigint,
                new_row.rx_reset_count::bigint,
                new_row.tx_reset_count::bigint,
                1::bigint
            FROM new_traffic_counter_hourly_usage new_row
        ) row
        GROUP BY row.client_id, row.source_kind, row.interface
    )
    UPDATE traffic_counter_streams stream
    SET
        usage_rx_bytes = stream.usage_rx_bytes + delta.rx_bytes,
        usage_tx_bytes = stream.usage_tx_bytes + delta.tx_bytes,
        usage_rx_reset_count =
            stream.usage_rx_reset_count + delta.rx_reset_count,
        usage_tx_reset_count =
            stream.usage_tx_reset_count + delta.tx_reset_count,
        usage_row_count = stream.usage_row_count + delta.row_count,
        updated_at = now()
    FROM deltas delta
    WHERE stream.client_id = delta.client_id
      AND stream.source_kind = delta.source_kind
      AND stream.interface = delta.interface;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.subtract_traffic_counter_hourly_usage_totals_after_delete() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    WITH removed AS (
        SELECT
            client_id,
            source_kind,
            interface,
            COALESCE(SUM(rx_bytes), 0)::bigint AS rx_bytes,
            COALESCE(SUM(tx_bytes), 0)::bigint AS tx_bytes,
            COALESCE(SUM(rx_reset_count), 0)::bigint AS rx_reset_count,
            COALESCE(SUM(tx_reset_count), 0)::bigint AS tx_reset_count,
            COUNT(*)::bigint AS row_count
        FROM old_traffic_counter_hourly_usage
        GROUP BY client_id, source_kind, interface
    )
    UPDATE traffic_counter_streams stream
    SET
        usage_rx_bytes = stream.usage_rx_bytes - removed.rx_bytes,
        usage_tx_bytes = stream.usage_tx_bytes - removed.tx_bytes,
        usage_rx_reset_count =
            stream.usage_rx_reset_count - removed.rx_reset_count,
        usage_tx_reset_count =
            stream.usage_tx_reset_count - removed.tx_reset_count,
        usage_row_count = stream.usage_row_count - removed.row_count,
        updated_at = now()
    FROM removed
    WHERE stream.client_id = removed.client_id
      AND stream.source_kind = removed.source_kind
      AND stream.interface = removed.interface;
    RETURN NULL;
END;
$$;



CREATE FUNCTION public.traffic_counter_cycle_start_utc(reset_day integer, reset_hour integer, as_of timestamp with time zone) RETURNS timestamp with time zone
    LANGUAGE sql IMMUTABLE STRICT PARALLEL SAFE
    AS $$
    WITH current_month AS (
        SELECT date_trunc('month', as_of AT TIME ZONE 'UTC')::date
            AS month_start
    ), current_boundary AS (
        SELECT (
            month_start
            + LEAST(
                reset_day,
                EXTRACT(
                    DAY FROM month_start + interval '1 month - 1 day'
                )::integer
              ) - 1
        )::timestamp AT TIME ZONE 'UTC'
            + make_interval(hours => reset_hour) AS boundary
        FROM current_month
    ), selected_month AS (
        SELECT CASE
            WHEN as_of >= boundary
            THEN date_trunc('month', as_of AT TIME ZONE 'UTC')::date
            ELSE (
                date_trunc('month', as_of AT TIME ZONE 'UTC')
                - interval '1 month'
            )::date
        END AS month_start
        FROM current_boundary
    )
    SELECT (
        month_start
        + LEAST(
            reset_day,
            EXTRACT(
                DAY FROM month_start + interval '1 month - 1 day'
            )::integer
          ) - 1
    )::timestamp AT TIME ZONE 'UTC' + make_interval(hours => reset_hour)
    FROM selected_month
$$;



-- Tables.

CREATE TABLE public.traffic_counter_hourly_usage (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    rx_bytes bigint DEFAULT 0 NOT NULL,
    tx_bytes bigint DEFAULT 0 NOT NULL,
    rx_reset_count integer DEFAULT 0 NOT NULL,
    tx_reset_count integer DEFAULT 0 NOT NULL,
    sample_count integer NOT NULL,
    first_observed_at timestamp with time zone NOT NULL,
    latest_observed_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT traffic_counter_hourly_usage_bucket_start_check CHECK ((bucket_start = date_bin('01:00:00'::interval, bucket_start, '1970-01-01 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT traffic_counter_hourly_usage_check CHECK (((rx_bytes >= 0) AND (tx_bytes >= 0))),
    CONSTRAINT traffic_counter_hourly_usage_check1 CHECK (((rx_reset_count >= 0) AND (tx_reset_count >= 0))),
    CONSTRAINT traffic_counter_hourly_usage_check2 CHECK (((first_observed_at >= bucket_start) AND (latest_observed_at < (bucket_start + '01:00:00'::interval)) AND (first_observed_at <= latest_observed_at))),
    CONSTRAINT traffic_counter_hourly_usage_interface_check CHECK (((length(interface) >= 1) AND (length(interface) <= 128))),
    CONSTRAINT traffic_counter_hourly_usage_sample_count_check CHECK ((sample_count > 0)),
    CONSTRAINT traffic_counter_hourly_usage_source_kind_check CHECK ((source_kind = ANY (ARRAY['host'::text, 'tunnel'::text]))),
    CONSTRAINT traffic_counter_hourly_usage_pkey PRIMARY KEY (client_id, source_kind, interface, bucket_start),
    CONSTRAINT traffic_counter_hourly_usage_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.traffic_counter_active_cycle_usage (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    cycle_start timestamp with time zone NOT NULL,
    completed_through timestamp with time zone NOT NULL,
    rx_bytes bigint DEFAULT 0 NOT NULL,
    tx_bytes bigint DEFAULT 0 NOT NULL,
    rx_reset_count bigint DEFAULT 0 NOT NULL,
    tx_reset_count bigint DEFAULT 0 NOT NULL,
    source_revision bigint DEFAULT 0 NOT NULL,
    materialized_revision bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT traffic_counter_active_cycle_usage_check CHECK (((completed_through = date_bin('01:00:00'::interval, completed_through, '1970-01-01 00:00:00+00'::timestamp with time zone)) AND (completed_through >= cycle_start))),
    CONSTRAINT traffic_counter_active_cycle_usage_check1 CHECK (((rx_bytes >= 0) AND (tx_bytes >= 0) AND (rx_reset_count >= 0) AND (tx_reset_count >= 0))),
    CONSTRAINT traffic_counter_active_cycle_usage_check2 CHECK (((source_revision >= 0) AND (materialized_revision >= 0) AND (materialized_revision <= source_revision))),
    CONSTRAINT traffic_counter_active_cycle_usage_cycle_start_check CHECK ((cycle_start = date_bin('01:00:00'::interval, cycle_start, '1970-01-01 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT traffic_counter_active_cycle_usage_interface_check CHECK (((length(interface) >= 1) AND (length(interface) <= 128))),
    CONSTRAINT traffic_counter_active_cycle_usage_source_kind_check CHECK ((source_kind = ANY (ARRAY['host'::text, 'tunnel'::text]))),
    CONSTRAINT traffic_counter_active_cycle_usage_pkey PRIMARY KEY (client_id, source_kind, interface),
    CONSTRAINT traffic_counter_active_cycle__client_id_source_kind_interf_fkey FOREIGN KEY (client_id, source_kind, interface) REFERENCES public.traffic_counter_streams(client_id, source_kind, interface) ON DELETE CASCADE
);



-- One bounded durable owner per client. Producers only advance the requested
-- revision; the worker consumer leases, reconstructs, and publishes it.
CREATE TABLE public.traffic_counter_active_cycle_rebuild_work (
    client_id text NOT NULL,
    requested_revision bigint DEFAULT 1 NOT NULL,
    materialized_revision bigint DEFAULT 0 NOT NULL,
    requested_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    lease_id uuid,
    lease_until timestamp with time zone,
    last_error text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT traffic_counter_active_cycle_rebuild_work_pkey PRIMARY KEY (client_id),
    CONSTRAINT traffic_counter_active_cycle_rebuild_work_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT traffic_counter_active_cycle_rebuild_work_revision_check CHECK ((requested_revision >= 1) AND (materialized_revision >= 0) AND (materialized_revision <= requested_revision)),
    CONSTRAINT traffic_counter_active_cycle_rebuild_work_lease_shape_check CHECK (((lease_id IS NULL) = (lease_until IS NULL)))
);



CREATE TABLE public.traffic_counter_promoted_boundaries (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    CONSTRAINT traffic_counter_promoted_boundaries_interface_check CHECK (((length(interface) >= 1) AND (length(interface) <= 128))),
    CONSTRAINT traffic_counter_promoted_boundaries_observed_at_check CHECK ((observed_at = date_trunc('minute'::text, observed_at))),
    CONSTRAINT traffic_counter_promoted_boundaries_source_kind_check CHECK ((source_kind = ANY (ARRAY['host'::text, 'tunnel'::text]))),
    CONSTRAINT traffic_counter_promoted_boundaries_pkey PRIMARY KEY (client_id, source_kind, interface, observed_at),
    CONSTRAINT traffic_counter_promoted_boundaries_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.traffic_counter_rollup_summary_streams (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    source_revision bigint DEFAULT 0 NOT NULL,
    materialized_revision bigint DEFAULT 0 NOT NULL,
    rollup_row_count bigint DEFAULT 0 NOT NULL,
    tier_count integer DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT traffic_counter_rollup_summary_stre_materialized_revision_check CHECK ((materialized_revision >= 0)),
    CONSTRAINT traffic_counter_rollup_summary_streams_check CHECK ((materialized_revision <= source_revision)),
    CONSTRAINT traffic_counter_rollup_summary_streams_interface_check CHECK (((length(interface) >= 1) AND (length(interface) <= 128))),
    CONSTRAINT traffic_counter_rollup_summary_streams_rollup_row_count_check CHECK ((rollup_row_count >= 0)),
    CONSTRAINT traffic_counter_rollup_summary_streams_source_kind_check CHECK ((source_kind = ANY (ARRAY['host'::text, 'tunnel'::text]))),
    CONSTRAINT traffic_counter_rollup_summary_streams_source_revision_check CHECK ((source_revision >= 0)),
    CONSTRAINT traffic_counter_rollup_summary_streams_tier_count_check CHECK ((tier_count >= 0)),
    CONSTRAINT traffic_counter_rollup_summary_streams_pkey PRIMARY KEY (client_id, source_kind, interface),
    CONSTRAINT traffic_counter_rollup_summary_streams_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.traffic_counter_rollup_tier_summaries (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    origin_kind text NOT NULL,
    bucket_secs integer NOT NULL,
    first_bucket_start timestamp with time zone NOT NULL,
    latest_bucket_start timestamp with time zone NOT NULL,
    last_bucket_end timestamp with time zone NOT NULL,
    rx_bytes bigint NOT NULL,
    tx_bytes bigint NOT NULL,
    rx_reset_count bigint NOT NULL,
    tx_reset_count bigint NOT NULL,
    rollup_row_count bigint NOT NULL,
    materialized_revision bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT traffic_counter_rollup_tier_summari_materialized_revision_check CHECK ((materialized_revision >= 0)),
    CONSTRAINT traffic_counter_rollup_tier_summaries_bucket_secs_check CHECK ((bucket_secs = ANY (ARRAY[3600, 10800, 21600, 86400]))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_check CHECK ((first_bucket_start <= latest_bucket_start)),
    CONSTRAINT traffic_counter_rollup_tier_summaries_check1 CHECK ((last_bucket_end = (latest_bucket_start + make_interval(secs => (bucket_secs)::double precision)))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_check2 CHECK (((rx_bytes >= 0) AND (tx_bytes >= 0))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_check3 CHECK (((rx_reset_count >= 0) AND (tx_reset_count >= 0))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_interface_check CHECK (((length(interface) >= 1) AND (length(interface) <= 128))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_origin_kind_check CHECK ((origin_kind = ANY (ARRAY['live'::text, 'vnstat_import'::text]))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_rollup_row_count_check CHECK ((rollup_row_count > 0)),
    CONSTRAINT traffic_counter_rollup_tier_summaries_source_kind_check CHECK ((source_kind = ANY (ARRAY['host'::text, 'tunnel'::text]))),
    CONSTRAINT traffic_counter_rollup_tier_summaries_pkey PRIMARY KEY (client_id, source_kind, interface, origin_kind, bucket_secs),
    CONSTRAINT traffic_counter_rollup_tier_s_client_id_source_kind_interf_fkey FOREIGN KEY (client_id, source_kind, interface) REFERENCES public.traffic_counter_rollup_summary_streams(client_id, source_kind, interface) ON DELETE CASCADE
);



CREATE TABLE public.traffic_counter_rollups (
    client_id text NOT NULL,
    source_kind text NOT NULL,
    interface text NOT NULL,
    origin_kind text NOT NULL,
    bucket_secs integer NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    rx_bytes bigint DEFAULT 0 NOT NULL,
    tx_bytes bigint DEFAULT 0 NOT NULL,
    rx_valid_count integer DEFAULT 0 NOT NULL,
    tx_valid_count integer DEFAULT 0 NOT NULL,
    any_valid_count integer DEFAULT 0 NOT NULL,
    rx_reset_count integer DEFAULT 0 NOT NULL,
    tx_reset_count integer DEFAULT 0 NOT NULL,
    any_reset_count integer DEFAULT 0 NOT NULL,
    first_observed_at timestamp with time zone NOT NULL,
    latest_observed_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT traffic_counter_rollups_bucket_secs_check CHECK ((bucket_secs = ANY (ARRAY[3600, 10800, 21600, 86400]))),
    CONSTRAINT traffic_counter_rollups_check CHECK ((((EXTRACT(epoch FROM bucket_start))::bigint % (bucket_secs)::bigint) = 0)),
    CONSTRAINT traffic_counter_rollups_check1 CHECK (((rx_bytes >= 0) AND (tx_bytes >= 0))),
    CONSTRAINT traffic_counter_rollups_check2 CHECK (((rx_valid_count >= 0) AND (tx_valid_count >= 0) AND (any_valid_count >= 0) AND (rx_reset_count >= 0) AND (tx_reset_count >= 0) AND (any_reset_count >= 0))),
    CONSTRAINT traffic_counter_rollups_check3 CHECK ((first_observed_at <= latest_observed_at)),
    CONSTRAINT traffic_counter_rollups_check4 CHECK ((first_observed_at >= bucket_start)),
    CONSTRAINT traffic_counter_rollups_check5 CHECK ((latest_observed_at < (bucket_start + make_interval(secs => (bucket_secs)::double precision)))),
    CONSTRAINT traffic_counter_rollups_interface_check CHECK (((length(interface) >= 1) AND (length(interface) <= 128))),
    CONSTRAINT traffic_counter_rollups_origin_kind_check CHECK ((origin_kind = ANY (ARRAY['live'::text, 'vnstat_import'::text]))),
    CONSTRAINT traffic_counter_rollups_source_kind_check CHECK ((source_kind = ANY (ARRAY['host'::text, 'tunnel'::text]))),
    CONSTRAINT traffic_counter_rollups_pkey PRIMARY KEY (client_id, source_kind, interface, origin_kind, bucket_secs, bucket_start),
    CONSTRAINT traffic_counter_rollups_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.vps_rule_values (
    client_id text NOT NULL,
    key text NOT NULL,
    value_raw text NOT NULL,
    value_json jsonb NOT NULL,
    source_kind text DEFAULT 'operator'::text NOT NULL,
    source_id uuid,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT vps_rule_values_key_check CHECK ((key = ANY (ARRAY['traffic.reset_day'::text, 'traffic.quota.total'::text, 'traffic.quota.rx'::text, 'traffic.quota.tx'::text, 'traffic.selectors'::text, 'billing.price'::text, 'billing.cycle'::text, 'network.port_speed'::text, 'network.interfaces'::text, 'network.rate.interfaces'::text, 'product.name'::text]))),
    CONSTRAINT vps_rule_values_value_json_check CHECK ((jsonb_typeof(value_json) = 'object'::text)),
    CONSTRAINT vps_rule_values_value_raw_check CHECK (((length(value_raw) >= 1) AND (length(value_raw) <= 4096))),
    CONSTRAINT vps_rule_values_pkey PRIMARY KEY (client_id, key),
    CONSTRAINT vps_rule_values_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT vps_rule_values_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id)
);


-- Resolve the current network-interface rule and managed tunnel names once
-- per requested client.  This relation-reading stage belongs here because
-- vps_rule_values is traffic/accounting state; the immutable admission
-- predicate itself remains in the telemetry schema.  The function stores no
-- derived state, so the next statement observes every committed rule/plan
-- change.
CREATE FUNCTION public.resolve_telemetry_interface_policies(
    p_client_ids TEXT[]
)
RETURNS TABLE (
    client_id TEXT,
    admission_mode TEXT,
    interface_patterns TEXT[],
    managed_tunnel_interfaces TEXT[]
)
LANGUAGE sql
STABLE
STRICT
PARALLEL SAFE
AS $$
    WITH requested AS MATERIALIZED (
        SELECT DISTINCT requested.client_id
        FROM unnest(p_client_ids) requested(client_id)
        WHERE requested.client_id IS NOT NULL
    ), rule_snapshot AS MATERIALIZED (
        SELECT requested.client_id,
               rule.client_id IS NOT NULL AS has_rule,
               rule.value_json
        FROM requested
        LEFT JOIN public.vps_rule_values rule
          ON rule.client_id = requested.client_id
         AND rule.key = 'network.interfaces'
    ), managed_endpoint AS MATERIALIZED (
        SELECT plan.left_client_id AS client_id,
               plan.plan ->> 'interface_name' AS interface
        FROM public.tunnel_plans plan
        JOIN requested ON requested.client_id = plan.left_client_id
        WHERE plan.enabled IS TRUE
          AND plan.deleted_at IS NULL

        UNION

        SELECT plan.right_client_id AS client_id,
               plan.plan ->> 'interface_name' AS interface
        FROM public.tunnel_plans plan
        JOIN requested ON requested.client_id = plan.right_client_id
        WHERE plan.enabled IS TRUE
          AND plan.deleted_at IS NULL
    ), managed AS MATERIALIZED (
        SELECT endpoint.client_id,
               array_agg(
                   endpoint.interface ORDER BY endpoint.interface COLLATE "C"
               ) AS interfaces
        FROM managed_endpoint endpoint
        WHERE endpoint.interface IS NOT NULL
        GROUP BY endpoint.client_id
    )
    SELECT rule.client_id,
           CASE WHEN rule.has_rule
                THEN rule.value_json ->> 'mode'
                ELSE 'default_physical'
           END AS admission_mode,
           CASE WHEN rule.has_rule
                  AND rule.value_json ->> 'mode' = 'patterns'
                THEN ARRAY(
                    SELECT pattern.value
                    FROM jsonb_array_elements_text(
                        COALESCE(
                            rule.value_json -> 'patterns',
                            '[]'::JSONB
                        )
                    ) WITH ORDINALITY pattern(value, ordinal)
                    ORDER BY pattern.ordinal
                )
                ELSE ARRAY[]::TEXT[]
           END AS interface_patterns,
           COALESCE(managed.interfaces, ARRAY[]::TEXT[])
                AS managed_tunnel_interfaces
    FROM rule_snapshot rule
    LEFT JOIN managed ON managed.client_id = rule.client_id
    ORDER BY rule.client_id
$$;



-- Indexes.

CREATE INDEX traffic_counter_rollups_range_idx ON public.traffic_counter_rollups USING btree (client_id, source_kind, interface, bucket_start, bucket_secs) INCLUDE (origin_kind, rx_bytes, tx_bytes, rx_valid_count, tx_valid_count, any_valid_count, rx_reset_count, tx_reset_count, any_reset_count, first_observed_at, latest_observed_at);



CREATE INDEX traffic_counter_rollups_retention_idx ON public.traffic_counter_rollups USING btree (bucket_secs, bucket_start);



CREATE INDEX traffic_counter_active_cycle_rebuild_work_due_idx ON public.traffic_counter_active_cycle_rebuild_work USING btree (next_attempt_at, lease_until, requested_at, client_id) WHERE (materialized_revision < requested_revision);



-- Exact raw-retention frontier. Ordinary append/update fast paths deliberately
-- do not assign this indexed column; it changes only when the first
-- unpromoted row changes, preserving HOT behavior for steady telemetry.
CREATE INDEX traffic_counter_streams_first_unpromoted_idx ON public.traffic_counter_streams USING btree (first_unpromoted_observed_at, client_id, source_kind, interface) WHERE (first_unpromoted_observed_at IS NOT NULL);



CREATE INDEX traffic_counter_samples_import_class_stream_idx ON public.traffic_counter_samples USING btree (client_id, source_kind, interface, ((sample_source ~~ 'vnstat_import:%'::text)), observed_at);



CREATE INDEX traffic_counter_samples_observed_idx ON public.traffic_counter_samples USING btree (observed_at DESC);



-- The raw network export reads only the one-day, unpromoted host frontier in
-- effective-observation order. This partial key lets that global page stop at
-- its requested rows without indexing tunnels, promoted rows, or retained
-- history outside that exact frontier.
CREATE INDEX traffic_counter_samples_unpromoted_host_effective_idx
ON public.traffic_counter_samples USING btree (
    latest_observed_at DESC, client_id, interface, observed_at DESC
)
WHERE source_kind = 'host' AND NOT inbound_promoted;



CREATE INDEX vps_rule_values_key_idx ON public.vps_rule_values USING btree (key, client_id);



-- Triggers.

CREATE TRIGGER traffic_counter_samples_normalize_aggregate
BEFORE INSERT OR UPDATE ON public.traffic_counter_samples
FOR EACH ROW
EXECUTE FUNCTION public.normalize_traffic_counter_sample_aggregate();



CREATE TRIGGER traffic_counter_samples_retention_insert
AFTER INSERT ON public.traffic_counter_samples
REFERENCING NEW TABLE AS new_traffic_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_traffic_counter_samples_retention_effect();



CREATE TRIGGER traffic_counter_samples_retention_update
AFTER UPDATE ON public.traffic_counter_samples
REFERENCING OLD TABLE AS old_traffic_retention_rows
            NEW TABLE AS new_traffic_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_traffic_counter_samples_retention_effect();



CREATE TRIGGER traffic_counter_rollups_retention_insert
AFTER INSERT ON public.traffic_counter_rollups
REFERENCING NEW TABLE AS new_traffic_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_traffic_counter_rollups_retention_effect();



CREATE TRIGGER traffic_counter_rollups_retention_update
AFTER UPDATE ON public.traffic_counter_rollups
REFERENCING OLD TABLE AS old_traffic_retention_rows
            NEW TABLE AS new_traffic_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_traffic_counter_rollups_retention_effect();



CREATE TRIGGER traffic_counter_active_cycle_after_delete AFTER DELETE ON public.traffic_counter_hourly_usage REFERENCING OLD TABLE AS old_traffic_counter_hourly_usage FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_traffic_counter_active_cycle_after_delete();



CREATE TRIGGER traffic_counter_active_cycle_after_insert AFTER INSERT ON public.traffic_counter_hourly_usage REFERENCING NEW TABLE AS new_traffic_counter_hourly_usage FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_traffic_counter_active_cycle_after_insert();



CREATE TRIGGER traffic_counter_active_cycle_after_rule_change AFTER INSERT OR DELETE OR UPDATE ON public.vps_rule_values FOR EACH ROW EXECUTE FUNCTION public.refresh_traffic_counter_active_cycle_after_rule_change();



CREATE TRIGGER traffic_counter_active_cycle_after_update AFTER UPDATE ON public.traffic_counter_hourly_usage REFERENCING OLD TABLE AS old_traffic_counter_hourly_usage NEW TABLE AS new_traffic_counter_hourly_usage FOR EACH STATEMENT EXECUTE FUNCTION public.maintain_traffic_counter_active_cycle_after_update();



CREATE TRIGGER traffic_counter_hourly_usage_after_delete AFTER DELETE ON public.traffic_counter_samples REFERENCING OLD TABLE AS old_traffic_counter_samples FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_hourly_usage_after_delete();



CREATE TRIGGER traffic_counter_hourly_usage_after_insert AFTER INSERT ON public.traffic_counter_samples REFERENCING NEW TABLE AS new_traffic_counter_samples FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_hourly_usage_after_insert();



CREATE TRIGGER traffic_counter_hourly_usage_after_update AFTER UPDATE ON public.traffic_counter_samples REFERENCING OLD TABLE AS old_traffic_counter_samples NEW TABLE AS new_traffic_counter_samples FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_hourly_usage_after_update();



CREATE TRIGGER traffic_counter_hourly_usage_totals_after_delete AFTER DELETE ON public.traffic_counter_hourly_usage REFERENCING OLD TABLE AS old_traffic_counter_hourly_usage FOR EACH STATEMENT EXECUTE FUNCTION public.subtract_traffic_counter_hourly_usage_totals_after_delete();



CREATE TRIGGER traffic_counter_hourly_usage_totals_after_insert AFTER INSERT ON public.traffic_counter_hourly_usage REFERENCING NEW TABLE AS new_traffic_counter_hourly_usage FOR EACH STATEMENT EXECUTE FUNCTION public.add_traffic_counter_hourly_usage_totals_after_insert();



CREATE TRIGGER traffic_counter_hourly_usage_totals_after_update AFTER UPDATE ON public.traffic_counter_hourly_usage REFERENCING OLD TABLE AS old_traffic_counter_hourly_usage NEW TABLE AS new_traffic_counter_hourly_usage FOR EACH STATEMENT EXECUTE FUNCTION public.replace_traffic_counter_hourly_usage_totals_after_update();



CREATE TRIGGER traffic_counter_rollup_summaries_after_delete AFTER DELETE ON public.traffic_counter_rollups REFERENCING OLD TABLE AS old_traffic_counter_rollups FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_rollup_summaries_after_delete();



CREATE TRIGGER traffic_counter_rollup_summaries_after_insert AFTER INSERT ON public.traffic_counter_rollups REFERENCING NEW TABLE AS new_traffic_counter_rollups FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_rollup_summaries_after_insert();



CREATE TRIGGER traffic_counter_rollup_summaries_after_update AFTER UPDATE ON public.traffic_counter_rollups REFERENCING OLD TABLE AS old_traffic_counter_rollups NEW TABLE AS new_traffic_counter_rollups FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_rollup_summaries_after_update();



CREATE TRIGGER traffic_counter_sample_edges_after_delete AFTER DELETE ON public.traffic_counter_samples REFERENCING OLD TABLE AS old_traffic_counter_sample_edges FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_sample_edges_after_delete();



CREATE TRIGGER traffic_counter_sample_edges_after_insert AFTER INSERT ON public.traffic_counter_samples REFERENCING NEW TABLE AS new_traffic_counter_sample_edges FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_sample_edges_after_insert();



CREATE TRIGGER traffic_counter_sample_edges_after_update AFTER UPDATE ON public.traffic_counter_samples REFERENCING OLD TABLE AS old_traffic_counter_sample_edges NEW TABLE AS new_traffic_counter_sample_edges FOR EACH STATEMENT EXECUTE FUNCTION public.refresh_traffic_counter_sample_edges_after_update();
