-- Ordinary dashboard coordinates share one source snapshot, while every
-- (client, domain) remains an independently fenced publication transaction.

CREATE FUNCTION public.acquire_telemetry_dashboard_coordinate_projection_owners()
RETURNS TABLE (
    owner_id BIGINT,
    client_id TEXT,
    domain TEXT,
    ready_revision BIGINT
)
LANGUAGE plpgsql
AS $$
DECLARE
    candidate RECORD;
BEGIN
    -- There is deliberately no batch limit: this is the currently ready work
    -- relation, not a pacing mechanism. Generation and full-block work retain
    -- the established exact-owner path.
    FOR candidate IN
        SELECT ready.owner_id,
               fence.client_id,
               fence.domain,
               ready.wake_revision
        FROM public.telemetry_dashboard_ready_owners ready
        JOIN public.telemetry_dashboard_projection_fences fence
          ON fence.owner_id = ready.owner_id
        WHERE ready.retry_not_before <= clock_timestamp()
          AND EXISTS (
              SELECT 1
              FROM public.telemetry_dashboard_block_events event
              WHERE event.client_id = fence.client_id
                AND event.domain = fence.domain
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.telemetry_dashboard_generation_events event
              WHERE event.client_id = fence.client_id
                AND event.domain = fence.domain
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.telemetry_dashboard_block_events event
              WHERE event.client_id = fence.client_id
                AND event.domain = fence.domain
                AND event.event_kind <> 'coordinate'
          )
        ORDER BY ready.ready_at, ready.owner_id
    LOOP
        IF pg_try_advisory_lock(candidate.owner_id) THEN
            owner_id := candidate.owner_id;
            client_id := candidate.client_id;
            domain := candidate.domain;
            ready_revision := candidate.wake_revision;
            RETURN NEXT;
        END IF;
    END LOOP;
END
$$;

CREATE FUNCTION public.telemetry_dashboard_coordinate_projection_claims(
    p_owner_ids BIGINT[]
)
RETURNS TABLE (
    owner_id BIGINT,
    client_id TEXT,
    domain TEXT,
    ready_revision BIGINT,
    event_kind TEXT[],
    source_bucket_secs INTEGER[],
    block_start_unix BIGINT[],
    bucket_start_unix BIGINT[],
    captured_block_event_ids BIGINT[],
    expected_generation BIGINT,
    expected_revision BIGINT,
    generation_interfaces TEXT[],
    generation_source_kinds TEXT[]
)
LANGUAGE sql
STABLE
AS $$
    WITH requested AS MATERIALIZED (
        SELECT DISTINCT requested.owner_id
        FROM unnest(COALESCE(p_owner_ids, ARRAY[]::BIGINT[]))
            requested(owner_id)
    ), eligible AS MATERIALIZED (
        SELECT fence.owner_id,
               fence.client_id,
               fence.domain,
               ready.wake_revision
        FROM requested
        JOIN public.telemetry_dashboard_projection_fences fence
          ON fence.owner_id = requested.owner_id
        JOIN public.telemetry_dashboard_ready_owners ready
          ON ready.owner_id = fence.owner_id
        WHERE EXISTS (
              SELECT 1
              FROM public.telemetry_dashboard_block_events event
              WHERE event.client_id = fence.client_id
                AND event.domain = fence.domain
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.telemetry_dashboard_generation_events event
              WHERE event.client_id = fence.client_id
                AND event.domain = fence.domain
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.telemetry_dashboard_block_events event
              WHERE event.client_id = fence.client_id
                AND event.domain = fence.domain
                AND event.event_kind <> 'coordinate'
          )
    ), captured AS MATERIALIZED (
        SELECT eligible.owner_id,
               array_agg(event.event_id ORDER BY event.event_id) AS event_ids
        FROM eligible
        JOIN public.telemetry_dashboard_block_events event
          ON event.client_id = eligible.client_id
         AND event.domain = eligible.domain
        GROUP BY eligible.owner_id
    ), work_items AS MATERIALIZED (
        SELECT eligible.owner_id,
               event.source_bucket_secs,
               event.block_start_unix,
               event.bucket_start_unix
        FROM eligible
        JOIN public.telemetry_dashboard_block_events event
          ON event.client_id = eligible.client_id
         AND event.domain = eligible.domain
        GROUP BY eligible.owner_id,
                 event.source_bucket_secs,
                 event.block_start_unix,
                 event.bucket_start_unix
    ), work AS MATERIALIZED (
        SELECT item.owner_id,
               array_agg(
                   'coordinate'::TEXT
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.bucket_start_unix
               ) AS event_kind,
               array_agg(
                   item.source_bucket_secs
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.bucket_start_unix
               ) AS source_bucket_secs,
               array_agg(
                   item.block_start_unix
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.bucket_start_unix
               ) AS block_start_unix,
               array_agg(
                   item.bucket_start_unix
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.bucket_start_unix
               ) AS bucket_start_unix
        FROM work_items item
        GROUP BY item.owner_id
    ), headed AS MATERIALIZED (
        SELECT eligible.*,
               COALESCE(
                   resource_head.resource_generation,
                   network_head.network_generation,
                   traffic_head.traffic_generation
               ) AS expected_generation,
               COALESCE(
                   resource_head.resource_revision,
                   network_head.network_revision,
                   traffic_head.traffic_revision
               ) AS expected_revision,
               CASE eligible.domain
                   WHEN 'network' THEN
                       network_head.network_generation_interfaces
                   WHEN 'traffic' THEN
                       traffic_head.traffic_generation_interfaces
                   ELSE ARRAY[]::TEXT[]
               END AS generation_interfaces,
               CASE eligible.domain
                   WHEN 'traffic' THEN
                       traffic_head.traffic_generation_source_kinds
                   ELSE ARRAY[]::TEXT[]
               END AS generation_source_kinds
        FROM eligible
        LEFT JOIN public.telemetry_dashboard_resource_projection_heads
            resource_head
          ON eligible.domain = 'resource'
         AND resource_head.client_id = eligible.client_id
        LEFT JOIN public.telemetry_dashboard_network_projection_heads
            network_head
          ON eligible.domain = 'network'
         AND network_head.client_id = eligible.client_id
        LEFT JOIN public.telemetry_dashboard_traffic_projection_heads
            traffic_head
          ON eligible.domain = 'traffic'
         AND traffic_head.client_id = eligible.client_id
    )
    SELECT headed.owner_id,
           headed.client_id,
           headed.domain,
           headed.wake_revision,
           work.event_kind,
           work.source_bucket_secs,
           work.block_start_unix,
           work.bucket_start_unix,
           captured.event_ids,
           headed.expected_generation,
           headed.expected_revision,
           headed.generation_interfaces,
           headed.generation_source_kinds
    FROM headed
    JOIN captured USING (owner_id)
    JOIN work USING (owner_id)
    WHERE headed.expected_generation IS NOT NULL
      AND headed.expected_revision IS NOT NULL
    ORDER BY headed.owner_id
