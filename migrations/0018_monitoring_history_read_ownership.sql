-- Monitoring history read ownership: Network, Ping, then Resources.
-- Engineering-only read definitions; tables, stored data and retention are unchanged.

-- Keep projected raw network shadow membership with its exact client/interface
-- owner.  Only the read execution shape changes: admission, raw minute
-- replacement, strict durable predecessors, import boundaries and counter
-- epochs retain the existing canonical definition.  Existing migrations stay
-- immutable so upgraded and fresh databases receive this same replacement.
CREATE OR REPLACE FUNCTION public.telemetry_projected_raw_network_minutes_source(
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
), touched_owners AS MATERIALIZED (
    -- Shadow membership belongs to one exact host stream.  Carry its minute
    -- set into each lookup instead of rescanning every requested stream's
    -- touched rows from a correlated InitPlan, including for NULL edges.
    SELECT client_id, interface,
           array_agg(DISTINCT bucket_start ORDER BY bucket_start) AS raw_minutes
    FROM expanded
    WHERE is_raw
    GROUP BY client_id, interface
), touched_predecessors AS MATERIALIZED (
    -- Missing wall minutes do not own counter continuity.  Every touched raw
    -- coordinate resolves its latest durable coordinate after excluding rows
    -- shadowed by another touched raw minute.  Raw minutes with the same
    -- predecessor are therefore one stream even across a wall gap; an actual
    -- intervening durable/import observation is the only segment boundary.
    SELECT owner.client_id, owner.interface, touched.bucket_start,
           predecessor.bucket_start AS durable_predecessor_bucket_start,
           predecessor.rx_bytes AS prior_rx_bytes,
           predecessor.tx_bytes AS prior_tx_bytes,
           COALESCE(predecessor.rx_counter_epoch, 0)
               AS prior_rx_counter_epoch,
           COALESCE(predecessor.tx_counter_epoch, 0)
               AS prior_tx_counter_epoch,
           predecessor.sample_source AS prior_sample_source
    FROM touched_owners owner
    CROSS JOIN LATERAL unnest(owner.raw_minutes) touched(bucket_start)
    LEFT JOIN public.traffic_counter_streams stream
      ON stream.client_id = owner.client_id
     AND stream.source_kind = 'host'
     AND stream.interface = owner.interface
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
              AND NOT (
                  date_trunc('minute', stream.latest_sample_observed_at)
                      = ANY(owner.raw_minutes)
              )

            UNION ALL

            SELECT sample.observed_at AS bucket_start,
                   sample.rx_bytes, sample.tx_bytes,
                   sample.rx_counter_epoch, sample.tx_counter_epoch,
                   sample.sample_source,
                   0::INTEGER AS source_priority
            FROM public.traffic_counter_samples sample
            WHERE sample.client_id = owner.client_id
              AND sample.source_kind = 'host'
              AND sample.interface = owner.interface
              AND sample.observed_at < touched.bucket_start
              AND NOT (sample.observed_at = ANY(owner.raw_minutes))
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


-- Scope Ping history by its exact series before raw reconstruction. Existing
-- public views remain unchanged; callers retain assignment/generation policy.
-- Empty or NULL series arrays select no owners. Bounds apply to completed
-- canonical minutes, never to corrections before source identity is resolved.
CREATE FUNCTION public.telemetry_ping_points_source(
    p_series_ids BIGINT[],
    p_min_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_max_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_bucket_secs INTEGER DEFAULT NULL
)
RETURNS SETOF public.telemetry_ping_rollups
LANGUAGE sql
STABLE
AS $$
WITH requested_ids AS MATERIALIZED (
    SELECT DISTINCT requested.series_id
    FROM unnest(p_series_ids) requested(series_id)
), requested_series AS MATERIALIZED (
    SELECT series.id, series.client_id, series.target_id, series.generation
    FROM requested_ids requested
    JOIN public.telemetry_ping_series series ON series.id = requested.series_id
), requested_clients AS MATERIALIZED (
    SELECT DISTINCT series.client_id
    FROM requested_series series
), scoped_samples AS NOT MATERIALIZED (
    SELECT sample.*
    FROM requested_clients requested
    JOIN public.telemetry_minute_materialization_heads minute
      ON minute.client_id = requested.client_id
    JOIN public.telemetry_projection_heads projection
      ON projection.client_id = requested.client_id
    CROSS JOIN LATERAL (
        SELECT sample.*
        FROM public.telemetry_samples sample
        WHERE sample.client_id = requested.client_id
          AND sample.accepted_seq > minute.materialized_seq
          AND sample.accepted_seq <= projection.projected_seq
        OFFSET 0
    ) sample
), expanded AS MATERIALIZED (
    -- Decode each requested client's suffix once before matching its series.
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
    FROM scoped_samples sample
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(sample.payload -> 'ping_results') = 'array'
            THEN sample.payload -> 'ping_results' ELSE '[]'::JSONB END
    ) WITH ORDINALITY ping(value, ordinality)
    WHERE ping.ordinality <= cardinality(sample.ping_source_checked_unix)
), raw_evidence AS MATERIALIZED (
    SELECT
        series.id AS series_id,
        expanded.*
    FROM expanded
    JOIN requested_series series
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
    CROSS JOIN LATERAL (
        SELECT fact.*
        FROM public.telemetry_ping_facts fact
        WHERE fact.series_id = touched.series_id
          AND fact.checked_unix >= touched.bucket_start_unix * 60
          AND fact.checked_unix < (touched.bucket_start_unix + 1) * 60
        OFFSET 0
    ) fact
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
), projected_suffix AS MATERIALIZED (
    SELECT series_id, bucket_start, 60::INTEGER AS bucket_secs,
           sample_count, success_count, latency_sum_ms,
           latency_avg_ms, latency_min_ms, latency_max_ms,
           loss_ratio_avg, loss_ratio_sum, loss_ratio_max,
           latest_status, latest_reason, latest_checked_at, updated_at
    FROM grouped
    -- Corrections are canonical before output time filtering: their trusted
    -- checked time can move between minutes without changing source identity.
    WHERE bucket_start >= COALESCE(
              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
          )
      AND bucket_start <= COALESCE(
              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
          )
      AND (p_bucket_secs IS NULL OR p_bucket_secs = 60)
), point_owners AS MATERIALIZED (
    -- Keep raw shadow coordinates and rows once per exact series. Joining
    -- materialized owner lists back to the fleet suffix would repeat work.
    SELECT input.series_id,
           array_agg((input.point).bucket_start)
               FILTER (WHERE (input.point).series_id IS NOT NULL)
               AS suffix_bucket_starts,
           array_agg(input.point)
               FILTER (WHERE (input.point).series_id IS NOT NULL)
               AS suffix_points
    FROM (
        SELECT series.id AS series_id,
               NULL::public.telemetry_ping_rollups AS point
        FROM requested_series series
        UNION ALL
        SELECT suffix.series_id,
               ROW(suffix.*)::public.telemetry_ping_rollups
        FROM projected_suffix suffix
    ) input
    GROUP BY input.series_id
)
SELECT retained.*
FROM point_owners owner
CROSS JOIN LATERAL (
    SELECT retained.*
    FROM public.telemetry_ping_rollups retained
    WHERE retained.series_id = owner.series_id
      AND retained.bucket_start >= COALESCE(
              p_min_bucket_start, '-infinity'::TIMESTAMPTZ
          )
      AND retained.bucket_start <= COALESCE(
              p_max_bucket_start, 'infinity'::TIMESTAMPTZ
          )
      AND (p_bucket_secs IS NULL OR retained.bucket_secs = p_bucket_secs)
      AND (
          retained.bucket_secs <> 60
          OR NOT retained.bucket_start = ANY(COALESCE(
              owner.suffix_bucket_starts, ARRAY[]::TIMESTAMPTZ[]
          ))
      )
    OFFSET 0
) retained
UNION ALL
SELECT point.*
FROM point_owners owner
CROSS JOIN LATERAL unnest(owner.suffix_points) point
$$;


