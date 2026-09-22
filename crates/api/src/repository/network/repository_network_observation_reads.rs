use super::OBSERVATION_COLUMNS;

/// Select exact evidence before reconstructing wide history. A physical
/// automatic series is contained in one logical rank group (even when graph
/// groups omit topology identity). Its newest K eligible rows therefore contain
/// every possible winner of that group's newest K. Payload validity and all
/// row filters must precede this physical limit; manual evidence and the active
/// latest fallback still compete in the same final rank, with no refill.
pub(super) fn query(for_topology: bool) -> String {
    let visibility = if for_topology {
        r#"
        NOT $9 OR (
            NOT EXISTS (
                SELECT 1 FROM visible_clients suspended
                WHERE suspended.status = 'suspended'
                  AND suspended.id IN (observation.client_id, observation.peer_client_id)
            )
            AND NOT EXISTS (
                SELECT 1 FROM tunnel_plans plan
                JOIN visible_clients suspended ON suspended.status = 'suspended'
                  AND suspended.id IN (plan.left_client_id, plan.right_client_id)
                WHERE plan.id = observation.plan_id
            )
        )
        "#
    } else {
        r#"
        NOT $9 OR (
            EXISTS (SELECT 1 FROM visible_clients WHERE id = observation.client_id AND status <> 'suspended')
            AND (observation.peer_client_id IS NULL OR EXISTS (
                SELECT 1 FROM visible_clients WHERE id = observation.peer_client_id AND status <> 'suspended'
            ))
            AND EXISTS (
                SELECT 1 FROM tunnel_plans plan
                WHERE plan.id = observation.plan_id AND plan.deleted_at IS NULL
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id = plan.left_client_id AND endpoint.status = 'suspended'
                  )
                  AND NOT EXISTS (
                      SELECT 1 FROM visible_clients endpoint
                      WHERE endpoint.id = plan.right_client_id AND endpoint.status = 'suspended'
                  )
            )
        )
        "#
    };
    let kind_scope = if for_topology {
        "AND observation.kind IN ('tunnel_reachability', 'network_speed_test', 'network_status')"
    } else {
        ""
    };
    let identity_partition = if for_topology {
        ""
    } else {
        "topology_identity_hash,"
    };
    let rank_order = if for_topology { "" } else { "evidence_rank," };

    format!(
        r#"
WITH automatic_series AS (
    SELECT observation.*
    FROM network_observation_series observation
    WHERE (cardinality($3::uuid[]) = 0 OR observation.plan_id = ANY($3::uuid[]))
      AND ($4::text IS NULL OR observation.client_id = $4 OR observation.peer_client_id = $4)
      AND ($5::text IS NULL OR $5 = 'automatic')
      AND ($6::text IS NULL OR $6 = 'tunnel_reachability')
      AND ({visibility})
), manual_keys AS (
    -- There is no physical-series catalogue for manual job evidence. Keep its
    -- exact filtered key pass; do not truncate it before the common rank.
    SELECT observation.id, observation.plan_id, observation.topology_identity_hash,
           observation.kind, COALESCE(observation.endpoint_side, observation.client_id) AS endpoint,
           observation.observed_at
    FROM network_observations observation
    WHERE observation.source = 'manual'
      AND observation.observed_at >= to_timestamp($1)
      AND observation.observed_at <= to_timestamp($2)
      AND (cardinality($3::uuid[]) = 0 OR observation.plan_id = ANY($3::uuid[]))
      AND ($4::text IS NULL OR observation.client_id = $4 OR observation.peer_client_id = $4)
      AND ($5::text IS NULL OR observation.source = $5)
      AND ($6::text IS NULL OR observation.kind = $6)
      {kind_scope}
      AND (
          $7::text IS NULL
          OR ($7 = 'healthy' AND observation.healthy IS TRUE)
          OR ($7 = 'unhealthy' AND observation.healthy IS FALSE)
          OR ($7 = 'unknown' AND observation.healthy IS NULL)
      )
      AND (
          $8::text IS NULL
          OR concat_ws(' ', observation.client_id, observation.peer_client_id,
              observation.plan_name, observation.interface_name, observation.target,
              observation.reason, observation.kind, observation.source) ILIKE '%' || $8 || '%'
      )
      AND ({visibility})
), rank_sources AS MATERIALIZED (
    -- Resolve the existing grouping tuples once, before ranking sample keys.
    -- IDs are query-local and collision-free, not persisted evidence identities.
    -- Carry the existing series record into its producer rather than joining
    -- candidate rows back to a materialized series catalogue.
    SELECT sources.series_record, sources.series_id, sources.observation_id, sources.observed_at,
           dense_rank() OVER (
               ORDER BY plan_id, {identity_partition} kind, endpoint
           ) AS group_id
    FROM (
        SELECT ROW(automatic_series.*)::public.network_observation_series AS series_record,
               id AS series_id, NULL::uuid AS observation_id,
               NULL::timestamptz AS observed_at, plan_id, {identity_partition}
               'tunnel_reachability'::text AS kind, endpoint_side AS endpoint
        FROM automatic_series
        UNION ALL
        SELECT NULL::public.network_observation_series, NULL::bigint,
               id, observed_at, plan_id, {identity_partition} kind, endpoint
        FROM manual_keys
    ) sources
), grouped_series AS MATERIALIZED (
    SELECT (series_record).*, group_id FROM rank_sources WHERE series_id IS NOT NULL
), automatic_raw AS (
    SELECT series.id AS series_id, series.group_id, bounded.*
    FROM grouped_series series
    CROSS JOIN LATERAL (
        SELECT locator.id, locator.plan_name, locator.observed_at,
               locator.received_at, raw.observation
        FROM network_observations locator
        CROSS JOIN LATERAL (
            -- Preserve a keyed payload lookup per ordered locator. Without
            -- this boundary, a hash join can scan the entire sample table for
            -- every physical series before the outer LIMIT takes effect.
            SELECT payload.observation
            FROM telemetry_samples sample
            CROSS JOIN LATERAL (
                SELECT sample.payload -> 'tunnel_reachability'
                             -> (locator.automatic_payload_ordinal::integer - 1) AS observation
                OFFSET 0
            ) payload
            WHERE sample.id = locator.automatic_sample_id
              AND payload.observation IS NOT NULL
              AND (payload.observation ->> 'id')::uuid = locator.id
              AND (
                  $7::text IS NULL
                  OR ($7 = 'healthy' AND (payload.observation ->> 'healthy')::boolean IS TRUE)
                  OR ($7 = 'unhealthy' AND (payload.observation ->> 'healthy')::boolean IS FALSE)
                  OR ($7 = 'unknown' AND (payload.observation ->> 'healthy')::boolean IS NULL)
              )
              AND (
                  $8::text IS NULL
                  OR concat_ws(' ', series.client_id, series.peer_client_id,
                      locator.plan_name, series.interface_name, series.target,
                      payload.observation ->> 'reason', 'tunnel_reachability', 'automatic') ILIKE '%' || $8 || '%'
              )
            OFFSET 0
        ) raw
        WHERE locator.automatic_series_id = series.id
          AND locator.automatic_series_id IS NOT NULL
          AND locator.source = 'automatic'
          AND locator.observed_at >= to_timestamp($1)
          AND locator.observed_at <= to_timestamp($2)
        ORDER BY locator.observed_at DESC, locator.id DESC
        LIMIT $10
    ) bounded
), automatic_latest AS (
    SELECT latest.*, series.group_id, latest AS latest_record
    FROM grouped_series series
    JOIN network_observation_latest latest ON latest.series_id = series.id
    WHERE series.active
      AND latest.observed_at >= to_timestamp($1)
      AND latest.observed_at <= to_timestamp($2)
      -- Exclude by the entire UUID registry, not the bounded/range-filtered
      -- candidates. A present but invalid/filtered locator still shadows latest.
      AND NOT EXISTS (
          SELECT 1 FROM network_observations locator WHERE locator.id = latest.observation_id
      )
      AND (
          $7::text IS NULL
          OR ($7 = 'healthy' AND latest.healthy IS TRUE)
          OR ($7 = 'unhealthy' AND latest.healthy IS FALSE)
          OR ($7 = 'unknown' AND latest.healthy IS NULL)
      )
      AND (
          $8::text IS NULL
          OR concat_ws(' ', series.client_id, series.peer_client_id,
              series.plan_name, series.interface_name, series.target,
              latest.reason, 'tunnel_reachability', 'automatic') ILIKE '%' || $8 || '%'
      )
), ranked AS (
    -- Carry already-read payloads forward; reattaching materialized candidates
    -- after selection can multiply work under generic-plan row estimates.
    -- Origin tags name disjoint existing sources: raw=1, latest=2, manual=3.
    -- Registry UUID uniqueness and latest's global anti-join ensure no overlap.
    SELECT keys.*,
           row_number() OVER (
               PARTITION BY group_id ORDER BY observed_at DESC, id DESC
           ) AS evidence_rank
    FROM (
        SELECT raw.id, raw.group_id, raw.observed_at, 1 AS origin,
               raw.series_id, raw.plan_name, raw.received_at, raw.observation,
               NULL::public.network_observation_latest AS latest_record
        FROM automatic_raw raw
        UNION ALL
        SELECT latest.observation_id, latest.group_id, latest.observed_at, 2,
               latest.series_id, NULL::text, NULL::timestamptz, NULL::jsonb,
               latest.latest_record
        FROM automatic_latest latest
        UNION ALL
        SELECT observation_id, group_id, observed_at, 3, NULL::bigint,
               NULL::text, NULL::timestamptz, NULL::jsonb,
               NULL::public.network_observation_latest
        FROM rank_sources
        WHERE observation_id IS NOT NULL
    ) keys
), selected AS MATERIALIZED (
    -- Keep the final selection boundary: only winning payloads may have their
    -- numeric metric fields decoded by the hydration branches below.
    SELECT *
    FROM ranked
    WHERE evidence_rank <= $10
    -- Existing response ordering uses the projected timestamp text; ranking
    -- itself above deliberately keeps native timestamp/UUID ordering.
    ORDER BY {rank_order} observed_at::text DESC, id DESC
    LIMIT $11
), hydrated AS (
    SELECT observation.id, observation.job_id, observation.client_id, observation.seq,
           observation.kind, observation.source, observation.role, observation.plan_id,
           observation.topology_identity_hash, observation.plan_name, observation.interface_name,
           observation.peer_client_id, observation.target, observation.endpoint_side,
           observation.address_family, observation.stale_after_secs, observation.healthy,
           observation.transmitted, observation.received, observation.latency_min_ms,
           observation.latency_avg_ms, observation.latency_max_ms, observation.latency_mdev_ms,
           observation.packet_loss_ratio, observation.reason, observation.throughput_mbps,
           observation.bytes, observation.metadata, observation.observed_at, observation.received_at,
           selected.evidence_rank
    FROM selected JOIN network_observations observation USING (id)
    WHERE observation.source = 'manual' AND selected.origin = 3
    UNION ALL
    SELECT raw.id, NULL::uuid, series.client_id, NULL::integer,
           'tunnel_reachability', 'automatic', 'endpoint', series.plan_id,
           series.topology_identity_hash, raw.plan_name, series.interface_name,
           series.peer_client_id, series.target, series.endpoint_side, series.address_family,
           (raw.observation ->> 'stale_after_secs')::bigint,
           (raw.observation ->> 'healthy')::boolean,
           (raw.observation ->> 'transmitted')::integer,
           (raw.observation ->> 'received')::integer,
           (raw.observation ->> 'latency_min_ms')::double precision,
           (raw.observation ->> 'latency_avg_ms')::double precision,
           (raw.observation ->> 'latency_max_ms')::double precision,
           (raw.observation ->> 'latency_mdev_ms')::double precision,
           (raw.observation ->> 'packet_loss_ratio')::double precision,
           raw.observation ->> 'reason', NULL::double precision, NULL::bigint,
           jsonb_build_object('type', 'tunnel_reachability', 'source', 'automatic'),
           raw.observed_at, raw.received_at, raw.evidence_rank
    FROM selected raw
    JOIN network_observation_series series ON series.id = raw.series_id
    WHERE raw.origin = 1
    UNION ALL
    SELECT latest.observation_id, NULL::uuid, series.client_id, NULL::integer,
           'tunnel_reachability', 'automatic', 'endpoint', series.plan_id,
           series.topology_identity_hash, series.plan_name, series.interface_name,
           series.peer_client_id, series.target, series.endpoint_side, series.address_family,
           latest.stale_after_secs, latest.healthy, latest.transmitted, latest.received,
           latest.latency_min_ms, latest.latency_avg_ms, latest.latency_max_ms,
           latest.latency_mdev_ms, latest.packet_loss_ratio, latest.reason,
           NULL::double precision, NULL::bigint, latest.metadata,
           latest.observed_at, latest.received_at, selected.evidence_rank
    FROM selected
    JOIN network_observation_series series ON series.id = selected.series_id
    CROSS JOIN LATERAL (SELECT (selected.latest_record).*) latest
    WHERE selected.origin = 2
)
SELECT {OBSERVATION_COLUMNS}
FROM hydrated
ORDER BY {rank_order} observed_at DESC, id DESC
"#
    )
}