$$;

CREATE FUNCTION public.prepare_telemetry_dashboard_resource_coordinate_blocks(
    p_owner_ids BIGINT[]
)
RETURNS TABLE (
    owner_id BIGINT,
    source_bucket_secs INTEGER,
    block_start_unix BIGINT,
    has_samples BOOLEAN,
    sample_counts BIGINT[],
    cpu_load_1_sums DOUBLE PRECISION[],
    cpu_load_1_maxes REAL[],
    memory_total_bytes_maxes BIGINT[],
    memory_used_ratio_sums DOUBLE PRECISION[],
    memory_used_ratio_maxes REAL[],
    disk_sample_counts BIGINT[],
    disk_total_bytes_maxes BIGINT[],
    disk_used_ratio_sums DOUBLE PRECISION[],
    disk_used_ratio_maxes REAL[],
    latest_observed_unix BIGINT[]
)
LANGUAGE sql
STABLE
AS $$
    WITH claims AS MATERIALIZED (
        SELECT claim.*
        FROM public.telemetry_dashboard_coordinate_projection_claims(
            p_owner_ids
        ) claim
        WHERE claim.domain = 'resource'
    ), requested AS MATERIALIZED (
        SELECT claim.owner_id,
               claim.client_id,
               claim.expected_generation,
               claim.expected_revision + 1 AS published_revision,
               coordinate.source_bucket_secs,
               coordinate.bucket_start_unix,
               public.telemetry_dashboard_block_start(
                   coordinate.bucket_start_unix,
                   coordinate.source_bucket_secs
               ) AS block_start_unix
        FROM claims claim
        CROSS JOIN LATERAL unnest(
            claim.source_bucket_secs,
            claim.bucket_start_unix
        ) coordinate(source_bucket_secs, bucket_start_unix)
    ), coordinate_source AS MATERIALIZED (
        SELECT requested.*,
               source.client_id AS source_client_id,
               source.sample_count,
               source.cpu_load_1_sum,
               source.cpu_load_1_max,
               source.memory_total_bytes_max,
               source.memory_used_ratio_sum,
               source.memory_used_ratio_max,
               source.disk_sample_count,
               source.disk_total_bytes_max,
               source.disk_used_ratio_sum,
               source.disk_used_ratio_max,
               source.latest_observed_at
        FROM requested
        LEFT JOIN public.telemetry_rollups source
          ON source.client_id = requested.client_id
         AND source.bucket_secs = requested.source_bucket_secs
         AND source.bucket_start = to_timestamp(
             requested.bucket_start_unix
         )
    ), affected AS MATERIALIZED (
        SELECT DISTINCT requested.owner_id,
               requested.client_id,
               requested.expected_generation,
               requested.published_revision,
               requested.source_bucket_secs,
               requested.block_start_unix
        FROM requested
    ), assembled AS MATERIALIZED (
        SELECT affected.owner_id,
               affected.source_bucket_secs,
               affected.block_start_unix,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       COALESCE(source.sample_count, 0)::BIGINT
                   ELSE COALESCE(
                       prior.sample_counts[slot.ordinal + 1], 0
                   ) END ORDER BY slot.ordinal
               ) AS sample_counts,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.cpu_load_1_sum
                   ELSE prior.cpu_load_1_sums[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS cpu_load_1_sums,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.cpu_load_1_max::REAL
                   ELSE prior.cpu_load_1_maxes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS cpu_load_1_maxes,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.memory_total_bytes_max
                   ELSE prior.memory_total_bytes_maxes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS memory_total_bytes_maxes,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.memory_used_ratio_sum
                   ELSE prior.memory_used_ratio_sums[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS memory_used_ratio_sums,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.memory_used_ratio_max::REAL
                   ELSE prior.memory_used_ratio_maxes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS memory_used_ratio_maxes,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       COALESCE(source.disk_sample_count, 0)::BIGINT
                   ELSE COALESCE(
                       prior.disk_sample_counts[slot.ordinal + 1], 0
                   ) END ORDER BY slot.ordinal
               ) AS disk_sample_counts,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.disk_total_bytes_max
                   ELSE prior.disk_total_bytes_maxes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS disk_total_bytes_maxes,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.disk_used_ratio_sum
                   ELSE prior.disk_used_ratio_sums[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS disk_used_ratio_sums,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.disk_used_ratio_max::REAL
                   ELSE prior.disk_used_ratio_maxes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS disk_used_ratio_maxes,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       extract(epoch FROM source.latest_observed_at)::BIGINT
                   ELSE prior.latest_observed_unix[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS latest_observed_unix
        FROM affected
        CROSS JOIN generate_series(
            0, public.telemetry_dashboard_block_factor() - 1
        ) slot(ordinal)
        LEFT JOIN public.telemetry_dashboard_resource_blocks prior
          ON prior.client_id = affected.client_id
         AND prior.generation = affected.expected_generation
         AND prior.source_bucket_secs = affected.source_bucket_secs
         AND prior.block_start_unix = affected.block_start_unix
        LEFT JOIN coordinate_source source
          ON source.owner_id = affected.owner_id
         AND source.source_bucket_secs = affected.source_bucket_secs
         AND source.bucket_start_unix = affected.block_start_unix
                + slot.ordinal::BIGINT * affected.source_bucket_secs
        GROUP BY affected.owner_id,
                 affected.source_bucket_secs,
                 affected.block_start_unix
    )
    SELECT assembled.owner_id,
           assembled.source_bucket_secs,
           assembled.block_start_unix,
           EXISTS (
               SELECT 1
               FROM unnest(assembled.sample_counts) count(value)
               WHERE count.value > 0
           ),
           assembled.sample_counts,
           assembled.cpu_load_1_sums,
           assembled.cpu_load_1_maxes,
           assembled.memory_total_bytes_maxes,
           assembled.memory_used_ratio_sums,
           assembled.memory_used_ratio_maxes,
           assembled.disk_sample_counts,
           assembled.disk_total_bytes_maxes,
           assembled.disk_used_ratio_sums,
           assembled.disk_used_ratio_maxes,
           assembled.latest_observed_unix
    FROM assembled
    ORDER BY assembled.owner_id,
             assembled.source_bucket_secs,
             assembled.block_start_unix
$$;

CREATE FUNCTION public.prepare_telemetry_dashboard_network_coordinate_blocks(
    p_owner_ids BIGINT[]
)
RETURNS TABLE (
    owner_id BIGINT,
    source_bucket_secs INTEGER,
    block_start_unix BIGINT,
    interface_width INTEGER,
    has_samples BOOLEAN,
    sample_counts BIGINT[],
    latest_observed_unix BIGINT[],
    rx_bytes_last BIGINT[],
    tx_bytes_last BIGINT[],
    rx_counter_epoch BIGINT[],
    tx_counter_epoch BIGINT[]
)
LANGUAGE sql
STABLE
AS $$
    WITH claims AS MATERIALIZED (
        SELECT claim.*
        FROM public.telemetry_dashboard_coordinate_projection_claims(
            p_owner_ids
        ) claim
        WHERE claim.domain = 'network'
    ), requested AS MATERIALIZED (
        SELECT claim.owner_id,
               claim.client_id,
               claim.expected_generation,
               claim.expected_revision + 1 AS published_revision,
               claim.generation_interfaces,
               cardinality(claim.generation_interfaces) AS interface_width,
               coordinate.source_bucket_secs,
               coordinate.bucket_start_unix,
               public.telemetry_dashboard_block_start(
                   coordinate.bucket_start_unix,
                   coordinate.source_bucket_secs
               ) AS block_start_unix
        FROM claims claim
        CROSS JOIN LATERAL unnest(
            claim.source_bucket_secs,
            claim.bucket_start_unix
        ) coordinate(source_bucket_secs, bucket_start_unix)
    ), requested_streams AS MATERIALIZED (
        SELECT DISTINCT requested.owner_id,
               requested.client_id,
               requested.source_bucket_secs,
               requested.bucket_start_unix,
               interface.name AS interface
        FROM requested
        CROSS JOIN LATERAL unnest(requested.generation_interfaces)
            interface(name)
    ), durable_source AS MATERIALIZED (
        SELECT stream.owner_id,
               minute.client_id,
               minute.interface,
               minute.bucket_start,
               minute.bucket_secs,
               minute.sample_count,
               minute.latest_observed_at,
               minute.rx_bytes_last,
               minute.tx_bytes_last,
               minute.rx_counter_epoch,
               minute.tx_counter_epoch
        FROM requested_streams stream
        JOIN public.telemetry_network_rates_minute minute
          ON stream.source_bucket_secs = 60
         AND minute.bucket_secs = 60
         AND minute.bucket_start = to_timestamp(stream.bucket_start_unix)
         AND minute.client_id = stream.client_id
         AND minute.interface = stream.interface

        UNION ALL

        SELECT stream.owner_id,
               sample.client_id,
               sample.interface,
               sample.observed_at,
               60::INTEGER,
               sample.sample_count,
               sample.latest_observed_at,
               sample.rx_bytes,
               sample.tx_bytes,
               sample.rx_counter_epoch,
               sample.tx_counter_epoch
        FROM requested_streams stream
        JOIN public.traffic_counter_samples sample
          ON stream.source_bucket_secs = 60
         AND sample.observed_at = to_timestamp(stream.bucket_start_unix)
         AND sample.client_id = stream.client_id
         AND sample.source_kind = 'host'
         AND sample.interface = stream.interface
         AND NOT sample.inbound_promoted

        UNION ALL

        SELECT stream.owner_id,
               coarse.client_id,
               coarse.interface,
               coarse.bucket_start,
               coarse.bucket_secs,
               coarse.sample_count,
               coarse.latest_observed_at,
               coarse.rx_bytes_last,
               coarse.tx_bytes_last,
               coarse.rx_counter_epoch,
               coarse.tx_counter_epoch
        FROM requested_streams stream
        JOIN public.telemetry_network_rates_coarse coarse
          ON stream.source_bucket_secs <> 60
         AND coarse.bucket_secs = stream.source_bucket_secs
         AND coarse.bucket_start = to_timestamp(stream.bucket_start_unix)
         AND coarse.client_id = stream.client_id
         AND coarse.interface = stream.interface
    ), coordinate_source AS MATERIALIZED (
        SELECT requested.owner_id,
               requested.source_bucket_secs,
               requested.bucket_start_unix,
               interface.name AS interface,
               interface.ordinality AS interface_ordinal,
               source.client_id AS source_client_id,
               source.sample_count,
               source.latest_observed_at,
               source.rx_bytes_last,
               source.tx_bytes_last,
               source.rx_counter_epoch,
               source.tx_counter_epoch
        FROM requested
        CROSS JOIN LATERAL unnest(requested.generation_interfaces)
            WITH ORDINALITY interface(name, ordinality)
        LEFT JOIN durable_source source
          ON source.owner_id = requested.owner_id
         AND source.client_id = requested.client_id
         AND source.interface = interface.name
         AND source.bucket_secs = requested.source_bucket_secs
         AND source.bucket_start = to_timestamp(
             requested.bucket_start_unix
         )
    ), affected AS MATERIALIZED (
        SELECT DISTINCT requested.owner_id,
               requested.client_id,
               requested.expected_generation,
               requested.interface_width,
               requested.source_bucket_secs,
               requested.block_start_unix,
               requested.generation_interfaces
        FROM requested
    ), assembled AS MATERIALIZED (
        SELECT affected.owner_id,
               affected.source_bucket_secs,
               affected.block_start_unix,
               affected.interface_width,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       COALESCE(source.sample_count, 0)::BIGINT
                   ELSE COALESCE(prior.sample_counts[
                       slot.ordinal * affected.interface_width
                           + interface.ordinality
                   ], 0) END
                   ORDER BY slot.ordinal, interface.ordinality
               ) AS sample_counts,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       extract(epoch FROM source.latest_observed_at)::BIGINT
                   ELSE prior.latest_observed_unix[
                       slot.ordinal * affected.interface_width
                           + interface.ordinality
                   ] END
                   ORDER BY slot.ordinal, interface.ordinality
               ) AS latest_observed_unix,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.rx_bytes_last
                   ELSE prior.rx_bytes_last[
                       slot.ordinal * affected.interface_width
                           + interface.ordinality
                   ] END
                   ORDER BY slot.ordinal, interface.ordinality
               ) AS rx_bytes_last,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.tx_bytes_last
                   ELSE prior.tx_bytes_last[
                       slot.ordinal * affected.interface_width
                           + interface.ordinality
                   ] END
                   ORDER BY slot.ordinal, interface.ordinality
               ) AS tx_bytes_last,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.rx_counter_epoch
                   ELSE prior.rx_counter_epoch[
                       slot.ordinal * affected.interface_width
                           + interface.ordinality
                   ] END
                   ORDER BY slot.ordinal, interface.ordinality
               ) AS rx_counter_epoch,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.tx_counter_epoch
                   ELSE prior.tx_counter_epoch[
                       slot.ordinal * affected.interface_width
                           + interface.ordinality
                   ] END
                   ORDER BY slot.ordinal, interface.ordinality
               ) AS tx_counter_epoch
        FROM affected
        CROSS JOIN generate_series(
            0, public.telemetry_dashboard_block_factor() - 1
        ) slot(ordinal)
        CROSS JOIN LATERAL unnest(affected.generation_interfaces)
            WITH ORDINALITY interface(name, ordinality)
        LEFT JOIN public.telemetry_dashboard_network_blocks prior
          ON prior.client_id = affected.client_id
         AND prior.generation = affected.expected_generation
         AND prior.source_bucket_secs = affected.source_bucket_secs
         AND prior.block_start_unix = affected.block_start_unix
        LEFT JOIN coordinate_source source
          ON source.owner_id = affected.owner_id
         AND source.source_bucket_secs = affected.source_bucket_secs
         AND source.bucket_start_unix = affected.block_start_unix
                + slot.ordinal::BIGINT * affected.source_bucket_secs
         AND source.interface = interface.name
        GROUP BY affected.owner_id,
                 affected.source_bucket_secs,
                 affected.block_start_unix,
                 affected.interface_width
    )
    SELECT assembled.owner_id,
           assembled.source_bucket_secs,
           assembled.block_start_unix,
           assembled.interface_width,
           EXISTS (
               SELECT 1
               FROM unnest(assembled.sample_counts) count(value)
               WHERE count.value > 0
           ),
           assembled.sample_counts,
           assembled.latest_observed_unix,
           assembled.rx_bytes_last,
           assembled.tx_bytes_last,
           assembled.rx_counter_epoch,
           assembled.tx_counter_epoch
    FROM assembled
    ORDER BY assembled.owner_id,
             assembled.source_bucket_secs,
             assembled.block_start_unix
