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

