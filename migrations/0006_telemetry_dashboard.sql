-- Clean dashboard projection schema over authoritative retained telemetry.
--
-- Resource, network, and traffic history are published as fixed-calendar
-- blocks of exactly sixteen native-tier buckets. Closed blocks are immutable
-- values;
-- only the newest block of each tier is represented by normalized active
-- rows. Source transactions append immutable exact-coordinate events. A
-- publisher claims every visible event for one client/domain owner as either
-- one exact coordinate union or one full-generation snapshot, then advances
-- the head only after every affected block is ready.

CREATE FUNCTION public.telemetry_dashboard_source_tier_is_valid(
    p_bucket_secs INTEGER
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT p_bucket_secs = ANY (
        ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400]
    )
$$;

CREATE FUNCTION public.telemetry_dashboard_traffic_source_tier_is_valid(
    p_bucket_secs INTEGER
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT p_bucket_secs = ANY (
        ARRAY[60, 3600, 10800, 21600, 86400]
    )
$$;

-- Exact-row and retained-rollup visibility must use one origin classifier.
-- Imports are namespaced by their source prefix; every other exact sample is
-- part of the live representation.
CREATE FUNCTION public.telemetry_dashboard_traffic_origin_kind(
    p_sample_source TEXT
)
RETURNS TEXT
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT CASE WHEN p_sample_source LIKE 'vnstat_import:%'
        THEN 'vnstat_import' ELSE 'live' END
$$;

CREATE FUNCTION public.telemetry_dashboard_block_factor()
RETURNS INTEGER
LANGUAGE sql
IMMUTABLE
PARALLEL SAFE
AS $$
    SELECT 16
$$;

CREATE FUNCTION public.telemetry_dashboard_block_start(
    p_bucket_start_unix BIGINT,
    p_source_bucket_secs INTEGER
)
RETURNS BIGINT
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT floor(
        p_bucket_start_unix::NUMERIC
        / (p_source_bucket_secs::BIGINT
           * public.telemetry_dashboard_block_factor())
    )::BIGINT
    * p_source_bucket_secs::BIGINT
    * public.telemetry_dashboard_block_factor()
$$;

CREATE FUNCTION public.telemetry_dashboard_change_is_valid(
    p_change TEXT,
    p_source_bucket_secs INTEGER[],
    p_block_start_unix BIGINT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT p_change IN ('block', 'generation')
       AND COALESCE(array_ndims(p_source_bucket_secs), 1) = 1
       AND COALESCE(array_lower(p_source_bucket_secs, 1), 1) = 1
       AND COALESCE(array_ndims(p_block_start_unix), 1) = 1
       AND COALESCE(array_lower(p_block_start_unix, 1), 1) = 1
       AND cardinality(p_source_bucket_secs) =
            cardinality(p_block_start_unix)
       AND (
            (p_change = 'generation'
             AND cardinality(p_source_bucket_secs) = 0)
            OR
            (p_change = 'block'
             AND cardinality(p_source_bucket_secs) > 0)
       )
       AND array_position(p_source_bucket_secs, NULL::INTEGER) IS NULL
       AND array_position(p_block_start_unix, NULL::BIGINT) IS NULL
       AND NOT EXISTS (
            SELECT 1
            FROM generate_subscripts(p_source_bucket_secs, 1) ordinal
            WHERE NOT public.telemetry_dashboard_source_tier_is_valid(
                p_source_bucket_secs[ordinal]
            )
               OR p_block_start_unix[ordinal] <>
                    public.telemetry_dashboard_block_start(
                        p_block_start_unix[ordinal],
                        p_source_bucket_secs[ordinal]
                    )
               OR (
                    ordinal > 1
                    AND (
                        p_source_bucket_secs[ordinal],
                        p_block_start_unix[ordinal]
                    ) <= (
                        p_source_bucket_secs[ordinal - 1],
                        p_block_start_unix[ordinal - 1]
                    )
               )
       )
$$;

CREATE FUNCTION public.telemetry_dashboard_traffic_change_is_valid(
    p_change TEXT,
    p_source_bucket_secs INTEGER[],
    p_block_start_unix BIGINT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT public.telemetry_dashboard_change_is_valid(
               p_change, p_source_bucket_secs, p_block_start_unix
           )
       AND NOT EXISTS (
            SELECT 1
            FROM unnest(p_source_bucket_secs) tier(bucket_secs)
            WHERE NOT public.telemetry_dashboard_traffic_source_tier_is_valid(
                tier.bucket_secs
            )
       )
$$;

CREATE FUNCTION public.telemetry_dashboard_interfaces_are_canonical(
    p_interfaces TEXT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT COALESCE(array_ndims(p_interfaces), 1) = 1
       AND COALESCE(array_lower(p_interfaces, 1), 1) = 1
       AND array_position(p_interfaces, NULL::TEXT) IS NULL
       AND NOT EXISTS (
            SELECT 1
            FROM unnest(p_interfaces) WITH ORDINALITY item(interface, ordinal)
            WHERE octet_length(item.interface) NOT BETWEEN 1 AND 128
               OR (
                    item.ordinal > 1
                    AND item.interface COLLATE "C"
                        <= p_interfaces[item.ordinal - 1] COLLATE "C"
               )
       )
$$;

CREATE FUNCTION public.telemetry_dashboard_traffic_identities_are_canonical(
    p_source_kinds TEXT[],
    p_interfaces TEXT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT COALESCE(array_ndims(p_source_kinds), 1) = 1
       AND COALESCE(array_lower(p_source_kinds, 1), 1) = 1
       AND COALESCE(array_ndims(p_interfaces), 1) = 1
       AND COALESCE(array_lower(p_interfaces, 1), 1) = 1
       AND cardinality(p_source_kinds) = cardinality(p_interfaces)
       AND array_position(p_source_kinds, NULL::TEXT) IS NULL
       AND array_position(p_interfaces, NULL::TEXT) IS NULL
       AND NOT EXISTS (
            SELECT 1
            FROM generate_subscripts(p_source_kinds, 1) ordinal
            WHERE p_source_kinds[ordinal] NOT IN ('host', 'tunnel')
               OR octet_length(p_interfaces[ordinal]) NOT BETWEEN 1 AND 128
               OR (
                    ordinal > 1
                    AND ROW(
                        p_source_kinds[ordinal] COLLATE "C",
                        p_interfaces[ordinal] COLLATE "C"
                    ) <= ROW(
                        p_source_kinds[ordinal - 1] COLLATE "C",
                        p_interfaces[ordinal - 1] COLLATE "C"
                    )
               )
       )
$$;

CREATE FUNCTION public.telemetry_dashboard_traffic_identity_is_selected(
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_source_kind TEXT,
    p_interface TEXT
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT EXISTS (
        SELECT 1
        FROM unnest(p_source_kinds, p_interfaces)
            identity(source_kind, interface)
        WHERE identity.source_kind = p_source_kind
          AND identity.interface = p_interface
    )
$$;

CREATE FUNCTION public.telemetry_dashboard_network_vectors_are_valid(
    p_sample_counts BIGINT[],
    p_latest_observed_unix BIGINT[],
    p_rx_bytes_last BIGINT[],
    p_tx_bytes_last BIGINT[],
    p_rx_counter_epoch BIGINT[],
    p_tx_counter_epoch BIGINT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT NOT EXISTS (
        SELECT 1
        FROM generate_subscripts(p_sample_counts, 1) ordinal
        WHERE p_sample_counts[ordinal] IS NULL
           OR p_sample_counts[ordinal] < 0
           OR (
                p_sample_counts[ordinal] = 0
                AND num_nonnulls(
                    p_latest_observed_unix[ordinal],
                    p_rx_bytes_last[ordinal],
                    p_tx_bytes_last[ordinal],
                    p_rx_counter_epoch[ordinal],
                    p_tx_counter_epoch[ordinal]
                ) <> 0
           )
           OR (
                p_sample_counts[ordinal] > 0
                AND num_nonnulls(
                    p_latest_observed_unix[ordinal],
                    p_rx_bytes_last[ordinal],
                    p_tx_bytes_last[ordinal],
                    p_rx_counter_epoch[ordinal],
                    p_tx_counter_epoch[ordinal]
                ) <> 5
           )
           OR COALESCE(p_latest_observed_unix[ordinal], 0) < 0
           OR COALESCE(p_rx_bytes_last[ordinal], 0) < 0
           OR COALESCE(p_tx_bytes_last[ordinal], 0) < 0
           OR COALESCE(p_rx_counter_epoch[ordinal], 0) < 0
           OR COALESCE(p_tx_counter_epoch[ordinal], 0) < 0
    )
$$;

CREATE FUNCTION public.telemetry_dashboard_resource_vectors_are_valid(
    p_sample_counts BIGINT[],
    p_cpu_load_1_sums DOUBLE PRECISION[],
    p_cpu_load_1_maxes REAL[],
    p_memory_total_bytes_maxes BIGINT[],
    p_memory_used_ratio_sums DOUBLE PRECISION[],
    p_memory_used_ratio_maxes REAL[],
    p_disk_sample_counts BIGINT[],
    p_disk_total_bytes_maxes BIGINT[],
    p_disk_used_ratio_sums DOUBLE PRECISION[],
    p_disk_used_ratio_maxes REAL[],
    p_latest_observed_unix BIGINT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT NOT EXISTS (
        SELECT 1
        FROM generate_subscripts(p_sample_counts, 1) ordinal
        WHERE p_sample_counts[ordinal] IS NULL
           OR p_sample_counts[ordinal] < 0
           OR p_disk_sample_counts[ordinal] IS NULL
           OR p_disk_sample_counts[ordinal] < 0
           OR p_disk_sample_counts[ordinal] >
                p_sample_counts[ordinal]
           OR (
                p_sample_counts[ordinal] = 0
                AND (
                    p_disk_sample_counts[ordinal] <> 0
                    OR num_nonnulls(
                        p_cpu_load_1_sums[ordinal],
                        p_cpu_load_1_maxes[ordinal],
                        p_memory_total_bytes_maxes[ordinal],
                        p_memory_used_ratio_sums[ordinal],
                        p_memory_used_ratio_maxes[ordinal],
                        p_disk_total_bytes_maxes[ordinal],
                        p_disk_used_ratio_sums[ordinal],
                        p_disk_used_ratio_maxes[ordinal],
                        p_latest_observed_unix[ordinal]
                    ) <> 0
                )
           )
           OR (
                p_sample_counts[ordinal] > 0
                AND (
                    num_nonnulls(
                        p_cpu_load_1_sums[ordinal],
                        p_cpu_load_1_maxes[ordinal],
                        p_memory_total_bytes_maxes[ordinal],
                        p_memory_used_ratio_sums[ordinal],
                        p_memory_used_ratio_maxes[ordinal],
                        p_disk_total_bytes_maxes[ordinal],
                        p_disk_used_ratio_sums[ordinal],
                        p_disk_used_ratio_maxes[ordinal],
                        p_latest_observed_unix[ordinal]
                    ) <> 9
                    OR p_cpu_load_1_sums[ordinal] < 0
                    OR p_cpu_load_1_maxes[ordinal] < 0
                    OR p_memory_total_bytes_maxes[ordinal] < 0
                    OR p_memory_used_ratio_sums[ordinal] < 0
                    OR p_memory_used_ratio_sums[ordinal] >
                        p_sample_counts[ordinal]
                    OR p_memory_used_ratio_maxes[ordinal] NOT BETWEEN 0 AND 1
                    OR p_disk_total_bytes_maxes[ordinal] < 0
                    OR p_disk_used_ratio_sums[ordinal] < 0
                    OR p_disk_used_ratio_sums[ordinal] >
                        p_disk_sample_counts[ordinal]
                    OR p_disk_used_ratio_maxes[ordinal] NOT BETWEEN 0 AND 1
                    OR p_latest_observed_unix[ordinal] < 0
                )
           )
    )
$$;

CREATE FUNCTION public.telemetry_dashboard_traffic_vectors_are_valid(
    p_rx_valid_counts BIGINT[],
    p_tx_valid_counts BIGINT[],
    p_rx_bytes BIGINT[],
    p_tx_bytes BIGINT[]
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT NOT EXISTS (
        SELECT 1
        FROM generate_subscripts(p_rx_valid_counts, 1) ordinal
        WHERE (p_rx_valid_counts[ordinal] IS NULL) <>
                (p_tx_valid_counts[ordinal] IS NULL)
           OR COALESCE(p_rx_valid_counts[ordinal], 0) < 0
           OR COALESCE(p_tx_valid_counts[ordinal], 0) < 0
           OR (
                COALESCE(p_rx_valid_counts[ordinal], 0) = 0
                AND p_rx_bytes[ordinal] IS NOT NULL
           )
           OR (
                COALESCE(p_tx_valid_counts[ordinal], 0) = 0
                AND p_tx_bytes[ordinal] IS NOT NULL
           )
           OR (
                p_rx_valid_counts[ordinal] > 0
                AND COALESCE(p_rx_bytes[ordinal], -1) < 0
           )
           OR (
                p_tx_valid_counts[ordinal] > 0
                AND COALESCE(p_tx_bytes[ordinal], -1) < 0
           )
    )
$$;

CREATE TYPE public.telemetry_dashboard_network_selection AS (
    select_all BOOLEAN,
    interfaces TEXT[],
    patterns TEXT[]
);

CREATE TYPE public.telemetry_dashboard_traffic_selection AS (
    source_kinds TEXT[],
    interfaces TEXT[]
);

CREATE SEQUENCE public.telemetry_dashboard_event_seq AS BIGINT;
CREATE SEQUENCE public.telemetry_dashboard_generation_seq AS BIGINT START WITH 2;

CREATE TABLE public.telemetry_dashboard_clients (
    client_id TEXT PRIMARY KEY
        REFERENCES public.clients(id) ON DELETE CASCADE
);

CREATE TABLE public.telemetry_dashboard_resource_projection_heads (
    client_id TEXT PRIMARY KEY
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    resource_generation BIGINT NOT NULL DEFAULT 1
        CHECK (resource_generation > 0),
    resource_revision BIGINT NOT NULL DEFAULT 0
        CHECK (resource_revision >= 0),
    resource_change TEXT NOT NULL DEFAULT 'generation',
    resource_change_source_bucket_secs INTEGER[] NOT NULL
        DEFAULT ARRAY[]::INTEGER[],
    resource_change_block_start_unix BIGINT[] NOT NULL
        DEFAULT ARRAY[]::BIGINT[],
    resource_first_at TIMESTAMPTZ,
    resource_through_at TIMESTAMPTZ,
    CHECK ((resource_first_at IS NULL) = (resource_through_at IS NULL)),
    CHECK (
        resource_first_at IS NULL
        OR resource_first_at <= resource_through_at
    ),
    CHECK (public.telemetry_dashboard_change_is_valid(
        resource_change,
        resource_change_source_bucket_secs,
        resource_change_block_start_unix
    ))
);

CREATE TABLE public.telemetry_dashboard_network_generations (
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    select_all BOOLEAN NOT NULL,
    interfaces TEXT[] NOT NULL,
    interface_width INTEGER NOT NULL CHECK (interface_width >= 0),
    PRIMARY KEY (client_id, generation),
    UNIQUE (client_id, generation, interface_width),
    UNIQUE (
        client_id, generation, select_all, interfaces, interface_width
    ),
    CHECK (public.telemetry_dashboard_interfaces_are_canonical(interfaces)),
    CHECK (interface_width = cardinality(interfaces))
);

CREATE TABLE public.telemetry_dashboard_traffic_generations (
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    source_kinds TEXT[] NOT NULL,
    interfaces TEXT[] NOT NULL,
    stream_width INTEGER NOT NULL CHECK (stream_width >= 0),
    PRIMARY KEY (client_id, generation),
    UNIQUE (client_id, generation, stream_width),
    UNIQUE (
        client_id, generation, source_kinds, interfaces, stream_width
    ),
    CHECK (public.telemetry_dashboard_traffic_identities_are_canonical(
        source_kinds, interfaces
    )),
    CHECK (
        stream_width = cardinality(source_kinds)
        AND stream_width = cardinality(interfaces)
    )
);

CREATE TABLE public.telemetry_dashboard_network_projection_heads (
    client_id TEXT PRIMARY KEY
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    network_generation BIGINT NOT NULL DEFAULT 1
        CHECK (network_generation > 0),
    network_revision BIGINT NOT NULL DEFAULT 0
        CHECK (network_revision >= 0),
    network_change TEXT NOT NULL DEFAULT 'generation',
    network_change_source_bucket_secs INTEGER[] NOT NULL
        DEFAULT ARRAY[]::INTEGER[],
    network_change_block_start_unix BIGINT[] NOT NULL
        DEFAULT ARRAY[]::BIGINT[],
    network_select_all BOOLEAN NOT NULL DEFAULT FALSE,
    network_generation_interfaces TEXT[] NOT NULL DEFAULT ARRAY[]::TEXT[],
    network_interface_width INTEGER NOT NULL DEFAULT 0
        CHECK (network_interface_width >= 0),
    network_first_at TIMESTAMPTZ,
    network_through_at TIMESTAMPTZ,
    FOREIGN KEY (
        client_id, network_generation, network_select_all,
        network_generation_interfaces, network_interface_width
    ) REFERENCES public.telemetry_dashboard_network_generations (
        client_id, generation, select_all, interfaces, interface_width
    ),
    CHECK (public.telemetry_dashboard_interfaces_are_canonical(
        network_generation_interfaces
    )),
    CHECK (
        network_interface_width = cardinality(network_generation_interfaces)
    ),
    CHECK ((network_first_at IS NULL) = (network_through_at IS NULL)),
    CHECK (
        network_first_at IS NULL
        OR network_first_at <= network_through_at
    ),
    CHECK (public.telemetry_dashboard_change_is_valid(
        network_change,
        network_change_source_bucket_secs,
        network_change_block_start_unix
    ))
);

CREATE TABLE public.telemetry_dashboard_traffic_projection_heads (
    client_id TEXT PRIMARY KEY
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    traffic_generation BIGINT NOT NULL DEFAULT 1
        CHECK (traffic_generation > 0),
    traffic_revision BIGINT NOT NULL DEFAULT 0
        CHECK (traffic_revision >= 0),
    traffic_change TEXT NOT NULL DEFAULT 'generation',
    traffic_change_source_bucket_secs INTEGER[] NOT NULL
        DEFAULT ARRAY[]::INTEGER[],
    traffic_change_block_start_unix BIGINT[] NOT NULL
        DEFAULT ARRAY[]::BIGINT[],
    traffic_generation_source_kinds TEXT[] NOT NULL
        DEFAULT ARRAY[]::TEXT[],
    traffic_generation_interfaces TEXT[] NOT NULL
        DEFAULT ARRAY[]::TEXT[],
    traffic_stream_width INTEGER NOT NULL DEFAULT 0
        CHECK (traffic_stream_width >= 0),
    traffic_first_at TIMESTAMPTZ,
    traffic_through_at TIMESTAMPTZ,
    FOREIGN KEY (
        client_id, traffic_generation,
        traffic_generation_source_kinds,
        traffic_generation_interfaces,
        traffic_stream_width
    ) REFERENCES public.telemetry_dashboard_traffic_generations (
        client_id, generation, source_kinds, interfaces, stream_width
    ),
    CHECK (public.telemetry_dashboard_traffic_identities_are_canonical(
        traffic_generation_source_kinds,
        traffic_generation_interfaces
    )),
    CHECK (
        traffic_stream_width = cardinality(
            traffic_generation_source_kinds
        )
        AND traffic_stream_width = cardinality(
            traffic_generation_interfaces
        )
    ),
    CHECK ((traffic_first_at IS NULL) = (traffic_through_at IS NULL)),
    CHECK (
        traffic_first_at IS NULL
        OR traffic_first_at <= traffic_through_at
    ),
    CHECK (public.telemetry_dashboard_traffic_change_is_valid(
        traffic_change,
        traffic_change_source_bucket_secs,
        traffic_change_block_start_unix
    ))
);

CREATE TABLE public.telemetry_dashboard_ping_projection_heads (
    client_id TEXT PRIMARY KEY
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    ping_first_at TIMESTAMPTZ
);

CREATE VIEW public.telemetry_dashboard_projection_heads AS
SELECT resource.client_id,
       resource.resource_generation,
       resource.resource_revision,
       resource.resource_change,
       resource.resource_change_source_bucket_secs,
       resource.resource_change_block_start_unix,
       resource.resource_first_at,
       resource.resource_through_at,
       network.network_generation,
       network.network_revision,
       network.network_change,
       network.network_change_source_bucket_secs,
       network.network_change_block_start_unix,
       network.network_select_all,
       network.network_generation_interfaces,
       network.network_interface_width,
       network.network_first_at,
       network.network_through_at,
       traffic.traffic_generation,
       traffic.traffic_revision,
       traffic.traffic_change,
       traffic.traffic_change_source_bucket_secs,
       traffic.traffic_change_block_start_unix,
       traffic.traffic_generation_source_kinds,
       traffic.traffic_generation_interfaces,
       traffic.traffic_stream_width,
       traffic.traffic_first_at,
       traffic.traffic_through_at,
       ping.ping_first_at
FROM public.telemetry_dashboard_resource_projection_heads resource
JOIN public.telemetry_dashboard_network_projection_heads network
  USING (client_id)
JOIN public.telemetry_dashboard_traffic_projection_heads traffic
  USING (client_id)
JOIN public.telemetry_dashboard_ping_projection_heads ping
  USING (client_id);

CREATE TABLE public.telemetry_dashboard_projection_fences (
    owner_id BIGINT GENERATED ALWAYS AS IDENTITY
        PRIMARY KEY CHECK (owner_id > 0),
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    domain TEXT NOT NULL CHECK (
        domain IN ('resource', 'network', 'traffic')
    ),
    UNIQUE (client_id, domain)
);

CREATE TABLE public.telemetry_dashboard_block_events (
    event_id BIGINT PRIMARY KEY DEFAULT
        nextval('public.telemetry_dashboard_event_seq'),
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    domain TEXT NOT NULL CHECK (
        domain IN ('resource', 'network', 'traffic')
    ),
    event_kind TEXT NOT NULL
        CHECK (event_kind IN ('coordinate', 'full_block')),
    source_bucket_secs INTEGER NOT NULL CHECK (
        (
            domain = 'traffic'
            AND public.telemetry_dashboard_traffic_source_tier_is_valid(
                source_bucket_secs
            )
        ) OR (
            domain <> 'traffic'
            AND public.telemetry_dashboard_source_tier_is_valid(
                source_bucket_secs
            )
        )
    ),
    block_start_unix BIGINT NOT NULL,
    bucket_start_unix BIGINT,
    queued_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (event_id > 0),
    CHECK (
        block_start_unix = public.telemetry_dashboard_block_start(
            block_start_unix, source_bucket_secs
        )
    ),
    CHECK (
        (event_kind = 'full_block' AND bucket_start_unix IS NULL)
        OR (
            event_kind = 'coordinate'
            AND bucket_start_unix IS NOT NULL
            AND mod(bucket_start_unix, source_bucket_secs) = 0
            AND block_start_unix =
                public.telemetry_dashboard_block_start(
                    bucket_start_unix, source_bucket_secs
                )
        )
    )
);

-- Traffic history has two canonical owners only: exact unpromoted counter
-- minutes and retained counter rollups. Authoritative minutes carry their
-- independently valid directional usage; non-authoritative endpoints derive
-- it from the immediate stream predecessor. The requested identity arrays are a
-- generation-fenced set; values are aggregated per client/native bucket and
-- never persisted as per-interface vectors.
CREATE FUNCTION public.telemetry_dashboard_traffic_source_points(
    p_client_id TEXT,
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER DEFAULT NULL,
    p_first_bucket_start TIMESTAMPTZ DEFAULT NULL,
    p_last_bucket_start TIMESTAMPTZ DEFAULT NULL
)
RETURNS TABLE (
    client_id TEXT,
    bucket_start TIMESTAMPTZ,
    bucket_secs INTEGER,
    rx_valid_count BIGINT,
    tx_valid_count BIGINT,
    rx_bytes BIGINT,
    tx_bytes BIGINT
)
LANGUAGE plpgsql
STABLE
AS $$
BEGIN
    -- Exact minutes and retained tiers are disjoint physical owners.  Select
    -- the owner before planning its relation so a live 60-second coordinate
    -- never probes years of retained traffic history.
    IF p_source_bucket_secs IS NULL OR p_source_bucket_secs = 60 THEN
        RETURN QUERY
        WITH requested AS MATERIALIZED (
            SELECT identity.source_kind, identity.interface
            FROM unnest(p_source_kinds, p_interfaces)
                identity(source_kind, interface)
        ), raw AS MATERIALIZED (
            SELECT sample.source_kind,
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
            FROM requested
            JOIN public.traffic_counter_samples sample
              ON sample.client_id = p_client_id
             AND sample.source_kind = requested.source_kind
             AND sample.interface = requested.interface
            WHERE NOT sample.inbound_promoted
              AND (
                  p_first_bucket_start IS NULL
                  OR sample.observed_at >= p_first_bucket_start
              )
              AND (
                  p_last_bucket_start IS NULL
                  OR sample.observed_at <= p_last_bucket_start
              )
        ), raw_evaluated AS MATERIALIZED (
            -- Materialized live minutes already own exact directional usage;
            -- only imported/non-authoritative endpoints need a predecessor.
            SELECT source.source_kind,
                   source.interface,
                   source.observed_at,
                   source.rx_valid_count::BIGINT
                       AS effective_rx_valid_count,
                   source.tx_valid_count::BIGINT
                       AS effective_tx_valid_count,
                   CASE WHEN source.rx_valid_count > 0
                       THEN source.rx_usage_bytes ELSE 0::BIGINT
                   END AS effective_rx_bytes,
                   CASE WHEN source.tx_valid_count > 0
                       THEN source.tx_usage_bytes ELSE 0::BIGINT
                   END AS effective_tx_bytes
            FROM raw source
            WHERE source.usage_authoritative

            UNION ALL

            SELECT source.source_kind,
                   source.interface,
                   source.observed_at,
                   CASE
                       WHEN source.rx_counter_epoch =
                                predecessor.rx_counter_epoch
                        AND source.rx_bytes >= predecessor.rx_bytes
                       THEN 1::BIGINT ELSE 0::BIGINT
                   END AS effective_rx_valid_count,
                   CASE
                       WHEN source.tx_counter_epoch =
                                predecessor.tx_counter_epoch
                        AND source.tx_bytes >= predecessor.tx_bytes
                       THEN 1::BIGINT ELSE 0::BIGINT
                   END AS effective_tx_valid_count,
                   CASE
                       WHEN source.rx_counter_epoch =
                                predecessor.rx_counter_epoch
                        AND source.rx_bytes >= predecessor.rx_bytes
                       THEN source.rx_bytes - predecessor.rx_bytes
                       ELSE 0::BIGINT
                   END AS effective_rx_bytes,
                   CASE
                       WHEN source.tx_counter_epoch =
                                predecessor.tx_counter_epoch
                        AND source.tx_bytes >= predecessor.tx_bytes
                       THEN source.tx_bytes - predecessor.tx_bytes
                       ELSE 0::BIGINT
                   END AS effective_tx_bytes
            FROM raw source
            LEFT JOIN LATERAL (
                SELECT prior.rx_bytes,
                       prior.tx_bytes,
                       prior.rx_counter_epoch,
                       prior.tx_counter_epoch
                FROM public.traffic_counter_samples prior
                WHERE prior.client_id = p_client_id
                  AND prior.source_kind = source.source_kind
                  AND prior.interface = source.interface
                  AND prior.observed_at < source.observed_at
                ORDER BY prior.observed_at DESC
                LIMIT 1
            ) predecessor ON TRUE
            WHERE NOT source.usage_authoritative
        )
        SELECT p_client_id AS client_id,
               source.observed_at AS bucket_start,
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
        GROUP BY source.observed_at;
    END IF;

    IF p_source_bucket_secs IS NULL OR p_source_bucket_secs <> 60 THEN
        RETURN QUERY
        WITH requested AS MATERIALIZED (
            SELECT identity.source_kind, identity.interface
            FROM unnest(p_source_kinds, p_interfaces)
                identity(source_kind, interface)
        )
        SELECT p_client_id AS client_id,
               rollup.bucket_start,
               rollup.bucket_secs,
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
        FROM requested
        JOIN public.traffic_counter_rollups rollup
          ON rollup.client_id = p_client_id
         AND rollup.source_kind = requested.source_kind
         AND rollup.interface = requested.interface
        WHERE (
              p_source_bucket_secs IS NULL
              OR rollup.bucket_secs = p_source_bucket_secs
          )
          AND (
              p_first_bucket_start IS NULL
              OR rollup.bucket_start >= p_first_bucket_start
          )
          AND (
              p_last_bucket_start IS NULL
              OR rollup.bucket_start <= p_last_bucket_start
          )
          AND NOT EXISTS (
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
        GROUP BY rollup.bucket_start, rollup.bucket_secs;
    END IF;
END;
$$;

-- Internal readers name their exact owners and, for an incremental read, the
-- exact F16 coordinates.  Put those predicates before suffix discovery and
-- its DISTINCT/LATERAL stages: the amount of work is then bounded by the
-- requested owners' open raw suffix rather than by the fleet-wide suffix.
-- NULL owner/coordinate arrays deliberately retain complete direct-source
-- inspection semantics.
CREATE FUNCTION public.telemetry_dashboard_traffic_overlay_source(
    p_client_ids TEXT[],
    p_source_bucket_secs INTEGER[] DEFAULT NULL,
    p_block_start_unix BIGINT[] DEFAULT NULL
)
RETURNS TABLE (
    client_id TEXT,
    bucket_start TIMESTAMPTZ,
    bucket_secs INTEGER,
    rx_valid_count BIGINT,
    tx_valid_count BIGINT,
    rx_bytes BIGINT,
    tx_bytes BIGINT
)
LANGUAGE sql
STABLE
AS $$
    WITH requested_blocks AS MATERIALIZED (
        SELECT DISTINCT requested.source_bucket_secs,
               requested.block_start_unix
        FROM unnest(p_source_bucket_secs, p_block_start_unix)
            requested(source_bucket_secs, block_start_unix)
    ), requested_heads AS MATERIALIZED (
        SELECT head.*
        FROM public.telemetry_dashboard_traffic_projection_heads head
        WHERE p_client_ids IS NULL
           OR head.client_id = ANY(p_client_ids)
    ), touched AS MATERIALIZED (
        SELECT DISTINCT head.client_id,
               date_trunc('minute', sample.observed_at) AS bucket_start
        FROM requested_heads head
        JOIN public.traffic_counter_minute_heads minute
          ON minute.client_id = head.client_id
        JOIN public.telemetry_projection_heads projection
          ON projection.client_id = minute.client_id
        JOIN public.telemetry_samples sample
          ON sample.client_id = minute.client_id
         AND sample.accepted_seq > minute.materialized_seq
         AND sample.accepted_seq <= projection.projected_seq
        WHERE p_source_bucket_secs IS NULL
           OR EXISTS (
                SELECT 1
                FROM requested_blocks requested
                WHERE requested.source_bucket_secs = 60
                  AND requested.block_start_unix =
                        public.telemetry_dashboard_block_start(
                            extract(epoch FROM sample.observed_at)::BIGINT,
                            60
                        )
           )
    )
    SELECT source.client_id,
           source.bucket_start,
           source.bucket_secs,
           source.rx_valid_count,
           source.tx_valid_count,
           source.rx_bytes,
           source.tx_bytes
    FROM touched
    JOIN requested_heads head USING (client_id)
    CROSS JOIN LATERAL public.telemetry_dashboard_traffic_source_points(
        touched.client_id,
        head.traffic_generation_source_kinds,
        head.traffic_generation_interfaces,
        60,
        touched.bucket_start,
        touched.bucket_start
    ) source
$$;
-- Raw suffix minutes are the sole unpublished owner. Internal readers state
-- their client and optional F16 coordinates before canonical raw discovery;
-- NULL coordinates retain complete direct-source inspection semantics.
CREATE FUNCTION public.telemetry_dashboard_resource_overlay_source(
    p_client_ids TEXT[],
    p_source_bucket_secs INTEGER[] DEFAULT NULL,
    p_block_start_unix BIGINT[] DEFAULT NULL
)
RETURNS TABLE (
    client_id TEXT,
    bucket_start TIMESTAMPTZ,
    bucket_secs INTEGER,
    sample_count INTEGER,
    cpu_load_1_sum DOUBLE PRECISION,
    cpu_load_1_max DOUBLE PRECISION,
    memory_total_bytes_max BIGINT,
    memory_used_ratio_sum DOUBLE PRECISION,
    memory_used_ratio_max DOUBLE PRECISION,
    disk_sample_count INTEGER,
    disk_total_bytes_max BIGINT,
    disk_used_ratio_sum DOUBLE PRECISION,
    disk_used_ratio_max DOUBLE PRECISION,
    latest_observed_at TIMESTAMPTZ
)
LANGUAGE sql
STABLE
AS $$
    WITH requested_blocks AS MATERIALIZED (
        SELECT DISTINCT requested.source_bucket_secs,
               requested.block_start_unix
        FROM unnest(p_source_bucket_secs, p_block_start_unix)
            requested(source_bucket_secs, block_start_unix)
    )
    SELECT suffix.client_id, suffix.bucket_start, suffix.bucket_secs,
           suffix.sample_count, suffix.cpu_load_1_sum, suffix.cpu_load_1_max,
           suffix.memory_total_bytes_max, suffix.memory_used_ratio_sum,
           suffix.memory_used_ratio_max, suffix.disk_sample_count,
           suffix.disk_total_bytes_max, suffix.disk_used_ratio_sum,
           suffix.disk_used_ratio_max, suffix.latest_observed_at
    FROM public.telemetry_projected_raw_resource_minutes_source(
        p_client_ids
    ) suffix
    WHERE p_source_bucket_secs IS NULL
       OR EXISTS (
            SELECT 1
            FROM requested_blocks requested
            WHERE requested.source_bucket_secs = suffix.bucket_secs
              AND requested.block_start_unix =
                    public.telemetry_dashboard_block_start(
                        extract(epoch FROM suffix.bucket_start)::BIGINT,
                        suffix.bucket_secs
                    )
       )
$$;
CREATE FUNCTION public.telemetry_dashboard_network_overlay_source(
    p_client_ids TEXT[],
    p_source_bucket_secs INTEGER[] DEFAULT NULL,
    p_block_start_unix BIGINT[] DEFAULT NULL
)
RETURNS TABLE (
    client_id TEXT,
    interface TEXT,
    bucket_start TIMESTAMPTZ,
    bucket_secs INTEGER,
    sample_count INTEGER,
    latest_observed_at TIMESTAMPTZ,
    rx_bytes_last BIGINT,
    tx_bytes_last BIGINT,
    rx_counter_epoch BIGINT,
    tx_counter_epoch BIGINT
)
LANGUAGE sql
STABLE
AS $$
    WITH requested_blocks AS MATERIALIZED (
        SELECT DISTINCT requested.source_bucket_secs,
               requested.block_start_unix
        FROM unnest(p_source_bucket_secs, p_block_start_unix)
            requested(source_bucket_secs, block_start_unix)
    )
    SELECT
        suffix.client_id,
        suffix.interface,
        suffix.bucket_start,
        suffix.bucket_secs,
        suffix.sample_count,
        suffix.latest_observed_at,
        suffix.rx_bytes_last,
        suffix.tx_bytes_last,
        suffix.rx_counter_epoch,
        suffix.tx_counter_epoch
    FROM public.telemetry_projected_raw_network_minutes_source(
        p_client_ids
    ) suffix
    WHERE p_source_bucket_secs IS NULL
       OR EXISTS (
            SELECT 1
            FROM requested_blocks requested
            WHERE requested.source_bucket_secs = suffix.bucket_secs
              AND requested.block_start_unix =
                    public.telemetry_dashboard_block_start(
                        extract(epoch FROM suffix.bucket_start)::BIGINT,
                        suffix.bucket_secs
                    )
       )
$$;
CREATE INDEX telemetry_dashboard_block_events_client_age_idx
ON public.telemetry_dashboard_block_events (
    client_id, queued_at, event_id
);

CREATE INDEX telemetry_dashboard_block_events_owner_event_idx
ON public.telemetry_dashboard_block_events (
    client_id, domain, event_id
);

CREATE TABLE public.telemetry_dashboard_generation_events (
    event_id BIGINT PRIMARY KEY DEFAULT
        nextval('public.telemetry_dashboard_event_seq'),
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    domain TEXT NOT NULL CHECK (
        domain IN ('resource', 'network', 'traffic')
    ),
    queued_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    CHECK (event_id > 0)
);

CREATE INDEX telemetry_dashboard_generation_events_client_age_idx
ON public.telemetry_dashboard_generation_events (
    client_id, queued_at, event_id
);

CREATE INDEX telemetry_dashboard_generation_events_owner_event_idx
ON public.telemetry_dashboard_generation_events (
    client_id, domain, event_id
);

-- A ready row is only a bounded owner-discovery hint. Immutable source events
-- remain the publication authority. wake_revision fences post-commit cleanup:
-- an enqueue concurrent with publication changes the token, so cleanup cannot
-- remove the newer wakeup. retry_not_before defers only a failed derived
-- publication; every newer enqueue resets that owner to immediate eligibility.
CREATE TABLE public.telemetry_dashboard_ready_owners (
    owner_id BIGINT PRIMARY KEY
        REFERENCES public.telemetry_dashboard_projection_fences(owner_id)
        ON DELETE CASCADE,
    ready_at TIMESTAMPTZ NOT NULL,
    retry_not_before TIMESTAMPTZ NOT NULL
        DEFAULT '-infinity'::TIMESTAMPTZ,
    wake_revision BIGINT NOT NULL DEFAULT 1
        CHECK (wake_revision > 0)
);

CREATE INDEX telemetry_dashboard_ready_owners_fifo_idx
ON public.telemetry_dashboard_ready_owners (
    ready_at, owner_id
);

CREATE FUNCTION public.mark_telemetry_dashboard_owners_ready()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_ready_owners AS ready (
        owner_id, ready_at, retry_not_before, wake_revision
    )
    SELECT fence.owner_id,
           min(event.queued_at),
           '-infinity'::TIMESTAMPTZ,
           1
    FROM new_telemetry_dashboard_events event
    JOIN public.telemetry_dashboard_projection_fences fence
      ON fence.client_id = event.client_id
     AND fence.domain = event.domain
    GROUP BY fence.owner_id
    ON CONFLICT (owner_id) DO UPDATE SET
        ready_at = LEAST(ready.ready_at, EXCLUDED.ready_at),
        retry_not_before = '-infinity'::TIMESTAMPTZ,
        wake_revision = ready.wake_revision + 1;
    RETURN NULL;
END
$$;

CREATE TRIGGER telemetry_dashboard_block_events_mark_owners_ready
AFTER INSERT ON public.telemetry_dashboard_block_events
REFERENCING NEW TABLE AS new_telemetry_dashboard_events
FOR EACH STATEMENT
EXECUTE FUNCTION public.mark_telemetry_dashboard_owners_ready();

CREATE TRIGGER telemetry_dashboard_generation_events_mark_owners_ready
AFTER INSERT ON public.telemetry_dashboard_generation_events
REFERENCING NEW TABLE AS new_telemetry_dashboard_events
FOR EACH STATEMENT
EXECUTE FUNCTION public.mark_telemetry_dashboard_owners_ready();

CREATE TABLE public.telemetry_dashboard_resource_generation_bounds (
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    source_bucket_secs INTEGER NOT NULL CHECK (
        public.telemetry_dashboard_source_tier_is_valid(source_bucket_secs)
    ),
    first_bucket_start_unix BIGINT NOT NULL,
    last_bucket_start_unix BIGINT NOT NULL,
    active_block_start_unix BIGINT NOT NULL,
    PRIMARY KEY (client_id, generation, source_bucket_secs),
    CHECK (first_bucket_start_unix <= last_bucket_start_unix),
    CHECK (mod(first_bucket_start_unix, source_bucket_secs) = 0),
    CHECK (mod(last_bucket_start_unix, source_bucket_secs) = 0),
    CHECK (
        active_block_start_unix = public.telemetry_dashboard_block_start(
            last_bucket_start_unix, source_bucket_secs
        )
    )
);

CREATE TABLE public.telemetry_dashboard_network_generation_bounds (
    client_id TEXT NOT NULL,
    generation BIGINT NOT NULL,
    interface_width INTEGER NOT NULL CHECK (interface_width > 0),
    source_bucket_secs INTEGER NOT NULL CHECK (
        public.telemetry_dashboard_source_tier_is_valid(source_bucket_secs)
    ),
    first_bucket_start_unix BIGINT NOT NULL,
    last_bucket_start_unix BIGINT NOT NULL,
    active_block_start_unix BIGINT NOT NULL,
    PRIMARY KEY (client_id, generation, source_bucket_secs),
    FOREIGN KEY (
        client_id, generation, interface_width
    ) REFERENCES public.telemetry_dashboard_network_generations (
        client_id, generation, interface_width
    ) ON DELETE CASCADE,
    CHECK (first_bucket_start_unix <= last_bucket_start_unix),
    CHECK (mod(first_bucket_start_unix, source_bucket_secs) = 0),
    CHECK (mod(last_bucket_start_unix, source_bucket_secs) = 0),
    CHECK (
        active_block_start_unix = public.telemetry_dashboard_block_start(
            last_bucket_start_unix, source_bucket_secs
        )
    )
);

CREATE TABLE public.telemetry_dashboard_traffic_generation_bounds (
    client_id TEXT NOT NULL,
    generation BIGINT NOT NULL,
    stream_width INTEGER NOT NULL CHECK (stream_width > 0),
    source_bucket_secs INTEGER NOT NULL CHECK (
        public.telemetry_dashboard_traffic_source_tier_is_valid(
            source_bucket_secs
        )
    ),
    first_bucket_start_unix BIGINT NOT NULL,
    last_bucket_start_unix BIGINT NOT NULL,
    active_block_start_unix BIGINT NOT NULL,
    PRIMARY KEY (client_id, generation, source_bucket_secs),
    FOREIGN KEY (
        client_id, generation, stream_width
    ) REFERENCES public.telemetry_dashboard_traffic_generations (
        client_id, generation, stream_width
    ) ON DELETE CASCADE,
    CHECK (first_bucket_start_unix <= last_bucket_start_unix),
    CHECK (mod(first_bucket_start_unix, source_bucket_secs) = 0),
    CHECK (mod(last_bucket_start_unix, source_bucket_secs) = 0),
    CHECK (
        active_block_start_unix = public.telemetry_dashboard_block_start(
            last_bucket_start_unix, source_bucket_secs
        )
    )
);

CREATE TABLE public.telemetry_dashboard_resource_blocks (
    client_id TEXT NOT NULL
        REFERENCES public.telemetry_dashboard_clients(client_id)
        ON DELETE CASCADE,
    generation BIGINT NOT NULL CHECK (generation > 0),
    source_bucket_secs INTEGER NOT NULL CHECK (
        public.telemetry_dashboard_source_tier_is_valid(source_bucket_secs)
    ),
    block_start_unix BIGINT NOT NULL,
    published_revision BIGINT NOT NULL CHECK (published_revision > 0),
    sample_counts BIGINT[] NOT NULL,
    cpu_load_1_sums DOUBLE PRECISION[] NOT NULL,
    cpu_load_1_maxes REAL[] NOT NULL,
    memory_total_bytes_maxes BIGINT[] NOT NULL,
    memory_used_ratio_sums DOUBLE PRECISION[] NOT NULL,
    memory_used_ratio_maxes REAL[] NOT NULL,
    disk_sample_counts BIGINT[] NOT NULL,
    disk_total_bytes_maxes BIGINT[] NOT NULL,
    disk_used_ratio_sums DOUBLE PRECISION[] NOT NULL,
    disk_used_ratio_maxes REAL[] NOT NULL,
    latest_observed_unix BIGINT[] NOT NULL,
    PRIMARY KEY (
        client_id, generation, source_bucket_secs, block_start_unix
    ),
    CHECK (
        block_start_unix = public.telemetry_dashboard_block_start(
            block_start_unix, source_bucket_secs
        )
    ),
    CHECK (
        array_ndims(sample_counts) = 1
        AND array_lower(sample_counts, 1) = 1
        AND cardinality(sample_counts) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(cpu_load_1_sums) = 1
        AND array_lower(cpu_load_1_sums, 1) = 1
        AND cardinality(cpu_load_1_sums) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(cpu_load_1_maxes) = 1
        AND array_lower(cpu_load_1_maxes, 1) = 1
        AND cardinality(cpu_load_1_maxes) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(memory_total_bytes_maxes) = 1
        AND array_lower(memory_total_bytes_maxes, 1) = 1
        AND cardinality(memory_total_bytes_maxes) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(memory_used_ratio_sums) = 1
        AND array_lower(memory_used_ratio_sums, 1) = 1
        AND cardinality(memory_used_ratio_sums) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(memory_used_ratio_maxes) = 1
        AND array_lower(memory_used_ratio_maxes, 1) = 1
        AND cardinality(memory_used_ratio_maxes) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(disk_sample_counts) = 1
        AND array_lower(disk_sample_counts, 1) = 1
        AND cardinality(disk_sample_counts) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(disk_total_bytes_maxes) = 1
        AND array_lower(disk_total_bytes_maxes, 1) = 1
        AND cardinality(disk_total_bytes_maxes) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(disk_used_ratio_sums) = 1
        AND array_lower(disk_used_ratio_sums, 1) = 1
        AND cardinality(disk_used_ratio_sums) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(disk_used_ratio_maxes) = 1
        AND array_lower(disk_used_ratio_maxes, 1) = 1
        AND cardinality(disk_used_ratio_maxes) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(latest_observed_unix) = 1
        AND array_lower(latest_observed_unix, 1) = 1
        AND cardinality(latest_observed_unix) =
            public.telemetry_dashboard_block_factor()
    ),
    CHECK (public.telemetry_dashboard_resource_vectors_are_valid(
        sample_counts, cpu_load_1_sums, cpu_load_1_maxes,
        memory_total_bytes_maxes, memory_used_ratio_sums,
        memory_used_ratio_maxes, disk_sample_counts,
        disk_total_bytes_maxes, disk_used_ratio_sums,
        disk_used_ratio_maxes, latest_observed_unix
    ))
);

CREATE TABLE public.telemetry_dashboard_network_blocks (
    client_id TEXT NOT NULL,
    generation BIGINT NOT NULL,
    interface_width INTEGER NOT NULL CHECK (interface_width > 0),
    source_bucket_secs INTEGER NOT NULL CHECK (
        public.telemetry_dashboard_source_tier_is_valid(source_bucket_secs)
    ),
    block_start_unix BIGINT NOT NULL,
    published_revision BIGINT NOT NULL CHECK (published_revision > 0),
    sample_counts BIGINT[] NOT NULL,
    latest_observed_unix BIGINT[] NOT NULL,
    rx_bytes_last BIGINT[] NOT NULL,
    tx_bytes_last BIGINT[] NOT NULL,
    rx_counter_epoch BIGINT[] NOT NULL,
    tx_counter_epoch BIGINT[] NOT NULL,
    PRIMARY KEY (
        client_id, generation, source_bucket_secs, block_start_unix
    ),
    FOREIGN KEY (
        client_id, generation, interface_width
    ) REFERENCES public.telemetry_dashboard_network_generations (
        client_id, generation, interface_width
    ) ON DELETE CASCADE,
    CHECK (
        block_start_unix = public.telemetry_dashboard_block_start(
            block_start_unix, source_bucket_secs
        )
    ),
    CHECK (
        array_ndims(sample_counts) = 1
        AND array_lower(sample_counts, 1) = 1
        AND cardinality(sample_counts) =
            public.telemetry_dashboard_block_factor() * interface_width
        AND array_ndims(latest_observed_unix) = 1
        AND array_lower(latest_observed_unix, 1) = 1
        AND cardinality(latest_observed_unix) =
            public.telemetry_dashboard_block_factor() * interface_width
        AND array_ndims(rx_bytes_last) = 1
        AND array_lower(rx_bytes_last, 1) = 1
        AND cardinality(rx_bytes_last) =
            public.telemetry_dashboard_block_factor() * interface_width
        AND array_ndims(tx_bytes_last) = 1
        AND array_lower(tx_bytes_last, 1) = 1
        AND cardinality(tx_bytes_last) =
            public.telemetry_dashboard_block_factor() * interface_width
        AND array_ndims(rx_counter_epoch) = 1
        AND array_lower(rx_counter_epoch, 1) = 1
        AND cardinality(rx_counter_epoch) =
            public.telemetry_dashboard_block_factor() * interface_width
        AND array_ndims(tx_counter_epoch) = 1
        AND array_lower(tx_counter_epoch, 1) = 1
        AND cardinality(tx_counter_epoch) =
            public.telemetry_dashboard_block_factor() * interface_width
    ),
    CHECK (public.telemetry_dashboard_network_vectors_are_valid(
        sample_counts, latest_observed_unix,
        rx_bytes_last, tx_bytes_last,
        rx_counter_epoch, tx_counter_epoch
    ))
);

CREATE TABLE public.telemetry_dashboard_traffic_blocks (
    client_id TEXT NOT NULL,
    generation BIGINT NOT NULL,
    source_bucket_secs INTEGER NOT NULL CHECK (
        public.telemetry_dashboard_traffic_source_tier_is_valid(
            source_bucket_secs
        )
    ),
    block_start_unix BIGINT NOT NULL,
    published_revision BIGINT NOT NULL CHECK (published_revision > 0),
    rx_valid_counts BIGINT[] NOT NULL,
    tx_valid_counts BIGINT[] NOT NULL,
    rx_bytes BIGINT[] NOT NULL,
    tx_bytes BIGINT[] NOT NULL,
    PRIMARY KEY (
        client_id, generation, source_bucket_secs, block_start_unix
    ),
    FOREIGN KEY (
        client_id, generation
    ) REFERENCES public.telemetry_dashboard_traffic_generations (
        client_id, generation
    ) ON DELETE CASCADE,
    CHECK (
        block_start_unix = public.telemetry_dashboard_block_start(
            block_start_unix, source_bucket_secs
        )
    ),
    CHECK (
        array_ndims(rx_valid_counts) = 1
        AND array_lower(rx_valid_counts, 1) = 1
        AND cardinality(rx_valid_counts) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(tx_valid_counts) = 1
        AND array_lower(tx_valid_counts, 1) = 1
        AND cardinality(tx_valid_counts) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(rx_bytes) = 1
        AND array_lower(rx_bytes, 1) = 1
        AND cardinality(rx_bytes) =
            public.telemetry_dashboard_block_factor()
        AND array_ndims(tx_bytes) = 1
        AND array_lower(tx_bytes, 1) = 1
        AND cardinality(tx_bytes) =
            public.telemetry_dashboard_block_factor()
    ),
    CHECK (public.telemetry_dashboard_traffic_vectors_are_valid(
        rx_valid_counts, tx_valid_counts, rx_bytes, tx_bytes
    ))
);

CREATE TABLE public.telemetry_dashboard_ping_series_bounds (
    series_id BIGINT PRIMARY KEY
        REFERENCES public.telemetry_ping_series(id) ON DELETE CASCADE,
    first_bucket_start TIMESTAMPTZ NOT NULL,
    last_bucket_start TIMESTAMPTZ NOT NULL,
    CHECK (first_bucket_start <= last_bucket_start)
);

CREATE FUNCTION public.refresh_telemetry_dashboard_ping_series_bound_edges(
    p_series_id BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    first_at TIMESTAMPTZ;
    last_at TIMESTAMPTZ;
BEGIN
    -- These bounds describe the retained owner only.  The raw suffix has its
    -- own bounded range probe and must not leak into retention edge repair.
    -- The series/range index stops each lookup at one physical row regardless
    -- of retained age or the number of rollup tiers.
    SELECT (
               SELECT source.bucket_start
               FROM public.telemetry_ping_rollups source
               WHERE source.series_id = p_series_id
               ORDER BY source.bucket_start
               LIMIT 1
           ),
           (
               SELECT source.bucket_start
               FROM public.telemetry_ping_rollups source
               WHERE source.series_id = p_series_id
               ORDER BY source.bucket_start DESC
               LIMIT 1
           )
    INTO first_at, last_at;

    IF first_at IS NULL THEN
        DELETE FROM public.telemetry_dashboard_ping_series_bounds
        WHERE series_id = p_series_id;
    ELSE
        INSERT INTO public.telemetry_dashboard_ping_series_bounds (
            series_id, first_bucket_start, last_bucket_start
        )
        VALUES (p_series_id, first_at, last_at)
        ON CONFLICT (series_id) DO UPDATE SET
            first_bucket_start = EXCLUDED.first_bucket_start,
            last_bucket_start = EXCLUDED.last_bucket_start
        WHERE (
            telemetry_dashboard_ping_series_bounds.first_bucket_start,
            telemetry_dashboard_ping_series_bounds.last_bucket_start
        ) IS DISTINCT FROM (
            EXCLUDED.first_bucket_start, EXCLUDED.last_bucket_start
        );
    END IF;
END
$$;

CREATE FUNCTION public.telemetry_dashboard_effective_network_selection(
    p_client_id TEXT
)
RETURNS public.telemetry_dashboard_network_selection
LANGUAGE plpgsql
STABLE
AS $$
DECLARE
    interface_rule JSONB;
    rate_rule JSONB;
    traffic_rule JSONB;
    interface_mode TEXT;
    rate_mode TEXT;
    traffic_mode TEXT;
    requested_all BOOLEAN := FALSE;
    admitted_all BOOLEAN := FALSE;
    requested_interfaces TEXT[] := ARRAY[]::TEXT[];
    admitted_patterns TEXT[] := ARRAY[]::TEXT[];
    interfaces TEXT[] := ARRAY[]::TEXT[];
BEGIN
    SELECT (
               SELECT value_json
               FROM public.vps_rule_values
               WHERE client_id = p_client_id
                 AND key = 'network.interfaces'
           ),
           (
               SELECT value_json
               FROM public.vps_rule_values
               WHERE client_id = p_client_id
                 AND key = 'network.rate.interfaces'
           ),
           (
               SELECT value_json
               FROM public.vps_rule_values
               WHERE client_id = p_client_id
                 AND key = 'traffic.selectors'
           )
    INTO interface_rule, rate_rule, traffic_rule;

    -- An absent rate rule deliberately selects nothing.  All three non-empty
    -- modes remain subordinate to the VPS interface admission boundary.
    IF rate_rule IS NULL THEN
        RETURN ROW(FALSE, ARRAY[]::TEXT[], ARRAY[]::TEXT[])
            ::public.telemetry_dashboard_network_selection;
    END IF;

    rate_mode := rate_rule ->> 'mode';
    CASE rate_mode
        WHEN 'all' THEN
            requested_all := TRUE;
        WHEN 'exact' THEN
            SELECT COALESCE(
                array_agg(
                    DISTINCT (selector ->> 'interface') COLLATE "C"
                    ORDER BY (selector ->> 'interface') COLLATE "C"
                ),
                ARRAY[]::TEXT[]
            )
            INTO requested_interfaces
            FROM jsonb_array_elements(
                COALESCE(rate_rule -> 'selectors', '[]'::JSONB)
            ) selector
            WHERE selector ->> 'source' = 'host'
              AND octet_length(selector ->> 'interface') BETWEEN 1 AND 128;
        WHEN 'reference' THEN
            traffic_mode := COALESCE(traffic_rule ->> 'mode', 'exact');
            IF traffic_mode = 'all' THEN
                requested_all := TRUE;
            ELSIF traffic_mode = 'exact' THEN
                SELECT COALESCE(
                    array_agg(
                        DISTINCT (selector ->> 'interface') COLLATE "C"
                        ORDER BY (selector ->> 'interface') COLLATE "C"
                    ),
                    ARRAY[]::TEXT[]
                )
                INTO requested_interfaces
                FROM jsonb_array_elements(
                    COALESCE(traffic_rule -> 'selectors', '[]'::JSONB)
                ) selector
                WHERE selector ->> 'source' = 'host'
                  AND octet_length(selector ->> 'interface') BETWEEN 1 AND 128;
            ELSE
                RAISE EXCEPTION
                    'invalid traffic selection mode for client %',
                    p_client_id;
            END IF;
        ELSE
            RAISE EXCEPTION
                'invalid network-rate selection mode for client %',
                p_client_id;
    END CASE;

    -- Absence is the product default: admit ordinary e*/w* host interfaces.
    -- Operators may instead admit every host interface or a canonical list of
    -- exact/trailing-star prefixes.  Prefixes use explicit left comparison;
    -- operator text is never interpreted as a SQL pattern.
    IF interface_rule IS NULL THEN
        admitted_patterns := ARRAY['e*', 'w*']::TEXT[];
    ELSE
        interface_mode := interface_rule ->> 'mode';
        IF interface_mode = 'all' THEN
            admitted_all := TRUE;
        ELSIF interface_mode = 'patterns' THEN
            SELECT COALESCE(
                array_agg(
                    DISTINCT pattern.value COLLATE "C"
                    ORDER BY pattern.value COLLATE "C"
                ),
                ARRAY[]::TEXT[]
            )
            INTO admitted_patterns
            FROM jsonb_array_elements_text(
                COALESCE(interface_rule -> 'patterns', '[]'::JSONB)
            ) pattern(value)
            WHERE octet_length(pattern.value) BETWEEN 1 AND 128;
        ELSE
            RAISE EXCEPTION
                'invalid network-interface admission mode for client %',
                p_client_id;
        END IF;
    END IF;

    IF requested_all AND admitted_all THEN
        RETURN ROW(TRUE, ARRAY[]::TEXT[], ARRAY[]::TEXT[])
            ::public.telemetry_dashboard_network_selection;
    END IF;

    -- Preserve wildcard admission as a compact predicate.  It is expanded
    -- only when a generation is rebuilt, never once per arriving point.
    IF requested_all THEN
        IF NOT public.telemetry_dashboard_interfaces_are_canonical(
            admitted_patterns
        ) THEN
            RAISE EXCEPTION
                'noncanonical network-interface admission for client %',
                p_client_id;
        END IF;
        RETURN ROW(FALSE, ARRAY[]::TEXT[], admitted_patterns)
            ::public.telemetry_dashboard_network_selection;
    ELSE
        SELECT COALESCE(
            array_agg(
                candidate.interface
                ORDER BY candidate.interface COLLATE "C"
            ),
            ARRAY[]::TEXT[]
        )
        INTO interfaces
        FROM (
            SELECT requested.interface COLLATE "C" AS interface
            FROM unnest(requested_interfaces) requested(interface)
            WHERE admitted_all
               OR EXISTS (
                    SELECT 1
                    FROM unnest(admitted_patterns) pattern(value)
                    WHERE (
                        right(pattern.value, 1) = '*'
                        AND left(
                            requested.interface,
                            length(pattern.value) - 1
                        ) = left(pattern.value, length(pattern.value) - 1)
                    ) OR (
                        right(pattern.value, 1) <> '*'
                        AND requested.interface = pattern.value
                    )
               )
        ) candidate;
    END IF;

    IF NOT public.telemetry_dashboard_interfaces_are_canonical(interfaces) THEN
        RAISE EXCEPTION
            'noncanonical network-rate interface selection for client %',
            p_client_id;
    END IF;

    RETURN ROW(FALSE, interfaces, ARRAY[]::TEXT[])
        ::public.telemetry_dashboard_network_selection;
END
$$;

-- One non-materialized authority owns the exact currently reported tunnel
-- identity used by tunnel-counter admission, operational readers, and
-- delivery validation. Default host-interface collision is independently
-- plan-owned below. The view stores no derived state: plan lifecycle changes
-- are visible in the same statement snapshot.
CREATE VIEW public.telemetry_current_tunnels AS
SELECT
    tunnel.*,
    current_plan.plan AS current_plan
FROM public.telemetry_tunnels tunnel
JOIN public.tunnel_plans current_plan
  ON current_plan.id = tunnel.telemetry_plan_id
 AND current_plan.deleted_at IS NULL
 AND current_plan.enabled
 AND current_plan.name = tunnel.telemetry_plan_name
 AND current_plan.kind = tunnel.kind
 AND current_plan.plan->>'interface_name' = tunnel.interface
 AND (
    (
        tunnel.telemetry_endpoint_side = 'left'
        AND current_plan.left_client_id = tunnel.client_id
        AND current_plan.right_client_id = tunnel.telemetry_peer_client_id
    )
    OR (
        tunnel.telemetry_endpoint_side = 'right'
        AND current_plan.right_client_id = tunnel.client_id
        AND current_plan.left_client_id = tunnel.telemetry_peer_client_id
    )
 )
WHERE octet_length(tunnel.kind) BETWEEN 1 AND 64
  AND octet_length(tunnel.telemetry_plan_name) BETWEEN 1 AND 128;

-- The hot-path admission model has two explicit stages. The traffic schema,
-- which owns vps_rule_values, installs the stable resolver after that table;
-- this telemetry-owned immutable predicate then evaluates any number of
-- candidate interfaces without consulting a relation. Nothing is cached or
-- persisted, so rule/plan commits become visible at the next statement.
CREATE FUNCTION public.telemetry_interface_is_admitted_resolved(
    p_admission_mode TEXT,
    p_interface_patterns TEXT[],
    p_managed_tunnel_interfaces TEXT[],
    p_source_kind TEXT,
    p_interface TEXT
)
RETURNS BOOLEAN
LANGUAGE plpgsql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
BEGIN
    IF p_source_kind NOT IN ('host', 'tunnel')
       OR octet_length(p_interface) NOT BETWEEN 1 AND 128 THEN
        RETURN FALSE;
    END IF;

    IF p_admission_mode = 'default_physical' THEN
        RETURN p_source_kind = 'host'
           AND left(p_interface, 1) IN ('e', 'w')
           AND NOT p_interface = ANY (p_managed_tunnel_interfaces);
    ELSIF p_admission_mode = 'all' THEN
        RETURN TRUE;
    ELSIF p_admission_mode = 'patterns' THEN
        RETURN EXISTS (
            SELECT 1
            FROM unnest(p_interface_patterns) pattern(value)
            WHERE (
                right(pattern.value, 1) = '*'
                AND left(p_interface, length(pattern.value) - 1) =
                    left(pattern.value, length(pattern.value) - 1)
            ) OR (
                right(pattern.value, 1) <> '*'
                AND p_interface = pattern.value
            )
        );
    END IF;

    RAISE EXCEPTION 'invalid resolved network-interface admission mode %',
        p_admission_mode;
END
$$;

-- Diagnostic traffic selection ignores accounting direction by design: once
-- an admitted stream identity is selected, both counter directions are
-- projected. An absent traffic.selectors rule selects no history. Exact rules
-- preserve identities even before their first sample; `all` expands only the
-- durable traffic stream registry at generation-build time.
CREATE FUNCTION public.telemetry_dashboard_effective_traffic_selection(
    p_client_id TEXT
)
RETURNS public.telemetry_dashboard_traffic_selection
LANGUAGE plpgsql
STABLE
AS $$
DECLARE
    traffic_rule JSONB;
    selection_mode TEXT;
    selected_source_kinds TEXT[] := ARRAY[]::TEXT[];
    selected_interfaces TEXT[] := ARRAY[]::TEXT[];
BEGIN
    SELECT rule.value_json
    INTO traffic_rule
    FROM public.vps_rule_values rule
    WHERE rule.client_id = p_client_id
      AND rule.key = 'traffic.selectors';

    IF NOT FOUND THEN
        RETURN ROW(ARRAY[]::TEXT[], ARRAY[]::TEXT[])
            ::public.telemetry_dashboard_traffic_selection;
    END IF;

    selection_mode := traffic_rule ->> 'mode';
    IF selection_mode = 'exact' THEN
        WITH policy AS MATERIALIZED (
            SELECT *
            FROM public.resolve_telemetry_interface_policies(
                ARRAY[p_client_id]
            )
        ), candidate AS MATERIALIZED (
            SELECT DISTINCT
                   (selector ->> 'source') COLLATE "C" AS source_kind,
                   (selector ->> 'interface') COLLATE "C" AS interface
            FROM jsonb_array_elements(
                COALESCE(traffic_rule -> 'selectors', '[]'::JSONB)
            ) selector
            WHERE selector ->> 'source' IN ('host', 'tunnel')
              AND octet_length(selector ->> 'interface') BETWEEN 1 AND 128
        ), selected AS MATERIALIZED (
            SELECT candidate.source_kind, candidate.interface
            FROM candidate
            CROSS JOIN policy
            WHERE public.telemetry_interface_is_admitted_resolved(
                policy.admission_mode,
                policy.interface_patterns,
                policy.managed_tunnel_interfaces,
                candidate.source_kind,
                candidate.interface
            )
        )
        SELECT COALESCE(
                   array_agg(
                       selected.source_kind
                       ORDER BY selected.source_kind COLLATE "C",
                                selected.interface COLLATE "C"
                   ),
                   ARRAY[]::TEXT[]
               ),
               COALESCE(
                   array_agg(
                       selected.interface
                       ORDER BY selected.source_kind COLLATE "C",
                                selected.interface COLLATE "C"
                   ),
                   ARRAY[]::TEXT[]
               )
        INTO selected_source_kinds, selected_interfaces
        FROM selected;
    ELSIF selection_mode = 'all' THEN
        WITH policy AS MATERIALIZED (
            SELECT *
            FROM public.resolve_telemetry_interface_policies(
                ARRAY[p_client_id]
            )
        ), selected AS MATERIALIZED (
            SELECT stream.source_kind COLLATE "C" AS source_kind,
                   stream.interface COLLATE "C" AS interface
            FROM public.traffic_counter_streams stream
            CROSS JOIN policy
            WHERE stream.client_id = p_client_id
              AND public.telemetry_interface_is_admitted_resolved(
                  policy.admission_mode,
                  policy.interface_patterns,
                  policy.managed_tunnel_interfaces,
                  stream.source_kind,
                  stream.interface
              )
        )
        SELECT COALESCE(
                   array_agg(
                       selected.source_kind
                       ORDER BY selected.source_kind COLLATE "C",
                                selected.interface COLLATE "C"
                   ),
                   ARRAY[]::TEXT[]
               ),
               COALESCE(
                   array_agg(
                       selected.interface
                       ORDER BY selected.source_kind COLLATE "C",
                                selected.interface COLLATE "C"
                   ),
                   ARRAY[]::TEXT[]
               )
        INTO selected_source_kinds, selected_interfaces
        FROM selected;
    ELSE
        RAISE EXCEPTION 'invalid traffic selection mode for client %',
            p_client_id;
    END IF;

    IF NOT public.telemetry_dashboard_traffic_identities_are_canonical(
        selected_source_kinds, selected_interfaces
    ) THEN
        RAISE EXCEPTION 'noncanonical traffic selection for client %',
            p_client_id;
    END IF;

    RETURN ROW(selected_source_kinds, selected_interfaces)
        ::public.telemetry_dashboard_traffic_selection;
END
$$;

CREATE FUNCTION public.telemetry_dashboard_network_interface_selected_resolved(
    p_admission_mode TEXT,
    p_interface_patterns TEXT[],
    p_managed_tunnel_interfaces TEXT[],
    p_selection public.telemetry_dashboard_network_selection,
    p_interface TEXT
)
RETURNS BOOLEAN
LANGUAGE sql
IMMUTABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT public.telemetry_interface_is_admitted_resolved(
               p_admission_mode,
               p_interface_patterns,
               p_managed_tunnel_interfaces,
               'host',
               p_interface
           )
       AND (
            (p_selection).select_all
            OR p_interface = ANY ((p_selection).interfaces)
            OR EXISTS (
                SELECT 1
                FROM unnest((p_selection).patterns) pattern(value)
                WHERE (
                    right(pattern.value, 1) = '*'
                    AND left(p_interface, length(pattern.value) - 1) =
                        left(pattern.value, length(pattern.value) - 1)
                ) OR (
                    right(pattern.value, 1) <> '*'
                    AND p_interface = pattern.value
                )
            )
       )
$$;

CREATE FUNCTION public.telemetry_dashboard_network_interface_selected(
    p_client_id TEXT,
    p_selection public.telemetry_dashboard_network_selection,
    p_interface TEXT
)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
STRICT
PARALLEL SAFE
AS $$
    SELECT public.telemetry_dashboard_network_interface_selected_resolved(
        policy.admission_mode,
        policy.interface_patterns,
        policy.managed_tunnel_interfaces,
        p_selection,
        p_interface
    )
    FROM public.resolve_telemetry_interface_policies(ARRAY[p_client_id]) policy
$$;

CREATE FUNCTION public.telemetry_dashboard_generation_interfaces(
    p_client_id TEXT,
    p_selection public.telemetry_dashboard_network_selection
)
RETURNS TEXT[]
LANGUAGE sql
STABLE
STRICT
AS $$
    WITH policy AS MATERIALIZED (
        SELECT *
        FROM public.resolve_telemetry_interface_policies(ARRAY[p_client_id])
    ), candidate_interfaces AS MATERIALIZED (
        -- Durable stream identity is the compact discovery owner. Retained
        -- existence is one index-stopping probe per stream; the two transient
        -- owners cover closed-but-unpromoted and projected open minutes.
        SELECT stream.interface COLLATE "C" AS interface
        FROM public.traffic_counter_streams stream
        WHERE stream.client_id = p_client_id
          AND stream.source_kind = 'host'
          AND (
              stream.first_unpromoted_observed_at IS NOT NULL
              OR EXISTS (
                  SELECT 1
                  FROM public.telemetry_network_rates retained
                  WHERE retained.client_id = stream.client_id
                    AND retained.interface = stream.interface
                  LIMIT 1
              )
          )

        UNION

        SELECT suffix.interface COLLATE "C"
        FROM public.telemetry_projected_raw_network_minutes_source(
            ARRAY[p_client_id]
        ) suffix
        WHERE suffix.client_id = p_client_id
    )
    SELECT CASE
        WHEN (p_selection).select_all
          OR cardinality((p_selection).patterns) > 0 THEN COALESCE(
            (
                SELECT array_agg(
                    candidate.interface
                    ORDER BY candidate.interface COLLATE "C"
                )
                FROM (
                    SELECT candidate.interface
                    FROM candidate_interfaces candidate
                    CROSS JOIN policy
                    WHERE public.telemetry_dashboard_network_interface_selected_resolved(
                          policy.admission_mode,
                          policy.interface_patterns,
                          policy.managed_tunnel_interfaces,
                          p_selection,
                          candidate.interface
                      )
                ) candidate
            ),
            ARRAY[]::TEXT[]
        )
        ELSE COALESCE(
            (
                SELECT array_agg(
                    candidate.interface
                    ORDER BY candidate.interface COLLATE "C"
                )
                FROM policy
                CROSS JOIN unnest((p_selection).interfaces)
                    candidate(interface)
                WHERE public.telemetry_dashboard_network_interface_selected_resolved(
                    policy.admission_mode,
                    policy.interface_patterns,
                    policy.managed_tunnel_interfaces,
                    p_selection,
                    candidate.interface
                )
            ),
            ARRAY[]::TEXT[]
        )
    END
$$;

CREATE FUNCTION public.telemetry_dashboard_event_queued_at()
RETURNS TIMESTAMPTZ
LANGUAGE sql
STABLE
AS $$
    SELECT COALESCE(
        NULLIF(
            current_setting('vpsman.telemetry_accepted_at', TRUE),
            ''
        )::TIMESTAMPTZ,
        statement_timestamp()
    )
$$;

CREATE FUNCTION public.telemetry_dashboard_full_block_requested()
RETURNS BOOLEAN
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT current_setting(
        'vpsman.telemetry_history_compaction', TRUE
    ) = 'on'
$$;

-- Moving one logical point from its active owner to retained history is a
-- physical ownership change. Retained dashboard triggers use this marker to
-- skip membership work, but still queue the one closed coordinate that hands
-- the ended minute from the resident live overlay to its retained F16 block.
-- Core retained-history due-event producers remain independent.
CREATE FUNCTION public.telemetry_dashboard_ownership_transfer_requested()
RETURNS BOOLEAN
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT COALESCE(
        current_setting(
            'vpsman.telemetry_ownership_transfer', TRUE
        ) = 'on',
        FALSE
    )
$$;

-- Day-one traffic retention moves one already-closed network minute between
-- retained physical owners. Its logical coordinate and value do not change,
-- so neither membership nor dashboard block publication changes.
CREATE FUNCTION public.telemetry_dashboard_retained_transfer_requested()
RETURNS BOOLEAN
LANGUAGE sql
STABLE
PARALLEL SAFE
AS $$
    SELECT COALESCE(
        current_setting(
            'vpsman.telemetry_retained_ownership_transfer', TRUE
        ) = 'on',
        FALSE
    )
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_generation(
    p_client_id TEXT,
    p_domain TEXT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF p_domain NOT IN ('resource', 'network', 'traffic') THEN
        RAISE EXCEPTION 'invalid dashboard generation event';
    END IF;

    -- The dashboard root owns this queue. During a client cascade, dependent
    -- rule and tunnel deletes can still call this function after that root is
    -- gone; such a client has neither a visible projection nor a consumer.
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT dashboard_client.client_id,
           p_domain,
           public.telemetry_dashboard_event_queued_at()
    FROM public.telemetry_dashboard_clients dashboard_client
    WHERE dashboard_client.client_id = p_client_id;
END
$$;

-- A value-only source update can commit while an older repeatable-read
-- generation build is in flight. If the identity is desired but not in the
-- published generation, append a later immutable generation event so the
-- older capture cannot publish and acknowledge away the newer value.
CREATE FUNCTION public.queue_telemetry_dashboard_network_pending_generation_catchup(
    p_client_ids TEXT[],
    p_interfaces TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF cardinality(p_client_ids) <> cardinality(p_interfaces) THEN
        RAISE EXCEPTION 'misaligned network generation catch-up identities';
    END IF;

    WITH changed AS MATERIALIZED (
        SELECT DISTINCT identity.client_id, identity.interface
        FROM unnest(p_client_ids, p_interfaces)
            identity(client_id, interface)
        WHERE identity.client_id IS NOT NULL
          AND octet_length(identity.interface) BETWEEN 1 AND 128
    ), candidate AS MATERIALIZED (
        SELECT changed.*
        FROM changed
        JOIN public.telemetry_dashboard_network_projection_heads head
          ON head.client_id = changed.client_id
         AND NOT changed.interface = ANY(
             head.network_generation_interfaces
         )
        WHERE EXISTS (
            SELECT 1
            FROM public.telemetry_dashboard_generation_events event
            WHERE event.client_id = changed.client_id
              AND event.domain = 'network'
        )
    ), owners AS MATERIALIZED (
        SELECT DISTINCT candidate.client_id FROM candidate
    ), selections AS MATERIALIZED (
        SELECT owner.client_id,
               public.telemetry_dashboard_effective_network_selection(
                   owner.client_id
               ) AS selection
        FROM owners owner
    ), policies AS MATERIALIZED (
        SELECT policy.*
        FROM public.resolve_telemetry_interface_policies(ARRAY(
            SELECT owner.client_id
            FROM owners owner
            ORDER BY owner.client_id
        )) policy
    )
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT candidate.client_id,
           'network',
           public.telemetry_dashboard_event_queued_at()
    FROM candidate
    JOIN selections USING (client_id)
    JOIN policies USING (client_id)
    WHERE public.telemetry_dashboard_network_interface_selected_resolved(
        policies.admission_mode,
        policies.interface_patterns,
        policies.managed_tunnel_interfaces,
        selections.selection,
        candidate.interface
    );
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_pending_generation_catchup(
    p_client_ids TEXT[],
    p_source_kinds TEXT[],
    p_interfaces TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF cardinality(p_client_ids) <> cardinality(p_source_kinds)
       OR cardinality(p_source_kinds) <> cardinality(p_interfaces) THEN
        RAISE EXCEPTION 'misaligned traffic generation catch-up identities';
    END IF;

    WITH changed AS MATERIALIZED (
        SELECT DISTINCT identity.client_id,
               identity.source_kind COLLATE "C" AS source_kind,
               identity.interface COLLATE "C" AS interface
        FROM unnest(p_client_ids, p_source_kinds, p_interfaces)
            identity(client_id, source_kind, interface)
        WHERE identity.client_id IS NOT NULL
          AND identity.source_kind IN ('host', 'tunnel')
          AND octet_length(identity.interface) BETWEEN 1 AND 128
    ), candidate AS MATERIALIZED (
        SELECT changed.*
        FROM changed
        JOIN public.telemetry_dashboard_traffic_projection_heads head
          ON head.client_id = changed.client_id
         AND NOT public.telemetry_dashboard_traffic_identity_is_selected(
             head.traffic_generation_source_kinds,
             head.traffic_generation_interfaces,
             changed.source_kind,
             changed.interface
         )
        WHERE EXISTS (
            SELECT 1
            FROM public.telemetry_dashboard_generation_events event
            WHERE event.client_id = changed.client_id
              AND event.domain = 'traffic'
        )
    ), owners AS MATERIALIZED (
        SELECT DISTINCT candidate.client_id FROM candidate
    ), selections AS MATERIALIZED (
        SELECT owner.client_id,
               public.telemetry_dashboard_effective_traffic_selection(
                   owner.client_id
               ) AS selection
        FROM owners owner
    )
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT candidate.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM candidate
    JOIN selections USING (client_id)
    WHERE public.telemetry_dashboard_traffic_identity_is_selected(
        (selections.selection).source_kinds,
        (selections.selection).interfaces,
        candidate.source_kind,
        candidate.interface
    );
END
$$;

-- Queue each physical traffic coordinate plus only physically present higher
-- rollups whose visibility can change with it. Coarse source ownership is
-- origin-local: an exact live row cannot shadow a vnstat-import rollup, and a
-- finer live rollup cannot shadow a vnstat-import rollup. Fixed-tier aligned
-- primary-key probes therefore provide complete dependency closure without
-- expanding every live minute to every possible tier.
CREATE FUNCTION public.queue_telemetry_dashboard_traffic_coordinates(
    p_client_ids TEXT[],
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_origin_kinds TEXT[],
    p_bucket_starts TIMESTAMPTZ[],
    p_native_bucket_secs INTEGER[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF cardinality(p_client_ids) <> cardinality(p_source_kinds)
       OR cardinality(p_source_kinds) <> cardinality(p_interfaces)
       OR cardinality(p_interfaces) <> cardinality(p_origin_kinds)
       OR cardinality(p_origin_kinds) <> cardinality(p_bucket_starts)
       OR cardinality(p_bucket_starts) <>
            cardinality(p_native_bucket_secs) THEN
        RAISE EXCEPTION 'misaligned traffic dashboard coordinates';
    END IF;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    WITH changed AS MATERIALIZED (
        SELECT DISTINCT item.client_id,
               item.source_kind,
               item.interface,
               item.origin_kind,
               item.bucket_start,
               item.native_bucket_secs
        FROM unnest(
            p_client_ids, p_source_kinds, p_interfaces,
            p_origin_kinds, p_bucket_starts, p_native_bucket_secs
        ) item(
            client_id, source_kind, interface,
            origin_kind, bucket_start, native_bucket_secs
        )
        WHERE item.client_id IS NOT NULL
          AND item.source_kind IN ('host', 'tunnel')
          AND item.origin_kind IN ('live', 'vnstat_import')
          AND octet_length(item.interface) BETWEEN 1 AND 128
          AND public.telemetry_dashboard_traffic_source_tier_is_valid(
              item.native_bucket_secs
          )
          AND extract(epoch FROM item.bucket_start)::BIGINT
                % item.native_bucket_secs = 0
    ), selected AS MATERIALIZED (
        SELECT changed.*
        FROM changed
        JOIN public.telemetry_dashboard_clients dashboard_client
          ON dashboard_client.client_id = changed.client_id
        JOIN public.telemetry_dashboard_traffic_projection_heads head
          ON head.client_id = changed.client_id
         AND public.telemetry_dashboard_traffic_identity_is_selected(
             head.traffic_generation_source_kinds,
             head.traffic_generation_interfaces,
             changed.source_kind,
             changed.interface
         )
    ), coordinate AS MATERIALIZED (
        SELECT DISTINCT selected.client_id,
               selected.native_bucket_secs AS source_bucket_secs,
               extract(epoch FROM selected.bucket_start)::BIGINT
                   AS bucket_start_unix
        FROM selected

        UNION

        SELECT selected.client_id,
               rollup.bucket_secs AS source_bucket_secs,
               extract(epoch FROM rollup.bucket_start)::BIGINT
                   AS bucket_start_unix
        FROM selected
        CROSS JOIN unnest(
            ARRAY[3600, 10800, 21600, 86400]::INTEGER[]
        ) tier(bucket_secs)
        JOIN public.traffic_counter_rollups rollup
          ON rollup.client_id = selected.client_id
         AND rollup.source_kind = selected.source_kind
         AND rollup.interface = selected.interface
         AND rollup.origin_kind = selected.origin_kind
         AND rollup.bucket_secs = tier.bucket_secs
         AND rollup.bucket_start = to_timestamp(
             floor(
                 extract(epoch FROM selected.bucket_start)::NUMERIC
                 / tier.bucket_secs
             )::BIGINT * tier.bucket_secs::BIGINT
         )
        WHERE tier.bucket_secs > selected.native_bucket_secs
          AND mod(tier.bucket_secs, selected.native_bucket_secs) = 0
    )
    SELECT DISTINCT coordinate.client_id,
           'traffic',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           coordinate.source_bucket_secs,
           public.telemetry_dashboard_block_start(
               coordinate.bucket_start_unix,
               coordinate.source_bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE coordinate.bucket_start_unix END,
           public.telemetry_dashboard_event_queued_at()
    FROM coordinate;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_change(
    p_client_id TEXT,
    p_interfaces TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    selection public.telemetry_dashboard_network_selection;
    selected_interfaces TEXT[];
    head_interfaces TEXT[];
BEGIN
    IF cardinality(p_interfaces) = 0
       OR NOT public.telemetry_dashboard_interfaces_are_canonical(
            p_interfaces
       ) THEN
        RAISE EXCEPTION 'invalid dashboard network interface set';
    END IF;

    SELECT head.network_generation_interfaces
    INTO head_interfaces
    FROM public.telemetry_dashboard_network_projection_heads head
    WHERE head.client_id = p_client_id;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    -- The projector already supplies the canonical admitted host-interface
    -- set. Existing generation membership is therefore a complete steady-state
    -- receipt; only a genuinely new member needs selector evaluation. Rule and
    -- tunnel-plan mutations independently rebuild the full generation.
    IF p_interfaces <@ head_interfaces THEN
        RETURN;
    END IF;

    selection :=
        public.telemetry_dashboard_effective_network_selection(p_client_id);
    SELECT COALESCE(
        array_agg(
            changed.interface
            ORDER BY changed.interface COLLATE "C"
        ),
        ARRAY[]::TEXT[]
    )
    INTO selected_interfaces
    FROM unnest(p_interfaces) changed(interface)
    WHERE public.telemetry_dashboard_network_interface_selected(
        p_client_id, selection, changed.interface
    );

    IF cardinality(selected_interfaces) = 0 THEN
        RETURN;
    END IF;

    IF NOT selected_interfaces <@ head_interfaces THEN
        -- The current selector owns membership, including compact admission
        -- patterns. One newly selected interface makes a full generation the
        -- natural work unit; that rebuild includes every selected interface.
        PERFORM public.queue_telemetry_dashboard_generation(
            p_client_id, 'network'
        );
    END IF;
END
$$;

-- Deletion only changes wildcard/pattern membership when the last exact point
-- for a published interface disappears. The producer performs indexed identity
-- probes; the generation consumer remains the sole retained-history scanner.
CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_removal(
    p_client_id TEXT,
    p_interfaces TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    selection public.telemetry_dashboard_network_selection;
    head_interfaces TEXT[];
BEGIN
    IF cardinality(p_interfaces) = 0
       OR NOT public.telemetry_dashboard_interfaces_are_canonical(
            p_interfaces
       ) THEN
        RAISE EXCEPTION 'invalid dashboard network interface set';
    END IF;

    SELECT head.network_generation_interfaces
    INTO head_interfaces
    FROM public.telemetry_dashboard_network_projection_heads head
    WHERE head.client_id = p_client_id;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    selection :=
        public.telemetry_dashboard_effective_network_selection(p_client_id);
    IF EXISTS (
        SELECT 1
        FROM public.telemetry_dashboard_generation_events event
        WHERE event.client_id = p_client_id
          AND event.domain = 'network'
        LIMIT 1
    ) AND EXISTS (
        SELECT 1
        FROM unnest(p_interfaces) changed(interface)
        WHERE public.telemetry_dashboard_network_interface_selected(
            p_client_id, selection, changed.interface
        )
    ) THEN
        -- A publisher may already hold a snapshot from before this removal.
        -- A later exact-client event is the durable catch-up fence.
        PERFORM public.queue_telemetry_dashboard_generation(
            p_client_id, 'network'
        );
        RETURN;
    END IF;

    IF NOT (p_interfaces && head_interfaces) THEN
        RETURN;
    END IF;

    IF NOT (selection).select_all
       AND cardinality((selection).patterns) = 0 THEN
        -- Explicit interface selections remain generation members even while
        -- they have no retained point.
        RETURN;
    END IF;

    IF EXISTS (
        WITH projected_suffix_interfaces AS MATERIALIZED (
            SELECT DISTINCT suffix.interface
            FROM public.telemetry_projected_raw_network_minutes_source(
                ARRAY[p_client_id]
            ) suffix
            WHERE suffix.client_id = p_client_id
        )
        SELECT 1
        FROM unnest(p_interfaces) changed(interface)
        WHERE changed.interface = ANY(head_interfaces)
          AND public.telemetry_dashboard_network_interface_selected(
              p_client_id, selection, changed.interface
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.telemetry_network_rates retained
              WHERE retained.client_id = p_client_id
                AND retained.interface = changed.interface
              LIMIT 1
          )
          AND NOT EXISTS (
              SELECT 1
              FROM public.traffic_counter_streams stream
              WHERE stream.client_id = p_client_id
                AND stream.source_kind = 'host'
                AND stream.interface = changed.interface
                AND stream.first_unpromoted_observed_at IS NOT NULL
          )
          AND NOT EXISTS (
              SELECT 1
              FROM projected_suffix_interfaces suffix
              WHERE suffix.interface = changed.interface
          )
    ) THEN
        PERFORM public.queue_telemetry_dashboard_generation(
            p_client_id, 'network'
        );
    END IF;
END
$$;

CREATE FUNCTION public.initialize_telemetry_dashboard_client()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_clients (client_id)
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_resource_projection_heads (
        client_id
    )
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_network_generations (
        client_id, generation, select_all, interfaces, interface_width
    )
    VALUES (NEW.id, 1, FALSE, ARRAY[]::TEXT[], 0)
    ON CONFLICT (client_id, generation) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_network_projection_heads (
        client_id
    )
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_traffic_generations (
        client_id, generation, source_kinds, interfaces, stream_width
    )
    VALUES (
        NEW.id, 1, ARRAY[]::TEXT[], ARRAY[]::TEXT[], 0
    )
    ON CONFLICT (client_id, generation) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_traffic_projection_heads (
        client_id
    )
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_ping_projection_heads (
        client_id
    )
    VALUES (NEW.id)
    ON CONFLICT (client_id) DO NOTHING;

    INSERT INTO public.telemetry_dashboard_projection_fences (
        client_id, domain
    )
    VALUES (NEW.id, 'resource'),
           (NEW.id, 'network'),
           (NEW.id, 'traffic')
    ON CONFLICT (client_id, domain) DO NOTHING;

    -- NOTIFY is delivered only if this transaction commits, after both
    -- ready-empty owners and their fences are visible to the listener.
    PERFORM pg_notify(
        'vpsman_telemetry_projection',
        jsonb_build_object(
            'owner', 'dashboard',
            'domain', 'client',
            'change', 'initialize',
            'client_id', NEW.id
        )::TEXT
    );

    RETURN NULL;
END
$$;

CREATE TRIGGER clients_telemetry_dashboard_initialize
AFTER INSERT ON public.clients
FOR EACH ROW
EXECUTE FUNCTION public.initialize_telemetry_dashboard_client();

CREATE FUNCTION public.remove_telemetry_dashboard_client()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    -- The event and cascading delete share one commit boundary.  A listener
    -- therefore removes the owner only after it can no longer reload it.
    PERFORM pg_notify(
        'vpsman_telemetry_projection',
        jsonb_build_object(
            'owner', 'dashboard',
            'domain', 'client',
            'change', 'remove',
            'client_id', OLD.id
        )::TEXT
    );
    RETURN OLD;
END
$$;

CREATE TRIGGER clients_telemetry_dashboard_remove
BEFORE DELETE ON public.clients
FOR EACH ROW
EXECUTE FUNCTION public.remove_telemetry_dashboard_client();

CREATE FUNCTION public.refresh_telemetry_dashboard_network_selection(
    p_client_id TEXT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    -- Rule and tunnel-plan mutations are already known invalidations. They only
    -- enqueue exact-client work; the generation consumer resolves membership
    -- from retained history outside the producer transaction.
    IF EXISTS (
        SELECT 1
        FROM public.telemetry_dashboard_network_projection_heads head
        WHERE head.client_id = p_client_id
    ) THEN
        PERFORM public.queue_telemetry_dashboard_generation(
            p_client_id, 'network'
        );
    END IF;
END
$$;

CREATE FUNCTION public.sync_telemetry_network_selection_rule()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected_client_id TEXT := COALESCE(NEW.client_id, OLD.client_id);
    affected_key TEXT := COALESCE(NEW.key, OLD.key);
BEGIN
    IF affected_key IN (
        'network.interfaces', 'network.rate.interfaces', 'traffic.selectors'
    ) THEN
        PERFORM public.refresh_telemetry_dashboard_network_selection(
            affected_client_id
        );
    END IF;
    RETURN NULL;
END
$$;

CREATE TRIGGER vps_rule_values_telemetry_network_selection_sync
AFTER INSERT OR DELETE OR UPDATE ON public.vps_rule_values
FOR EACH ROW EXECUTE FUNCTION public.sync_telemetry_network_selection_rule();

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_rules_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT rule.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM new_telemetry_dashboard_traffic_rules rule
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = rule.client_id
    WHERE rule.key IN ('traffic.selectors', 'network.interfaces');
    RETURN NULL;
END
$$;

-- A client cascade may expose OLD transition rows after the immutable
-- dashboard root has gone. Deletion producers queue only for a still-live
-- root; otherwise no consumer or visible dashboard state remains.
CREATE FUNCTION public.queue_telemetry_dashboard_traffic_rules_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT rule.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM old_telemetry_dashboard_traffic_rules rule
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = rule.client_id
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = rule.client_id
    WHERE rule.key IN ('traffic.selectors', 'network.interfaces');
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_rules_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT changed.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM (
        SELECT client_id, key
        FROM old_telemetry_dashboard_traffic_rules
        UNION
        SELECT client_id, key
        FROM new_telemetry_dashboard_traffic_rules
    ) changed
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = changed.client_id
    WHERE changed.key IN ('traffic.selectors', 'network.interfaces');
    RETURN NULL;
END
$$;

CREATE TRIGGER vps_rule_values_dashboard_traffic_after_insert
AFTER INSERT ON public.vps_rule_values
REFERENCING NEW TABLE AS new_telemetry_dashboard_traffic_rules
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_rules_after_insert();

CREATE TRIGGER vps_rule_values_dashboard_traffic_after_delete
AFTER DELETE ON public.vps_rule_values
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_rules
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_rules_after_delete();

CREATE TRIGGER vps_rule_values_dashboard_traffic_after_update
AFTER UPDATE ON public.vps_rule_values
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_rules
            NEW TABLE AS new_telemetry_dashboard_traffic_rules
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_rules_after_update();

-- Plan identity and lifecycle exclusively own default managed-interface
-- admission. Rare plan insert/update/delete transitions invalidate only their
-- endpoint clients; tunnel telemetry writes do no selection-generation work.
CREATE FUNCTION public.sync_telemetry_network_selection_after_plan_change()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected_client_id TEXT;
BEGIN
    FOR affected_client_id IN
        SELECT DISTINCT endpoint.client_id
        FROM unnest(ARRAY[
            CASE WHEN TG_OP <> 'INSERT' THEN
                CASE WHEN OLD.enabled IS TRUE AND OLD.deleted_at IS NULL
                    THEN OLD.left_client_id END
            END,
            CASE WHEN TG_OP <> 'INSERT' THEN
                CASE WHEN OLD.enabled IS TRUE AND OLD.deleted_at IS NULL
                    THEN OLD.right_client_id END
            END,
            CASE WHEN TG_OP <> 'DELETE' THEN
                CASE WHEN NEW.enabled IS TRUE AND NEW.deleted_at IS NULL
                    THEN NEW.left_client_id END
            END,
            CASE WHEN TG_OP <> 'DELETE' THEN
                CASE WHEN NEW.enabled IS TRUE AND NEW.deleted_at IS NULL
                    THEN NEW.right_client_id END
            END
        ]) endpoint(client_id)
        WHERE endpoint.client_id IS NOT NULL
    LOOP
        PERFORM public.refresh_telemetry_dashboard_network_selection(
            affected_client_id
        );
    END LOOP;
    RETURN NULL;
END
$$;

CREATE TRIGGER tunnel_plans_dashboard_selection_after_insert
AFTER INSERT ON public.tunnel_plans
FOR EACH ROW
WHEN (NEW.enabled IS TRUE AND NEW.deleted_at IS NULL)
EXECUTE FUNCTION public.sync_telemetry_network_selection_after_plan_change();

CREATE TRIGGER tunnel_plans_dashboard_selection_after_managed_interface_update
AFTER UPDATE OF
    enabled, left_client_id, right_client_id, plan, deleted_at
ON public.tunnel_plans
FOR EACH ROW
WHEN (
    ROW(
        OLD.enabled IS TRUE AND OLD.deleted_at IS NULL,
        CASE WHEN OLD.enabled IS TRUE AND OLD.deleted_at IS NULL
            THEN OLD.left_client_id END,
        CASE WHEN OLD.enabled IS TRUE AND OLD.deleted_at IS NULL
            THEN OLD.right_client_id END,
        CASE WHEN OLD.enabled IS TRUE AND OLD.deleted_at IS NULL
            THEN OLD.plan ->> 'interface_name' END
    ) IS DISTINCT FROM ROW(
        NEW.enabled IS TRUE AND NEW.deleted_at IS NULL,
        CASE WHEN NEW.enabled IS TRUE AND NEW.deleted_at IS NULL
            THEN NEW.left_client_id END,
        CASE WHEN NEW.enabled IS TRUE AND NEW.deleted_at IS NULL
            THEN NEW.right_client_id END,
        CASE WHEN NEW.enabled IS TRUE AND NEW.deleted_at IS NULL
            THEN NEW.plan ->> 'interface_name' END
    )
)
EXECUTE FUNCTION public.sync_telemetry_network_selection_after_plan_change();

CREATE TRIGGER tunnel_plans_dashboard_selection_after_delete
AFTER DELETE ON public.tunnel_plans
FOR EACH ROW
WHEN (OLD.enabled IS TRUE AND OLD.deleted_at IS NULL)
EXECUTE FUNCTION public.sync_telemetry_network_selection_after_plan_change();

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_plans_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT endpoint.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM new_telemetry_dashboard_traffic_plans plan
    CROSS JOIN LATERAL unnest(ARRAY[
        plan.left_client_id, plan.right_client_id
    ]) endpoint(client_id)
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = endpoint.client_id
    WHERE plan.enabled IS TRUE
      AND plan.deleted_at IS NULL;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_plans_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT endpoint.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM old_telemetry_dashboard_traffic_plans plan
    CROSS JOIN LATERAL unnest(ARRAY[
        plan.left_client_id, plan.right_client_id
    ]) endpoint(client_id)
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = endpoint.client_id
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = endpoint.client_id
    WHERE plan.enabled IS TRUE
      AND plan.deleted_at IS NULL;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_plans_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    WITH old_identity AS MATERIALIZED (
        SELECT plan.id,
               endpoint.side,
               endpoint.client_id,
               plan.plan ->> 'interface_name' AS interface
        FROM old_telemetry_dashboard_traffic_plans plan
        CROSS JOIN LATERAL (VALUES
            ('left'::TEXT, plan.left_client_id),
            ('right'::TEXT, plan.right_client_id)
        ) endpoint(side, client_id)
        WHERE plan.enabled IS TRUE
          AND plan.deleted_at IS NULL
    ), new_identity AS MATERIALIZED (
        SELECT plan.id,
               endpoint.side,
               endpoint.client_id,
               plan.plan ->> 'interface_name' AS interface
        FROM new_telemetry_dashboard_traffic_plans plan
        CROSS JOIN LATERAL (VALUES
            ('left'::TEXT, plan.left_client_id),
            ('right'::TEXT, plan.right_client_id)
        ) endpoint(side, client_id)
        WHERE plan.enabled IS TRUE
          AND plan.deleted_at IS NULL
    ), changed AS MATERIALIZED (
        (SELECT * FROM old_identity EXCEPT SELECT * FROM new_identity)
        UNION
        (SELECT * FROM new_identity EXCEPT SELECT * FROM old_identity)
    )
    SELECT DISTINCT changed.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM changed
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = changed.client_id;
    RETURN NULL;
END
$$;

CREATE TRIGGER tunnel_plans_dashboard_traffic_after_insert
AFTER INSERT ON public.tunnel_plans
REFERENCING NEW TABLE AS new_telemetry_dashboard_traffic_plans
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_plans_after_insert();

CREATE TRIGGER tunnel_plans_dashboard_traffic_after_delete
AFTER DELETE ON public.tunnel_plans
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_plans
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_plans_after_delete();

CREATE TRIGGER tunnel_plans_dashboard_traffic_after_update
AFTER UPDATE ON public.tunnel_plans
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_plans
            NEW TABLE AS new_telemetry_dashboard_traffic_plans
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_plans_after_update();

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_streams_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT stream.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM new_telemetry_dashboard_traffic_streams stream
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = stream.client_id;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_streams_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT stream.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM old_telemetry_dashboard_traffic_streams stream
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = stream.client_id
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = stream.client_id;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_streams_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    WITH old_identity AS MATERIALIZED (
        SELECT client_id, source_kind, interface
        FROM old_telemetry_dashboard_traffic_streams
    ), new_identity AS MATERIALIZED (
        SELECT client_id, source_kind, interface
        FROM new_telemetry_dashboard_traffic_streams
    ), changed AS MATERIALIZED (
        (SELECT * FROM old_identity EXCEPT SELECT * FROM new_identity)
        UNION
        (SELECT * FROM new_identity EXCEPT SELECT * FROM old_identity)
    )
    SELECT DISTINCT changed.client_id,
           'traffic',
           public.telemetry_dashboard_event_queued_at()
    FROM changed
    JOIN public.telemetry_dashboard_traffic_projection_heads head
      ON head.client_id = changed.client_id;
    RETURN NULL;
END
$$;

CREATE TRIGGER traffic_counter_streams_dashboard_after_insert
AFTER INSERT ON public.traffic_counter_streams
REFERENCING NEW TABLE AS new_telemetry_dashboard_traffic_streams
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_streams_after_insert();

CREATE TRIGGER traffic_counter_streams_dashboard_after_delete
AFTER DELETE ON public.traffic_counter_streams
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_streams
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_streams_after_delete();

CREATE TRIGGER traffic_counter_streams_dashboard_after_update
AFTER UPDATE ON public.traffic_counter_streams
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_streams
            NEW TABLE AS new_telemetry_dashboard_traffic_streams
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_streams_after_update();

CREATE FUNCTION public.queue_telemetry_resource_blocks_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT rows.client_id,
           'resource',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           rows.bucket_secs,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM rows.bucket_start)::BIGINT,
               rows.bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE
                   extract(epoch FROM rows.bucket_start)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM new_telemetry_rollups rows;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_resource_blocks_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT rows.client_id,
           'resource',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           rows.bucket_secs,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM rows.bucket_start)::BIGINT,
               rows.bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE
                   extract(epoch FROM rows.bucket_start)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM old_telemetry_rollups rows
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = rows.client_id;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_resource_blocks_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT changed.client_id,
           'resource',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           changed.bucket_secs,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM changed.bucket_start)::BIGINT,
               changed.bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE
                   extract(epoch FROM changed.bucket_start)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM (
        SELECT client_id, bucket_secs, bucket_start
        FROM old_telemetry_rollups
        UNION
        SELECT client_id, bucket_secs, bucket_start
        FROM new_telemetry_rollups
    ) changed;
    RETURN NULL;
END
$$;

CREATE TRIGGER telemetry_rollups_dashboard_after_insert
AFTER INSERT ON public.telemetry_rollups
REFERENCING NEW TABLE AS new_telemetry_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_resource_blocks_after_insert();

CREATE TRIGGER telemetry_rollups_dashboard_after_delete
AFTER DELETE ON public.telemetry_rollups
REFERENCING OLD TABLE AS old_telemetry_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_resource_blocks_after_delete();

CREATE TRIGGER telemetry_rollups_dashboard_after_update
AFTER UPDATE ON public.telemetry_rollups
REFERENCING OLD TABLE AS old_telemetry_rollups
            NEW TABLE AS new_telemetry_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_resource_blocks_after_update();

CREATE FUNCTION public.queue_telemetry_network_blocks_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected RECORD;
BEGIN
    IF public.telemetry_dashboard_retained_transfer_requested() THEN
        RETURN NULL;
    END IF;
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        -- A close transaction already established selector membership while
        -- the row was live. Queue one complete client/minute vector handoff,
        -- but do not re-evaluate or mutate the published generation.
        INSERT INTO public.telemetry_dashboard_block_events (
            client_id, domain, event_kind, source_bucket_secs,
            block_start_unix, bucket_start_unix, queued_at
        )
        SELECT DISTINCT rows.client_id,
               'network',
               'coordinate',
               rows.bucket_secs,
               public.telemetry_dashboard_block_start(
                   extract(epoch FROM rows.bucket_start)::BIGINT,
                   rows.bucket_secs
               ),
               extract(epoch FROM rows.bucket_start)::BIGINT,
               public.telemetry_dashboard_event_queued_at()
        FROM new_telemetry_network_rates rows
        JOIN public.telemetry_dashboard_network_projection_heads head
          ON head.client_id = rows.client_id
         AND rows.interface = ANY(head.network_generation_interfaces);
        RETURN NULL;
    END IF;

    IF NOT EXISTS (SELECT 1 FROM new_telemetry_network_rates) THEN
        RETURN NULL;
    END IF;

    FOR affected IN
        SELECT rows.client_id,
               array_agg(
                   DISTINCT rows.interface COLLATE "C"
                   ORDER BY rows.interface COLLATE "C"
               ) AS interfaces
        FROM new_telemetry_network_rates rows
        GROUP BY rows.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_change(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT rows.client_id,
           'network',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           rows.bucket_secs,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM rows.bucket_start)::BIGINT,
               rows.bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE
                   extract(epoch FROM rows.bucket_start)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM new_telemetry_network_rates rows
    JOIN public.telemetry_dashboard_network_projection_heads head
      ON head.client_id = rows.client_id
     AND rows.interface = ANY(head.network_generation_interfaces);

    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_network_blocks_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected RECORD;
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;

    IF NOT EXISTS (SELECT 1 FROM old_telemetry_network_rates) THEN
        RETURN NULL;
    END IF;

    FOR affected IN
        SELECT rows.client_id,
               array_agg(
                   DISTINCT rows.interface COLLATE "C"
                   ORDER BY rows.interface COLLATE "C"
               ) AS interfaces
        FROM old_telemetry_network_rates rows
        JOIN public.telemetry_dashboard_clients dashboard_client
          ON dashboard_client.client_id = rows.client_id
        GROUP BY rows.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_removal(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT rows.client_id,
           'network',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           rows.bucket_secs,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM rows.bucket_start)::BIGINT,
               rows.bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE
                   extract(epoch FROM rows.bucket_start)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM old_telemetry_network_rates rows
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = rows.client_id
    JOIN public.telemetry_dashboard_network_projection_heads head
      ON head.client_id = rows.client_id
     AND rows.interface = ANY(head.network_generation_interfaces);

    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_network_blocks_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected RECORD;
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;

    IF NOT EXISTS (SELECT 1 FROM old_telemetry_network_rates)
       AND NOT EXISTS (SELECT 1 FROM new_telemetry_network_rates) THEN
        RETURN NULL;
    END IF;

    FOR affected IN
        SELECT changed.client_id,
               array_agg(
                   changed.interface ORDER BY changed.interface COLLATE "C"
               ) AS interfaces
        FROM (
            SELECT DISTINCT client_id, interface
            FROM new_telemetry_network_rates
            EXCEPT
            SELECT DISTINCT client_id, interface
            FROM old_telemetry_network_rates
        ) changed
        GROUP BY changed.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_change(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    FOR affected IN
        SELECT changed.client_id,
               array_agg(
                   changed.interface ORDER BY changed.interface COLLATE "C"
               ) AS interfaces
        FROM (
            SELECT DISTINCT client_id, interface
            FROM old_telemetry_network_rates
            EXCEPT
            SELECT DISTINCT client_id, interface
            FROM new_telemetry_network_rates
        ) changed
        GROUP BY changed.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_removal(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    PERFORM public.queue_telemetry_dashboard_network_pending_generation_catchup(
        COALESCE(
            array_agg(
                stable.client_id
                ORDER BY stable.client_id, stable.interface COLLATE "C"
            ),
            ARRAY[]::TEXT[]
        ),
        COALESCE(
            array_agg(
                stable.interface
                ORDER BY stable.client_id, stable.interface COLLATE "C"
            ),
            ARRAY[]::TEXT[]
        )
    )
    FROM (
        SELECT DISTINCT client_id, interface
        FROM old_telemetry_network_rates
        INTERSECT
        SELECT DISTINCT client_id, interface
        FROM new_telemetry_network_rates
    ) stable;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT changed.client_id,
           'network',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           changed.bucket_secs,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM changed.bucket_start)::BIGINT,
               changed.bucket_secs
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE
                   extract(epoch FROM changed.bucket_start)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM (
        SELECT client_id, interface, bucket_secs, bucket_start
        FROM old_telemetry_network_rates
        UNION
        SELECT client_id, interface, bucket_secs, bucket_start
        FROM new_telemetry_network_rates
    ) changed
    JOIN public.telemetry_dashboard_network_projection_heads head
      ON head.client_id = changed.client_id
     AND changed.interface = ANY(head.network_generation_interfaces);

    RETURN NULL;
END
$$;

CREATE TRIGGER telemetry_network_rates_dashboard_after_insert
AFTER INSERT ON public.telemetry_network_rates
REFERENCING NEW TABLE AS new_telemetry_network_rates
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_network_blocks_after_insert();

CREATE TRIGGER telemetry_network_rates_dashboard_after_delete
AFTER DELETE ON public.telemetry_network_rates
REFERENCING OLD TABLE AS old_telemetry_network_rates
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_network_blocks_after_delete();

CREATE TRIGGER telemetry_network_rates_dashboard_after_update
AFTER UPDATE ON public.telemetry_network_rates
REFERENCING OLD TABLE AS old_telemetry_network_rates
            NEW TABLE AS new_telemetry_network_rates
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_network_blocks_after_update();

-- Closed 60-second host aggregates live in traffic_counter_samples until the
-- day-one retained-owner handoff. A live close queues its exact coordinate;
-- the later representation-only transfer is publication-silent. Unmarked
-- import/correction changes still reconcile selector membership because they
-- can introduce or remove a historical interface.
CREATE FUNCTION public.queue_telemetry_network_samples_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        INSERT INTO public.telemetry_dashboard_block_events (
            client_id, domain, event_kind, source_bucket_secs,
            block_start_unix, bucket_start_unix, queued_at
        )
        SELECT DISTINCT sample.client_id, 'network', 'coordinate', 60,
               public.telemetry_dashboard_block_start(
                   extract(epoch FROM sample.observed_at)::BIGINT, 60
               ),
               extract(epoch FROM sample.observed_at)::BIGINT,
               public.telemetry_dashboard_event_queued_at()
        FROM new_traffic_counter_samples sample
        JOIN public.telemetry_dashboard_network_projection_heads head
          ON head.client_id = sample.client_id
         AND sample.interface = ANY(head.network_generation_interfaces)
        WHERE sample.source_kind = 'host';
        RETURN NULL;
    END IF;

    -- The live minute publisher inserts every claimed client setwise. Preserve
    -- that ownership here: compare the complete changed relation with compact
    -- generation membership once, and resolve rules only for genuinely new
    -- interfaces. A steady minute therefore performs no per-client function
    -- loop and a first interface still queues exactly one generation owner.
    WITH changed AS MATERIALIZED (
        SELECT DISTINCT sample.client_id, sample.interface
        FROM new_traffic_counter_samples sample
        WHERE sample.source_kind = 'host'
    ), novel AS MATERIALIZED (
        SELECT changed.client_id, changed.interface
        FROM changed
        JOIN public.telemetry_dashboard_network_projection_heads head
          ON head.client_id = changed.client_id
        WHERE NOT changed.interface = ANY(head.network_generation_interfaces)
    ), owners AS MATERIALIZED (
        SELECT DISTINCT novel.client_id
        FROM novel
    ), selections AS MATERIALIZED (
        SELECT owner.client_id,
               public.telemetry_dashboard_effective_network_selection(
                   owner.client_id
               ) AS selection
        FROM owners owner
    ), policies AS MATERIALIZED (
        SELECT policy.*
        FROM public.resolve_telemetry_interface_policies(ARRAY(
            SELECT owner.client_id FROM owners owner ORDER BY owner.client_id
        )) policy
    )
    INSERT INTO public.telemetry_dashboard_generation_events (
        client_id, domain, queued_at
    )
    SELECT DISTINCT novel.client_id, 'network',
           public.telemetry_dashboard_event_queued_at()
    FROM novel
    JOIN selections USING (client_id)
    JOIN policies USING (client_id)
    WHERE public.telemetry_dashboard_network_interface_selected_resolved(
        policies.admission_mode,
        policies.interface_patterns,
        policies.managed_tunnel_interfaces,
        selections.selection,
        novel.interface
    );

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT sample.client_id, 'network',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           60,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM sample.observed_at)::BIGINT, 60
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE extract(epoch FROM sample.observed_at)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM new_traffic_counter_samples sample
    JOIN public.telemetry_dashboard_network_projection_heads head
      ON head.client_id = sample.client_id
     AND sample.interface = ANY(head.network_generation_interfaces)
    WHERE sample.source_kind = 'host';
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_network_samples_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected RECORD;
BEGIN
    IF public.telemetry_dashboard_retained_transfer_requested() THEN
        RETURN NULL;
    END IF;
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;

    FOR affected IN
        SELECT changed.client_id,
               array_agg(
                   changed.interface ORDER BY changed.interface COLLATE "C"
               ) AS interfaces
        FROM (
            SELECT DISTINCT sample.client_id, sample.interface
            FROM old_traffic_counter_samples sample
            JOIN public.telemetry_dashboard_clients dashboard_client
              ON dashboard_client.client_id = sample.client_id
            WHERE sample.source_kind = 'host'
        ) changed
        GROUP BY changed.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_removal(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT sample.client_id, 'network',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           60,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM sample.observed_at)::BIGINT, 60
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE extract(epoch FROM sample.observed_at)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM old_traffic_counter_samples sample
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = sample.client_id
    JOIN public.telemetry_dashboard_network_projection_heads head
      ON head.client_id = sample.client_id
     AND sample.interface = ANY(head.network_generation_interfaces)
    WHERE sample.source_kind = 'host';
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_network_samples_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    affected RECORD;
BEGIN
    PERFORM public.queue_telemetry_dashboard_network_pending_generation_catchup(
        COALESCE(
            array_agg(
                stable.client_id
                ORDER BY stable.client_id, stable.interface COLLATE "C"
            ),
            ARRAY[]::TEXT[]
        ),
        COALESCE(
            array_agg(
                stable.interface
                ORDER BY stable.client_id, stable.interface COLLATE "C"
            ),
            ARRAY[]::TEXT[]
        )
    )
    FROM (
        SELECT DISTINCT sample.client_id, sample.interface
        FROM old_traffic_counter_samples sample
        WHERE sample.source_kind = 'host'
        INTERSECT
        SELECT DISTINCT sample.client_id, sample.interface
        FROM new_traffic_counter_samples sample
        WHERE sample.source_kind = 'host'
    ) stable;

    IF public.telemetry_dashboard_retained_transfer_requested() THEN
        RETURN NULL;
    END IF;
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        INSERT INTO public.telemetry_dashboard_block_events (
            client_id, domain, event_kind, source_bucket_secs,
            block_start_unix, bucket_start_unix, queued_at
        )
        SELECT DISTINCT sample.client_id, 'network', 'coordinate', 60,
               public.telemetry_dashboard_block_start(
                   extract(epoch FROM sample.observed_at)::BIGINT, 60
               ),
               extract(epoch FROM sample.observed_at)::BIGINT,
               public.telemetry_dashboard_event_queued_at()
        FROM new_traffic_counter_samples sample
        JOIN public.telemetry_dashboard_network_projection_heads head
          ON head.client_id = sample.client_id
         AND sample.interface = ANY(head.network_generation_interfaces)
        WHERE sample.source_kind = 'host';
        RETURN NULL;
    END IF;

    FOR affected IN
        SELECT changed.client_id,
               array_agg(
                   changed.interface ORDER BY changed.interface COLLATE "C"
               ) AS interfaces
        FROM (
            SELECT DISTINCT sample.client_id, sample.interface
            FROM new_traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
            EXCEPT
            SELECT DISTINCT sample.client_id, sample.interface
            FROM old_traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
        ) changed
        GROUP BY changed.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_change(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    FOR affected IN
        SELECT changed.client_id,
               array_agg(
                   changed.interface ORDER BY changed.interface COLLATE "C"
               ) AS interfaces
        FROM (
            SELECT DISTINCT sample.client_id, sample.interface
            FROM old_traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
            EXCEPT
            SELECT DISTINCT sample.client_id, sample.interface
            FROM new_traffic_counter_samples sample
            WHERE sample.source_kind = 'host'
        ) changed
        GROUP BY changed.client_id
    LOOP
        PERFORM public.queue_telemetry_dashboard_network_membership_removal(
            affected.client_id, affected.interfaces
        );
    END LOOP;

    INSERT INTO public.telemetry_dashboard_block_events (
        client_id, domain, event_kind, source_bucket_secs,
        block_start_unix, bucket_start_unix, queued_at
    )
    SELECT DISTINCT changed.client_id, 'network',
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN 'full_block' ELSE 'coordinate' END,
           60,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM changed.observed_at)::BIGINT, 60
           ),
           CASE WHEN public.telemetry_dashboard_full_block_requested()
               THEN NULL ELSE extract(epoch FROM changed.observed_at)::BIGINT
               END,
           public.telemetry_dashboard_event_queued_at()
    FROM (
        SELECT sample.client_id, sample.interface, sample.observed_at
        FROM old_traffic_counter_samples sample
        WHERE sample.source_kind = 'host'
        UNION
        SELECT sample.client_id, sample.interface, sample.observed_at
        FROM new_traffic_counter_samples sample
        WHERE sample.source_kind = 'host'
    ) changed
    JOIN public.telemetry_dashboard_network_projection_heads head
      ON head.client_id = changed.client_id
     AND changed.interface = ANY(head.network_generation_interfaces);
    RETURN NULL;
END
$$;

CREATE TRIGGER traffic_counter_samples_dashboard_after_insert
AFTER INSERT ON public.traffic_counter_samples
REFERENCING NEW TABLE AS new_traffic_counter_samples
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_network_samples_after_insert();

CREATE TRIGGER traffic_counter_samples_dashboard_after_delete
AFTER DELETE ON public.traffic_counter_samples
REFERENCING OLD TABLE AS old_traffic_counter_samples
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_network_samples_after_delete();

CREATE TRIGGER traffic_counter_samples_dashboard_after_update
AFTER UPDATE ON public.traffic_counter_samples
REFERENCING OLD TABLE AS old_traffic_counter_samples
            NEW TABLE AS new_traffic_counter_samples
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_network_samples_after_update();

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.queue_telemetry_dashboard_traffic_coordinates(
        payload.client_ids,
        payload.source_kinds,
        payload.interfaces,
        payload.origin_kinds,
        payload.bucket_starts,
        payload.bucket_secs
    )
    FROM (
        WITH changed AS MATERIALIZED (
            SELECT DISTINCT sample.client_id,
                   sample.source_kind,
                   sample.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       sample.sample_source
                   ) AS origin_kind,
                   sample.observed_at
            FROM new_telemetry_dashboard_traffic_samples sample
        ), affected AS MATERIALIZED (
            SELECT * FROM changed
            UNION
            SELECT changed.client_id,
                   changed.source_kind,
                   changed.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       successor.sample_source
                   ) AS origin_kind,
                   successor.observed_at
            FROM changed
            JOIN LATERAL (
                SELECT sample.observed_at,
                       sample.sample_source,
                       sample.usage_authoritative
                FROM public.traffic_counter_samples sample
                WHERE sample.client_id = changed.client_id
                  AND sample.source_kind = changed.source_kind
                  AND sample.interface = changed.interface
                  AND sample.observed_at > changed.observed_at
                ORDER BY sample.observed_at
                LIMIT 1
            ) successor ON NOT successor.usage_authoritative
        )
        SELECT COALESCE(array_agg(
                   affected.client_id
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS client_ids,
               COALESCE(array_agg(
                   affected.source_kind
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS source_kinds,
               COALESCE(array_agg(
                   affected.interface
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS interfaces,
               COALESCE(array_agg(
                   affected.origin_kind
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS origin_kinds,
               COALESCE(array_agg(
                   affected.observed_at
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TIMESTAMPTZ[]) AS bucket_starts,
               COALESCE(array_agg(
                   60::INTEGER
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::INTEGER[]) AS bucket_secs
        FROM affected
    ) payload;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.queue_telemetry_dashboard_traffic_coordinates(
        payload.client_ids,
        payload.source_kinds,
        payload.interfaces,
        payload.origin_kinds,
        payload.bucket_starts,
        payload.bucket_secs
    )
    FROM (
        WITH changed AS MATERIALIZED (
            SELECT DISTINCT sample.client_id,
                   sample.source_kind,
                   sample.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       sample.sample_source
                   ) AS origin_kind,
                   sample.observed_at
            FROM old_telemetry_dashboard_traffic_samples sample
        ), affected AS MATERIALIZED (
            SELECT * FROM changed
            UNION
            SELECT changed.client_id,
                   changed.source_kind,
                   changed.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       successor.sample_source
                   ) AS origin_kind,
                   successor.observed_at
            FROM changed
            JOIN LATERAL (
                SELECT sample.observed_at,
                       sample.sample_source,
                       sample.usage_authoritative
                FROM public.traffic_counter_samples sample
                WHERE sample.client_id = changed.client_id
                  AND sample.source_kind = changed.source_kind
                  AND sample.interface = changed.interface
                  AND sample.observed_at > changed.observed_at
                ORDER BY sample.observed_at
                LIMIT 1
            ) successor ON NOT successor.usage_authoritative
        )
        SELECT COALESCE(array_agg(
                   affected.client_id
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS client_ids,
               COALESCE(array_agg(
                   affected.source_kind
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS source_kinds,
               COALESCE(array_agg(
                   affected.interface
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS interfaces,
               COALESCE(array_agg(
                   affected.origin_kind
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS origin_kinds,
               COALESCE(array_agg(
                   affected.observed_at
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TIMESTAMPTZ[]) AS bucket_starts,
               COALESCE(array_agg(
                   60::INTEGER
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::INTEGER[]) AS bucket_secs
        FROM affected
    ) payload;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.queue_telemetry_dashboard_traffic_pending_generation_catchup(
        COALESCE(array_agg(
            stable.client_id
            ORDER BY stable.client_id,
                     stable.source_kind COLLATE "C",
                     stable.interface COLLATE "C"
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            stable.source_kind
            ORDER BY stable.client_id,
                     stable.source_kind COLLATE "C",
                     stable.interface COLLATE "C"
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            stable.interface
            ORDER BY stable.client_id,
                     stable.source_kind COLLATE "C",
                     stable.interface COLLATE "C"
        ), ARRAY[]::TEXT[])
    )
    FROM (
        SELECT DISTINCT client_id, source_kind, interface
        FROM old_telemetry_dashboard_traffic_samples
        INTERSECT
        SELECT DISTINCT client_id, source_kind, interface
        FROM new_telemetry_dashboard_traffic_samples
    ) stable;

    PERFORM public.queue_telemetry_dashboard_traffic_coordinates(
        payload.client_ids,
        payload.source_kinds,
        payload.interfaces,
        payload.origin_kinds,
        payload.bucket_starts,
        payload.bucket_secs
    )
    FROM (
        WITH changed AS MATERIALIZED (
            SELECT sample.client_id,
                   sample.source_kind,
                   sample.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       sample.sample_source
                   ) AS origin_kind,
                   sample.observed_at
            FROM old_telemetry_dashboard_traffic_samples sample
            UNION
            SELECT sample.client_id,
                   sample.source_kind,
                   sample.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       sample.sample_source
                   ) AS origin_kind,
                   sample.observed_at
            FROM new_telemetry_dashboard_traffic_samples sample
        ), affected AS MATERIALIZED (
            SELECT * FROM changed
            UNION
            SELECT changed.client_id,
                   changed.source_kind,
                   changed.interface,
                   public.telemetry_dashboard_traffic_origin_kind(
                       successor.sample_source
                   ) AS origin_kind,
                   successor.observed_at
            FROM changed
            JOIN LATERAL (
                SELECT sample.observed_at,
                       sample.sample_source,
                       sample.usage_authoritative
                FROM public.traffic_counter_samples sample
                WHERE sample.client_id = changed.client_id
                  AND sample.source_kind = changed.source_kind
                  AND sample.interface = changed.interface
                  AND sample.observed_at > changed.observed_at
                ORDER BY sample.observed_at
                LIMIT 1
            ) successor ON NOT successor.usage_authoritative
        )
        SELECT COALESCE(array_agg(
                   affected.client_id
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS client_ids,
               COALESCE(array_agg(
                   affected.source_kind
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS source_kinds,
               COALESCE(array_agg(
                   affected.interface
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS interfaces,
               COALESCE(array_agg(
                   affected.origin_kind
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TEXT[]) AS origin_kinds,
               COALESCE(array_agg(
                   affected.observed_at
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::TIMESTAMPTZ[]) AS bucket_starts,
               COALESCE(array_agg(
                   60::INTEGER
                   ORDER BY affected.client_id,
                            affected.source_kind COLLATE "C",
                            affected.interface COLLATE "C",
                            affected.observed_at,
                            affected.origin_kind COLLATE "C"
               ), ARRAY[]::INTEGER[]) AS bucket_secs
        FROM affected
    ) payload;
    RETURN NULL;
END
$$;

CREATE TRIGGER traffic_counter_samples_dashboard_traffic_after_insert
AFTER INSERT ON public.traffic_counter_samples
REFERENCING NEW TABLE AS new_telemetry_dashboard_traffic_samples
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_insert();

CREATE TRIGGER traffic_counter_samples_dashboard_traffic_after_delete
AFTER DELETE ON public.traffic_counter_samples
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_samples
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_delete();

CREATE TRIGGER traffic_counter_samples_dashboard_traffic_after_update
AFTER UPDATE ON public.traffic_counter_samples
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_samples
            NEW TABLE AS new_telemetry_dashboard_traffic_samples
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_samples_after_update();

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_rollups_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.queue_telemetry_dashboard_traffic_coordinates(
        COALESCE(array_agg(
            rollup.client_id ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.source_kind ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.interface ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.origin_kind ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.bucket_start ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TIMESTAMPTZ[]),
        COALESCE(array_agg(
            rollup.bucket_secs ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::INTEGER[])
    )
    FROM (
        SELECT DISTINCT client_id, source_kind, interface, origin_kind,
               bucket_secs, bucket_start
        FROM new_telemetry_dashboard_traffic_rollups
    ) rollup;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_rollups_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.queue_telemetry_dashboard_traffic_coordinates(
        COALESCE(array_agg(
            rollup.client_id ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.source_kind ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.interface ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.origin_kind ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.bucket_start ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TIMESTAMPTZ[]),
        COALESCE(array_agg(
            rollup.bucket_secs ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::INTEGER[])
    )
    FROM (
        SELECT DISTINCT client_id, source_kind, interface, origin_kind,
               bucket_secs, bucket_start
        FROM old_telemetry_dashboard_traffic_rollups
    ) rollup;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.queue_telemetry_dashboard_traffic_rollups_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.queue_telemetry_dashboard_traffic_pending_generation_catchup(
        COALESCE(array_agg(
            stable.client_id ORDER BY stable.client_id,
                stable.source_kind COLLATE "C",
                stable.interface COLLATE "C"
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            stable.source_kind ORDER BY stable.client_id,
                stable.source_kind COLLATE "C",
                stable.interface COLLATE "C"
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            stable.interface ORDER BY stable.client_id,
                stable.source_kind COLLATE "C",
                stable.interface COLLATE "C"
        ), ARRAY[]::TEXT[])
    )
    FROM (
        SELECT DISTINCT client_id, source_kind, interface
        FROM old_telemetry_dashboard_traffic_rollups
        INTERSECT
        SELECT DISTINCT client_id, source_kind, interface
        FROM new_telemetry_dashboard_traffic_rollups
    ) stable;

    PERFORM public.queue_telemetry_dashboard_traffic_coordinates(
        COALESCE(array_agg(
            rollup.client_id ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.source_kind ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.interface ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.origin_kind ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TEXT[]),
        COALESCE(array_agg(
            rollup.bucket_start ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::TIMESTAMPTZ[]),
        COALESCE(array_agg(
            rollup.bucket_secs ORDER BY rollup.client_id,
                rollup.source_kind COLLATE "C",
                rollup.interface COLLATE "C",
                rollup.origin_kind COLLATE "C",
                rollup.bucket_secs, rollup.bucket_start
        ), ARRAY[]::INTEGER[])
    )
    FROM (
        SELECT client_id, source_kind, interface, origin_kind,
               bucket_secs, bucket_start
        FROM old_telemetry_dashboard_traffic_rollups
        UNION
        SELECT client_id, source_kind, interface, origin_kind,
               bucket_secs, bucket_start
        FROM new_telemetry_dashboard_traffic_rollups
    ) rollup;
    RETURN NULL;
END
$$;

CREATE TRIGGER traffic_counter_rollups_dashboard_traffic_after_insert
AFTER INSERT ON public.traffic_counter_rollups
REFERENCING NEW TABLE AS new_telemetry_dashboard_traffic_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_rollups_after_insert();

CREATE TRIGGER traffic_counter_rollups_dashboard_traffic_after_delete
AFTER DELETE ON public.traffic_counter_rollups
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_rollups_after_delete();

CREATE TRIGGER traffic_counter_rollups_dashboard_traffic_after_update
AFTER UPDATE ON public.traffic_counter_rollups
REFERENCING OLD TABLE AS old_telemetry_dashboard_traffic_rollups
            NEW TABLE AS new_telemetry_dashboard_traffic_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.queue_telemetry_dashboard_traffic_rollups_after_update();

CREATE FUNCTION public.replace_telemetry_dashboard_resource_closed_block(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_bucket_secs INTEGER,
    p_block_start_unix BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    DELETE FROM public.telemetry_dashboard_resource_blocks block
    WHERE block.client_id = p_client_id
      AND block.generation = p_generation
      AND block.source_bucket_secs = p_source_bucket_secs
      AND block.block_start_unix = p_block_start_unix;

    INSERT INTO public.telemetry_dashboard_resource_blocks (
        client_id, generation, source_bucket_secs,
        block_start_unix, published_revision,
        sample_counts, cpu_load_1_sums, cpu_load_1_maxes,
        memory_total_bytes_maxes, memory_used_ratio_sums,
        memory_used_ratio_maxes, disk_sample_counts,
        disk_total_bytes_maxes, disk_used_ratio_sums,
        disk_used_ratio_maxes, latest_observed_unix
    )
    SELECT p_client_id, p_generation, p_source_bucket_secs,
           p_block_start_unix, p_revision,
           array_agg(
               COALESCE(source.sample_count, 0)::BIGINT
               ORDER BY slot.ordinal
           ),
           array_agg(source.cpu_load_1_sum ORDER BY slot.ordinal),
           array_agg(
               source.cpu_load_1_max::REAL ORDER BY slot.ordinal
           ),
           array_agg(
               source.memory_total_bytes_max ORDER BY slot.ordinal
           ),
           array_agg(
               source.memory_used_ratio_sum ORDER BY slot.ordinal
           ),
           array_agg(
               source.memory_used_ratio_max::REAL ORDER BY slot.ordinal
           ),
           array_agg(
               COALESCE(source.disk_sample_count, 0)::BIGINT
               ORDER BY slot.ordinal
           ),
           array_agg(
               source.disk_total_bytes_max ORDER BY slot.ordinal
           ),
           array_agg(
               source.disk_used_ratio_sum ORDER BY slot.ordinal
           ),
           array_agg(
               source.disk_used_ratio_max::REAL ORDER BY slot.ordinal
           ),
           array_agg(
               extract(epoch FROM source.latest_observed_at)::BIGINT
               ORDER BY slot.ordinal
           )
    FROM generate_series(
        0, public.telemetry_dashboard_block_factor() - 1
    ) slot(ordinal)
    LEFT JOIN public.telemetry_rollups source
      ON source.client_id = p_client_id
     AND source.bucket_secs = p_source_bucket_secs
     AND source.bucket_start = to_timestamp(
         p_block_start_unix
         + slot.ordinal::BIGINT * p_source_bucket_secs
     )
    HAVING count(source.client_id) > 0;
END
$$;

-- One owner revision may contain several resource tiers, slots, or F16 blocks.
-- Resolve each requested coordinate once, then write each affected F16 primary
-- key once. Unrequested slots retain their prior value; requested coordinates
-- replace their slot even when the source disappeared.
CREATE FUNCTION public.replace_telemetry_dashboard_resource_coordinates(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_bucket_secs INTEGER[],
    p_bucket_start_unix BIGINT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) = 0
       OR cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) <> cardinality(COALESCE(
           p_bucket_start_unix, ARRAY[]::BIGINT[]
       ))
       OR EXISTS (
            SELECT 1
            FROM unnest(p_source_bucket_secs, p_bucket_start_unix)
                coordinate(source_bucket_secs, bucket_start_unix)
            WHERE NOT public.telemetry_dashboard_source_tier_is_valid(
                      coordinate.source_bucket_secs
                  )
               OR mod(
                      coordinate.bucket_start_unix,
                      coordinate.source_bucket_secs
                  ) <> 0
       ) THEN
        RAISE EXCEPTION 'invalid resource dashboard coordinate set';
    END IF;

    WITH requested AS MATERIALIZED (
        SELECT DISTINCT coordinate.source_bucket_secs,
               coordinate.bucket_start_unix,
               public.telemetry_dashboard_block_start(
                   coordinate.bucket_start_unix,
                   coordinate.source_bucket_secs
               ) AS block_start_unix
        FROM unnest(p_source_bucket_secs, p_bucket_start_unix)
            coordinate(source_bucket_secs, bucket_start_unix)
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
          ON source.client_id = p_client_id
         AND source.bucket_secs = requested.source_bucket_secs
         AND source.bucket_start = to_timestamp(
             requested.bucket_start_unix
         )
    ), affected_blocks AS MATERIALIZED (
        SELECT DISTINCT requested.source_bucket_secs,
               requested.block_start_unix
        FROM requested
    ), assembled AS MATERIALIZED (
        SELECT p_client_id AS client_id,
           p_generation AS generation,
           affected.source_bucket_secs,
           affected.block_start_unix,
           p_revision AS published_revision,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   COALESCE(source.sample_count, 0)::BIGINT
               ELSE COALESCE(prior.sample_counts[slot.ordinal + 1], 0)
               END ORDER BY slot.ordinal
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
        FROM generate_series(
            0, public.telemetry_dashboard_block_factor() - 1
        ) slot(ordinal)
        CROSS JOIN affected_blocks affected
        LEFT JOIN public.telemetry_dashboard_resource_blocks prior
          ON prior.client_id = p_client_id
         AND prior.generation = p_generation
         AND prior.source_bucket_secs = affected.source_bucket_secs
         AND prior.block_start_unix = affected.block_start_unix
        LEFT JOIN coordinate_source source
          ON source.source_bucket_secs = affected.source_bucket_secs
         AND source.bucket_start_unix = affected.block_start_unix
                + slot.ordinal::BIGINT * affected.source_bucket_secs
        GROUP BY affected.source_bucket_secs, affected.block_start_unix
    ), replacement AS MATERIALIZED (
        SELECT assembled.*,
               EXISTS (
                   SELECT 1
                   FROM unnest(assembled.sample_counts) count(value)
                   WHERE count.value > 0
               ) AS has_samples
        FROM assembled
    )
    MERGE INTO public.telemetry_dashboard_resource_blocks AS target
    USING replacement AS source
      ON target.client_id = source.client_id
     AND target.generation = source.generation
     AND target.source_bucket_secs = source.source_bucket_secs
     AND target.block_start_unix = source.block_start_unix
    WHEN MATCHED AND NOT source.has_samples THEN
        DELETE
    WHEN MATCHED THEN
        UPDATE SET
            published_revision = source.published_revision,
            sample_counts = source.sample_counts,
            cpu_load_1_sums = source.cpu_load_1_sums,
            cpu_load_1_maxes = source.cpu_load_1_maxes,
            memory_total_bytes_maxes = source.memory_total_bytes_maxes,
            memory_used_ratio_sums = source.memory_used_ratio_sums,
            memory_used_ratio_maxes = source.memory_used_ratio_maxes,
            disk_sample_counts = source.disk_sample_counts,
            disk_total_bytes_maxes = source.disk_total_bytes_maxes,
            disk_used_ratio_sums = source.disk_used_ratio_sums,
            disk_used_ratio_maxes = source.disk_used_ratio_maxes,
            latest_observed_unix = source.latest_observed_unix
    WHEN NOT MATCHED AND source.has_samples THEN
        INSERT (
            client_id, generation, source_bucket_secs,
            block_start_unix, published_revision,
            sample_counts, cpu_load_1_sums, cpu_load_1_maxes,
            memory_total_bytes_maxes, memory_used_ratio_sums,
            memory_used_ratio_maxes, disk_sample_counts,
            disk_total_bytes_maxes, disk_used_ratio_sums,
            disk_used_ratio_maxes, latest_observed_unix
        ) VALUES (
            source.client_id, source.generation, source.source_bucket_secs,
            source.block_start_unix, source.published_revision,
            source.sample_counts, source.cpu_load_1_sums,
            source.cpu_load_1_maxes, source.memory_total_bytes_maxes,
            source.memory_used_ratio_sums, source.memory_used_ratio_maxes,
            source.disk_sample_counts, source.disk_total_bytes_maxes,
            source.disk_used_ratio_sums, source.disk_used_ratio_maxes,
            source.latest_observed_unix
        );
END
$$;

-- Generation bounds are properties of the compact projection, not another
-- reason to walk retained telemetry after every coordinate change. Empty
-- blocks are deleted by the replacement owner, so one primary-key seek at
-- each edge and one fixed sixteen-slot inspection recover the exact bounds.
CREATE FUNCTION public.telemetry_dashboard_resource_block_edges(
    p_client_id TEXT,
    p_generation BIGINT,
    p_source_bucket_secs INTEGER
)
RETURNS TABLE (first_unix BIGINT, last_unix BIGINT)
LANGUAGE sql
STABLE
STRICT
AS $$
    WITH first_block AS MATERIALIZED (
        SELECT block.block_start_unix, block.sample_counts
        FROM public.telemetry_dashboard_resource_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
          AND block.source_bucket_secs = p_source_bucket_secs
        ORDER BY block.block_start_unix
        LIMIT 1
    ), last_block AS MATERIALIZED (
        SELECT block.block_start_unix, block.sample_counts
        FROM public.telemetry_dashboard_resource_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
          AND block.source_bucket_secs = p_source_bucket_secs
        ORDER BY block.block_start_unix DESC
        LIMIT 1
    )
    SELECT
        first_block.block_start_unix
            + (first_slot.ordinal - 1)::BIGINT * p_source_bucket_secs,
        last_block.block_start_unix
            + (last_slot.ordinal - 1)::BIGINT * p_source_bucket_secs
    FROM first_block
    CROSS JOIN last_block
    CROSS JOIN LATERAL (
        SELECT min(ordinal)::INTEGER AS ordinal
        FROM generate_subscripts(first_block.sample_counts, 1) ordinal
        WHERE first_block.sample_counts[ordinal] > 0
    ) first_slot
    CROSS JOIN LATERAL (
        SELECT max(ordinal)::INTEGER AS ordinal
        FROM generate_subscripts(last_block.sample_counts, 1) ordinal
        WHERE last_block.sample_counts[ordinal] > 0
    ) last_slot
$$;

CREATE FUNCTION public.refresh_telemetry_dashboard_resource_coordinates(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_bucket_secs INTEGER[],
    p_bucket_start_unix BIGINT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.replace_telemetry_dashboard_resource_coordinates(
        p_client_id, p_generation, p_revision,
        p_source_bucket_secs, p_bucket_start_unix
    );

    WITH requested_tiers AS MATERIALIZED (
        SELECT DISTINCT tier.source_bucket_secs
        FROM unnest(p_source_bucket_secs) tier(source_bucket_secs)
    ), current_edges AS MATERIALIZED (
        SELECT tier.source_bucket_secs,
               edge.first_unix,
               edge.last_unix
        FROM requested_tiers tier
        LEFT JOIN LATERAL public.telemetry_dashboard_resource_block_edges(
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
        )
        VALUES (
            p_client_id, p_generation, source.source_bucket_secs,
            source.first_unix, source.last_unix,
            public.telemetry_dashboard_block_start(
                source.last_unix, source.source_bucket_secs
            )
        );
END
$$;

CREATE FUNCTION public.refresh_telemetry_dashboard_resource_block(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_bucket_secs INTEGER,
    p_dirty_block_start_unix BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    new_first BIGINT;
    new_last BIGINT;
    new_active BIGINT;
BEGIN
    PERFORM public.replace_telemetry_dashboard_resource_closed_block(
        p_client_id, p_generation, p_revision,
        p_source_bucket_secs, p_dirty_block_start_unix
    );

    SELECT edge.first_unix, edge.last_unix
    INTO new_first, new_last
    FROM public.telemetry_dashboard_resource_block_edges(
        p_client_id, p_generation, p_source_bucket_secs
    ) edge;

    IF new_last IS NOT NULL THEN
        new_active := public.telemetry_dashboard_block_start(
            new_last, p_source_bucket_secs
        );
    END IF;

    IF new_first IS NULL THEN
        DELETE FROM public.telemetry_dashboard_resource_generation_bounds bounds
        WHERE bounds.client_id = p_client_id
          AND bounds.generation = p_generation
          AND bounds.source_bucket_secs = p_source_bucket_secs;
    ELSE
        INSERT INTO public.telemetry_dashboard_resource_generation_bounds (
            client_id, generation, source_bucket_secs,
            first_bucket_start_unix, last_bucket_start_unix,
            active_block_start_unix
        )
        VALUES (
            p_client_id, p_generation, p_source_bucket_secs,
            new_first, new_last, new_active
        )
        ON CONFLICT (
            client_id, generation, source_bucket_secs
        ) DO UPDATE SET
            first_bucket_start_unix =
                EXCLUDED.first_bucket_start_unix,
            last_bucket_start_unix =
                EXCLUDED.last_bucket_start_unix,
            active_block_start_unix =
                EXCLUDED.active_block_start_unix;
    END IF;
END
$$;

CREATE FUNCTION public.replace_telemetry_dashboard_network_closed_block(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER,
    p_block_start_unix BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    width INTEGER := cardinality(p_interfaces);
BEGIN
    DELETE FROM public.telemetry_dashboard_network_blocks block
    WHERE block.client_id = p_client_id
      AND block.generation = p_generation
      AND block.source_bucket_secs = p_source_bucket_secs
      AND block.block_start_unix = p_block_start_unix;

    IF width = 0 THEN
        RETURN;
    END IF;

    INSERT INTO public.telemetry_dashboard_network_blocks (
        client_id, generation, interface_width,
        source_bucket_secs, block_start_unix, published_revision,
        sample_counts, latest_observed_unix,
        rx_bytes_last, tx_bytes_last,
        rx_counter_epoch, tx_counter_epoch
    )
    SELECT p_client_id, p_generation, width,
           p_source_bucket_secs, p_block_start_unix, p_revision,
           array_agg(
               COALESCE(source.sample_count, 0)::BIGINT
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               extract(epoch FROM source.latest_observed_at)::BIGINT
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.rx_bytes_last
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.tx_bytes_last
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.rx_counter_epoch
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.tx_counter_epoch
               ORDER BY slot.ordinal, interface.ordinal
           )
    FROM generate_series(
        0, public.telemetry_dashboard_block_factor() - 1
    ) slot(ordinal)
    CROSS JOIN unnest(p_interfaces) WITH ORDINALITY
        interface(name, ordinal)
    LEFT JOIN public.telemetry_network_durable_points_source(
        ARRAY[p_client_id],
        to_timestamp(p_block_start_unix),
        to_timestamp(
            p_block_start_unix
            + (public.telemetry_dashboard_block_factor() - 1)::BIGINT
                * p_source_bucket_secs
        ),
        p_source_bucket_secs,
        p_interfaces
    ) source
      ON source.client_id = p_client_id
     AND source.interface = interface.name
     AND source.bucket_secs = p_source_bucket_secs
     AND source.bucket_start = to_timestamp(
         p_block_start_unix
         + slot.ordinal::BIGINT * p_source_bucket_secs
     )
    HAVING count(source.client_id) > 0;
END
$$;

-- Network coordinates share the same owner-revision boundary as resource and
-- traffic coordinates. Resolve every requested coordinate/interface once and
-- update each affected slot-major F16 vector once.
CREATE FUNCTION public.replace_telemetry_dashboard_network_coordinates(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER[],
    p_bucket_start_unix BIGINT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    width INTEGER := cardinality(p_interfaces);
BEGIN
    IF cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) = 0
       OR cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) <> cardinality(COALESCE(
           p_bucket_start_unix, ARRAY[]::BIGINT[]
       ))
       OR EXISTS (
            SELECT 1
            FROM unnest(p_source_bucket_secs, p_bucket_start_unix)
                coordinate(source_bucket_secs, bucket_start_unix)
            WHERE NOT public.telemetry_dashboard_source_tier_is_valid(
                      coordinate.source_bucket_secs
                  )
               OR mod(
                      coordinate.bucket_start_unix,
                      coordinate.source_bucket_secs
                  ) <> 0
       ) THEN
        RAISE EXCEPTION 'invalid network dashboard coordinate set';
    END IF;

    IF width = 0 THEN
        RETURN;
    END IF;

    WITH requested AS MATERIALIZED (
        SELECT DISTINCT coordinate.source_bucket_secs,
               coordinate.bucket_start_unix,
               public.telemetry_dashboard_block_start(
                   coordinate.bucket_start_unix,
                   coordinate.source_bucket_secs
               ) AS block_start_unix
        FROM unnest(p_source_bucket_secs, p_bucket_start_unix)
            coordinate(source_bucket_secs, bucket_start_unix)
    ), durable_source AS MATERIALIZED (
        -- Resolve each exact dirty coordinate once for this client.  The source
        -- function receives the physical owner, minute and tier before either
        -- durable relation is read; the frozen interface vector completes the
        -- physical lookup key and remains joined below to preserve its order.
        SELECT source.*
        FROM requested
        CROSS JOIN LATERAL public.telemetry_network_durable_points_source(
            ARRAY[p_client_id],
            to_timestamp(requested.bucket_start_unix),
            to_timestamp(requested.bucket_start_unix),
            requested.source_bucket_secs,
            p_interfaces
        ) source
        WHERE source.interface = ANY(p_interfaces)
    ), coordinate_source AS MATERIALIZED (
        SELECT requested.*,
               interface.name AS interface,
               source.client_id AS source_client_id,
               source.sample_count,
               source.latest_observed_at,
               source.rx_bytes_last,
               source.tx_bytes_last,
               source.rx_counter_epoch,
               source.tx_counter_epoch
        FROM requested
        CROSS JOIN unnest(p_interfaces) interface(name)
        LEFT JOIN durable_source source
          ON source.client_id = p_client_id
         AND source.interface = interface.name
         AND source.bucket_secs = requested.source_bucket_secs
         AND source.bucket_start = to_timestamp(
             requested.bucket_start_unix
         )
    ), affected_blocks AS MATERIALIZED (
        SELECT DISTINCT requested.source_bucket_secs,
               requested.block_start_unix
        FROM requested
    ), assembled AS MATERIALIZED (
        SELECT p_client_id AS client_id,
           p_generation AS generation,
           width AS interface_width,
           affected.source_bucket_secs,
           affected.block_start_unix,
           p_revision AS published_revision,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   COALESCE(source.sample_count, 0)::BIGINT
               ELSE COALESCE(prior.sample_counts[
                   slot.ordinal * width + interface.ordinal
               ], 0) END
               ORDER BY slot.ordinal, interface.ordinal
           ) AS sample_counts,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   extract(epoch FROM source.latest_observed_at)::BIGINT
               ELSE prior.latest_observed_unix[
                   slot.ordinal * width + interface.ordinal
               ] END
               ORDER BY slot.ordinal, interface.ordinal
           ) AS latest_observed_unix,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   source.rx_bytes_last
               ELSE prior.rx_bytes_last[
                   slot.ordinal * width + interface.ordinal
               ] END
               ORDER BY slot.ordinal, interface.ordinal
           ) AS rx_bytes_last,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   source.tx_bytes_last
               ELSE prior.tx_bytes_last[
                   slot.ordinal * width + interface.ordinal
               ] END
               ORDER BY slot.ordinal, interface.ordinal
           ) AS tx_bytes_last,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   source.rx_counter_epoch
               ELSE prior.rx_counter_epoch[
                   slot.ordinal * width + interface.ordinal
               ] END
               ORDER BY slot.ordinal, interface.ordinal
           ) AS rx_counter_epoch,
           array_agg(
               CASE WHEN source.bucket_start_unix IS NOT NULL THEN
                   source.tx_counter_epoch
               ELSE prior.tx_counter_epoch[
                   slot.ordinal * width + interface.ordinal
               ] END
               ORDER BY slot.ordinal, interface.ordinal
           ) AS tx_counter_epoch
        FROM generate_series(
            0, public.telemetry_dashboard_block_factor() - 1
        ) slot(ordinal)
        CROSS JOIN affected_blocks affected
        CROSS JOIN unnest(p_interfaces) WITH ORDINALITY
            interface(name, ordinal)
        LEFT JOIN public.telemetry_dashboard_network_blocks prior
          ON prior.client_id = p_client_id
         AND prior.generation = p_generation
         AND prior.source_bucket_secs = affected.source_bucket_secs
         AND prior.block_start_unix = affected.block_start_unix
        LEFT JOIN coordinate_source source
          ON source.source_bucket_secs = affected.source_bucket_secs
         AND source.bucket_start_unix = affected.block_start_unix
                + slot.ordinal::BIGINT * affected.source_bucket_secs
         AND source.interface = interface.name
        GROUP BY affected.source_bucket_secs, affected.block_start_unix
    ), replacement AS MATERIALIZED (
        SELECT assembled.*,
               EXISTS (
                   SELECT 1
                   FROM unnest(assembled.sample_counts) count(value)
                   WHERE count.value > 0
               ) AS has_samples
        FROM assembled
    )
    MERGE INTO public.telemetry_dashboard_network_blocks AS target
    USING replacement AS source
      ON target.client_id = source.client_id
     AND target.generation = source.generation
     AND target.source_bucket_secs = source.source_bucket_secs
     AND target.block_start_unix = source.block_start_unix
    WHEN MATCHED AND NOT source.has_samples THEN
        DELETE
    WHEN MATCHED THEN
        UPDATE SET
            published_revision = source.published_revision,
            interface_width = source.interface_width,
            sample_counts = source.sample_counts,
            latest_observed_unix = source.latest_observed_unix,
            rx_bytes_last = source.rx_bytes_last,
            tx_bytes_last = source.tx_bytes_last,
            rx_counter_epoch = source.rx_counter_epoch,
            tx_counter_epoch = source.tx_counter_epoch
    WHEN NOT MATCHED AND source.has_samples THEN
        INSERT (
            client_id, generation, interface_width,
            source_bucket_secs, block_start_unix, published_revision,
            sample_counts, latest_observed_unix,
            rx_bytes_last, tx_bytes_last,
            rx_counter_epoch, tx_counter_epoch
        ) VALUES (
            source.client_id, source.generation, source.interface_width,
            source.source_bucket_secs, source.block_start_unix,
            source.published_revision, source.sample_counts,
            source.latest_observed_unix, source.rx_bytes_last,
            source.tx_bytes_last, source.rx_counter_epoch,
            source.tx_counter_epoch
        );
END
$$;

-- Network blocks use one fixed slot-major vector per sixteen calendar
-- slots. The first and last primary-key blocks therefore contain the exact
-- generation edges; only those two bounded vectors need inspection.
CREATE FUNCTION public.telemetry_dashboard_network_block_edges(
    p_client_id TEXT,
    p_generation BIGINT,
    p_source_bucket_secs INTEGER
)
RETURNS TABLE (first_unix BIGINT, last_unix BIGINT)
LANGUAGE sql
STABLE
STRICT
AS $$
    WITH first_block AS MATERIALIZED (
        SELECT block.block_start_unix, block.interface_width,
               block.sample_counts
        FROM public.telemetry_dashboard_network_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
          AND block.source_bucket_secs = p_source_bucket_secs
        ORDER BY block.block_start_unix
        LIMIT 1
    ), last_block AS MATERIALIZED (
        SELECT block.block_start_unix, block.interface_width,
               block.sample_counts
        FROM public.telemetry_dashboard_network_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
          AND block.source_bucket_secs = p_source_bucket_secs
        ORDER BY block.block_start_unix DESC
        LIMIT 1
    )
    SELECT
        first_block.block_start_unix
            + first_slot.ordinal::BIGINT * p_source_bucket_secs,
        last_block.block_start_unix
            + last_slot.ordinal::BIGINT * p_source_bucket_secs
    FROM first_block
    CROSS JOIN last_block
    CROSS JOIN LATERAL (
        SELECT min(
            (ordinal - 1) / first_block.interface_width
        )::INTEGER AS ordinal
        FROM generate_subscripts(first_block.sample_counts, 1) ordinal
        WHERE first_block.sample_counts[ordinal] > 0
    ) first_slot
    CROSS JOIN LATERAL (
        SELECT max(
            (ordinal - 1) / last_block.interface_width
        )::INTEGER AS ordinal
        FROM generate_subscripts(last_block.sample_counts, 1) ordinal
        WHERE last_block.sample_counts[ordinal] > 0
    ) last_slot
$$;

CREATE FUNCTION public.refresh_telemetry_dashboard_network_coordinates(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER[],
    p_bucket_start_unix BIGINT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.replace_telemetry_dashboard_network_coordinates(
        p_client_id, p_generation, p_revision, p_interfaces,
        p_source_bucket_secs, p_bucket_start_unix
    );

    WITH requested_tiers AS MATERIALIZED (
        SELECT DISTINCT tier.source_bucket_secs
        FROM unnest(p_source_bucket_secs) tier(source_bucket_secs)
    ), current_edges AS MATERIALIZED (
        SELECT tier.source_bucket_secs,
               edge.first_unix,
               edge.last_unix
        FROM requested_tiers tier
        LEFT JOIN LATERAL public.telemetry_dashboard_network_block_edges(
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
            interface_width = cardinality(p_interfaces),
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
        )
        VALUES (
            p_client_id, p_generation, cardinality(p_interfaces),
            source.source_bucket_secs, source.first_unix, source.last_unix,
            public.telemetry_dashboard_block_start(
                source.last_unix, source.source_bucket_secs
            )
        );
END
$$;

CREATE FUNCTION public.refresh_telemetry_dashboard_network_block(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER,
    p_dirty_block_start_unix BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    width INTEGER := cardinality(p_interfaces);
    new_first BIGINT;
    new_last BIGINT;
    new_active BIGINT;
BEGIN
    PERFORM public.replace_telemetry_dashboard_network_closed_block(
        p_client_id, p_generation, p_revision, p_interfaces,
        p_source_bucket_secs, p_dirty_block_start_unix
    );

    SELECT edge.first_unix, edge.last_unix
    INTO new_first, new_last
    FROM public.telemetry_dashboard_network_block_edges(
        p_client_id, p_generation, p_source_bucket_secs
    ) edge;

    IF new_last IS NOT NULL THEN
        new_active := public.telemetry_dashboard_block_start(
            new_last, p_source_bucket_secs
        );
    END IF;

    IF new_first IS NULL THEN
        DELETE FROM public.telemetry_dashboard_network_generation_bounds bounds
        WHERE bounds.client_id = p_client_id
          AND bounds.generation = p_generation
          AND bounds.source_bucket_secs = p_source_bucket_secs;
    ELSE
        INSERT INTO public.telemetry_dashboard_network_generation_bounds (
            client_id, generation, interface_width,
            source_bucket_secs, first_bucket_start_unix,
            last_bucket_start_unix, active_block_start_unix
        )
        VALUES (
            p_client_id, p_generation, width,
            p_source_bucket_secs, new_first, new_last, new_active
        )
        ON CONFLICT (
            client_id, generation, source_bucket_secs
        ) DO UPDATE SET
            interface_width = EXCLUDED.interface_width,
            first_bucket_start_unix =
                EXCLUDED.first_bucket_start_unix,
            last_bucket_start_unix =
                EXCLUDED.last_bucket_start_unix,
            active_block_start_unix =
                EXCLUDED.active_block_start_unix;
    END IF;
END
$$;

CREATE FUNCTION public.replace_telemetry_dashboard_traffic_closed_block(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER,
    p_block_start_unix BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    DELETE FROM public.telemetry_dashboard_traffic_blocks block
    WHERE block.client_id = p_client_id
      AND block.generation = p_generation
      AND block.source_bucket_secs = p_source_bucket_secs
      AND block.block_start_unix = p_block_start_unix;

    INSERT INTO public.telemetry_dashboard_traffic_blocks (
        client_id, generation, source_bucket_secs,
        block_start_unix, published_revision,
        rx_valid_counts, tx_valid_counts, rx_bytes, tx_bytes
    )
    WITH source AS MATERIALIZED (
        SELECT point.*
        FROM public.telemetry_dashboard_traffic_source_points(
            p_client_id,
            p_source_kinds,
            p_interfaces,
            p_source_bucket_secs,
            to_timestamp(p_block_start_unix),
            to_timestamp(
                p_block_start_unix
                + (public.telemetry_dashboard_block_factor() - 1)::BIGINT
                    * p_source_bucket_secs
            )
        ) point
    )
    SELECT p_client_id,
           p_generation,
           p_source_bucket_secs,
           p_block_start_unix,
           p_revision,
           array_agg(source.rx_valid_count ORDER BY slot.ordinal),
           array_agg(source.tx_valid_count ORDER BY slot.ordinal),
           array_agg(source.rx_bytes ORDER BY slot.ordinal),
           array_agg(source.tx_bytes ORDER BY slot.ordinal)
    FROM generate_series(
        0, public.telemetry_dashboard_block_factor() - 1
    ) slot(ordinal)
    LEFT JOIN source
      ON source.bucket_start = to_timestamp(
          p_block_start_unix
          + slot.ordinal::BIGINT * p_source_bucket_secs
      )
    HAVING count(source.client_id) > 0;
END
$$;

-- One owner revision can name several tiers, slots, or F16 blocks.  Reduce the
-- exact coordinate set once, probe each source coordinate once, and write each
-- affected F16 primary-key row once.  Unnamed slots remain byte-for-byte owned
-- by the prior block; named coordinates are replaced even when their source
-- disappeared, preserving correction/deletion semantics.
CREATE FUNCTION public.replace_telemetry_dashboard_traffic_coordinates(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER[],
    p_bucket_start_unix BIGINT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    IF cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) = 0
       OR cardinality(COALESCE(
           p_source_bucket_secs, ARRAY[]::INTEGER[]
       )) <> cardinality(COALESCE(
           p_bucket_start_unix, ARRAY[]::BIGINT[]
       ))
       OR EXISTS (
            SELECT 1
            FROM unnest(p_source_bucket_secs, p_bucket_start_unix)
                coordinate(source_bucket_secs, bucket_start_unix)
            WHERE NOT public.telemetry_dashboard_traffic_source_tier_is_valid(
                      coordinate.source_bucket_secs
                  )
               OR mod(
                      coordinate.bucket_start_unix,
                      coordinate.source_bucket_secs
                  ) <> 0
       ) THEN
        RAISE EXCEPTION 'invalid traffic dashboard coordinate set';
    END IF;

    WITH requested AS MATERIALIZED (
        SELECT DISTINCT coordinate.source_bucket_secs,
               coordinate.bucket_start_unix,
               public.telemetry_dashboard_block_start(
                   coordinate.bucket_start_unix,
                   coordinate.source_bucket_secs
               ) AS block_start_unix
        FROM unnest(p_source_bucket_secs, p_bucket_start_unix)
            coordinate(source_bucket_secs, bucket_start_unix)
    ), coordinate_source AS MATERIALIZED (
        SELECT requested.*,
               source.client_id AS source_client_id,
               source.rx_valid_count,
               source.tx_valid_count,
               source.rx_bytes,
               source.tx_bytes
        FROM requested
        LEFT JOIN LATERAL public.telemetry_dashboard_traffic_source_points(
            p_client_id,
            p_source_kinds,
            p_interfaces,
            requested.source_bucket_secs,
            to_timestamp(requested.bucket_start_unix),
            to_timestamp(requested.bucket_start_unix)
        ) source ON TRUE
    ), affected_blocks AS MATERIALIZED (
        SELECT DISTINCT requested.source_bucket_secs,
               requested.block_start_unix
        FROM requested
    ), assembled AS MATERIALIZED (
        SELECT p_client_id AS client_id,
           p_generation AS generation,
           affected.source_bucket_secs,
           affected.block_start_unix,
           p_revision AS published_revision,
           array_agg(CASE WHEN source.bucket_start_unix IS NOT NULL
               THEN source.rx_valid_count
               ELSE prior.rx_valid_counts[slot.ordinal + 1]
           END ORDER BY slot.ordinal) AS rx_valid_counts,
           array_agg(CASE WHEN source.bucket_start_unix IS NOT NULL
               THEN source.tx_valid_count
               ELSE prior.tx_valid_counts[slot.ordinal + 1]
           END ORDER BY slot.ordinal) AS tx_valid_counts,
           array_agg(CASE WHEN source.bucket_start_unix IS NOT NULL
               THEN source.rx_bytes
               ELSE prior.rx_bytes[slot.ordinal + 1]
           END ORDER BY slot.ordinal) AS rx_bytes,
           array_agg(CASE WHEN source.bucket_start_unix IS NOT NULL
               THEN source.tx_bytes
               ELSE prior.tx_bytes[slot.ordinal + 1]
           END ORDER BY slot.ordinal) AS tx_bytes
        FROM affected_blocks affected
        CROSS JOIN generate_series(
            0, public.telemetry_dashboard_block_factor() - 1
        ) slot(ordinal)
        LEFT JOIN public.telemetry_dashboard_traffic_blocks prior
          ON prior.client_id = p_client_id
         AND prior.generation = p_generation
         AND prior.source_bucket_secs = affected.source_bucket_secs
         AND prior.block_start_unix = affected.block_start_unix
        LEFT JOIN coordinate_source source
          ON source.source_bucket_secs = affected.source_bucket_secs
         AND source.bucket_start_unix = affected.block_start_unix
                + slot.ordinal::BIGINT * affected.source_bucket_secs
        GROUP BY affected.source_bucket_secs, affected.block_start_unix
    ), replacement AS MATERIALIZED (
        SELECT assembled.*,
               EXISTS (
                   SELECT 1
                   FROM unnest(assembled.rx_valid_counts) count(value)
                   WHERE count.value IS NOT NULL
               ) AS has_samples
        FROM assembled
    )
    MERGE INTO public.telemetry_dashboard_traffic_blocks AS target
    USING replacement AS source
      ON target.client_id = source.client_id
     AND target.generation = source.generation
     AND target.source_bucket_secs = source.source_bucket_secs
     AND target.block_start_unix = source.block_start_unix
    WHEN MATCHED AND NOT source.has_samples THEN
        DELETE
    WHEN MATCHED THEN
        UPDATE SET
            published_revision = source.published_revision,
            rx_valid_counts = source.rx_valid_counts,
            tx_valid_counts = source.tx_valid_counts,
            rx_bytes = source.rx_bytes,
            tx_bytes = source.tx_bytes
    WHEN NOT MATCHED AND source.has_samples THEN
        INSERT (
            client_id, generation, source_bucket_secs,
            block_start_unix, published_revision,
            rx_valid_counts, tx_valid_counts, rx_bytes, tx_bytes
        ) VALUES (
            source.client_id, source.generation, source.source_bucket_secs,
            source.block_start_unix, source.published_revision,
            source.rx_valid_counts, source.tx_valid_counts,
            source.rx_bytes, source.tx_bytes
        );
END
$$;

CREATE FUNCTION public.telemetry_dashboard_traffic_block_edges(
    p_client_id TEXT,
    p_generation BIGINT,
    p_source_bucket_secs INTEGER
)
RETURNS TABLE (first_unix BIGINT, last_unix BIGINT)
LANGUAGE sql
STABLE
STRICT
AS $$
    WITH first_block AS MATERIALIZED (
        SELECT block.block_start_unix, block.rx_valid_counts
        FROM public.telemetry_dashboard_traffic_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
          AND block.source_bucket_secs = p_source_bucket_secs
        ORDER BY block.block_start_unix
        LIMIT 1
    ), last_block AS MATERIALIZED (
        SELECT block.block_start_unix, block.rx_valid_counts
        FROM public.telemetry_dashboard_traffic_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
          AND block.source_bucket_secs = p_source_bucket_secs
        ORDER BY block.block_start_unix DESC
        LIMIT 1
    )
    SELECT first_block.block_start_unix
               + (first_slot.ordinal - 1)::BIGINT
                    * p_source_bucket_secs,
           last_block.block_start_unix
               + (last_slot.ordinal - 1)::BIGINT
                    * p_source_bucket_secs
    FROM first_block
    CROSS JOIN last_block
    CROSS JOIN LATERAL (
        SELECT min(ordinal)::INTEGER AS ordinal
        FROM generate_subscripts(first_block.rx_valid_counts, 1) ordinal
        WHERE first_block.rx_valid_counts[ordinal] IS NOT NULL
    ) first_slot
    CROSS JOIN LATERAL (
        SELECT max(ordinal)::INTEGER AS ordinal
        FROM generate_subscripts(last_block.rx_valid_counts, 1) ordinal
        WHERE last_block.rx_valid_counts[ordinal] IS NOT NULL
    ) last_slot
$$;

-- Reconcile each affected tier only after every F16 mutation is installed.
-- The edge function stops at the first and last primary-key block, so this is
-- bounded by distinct requested tiers rather than retained history length.
CREATE FUNCTION public.refresh_telemetry_dashboard_traffic_coordinates(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER[],
    p_bucket_start_unix BIGINT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM public.replace_telemetry_dashboard_traffic_coordinates(
        p_client_id, p_generation, p_revision,
        p_source_kinds, p_interfaces,
        p_source_bucket_secs, p_bucket_start_unix
    );

    WITH requested_tiers AS MATERIALIZED (
        SELECT DISTINCT tier.source_bucket_secs
        FROM unnest(p_source_bucket_secs) tier(source_bucket_secs)
    ), current_edges AS MATERIALIZED (
        SELECT tier.source_bucket_secs,
               edge.first_unix,
               edge.last_unix
        FROM requested_tiers tier
        LEFT JOIN LATERAL public.telemetry_dashboard_traffic_block_edges(
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
            stream_width = cardinality(p_source_kinds),
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
            p_client_id, p_generation, cardinality(p_source_kinds),
            source.source_bucket_secs, source.first_unix,
            source.last_unix,
            public.telemetry_dashboard_block_start(
                source.last_unix, source.source_bucket_secs
            )
        );
END
$$;

CREATE FUNCTION public.refresh_telemetry_dashboard_traffic_block(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_kinds TEXT[],
    p_interfaces TEXT[],
    p_source_bucket_secs INTEGER,
    p_dirty_block_start_unix BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    new_first BIGINT;
    new_last BIGINT;
BEGIN
    PERFORM public.replace_telemetry_dashboard_traffic_closed_block(
        p_client_id, p_generation, p_revision,
        p_source_kinds, p_interfaces,
        p_source_bucket_secs, p_dirty_block_start_unix
    );

    SELECT edge.first_unix, edge.last_unix
    INTO new_first, new_last
    FROM public.telemetry_dashboard_traffic_block_edges(
        p_client_id, p_generation, p_source_bucket_secs
    ) edge;

    IF new_first IS NULL THEN
        DELETE FROM public.telemetry_dashboard_traffic_generation_bounds bounds
        WHERE bounds.client_id = p_client_id
          AND bounds.generation = p_generation
          AND bounds.source_bucket_secs = p_source_bucket_secs;
    ELSE
        INSERT INTO public.telemetry_dashboard_traffic_generation_bounds (
            client_id, generation, stream_width,
            source_bucket_secs, first_bucket_start_unix,
            last_bucket_start_unix, active_block_start_unix
        ) VALUES (
            p_client_id, p_generation, cardinality(p_source_kinds),
            p_source_bucket_secs, new_first, new_last,
            public.telemetry_dashboard_block_start(
                new_last, p_source_bucket_secs
            )
        )
        ON CONFLICT (client_id, generation, source_bucket_secs)
        DO UPDATE SET
            stream_width = EXCLUDED.stream_width,
            first_bucket_start_unix = EXCLUDED.first_bucket_start_unix,
            last_bucket_start_unix = EXCLUDED.last_bucket_start_unix,
            active_block_start_unix = EXCLUDED.active_block_start_unix;
    END IF;
END
$$;

CREATE FUNCTION public.build_telemetry_dashboard_resource_generation(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    DELETE FROM public.telemetry_dashboard_resource_blocks
    WHERE client_id = p_client_id AND generation = p_generation;
    DELETE FROM public.telemetry_dashboard_resource_generation_bounds
    WHERE client_id = p_client_id AND generation = p_generation;

    INSERT INTO public.telemetry_dashboard_resource_generation_bounds (
        client_id, generation, source_bucket_secs,
        first_bucket_start_unix, last_bucket_start_unix,
        active_block_start_unix
    )
    SELECT p_client_id, p_generation, source.bucket_secs,
           extract(epoch FROM min(source.bucket_start))::BIGINT,
           extract(epoch FROM max(source.bucket_start))::BIGINT,
           public.telemetry_dashboard_block_start(
               extract(epoch FROM max(source.bucket_start))::BIGINT,
               source.bucket_secs
           )
    FROM public.telemetry_rollups source
    WHERE source.client_id = p_client_id
    GROUP BY source.bucket_secs;

    INSERT INTO public.telemetry_dashboard_resource_blocks (
        client_id, generation, source_bucket_secs,
        block_start_unix, published_revision,
        sample_counts, cpu_load_1_sums, cpu_load_1_maxes,
        memory_total_bytes_maxes, memory_used_ratio_sums,
        memory_used_ratio_maxes, disk_sample_counts,
        disk_total_bytes_maxes, disk_used_ratio_sums,
        disk_used_ratio_maxes, latest_observed_unix
    )
    WITH block_keys AS MATERIALIZED (
        SELECT DISTINCT source.bucket_secs,
               public.telemetry_dashboard_block_start(
                   extract(epoch FROM source.bucket_start)::BIGINT,
                   source.bucket_secs
               ) AS block_start_unix
        FROM public.telemetry_rollups source
        WHERE source.client_id = p_client_id
    )
    SELECT p_client_id, p_generation, block.bucket_secs,
           block.block_start_unix, p_revision,
           array_agg(
               COALESCE(source.sample_count, 0)::BIGINT
               ORDER BY slot.ordinal
           ),
           array_agg(source.cpu_load_1_sum ORDER BY slot.ordinal),
           array_agg(
               source.cpu_load_1_max::REAL ORDER BY slot.ordinal
           ),
           array_agg(
               source.memory_total_bytes_max ORDER BY slot.ordinal
           ),
           array_agg(
               source.memory_used_ratio_sum ORDER BY slot.ordinal
           ),
           array_agg(
               source.memory_used_ratio_max::REAL ORDER BY slot.ordinal
           ),
           array_agg(
               COALESCE(source.disk_sample_count, 0)::BIGINT
               ORDER BY slot.ordinal
           ),
           array_agg(
               source.disk_total_bytes_max ORDER BY slot.ordinal
           ),
           array_agg(
               source.disk_used_ratio_sum ORDER BY slot.ordinal
           ),
           array_agg(
               source.disk_used_ratio_max::REAL ORDER BY slot.ordinal
           ),
           array_agg(
               extract(epoch FROM source.latest_observed_at)::BIGINT
               ORDER BY slot.ordinal
           )
    FROM block_keys block
    CROSS JOIN generate_series(
        0, public.telemetry_dashboard_block_factor() - 1
    ) slot(ordinal)
    LEFT JOIN public.telemetry_rollups source
      ON source.client_id = p_client_id
     AND source.bucket_secs = block.bucket_secs
     AND source.bucket_start = to_timestamp(
         block.block_start_unix
         + slot.ordinal::BIGINT * block.bucket_secs
     )
    GROUP BY block.bucket_secs, block.block_start_unix;
END
$$;

CREATE FUNCTION public.build_telemetry_dashboard_network_generation(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_interfaces TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    width INTEGER := cardinality(p_interfaces);
BEGIN
    IF NOT public.telemetry_dashboard_interfaces_are_canonical(p_interfaces) THEN
        RAISE EXCEPTION 'network generation interfaces are not canonical';
    END IF;

    DELETE FROM public.telemetry_dashboard_network_blocks
    WHERE client_id = p_client_id AND generation = p_generation;
    DELETE FROM public.telemetry_dashboard_network_generation_bounds
    WHERE client_id = p_client_id AND generation = p_generation;

    IF width = 0 THEN
        RETURN;
    END IF;

    INSERT INTO public.telemetry_dashboard_network_blocks (
        client_id, generation, interface_width,
        source_bucket_secs, block_start_unix, published_revision,
        sample_counts, latest_observed_unix,
        rx_bytes_last, tx_bytes_last,
        rx_counter_epoch, tx_counter_epoch
    )
    WITH effective_source AS MATERIALIZED (
        -- A generation rebuild is the one owner allowed to read this client's
        -- selected history. Resolve the retained/transient overlay once, then
        -- derive every block key and payload from that same snapshot.
        SELECT source.*
        FROM public.telemetry_network_durable_points_source(
            ARRAY[p_client_id],
            NULL::TIMESTAMPTZ,
            NULL::TIMESTAMPTZ,
            NULL::INTEGER,
            p_interfaces
        ) source
        WHERE source.client_id = p_client_id
          AND source.interface = ANY(p_interfaces)
    ), block_keys AS MATERIALIZED (
        SELECT DISTINCT source.bucket_secs,
               public.telemetry_dashboard_block_start(
                   extract(epoch FROM source.bucket_start)::BIGINT,
                   source.bucket_secs
               ) AS block_start_unix
        FROM effective_source source
    )
    SELECT p_client_id, p_generation, width,
           block.bucket_secs, block.block_start_unix, p_revision,
           array_agg(
               COALESCE(source.sample_count, 0)::BIGINT
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               extract(epoch FROM source.latest_observed_at)::BIGINT
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.rx_bytes_last
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.tx_bytes_last
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.rx_counter_epoch
               ORDER BY slot.ordinal, interface.ordinal
           ),
           array_agg(
               source.tx_counter_epoch
               ORDER BY slot.ordinal, interface.ordinal
           )
    FROM block_keys block
    CROSS JOIN generate_series(
        0, public.telemetry_dashboard_block_factor() - 1
    ) slot(ordinal)
    CROSS JOIN unnest(p_interfaces) WITH ORDINALITY
        interface(name, ordinal)
    LEFT JOIN effective_source source
      ON source.interface = interface.name
     AND source.bucket_secs = block.bucket_secs
     AND source.bucket_start = to_timestamp(
         block.block_start_unix
         + slot.ordinal::BIGINT * block.bucket_secs
     )
    GROUP BY block.bucket_secs, block.block_start_unix;

    -- Bounds belong to the compact generation just written.  Each tier reads
    -- only its first and last primary-key block and the fixed sixteen-slot
    -- vectors, so retained history is not scanned a second time.
    INSERT INTO public.telemetry_dashboard_network_generation_bounds (
        client_id, generation, interface_width,
        source_bucket_secs, first_bucket_start_unix,
        last_bucket_start_unix, active_block_start_unix
    )
    SELECT p_client_id, p_generation, width,
           tier.source_bucket_secs,
           edge.first_unix, edge.last_unix,
           public.telemetry_dashboard_block_start(
               edge.last_unix, tier.source_bucket_secs
           )
    FROM (
        SELECT DISTINCT block.source_bucket_secs
        FROM public.telemetry_dashboard_network_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
    ) tier
    CROSS JOIN LATERAL public.telemetry_dashboard_network_block_edges(
        p_client_id, p_generation, tier.source_bucket_secs
    ) edge;
END
$$;

CREATE FUNCTION public.build_telemetry_dashboard_traffic_generation(
    p_client_id TEXT,
    p_generation BIGINT,
    p_revision BIGINT,
    p_source_kinds TEXT[],
    p_interfaces TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
DECLARE
    width INTEGER := cardinality(p_source_kinds);
BEGIN
    IF NOT public.telemetry_dashboard_traffic_identities_are_canonical(
        p_source_kinds, p_interfaces
    ) THEN
        RAISE EXCEPTION 'traffic generation identities are not canonical';
    END IF;

    DELETE FROM public.telemetry_dashboard_traffic_blocks
    WHERE client_id = p_client_id AND generation = p_generation;
    DELETE FROM public.telemetry_dashboard_traffic_generation_bounds
    WHERE client_id = p_client_id AND generation = p_generation;

    IF width = 0 THEN
        RETURN;
    END IF;

    INSERT INTO public.telemetry_dashboard_traffic_blocks (
        client_id, generation, source_bucket_secs,
        block_start_unix, published_revision,
        rx_valid_counts, tx_valid_counts, rx_bytes, tx_bytes
    )
    WITH effective_source AS MATERIALIZED (
        SELECT source.*
        FROM public.telemetry_dashboard_traffic_source_points(
            p_client_id,
            p_source_kinds,
            p_interfaces,
            NULL,
            NULL,
            NULL
        ) source
    ), block_keys AS MATERIALIZED (
        SELECT DISTINCT source.bucket_secs,
               public.telemetry_dashboard_block_start(
                   extract(epoch FROM source.bucket_start)::BIGINT,
                   source.bucket_secs
               ) AS block_start_unix
        FROM effective_source source
    )
    SELECT p_client_id,
           p_generation,
           block.bucket_secs,
           block.block_start_unix,
           p_revision,
           array_agg(source.rx_valid_count ORDER BY slot.ordinal),
           array_agg(source.tx_valid_count ORDER BY slot.ordinal),
           array_agg(source.rx_bytes ORDER BY slot.ordinal),
           array_agg(source.tx_bytes ORDER BY slot.ordinal)
    FROM block_keys block
    CROSS JOIN generate_series(
        0, public.telemetry_dashboard_block_factor() - 1
    ) slot(ordinal)
    LEFT JOIN effective_source source
      ON source.bucket_secs = block.bucket_secs
     AND source.bucket_start = to_timestamp(
         block.block_start_unix
         + slot.ordinal::BIGINT * block.bucket_secs
     )
    GROUP BY block.bucket_secs, block.block_start_unix;

    INSERT INTO public.telemetry_dashboard_traffic_generation_bounds (
        client_id, generation, stream_width,
        source_bucket_secs, first_bucket_start_unix,
        last_bucket_start_unix, active_block_start_unix
    )
    SELECT p_client_id,
           p_generation,
           width,
           tier.source_bucket_secs,
           edge.first_unix,
           edge.last_unix,
           public.telemetry_dashboard_block_start(
               edge.last_unix, tier.source_bucket_secs
           )
    FROM (
        SELECT DISTINCT block.source_bucket_secs
        FROM public.telemetry_dashboard_traffic_blocks block
        WHERE block.client_id = p_client_id
          AND block.generation = p_generation
    ) tier
    CROSS JOIN LATERAL public.telemetry_dashboard_traffic_block_edges(
        p_client_id, p_generation, tier.source_bucket_secs
    ) edge;
END
$$;

CREATE FUNCTION public.acquire_next_telemetry_dashboard_projection_owner()
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
    -- Ownership is session-scoped and is acquired before the publisher opens
    -- its repeatable-read source snapshot.  The registry row is immutable, so
    -- independent publishers never try to lock a tuple version changed by another
    -- publisher. The ready relation has at most one row per pending owner, so
    -- duplicate source events cannot expand acquisition work.
    FOR candidate IN
        SELECT ready.owner_id,
               ready.wake_revision
        FROM public.telemetry_dashboard_ready_owners ready
        WHERE ready.retry_not_before <= clock_timestamp()
        ORDER BY ready.ready_at, ready.owner_id
    LOOP
        IF pg_try_advisory_lock(candidate.owner_id) THEN
            SELECT fence.client_id,
                   fence.domain
            INTO client_id,
                 domain
            FROM public.telemetry_dashboard_projection_fences fence
            WHERE fence.owner_id = candidate.owner_id;
            IF NOT FOUND THEN
                -- A concurrent client deletion can remove the immutable fence
                -- after the ready cursor's statement snapshot. Never return a
                -- nameless owner or leak its session advisory lock.
                PERFORM pg_advisory_unlock(candidate.owner_id);
                CONTINUE;
            END IF;
            -- The ready row is discovery, not publication authority. Return it
            -- even if the later RR claim is empty; that empty transaction can
            -- consume the exact hint without rereading source history.
            owner_id := candidate.owner_id;
            ready_revision := candidate.wake_revision;
            RETURN NEXT;
            RETURN;
        END IF;
    END LOOP;
END
$$;

CREATE FUNCTION public.claim_telemetry_dashboard_projection(
    p_owner_id BIGINT
)
RETURNS TABLE (
    client_id TEXT,
    domain TEXT,
    change TEXT,
    event_kind TEXT[],
    source_bucket_secs INTEGER[],
    block_start_unix BIGINT[],
    bucket_start_unix BIGINT[],
    captured_block_event_ids BIGINT[],
    captured_generation_event_ids BIGINT[],
    expected_generation BIGINT,
    expected_revision BIGINT
)
LANGUAGE plpgsql
AS $$
DECLARE
    selected_client_id TEXT;
    selected_domain TEXT;
    selected_change TEXT;
BEGIN
    SELECT fence.client_id,
           fence.domain
    INTO selected_client_id,
         selected_domain
    FROM public.telemetry_dashboard_projection_fences fence
    WHERE fence.owner_id = p_owner_id;

    IF NOT FOUND THEN
        RETURN;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.telemetry_dashboard_generation_events event
        WHERE event.client_id = selected_client_id
          AND event.domain = selected_domain
    ) THEN
        selected_change := 'generation';
    ELSIF EXISTS (
        SELECT 1
        FROM public.telemetry_dashboard_block_events event
        WHERE event.client_id = selected_client_id
          AND event.domain = selected_domain
    ) THEN
        selected_change := 'block';
    ELSE
        RETURN;
    END IF;

    RETURN QUERY
    WITH locked AS MATERIALIZED (
        SELECT selected_client_id AS client_id,
               selected_domain AS domain,
               selected_change AS change
    ), block_capture AS MATERIALIZED (
        SELECT array_agg(event.event_id ORDER BY event.event_id) AS ids
        FROM locked
        JOIN public.telemetry_dashboard_block_events event
          ON event.client_id = locked.client_id
         AND event.domain = locked.domain
         -- One owner snapshot publishes every block mutation it can see.  A
         -- later source commit is invisible to this repeatable-read snapshot
         -- and therefore remains queued for the next revision.
    ), captured_events AS MATERIALIZED (
        SELECT event.event_kind,
               event.source_bucket_secs,
               event.block_start_unix,
               event.bucket_start_unix
        FROM locked
        JOIN public.telemetry_dashboard_block_events event
          ON event.client_id = locked.client_id
         AND event.domain = locked.domain
         AND locked.change = 'block'
    ), work_items AS MATERIALIZED (
        SELECT 'full_block'::TEXT AS event_kind,
               event.source_bucket_secs,
               event.block_start_unix,
               NULL::BIGINT AS bucket_start_unix
        FROM captured_events event
        WHERE event.event_kind = 'full_block'
        GROUP BY event.source_bucket_secs, event.block_start_unix
        UNION ALL
        SELECT 'coordinate'::TEXT,
               event.source_bucket_secs,
               event.block_start_unix,
               event.bucket_start_unix
        FROM captured_events event
        WHERE event.event_kind = 'coordinate'
          AND NOT EXISTS (
              SELECT 1
              FROM captured_events whole
              WHERE whole.event_kind = 'full_block'
                AND whole.source_bucket_secs = event.source_bucket_secs
                AND whole.block_start_unix = event.block_start_unix
          )
        GROUP BY event.source_bucket_secs,
                 event.block_start_unix,
                 event.bucket_start_unix
    ), work AS MATERIALIZED (
        SELECT array_agg(
                   item.event_kind
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.event_kind,
                            item.bucket_start_unix
               ) AS kinds,
               array_agg(
                   item.source_bucket_secs
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.event_kind,
                            item.bucket_start_unix
               ) AS tiers,
               array_agg(
                   item.block_start_unix
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.event_kind,
                            item.bucket_start_unix
               ) AS starts,
               array_agg(
                   item.bucket_start_unix
                   ORDER BY item.source_bucket_secs,
                            item.block_start_unix,
                            item.event_kind,
                            item.bucket_start_unix
               ) AS buckets
        FROM work_items item
    ), generation_capture AS MATERIALIZED (
        SELECT array_agg(event.event_id ORDER BY event.event_id) AS ids
        FROM locked
        JOIN public.telemetry_dashboard_generation_events event
          ON event.client_id = locked.client_id
         AND event.domain = locked.domain
         AND locked.change = 'generation'
    )
    SELECT locked.client_id,
           locked.domain,
           locked.change,
           COALESCE(work.kinds, ARRAY[]::TEXT[]),
           COALESCE(work.tiers, ARRAY[]::INTEGER[]),
           COALESCE(work.starts, ARRAY[]::BIGINT[]),
           COALESCE(work.buckets, ARRAY[]::BIGINT[]),
           COALESCE(block_capture.ids, ARRAY[]::BIGINT[]),
           COALESCE(generation_capture.ids, ARRAY[]::BIGINT[]),
           CASE
               WHEN locked.domain = 'resource' THEN (
                   SELECT head.resource_generation
                   FROM public.telemetry_dashboard_resource_projection_heads head
                   WHERE head.client_id = locked.client_id
               )
               WHEN locked.domain = 'network' THEN (
                   SELECT head.network_generation
                   FROM public.telemetry_dashboard_network_projection_heads head
                   WHERE head.client_id = locked.client_id
               )
               ELSE (
                   SELECT head.traffic_generation
                   FROM public.telemetry_dashboard_traffic_projection_heads head
                   WHERE head.client_id = locked.client_id
               )
           END,
           CASE
               WHEN locked.domain = 'resource' THEN (
                   SELECT head.resource_revision
                   FROM public.telemetry_dashboard_resource_projection_heads head
                   WHERE head.client_id = locked.client_id
               )
               WHEN locked.domain = 'network' THEN (
                   SELECT head.network_revision
                   FROM public.telemetry_dashboard_network_projection_heads head
                   WHERE head.client_id = locked.client_id
               )
               ELSE (
                   SELECT head.traffic_revision
                   FROM public.telemetry_dashboard_traffic_projection_heads head
                   WHERE head.client_id = locked.client_id
               )
           END
    FROM locked
    CROSS JOIN block_capture
    CROSS JOIN work
    CROSS JOIN generation_capture;
END
$$;

CREATE FUNCTION public.publish_telemetry_dashboard_projection(
    p_client_id TEXT,
    p_domain TEXT,
    p_change TEXT,
    p_event_kind TEXT[],
    p_source_bucket_secs INTEGER[],
    p_block_start_unix BIGINT[],
    p_bucket_start_unix BIGINT[],
    p_captured_block_event_ids BIGINT[],
    p_captured_generation_event_ids BIGINT[],
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
    new_generation BIGINT;
    new_revision BIGINT := p_expected_revision + 1;
    selection public.telemetry_dashboard_network_selection;
    traffic_selection public.telemetry_dashboard_traffic_selection;
    generation_interfaces TEXT[];
    generation_source_kinds TEXT[];
    generation_select_all BOOLEAN;
    coordinate_tiers INTEGER[];
    coordinate_starts BIGINT[];
    flipped BOOLEAN;
    block_coordinate RECORD;
    notice JSONB;
BEGIN
    IF p_domain NOT IN ('resource', 'network', 'traffic')
       OR p_change NOT IN ('block', 'generation')
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
       )) THEN
        RAISE EXCEPTION 'invalid dashboard publication request';
    END IF;

    IF p_change = 'block' THEN
        IF cardinality(COALESCE(
               p_event_kind, ARRAY[]::TEXT[]
           )) = 0
           OR cardinality(COALESCE(
               p_source_bucket_secs, ARRAY[]::INTEGER[]
           )) = 0
           OR cardinality(COALESCE(
               p_captured_block_event_ids, ARRAY[]::BIGINT[]
           )) = 0
           OR cardinality(COALESCE(
               p_captured_generation_event_ids, ARRAY[]::BIGINT[]
           )) <> 0 THEN
            RAISE EXCEPTION 'invalid dashboard block publication shape';
        END IF;

        SELECT count(*)
        INTO matched_count
        FROM public.telemetry_dashboard_block_events event
        WHERE event.event_id = ANY(COALESCE(
                  p_captured_block_event_ids, ARRAY[]::BIGINT[]
              ))
          AND event.client_id = p_client_id
          AND event.domain = p_domain;

        IF matched_count <> cardinality(COALESCE(
            p_captured_block_event_ids, ARRAY[]::BIGINT[]
        )) THEN
            RAISE EXCEPTION 'dashboard block capture changed';
        END IF;

        WITH captured AS MATERIALIZED (
            SELECT event.event_kind,
                   event.source_bucket_secs,
                   event.block_start_unix,
                   event.bucket_start_unix
            FROM public.telemetry_dashboard_block_events event
            WHERE event.event_id = ANY(COALESCE(
                      p_captured_block_event_ids, ARRAY[]::BIGINT[]
                  ))
        ), normalized AS MATERIALIZED (
            SELECT 'full_block'::TEXT AS event_kind,
                   event.source_bucket_secs,
                   event.block_start_unix,
                   NULL::BIGINT AS bucket_start_unix
            FROM captured event
            WHERE event.event_kind = 'full_block'
            GROUP BY event.source_bucket_secs, event.block_start_unix
            UNION ALL
            SELECT 'coordinate'::TEXT,
                   event.source_bucket_secs,
                   event.block_start_unix,
                   event.bucket_start_unix
            FROM captured event
            WHERE event.event_kind = 'coordinate'
              AND NOT EXISTS (
                  SELECT 1
                  FROM captured whole
                  WHERE whole.event_kind = 'full_block'
                    AND whole.source_bucket_secs = event.source_bucket_secs
                    AND whole.block_start_unix = event.block_start_unix
              )
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
            RAISE EXCEPTION 'dashboard block work changed';
        END IF;
    ELSE
        IF cardinality(COALESCE(
               p_event_kind, ARRAY[]::TEXT[]
           )) <> 0
           OR cardinality(COALESCE(
               p_source_bucket_secs, ARRAY[]::INTEGER[]
           )) <> 0
           OR cardinality(COALESCE(
               p_block_start_unix, ARRAY[]::BIGINT[]
           )) <> 0
           OR cardinality(COALESCE(
               p_bucket_start_unix, ARRAY[]::BIGINT[]
           )) <> 0
           OR cardinality(COALESCE(
               p_captured_generation_event_ids, ARRAY[]::BIGINT[]
           )) = 0 THEN
            RAISE EXCEPTION 'invalid dashboard generation publication shape';
        END IF;

        SELECT count(*)
        INTO matched_count
        FROM public.telemetry_dashboard_generation_events event
        WHERE event.event_id = ANY(COALESCE(
                  p_captured_generation_event_ids, ARRAY[]::BIGINT[]
              ))
          AND event.client_id = p_client_id
          AND event.domain = p_domain;

        IF matched_count <> cardinality(COALESCE(
            p_captured_generation_event_ids, ARRAY[]::BIGINT[]
        )) THEN
            RAISE EXCEPTION 'dashboard generation capture changed';
        END IF;

        SELECT count(*)
        INTO matched_count
        FROM public.telemetry_dashboard_block_events event
        WHERE event.event_id = ANY(COALESCE(
                  p_captured_block_event_ids, ARRAY[]::BIGINT[]
              ))
          AND event.client_id = p_client_id
          AND event.domain = p_domain;

        IF matched_count <> cardinality(COALESCE(
            p_captured_block_event_ids, ARRAY[]::BIGINT[]
        )) THEN
            RAISE EXCEPTION 'dashboard generation block capture changed';
        END IF;
    END IF;

    IF p_change = 'block' THEN
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
    END IF;

    IF p_change = 'generation' THEN
        new_generation :=
            nextval('public.telemetry_dashboard_generation_seq');

        IF p_domain = 'resource' THEN
            PERFORM public.build_telemetry_dashboard_resource_generation(
                p_client_id, new_generation, new_revision
            );

            UPDATE public.telemetry_dashboard_resource_projection_heads head
            SET resource_generation = new_generation,
                resource_revision = new_revision,
                resource_change = 'generation',
                resource_change_source_bucket_secs =
                    ARRAY[]::INTEGER[],
                resource_change_block_start_unix =
                    ARRAY[]::BIGINT[],
                resource_first_at = (
                    SELECT to_timestamp(
                        min(bounds.first_bucket_start_unix)
                    )
                    FROM public.telemetry_dashboard_resource_generation_bounds
                        bounds
                    WHERE bounds.client_id = p_client_id
                      AND bounds.generation = new_generation
                ),
                resource_through_at = (
                    SELECT to_timestamp(max(
                        bounds.last_bucket_start_unix
                        + bounds.source_bucket_secs
                    ))
                    FROM public.telemetry_dashboard_resource_generation_bounds
                        bounds
                    WHERE bounds.client_id = p_client_id
                      AND bounds.generation = new_generation
                )
            WHERE head.client_id = p_client_id
              AND head.resource_generation = p_expected_generation
              AND head.resource_revision = p_expected_revision
            RETURNING TRUE INTO flipped;

            IF NOT COALESCE(flipped, FALSE) THEN
                RAISE EXCEPTION 'resource dashboard head CAS failed';
            END IF;

            DELETE FROM public.telemetry_dashboard_resource_blocks
            WHERE client_id = p_client_id
              AND generation = p_expected_generation;
            DELETE FROM public.telemetry_dashboard_resource_generation_bounds
            WHERE client_id = p_client_id
              AND generation = p_expected_generation;
        ELSIF p_domain = 'network' THEN
            selection :=
                public.telemetry_dashboard_effective_network_selection(
                    p_client_id
                );
            generation_select_all := (selection).select_all;
            generation_interfaces :=
                public.telemetry_dashboard_generation_interfaces(
                    p_client_id, selection
                );

            INSERT INTO public.telemetry_dashboard_network_generations (
                client_id, generation, select_all,
                interfaces, interface_width
            )
            VALUES (
                p_client_id, new_generation, generation_select_all,
                generation_interfaces, cardinality(generation_interfaces)
            );

            PERFORM public.build_telemetry_dashboard_network_generation(
                p_client_id, new_generation, new_revision,
                generation_interfaces
            );

            UPDATE public.telemetry_dashboard_network_projection_heads head
            SET network_generation = new_generation,
                network_revision = new_revision,
                network_change = 'generation',
                network_change_source_bucket_secs =
                    ARRAY[]::INTEGER[],
                network_change_block_start_unix =
                    ARRAY[]::BIGINT[],
                network_select_all = generation_select_all,
                network_generation_interfaces = generation_interfaces,
                network_interface_width =
                    cardinality(generation_interfaces),
                network_first_at = (
                    SELECT to_timestamp(
                        min(bounds.first_bucket_start_unix)
                    )
                    FROM public.telemetry_dashboard_network_generation_bounds
                        bounds
                    WHERE bounds.client_id = p_client_id
                      AND bounds.generation = new_generation
                ),
                network_through_at = (
                    SELECT to_timestamp(max(
                        bounds.last_bucket_start_unix
                        + bounds.source_bucket_secs
                    ))
                    FROM public.telemetry_dashboard_network_generation_bounds
                        bounds
                    WHERE bounds.client_id = p_client_id
                      AND bounds.generation = new_generation
                )
            WHERE head.client_id = p_client_id
              AND head.network_generation = p_expected_generation
              AND head.network_revision = p_expected_revision
            RETURNING TRUE INTO flipped;

            IF NOT COALESCE(flipped, FALSE) THEN
                RAISE EXCEPTION 'network dashboard head CAS failed';
            END IF;

            DELETE FROM public.telemetry_dashboard_network_generations
            WHERE client_id = p_client_id
              AND generation = p_expected_generation;
        ELSE
            traffic_selection :=
                public.telemetry_dashboard_effective_traffic_selection(
                    p_client_id
                );
            generation_source_kinds :=
                (traffic_selection).source_kinds;
            generation_interfaces :=
                (traffic_selection).interfaces;

            INSERT INTO public.telemetry_dashboard_traffic_generations (
                client_id, generation, source_kinds,
                interfaces, stream_width
            )
            VALUES (
                p_client_id,
                new_generation,
                generation_source_kinds,
                generation_interfaces,
                cardinality(generation_source_kinds)
            );

            PERFORM public.build_telemetry_dashboard_traffic_generation(
                p_client_id,
                new_generation,
                new_revision,
                generation_source_kinds,
                generation_interfaces
            );

            UPDATE public.telemetry_dashboard_traffic_projection_heads head
            SET traffic_generation = new_generation,
                traffic_revision = new_revision,
                traffic_change = 'generation',
                traffic_change_source_bucket_secs =
                    ARRAY[]::INTEGER[],
                traffic_change_block_start_unix =
                    ARRAY[]::BIGINT[],
                traffic_generation_source_kinds =
                    generation_source_kinds,
                traffic_generation_interfaces = generation_interfaces,
                traffic_stream_width =
                    cardinality(generation_source_kinds),
                traffic_first_at = (
                    SELECT to_timestamp(
                        min(bounds.first_bucket_start_unix)
                    )
                    FROM public.telemetry_dashboard_traffic_generation_bounds
                        bounds
                    WHERE bounds.client_id = p_client_id
                      AND bounds.generation = new_generation
                ),
                traffic_through_at = (
                    SELECT to_timestamp(max(
                        bounds.last_bucket_start_unix
                        + bounds.source_bucket_secs
                    ))
                    FROM public.telemetry_dashboard_traffic_generation_bounds
                        bounds
                    WHERE bounds.client_id = p_client_id
                      AND bounds.generation = new_generation
                )
            WHERE head.client_id = p_client_id
              AND head.traffic_generation = p_expected_generation
              AND head.traffic_revision = p_expected_revision
            RETURNING TRUE INTO flipped;

            IF NOT COALESCE(flipped, FALSE) THEN
                RAISE EXCEPTION 'traffic dashboard head CAS failed';
            END IF;

            DELETE FROM public.telemetry_dashboard_traffic_generations
            WHERE client_id = p_client_id
              AND generation = p_expected_generation;
        END IF;
    ELSE
        IF p_domain = 'resource' THEN
            -- Full-block ownership stays one bounded F16 rebuild. Ordinary
            -- coordinates are installed together below so one owner revision
            -- writes each affected block and tier bound only once.
            FOR block_coordinate IN
                SELECT event_kind, tier, block_start, bucket_start
                FROM unnest(
                    p_event_kind, p_source_bucket_secs,
                    p_block_start_unix, p_bucket_start_unix
                ) coordinate(event_kind, tier, block_start, bucket_start)
                WHERE event_kind = 'full_block'
                ORDER BY tier, block_start, event_kind, bucket_start
            LOOP
                PERFORM public.refresh_telemetry_dashboard_resource_block(
                    p_client_id,
                    p_expected_generation,
                    new_revision,
                    block_coordinate.tier,
                    block_coordinate.block_start
                );
            END LOOP;

            SELECT array_agg(coordinate.tier ORDER BY coordinate.tier,
                             coordinate.bucket_start),
                   array_agg(coordinate.bucket_start ORDER BY coordinate.tier,
                             coordinate.bucket_start)
            INTO coordinate_tiers, coordinate_starts
            FROM unnest(
                p_event_kind, p_source_bucket_secs,
                p_bucket_start_unix
            ) coordinate(event_kind, tier, bucket_start)
            WHERE coordinate.event_kind = 'coordinate';

            IF cardinality(COALESCE(
                   coordinate_tiers, ARRAY[]::INTEGER[]
               )) > 0 THEN
                PERFORM public.refresh_telemetry_dashboard_resource_coordinates(
                    p_client_id,
                    p_expected_generation,
                    new_revision,
                    coordinate_tiers,
                    coordinate_starts
                );
            END IF;

            UPDATE public.telemetry_dashboard_resource_projection_heads head
            SET resource_revision = new_revision,
                resource_change = 'block',
                resource_change_source_bucket_secs =
                    changed_tiers,
                resource_change_block_start_unix =
                    changed_starts,
                resource_first_at = (
                    SELECT to_timestamp(
                        min(bounds.first_bucket_start_unix)
                    )
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
            SELECT head.network_generation_interfaces
            INTO generation_interfaces
            FROM public.telemetry_dashboard_network_projection_heads head
            WHERE head.client_id = p_client_id
              AND head.network_generation = p_expected_generation
              AND head.network_revision = p_expected_revision;

            IF NOT FOUND THEN
                RAISE EXCEPTION 'network dashboard head fence changed';
            END IF;

            FOR block_coordinate IN
                SELECT event_kind, tier, block_start, bucket_start
                FROM unnest(
                    p_event_kind, p_source_bucket_secs,
                    p_block_start_unix, p_bucket_start_unix
                ) coordinate(event_kind, tier, block_start, bucket_start)
                WHERE event_kind = 'full_block'
                ORDER BY tier, block_start, event_kind, bucket_start
            LOOP
                PERFORM public.refresh_telemetry_dashboard_network_block(
                    p_client_id,
                    p_expected_generation,
                    new_revision,
                    generation_interfaces,
                    block_coordinate.tier,
                    block_coordinate.block_start
                );
            END LOOP;

            SELECT array_agg(coordinate.tier ORDER BY coordinate.tier,
                             coordinate.bucket_start),
                   array_agg(coordinate.bucket_start ORDER BY coordinate.tier,
                             coordinate.bucket_start)
            INTO coordinate_tiers, coordinate_starts
            FROM unnest(
                p_event_kind, p_source_bucket_secs,
                p_bucket_start_unix
            ) coordinate(event_kind, tier, bucket_start)
            WHERE coordinate.event_kind = 'coordinate';

            IF cardinality(COALESCE(
                   coordinate_tiers, ARRAY[]::INTEGER[]
               )) > 0 THEN
                PERFORM public.refresh_telemetry_dashboard_network_coordinates(
                    p_client_id,
                    p_expected_generation,
                    new_revision,
                    generation_interfaces,
                    coordinate_tiers,
                    coordinate_starts
                );
            END IF;

            UPDATE public.telemetry_dashboard_network_projection_heads head
            SET network_revision = new_revision,
                network_change = 'block',
                network_change_source_bucket_secs =
                    changed_tiers,
                network_change_block_start_unix =
                    changed_starts,
                network_first_at = (
                    SELECT to_timestamp(
                        min(bounds.first_bucket_start_unix)
                    )
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
            SELECT head.traffic_generation_source_kinds,
                   head.traffic_generation_interfaces
            INTO generation_source_kinds,
                 generation_interfaces
            FROM public.telemetry_dashboard_traffic_projection_heads head
            WHERE head.client_id = p_client_id
              AND head.traffic_generation = p_expected_generation
              AND head.traffic_revision = p_expected_revision;

            IF NOT FOUND THEN
                RAISE EXCEPTION 'traffic dashboard head fence changed';
            END IF;

            -- A full-block rebuild remains one bounded F16 source read.  All
            -- ordinary coordinate changes in this owner revision are patched
            -- into their affected F16 rows setwise, then every affected tier's
            -- bounds are reconciled once.
            FOR block_coordinate IN
                SELECT event_kind, tier, block_start, bucket_start
                FROM unnest(
                    p_event_kind, p_source_bucket_secs,
                    p_block_start_unix, p_bucket_start_unix
                ) coordinate(event_kind, tier, block_start, bucket_start)
                WHERE event_kind = 'full_block'
                ORDER BY tier, block_start, event_kind, bucket_start
            LOOP
                PERFORM public.refresh_telemetry_dashboard_traffic_block(
                    p_client_id,
                    p_expected_generation,
                    new_revision,
                    generation_source_kinds,
                    generation_interfaces,
                    block_coordinate.tier,
                    block_coordinate.block_start
                );
            END LOOP;

            SELECT array_agg(coordinate.tier ORDER BY coordinate.tier,
                             coordinate.bucket_start),
                   array_agg(coordinate.bucket_start ORDER BY coordinate.tier,
                             coordinate.bucket_start)
            INTO coordinate_tiers, coordinate_starts
            FROM unnest(
                p_event_kind, p_source_bucket_secs,
                p_bucket_start_unix
            ) coordinate(event_kind, tier, bucket_start)
            WHERE coordinate.event_kind = 'coordinate';

            IF cardinality(COALESCE(
                   coordinate_tiers, ARRAY[]::INTEGER[]
               )) > 0 THEN
                PERFORM public.refresh_telemetry_dashboard_traffic_coordinates(
                    p_client_id,
                    p_expected_generation,
                    new_revision,
                    generation_source_kinds,
                    generation_interfaces,
                    coordinate_tiers,
                    coordinate_starts
                );
            END IF;

            UPDATE public.telemetry_dashboard_traffic_projection_heads head
            SET traffic_revision = new_revision,
                traffic_change = 'block',
                traffic_change_source_bucket_secs = changed_tiers,
                traffic_change_block_start_unix = changed_starts,
                traffic_first_at = (
                    SELECT to_timestamp(
                        min(bounds.first_bucket_start_unix)
                    )
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
    END IF;

    DELETE FROM public.telemetry_dashboard_block_events event
    WHERE event.event_id = ANY(COALESCE(
        p_captured_block_event_ids, ARRAY[]::BIGINT[]
    ));

    DELETE FROM public.telemetry_dashboard_generation_events event
    WHERE event.event_id = ANY(COALESCE(
        p_captured_generation_event_ids, ARRAY[]::BIGINT[]
    ));

    IF p_change = 'block' THEN
        -- PostgreSQL NOTIFY payloads are bounded. Publish the uncapped owner
        -- coordinate union as one-coordinate fragments. These pg_notify calls
        -- share this publication transaction, so PostgreSQL delivers them in
        -- send order after commit; the final flag exposes the revision only
        -- after every preceding coordinate has been collected.
        FOR block_coordinate IN
            SELECT coordinate.tier,
                   coordinate.block_start,
                   coordinate.ordinality
            FROM unnest(
                changed_tiers, changed_starts
            ) WITH ORDINALITY coordinate(tier, block_start, ordinality)
            ORDER BY coordinate.ordinality
        LOOP
            notice := jsonb_build_object(
                'owner', 'dashboard',
                'client_id', p_client_id,
                'domain', p_domain,
                'change', p_change,
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
            PERFORM pg_notify(
                'vpsman_telemetry_projection',
                notice::TEXT
            );
        END LOOP;
    ELSE
        notice := jsonb_build_object(
            'owner', 'dashboard',
            'client_id', p_client_id,
            'domain', p_domain,
            'change', p_change,
            'generation', new_generation,
            'previous_revision', p_expected_revision,
            'revision', new_revision,
            'source_bucket_secs', ARRAY[]::INTEGER[],
            'block_start_unix', ARRAY[]::BIGINT[],
            'complete', TRUE
        );
        PERFORM pg_notify(
            'vpsman_telemetry_projection',
            notice::TEXT
        );
    END IF;

    RETURN TRUE;
END
$$;

-- Ping owns no range tree here.  Only its exact retained envelope is cached so
-- the common telemetry-start decision remains O(1).
CREATE FUNCTION public.refresh_telemetry_dashboard_ping_heads(
    p_client_ids TEXT[]
)
RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    -- Different retained Ping series share one client envelope.  Serialize
    -- that exact owner after all series-bound locks, then recompute in a new
    -- READ COMMITTED statement snapshot so a waiter sees the prior commit.
    PERFORM head.client_id
    FROM public.telemetry_dashboard_ping_projection_heads head
    WHERE head.client_id = ANY(COALESCE(p_client_ids, ARRAY[]::TEXT[]))
    ORDER BY head.client_id
    FOR UPDATE;

    UPDATE public.telemetry_dashboard_ping_projection_heads head
    SET ping_first_at = source.first_at
    FROM unnest(COALESCE(p_client_ids, ARRAY[]::TEXT[])) requested(client_id)
    LEFT JOIN LATERAL (
        SELECT min(bounds.first_bucket_start) AS first_at
        FROM public.telemetry_ping_series series
        JOIN public.telemetry_dashboard_ping_series_bounds bounds
          ON bounds.series_id = series.id
        WHERE series.client_id = requested.client_id
    ) source ON TRUE
    WHERE head.client_id = requested.client_id
      AND head.ping_first_at IS DISTINCT FROM source.first_at;
END
$$;

-- A series row owns the durable client mapping used by the Ping envelope.
-- Refresh after its dependent rollups/bounds have cascaded away, and refresh
-- both owners when an explicit reassignment changes that mapping.
CREATE FUNCTION public.maintain_telemetry_ping_series_dashboard_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM old_telemetry_ping_series) THEN
        RETURN NULL;
    END IF;

    PERFORM public.refresh_telemetry_dashboard_ping_heads(
        array_agg(DISTINCT prior.client_id)
    )
    FROM old_telemetry_ping_series prior
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = prior.client_id;
    RETURN NULL;
END
$$;

CREATE FUNCTION public.maintain_telemetry_ping_series_dashboard_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM old_telemetry_ping_series prior
        JOIN new_telemetry_ping_series current USING (id)
        WHERE prior.client_id IS DISTINCT FROM current.client_id
    ) THEN
        RETURN NULL;
    END IF;

    PERFORM public.refresh_telemetry_dashboard_ping_heads(
        array_agg(DISTINCT changed.client_id)
    )
    FROM (
        SELECT prior.client_id
        FROM old_telemetry_ping_series prior
        JOIN new_telemetry_ping_series current USING (id)
        WHERE prior.client_id IS DISTINCT FROM current.client_id
        UNION
        SELECT current.client_id
        FROM old_telemetry_ping_series prior
        JOIN new_telemetry_ping_series current USING (id)
        WHERE prior.client_id IS DISTINCT FROM current.client_id
    ) changed;
    RETURN NULL;
END
$$;

CREATE TRIGGER telemetry_ping_series_dashboard_after_delete
AFTER DELETE ON public.telemetry_ping_series
REFERENCING OLD TABLE AS old_telemetry_ping_series
FOR EACH STATEMENT
EXECUTE FUNCTION public.maintain_telemetry_ping_series_dashboard_after_delete();

CREATE TRIGGER telemetry_ping_series_dashboard_after_update
AFTER UPDATE ON public.telemetry_ping_series
REFERENCING OLD TABLE AS old_telemetry_ping_series
            NEW TABLE AS new_telemetry_ping_series
FOR EACH STATEMENT
EXECUTE FUNCTION public.maintain_telemetry_ping_series_dashboard_after_update();

CREATE FUNCTION public.maintain_telemetry_ping_dashboard_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM new_telemetry_ping_rollups) THEN
        RETURN NULL;
    END IF;

    -- Rollup insertion is the producer and this statement trigger is its exact
    -- dashboard consumer.  The bounds primary key is the natural series owner:
    -- its conflict update serializes only the same series and never a synthetic
    -- advisory lock.  New coordinates can only move the client-wide first edge
    -- earlier, so the head advances from the changed series alone; the ordinary
    -- arrival path never rescans every Ping series belonging to that client.
    WITH changed_bounds AS MATERIALIZED (
        INSERT INTO public.telemetry_dashboard_ping_series_bounds AS bounds (
            series_id, first_bucket_start, last_bucket_start
        )
        SELECT rows.series_id,
               min(rows.bucket_start), max(rows.bucket_start)
        FROM new_telemetry_ping_rollups rows
        GROUP BY rows.series_id
        ORDER BY rows.series_id
        ON CONFLICT (series_id) DO UPDATE SET
            first_bucket_start = LEAST(
                bounds.first_bucket_start, EXCLUDED.first_bucket_start
            ),
            last_bucket_start = GREATEST(
                bounds.last_bucket_start, EXCLUDED.last_bucket_start
            )
        WHERE (
            bounds.first_bucket_start, bounds.last_bucket_start
        ) IS DISTINCT FROM (
            LEAST(bounds.first_bucket_start, EXCLUDED.first_bucket_start),
            GREATEST(bounds.last_bucket_start, EXCLUDED.last_bucket_start)
        )
        RETURNING bounds.series_id, bounds.first_bucket_start
    ), changed_clients AS MATERIALIZED (
        SELECT series.client_id,
               min(bounds.first_bucket_start) AS first_bucket_start
        FROM changed_bounds bounds
        JOIN public.telemetry_ping_series series
          ON series.id = bounds.series_id
        GROUP BY series.client_id
    ), locked_heads AS MATERIALIZED (
        SELECT head.client_id
        FROM public.telemetry_dashboard_ping_projection_heads head
        JOIN changed_clients changed
          ON changed.client_id = head.client_id
        WHERE head.ping_first_at IS NULL
           OR changed.first_bucket_start < head.ping_first_at
        ORDER BY head.client_id
        FOR UPDATE OF head
    )
    UPDATE public.telemetry_dashboard_ping_projection_heads head
    SET ping_first_at = CASE
        WHEN head.ping_first_at IS NULL
        THEN changed.first_bucket_start
        ELSE LEAST(head.ping_first_at, changed.first_bucket_start)
    END
    FROM changed_clients changed
    JOIN locked_heads locked USING (client_id)
    WHERE head.client_id = changed.client_id
      AND (
          head.ping_first_at IS NULL
          OR changed.first_bucket_start < head.ping_first_at
      );
    RETURN NULL;
END
$$;

CREATE FUNCTION public.maintain_telemetry_ping_dashboard_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    first_edge_clients TEXT[];
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;
    IF NOT EXISTS (SELECT 1 FROM old_telemetry_ping_rollups) THEN
        RETURN NULL;
    END IF;

    -- Retention/delete owns the non-monotonic edge repair.  Interior points do
    -- not change either compact edge, so lock only a series whose cached first
    -- or last coordinate was actually removed.  Natural series order remains
    -- the sole lock order.
    PERFORM bounds.series_id
    FROM public.telemetry_dashboard_ping_series_bounds bounds
    JOIN (
        SELECT DISTINCT rows.series_id
        FROM old_telemetry_ping_rollups rows
        JOIN public.telemetry_ping_series series
          ON series.id = rows.series_id
        JOIN public.telemetry_dashboard_clients dashboard_client
          ON dashboard_client.client_id = series.client_id
    ) affected
      ON affected.series_id = bounds.series_id
    WHERE EXISTS (
        SELECT 1
        FROM old_telemetry_ping_rollups edge
        WHERE edge.series_id = bounds.series_id
          AND edge.bucket_start IN (
              bounds.first_bucket_start, bounds.last_bucket_start
          )
    )
    ORDER BY bounds.series_id
    FOR UPDATE OF bounds;

    -- Only deleting a first edge can change the client-wide Ping head. Capture
    -- those exact clients while the corresponding series-bound rows are held;
    -- last-edge-only repairs never contend on the client owner.
    SELECT array_agg(DISTINCT series.client_id ORDER BY series.client_id)
    INTO first_edge_clients
    FROM old_telemetry_ping_rollups prior
    JOIN public.telemetry_ping_series series ON series.id = prior.series_id
    JOIN public.telemetry_dashboard_clients dashboard_client
      ON dashboard_client.client_id = series.client_id
    JOIN public.telemetry_dashboard_ping_series_bounds bounds
      ON bounds.series_id = prior.series_id
     AND prior.bucket_start = bounds.first_bucket_start;

    PERFORM public.refresh_telemetry_dashboard_ping_series_bound_edges(
        edge.series_id
    )
    FROM (
        SELECT DISTINCT prior.series_id
        FROM old_telemetry_ping_rollups prior
        JOIN public.telemetry_ping_series series
          ON series.id = prior.series_id
        JOIN public.telemetry_dashboard_clients dashboard_client
          ON dashboard_client.client_id = series.client_id
        JOIN public.telemetry_dashboard_ping_series_bounds bounds
          ON bounds.series_id = prior.series_id
         AND prior.bucket_start IN (
             bounds.first_bucket_start, bounds.last_bucket_start
         )
    ) edge;
    PERFORM public.refresh_telemetry_dashboard_ping_heads(
        first_edge_clients
    );
    RETURN NULL;
END
$$;

CREATE FUNCTION public.maintain_telemetry_ping_dashboard_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF public.telemetry_dashboard_ownership_transfer_requested() THEN
        RETURN NULL;
    END IF;
    IF NOT EXISTS (
        (SELECT series_id, bucket_secs, bucket_start
         FROM old_telemetry_ping_rollups
         EXCEPT
         SELECT series_id, bucket_secs, bucket_start
         FROM new_telemetry_ping_rollups)
        UNION ALL
        (SELECT series_id, bucket_secs, bucket_start
         FROM new_telemetry_ping_rollups
         EXCEPT
         SELECT series_id, bucket_secs, bucket_start
         FROM old_telemetry_ping_rollups)
    ) THEN
        RETURN NULL;
    END IF;

    -- Metric-only conflict updates retain their complete source identity and
    -- therefore perform no bound or client-envelope work.
    PERFORM bounds.series_id
    FROM public.telemetry_dashboard_ping_series_bounds bounds
    JOIN (
        SELECT DISTINCT changed.series_id
        FROM (
            (SELECT series_id, bucket_secs, bucket_start
             FROM old_telemetry_ping_rollups
             EXCEPT
             SELECT series_id, bucket_secs, bucket_start
             FROM new_telemetry_ping_rollups)
            UNION ALL
            (SELECT series_id, bucket_secs, bucket_start
             FROM new_telemetry_ping_rollups
             EXCEPT
             SELECT series_id, bucket_secs, bucket_start
             FROM old_telemetry_ping_rollups)
        ) changed
    ) affected ON affected.series_id = bounds.series_id
    ORDER BY bounds.series_id
    FOR UPDATE OF bounds;
    PERFORM public.refresh_telemetry_dashboard_ping_series_bound_edges(
        edge.series_id
    )
    FROM (
        SELECT DISTINCT vanished.series_id
        FROM (
            SELECT series_id, bucket_secs, bucket_start
            FROM old_telemetry_ping_rollups
            EXCEPT
            SELECT series_id, bucket_secs, bucket_start
            FROM new_telemetry_ping_rollups
        ) vanished
        JOIN public.telemetry_dashboard_ping_series_bounds bounds
          ON bounds.series_id = vanished.series_id
         AND vanished.bucket_start IN (
             bounds.first_bucket_start, bounds.last_bucket_start
         )
    ) edge;
    INSERT INTO public.telemetry_dashboard_ping_series_bounds (
        series_id, first_bucket_start, last_bucket_start
    )
    SELECT rows.series_id, min(rows.bucket_start), max(rows.bucket_start)
    FROM (
        SELECT series_id, bucket_secs, bucket_start
        FROM new_telemetry_ping_rollups
        EXCEPT
        SELECT series_id, bucket_secs, bucket_start
        FROM old_telemetry_ping_rollups
    ) rows
    GROUP BY rows.series_id
    ORDER BY rows.series_id
    ON CONFLICT (series_id) DO UPDATE SET
        first_bucket_start = LEAST(
            telemetry_dashboard_ping_series_bounds.first_bucket_start,
            EXCLUDED.first_bucket_start
        ),
        last_bucket_start = GREATEST(
            telemetry_dashboard_ping_series_bounds.last_bucket_start,
            EXCLUDED.last_bucket_start
        )
    WHERE (
        telemetry_dashboard_ping_series_bounds.first_bucket_start,
        telemetry_dashboard_ping_series_bounds.last_bucket_start
    ) IS DISTINCT FROM (
        LEAST(
            telemetry_dashboard_ping_series_bounds.first_bucket_start,
            EXCLUDED.first_bucket_start
        ),
        GREATEST(
            telemetry_dashboard_ping_series_bounds.last_bucket_start,
            EXCLUDED.last_bucket_start
        )
    );
    PERFORM public.refresh_telemetry_dashboard_ping_heads(
        array_agg(DISTINCT series.client_id)
    )
    FROM (
        SELECT DISTINCT changed.series_id
        FROM (
            (SELECT series_id, bucket_secs, bucket_start
             FROM old_telemetry_ping_rollups
             EXCEPT
             SELECT series_id, bucket_secs, bucket_start
             FROM new_telemetry_ping_rollups)
            UNION ALL
            (SELECT series_id, bucket_secs, bucket_start
             FROM new_telemetry_ping_rollups
             EXCEPT
             SELECT series_id, bucket_secs, bucket_start
             FROM old_telemetry_ping_rollups)
        ) changed
    ) rows
    JOIN public.telemetry_ping_series series ON series.id = rows.series_id;
    RETURN NULL;
END
$$;

CREATE TRIGGER telemetry_ping_rollups_dashboard_after_insert
AFTER INSERT ON public.telemetry_ping_rollups
REFERENCING NEW TABLE AS new_telemetry_ping_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.maintain_telemetry_ping_dashboard_after_insert();

CREATE TRIGGER telemetry_ping_rollups_dashboard_after_delete
AFTER DELETE ON public.telemetry_ping_rollups
REFERENCING OLD TABLE AS old_telemetry_ping_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.maintain_telemetry_ping_dashboard_after_delete();

CREATE TRIGGER telemetry_ping_rollups_dashboard_after_update
AFTER UPDATE ON public.telemetry_ping_rollups
REFERENCING OLD TABLE AS old_telemetry_ping_rollups
            NEW TABLE AS new_telemetry_ping_rollups
FOR EACH STATEMENT
EXECUTE FUNCTION public.maintain_telemetry_ping_dashboard_after_update();