$$;

CREATE FUNCTION public.prepare_telemetry_dashboard_traffic_coordinate_blocks(
    p_owner_ids BIGINT[]
)
RETURNS TABLE (
    owner_id BIGINT,
    source_bucket_secs INTEGER,
    block_start_unix BIGINT,
    has_samples BOOLEAN,
    rx_valid_counts BIGINT[],
    tx_valid_counts BIGINT[],
    rx_bytes BIGINT[],
    tx_bytes BIGINT[]
)
LANGUAGE sql
STABLE
AS $$
    WITH claims AS MATERIALIZED (
        SELECT claim.*
        FROM public.telemetry_dashboard_coordinate_projection_claims(
            p_owner_ids
        ) claim
        WHERE claim.domain = 'traffic'
    ), requested AS MATERIALIZED (
        SELECT claim.owner_id,
               claim.client_id,
               claim.expected_generation,
               claim.expected_revision + 1 AS published_revision,
               claim.generation_source_kinds,
               claim.generation_interfaces,
               coordinate.source_bucket_secs,
               coordinate.bucket_start_unix,
               public.telemetry_dashboard_block_start(
                   coordinate.bucket_start_unix,
                   coordinate.source_bucket_secs
               ) AS block_start_unix
        FROM claims claim
        CROSS JOIN LATERAL unnest(
            claim.source_bucket_secs,
            claim.bucket_start_unix
        ) coordinate(source_bucket_secs, bucket_start_unix)
    ), requested_streams AS MATERIALIZED (
        SELECT DISTINCT requested.owner_id,
               requested.client_id,
               requested.source_bucket_secs,
               requested.bucket_start_unix,
               identity.source_kind,
               identity.interface
        FROM requested
        CROSS JOIN LATERAL unnest(
            requested.generation_source_kinds,
            requested.generation_interfaces
        ) identity(source_kind, interface)
    ), raw AS MATERIALIZED (
        SELECT stream.owner_id,
               stream.bucket_start_unix,
               sample.client_id,
               sample.source_kind,
               sample.interface,
               sample.observed_at,
               sample.rx_bytes,
               sample.tx_bytes,
               sample.rx_counter_epoch,
               sample.tx_counter_epoch,
               sample.usage_authoritative,
               sample.rx_valid_count,
               sample.tx_valid_count,
               sample.rx_usage_bytes,
               sample.tx_usage_bytes
        FROM requested_streams stream
        JOIN public.traffic_counter_samples sample
          ON stream.source_bucket_secs = 60
         AND sample.client_id = stream.client_id
         AND sample.source_kind = stream.source_kind
         AND sample.interface = stream.interface
         AND sample.observed_at = to_timestamp(stream.bucket_start_unix)
         AND NOT sample.inbound_promoted
    ), raw_evaluated AS MATERIALIZED (
        SELECT source.owner_id,
               source.bucket_start_unix,
               source.rx_valid_count::BIGINT AS effective_rx_valid_count,
               source.tx_valid_count::BIGINT AS effective_tx_valid_count,
               CASE WHEN source.rx_valid_count > 0
                   THEN source.rx_usage_bytes ELSE 0::BIGINT
               END AS effective_rx_bytes,
               CASE WHEN source.tx_valid_count > 0
                   THEN source.tx_usage_bytes ELSE 0::BIGINT
               END AS effective_tx_bytes
        FROM raw source
        WHERE source.usage_authoritative

        UNION ALL

        SELECT source.owner_id,
               source.bucket_start_unix,
               CASE
                   WHEN source.rx_counter_epoch = predecessor.rx_counter_epoch
                    AND source.rx_bytes >= predecessor.rx_bytes
                   THEN 1::BIGINT ELSE 0::BIGINT
               END,
               CASE
                   WHEN source.tx_counter_epoch = predecessor.tx_counter_epoch
                    AND source.tx_bytes >= predecessor.tx_bytes
                   THEN 1::BIGINT ELSE 0::BIGINT
               END,
               CASE
                   WHEN source.rx_counter_epoch = predecessor.rx_counter_epoch
                    AND source.rx_bytes >= predecessor.rx_bytes
                   THEN source.rx_bytes - predecessor.rx_bytes
                   ELSE 0::BIGINT
               END,
               CASE
                   WHEN source.tx_counter_epoch = predecessor.tx_counter_epoch
                    AND source.tx_bytes >= predecessor.tx_bytes
                   THEN source.tx_bytes - predecessor.tx_bytes
                   ELSE 0::BIGINT
               END
        FROM raw source
        LEFT JOIN LATERAL (
            SELECT prior.rx_bytes,
                   prior.tx_bytes,
                   prior.rx_counter_epoch,
                   prior.tx_counter_epoch
            FROM public.traffic_counter_samples prior
            WHERE prior.client_id = source.client_id
              AND prior.source_kind = source.source_kind
              AND prior.interface = source.interface
              AND prior.observed_at < source.observed_at
            ORDER BY prior.observed_at DESC
            LIMIT 1
        ) predecessor ON TRUE
        WHERE NOT source.usage_authoritative
    ), raw_points AS MATERIALIZED (
        SELECT source.owner_id,
               source.bucket_start_unix,
               60::INTEGER AS bucket_secs,
               sum(source.effective_rx_valid_count)::BIGINT
                   AS rx_valid_count,
               sum(source.effective_tx_valid_count)::BIGINT
                   AS tx_valid_count,
               CASE WHEN sum(source.effective_rx_valid_count) > 0
                   THEN sum(source.effective_rx_bytes)::BIGINT
               END AS rx_bytes,
               CASE WHEN sum(source.effective_tx_valid_count) > 0
                   THEN sum(source.effective_tx_bytes)::BIGINT
               END AS tx_bytes
        FROM raw_evaluated source
        GROUP BY source.owner_id, source.bucket_start_unix
    ), coarse_points AS MATERIALIZED (
        SELECT stream.owner_id,
               stream.bucket_start_unix,
               stream.source_bucket_secs AS bucket_secs,
               sum(rollup.rx_valid_count)::BIGINT AS rx_valid_count,
               sum(rollup.tx_valid_count)::BIGINT AS tx_valid_count,
               CASE WHEN sum(rollup.rx_valid_count) > 0
                   THEN sum(CASE WHEN rollup.rx_valid_count > 0
                       THEN rollup.rx_bytes ELSE 0 END)::BIGINT
               END AS rx_bytes,
               CASE WHEN sum(rollup.tx_valid_count) > 0
                   THEN sum(CASE WHEN rollup.tx_valid_count > 0
                       THEN rollup.tx_bytes ELSE 0 END)::BIGINT
               END AS tx_bytes
        FROM requested_streams stream
        JOIN public.traffic_counter_rollups rollup
          ON stream.source_bucket_secs <> 60
         AND rollup.client_id = stream.client_id
         AND rollup.source_kind = stream.source_kind
         AND rollup.interface = stream.interface
         AND rollup.bucket_secs = stream.source_bucket_secs
         AND rollup.bucket_start = to_timestamp(stream.bucket_start_unix)
        WHERE NOT EXISTS (
              SELECT 1
              FROM public.traffic_counter_rollups finer
              WHERE finer.client_id = rollup.client_id
                AND finer.source_kind = rollup.source_kind
                AND finer.interface = rollup.interface
                AND finer.origin_kind = rollup.origin_kind
                AND finer.bucket_secs < rollup.bucket_secs
                AND finer.bucket_start >= rollup.bucket_start
                AND finer.bucket_start < rollup.bucket_start
                    + make_interval(secs => rollup.bucket_secs)
                AND finer.bucket_start
                    + make_interval(secs => finer.bucket_secs)
                    > rollup.bucket_start
              OFFSET 0
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.traffic_counter_samples exact
              WHERE exact.client_id = rollup.client_id
                AND exact.source_kind = rollup.source_kind
                AND exact.interface = rollup.interface
                AND NOT exact.inbound_promoted
                AND public.telemetry_dashboard_traffic_origin_kind(
                    exact.sample_source
                ) = rollup.origin_kind
                AND exact.observed_at >= rollup.bucket_start
                AND exact.observed_at < rollup.bucket_start
                    + make_interval(secs => rollup.bucket_secs)
              OFFSET 0
          )
        GROUP BY stream.owner_id,
                 stream.bucket_start_unix,
                 stream.source_bucket_secs
    ), source_points AS MATERIALIZED (
        SELECT * FROM raw_points
        UNION ALL
        SELECT * FROM coarse_points
    ), coordinate_source AS MATERIALIZED (
        SELECT requested.*,
               source.owner_id AS source_owner_id,
               source.rx_valid_count,
               source.tx_valid_count,
               source.rx_bytes,
               source.tx_bytes
        FROM requested
        LEFT JOIN source_points source
          ON source.owner_id = requested.owner_id
         AND source.bucket_secs = requested.source_bucket_secs
         AND source.bucket_start_unix = requested.bucket_start_unix
    ), affected AS MATERIALIZED (
        SELECT DISTINCT requested.owner_id,
               requested.client_id,
               requested.expected_generation,
               requested.source_bucket_secs,
               requested.block_start_unix
        FROM requested
    ), assembled AS MATERIALIZED (
        SELECT affected.owner_id,
               affected.source_bucket_secs,
               affected.block_start_unix,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.rx_valid_count
                   ELSE prior.rx_valid_counts[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS rx_valid_counts,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.tx_valid_count
                   ELSE prior.tx_valid_counts[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS tx_valid_counts,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.rx_bytes
                   ELSE prior.rx_bytes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS rx_bytes,
               array_agg(
                   CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                       source.tx_bytes
                   ELSE prior.tx_bytes[slot.ordinal + 1]
                   END ORDER BY slot.ordinal
               ) AS tx_bytes
        FROM affected
        CROSS JOIN generate_series(
            0, public.telemetry_dashboard_block_factor() - 1
        ) slot(ordinal)
        LEFT JOIN public.telemetry_dashboard_traffic_blocks prior
          ON prior.client_id = affected.client_id
         AND prior.generation = affected.expected_generation
         AND prior.source_bucket_secs = affected.source_bucket_secs
         AND prior.block_start_unix = affected.block_start_unix
        LEFT JOIN coordinate_source source
          ON source.owner_id = affected.owner_id
         AND source.source_bucket_secs = affected.source_bucket_secs
         AND source.bucket_start_unix = affected.block_start_unix
                + slot.ordinal::BIGINT * affected.source_bucket_secs
        GROUP BY affected.owner_id,
                 affected.source_bucket_secs,
                 affected.block_start_unix
    )
    SELECT assembled.owner_id,
           assembled.source_bucket_secs,
           assembled.block_start_unix,
           EXISTS (
               SELECT 1
               FROM unnest(assembled.rx_valid_counts) count(value)
               WHERE count.value IS NOT NULL
           ),
           assembled.rx_valid_counts,
           assembled.tx_valid_counts,
           assembled.rx_bytes,
           assembled.tx_bytes
    FROM assembled
    ORDER BY assembled.owner_id,
             assembled.source_bucket_secs,
             assembled.block_start_unix
$$;

CREATE FUNCTION public.reconcile_telemetry_dashboard_coordinate_bounds(
    p_client_id TEXT,
    p_domain TEXT,
    p_generation BIGINT,
    p_source_bucket_secs INTEGER[],
    p_width INTEGER
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF p_domain = 'resource' THEN
        WITH requested_tiers AS MATERIALIZED (
            SELECT DISTINCT tier.source_bucket_secs
            FROM unnest(p_source_bucket_secs) tier(source_bucket_secs)
        ), current_edges AS MATERIALIZED (
            SELECT tier.source_bucket_secs,
                   edge.first_unix,
                   edge.last_unix
            FROM requested_tiers tier
            LEFT JOIN LATERAL
                public.telemetry_dashboard_resource_block_edges(
                    p_client_id, p_generation, tier.source_bucket_secs
                ) edge ON TRUE
        )
        MERGE INTO public.telemetry_dashboard_resource_generation_bounds bounds
        USING current_edges source
          ON bounds.client_id = p_client_id
         AND bounds.generation = p_generation
         AND bounds.source_bucket_secs = source.source_bucket_secs
        WHEN MATCHED AND source.first_unix IS NULL THEN
            DELETE
        WHEN MATCHED THEN
            UPDATE SET
                first_bucket_start_unix = source.first_unix,
                last_bucket_start_unix = source.last_unix,
                active_block_start_unix =
                    public.telemetry_dashboard_block_start(
                        source.last_unix, source.source_bucket_secs
                    )
        WHEN NOT MATCHED AND source.first_unix IS NOT NULL THEN
            INSERT (
                client_id, generation, source_bucket_secs,
                first_bucket_start_unix, last_bucket_start_unix,
                active_block_start_unix
            ) VALUES (
                p_client_id, p_generation, source.source_bucket_secs,
                source.first_unix, source.last_unix,
                public.telemetry_dashboard_block_start(
                    source.last_unix, source.source_bucket_secs
                )
            );
    ELSIF p_domain = 'network' THEN
        WITH requested_tiers AS MATERIALIZED (
            SELECT DISTINCT tier.source_bucket_secs
            FROM unnest(p_source_bucket_secs) tier(source_bucket_secs)
        ), current_edges AS MATERIALIZED (
            SELECT tier.source_bucket_secs,
                   edge.first_unix,
                   edge.last_unix
            FROM requested_tiers tier
            LEFT JOIN LATERAL
                public.telemetry_dashboard_network_block_edges(
                    p_client_id, p_generation, tier.source_bucket_secs
                ) edge ON TRUE
        )
        MERGE INTO public.telemetry_dashboard_network_generation_bounds bounds
        USING current_edges source
          ON bounds.client_id = p_client_id
         AND bounds.generation = p_generation
         AND bounds.source_bucket_secs = source.source_bucket_secs
        WHEN MATCHED AND source.first_unix IS NULL THEN
            DELETE
        WHEN MATCHED THEN
            UPDATE SET
                interface_width = p_width,
                first_bucket_start_unix = source.first_unix,
                last_bucket_start_unix = source.last_unix,
                active_block_start_unix =
                    public.telemetry_dashboard_block_start(
                        source.last_unix, source.source_bucket_secs
                    )
        WHEN NOT MATCHED AND source.first_unix IS NOT NULL THEN
            INSERT (
                client_id, generation, interface_width,
                source_bucket_secs, first_bucket_start_unix,
                last_bucket_start_unix, active_block_start_unix
            ) VALUES (
                p_client_id, p_generation, p_width,
                source.source_bucket_secs, source.first_unix,
                source.last_unix,
                public.telemetry_dashboard_block_start(
                    source.last_unix, source.source_bucket_secs
                )
            );
    ELSIF p_domain = 'traffic' THEN
        WITH requested_tiers AS MATERIALIZED (
            SELECT DISTINCT tier.source_bucket_secs
            FROM unnest(p_source_bucket_secs) tier(source_bucket_secs)
        ), current_edges AS MATERIALIZED (
            SELECT tier.source_bucket_secs,
                   edge.first_unix,
                   edge.last_unix
            FROM requested_tiers tier
            LEFT JOIN LATERAL
                public.telemetry_dashboard_traffic_block_edges(
                    p_client_id, p_generation, tier.source_bucket_secs
                ) edge ON TRUE
        )
        MERGE INTO public.telemetry_dashboard_traffic_generation_bounds bounds
        USING current_edges source
          ON bounds.client_id = p_client_id
         AND bounds.generation = p_generation
         AND bounds.source_bucket_secs = source.source_bucket_secs
        WHEN MATCHED AND source.first_unix IS NULL THEN
            DELETE
        WHEN MATCHED THEN
            UPDATE SET
                stream_width = p_width,
                first_bucket_start_unix = source.first_unix,
                last_bucket_start_unix = source.last_unix,
                active_block_start_unix =
                    public.telemetry_dashboard_block_start(
                        source.last_unix, source.source_bucket_secs
                    )
        WHEN NOT MATCHED AND source.first_unix IS NOT NULL THEN
            INSERT (
                client_id, generation, stream_width,
                source_bucket_secs, first_bucket_start_unix,
                last_bucket_start_unix, active_block_start_unix
            ) VALUES (
                p_client_id, p_generation, p_width,
                source.source_bucket_secs, source.first_unix,
                source.last_unix,
                public.telemetry_dashboard_block_start(
                    source.last_unix, source.source_bucket_secs
                )
            );
    ELSE
        RAISE EXCEPTION 'invalid dashboard coordinate bounds domain';
    END IF;
END
$$;

CREATE FUNCTION public.complete_telemetry_dashboard_coordinate_projection(
    p_client_id TEXT,
    p_domain TEXT,
    p_event_kind TEXT[],
    p_source_bucket_secs INTEGER[],
    p_block_start_unix BIGINT[],
    p_bucket_start_unix BIGINT[],
    p_captured_block_event_ids BIGINT[],
    p_expected_generation BIGINT,
    p_expected_revision BIGINT
)
RETURNS BOOLEAN
LANGUAGE plpgsql
AS $$
DECLARE
    actual_kinds TEXT[];
    actual_tiers INTEGER[];
    actual_starts BIGINT[];
    actual_buckets BIGINT[];
    changed_tiers INTEGER[];
    changed_starts BIGINT[];
    matched_count BIGINT;
    new_revision BIGINT := p_expected_revision + 1;
    generation_width INTEGER;
    flipped BOOLEAN;
    block_coordinate RECORD;
    notice JSONB;
BEGIN
    IF p_domain NOT IN ('resource', 'network', 'traffic')
       OR cardinality(COALESCE(
           p_event_kind, ARRAY[]::TEXT[]
       )) = 0
       OR cardinality(COALESCE(
           p_event_kind, ARRAY[]::TEXT[]
       )) <> cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       ))
       OR cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) <> cardinality(COALESCE(
           p_block_start_unix, ARRAY[]::BIGINT[]
       ))
       OR cardinality(COALESCE(
           p_block_start_unix, ARRAY[]::BIGINT[]
       )) <> cardinality(COALESCE(
           p_bucket_start_unix, ARRAY[]::BIGINT[]
       ))
       OR cardinality(COALESCE(
           p_captured_block_event_ids, ARRAY[]::BIGINT[]
       )) = 0
       OR EXISTS (
           SELECT 1
           FROM unnest(
               p_event_kind,
               p_source_bucket_secs,
               p_block_start_unix,
               p_bucket_start_unix
           ) work(event_kind, tier, block_start, bucket_start)
           WHERE work.event_kind <> 'coordinate'
              OR work.bucket_start IS NULL
              OR work.bucket_start % work.tier <> 0
              OR work.block_start <>
                    public.telemetry_dashboard_block_start(
                        work.bucket_start, work.tier
                    )
              OR CASE p_domain
                    WHEN 'traffic' THEN NOT
                        public.telemetry_dashboard_traffic_source_tier_is_valid(
                            work.tier
                        )
                    ELSE NOT
                        public.telemetry_dashboard_source_tier_is_valid(
                            work.tier
                        )
                 END
       ) THEN
        RAISE EXCEPTION 'invalid prepared dashboard coordinate publication';
    END IF;

    SELECT count(*)
    INTO matched_count
    FROM public.telemetry_dashboard_block_events event
    WHERE event.event_id = ANY(p_captured_block_event_ids)
      AND event.client_id = p_client_id
      AND event.domain = p_domain;

    IF matched_count <> cardinality(p_captured_block_event_ids) THEN
        RAISE EXCEPTION 'prepared dashboard block capture changed';
    END IF;

    WITH captured AS MATERIALIZED (
        SELECT event.event_kind,
               event.source_bucket_secs,
               event.block_start_unix,
               event.bucket_start_unix
        FROM public.telemetry_dashboard_block_events event
        WHERE event.event_id = ANY(p_captured_block_event_ids)
    ), normalized AS MATERIALIZED (
        SELECT 'coordinate'::TEXT AS event_kind,
               event.source_bucket_secs,
               event.block_start_unix,
               event.bucket_start_unix
        FROM captured event
        GROUP BY event.source_bucket_secs,
                 event.block_start_unix,
                 event.bucket_start_unix
    )
    SELECT array_agg(
               normalized.event_kind
               ORDER BY normalized.source_bucket_secs,
                        normalized.block_start_unix,
                        normalized.event_kind,
                        normalized.bucket_start_unix
           ),
           array_agg(
               normalized.source_bucket_secs
               ORDER BY normalized.source_bucket_secs,
                        normalized.block_start_unix,
                        normalized.event_kind,
                        normalized.bucket_start_unix
           ),
           array_agg(
               normalized.block_start_unix
               ORDER BY normalized.source_bucket_secs,
                        normalized.block_start_unix,
                        normalized.event_kind,
                        normalized.bucket_start_unix
           ),
           array_agg(
               normalized.bucket_start_unix
               ORDER BY normalized.source_bucket_secs,
                        normalized.block_start_unix,
                        normalized.event_kind,
                        normalized.bucket_start_unix
           )
    INTO actual_kinds, actual_tiers, actual_starts, actual_buckets
    FROM normalized;

    IF actual_kinds IS DISTINCT FROM p_event_kind
       OR actual_tiers IS DISTINCT FROM p_source_bucket_secs
       OR actual_starts IS DISTINCT FROM p_block_start_unix
       OR actual_buckets IS DISTINCT FROM p_bucket_start_unix THEN
        RAISE EXCEPTION 'prepared dashboard coordinate work changed';
    END IF;

    SELECT array_agg(
               coordinate.tier
               ORDER BY coordinate.tier, coordinate.block_start
           ),
           array_agg(
               coordinate.block_start
               ORDER BY coordinate.tier, coordinate.block_start
           )
    INTO changed_tiers, changed_starts
    FROM (
        SELECT DISTINCT work.tier, work.block_start
        FROM unnest(
            p_source_bucket_secs, p_block_start_unix
        ) work(tier, block_start)
    ) coordinate;

    IF p_domain = 'resource' THEN
        generation_width := 0;
    ELSIF p_domain = 'network' THEN
        SELECT cardinality(head.network_generation_interfaces)
        INTO generation_width
        FROM public.telemetry_dashboard_network_projection_heads head
        WHERE head.client_id = p_client_id
          AND head.network_generation = p_expected_generation
          AND head.network_revision = p_expected_revision;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'network dashboard head fence changed';
        END IF;
    ELSE
        SELECT cardinality(head.traffic_generation_source_kinds)
        INTO generation_width
        FROM public.telemetry_dashboard_traffic_projection_heads head
        WHERE head.client_id = p_client_id
          AND head.traffic_generation = p_expected_generation
          AND head.traffic_revision = p_expected_revision;
        IF NOT FOUND THEN
            RAISE EXCEPTION 'traffic dashboard head fence changed';
        END IF;
    END IF;

    PERFORM public.reconcile_telemetry_dashboard_coordinate_bounds(
        p_client_id,
        p_domain,
        p_expected_generation,
        p_source_bucket_secs,
        generation_width
    );

    IF p_domain = 'resource' THEN
        UPDATE public.telemetry_dashboard_resource_projection_heads head
        SET resource_revision = new_revision,
            resource_change = 'block',
            resource_change_source_bucket_secs = changed_tiers,
            resource_change_block_start_unix = changed_starts,
            resource_first_at = (
                SELECT to_timestamp(min(bounds.first_bucket_start_unix))
                FROM public.telemetry_dashboard_resource_generation_bounds
                    bounds
                WHERE bounds.client_id = p_client_id
                  AND bounds.generation = p_expected_generation
            ),
            resource_through_at = (
                SELECT to_timestamp(max(
                    bounds.last_bucket_start_unix
                    + bounds.source_bucket_secs
                ))
                FROM public.telemetry_dashboard_resource_generation_bounds
                    bounds
                WHERE bounds.client_id = p_client_id
                  AND bounds.generation = p_expected_generation
            )
        WHERE head.client_id = p_client_id
          AND head.resource_generation = p_expected_generation
          AND head.resource_revision = p_expected_revision
        RETURNING TRUE INTO flipped;
    ELSIF p_domain = 'network' THEN
        UPDATE public.telemetry_dashboard_network_projection_heads head
        SET network_revision = new_revision,
            network_change = 'block',
            network_change_source_bucket_secs = changed_tiers,
            network_change_block_start_unix = changed_starts,
            network_first_at = (
                SELECT to_timestamp(min(bounds.first_bucket_start_unix))
                FROM public.telemetry_dashboard_network_generation_bounds
                    bounds
                WHERE bounds.client_id = p_client_id
                  AND bounds.generation = p_expected_generation
            ),
            network_through_at = (
                SELECT to_timestamp(max(
                    bounds.last_bucket_start_unix
                    + bounds.source_bucket_secs
                ))
                FROM public.telemetry_dashboard_network_generation_bounds
                    bounds
                WHERE bounds.client_id = p_client_id
                  AND bounds.generation = p_expected_generation
            )
        WHERE head.client_id = p_client_id
          AND head.network_generation = p_expected_generation
          AND head.network_revision = p_expected_revision
        RETURNING TRUE INTO flipped;
    ELSE
        UPDATE public.telemetry_dashboard_traffic_projection_heads head
        SET traffic_revision = new_revision,
            traffic_change = 'block',
            traffic_change_source_bucket_secs = changed_tiers,
            traffic_change_block_start_unix = changed_starts,
            traffic_first_at = (
                SELECT to_timestamp(min(bounds.first_bucket_start_unix))
                FROM public.telemetry_dashboard_traffic_generation_bounds
                    bounds
                WHERE bounds.client_id = p_client_id
                  AND bounds.generation = p_expected_generation
            ),
            traffic_through_at = (
                SELECT to_timestamp(max(
                    bounds.last_bucket_start_unix
                    + bounds.source_bucket_secs
                ))
                FROM public.telemetry_dashboard_traffic_generation_bounds
                    bounds
                WHERE bounds.client_id = p_client_id
                  AND bounds.generation = p_expected_generation
            )
        WHERE head.client_id = p_client_id
          AND head.traffic_generation = p_expected_generation
          AND head.traffic_revision = p_expected_revision
        RETURNING TRUE INTO flipped;
    END IF;

    IF NOT COALESCE(flipped, FALSE) THEN
        RAISE EXCEPTION '% dashboard head CAS failed', p_domain;
    END IF;

    DELETE FROM public.telemetry_dashboard_block_events event
    WHERE event.event_id = ANY(p_captured_block_event_ids);

    FOR block_coordinate IN
        SELECT coordinate.tier,
               coordinate.block_start,
               coordinate.ordinality
        FROM unnest(changed_tiers, changed_starts)
            WITH ORDINALITY coordinate(tier, block_start, ordinality)
        ORDER BY coordinate.ordinality
    LOOP
        notice := jsonb_build_object(
            'owner', 'dashboard',
            'client_id', p_client_id,
            'domain', p_domain,
            'change', 'block',
            'generation', p_expected_generation,
            'previous_revision', p_expected_revision,
            'revision', new_revision,
            'source_bucket_secs',
                ARRAY[block_coordinate.tier]::INTEGER[],
            'block_start_unix',
                ARRAY[block_coordinate.block_start]::BIGINT[],
            'complete', block_coordinate.ordinality = cardinality(
                changed_tiers
            )
        );
        PERFORM pg_notify('vpsman_telemetry_projection', notice::TEXT);
    END LOOP;

    RETURN TRUE;
END
$$;