-- Each requested client owns its projected resource-minute replacement set.
-- Keep the canonical source, bounds and top-N ordering unchanged; carry only
-- that owner's suffix into its indexed retained-history lookup.
CREATE OR REPLACE FUNCTION public.telemetry_resource_points_source(
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
), projected_owners AS MATERIALIZED (
    -- Empty markers retain requested clients with no open raw minute without
    -- a second join to the fleet-wide materialized suffix. Nullable resource
    -- fields are valid: has_point identifies markers, not a row NULL test.
    SELECT source.client_id,
           array_agg((source.point).bucket_start)
               FILTER (WHERE source.has_point) AS raw_minutes,
           array_agg(source.point)
               FILTER (WHERE source.has_point) AS raw_points
    FROM (
        SELECT requested.client_id, FALSE AS has_point,
               NULL::public.telemetry_rollups AS point
        FROM requested_clients requested

        UNION ALL

        SELECT suffix.client_id, TRUE AS has_point,
               suffix::public.telemetry_rollups AS point
        FROM projected_suffix suffix
        WHERE p_client_ids IS NOT NULL
    ) source
    GROUP BY source.client_id
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
    FROM projected_owners owner
    CROSS JOIN LATERAL (
        SELECT candidate.*
        FROM (
            (
                SELECT retained.*
                FROM public.telemetry_rollups retained
                WHERE retained.client_id = owner.client_id
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
                  AND (
                      retained.bucket_secs <> 60
                      OR NOT COALESCE(
                          retained.bucket_start = ANY(owner.raw_minutes),
                          FALSE
                      )
                  )
                ORDER BY retained.bucket_start DESC,
                         retained.latest_observed_at DESC,
                         retained.bucket_secs ASC
                LIMIT p_per_owner_limit
                OFFSET 0
            )

            UNION ALL

            SELECT suffix.*
            FROM unnest(owner.raw_points) suffix
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

