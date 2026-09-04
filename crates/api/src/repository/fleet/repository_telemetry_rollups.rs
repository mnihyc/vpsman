use std::collections::{HashMap, HashSet};

use crate::{
    model::{
        TelemetryNetworkRateView, TelemetryRollupView, TelemetrySampleView,
        TelemetryTunnelAdapterHealthView, TelemetryTunnelView,
    },
    model_alert_policies::NetworkRateInterfaceSelection,
    repository::Repository,
    util::compare_timestamps_desc,
};
use anyhow::Result;
use sqlx::Row;

const TELEMETRY_LIST_LIMIT_MAX: i64 = 50_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LatestNetworkRateVisibility {
    AdmittedOnly,
    SingleVpsDetail,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TunnelCounterVisibility {
    AdmittedOnly,
    SingleVpsDetail,
}

// Resource, network-rate, and Ping charts share the raw telemetry envelope.
// Their source selection needs the first retained observation without walking
// retained history; traffic has its own canonical ledger and is deliberately
// excluded. These bounds are maintained in the same transactions as their
// source rows. The only physical-row probe left here is the earliest accepted
// raw envelope for each requested client, which is one bounded
// `(client_id, observed_at)` index seek.
pub(crate) const RAW_TELEMETRY_COVERS_RANGE_START_SQL: &str = r#"
WITH requested AS MATERIALIZED (
    SELECT DISTINCT requested.client_id
    FROM UNNEST($1::TEXT[]) AS requested(client_id)
), retained_bounds AS MATERIALIZED (
    SELECT
        requested.client_id,
        LEAST(
            dashboard.resource_first_at,
            dashboard.network_first_at,
            dashboard.ping_first_at
        ) AS minute_start
    FROM requested
    LEFT JOIN telemetry_dashboard_projection_heads dashboard
      ON dashboard.client_id = requested.client_id
), raw_bounds AS MATERIALIZED (
    SELECT requested.client_id, first_raw.raw_start
    FROM requested
    LEFT JOIN LATERAL (
        SELECT sample.observed_at AS raw_start
        FROM telemetry_samples sample
        LEFT JOIN telemetry_projection_heads projection
          ON projection.client_id = sample.client_id
        WHERE sample.client_id = requested.client_id
          AND sample.accepted_seq <= projection.projected_seq
        ORDER BY sample.observed_at ASC
        LIMIT 1
    ) first_raw ON TRUE
)
SELECT COALESCE(bool_and(
    retained.minute_start IS NULL
    OR (
        raw.raw_start IS NOT NULL
        AND raw.raw_start <= GREATEST(
            retained.minute_start,
            to_timestamp($2)
        )
    )
), TRUE)
FROM requested
LEFT JOIN retained_bounds retained USING (client_id)
LEFT JOIN raw_bounds raw USING (client_id)
"#;

// Raw retained-network export is candidate driven. Each page first takes no
// more than the caller's public limit from the global effective-time key; a
// separate bounded lookup then fetches payload and one predecessor per key.
// The repository advances over every examined key and requests another page
// when reset/decrease candidates did not fill the public result.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RawTelemetryNetworkRateCursor {
    latest_observed_at: chrono::DateTime<chrono::Utc>,
    client_id: String,
    interface: String,
    bucket_start: chrono::DateTime<chrono::Utc>,
    bucket_secs: i32,
}

pub(crate) fn raw_telemetry_network_rate_candidate_keys_sql(
    has_client_id: bool,
    has_interface: bool,
    has_bucket_secs: bool,
    has_visible_client_ids: bool,
    has_cursor: bool,
    page_limit: usize,
) -> String {
    // This value is typed and bounded before interpolation. Keeping the LIMIT
    // visible in the statement lets a generic prepared plan cost the physical
    // index stop at its real page cardinality instead of costing five years of
    // retained rows for an unknown parameter.
    let page_limit = page_limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX as usize);
    let mut next_parameter = 1;
    let client_parameter = has_client_id.then(|| {
        let parameter = next_parameter;
        next_parameter += 1;
        parameter
    });
    let interface_parameter = has_interface.then(|| {
        let parameter = next_parameter;
        next_parameter += 1;
        parameter
    });
    let bucket_parameter = has_bucket_secs.then(|| {
        let parameter = next_parameter;
        next_parameter += 1;
        parameter
    });
    let visible_client_ids_parameter = has_visible_client_ids.then(|| {
        let parameter = next_parameter;
        next_parameter += 1;
        parameter
    });
    let cursor_parameters = has_cursor.then(|| {
        (
            next_parameter,
            next_parameter + 1,
            next_parameter + 2,
            next_parameter + 3,
            next_parameter + 4,
        )
    });
    let cursor_progress_expression = cursor_parameters
        .map(|(latest, client, interface, bucket, bucket_secs)| {
            format!(
                r#"candidate.latest_observed_at < ${latest}::TIMESTAMPTZ
        OR (
            candidate.latest_observed_at = ${latest}::TIMESTAMPTZ
            AND candidate.client_id > ${client}::TEXT
        )
        OR (
            candidate.latest_observed_at = ${latest}::TIMESTAMPTZ
            AND candidate.client_id = ${client}::TEXT
            AND candidate.interface > ${interface}::TEXT
        )
        OR (
            candidate.latest_observed_at = ${latest}::TIMESTAMPTZ
            AND candidate.client_id = ${client}::TEXT
            AND candidate.interface = ${interface}::TEXT
            AND candidate.bucket_start < ${bucket}::TIMESTAMPTZ
        )
        OR (
            candidate.latest_observed_at = ${latest}::TIMESTAMPTZ
            AND candidate.client_id = ${client}::TEXT
            AND candidate.interface = ${interface}::TEXT
            AND candidate.bucket_start = ${bucket}::TIMESTAMPTZ
            AND candidate.bucket_secs < ${bucket_secs}::INTEGER
        )"#
            )
        })
        .unwrap_or_else(|| "TRUE".to_string());
    // Each physical owner takes its own ordered public-size page before any
    // policy work. This is the only shape that lets PostgreSQL stop the two
    // retained indexes after the requested keys instead of expanding years of
    // the composite point view in order to join admission. Current admission
    // is evaluated once for the resulting bounded page below; the repository
    // advances past rejected keys and refills just as it does for resets.
    let mut candidate_filters = Vec::with_capacity(4);
    if let Some(parameter) = client_parameter {
        candidate_filters.push(format!("candidate.client_id = ${parameter}"));
    }
    if let Some(parameter) = interface_parameter {
        candidate_filters.push(format!("candidate.interface = ${parameter}"));
    }
    if let Some(parameter) = bucket_parameter {
        candidate_filters.push(format!("candidate.bucket_secs = ${parameter}"));
    }
    if let Some(parameter) = visible_client_ids_parameter {
        candidate_filters.push(format!("candidate.client_id = ANY(${parameter}::TEXT[])"));
    }

    let order = "candidate.latest_observed_at DESC,\n             candidate.client_id ASC,\n             candidate.interface ASC,\n             candidate.bucket_start DESC,\n             candidate.bucket_secs DESC";
    // Minute and coarse are one logical retained owner. Keeping them under a
    // single ordered LIMIT lets PostgreSQL use Merge Append and stop after the
    // global page plus one look-ahead tuple; limiting them independently would
    // read two full pages before throwing half away.
    let projected_raw_client_ids = if let Some(parameter) = client_parameter {
        format!("ARRAY[${parameter}::TEXT]")
    } else if let Some(parameter) = visible_client_ids_parameter {
        format!("${parameter}::TEXT[]")
    } else {
        "NULL::TEXT[]".to_string()
    };
    let physical_sources = [
        r#"SELECT rate.client_id, rate.interface, rate.bucket_start,
                  60::INTEGER AS bucket_secs, rate.latest_observed_at,
                  1::SMALLINT AS source_priority
           FROM telemetry_network_rates_minute rate
           UNION ALL
           SELECT rate.client_id, rate.interface, rate.bucket_start,
                  rate.bucket_secs, rate.latest_observed_at,
                  1::SMALLINT AS source_priority
           FROM telemetry_network_rates_coarse rate"#
            .to_string(),
        r#"SELECT sample.client_id, sample.interface,
                  sample.observed_at AS bucket_start,
                  60::INTEGER AS bucket_secs, sample.latest_observed_at,
                  2::SMALLINT AS source_priority
           FROM traffic_counter_samples sample
           WHERE sample.source_kind = 'host'
             AND NOT sample.inbound_promoted"#
            .to_string(),
        format!(
            r#"SELECT suffix.client_id, suffix.interface, suffix.bucket_start,
                  suffix.bucket_secs, suffix.latest_observed_at,
                  3::SMALLINT AS source_priority
           FROM telemetry_projected_raw_network_minutes_source(
               {projected_raw_client_ids}
           ) suffix"#
        ),
    ];
    let candidate_branch = |suffix: &[String], parenthesized: bool| {
        let mut filters = candidate_filters.clone();
        filters.extend_from_slice(suffix);
        let where_clause = if filters.is_empty() {
            String::new()
        } else {
            format!("WHERE {}\n", filters.join("\n  AND "))
        };
        let source_pages = physical_sources
            .iter()
            .map(|source| {
                format!(
                    r#"(
        SELECT candidate.*
        FROM (
            {source}
        ) candidate
        {where_clause}ORDER BY {order}
        LIMIT {page_limit}
    )"#
                )
            })
            .collect::<Vec<_>>()
            .join("\n    UNION ALL\n    ");
        // A projected raw minute shadows either durable representation. The
        // three logical owner pages (four physical relations) are already
        // bounded, so this reduction cannot become a retained-history sort.
        let select = format!(
            r#"SELECT normalized.client_id,
           normalized.interface,
           normalized.bucket_start,
           normalized.bucket_secs,
           normalized.latest_observed_at
    FROM (
        SELECT DISTINCT ON (
                   source.client_id,
                   source.interface,
                   source.bucket_start,
                   source.bucket_secs
               )
               source.*
        FROM (
            {source_pages}
        ) source
        ORDER BY source.client_id,
                 source.interface,
                 source.bucket_start,
                 source.bucket_secs,
                 source.source_priority DESC
    ) normalized
    ORDER BY normalized.latest_observed_at DESC,
             normalized.client_id ASC,
             normalized.interface ASC,
             normalized.bucket_start DESC,
             normalized.bucket_secs DESC
    LIMIT {page_limit}"#
        );
        if parenthesized {
            format!("({select})")
        } else {
            select
        }
    };

    let candidate_page =
        if let Some((latest, client, interface, bucket, bucket_secs)) = cursor_parameters {
            // The public key mixes descending timestamps with ascending text.
            // These disjoint branches are its exact strict suffix; a uniform ROW
            // comparison would skip or repeat keys at equal effective times.
            let mut suffixes = vec![vec![format!(
                "candidate.latest_observed_at < ${latest}::TIMESTAMPTZ"
            )]];
            if !has_client_id {
                suffixes.push(vec![
                    format!("candidate.latest_observed_at = ${latest}::TIMESTAMPTZ"),
                    format!("candidate.client_id > ${client}::TEXT"),
                ]);
            }
            if !has_interface {
                suffixes.push(vec![
                    format!("candidate.latest_observed_at = ${latest}::TIMESTAMPTZ"),
                    format!("candidate.client_id = ${client}::TEXT"),
                    format!("candidate.interface > ${interface}::TEXT"),
                ]);
            }
            suffixes.push(vec![
                format!("candidate.latest_observed_at = ${latest}::TIMESTAMPTZ"),
                format!("candidate.client_id = ${client}::TEXT"),
                format!("candidate.interface = ${interface}::TEXT"),
                format!("candidate.bucket_start < ${bucket}::TIMESTAMPTZ"),
            ]);
            if !has_bucket_secs {
                suffixes.push(vec![
                    format!("candidate.latest_observed_at = ${latest}::TIMESTAMPTZ"),
                    format!("candidate.client_id = ${client}::TEXT"),
                    format!("candidate.interface = ${interface}::TEXT"),
                    format!("candidate.bucket_start = ${bucket}::TIMESTAMPTZ"),
                    format!("candidate.bucket_secs < ${bucket_secs}::INTEGER"),
                ]);
            }
            let branches = suffixes
                .iter()
                .map(|suffix| candidate_branch(suffix, true))
                .collect::<Vec<_>>()
                .join("\n    UNION ALL\n    ");
            format!(
                r#"SELECT suffix.*
    FROM (
    {branches}
    ) suffix
    ORDER BY suffix.latest_observed_at DESC,
             suffix.client_id ASC,
             suffix.interface ASC,
             suffix.bucket_start DESC,
             suffix.bucket_secs DESC
    LIMIT {page_limit}"#
            )
        } else {
            candidate_branch(&[], false)
        };

    format!(
        r#"WITH candidate_keys AS MATERIALIZED (
    {candidate_page}
), resolved_interface_policies AS MATERIALIZED (
    SELECT policy.*
    FROM public.resolve_telemetry_interface_policies(ARRAY(
        SELECT DISTINCT candidate.client_id
        FROM candidate_keys candidate
        ORDER BY candidate.client_id
    )) policy
), admitted_keys AS MATERIALIZED (
    SELECT DISTINCT candidate.client_id, candidate.interface
    FROM candidate_keys candidate
    JOIN resolved_interface_policies policy
      ON policy.client_id = candidate.client_id
    WHERE public.telemetry_interface_is_admitted_resolved(
        policy.admission_mode,
        policy.interface_patterns,
        policy.managed_tunnel_interfaces,
        'host',
        candidate.interface
    )
)
SELECT
    candidate.client_id,
    candidate.interface,
    candidate.bucket_start,
    candidate.bucket_secs,
    candidate.latest_observed_at,
    EXISTS (
        SELECT 1
        FROM admitted_keys admitted
        WHERE admitted.client_id = candidate.client_id
          AND admitted.interface = candidate.interface
    ) AS admitted,
    ({cursor_progress_expression}) AS page_cursor_strictly_after
FROM candidate_keys candidate
ORDER BY candidate.latest_observed_at DESC,
         candidate.client_id ASC,
         candidate.interface ASC,
         candidate.bucket_start DESC,
         candidate.bucket_secs DESC
"#
    )
}

// Candidate keys are already page bounded. Resolve their exact payloads and
// strict predecessors from physical owners so every retained/raw lookup can
// stop on its natural index; the projected suffix is evaluated once per page.
pub(crate) fn raw_telemetry_network_rate_payload_sql(has_bucket_secs: bool) -> String {
    let retained_predecessor_bucket_filter = if has_bucket_secs {
        "          AND predecessor.bucket_secs = candidate.bucket_secs\n"
    } else {
        ""
    };
    let minute_predecessor_bucket_filter = if has_bucket_secs {
        "          AND candidate.bucket_secs = 60\n"
    } else {
        ""
    };

    format!(
        r#"WITH candidate_keys AS MATERIALIZED (
    SELECT
        candidate.client_id,
        candidate.interface,
        candidate.bucket_start,
        candidate.bucket_secs,
        candidate.ordinal
    FROM UNNEST(
        $1::TEXT[],
        $2::TEXT[],
        $3::TIMESTAMPTZ[],
        $4::INTEGER[]
    ) WITH ORDINALITY AS candidate(
        client_id,
        interface,
        bucket_start,
        bucket_secs,
        ordinal
    )
), candidate_streams AS MATERIALIZED (
    SELECT DISTINCT candidate.client_id, candidate.interface
    FROM candidate_keys candidate
), stream_keys AS MATERIALIZED (
    SELECT candidate.client_id, candidate.interface,
           stream.first_unpromoted_observed_at
    FROM candidate_streams candidate
    LEFT JOIN traffic_counter_streams stream
      ON stream.client_id = candidate.client_id
     AND stream.source_kind = 'host'
     AND stream.interface = candidate.interface
), projected_suffix AS MATERIALIZED (
    SELECT suffix.*
    FROM telemetry_projected_raw_network_minutes_source($1::TEXT[]) suffix
    JOIN candidate_streams candidate
      ON candidate.client_id = suffix.client_id
     AND candidate.interface = suffix.interface
), candidate_points AS MATERIALIZED (
    SELECT key.ordinal, retained.client_id, retained.interface,
           retained.bucket_start, retained.bucket_secs,
           retained.sample_count, retained.rx_bytes_avg,
           retained.tx_bytes_avg, retained.rx_bytes_last,
           retained.tx_bytes_last, retained.rx_counter_epoch,
           retained.tx_counter_epoch, retained.latest_observed_at,
           retained.updated_at
    FROM candidate_keys key
    JOIN telemetry_network_rates retained
      ON retained.client_id = key.client_id
     AND retained.interface = key.interface
     AND retained.bucket_secs = key.bucket_secs
     AND retained.bucket_start = key.bucket_start
    WHERE NOT EXISTS (
        SELECT 1
        FROM projected_suffix shadow
        WHERE shadow.client_id = retained.client_id
          AND shadow.interface = retained.interface
          AND shadow.bucket_secs = retained.bucket_secs
          AND shadow.bucket_start = retained.bucket_start
    )

    UNION ALL

    SELECT key.ordinal, sample.client_id, sample.interface,
           sample.observed_at, 60::INTEGER, sample.sample_count,
           round(
               sample.rx_bytes_sum / sample.sample_count::NUMERIC
           )::BIGINT,
           round(
               sample.tx_bytes_sum / sample.sample_count::NUMERIC
           )::BIGINT,
           sample.rx_bytes, sample.tx_bytes,
           sample.rx_counter_epoch, sample.tx_counter_epoch,
           sample.latest_observed_at, sample.updated_at
    FROM candidate_keys key
    JOIN stream_keys stream
      ON stream.client_id = key.client_id
     AND stream.interface = key.interface
    JOIN traffic_counter_samples sample
      ON sample.client_id = stream.client_id
     AND sample.source_kind = 'host'
     AND sample.interface = stream.interface
     AND sample.observed_at = key.bucket_start
    WHERE key.bucket_secs = 60
      AND stream.first_unpromoted_observed_at IS NOT NULL
      AND sample.observed_at >= stream.first_unpromoted_observed_at
      AND NOT sample.inbound_promoted
      AND NOT EXISTS (
          SELECT 1
          FROM projected_suffix shadow
          WHERE shadow.client_id = sample.client_id
            AND shadow.interface = sample.interface
            AND shadow.bucket_start = sample.observed_at
      )

    UNION ALL

    SELECT key.ordinal, suffix.client_id, suffix.interface,
           suffix.bucket_start, suffix.bucket_secs,
           suffix.sample_count, suffix.rx_bytes_avg,
           suffix.tx_bytes_avg, suffix.rx_bytes_last,
           suffix.tx_bytes_last, suffix.rx_counter_epoch,
           suffix.tx_counter_epoch, suffix.latest_observed_at,
           suffix.updated_at
    FROM candidate_keys key
    JOIN projected_suffix suffix
      ON suffix.client_id = key.client_id
     AND suffix.interface = key.interface
     AND suffix.bucket_secs = key.bucket_secs
     AND suffix.bucket_start = key.bucket_start
), examined AS MATERIALIZED (
    SELECT
        candidate.*,
        previous.latest_observed_at AS previous_observed_at,
        previous.rx_bytes_last AS previous_rx_bytes,
        previous.tx_bytes_last AS previous_tx_bytes,
        previous.rx_counter_epoch AS previous_rx_counter_epoch,
        previous.tx_counter_epoch AS previous_tx_counter_epoch,
        previous.latest_observed_at IS NOT NULL
          AND candidate.rx_counter_epoch = previous.rx_counter_epoch
          AND candidate.tx_counter_epoch = previous.tx_counter_epoch
          AND candidate.rx_bytes_last >= previous.rx_bytes_last
          AND candidate.tx_bytes_last >= previous.tx_bytes_last
            AS transition_valid
    FROM candidate_points candidate
    LEFT JOIN LATERAL (
        SELECT
            edge.latest_observed_at,
            edge.rx_bytes_last,
            edge.tx_bytes_last,
            edge.rx_counter_epoch,
            edge.tx_counter_epoch
        FROM (
            (
                SELECT predecessor.bucket_start,
                       predecessor.bucket_secs,
                       predecessor.latest_observed_at,
                       predecessor.rx_bytes_last,
                       predecessor.tx_bytes_last,
                       predecessor.rx_counter_epoch,
                       predecessor.tx_counter_epoch,
                       1::SMALLINT AS source_priority
                FROM telemetry_network_rates predecessor
                WHERE predecessor.client_id = candidate.client_id
                  AND predecessor.interface = candidate.interface
{retained_predecessor_bucket_filter}                  AND ROW(
                        predecessor.latest_observed_at,
                        predecessor.bucket_start,
                        predecessor.bucket_secs
                      ) < ROW(
                        candidate.latest_observed_at,
                        candidate.bucket_start,
                        candidate.bucket_secs
                      )
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix shadow
                      WHERE shadow.client_id = predecessor.client_id
                        AND shadow.interface = predecessor.interface
                        AND shadow.bucket_secs = predecessor.bucket_secs
                        AND shadow.bucket_start = predecessor.bucket_start
                  )
                ORDER BY predecessor.latest_observed_at DESC,
                         predecessor.bucket_start DESC,
                         predecessor.bucket_secs DESC
                LIMIT 1
            )

            UNION ALL

            (
                SELECT sample.observed_at, 60::INTEGER,
                       sample.latest_observed_at,
                       sample.rx_bytes, sample.tx_bytes,
                       sample.rx_counter_epoch, sample.tx_counter_epoch,
                       2::SMALLINT AS source_priority
                FROM stream_keys stream
                JOIN traffic_counter_samples sample
                  ON sample.client_id = stream.client_id
                 AND sample.source_kind = 'host'
                 AND sample.interface = stream.interface
                WHERE stream.client_id = candidate.client_id
                  AND stream.interface = candidate.interface
                  AND stream.first_unpromoted_observed_at IS NOT NULL
                  AND sample.observed_at >=
                      stream.first_unpromoted_observed_at
                  AND NOT sample.inbound_promoted
{minute_predecessor_bucket_filter}                  AND ROW(
                        sample.latest_observed_at,
                        sample.observed_at,
                        60::INTEGER
                      ) < ROW(
                        candidate.latest_observed_at,
                        candidate.bucket_start,
                        candidate.bucket_secs
                      )
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix shadow
                      WHERE shadow.client_id = sample.client_id
                        AND shadow.interface = sample.interface
                        AND shadow.bucket_start = sample.observed_at
                  )
                ORDER BY sample.observed_at DESC
                LIMIT 1
            )

            UNION ALL

            (
                SELECT predecessor.bucket_start,
                       predecessor.bucket_secs,
                       predecessor.latest_observed_at,
                       predecessor.rx_bytes_last,
                       predecessor.tx_bytes_last,
                       predecessor.rx_counter_epoch,
                       predecessor.tx_counter_epoch,
                       3::SMALLINT AS source_priority
                FROM projected_suffix predecessor
                WHERE predecessor.client_id = candidate.client_id
                  AND predecessor.interface = candidate.interface
{retained_predecessor_bucket_filter}                  AND ROW(
                        predecessor.latest_observed_at,
                        predecessor.bucket_start,
                        predecessor.bucket_secs
                      ) < ROW(
                        candidate.latest_observed_at,
                        candidate.bucket_start,
                        candidate.bucket_secs
                      )
                ORDER BY predecessor.latest_observed_at DESC,
                         predecessor.bucket_start DESC,
                         predecessor.bucket_secs DESC
                LIMIT 1
            )
        ) edge
        ORDER BY edge.latest_observed_at DESC,
                 edge.bucket_start DESC,
                 edge.bucket_secs DESC,
                 edge.source_priority ASC
        LIMIT 1
    ) previous ON TRUE
)
SELECT
    candidate.ordinal AS candidate_ordinal,
    candidate.client_id,
    candidate.interface,
    candidate.bucket_start::text AS bucket_start,
    candidate.bucket_secs,
    candidate.sample_count,
    candidate.rx_bytes_avg,
    candidate.tx_bytes_avg,
    candidate.rx_bytes_last,
    candidate.tx_bytes_last,
    candidate.rx_counter_epoch,
    candidate.tx_counter_epoch,
    CASE WHEN candidate.transition_valid THEN
        candidate.rx_bytes_last - candidate.previous_rx_bytes
    END AS rx_bytes_delta,
    CASE WHEN candidate.transition_valid THEN
        candidate.tx_bytes_last - candidate.previous_tx_bytes
    END AS tx_bytes_delta,
    CASE WHEN candidate.transition_valid THEN
        (candidate.rx_bytes_last - candidate.previous_rx_bytes)::double precision * 8
            / GREATEST(
                extract(epoch FROM (
                    candidate.latest_observed_at - candidate.previous_observed_at
                )),
                1
            )::double precision
    END AS rx_bps_avg,
    CASE WHEN candidate.transition_valid THEN
        (candidate.tx_bytes_last - candidate.previous_tx_bytes)::double precision * 8
            / GREATEST(
                extract(epoch FROM (
                    candidate.latest_observed_at - candidate.previous_observed_at
                )),
                1
            )::double precision
    END AS tx_bps_avg,
    candidate.latest_observed_at::text AS latest_observed_at,
    candidate.updated_at::text AS updated_at,
    candidate.transition_valid
FROM examined candidate
ORDER BY candidate.ordinal ASC
"#
    )
}

// Explicit physical-tier inspection remains backed by retained history. The
// dashboard sparse projection path does not use this query. Minute points have
// two short-lived overlays: accepted projected envelopes not yet materialized
// and materialized counter samples not yet promoted. Resolve the former once,
// then take the two newest distinct effective edges from the three physical
// owners. This preserves effective-history shadowing without expanding the
// composite source inside every per-stream index probe.
pub(crate) const LATEST_TELEMETRY_NETWORK_RATES_SQL: &str = r#"
WITH requested_clients AS MATERIALIZED (
    SELECT client.id AS client_id
    FROM visible_clients client
    WHERE client.status <> 'suspended'
      AND ($1::TEXT IS NULL OR client.id = $1)
      AND ($2::TEXT[] IS NULL OR client.id = ANY($2))
      AND (
          $6::BOOLEAN
          OR client.id = ANY($7::TEXT[])
          OR client.id = ANY($8::TEXT[])
      )
), durable_candidate_keys AS MATERIALIZED (
    SELECT
        streams.client_id,
        streams.source_kind,
        streams.interface,
        streams.first_unpromoted_observed_at
    FROM traffic_counter_streams streams
    JOIN visible_clients client
      ON client.id = streams.client_id
     AND client.status <> 'suspended'
    WHERE streams.source_kind = 'host'
      AND ($1::TEXT IS NULL OR streams.client_id = $1)
      AND ($2::TEXT[] IS NULL OR streams.client_id = ANY($2))
      AND ($3::TEXT IS NULL OR streams.interface = $3)
      AND (
          $6::BOOLEAN
          OR streams.client_id = ANY($7::TEXT[])
          OR EXISTS (
              SELECT 1
              FROM UNNEST($8::TEXT[], $9::TEXT[])
                  AS selected(client_id, interface)
              WHERE selected.client_id = streams.client_id
                AND selected.interface = streams.interface
          )
      )
), candidate_projected_suffix AS MATERIALIZED (
    SELECT suffix.*
    FROM telemetry_projected_raw_network_minutes_source(ARRAY(
        SELECT requested.client_id
        FROM requested_clients requested
        ORDER BY requested.client_id
    )) suffix
    JOIN requested_clients client
      ON client.client_id = suffix.client_id
    WHERE $4::INTEGER = 60
      AND ($1::TEXT IS NULL OR suffix.client_id = $1)
      AND ($2::TEXT[] IS NULL OR suffix.client_id = ANY($2))
      AND ($3::TEXT IS NULL OR suffix.interface = $3)
      AND (
          $6::BOOLEAN
          OR suffix.client_id = ANY($7::TEXT[])
          OR EXISTS (
              SELECT 1
              FROM UNNEST($8::TEXT[], $9::TEXT[])
                  AS selected(client_id, interface)
              WHERE selected.client_id = suffix.client_id
                AND selected.interface = suffix.interface
          )
      )
), candidate_stream_keys AS MATERIALIZED (
    SELECT candidate.client_id, 'host'::TEXT AS source_kind,
           candidate.interface,
           max(candidate.first_unpromoted_observed_at)
               AS first_unpromoted_observed_at
    FROM (
        SELECT durable.client_id, durable.interface,
               durable.first_unpromoted_observed_at
        FROM durable_candidate_keys durable

        UNION ALL

        SELECT suffix.client_id, suffix.interface,
               NULL::TIMESTAMPTZ AS first_unpromoted_observed_at
        FROM candidate_projected_suffix suffix
    ) candidate
    GROUP BY candidate.client_id, candidate.interface
), resolved_interface_policies AS MATERIALIZED (
    SELECT policy.*
    FROM public.resolve_telemetry_interface_policies(ARRAY(
        SELECT DISTINCT candidate.client_id
        FROM candidate_stream_keys candidate
        ORDER BY candidate.client_id
    )) policy
), stream_keys AS MATERIALIZED (
    SELECT
        candidate.client_id,
        candidate.interface,
        candidate.first_unpromoted_observed_at
    FROM candidate_stream_keys candidate
    JOIN resolved_interface_policies policy
      ON policy.client_id = candidate.client_id
    WHERE public.telemetry_interface_is_admitted_resolved(
        policy.admission_mode,
        policy.interface_patterns,
        policy.managed_tunnel_interfaces,
        candidate.source_kind,
        candidate.interface
    )
), projected_suffix AS MATERIALIZED (
    SELECT suffix.*
    FROM candidate_projected_suffix suffix
    JOIN stream_keys stream
      ON stream.client_id = suffix.client_id
     AND stream.interface = suffix.interface
    WHERE $4::INTEGER = 60
), stream_edges AS MATERIALIZED (
    SELECT
        stream.client_id,
        stream.interface,
        edge.bucket_start,
        edge.bucket_secs,
        edge.sample_count,
        edge.rx_bytes_avg,
        edge.tx_bytes_avg,
        edge.rx_bytes_last,
        edge.tx_bytes_last,
        edge.rx_counter_epoch,
        edge.tx_counter_epoch,
        edge.latest_observed_at,
        edge.updated_at,
        row_number() OVER (
            PARTITION BY stream.client_id, stream.interface
            ORDER BY edge.latest_observed_at DESC,
                     edge.bucket_start DESC,
                     edge.source_priority ASC
        ) AS recency_rank
    FROM stream_keys stream
    CROSS JOIN LATERAL (
        SELECT DISTINCT ON (candidate.latest_observed_at)
            candidate.*
        FROM (
            (
                SELECT
                    retained.bucket_start,
                    retained.bucket_secs,
                    retained.sample_count,
                    retained.rx_bytes_avg,
                    retained.tx_bytes_avg,
                    retained.rx_bytes_last,
                    retained.tx_bytes_last,
                    retained.rx_counter_epoch,
                    retained.tx_counter_epoch,
                    retained.latest_observed_at,
                    retained.updated_at,
                    1::SMALLINT AS source_priority
                FROM telemetry_network_rates retained
                WHERE retained.client_id = stream.client_id
                  AND retained.interface = stream.interface
                  AND retained.bucket_secs = $4
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix shadow
                      WHERE shadow.client_id = retained.client_id
                        AND shadow.interface = retained.interface
                        AND shadow.bucket_secs = retained.bucket_secs
                        AND shadow.bucket_start = retained.bucket_start
                  )
                ORDER BY retained.latest_observed_at DESC,
                         retained.bucket_start DESC
                LIMIT 2
            )

            UNION ALL

            (
                SELECT
                    sample.observed_at AS bucket_start,
                    60 AS bucket_secs,
                    sample.sample_count,
                    round(
                        sample.rx_bytes_sum / sample.sample_count::NUMERIC
                    )::BIGINT AS rx_bytes_avg,
                    round(
                        sample.tx_bytes_sum / sample.sample_count::NUMERIC
                    )::BIGINT AS tx_bytes_avg,
                    sample.rx_bytes AS rx_bytes_last,
                    sample.tx_bytes AS tx_bytes_last,
                    sample.rx_counter_epoch,
                    sample.tx_counter_epoch,
                    sample.latest_observed_at,
                    sample.updated_at,
                    2::SMALLINT AS source_priority
                FROM traffic_counter_samples sample
                WHERE $4::INTEGER = 60
                  AND stream.first_unpromoted_observed_at IS NOT NULL
                  AND sample.client_id = stream.client_id
                  AND sample.source_kind = 'host'
                  AND sample.interface = stream.interface
                  AND sample.observed_at >=
                      stream.first_unpromoted_observed_at
                  AND NOT sample.inbound_promoted
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix shadow
                      WHERE shadow.client_id = sample.client_id
                        AND shadow.interface = sample.interface
                        AND shadow.bucket_start = sample.observed_at
                  )
                -- `latest_observed_at` is inside this exact minute, so the
                -- primary-key minute order is its strict effective-time order.
                ORDER BY sample.observed_at DESC
                LIMIT 2
            )

            UNION ALL

            (
                SELECT
                    suffix.bucket_start,
                    suffix.bucket_secs,
                    suffix.sample_count,
                    suffix.rx_bytes_avg,
                    suffix.tx_bytes_avg,
                    suffix.rx_bytes_last,
                    suffix.tx_bytes_last,
                    suffix.rx_counter_epoch,
                    suffix.tx_counter_epoch,
                    suffix.latest_observed_at,
                    suffix.updated_at,
                    3::SMALLINT AS source_priority
                FROM projected_suffix suffix
                WHERE suffix.client_id = stream.client_id
                  AND suffix.interface = stream.interface
                ORDER BY suffix.bucket_start DESC
                LIMIT 2
            )
        ) candidate
        -- The predecessor contract is strictly older by effective time. A
        -- duplicate logical minute from two owners must not become its own
        -- predecessor while ownership is transferring.
        ORDER BY candidate.latest_observed_at DESC,
                 candidate.bucket_start DESC,
                 candidate.source_priority ASC
        LIMIT 2
    ) edge
), examined AS (
    SELECT
        latest.*,
        previous.latest_observed_at AS previous_effective_at,
        previous.rx_bytes_last AS previous_rx_bytes_last,
        previous.tx_bytes_last AS previous_tx_bytes_last,
        previous.rx_counter_epoch AS previous_rx_counter_epoch,
        previous.tx_counter_epoch AS previous_tx_counter_epoch
    FROM stream_edges latest
    JOIN stream_edges previous
      ON previous.client_id = latest.client_id
     AND previous.interface = latest.interface
     AND previous.recency_rank = 2
    WHERE latest.recency_rank = 1
)
SELECT
    latest.client_id,
    latest.interface,
    latest.bucket_start::text AS bucket_start,
    latest.bucket_secs,
    latest.sample_count,
    latest.rx_bytes_avg,
    latest.tx_bytes_avg,
    latest.rx_bytes_last,
    latest.tx_bytes_last,
    latest.rx_counter_epoch,
    latest.tx_counter_epoch,
    latest.rx_bytes_last - latest.previous_rx_bytes_last AS rx_bytes_delta,
    latest.tx_bytes_last - latest.previous_tx_bytes_last AS tx_bytes_delta,
    (latest.rx_bytes_last - latest.previous_rx_bytes_last)::double precision * 8.0
        / GREATEST(
            extract(epoch FROM (
                latest.latest_observed_at - latest.previous_effective_at
            )),
            1
        )::double precision AS rx_bps_avg,
    (latest.tx_bytes_last - latest.previous_tx_bytes_last)::double precision * 8.0
        / GREATEST(
            extract(epoch FROM (
                latest.latest_observed_at - latest.previous_effective_at
            )),
            1
        )::double precision AS tx_bps_avg,
    latest.latest_observed_at::text AS latest_observed_at,
    latest.updated_at::text AS updated_at
FROM examined latest
WHERE latest.rx_counter_epoch = latest.previous_rx_counter_epoch
  AND latest.tx_counter_epoch = latest.previous_tx_counter_epoch
  AND latest.rx_bytes_last >= latest.previous_rx_bytes_last
  AND latest.tx_bytes_last >= latest.previous_tx_bytes_last
ORDER BY latest.latest_observed_at DESC, latest.client_id ASC, latest.interface ASC
LIMIT $5
"#;

// The fifteen-minute visibility exception belongs only to one VPS detail.
// Shared current readers consume the traffic materialization suffix instead;
// this parameterized reader expands exactly one requested client's recent raw
// payload and returns only interfaces excluded by its current admission rule.
const RECENT_EXCLUDED_NETWORK_TRANSITIONS_SQL: &str = r#"
WITH policy AS MATERIALIZED (
    SELECT *
    FROM public.resolve_telemetry_interface_policies(ARRAY[$1::TEXT])
), raw AS MATERIALIZED (
    SELECT
        sample.client_id,
        network.value ->> 'interface' AS interface,
        sample.observed_at,
        sample.accepted_seq,
        sample.accepted_at AS updated_at,
        network.ordinality,
        telemetry_u64_counter_to_bigint(
            network.value ->> 'rx_bytes'
        ) AS rx_bytes,
        telemetry_u64_counter_to_bigint(
            network.value ->> 'tx_bytes'
        ) AS tx_bytes
    FROM visible_clients client
    JOIN telemetry_projection_heads projection
      ON projection.client_id = client.id
    JOIN telemetry_samples sample
      ON sample.client_id = projection.client_id
     AND sample.accepted_seq <= projection.projected_seq
    CROSS JOIN LATERAL jsonb_array_elements(
        CASE WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
            THEN sample.payload -> 'networks' ELSE '[]'::JSONB END
    ) WITH ORDINALITY network(value, ordinality)
    WHERE client.id = $1
      AND client.status <> 'suspended'
      AND sample.observed_at >= statement_timestamp() - interval '15 minutes'
      AND octet_length(network.value ->> 'interface') BETWEEN 1 AND 128
), sequenced AS MATERIALIZED (
    SELECT raw.*,
           lag(raw.observed_at) OVER stream AS previous_observed_at,
           lag(raw.rx_bytes) OVER stream AS previous_rx_bytes,
           lag(raw.tx_bytes) OVER stream AS previous_tx_bytes,
           row_number() OVER (
               PARTITION BY raw.client_id, raw.interface
               ORDER BY raw.observed_at DESC,
                        raw.accepted_seq DESC,
                        raw.ordinality DESC
           ) AS recency
    FROM raw
    WINDOW stream AS (
        PARTITION BY raw.client_id, raw.interface
        ORDER BY raw.observed_at, raw.accepted_seq, raw.ordinality
    )
), latest AS MATERIALIZED (
    SELECT row.*
    FROM sequenced row
    CROSS JOIN policy
    WHERE row.recency = 1
      AND row.previous_observed_at IS NOT NULL
      AND row.observed_at > row.previous_observed_at
      AND row.rx_bytes >= row.previous_rx_bytes
      AND row.tx_bytes >= row.previous_tx_bytes
      AND NOT public.telemetry_interface_is_admitted_resolved(
          policy.admission_mode,
          policy.interface_patterns,
          policy.managed_tunnel_interfaces,
          'host',
          row.interface
      )
      AND ($2::TEXT IS NULL OR row.interface = $2)
), minute_summary AS MATERIALIZED (
    SELECT raw.client_id, raw.interface,
           date_trunc('minute', raw.observed_at) AS bucket_start,
           count(*)::INTEGER AS sample_count,
           round(avg(raw.rx_bytes::NUMERIC))::BIGINT AS rx_bytes_avg,
           round(avg(raw.tx_bytes::NUMERIC))::BIGINT AS tx_bytes_avg
    FROM raw
    JOIN latest
      ON latest.client_id = raw.client_id
     AND latest.interface = raw.interface
     AND date_trunc('minute', latest.observed_at) =
         date_trunc('minute', raw.observed_at)
    GROUP BY raw.client_id, raw.interface,
             date_trunc('minute', raw.observed_at)
)
SELECT latest.client_id,
       latest.interface,
       summary.bucket_start::TEXT AS bucket_start,
       60::INTEGER AS bucket_secs,
       summary.sample_count,
       summary.rx_bytes_avg,
       summary.tx_bytes_avg,
       latest.rx_bytes AS rx_bytes_last,
       latest.tx_bytes AS tx_bytes_last,
       latest.rx_bytes - latest.previous_rx_bytes AS rx_bytes_delta,
       latest.tx_bytes - latest.previous_tx_bytes AS tx_bytes_delta,
       (latest.rx_bytes - latest.previous_rx_bytes)::DOUBLE PRECISION * 8.0
           / GREATEST(
               extract(epoch FROM (
                   latest.observed_at - latest.previous_observed_at
               )),
               1.0
           )::DOUBLE PRECISION AS rx_bps_avg,
       (latest.tx_bytes - latest.previous_tx_bytes)::DOUBLE PRECISION * 8.0
           / GREATEST(
               extract(epoch FROM (
                   latest.observed_at - latest.previous_observed_at
               )),
               1.0
           )::DOUBLE PRECISION AS tx_bps_avg,
       latest.observed_at::TEXT AS latest_observed_at,
       latest.updated_at::TEXT AS updated_at
FROM latest
JOIN minute_summary summary USING (client_id, interface)
ORDER BY latest.observed_at DESC, latest.interface
"#;

pub(crate) struct DashboardTelemetryNetworkProjection {
    pub(crate) rates: Vec<TelemetryNetworkRateView>,
    /// Fleet-level history already summed across clients by the sparse query.
    /// `None` means `rates` still contains the complete unaggregated history.
    pub(crate) fleet_rates: Option<Vec<TelemetryNetworkRateView>>,
    pub(crate) latest_rates: Vec<TelemetryNetworkRateView>,
    pub(crate) interfaces_by_rate: HashMap<(String, String), Vec<String>>,
}

pub(crate) struct DashboardTelemetryResourceProjection {
    pub(crate) rollups: Vec<TelemetryRollupView>,
    pub(crate) latest_rollups: Vec<TelemetryRollupView>,
}

#[derive(Clone, Debug)]
pub(crate) struct DashboardTelemetryTrafficPoint {
    pub(crate) client_id: String,
    pub(crate) bucket_start: String,
    pub(crate) rx_bytes: Option<i64>,
    pub(crate) tx_bytes: Option<i64>,
}

pub(crate) struct DashboardTelemetryTrafficProjection {
    pub(crate) client_points: Vec<DashboardTelemetryTrafficPoint>,
    pub(crate) fleet_points: Vec<DashboardTelemetryTrafficPoint>,
    pub(crate) interfaces_by_client: HashMap<String, Vec<String>>,
    pub(crate) client_ids_in_rank_order: Vec<String>,
}

pub(crate) struct TelemetryResourceHistoryProjection {
    pub(crate) rows: Vec<TelemetryRollupView>,
    pub(crate) complete: bool,
}

pub(crate) struct TelemetryNetworkHistoryProjection {
    pub(crate) rows: Vec<TelemetryNetworkRateView>,
    pub(crate) complete: bool,
}

/// The earliest projected telemetry boundary for an exact requested client set.
///
/// `complete` distinguishes a valid empty history (`start_unix == None`) from
/// projection heads whose resource, selected-rate network, traffic, and Ping minima
/// have not yet converged.
pub(crate) struct DashboardTelemetryStart {
    pub(crate) start_unix: Option<u64>,
    pub(crate) complete: bool,
}

const TELEMETRY_RESOURCE_HISTORY_PROJECTION_SQL: &str = r#"
WITH requested AS MATERIALIZED (
    SELECT DISTINCT client_id
    FROM UNNEST($1::TEXT[]) requested(client_id)
), ready AS MATERIALIZED (
    SELECT NOT $6::BOOLEAN OR count(*) = count(head.client_id) AS value
    FROM requested
    LEFT JOIN telemetry_dashboard_resource_projection_heads head
      ON head.client_id = requested.client_id
), source AS MATERIALIZED (
    SELECT
        rollup.*,
        extract(epoch FROM rollup.bucket_start)::BIGINT AS source_start_unix,
        GREATEST($4, rollup.bucket_secs)::INTEGER AS effective_step,
        floor(
            extract(epoch FROM rollup.bucket_start)::DOUBLE PRECISION
                / GREATEST($4, rollup.bucket_secs)::DOUBLE PRECISION
        )::BIGINT * GREATEST($4, rollup.bucket_secs)::BIGINT
            AS chart_start_unix
    FROM requested
    JOIN telemetry_resource_points_source(
        $1::TEXT[],
        to_timestamp($2) - make_interval(secs => 86400),
        to_timestamp($3),
        NULL::INTEGER,
        $5::BIGINT * (($4::BIGINT + 59) / 60)
    ) rollup USING (client_id)
    WHERE rollup.client_id = ANY($1::TEXT[])
      AND rollup.bucket_start >= to_timestamp($2)
            - make_interval(secs => 86400)
      AND rollup.bucket_start <= to_timestamp($3)
      AND rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
            > to_timestamp($2)
), bucketed AS MATERIALIZED (
    SELECT
        source.client_id,
        source.chart_start_unix,
        source.effective_step,
        LEAST(sum(source.sample_count::BIGINT), 2147483647)::INTEGER
            AS sample_count,
        LEAST(sum(source.cpu_usage_sample_count::BIGINT), 2147483647)::INTEGER
            AS cpu_usage_sample_count,
        sum(source.cpu_usage_sum
            ORDER BY source.source_start_unix, source.bucket_secs)
            / NULLIF(sum(source.cpu_usage_sample_count)::DOUBLE PRECISION, 0)
            AS cpu_usage_avg,
        max(source.cpu_usage_max)::DOUBLE PRECISION AS cpu_usage_max,
        max(source.cpu_cores_max)::INTEGER AS cpu_cores_max,
        sum(source.cpu_load_1_sum
            ORDER BY source.source_start_unix, source.bucket_secs)
            / sum(source.sample_count)::DOUBLE PRECISION AS cpu_load_1_avg,
        max(source.cpu_load_1_max)::DOUBLE PRECISION AS cpu_load_1_max,
        sum(source.cpu_load_5_sum
            ORDER BY source.source_start_unix, source.bucket_secs)
            / sum(source.sample_count)::DOUBLE PRECISION AS cpu_load_5_avg,
        max(source.cpu_load_5_max)::DOUBLE PRECISION AS cpu_load_5_max,
        sum(source.cpu_load_15_sum
            ORDER BY source.source_start_unix, source.bucket_secs)
            / sum(source.sample_count)::DOUBLE PRECISION AS cpu_load_15_avg,
        max(source.cpu_load_15_max)::DOUBLE PRECISION AS cpu_load_15_max,
        max(source.memory_total_bytes_max)::BIGINT AS memory_total_bytes_max,
        round(
            sum(source.memory_available_bytes_sum)
                / sum(source.sample_count)::NUMERIC
        )::BIGINT AS memory_available_bytes_avg,
        min(source.memory_available_bytes_min)::BIGINT
            AS memory_available_bytes_min,
        sum(source.memory_used_ratio_sum
            ORDER BY source.source_start_unix, source.bucket_secs)
            / sum(source.sample_count)::DOUBLE PRECISION
            AS memory_used_ratio_avg,
        max(source.memory_used_ratio_max)::DOUBLE PRECISION
            AS memory_used_ratio_max,
        LEAST(sum(source.swap_sample_count::BIGINT), 2147483647)::INTEGER
            AS swap_sample_count,
        max(source.swap_total_bytes_max)::BIGINT AS swap_total_bytes_max,
        CASE WHEN sum(source.swap_sample_count) > 0 THEN round(
            sum(source.swap_available_bytes_sum)
                / sum(source.swap_sample_count)::NUMERIC
        )::BIGINT WHEN max(source.swap_total_bytes_max) = 0 THEN 0 END
            AS swap_available_bytes_avg,
        CASE WHEN sum(source.swap_sample_count) > 0 THEN
            min(source.swap_available_bytes_min)
                FILTER (WHERE source.swap_sample_count > 0)
            WHEN max(source.swap_total_bytes_max) = 0 THEN 0 END
            AS swap_available_bytes_min,
        sum(source.swap_used_ratio_sum
            ORDER BY source.source_start_unix, source.bucket_secs)
            / NULLIF(sum(source.swap_sample_count)::DOUBLE PRECISION, 0)
            AS swap_used_ratio_avg,
        max(source.swap_used_ratio_max)
            FILTER (WHERE source.swap_sample_count > 0)
            AS swap_used_ratio_max,
        LEAST(sum(source.disk_sample_count::BIGINT), 2147483647)::INTEGER
            AS disk_sample_count,
        COALESCE(max(source.disk_total_bytes_max)
            FILTER (WHERE source.disk_sample_count > 0), 0)::BIGINT
            AS disk_total_bytes_max,
        round(COALESCE(
            sum(source.disk_available_bytes_sum)
                FILTER (WHERE source.disk_sample_count > 0)
                / NULLIF(sum(source.disk_sample_count)::NUMERIC, 0),
            0
        ))::BIGINT AS disk_available_bytes_avg,
        COALESCE(min(source.disk_available_bytes_min)
            FILTER (WHERE source.disk_sample_count > 0), 0)::BIGINT
            AS disk_available_bytes_min,
        COALESCE(
            sum(source.disk_used_ratio_sum
                ORDER BY source.source_start_unix, source.bucket_secs)
                FILTER (WHERE source.disk_sample_count > 0)
                / NULLIF(sum(source.disk_sample_count)::DOUBLE PRECISION, 0),
            0
        )::DOUBLE PRECISION AS disk_used_ratio_avg,
        COALESCE(max(source.disk_used_ratio_max)
            FILTER (WHERE source.disk_sample_count > 0), 0)::DOUBLE PRECISION
            AS disk_used_ratio_max,
        LEAST(sum(source.connections_sample_count::BIGINT), 2147483647)::INTEGER
            AS connections_sample_count,
        (array_agg(
            source.tcp_sockets_latest
            ORDER BY source.connections_observed_at DESC NULLS LAST,
                     source.source_start_unix DESC, source.bucket_secs DESC
        ) FILTER (WHERE source.connections_sample_count > 0))[1]
            AS tcp_sockets_latest,
        (array_agg(
            source.udp_sockets_latest
            ORDER BY source.connections_observed_at DESC NULLS LAST,
                     source.source_start_unix DESC, source.bucket_secs DESC
        ) FILTER (WHERE source.connections_sample_count > 0))[1]
            AS udp_sockets_latest,
        max(source.connections_observed_at) AS connections_observed_at,
        max(source.latest_observed_at) AS latest_observed_at,
        max(source.updated_at) AS updated_at
    FROM source
    GROUP BY source.client_id, source.chart_start_unix, source.effective_step
), recent AS MATERIALIZED (
    SELECT ranked.*
    FROM (
        SELECT point.*,
               row_number() OVER (
                    PARTITION BY point.client_id
                    ORDER BY point.chart_start_unix DESC,
                             point.effective_step DESC
               ) AS recency_rank
        FROM bucketed point
    ) ranked
    WHERE ranked.recency_rank <= $5
), output AS MATERIALIZED (
    SELECT requested.client_id, FALSE AS has_point,
           NULL::BIGINT AS chart_start_unix,
           NULL::INTEGER AS effective_step
    FROM requested
    UNION ALL
    SELECT recent.client_id, TRUE, recent.chart_start_unix,
           recent.effective_step
    FROM recent
)
SELECT
    output.client_id,
    output.has_point,
    CASE WHEN output.has_point
        THEN to_timestamp(output.chart_start_unix)::TEXT
    END AS bucket_start,
    COALESCE(output.effective_step, $4)::INTEGER AS bucket_secs,
    recent.sample_count,
    recent.cpu_usage_sample_count,
    recent.cpu_usage_avg,
    recent.cpu_usage_max,
    recent.cpu_cores_max,
    recent.cpu_load_1_avg,
    recent.cpu_load_1_max,
    recent.cpu_load_5_avg,
    recent.cpu_load_5_max,
    recent.cpu_load_15_avg,
    recent.cpu_load_15_max,
    recent.memory_total_bytes_max,
    recent.memory_available_bytes_avg,
    recent.memory_available_bytes_min,
    recent.memory_used_ratio_avg,
    recent.memory_used_ratio_max,
    recent.swap_sample_count,
    recent.swap_total_bytes_max,
    recent.swap_available_bytes_avg,
    recent.swap_available_bytes_min,
    recent.swap_used_ratio_avg,
    recent.swap_used_ratio_max,
    recent.disk_sample_count,
    recent.disk_total_bytes_max,
    recent.disk_available_bytes_avg,
    recent.disk_available_bytes_min,
    recent.disk_used_ratio_avg,
    recent.disk_used_ratio_max,
    recent.connections_sample_count,
    recent.tcp_sockets_latest,
    recent.udp_sockets_latest,
    recent.connections_observed_at::TEXT AS connections_observed_at,
    recent.latest_observed_at::TEXT AS latest_observed_at,
    recent.updated_at::TEXT AS updated_at
FROM output
LEFT JOIN recent
  ON output.has_point
 AND recent.client_id = output.client_id
 AND recent.chart_start_unix = output.chart_start_unix
 AND recent.effective_step = output.effective_step
CROSS JOIN ready
WHERE ready.value
ORDER BY output.chart_start_unix ASC NULLS FIRST, output.client_id
"#;

const TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL: &str = r#"
WITH requested AS MATERIALIZED (
    SELECT DISTINCT client_id
    FROM UNNEST($1::TEXT[]) requested(client_id)
), exact_selected_streams AS MATERIALIZED (
    SELECT DISTINCT selected.client_id, selected.interface
    FROM UNNEST($7::TEXT[], $8::TEXT[])
        selected(client_id, interface)
), resolved_interface_policies AS MATERIALIZED (
    SELECT policy.*
    FROM public.resolve_telemetry_interface_policies(ARRAY(
        SELECT requested.client_id
        FROM requested
        ORDER BY requested.client_id
    )) policy
), heads AS MATERIALIZED (
    SELECT requested.client_id,
           head.network_generation_interfaces,
           head.client_id IS NOT NULL AS complete
    FROM requested
    LEFT JOIN telemetry_dashboard_network_projection_heads head
      ON head.client_id = requested.client_id
), ready AS MATERIALIZED (
    SELECT COALESCE(bool_and(heads.complete), TRUE) AS value
    FROM heads
), selected_streams AS MATERIALIZED (
    SELECT DISTINCT heads.client_id, selected.interface
    FROM heads
    JOIN resolved_interface_policies policy
      ON policy.client_id = heads.client_id
    CROSS JOIN LATERAL unnest(heads.network_generation_interfaces)
        selected(interface)
    LEFT JOIN exact_selected_streams exact_selection
      ON exact_selection.client_id = heads.client_id
     AND exact_selection.interface = selected.interface
    WHERE heads.complete
      AND (
          heads.client_id = ANY($6::TEXT[])
          OR exact_selection.client_id IS NOT NULL
      )
      AND public.telemetry_interface_is_admitted_resolved(
          policy.admission_mode,
          policy.interface_patterns,
          policy.managed_tunnel_interfaces,
          'host',
          selected.interface
      )
), selected_stream_arrays AS MATERIALIZED (
    SELECT
        COALESCE(array_agg(
            stream.client_id
            ORDER BY stream.client_id COLLATE "C",
                     stream.interface COLLATE "C"
        ), ARRAY[]::TEXT[]) AS client_ids,
        COALESCE(array_agg(
            stream.interface
            ORDER BY stream.client_id COLLATE "C",
                     stream.interface COLLATE "C"
        ), ARRAY[]::TEXT[]) AS interfaces
    FROM selected_streams stream
), projected_suffix AS MATERIALIZED (
    SELECT suffix.*
    FROM telemetry_projected_raw_network_minutes_source($1::TEXT[]) suffix
    JOIN selected_streams stream USING (client_id, interface)
    WHERE suffix.client_id = ANY($1::TEXT[])
      AND suffix.bucket_start >= to_timestamp($2)
            - make_interval(secs => 86400)
      AND suffix.bucket_start <= to_timestamp($3)
), bounded_durable AS MATERIALIZED (
    -- General detail ranges can span retained tiers, so their paired exact
    -- stream relation keeps the canonical per-stream physical stops. Cards
    -- request one recent 60-second window: its physical owner is already the
    -- minute partition, where one time-leading scan and an exact stream join
    -- avoid turning a fleet read into one executor invocation per interface.
    SELECT durable.*
    FROM selected_stream_arrays streams
    CROSS JOIN LATERAL telemetry_network_durable_points_source(
        streams.client_ids,
        to_timestamp($2) - make_interval(secs => 86400),
        to_timestamp($3),
        NULL::INTEGER,
        streams.interfaces,
        ($5::BIGINT + 1)
            * ((GREATEST($4, 60)::BIGINT + 59) / 60)
    ) durable
    WHERE NOT $9::BOOLEAN OR $4::INTEGER <> 60

    UNION ALL

    SELECT minute.*
    FROM telemetry_network_rates_minute minute
    JOIN selected_streams stream
      ON stream.client_id = minute.client_id
     AND stream.interface = minute.interface
    WHERE $9::BOOLEAN
      AND $4::INTEGER = 60
      AND minute.bucket_secs = 60
      AND minute.bucket_start >= date_trunc(
              'minute', to_timestamp($2)
          )
      AND minute.bucket_start <= to_timestamp($3)

    UNION ALL

    SELECT sample.client_id,
           sample.interface,
           sample.observed_at AS bucket_start,
           60::INTEGER AS bucket_secs,
           sample.sample_count,
           sample.rx_bytes_sum,
           sample.tx_bytes_sum,
           round(
               sample.rx_bytes_sum / sample.sample_count::NUMERIC
           )::BIGINT AS rx_bytes_avg,
           round(
               sample.tx_bytes_sum / sample.sample_count::NUMERIC
           )::BIGINT AS tx_bytes_avg,
           sample.rx_bytes AS rx_bytes_last,
           sample.tx_bytes AS tx_bytes_last,
           sample.rx_counter_epoch,
           sample.tx_counter_epoch,
           sample.latest_observed_at,
           sample.updated_at
    FROM traffic_counter_samples sample
    JOIN selected_streams stream
      ON stream.client_id = sample.client_id
     AND stream.interface = sample.interface
    WHERE $9::BOOLEAN
      AND $4::INTEGER = 60
      AND sample.source_kind = 'host'
      AND NOT sample.inbound_promoted
      AND sample.observed_at >= date_trunc(
              'minute', to_timestamp($2)
          )
      AND sample.observed_at <= to_timestamp($3)

    UNION ALL

    -- Preserve the canonical strict predecessor used to derive the first
    -- visible counter delta.  This is one bounded index stop per selected
    -- stream, rather than another range/function execution per stream.
    SELECT predecessor.*
    FROM selected_streams stream
    JOIN LATERAL (
        SELECT candidate.*
        FROM (
            (
                SELECT minute.*
                FROM telemetry_network_rates_minute minute
                WHERE minute.bucket_secs = 60
                  AND minute.client_id = stream.client_id
                  AND minute.interface = stream.interface
                  AND minute.bucket_start < date_trunc(
                          'minute', to_timestamp($2)
                      )
                  AND minute.bucket_start >= to_timestamp($2)
                        - make_interval(secs => 86400)
                ORDER BY minute.bucket_start DESC
                LIMIT 1
            )

            UNION ALL

            (
                SELECT sample.client_id,
                       sample.interface,
                       sample.observed_at AS bucket_start,
                       60::INTEGER AS bucket_secs,
                       sample.sample_count,
                       sample.rx_bytes_sum,
                       sample.tx_bytes_sum,
                       round(
                           sample.rx_bytes_sum
                               / sample.sample_count::NUMERIC
                       )::BIGINT AS rx_bytes_avg,
                       round(
                           sample.tx_bytes_sum
                               / sample.sample_count::NUMERIC
                       )::BIGINT AS tx_bytes_avg,
                       sample.rx_bytes AS rx_bytes_last,
                       sample.tx_bytes AS tx_bytes_last,
                       sample.rx_counter_epoch,
                       sample.tx_counter_epoch,
                       sample.latest_observed_at,
                       sample.updated_at
                FROM traffic_counter_samples sample
                WHERE sample.client_id = stream.client_id
                  AND sample.source_kind = 'host'
                  AND sample.interface = stream.interface
                  AND NOT sample.inbound_promoted
                  AND sample.observed_at < date_trunc(
                          'minute', to_timestamp($2)
                      )
                  AND sample.observed_at >= to_timestamp($2)
                        - make_interval(secs => 86400)
                ORDER BY sample.observed_at DESC
                LIMIT 1
            )
        ) candidate
        ORDER BY candidate.bucket_start DESC,
                 candidate.latest_observed_at DESC
        LIMIT 1
    ) predecessor ON $9::BOOLEAN AND $4::INTEGER = 60
), canonical_points AS MATERIALIZED (
    SELECT durable.*
    FROM bounded_durable durable
    WHERE NOT EXISTS (
        SELECT 1
        FROM projected_suffix shadow
        WHERE shadow.client_id = durable.client_id
          AND shadow.interface = durable.interface
          AND shadow.bucket_secs = durable.bucket_secs
          AND shadow.bucket_start = durable.bucket_start
    )

    UNION ALL

    SELECT suffix.*
    FROM projected_suffix suffix
), ranked_effective_points AS MATERIALIZED (
    SELECT candidate.*,
           row_number() OVER (
               PARTITION BY candidate.client_id, candidate.interface
               ORDER BY candidate.bucket_start DESC,
                        candidate.latest_observed_at DESC,
                        candidate.bucket_secs ASC
           ) AS physical_rank
    FROM canonical_points candidate
), effective_points AS MATERIALIZED (
    -- N chart points need at most N * ceil(step / 60) canonical
    -- physical rows. One additional chart-width allowance preserves the
    -- strict counter predecessor before reset/decrease filtering. This cap is
    -- set-wise but remains independently partitioned by the exact stream.
    SELECT ranked.*
    FROM ranked_effective_points ranked
    WHERE ranked.physical_rank <= ($5::BIGINT + 1)
        * ((GREATEST($4, 60)::BIGINT + 59) / 60)
), source AS (
    SELECT
        rate.client_id,
        rate.interface,
        rate.bucket_start,
        rate.bucket_secs,
        rate.sample_count,
        rate.updated_at,
        GREATEST($4, rate.bucket_secs)::INTEGER AS effective_step,
        floor(
            extract(epoch FROM rate.bucket_start)::DOUBLE PRECISION
                / GREATEST($4, rate.bucket_secs)::DOUBLE PRECISION
        )::BIGINT * GREATEST($4, rate.bucket_secs)::BIGINT
            AS chart_start_unix,
        ARRAY[
            extract(epoch FROM rate.latest_observed_at)::BIGINT,
            extract(epoch FROM rate.bucket_start)::BIGINT,
            rate.bucket_secs::BIGINT,
            rate.rx_bytes_avg,
            rate.tx_bytes_avg,
            rate.rx_bytes_last,
            rate.tx_bytes_last,
            rate.rx_counter_epoch,
            rate.tx_counter_epoch
        ] AS terminal_values
    FROM effective_points rate
    WHERE rate.client_id = ANY($1::TEXT[])
      AND rate.bucket_start >= to_timestamp($2)
            - make_interval(secs => 86400)
      AND rate.bucket_start <= to_timestamp($3)
      AND rate.bucket_start + make_interval(secs => rate.bucket_secs)
            > to_timestamp($2)
), bucket_values AS (
    SELECT
        source.client_id,
        source.interface,
        source.chart_start_unix,
        source.effective_step,
        sum(source.sample_count::BIGINT) AS merged_sample_count,
        min(source.bucket_start) AS first_source_start,
        max(source.terminal_values) AS terminal_values,
        max(source.updated_at) AS merged_updated_at
    FROM source
    GROUP BY source.client_id, source.interface,
             source.chart_start_unix, source.effective_step
), bucketed AS MATERIALIZED (
    SELECT
        row.client_id,
        row.interface,
        row.chart_start_unix,
        row.effective_step,
        row.first_source_start,
        row.merged_sample_count AS sample_count,
        to_timestamp(row.terminal_values[1]) AS latest_observed_at,
        row.terminal_values[4] AS rx_bytes_avg,
        row.terminal_values[5] AS tx_bytes_avg,
        row.terminal_values[6] AS rx_bytes_last,
        row.terminal_values[7] AS tx_bytes_last,
        row.terminal_values[8] AS rx_counter_epoch,
        row.terminal_values[9] AS tx_counter_epoch,
        row.merged_updated_at AS updated_at
    FROM bucket_values row
), ranked_points AS MATERIALIZED (
    SELECT point.*,
           row_number() OVER (
                PARTITION BY point.client_id, point.interface
                ORDER BY point.chart_start_unix DESC,
                         point.effective_step DESC
           ) AS recency_rank
    FROM bucketed point
), capped_points AS MATERIALIZED (
    SELECT point.* FROM ranked_points point
    WHERE point.recency_rank <= $5
), dropped_predecessors AS MATERIALIZED (
    SELECT point.client_id, point.interface,
           point.chart_start_unix, point.effective_step,
           point.sample_count, point.latest_observed_at,
           point.rx_bytes_avg, point.tx_bytes_avg,
           point.rx_bytes_last, point.tx_bytes_last,
           point.rx_counter_epoch, point.tx_counter_epoch, point.updated_at
    FROM ranked_points point
    WHERE point.recency_rank = $5 + 1
), oldest_returned AS MATERIALIZED (
    SELECT DISTINCT ON (point.client_id, point.interface)
           point.client_id, point.interface, point.first_source_start
    FROM capped_points point
    ORDER BY point.client_id, point.interface,
             point.chart_start_unix, point.effective_step
), missing_predecessor_keys AS MATERIALIZED (
    SELECT oldest.*
    FROM oldest_returned oldest
    WHERE NOT EXISTS (
        SELECT 1
        FROM dropped_predecessors dropped
        WHERE dropped.client_id = oldest.client_id
          AND dropped.interface = oldest.interface
    )
), retained_predecessors AS MATERIALIZED (
    SELECT
        oldest.client_id,
        oldest.interface,
        NULL::BIGINT AS chart_start_unix,
        $4::INTEGER AS effective_step,
        predecessor.sample_count::BIGINT AS sample_count,
        predecessor.latest_observed_at,
        predecessor.rx_bytes_avg,
        predecessor.tx_bytes_avg,
        predecessor.rx_bytes_last,
        predecessor.tx_bytes_last,
        predecessor.rx_counter_epoch,
        predecessor.tx_counter_epoch,
        predecessor.updated_at
    FROM missing_predecessor_keys oldest
    CROSS JOIN LATERAL (
        SELECT candidate.*
        FROM (
            (
                SELECT retained.bucket_start, retained.bucket_secs,
                       retained.sample_count, retained.latest_observed_at,
                       retained.rx_bytes_avg, retained.tx_bytes_avg,
                       retained.rx_bytes_last, retained.tx_bytes_last,
                       retained.rx_counter_epoch, retained.tx_counter_epoch,
                       retained.updated_at, 1::SMALLINT AS source_priority
                FROM telemetry_network_rates retained
                WHERE retained.client_id = oldest.client_id
                  AND retained.interface = oldest.interface
                  AND retained.latest_observed_at < oldest.first_source_start
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix shadow
                      WHERE shadow.client_id = retained.client_id
                        AND shadow.interface = retained.interface
                        AND shadow.bucket_secs = retained.bucket_secs
                        AND shadow.bucket_start = retained.bucket_start
                  )
                ORDER BY retained.latest_observed_at DESC,
                         retained.bucket_start DESC,
                         retained.bucket_secs DESC
                LIMIT 1
            )

            UNION ALL

            (
                SELECT sample.observed_at AS bucket_start,
                       60::INTEGER AS bucket_secs,
                       sample.sample_count, sample.latest_observed_at,
                       round(
                           sample.rx_bytes_sum / sample.sample_count::NUMERIC
                       )::BIGINT AS rx_bytes_avg,
                       round(
                           sample.tx_bytes_sum / sample.sample_count::NUMERIC
                       )::BIGINT AS tx_bytes_avg,
                       sample.rx_bytes AS rx_bytes_last,
                       sample.tx_bytes AS tx_bytes_last,
                       sample.rx_counter_epoch, sample.tx_counter_epoch,
                       sample.updated_at, 2::SMALLINT AS source_priority
                FROM traffic_counter_streams stream
                JOIN traffic_counter_samples sample
                  ON sample.client_id = stream.client_id
                 AND sample.source_kind = stream.source_kind
                 AND sample.interface = stream.interface
                WHERE stream.client_id = oldest.client_id
                  AND stream.source_kind = 'host'
                  AND stream.interface = oldest.interface
                  AND stream.first_unpromoted_observed_at IS NOT NULL
                  AND sample.observed_at >=
                      stream.first_unpromoted_observed_at
                  AND NOT sample.inbound_promoted
                  AND sample.latest_observed_at < oldest.first_source_start
                  AND NOT EXISTS (
                      SELECT 1
                      FROM projected_suffix shadow
                      WHERE shadow.client_id = sample.client_id
                        AND shadow.interface = sample.interface
                        AND shadow.bucket_start = sample.observed_at
                  )
                ORDER BY sample.observed_at DESC
                LIMIT 1
            )

            UNION ALL

            (
                SELECT suffix.bucket_start, suffix.bucket_secs,
                       suffix.sample_count, suffix.latest_observed_at,
                       suffix.rx_bytes_avg, suffix.tx_bytes_avg,
                       suffix.rx_bytes_last, suffix.tx_bytes_last,
                       suffix.rx_counter_epoch, suffix.tx_counter_epoch,
                       suffix.updated_at, 3::SMALLINT AS source_priority
                FROM projected_suffix suffix
                WHERE suffix.client_id = oldest.client_id
                  AND suffix.interface = oldest.interface
                  AND suffix.latest_observed_at < oldest.first_source_start
                ORDER BY suffix.latest_observed_at DESC,
                         suffix.bucket_start DESC
                LIMIT 1
            )
        ) candidate
        ORDER BY candidate.latest_observed_at DESC,
                 candidate.bucket_start DESC,
                 candidate.bucket_secs DESC,
                 candidate.source_priority ASC
        LIMIT 1
    ) predecessor
), predecessor_interfaces AS MATERIALIZED (
    SELECT predecessor.*
    FROM dropped_predecessors predecessor
    UNION ALL
    SELECT predecessor.*
    FROM retained_predecessors predecessor
), interface_states AS MATERIALIZED (
    SELECT point.client_id, 'point'::TEXT AS row_kind,
           point.chart_start_unix, point.effective_step,
           point.interface, point.sample_count, point.latest_observed_at,
           point.rx_bytes_avg, point.tx_bytes_avg,
           point.rx_bytes_last, point.tx_bytes_last,
           point.rx_counter_epoch, point.tx_counter_epoch, point.updated_at
    FROM capped_points point
    UNION ALL
    SELECT predecessor.client_id, 'predecessor'::TEXT,
           predecessor.chart_start_unix, predecessor.effective_step,
           predecessor.interface, predecessor.sample_count,
           predecessor.latest_observed_at,
           predecessor.rx_bytes_avg, predecessor.tx_bytes_avg,
           predecessor.rx_bytes_last, predecessor.tx_bytes_last,
           predecessor.rx_counter_epoch, predecessor.tx_counter_epoch,
           predecessor.updated_at
    FROM predecessor_interfaces predecessor
), ordered AS MATERIALIZED (
    SELECT state.*,
           lag(state.latest_observed_at) OVER stream AS previous_observed_at,
           lag(state.rx_bytes_last) OVER stream AS previous_rx_bytes,
           lag(state.tx_bytes_last) OVER stream AS previous_tx_bytes,
           lag(state.rx_counter_epoch) OVER stream AS previous_rx_epoch,
           lag(state.tx_counter_epoch) OVER stream AS previous_tx_epoch
    FROM interface_states state
    WINDOW stream AS (
        PARTITION BY state.client_id, state.interface
        ORDER BY state.latest_observed_at,
                 CASE state.row_kind WHEN 'predecessor' THEN 0 ELSE 1 END,
                 state.chart_start_unix NULLS FIRST,
                 state.effective_step
    )
), derived AS MATERIALIZED (
    SELECT state.*,
           state.rx_bytes_last - state.previous_rx_bytes AS rx_bytes_delta,
           state.tx_bytes_last - state.previous_tx_bytes AS tx_bytes_delta,
           (state.rx_bytes_last - state.previous_rx_bytes)::DOUBLE PRECISION * 8.0
               / GREATEST(
                    extract(epoch FROM (
                        state.latest_observed_at - state.previous_observed_at
                    )),
                    1.0
                 )::DOUBLE PRECISION AS rx_bps_avg,
           (state.tx_bytes_last - state.previous_tx_bytes)::DOUBLE PRECISION * 8.0
               / GREATEST(
                    extract(epoch FROM (
                        state.latest_observed_at - state.previous_observed_at
                    )),
                    1.0
                 )::DOUBLE PRECISION AS tx_bps_avg
    FROM ordered state
    WHERE state.row_kind = 'point'
      AND state.previous_observed_at IS NOT NULL
      AND state.latest_observed_at > state.previous_observed_at
      AND state.rx_counter_epoch = state.previous_rx_epoch
      AND state.tx_counter_epoch = state.previous_tx_epoch
      AND state.rx_bytes_last >= state.previous_rx_bytes
      AND state.tx_bytes_last >= state.previous_tx_bytes
), selected_output AS MATERIALIZED (
    SELECT derived.client_id, derived.interface,
           derived.chart_start_unix, derived.effective_step,
           derived.sample_count, derived.rx_bytes_avg,
           derived.tx_bytes_avg, derived.rx_bytes_delta,
           derived.tx_bytes_delta, derived.rx_bps_avg,
           derived.tx_bps_avg, derived.latest_observed_at,
           derived.updated_at
    FROM derived
    WHERE NOT $9::BOOLEAN

    UNION ALL

    -- Counter transitions remain interface-owned through `derived`. Cards
    -- combine only those already-validated rates, matching the former Rust
    -- fold without transferring every interface point out of PostgreSQL.
    SELECT derived.client_id, ''::TEXT,
           derived.chart_start_unix, derived.effective_step,
           max(derived.sample_count),
           LEAST(
               sum(derived.rx_bytes_avg::NUMERIC),
               9223372036854775807::NUMERIC
           )::BIGINT,
           LEAST(
               sum(derived.tx_bytes_avg::NUMERIC),
               9223372036854775807::NUMERIC
           )::BIGINT,
           LEAST(
               sum(derived.rx_bytes_delta::NUMERIC),
               9223372036854775807::NUMERIC
           )::BIGINT,
           LEAST(
               sum(derived.tx_bytes_delta::NUMERIC),
               9223372036854775807::NUMERIC
           )::BIGINT,
           sum(
               derived.rx_bps_avg
               ORDER BY derived.interface COLLATE "C"
           ),
           sum(
               derived.tx_bps_avg
               ORDER BY derived.interface COLLATE "C"
           ),
           max(derived.latest_observed_at), max(derived.updated_at)
    FROM derived
    WHERE $9::BOOLEAN
    GROUP BY derived.client_id, derived.chart_start_unix,
             derived.effective_step
), output AS MATERIALIZED (
    SELECT requested.client_id, FALSE AS has_point,
           NULL::TEXT AS interface, NULL::BIGINT AS chart_start_unix,
           $4::INTEGER AS effective_step,
           NULL::BIGINT AS sample_count,
           NULL::BIGINT AS rx_bytes_avg, NULL::BIGINT AS tx_bytes_avg,
           NULL::BIGINT AS rx_bytes_delta, NULL::BIGINT AS tx_bytes_delta,
           NULL::DOUBLE PRECISION AS rx_bps_avg,
           NULL::DOUBLE PRECISION AS tx_bps_avg,
           NULL::TIMESTAMPTZ AS latest_observed_at,
           NULL::TIMESTAMPTZ AS updated_at
    FROM requested
    UNION ALL
    SELECT selected.client_id, TRUE, selected.interface,
           selected.chart_start_unix, selected.effective_step,
           selected.sample_count, selected.rx_bytes_avg,
           selected.tx_bytes_avg, selected.rx_bytes_delta,
           selected.tx_bytes_delta, selected.rx_bps_avg,
           selected.tx_bps_avg, selected.latest_observed_at,
           selected.updated_at
    FROM selected_output selected
)
SELECT output.client_id, output.has_point, output.interface,
       CASE WHEN output.has_point
            THEN to_timestamp(output.chart_start_unix)::TEXT
       END AS bucket_start,
       output.effective_step AS bucket_secs,
       CASE WHEN output.has_point
            THEN LEAST(output.sample_count, 2147483647)::INTEGER
       END AS sample_count,
       output.rx_bytes_avg, output.tx_bytes_avg,
       output.rx_bytes_delta, output.tx_bytes_delta,
       output.rx_bps_avg, output.tx_bps_avg,
       CASE WHEN output.has_point
            THEN output.latest_observed_at::TEXT
       END AS latest_observed_at,
       CASE WHEN output.has_point
            THEN output.updated_at::TEXT
       END AS updated_at
FROM output CROSS JOIN ready
WHERE ready.value
ORDER BY output.chart_start_unix ASC NULLS FIRST,
         output.client_id, output.interface
"#;

impl Repository {
    pub(crate) async fn raw_telemetry_covers_range_start(
        &self,
        client_ids: &[String],
        start_unix: u64,
    ) -> Result<bool> {
        if client_ids.is_empty() {
            return Ok(true);
        }
        match self {
            Self::Postgres(pool) => {
                let covers = sqlx::query_scalar::<_, bool>(RAW_TELEMETRY_COVERS_RANGE_START_SQL)
                    .bind(client_ids)
                    .bind(start_unix as i64)
                    .fetch_one(pool)
                    .await?;
                Ok(covers)
            }
        }
    }

    pub(crate) async fn list_telemetry_samples(
        &self,
        limit: i64,
        client_id: Option<&str>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        visible_only: bool,
    ) -> Result<Vec<TelemetrySampleView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH source_samples AS MATERIALIZED (
                        SELECT
                            sample.id,
                            sample.client_id,
                            sample.observed_at,
                            sample.cpu_load_1,
                            sample.memory_total_bytes,
                            sample.memory_available_bytes,
                            sample.network_admission_mask,
                            sample.tunnel_admission_mask,
                            public.telemetry_ordinal_admission_mask_is_exact(
                                sample.network_admission_mask,
                                CASE
                                    WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
                                    THEN jsonb_array_length(sample.payload -> 'networks')::BIGINT
                                    ELSE 0
                                END
                            ) AS network_admission_mask_is_exact,
                            public.telemetry_ordinal_admission_mask_is_exact(
                                sample.tunnel_admission_mask,
                                CASE
                                    WHEN jsonb_typeof(sample.payload -> 'tunnels') = 'array'
                                    THEN jsonb_array_length(sample.payload -> 'tunnels')::BIGINT
                                    ELSE 0
                                END
                            ) AS tunnel_admission_mask_is_exact,
                            sample.payload
                        FROM telemetry_samples sample
                        LEFT JOIN telemetry_projection_heads projection
                          ON projection.client_id = sample.client_id
                        WHERE
                            ($1::TEXT IS NULL OR sample.client_id = $1)
                            AND sample.observed_at >= COALESCE(
                                to_timestamp($2::double precision),
                                '-infinity'::timestamptz
                            )
                            AND sample.observed_at <= COALESCE(
                                to_timestamp($3::double precision),
                                'infinity'::timestamptz
                            )
                            AND sample.accepted_seq <= projection.projected_seq
                            AND (
                                NOT $4
                                OR EXISTS (
                                    SELECT 1 FROM visible_clients
                                    WHERE visible_clients.id = sample.client_id
                                      AND visible_clients.status <> 'suspended'
                                )
                            )
                        ORDER BY sample.observed_at DESC, sample.id DESC
                        LIMIT $5
                    ), interface_candidates AS MATERIALIZED (
                        SELECT DISTINCT
                            sample.client_id,
                            'host'::TEXT AS source_kind,
                            network.value ->> 'interface' AS interface
                        FROM source_samples sample
                        CROSS JOIN LATERAL jsonb_array_elements(
                            CASE
                                WHEN jsonb_typeof(sample.payload -> 'networks') = 'array'
                                THEN sample.payload -> 'networks'
                                ELSE '[]'::JSONB
                            END
                        ) WITH ORDINALITY AS network(value, ordinality)
                        WHERE sample.network_admission_mask_is_exact
                          AND CASE
                                  WHEN network.ordinality <=
                                       octet_length(
                                           sample.network_admission_mask
                                       )::bigint * 8
                                  THEN get_bit(
                                      sample.network_admission_mask,
                                      (network.ordinality - 1)::integer
                                  ) = 1
                                  ELSE FALSE
                              END
                          AND octet_length(network.value ->> 'interface')
                              BETWEEN 1 AND 128

                        UNION

                        SELECT DISTINCT
                            sample.client_id,
                            'tunnel'::TEXT AS source_kind,
                            tunnel.value ->> 'interface' AS interface
                        FROM source_samples sample
                        CROSS JOIN LATERAL jsonb_array_elements(
                            CASE
                                WHEN jsonb_typeof(sample.payload -> 'tunnels') = 'array'
                                THEN sample.payload -> 'tunnels'
                                ELSE '[]'::JSONB
                            END
                        ) WITH ORDINALITY AS tunnel(value, ordinality)
                        WHERE sample.tunnel_admission_mask_is_exact
                          AND CASE
                                  WHEN tunnel.ordinality <=
                                       octet_length(
                                           sample.tunnel_admission_mask
                                       )::bigint * 8
                                  THEN get_bit(
                                      sample.tunnel_admission_mask,
                                      (tunnel.ordinality - 1)::integer
                                  ) = 1
                                  ELSE FALSE
                              END
                          AND octet_length(tunnel.value ->> 'interface')
                              BETWEEN 1 AND 128
                    ), resolved_interface_policies AS MATERIALIZED (
                        SELECT policy.*
                        FROM public.resolve_telemetry_interface_policies(ARRAY(
                            SELECT DISTINCT candidate.client_id
                            FROM interface_candidates candidate
                            ORDER BY candidate.client_id
                        )) policy
                    ), admitted_interfaces AS MATERIALIZED (
                        SELECT candidate.client_id,
                               candidate.source_kind,
                               candidate.interface
                        FROM interface_candidates candidate
                        JOIN resolved_interface_policies policy
                          ON policy.client_id = candidate.client_id
                        WHERE public.telemetry_interface_is_admitted_resolved(
                            policy.admission_mode,
                            policy.interface_patterns,
                            policy.managed_tunnel_interfaces,
                            candidate.source_kind,
                            candidate.interface
                        )
                    )
                    SELECT
                        sample.id,
                        sample.client_id,
                        sample.observed_at::text AS observed_at,
                        sample.cpu_load_1,
                        sample.memory_total_bytes,
                        sample.memory_available_bytes,
                        jsonb_set(
                            jsonb_set(
                                sample.payload,
                                '{networks}',
                                COALESCE((
                                    SELECT jsonb_agg(
                                        network.value ORDER BY network.ordinality
                                    )
                                    FROM jsonb_array_elements(
                                        CASE
                                            WHEN jsonb_typeof(
                                                sample.payload -> 'networks'
                                            ) = 'array'
                                            THEN sample.payload -> 'networks'
                                            ELSE '[]'::JSONB
                                        END
                                    ) WITH ORDINALITY AS network(value, ordinality)
                                    WHERE sample.network_admission_mask_is_exact
                                      AND CASE
                                              WHEN network.ordinality <=
                                                   octet_length(
                                                       sample.network_admission_mask
                                                   )::bigint * 8
                                              THEN get_bit(
                                                  sample.network_admission_mask,
                                                  (network.ordinality - 1)::integer
                                              ) = 1
                                              ELSE FALSE
                                          END
                                      AND EXISTS (
                                        SELECT 1
                                        FROM admitted_interfaces admitted
                                        WHERE admitted.client_id = sample.client_id
                                          AND admitted.source_kind = 'host'
                                          AND admitted.interface =
                                              network.value ->> 'interface'
                                    )
                                ), '[]'::JSONB)
                            ),
                            '{tunnels}',
                            COALESCE((
                                SELECT jsonb_agg(
                                    tunnel.value ORDER BY tunnel.ordinality
                                )
                                FROM jsonb_array_elements(
                                    CASE
                                        WHEN jsonb_typeof(
                                            sample.payload -> 'tunnels'
                                        ) = 'array'
                                        THEN sample.payload -> 'tunnels'
                                        ELSE '[]'::JSONB
                                    END
                                ) WITH ORDINALITY AS tunnel(value, ordinality)
                                WHERE sample.tunnel_admission_mask_is_exact
                                  AND CASE
                                          WHEN tunnel.ordinality <=
                                               octet_length(
                                                   sample.tunnel_admission_mask
                                               )::bigint * 8
                                          THEN get_bit(
                                              sample.tunnel_admission_mask,
                                              (tunnel.ordinality - 1)::integer
                                          ) = 1
                                          ELSE FALSE
                                      END
                                  AND EXISTS (
                                    SELECT 1
                                    FROM admitted_interfaces admitted
                                    WHERE admitted.client_id = sample.client_id
                                      AND admitted.source_kind = 'tunnel'
                                      AND admitted.interface =
                                          tunnel.value ->> 'interface'
                                )
                            ), '[]'::JSONB)
                        ) AS payload
                    FROM source_samples sample
                    ORDER BY sample.observed_at DESC, sample.id DESC
                    "#,
                )
                .bind(client_id)
                .bind(start_unix.map(|value| value as i64))
                .bind(end_unix.map(|value| value as i64))
                .bind(visible_only)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok(TelemetrySampleView {
                            id: row.try_get("id")?,
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            cpu_load_1: row.try_get("cpu_load_1")?,
                            memory_total_bytes: row.try_get("memory_total_bytes")?,
                            memory_available_bytes: row.try_get("memory_available_bytes")?,
                            payload: row.try_get("payload")?,
                        })
                    })
                    .collect()
            }
        }
    }

    pub(crate) async fn list_dashboard_raw_telemetry_rollups(
        &self,
        points_per_client: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryRollupView>> {
        Ok(self
            .query_telemetry_resource_history(
                points_per_client,
                start_unix,
                end_unix,
                step_secs,
                client_ids,
                false,
            )
            .await?
            .rows)
    }

    /// Reads recent and retained resource history through its single canonical
    /// owner. `require_projection_ready` is presentation policy only: retained
    /// detail ranges fail closed until their dashboard head exists, while cards
    /// and recent detail ranges keep returning the latest complete canonical
    /// points and expose lag through their existing freshness state.
    async fn query_telemetry_resource_history(
        &self,
        points_per_client: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        client_ids: &[String],
        require_projection_ready: bool,
    ) -> Result<TelemetryResourceHistoryProjection> {
        if client_ids.is_empty() {
            return Ok(TelemetryResourceHistoryProjection {
                rows: Vec::new(),
                complete: true,
            });
        }
        let points_per_client = points_per_client.clamp(2, 1_440) as usize;
        let step_secs = normalized_dashboard_step_secs(step_secs);
        let Self::Postgres(pool) = self;
        let rows = sqlx::query(TELEMETRY_RESOURCE_HISTORY_PROJECTION_SQL)
            .bind(client_ids)
            .bind(start_unix.min(i64::MAX as u64) as i64)
            .bind(end_unix.min(i64::MAX as u64) as i64)
            .bind(step_secs)
            .bind(points_per_client as i64)
            .bind(require_projection_ready)
            .fetch_all(pool)
            .await?;
        let requested_clients = client_ids.iter().collect::<HashSet<_>>().len();
        let mut returned_clients = HashSet::with_capacity(requested_clients);
        let mut history = Vec::with_capacity(requested_clients.saturating_mul(points_per_client));
        for row in rows {
            if row.try_get::<bool, _>("has_point")? {
                history.push(telemetry_rollup_from_row(row)?);
            } else {
                returned_clients.insert(row.try_get::<String, _>("client_id")?);
            }
        }
        let complete = returned_clients.len() == requested_clients;
        if !complete {
            history.clear();
        }
        Ok(TelemetryResourceHistoryProjection {
            rows: history,
            complete,
        })
    }

    #[cfg(test)]
    pub(crate) async fn list_dashboard_raw_telemetry_network_rates(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        let selection = NetworkRateInterfaceSelection::all(client_ids);
        self.list_dashboard_raw_telemetry_network_rates_selected(
            points_per_series,
            start_unix,
            end_unix,
            step_secs,
            &selection,
        )
        .await
    }

    pub(crate) async fn list_dashboard_raw_telemetry_network_rates_selected(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        self.list_dashboard_raw_telemetry_network_rates_selected_with_output(
            points_per_series,
            start_unix,
            end_unix,
            step_secs,
            selection,
            false,
        )
        .await
    }

    /// Returns one selected-interface total per client and chart bucket.
    ///
    /// This is deliberately separate from the existing per-interface reader: the
    /// Monitoring cards UI only graphs selected totals, while other API callers
    /// retain the existing interface-level response by default.
    pub(crate) async fn list_monitoring_card_raw_network_history_selected(
        &self,
        points_per_client: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        self.list_dashboard_raw_telemetry_network_rates_selected_with_output(
            points_per_client.clamp(2, 16),
            start_unix,
            end_unix,
            step_secs,
            selection,
            true,
        )
        .await
    }

    async fn list_dashboard_raw_telemetry_network_rates_selected_with_output(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
        aggregate_selected_interfaces: bool,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if selection.is_empty() {
            return Ok(Vec::new());
        }

        // "Raw" is a requested chart resolution, not permission to bypass the
        // canonical durable-plus-projected owner. The shared reader preserves
        // exact counter transitions across raw retention and compaction.
        let projection = self
            .query_projected_telemetry_network_history(
                points_per_series,
                start_unix,
                end_unix,
                step_secs,
                selection,
                aggregate_selected_interfaces,
            )
            .await?;
        if aggregate_selected_interfaces {
            Ok(projection.rows)
        } else {
            Ok(project_network_rate_selection(projection.rows, selection))
        }
    }

    pub(crate) async fn dashboard_telemetry_start_unix(
        &self,
        client_ids: &[String],
    ) -> Result<DashboardTelemetryStart> {
        if client_ids.is_empty() {
            return Ok(DashboardTelemetryStart {
                start_unix: None,
                complete: true,
            });
        }
        match self {
            Self::Postgres(pool) => {
                let (complete, value) = sqlx::query_as::<_, (bool, Option<f64>)>(
                    r#"
                    WITH requested AS MATERIALIZED (
                        SELECT DISTINCT requested.client_id
                        FROM UNNEST($1::TEXT[]) requested(client_id)
                    ), projected AS MATERIALIZED (
                        SELECT
                               requested.client_id,
                               COALESCE(
                                   dashboard.resource_generation > 0
                                       AND dashboard.network_generation > 0
                                       AND dashboard.traffic_generation > 0,
                                   FALSE
                               ) AS projection_ready,
                               dashboard.resource_first_at,
                               dashboard.network_first_at,
                               dashboard.traffic_first_at,
                               dashboard.ping_first_at
                        FROM requested
                        LEFT JOIN telemetry_dashboard_projection_heads dashboard
                          ON dashboard.client_id = requested.client_id
                    ), bounds AS (
                        SELECT projected.resource_first_at AS first_bucket
                        FROM projected
                        WHERE projected.resource_first_at IS NOT NULL

                        UNION ALL

                        SELECT projected.network_first_at AS first_bucket
                        FROM projected
                        WHERE projected.network_first_at IS NOT NULL

                        UNION ALL

                        SELECT projected.traffic_first_at AS first_bucket
                        FROM projected
                        WHERE projected.traffic_first_at IS NOT NULL

                        UNION ALL

                        SELECT projected.ping_first_at AS first_bucket
                        FROM projected
                        WHERE projected.ping_first_at IS NOT NULL
                    )
                    SELECT
                        COALESCE(bool_and(projected.projection_ready), TRUE),
                        (
                            SELECT extract(epoch FROM min(first_bucket))
                                ::double precision
                            FROM bounds
                        )
                    FROM projected
                    "#,
                )
                .bind(client_ids)
                .fetch_one(pool)
                .await?;
                Ok(DashboardTelemetryStart {
                    start_unix: value
                        .filter(|value| value.is_finite() && *value >= 0.0)
                        .map(|value| value as u64),
                    complete,
                })
            }
        }
    }

    pub(crate) async fn list_projected_telemetry_resource_history(
        &self,
        points_per_client: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        client_ids: &[String],
    ) -> Result<TelemetryResourceHistoryProjection> {
        self.query_telemetry_resource_history(
            points_per_client,
            start_unix,
            end_unix,
            step_secs,
            client_ids,
            true,
        )
        .await
    }

    pub(crate) async fn list_telemetry_rollups(
        &self,
        limit: i64,
        client_id: Option<&str>,
        bucket_secs: Option<i32>,
        visible_only: bool,
    ) -> Result<Vec<TelemetryRollupView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH requested_visible_clients AS MATERIALIZED (
                        SELECT visible.id AS client_id
                        FROM visible_clients visible
                        WHERE visible.status <> 'suspended'
                    )
                    SELECT
                        point.client_id,
                        point.bucket_start::text AS bucket_start,
                        point.bucket_secs,
                        point.sample_count,
                        point.cpu_usage_sample_count,
                        point.cpu_usage_avg,
                        point.cpu_usage_max,
                        point.cpu_cores_max,
                        point.cpu_load_1_avg,
                        point.cpu_load_1_max,
                        point.cpu_load_5_avg,
                        point.cpu_load_5_max,
                        point.cpu_load_15_avg,
                        point.cpu_load_15_max,
                        point.memory_total_bytes_max,
                        point.memory_available_bytes_avg,
                        point.memory_available_bytes_min,
                        point.memory_used_ratio_avg,
                        point.memory_used_ratio_max,
                        point.swap_sample_count,
                        point.swap_total_bytes_max,
                        point.swap_available_bytes_avg,
                        point.swap_available_bytes_min,
                        point.swap_used_ratio_avg,
                        point.swap_used_ratio_max,
                        point.disk_sample_count,
                        point.disk_total_bytes_max,
                        point.disk_available_bytes_avg,
                        point.disk_available_bytes_min,
                        point.disk_used_ratio_avg,
                        point.disk_used_ratio_max,
                        point.connections_sample_count,
                        point.tcp_sockets_latest,
                        point.udp_sockets_latest,
                        point.connections_observed_at::text AS connections_observed_at,
                        point.latest_observed_at::text AS latest_observed_at,
                        point.updated_at::text AS updated_at
                    FROM telemetry_resource_points_source(
                        CASE
                            WHEN $1::TEXT IS NOT NULL THEN ARRAY[$1::TEXT]
                            WHEN $3::BOOLEAN THEN ARRAY(
                                SELECT requested.client_id
                                FROM requested_visible_clients requested
                                ORDER BY requested.client_id
                            )
                            ELSE ARRAY(
                                SELECT client.id
                                FROM clients client
                                ORDER BY client.id
                            )
                        END,
                        NULL::TIMESTAMPTZ,
                        NULL::TIMESTAMPTZ,
                        $2::INTEGER,
                        $4::BIGINT
                    ) point
                    WHERE
                        ($1::TEXT IS NULL OR point.client_id = $1)
                        AND ($2::INTEGER IS NULL OR point.bucket_secs = $2)
                        AND (
                            NOT $3
                            OR EXISTS (
                                SELECT 1
                                FROM requested_visible_clients requested
                                WHERE requested.client_id = point.client_id
                            )
                        )
                    ORDER BY point.bucket_start DESC, point.client_id ASC
                    LIMIT $4
                    "#,
                )
                .bind(client_id)
                .bind(bucket_secs)
                .bind(visible_only)
                .bind(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX))
                .fetch_all(pool)
                .await?;

                rows.into_iter().map(telemetry_rollup_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_latest_telemetry_rollups(
        &self,
        limit: i64,
        client_id: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        self.list_latest_telemetry_rollups_matching(
            Some(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize),
            client_id,
            None,
            bucket_secs,
        )
        .await
    }

    pub(crate) async fn list_latest_telemetry_rollups_for_clients(
        &self,
        client_ids: &[String],
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        // Policy evaluation must cover its complete, already-resolved target
        // set. Keep this internal and require concrete client IDs rather than
        // widening the page-bounded public telemetry query.
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_latest_telemetry_rollups_matching(None, None, Some(client_ids), bucket_secs)
            .await
    }

    /// Pending age is client metadata, not a resource-row attribute.  Keeping
    /// it independent lets a first-ever sample surface the delayed warning
    /// before any resource-history row exists.
    pub(crate) async fn telemetry_projection_pending_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<HashMap<String, (Option<String>, Option<String>)>> {
        if client_ids.is_empty() {
            return Ok(HashMap::new());
        }
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH requested AS MATERIALIZED (
                        SELECT DISTINCT client_id
                        FROM UNNEST($1::TEXT[]) requested(client_id)
                    )
                    SELECT requested.client_id,
                           pending.pending_since::TEXT AS pending_since,
                           statement_timestamp()::TEXT AS checked_at
                    FROM requested
                    LEFT JOIN LATERAL (
                        SELECT min(source.pending_since) AS pending_since
                        FROM (
                            SELECT sample.accepted_at AS pending_since
                            FROM telemetry_projection_heads head
                            JOIN LATERAL (
                                SELECT sample.accepted_at
                                FROM telemetry_samples sample
                                WHERE sample.client_id = head.client_id
                                  AND sample.accepted_seq > head.projected_seq
                                  AND sample.accepted_seq <= head.accepted_seq
                                ORDER BY sample.accepted_seq
                                LIMIT 1
                            ) sample ON TRUE
                            WHERE head.client_id = requested.client_id

                            UNION ALL

                            SELECT min(event.queued_at)
                            FROM telemetry_dashboard_block_events event
                            WHERE event.client_id = requested.client_id

                            UNION ALL

                            SELECT min(event.queued_at)
                            FROM telemetry_dashboard_generation_events event
                            WHERE event.client_id = requested.client_id
                        ) source
                    ) pending ON TRUE
                    ORDER BY requested.client_id
                    "#,
                )
                .bind(client_ids)
                .fetch_all(pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        Ok((
                            row.try_get::<String, _>("client_id")?,
                            (
                                row.try_get::<Option<String>, _>("pending_since")?,
                                Some(row.try_get::<String, _>("checked_at")?),
                            ),
                        ))
                    })
                    .collect()
            }
        }
    }

    async fn list_latest_telemetry_rollups_matching(
        &self,
        result_limit: Option<usize>,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryRollupView>> {
        match self {
            Self::Postgres(pool) => {
                const QUERY: &str = r#"
                    WITH requested AS MATERIALIZED (
                        SELECT visible.id AS client_id
                        FROM visible_clients visible
                        WHERE visible.status <> 'suspended'
                          AND ($1::TEXT IS NULL OR visible.id = $1)
                          AND ($2::TEXT[] IS NULL OR visible.id = ANY($2))
                    ), projected_suffix AS MATERIALIZED (
                        SELECT suffix.*
                        FROM telemetry_projected_raw_resource_minutes_source(ARRAY(
                            SELECT owner.client_id
                            FROM requested owner
                            ORDER BY owner.client_id
                        )) suffix
                        JOIN requested USING (client_id)
                        WHERE ($1::TEXT IS NULL OR suffix.client_id = $1)
                          AND ($2::TEXT[] IS NULL
                               OR suffix.client_id = ANY($2))
                          AND ($3::INTEGER IS NULL OR $3 = 60)
                    ), latest AS (
                        SELECT point.*
                        FROM requested
                        CROSS JOIN LATERAL (
                            SELECT candidate.*
                            FROM (
                                (
                                    SELECT retained.*
                                    FROM telemetry_rollups retained
                                    WHERE retained.client_id = requested.client_id
                                      AND ($3::INTEGER IS NULL
                                           OR retained.bucket_secs = $3)
                                      AND NOT EXISTS (
                                          SELECT 1
                                          FROM projected_suffix shadow
                                          WHERE shadow.client_id =
                                                retained.client_id
                                            AND shadow.bucket_secs =
                                                retained.bucket_secs
                                            AND shadow.bucket_start =
                                                retained.bucket_start
                                      )
                                    ORDER BY retained.bucket_start DESC,
                                             retained.latest_observed_at DESC,
                                             retained.bucket_secs ASC
                                    LIMIT 1
                                )

                                UNION ALL

                                (
                                    SELECT suffix.*
                                    FROM projected_suffix suffix
                                    WHERE suffix.client_id = requested.client_id
                                      AND ($3::INTEGER IS NULL
                                           OR suffix.bucket_secs = $3)
                                    ORDER BY suffix.bucket_start DESC,
                                             suffix.latest_observed_at DESC,
                                             suffix.bucket_secs ASC
                                    LIMIT 1
                                )
                            ) candidate
                            ORDER BY candidate.bucket_start DESC,
                                     candidate.latest_observed_at DESC,
                                     candidate.bucket_secs ASC
                            LIMIT 1
                        ) point
                    )
                    SELECT
                        client_id,
                        bucket_start::text AS bucket_start,
                        bucket_secs,
                        sample_count,
                        cpu_usage_sample_count,
                        cpu_usage_avg,
                        cpu_usage_max,
                        cpu_cores_max,
                        cpu_load_1_avg,
                        cpu_load_1_max,
                        cpu_load_5_avg,
                        cpu_load_5_max,
                        cpu_load_15_avg,
                        cpu_load_15_max,
                        memory_total_bytes_max,
                        memory_available_bytes_avg,
                        memory_available_bytes_min,
                        memory_used_ratio_avg,
                        memory_used_ratio_max,
                        swap_sample_count,
                        swap_total_bytes_max,
                        swap_available_bytes_avg,
                        swap_available_bytes_min,
                        swap_used_ratio_avg,
                        swap_used_ratio_max,
                        disk_sample_count,
                        disk_total_bytes_max,
                        disk_available_bytes_avg,
                        disk_available_bytes_min,
                        disk_used_ratio_avg,
                        disk_used_ratio_max,
                        connections_sample_count,
                        tcp_sockets_latest,
                        udp_sockets_latest,
                        connections_observed_at::text AS connections_observed_at,
                        latest_observed_at::text AS latest_observed_at,
                        updated_at::text AS updated_at
                    FROM latest
                    ORDER BY latest_observed_at DESC, client_id ASC
                    LIMIT $4
                    "#;
                let rows = sqlx::query(QUERY)
                    .bind(client_id)
                    .bind(client_ids)
                    .bind(bucket_secs)
                    .bind(result_limit.map(|limit| limit as i64))
                    .fetch_all(pool)
                    .await?;
                rows.into_iter().map(telemetry_rollup_from_row).collect()
            }
        }
    }

    pub(crate) async fn list_projected_telemetry_network_history(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<TelemetryNetworkHistoryProjection> {
        self.query_projected_telemetry_network_history(
            points_per_series,
            start_unix,
            end_unix,
            step_secs,
            selection,
            false,
        )
        .await
    }

    async fn query_projected_telemetry_network_history(
        &self,
        points_per_series: i64,
        start_unix: u64,
        end_unix: u64,
        step_secs: i32,
        selection: &NetworkRateInterfaceSelection,
        aggregate_selected_interfaces: bool,
    ) -> Result<TelemetryNetworkHistoryProjection> {
        let client_ids = selection.client_ids();
        if client_ids.is_empty() {
            return Ok(TelemetryNetworkHistoryProjection {
                rows: Vec::new(),
                complete: true,
            });
        }
        let points_per_series = points_per_series.clamp(2, 1_440) as usize;
        let step_secs = normalized_dashboard_step_secs(step_secs);
        let (all_client_ids, exact_client_ids, exact_interfaces) = selection.query_parts();
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(TELEMETRY_NETWORK_HISTORY_PROJECTION_SQL)
                    .bind(&client_ids)
                    .bind(start_unix.min(i64::MAX as u64) as i64)
                    .bind(end_unix.min(i64::MAX as u64) as i64)
                    .bind(step_secs)
                    .bind(points_per_series as i64)
                    .bind(&all_client_ids)
                    .bind(&exact_client_ids)
                    .bind(&exact_interfaces)
                    .bind(aggregate_selected_interfaces)
                    .fetch_all(pool)
                    .await?;
                let requested_clients = client_ids.iter().collect::<HashSet<_>>().len();
                let mut returned_clients = HashSet::with_capacity(requested_clients);
                let mut history =
                    Vec::with_capacity(requested_clients.saturating_mul(points_per_series));
                for row in rows {
                    if row.try_get::<bool, _>("has_point")? {
                        history.push(telemetry_network_rate_from_row(row)?);
                    } else {
                        returned_clients.insert(row.try_get::<String, _>("client_id")?);
                    }
                }
                let complete = returned_clients.len() == requested_clients;
                if !complete {
                    history.clear();
                }
                Ok(TelemetryNetworkHistoryProjection {
                    rows: history,
                    complete,
                })
            }
        }
    }

    pub(crate) async fn list_telemetry_network_rates(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
        visible_only: bool,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        match self {
            Self::Postgres(pool) => {
                let requested = limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize;
                let mut transaction = pool.begin().await?;
                sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
                    .execute(&mut *transaction)
                    .await?;

                // Visibility is one fixed set in the same snapshot as every
                // history page. Binding it keeps fleet ownership out of the
                // ordered rate scan and avoids one client lookup per row.
                let visible_client_ids = if visible_only {
                    sqlx::query_scalar::<_, String>(
                        r#"
                        SELECT client.id
                        FROM visible_clients client
                        WHERE client.status <> 'suspended'
                        ORDER BY client.id
                        "#,
                    )
                    .fetch_all(&mut *transaction)
                    .await?
                } else {
                    Vec::new()
                };
                if visible_only && visible_client_ids.is_empty() {
                    transaction.commit().await?;
                    return Ok(Vec::new());
                }

                let mut result = Vec::with_capacity(requested);
                let mut cursor: Option<RawTelemetryNetworkRateCursor> = None;
                while result.len() < requested {
                    // The public request is the page-size authority. It gives
                    // a normal all-valid request one page and avoids reducing
                    // a late refill to one round trip per invalid transition;
                    // the unbounded loop, not a larger speculative batch,
                    // owns reset/decrease refill.
                    let page_limit = requested;
                    let candidate_sql = raw_telemetry_network_rate_candidate_keys_sql(
                        client_id.is_some(),
                        interface.is_some(),
                        bucket_secs.is_some(),
                        visible_only,
                        cursor.is_some(),
                        page_limit,
                    );
                    let mut candidate_query = sqlx::query(&candidate_sql);
                    if let Some(client_id) = client_id {
                        candidate_query = candidate_query.bind(client_id);
                    }
                    if let Some(interface) = interface {
                        candidate_query = candidate_query.bind(interface);
                    }
                    if let Some(bucket_secs) = bucket_secs {
                        candidate_query = candidate_query.bind(bucket_secs);
                    }
                    if visible_only {
                        candidate_query = candidate_query.bind(&visible_client_ids);
                    }
                    if let Some(cursor) = &cursor {
                        candidate_query = candidate_query
                            .bind(cursor.latest_observed_at)
                            .bind(&cursor.client_id)
                            .bind(&cursor.interface)
                            .bind(cursor.bucket_start)
                            .bind(cursor.bucket_secs);
                    }
                    let candidate_rows = candidate_query.fetch_all(&mut *transaction).await?;
                    if candidate_rows.is_empty() {
                        break;
                    }
                    if candidate_rows.len() > page_limit {
                        anyhow::bail!("raw network candidate page exceeded its requested size");
                    }

                    let page_was_full = candidate_rows.len() == page_limit;
                    let mut candidate_keys = Vec::with_capacity(candidate_rows.len());
                    let mut next_cursor = None;
                    for row in candidate_rows {
                        if !row.try_get::<bool, _>("page_cursor_strictly_after")? {
                            anyhow::bail!("raw network candidate cursor moved out of order");
                        }
                        let key = RawTelemetryNetworkRateCursor {
                            latest_observed_at: row.try_get("latest_observed_at")?,
                            client_id: row.try_get("client_id")?,
                            interface: row.try_get("interface")?,
                            bucket_start: row.try_get("bucket_start")?,
                            bucket_secs: row.try_get("bucket_secs")?,
                        };
                        next_cursor = Some(key.clone());
                        if row.try_get::<bool, _>("admitted")? {
                            candidate_keys.push(key);
                        }
                    }
                    let next_cursor = next_cursor
                        .ok_or_else(|| anyhow::anyhow!("raw network page lost its cursor"))?;
                    if cursor.as_ref() == Some(&next_cursor) {
                        anyhow::bail!("raw network candidate cursor did not advance");
                    }

                    // Admission is intentionally after the physical page
                    // boundary so it cannot turn an ordered index stop into a
                    // history join. Rejected keys still advance this exact
                    // cursor and a full page immediately refills.
                    if candidate_keys.is_empty() {
                        cursor = Some(next_cursor);
                        if page_was_full {
                            continue;
                        }
                        break;
                    }

                    // Arrays retain candidate ordinality while keeping the
                    // payload statement's cardinality independent of generic
                    // LIMIT estimates. Thus it can only probe the page's keys
                    // and one predecessor for each key.
                    let candidate_client_ids = candidate_keys
                        .iter()
                        .map(|key| key.client_id.clone())
                        .collect::<Vec<_>>();
                    let candidate_interfaces = candidate_keys
                        .iter()
                        .map(|key| key.interface.clone())
                        .collect::<Vec<_>>();
                    let candidate_bucket_starts = candidate_keys
                        .iter()
                        .map(|key| key.bucket_start.to_owned())
                        .collect::<Vec<_>>();
                    let candidate_bucket_secs = candidate_keys
                        .iter()
                        .map(|key| key.bucket_secs)
                        .collect::<Vec<_>>();
                    let payload_sql = raw_telemetry_network_rate_payload_sql(bucket_secs.is_some());
                    let payload_rows = sqlx::query(&payload_sql)
                        .bind(&candidate_client_ids)
                        .bind(&candidate_interfaces)
                        .bind(&candidate_bucket_starts)
                        .bind(&candidate_bucket_secs)
                        .fetch_all(&mut *transaction)
                        .await?;
                    if payload_rows.len() != candidate_keys.len() {
                        anyhow::bail!("raw network payload page lost a candidate");
                    }
                    for (index, row) in payload_rows.into_iter().enumerate() {
                        let ordinal = row.try_get::<i64, _>("candidate_ordinal")?;
                        if ordinal != index as i64 + 1 {
                            anyhow::bail!("raw network payload page moved out of order");
                        }
                        if result.len() < requested && row.try_get::<bool, _>("transition_valid")? {
                            result.push(telemetry_network_rate_from_row(row)?);
                        }
                    }

                    cursor = Some(next_cursor);
                    if !page_was_full {
                        break;
                    }
                }
                transaction.commit().await?;
                Ok(result)
            }
        }
    }

    pub(crate) async fn list_latest_telemetry_network_rates(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        self.list_latest_telemetry_network_rates_matching(
            Some(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize),
            client_id,
            None,
            interface,
            bucket_secs,
            None,
            LatestNetworkRateVisibility::AdmittedOnly,
        )
        .await
    }

    pub(crate) async fn list_latest_telemetry_network_rates_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_latest_telemetry_network_rates_matching(
            None,
            None,
            Some(client_ids),
            None,
            None,
            None,
            LatestNetworkRateVisibility::AdmittedOnly,
        )
        .await
    }

    pub(crate) async fn list_latest_telemetry_network_rates_for_vps_detail(
        &self,
        client_id: &str,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        self.list_latest_telemetry_network_rates_matching(
            None,
            Some(client_id),
            None,
            None,
            None,
            None,
            LatestNetworkRateVisibility::SingleVpsDetail,
        )
        .await
    }

    pub(crate) async fn list_latest_telemetry_network_rates_for_selection(
        &self,
        selection: &NetworkRateInterfaceSelection,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        if selection.is_empty() {
            return Ok(Vec::new());
        }
        let client_ids = selection.client_ids();
        let rows = self
            .list_latest_telemetry_network_rates_matching(
                None,
                None,
                Some(&client_ids),
                None,
                None,
                Some(selection),
                LatestNetworkRateVisibility::AdmittedOnly,
            )
            .await?;
        Ok(project_network_rate_selection(rows, selection))
    }

    async fn list_latest_telemetry_network_rates_matching(
        &self,
        result_limit: Option<usize>,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        interface: Option<&str>,
        bucket_secs: Option<i32>,
        selection: Option<&NetworkRateInterfaceSelection>,
        visibility: LatestNetworkRateVisibility,
    ) -> Result<Vec<TelemetryNetworkRateView>> {
        let unrestricted_selection = selection.is_none();
        let (all_client_ids, exact_client_ids, exact_interfaces) = selection
            .map(NetworkRateInterfaceSelection::query_parts)
            .unwrap_or_default();
        match self {
            Self::Postgres(pool) => {
                let rows = if bucket_secs.is_none() {
                    // Online/current membership comes from the canonical projected
                    // sample, not a wall-age or generation heuristic. Dashboard
                    // selection owns a separate projection and cannot change this
                    // core membership. An offline client deliberately keeps its
                    // bounded last-known projection even if the protected raw
                    // envelope is unavailable.
                    sqlx::query(
                        r#"
                        WITH online_current_interfaces AS MATERIALIZED (
                            SELECT
                                projection.client_id,
                                array_agg(
                                    projected_network.value ->> 'interface'
                                ) AS interfaces
                            FROM telemetry_projection_heads projection
                            JOIN visible_clients online_client
                              ON online_client.id = projection.client_id
                             AND online_client.status = 'online'
                            JOIN telemetry_samples latest
                              ON latest.id = projection.latest_projected_sample_id
                             AND latest.client_id = projection.client_id
                            CROSS JOIN LATERAL jsonb_array_elements(
                                CASE
                                    WHEN jsonb_typeof(
                                        latest.payload -> 'networks'
                                    ) = 'array'
                                    THEN latest.payload -> 'networks'
                                    ELSE '[]'::jsonb
                                END
                            ) WITH ORDINALITY AS projected_network(value, ordinality)
                            WHERE ($1::TEXT IS NULL OR projection.client_id = $1)
                              AND ($2::TEXT[] IS NULL OR projection.client_id = ANY($2))
                              AND (
                                  $5::BOOLEAN
                                  OR projection.client_id = ANY($6::TEXT[])
                                  OR projection.client_id = ANY($7::TEXT[])
                              )
                              AND CASE
                                  WHEN NOT public.telemetry_ordinal_admission_mask_is_exact(
                                      latest.network_admission_mask,
                                      CASE
                                          WHEN jsonb_typeof(
                                              latest.payload -> 'networks'
                                          ) = 'array'
                                          THEN jsonb_array_length(
                                              latest.payload -> 'networks'
                                          )::BIGINT
                                          ELSE 0
                                      END
                                  ) THEN FALSE
                                  ELSE get_bit(
                                      latest.network_admission_mask,
                                      (projected_network.ordinality - 1)::INTEGER
                                  ) = 1
                              END
                            GROUP BY projection.client_id
                        ), policy_clients AS MATERIALIZED (
                            SELECT client.id
                            FROM visible_clients client
                            WHERE client.status <> 'suspended'
                              AND ($1::TEXT IS NULL OR client.id = $1)
                              AND ($2::TEXT[] IS NULL OR client.id = ANY($2))
                              AND (
                                  $5::BOOLEAN
                                  OR client.id = ANY($6::TEXT[])
                                  OR client.id = ANY($7::TEXT[])
                              )
                        ), resolved_interface_policies AS MATERIALIZED (
                            SELECT policy.*
                            FROM public.resolve_telemetry_interface_policies(ARRAY(
                                SELECT client.id
                                FROM policy_clients client
                                ORDER BY client.id
                            )) policy
                        )
                        SELECT
                            network_current.client_id,
                            network_current.interface,
                            network_current.latest_bucket_start::text AS bucket_start,
                            network_current.latest_bucket_secs AS bucket_secs,
                            network_current.latest_sample_count AS sample_count,
                            network_current.latest_rx_bytes_avg AS rx_bytes_avg,
                            network_current.latest_tx_bytes_avg AS tx_bytes_avg,
                            network_current.latest_rx_bytes AS rx_bytes_last,
                            network_current.latest_tx_bytes AS tx_bytes_last,
                            network_current.latest_rx_counter_epoch AS rx_counter_epoch,
                            network_current.latest_tx_counter_epoch AS tx_counter_epoch,
                            network_current.rx_bytes_delta,
                            network_current.tx_bytes_delta,
                            network_current.rx_bps_avg,
                            network_current.tx_bps_avg,
                            network_current.latest_observed_at::text AS latest_observed_at,
                            network_current.updated_at::text AS updated_at
                        FROM telemetry_network_current_source(ARRAY(
                            SELECT client.id
                            FROM policy_clients client
                            ORDER BY client.id
                        )) network_current
                        JOIN telemetry_projection_heads projection
                          ON projection.client_id = network_current.client_id
                        JOIN visible_clients client
                          ON client.id = network_current.client_id
                         AND client.status <> 'suspended'
                        JOIN resolved_interface_policies policy
                          ON policy.client_id = network_current.client_id
                        LEFT JOIN online_current_interfaces current_interfaces
                          ON current_interfaces.client_id = network_current.client_id
                        CROSS JOIN LATERAL (
                            SELECT public.telemetry_interface_is_admitted_resolved(
                                policy.admission_mode,
                                policy.interface_patterns,
                                policy.managed_tunnel_interfaces,
                                'host',
                                network_current.interface
                            ) AS admitted
                            OFFSET 0
                        ) interface_policy
                        WHERE network_current.transition_valid
                          AND network_current.transition_admitted_at_projection
                          AND interface_policy.admitted
                          AND (
                              client.status <> 'online'
                              OR network_current.interface = ANY(
                                  current_interfaces.interfaces
                              )
                          )
                          AND ($1::TEXT IS NULL OR network_current.client_id = $1)
                          AND ($2::TEXT[] IS NULL OR network_current.client_id = ANY($2))
                          AND ($3::TEXT IS NULL OR network_current.interface = $3)
                          AND (
                              $5::BOOLEAN
                              OR network_current.client_id = ANY($6::TEXT[])
                              OR EXISTS (
                                  SELECT 1
                                  FROM UNNEST($7::TEXT[], $8::TEXT[])
                                      AS selected(client_id, interface)
                                  WHERE selected.client_id = network_current.client_id
                                    AND selected.interface = network_current.interface
                              )
                          )
                        ORDER BY network_current.latest_observed_at DESC,
                                 network_current.client_id ASC,
                                 network_current.interface ASC
                        LIMIT $4
                        "#,
                    )
                    .bind(client_id)
                    .bind(client_ids)
                    .bind(interface)
                    .bind(result_limit.map(|limit| limit as i64))
                    .bind(unrestricted_selection)
                    .bind(&all_client_ids)
                    .bind(&exact_client_ids)
                    .bind(&exact_interfaces)
                    .fetch_all(pool)
                    .await?
                } else {
                    // Explicit physical-tier inspection keeps its historical API
                    // semantics; endpoint snapshots use canonical current rows.
                    sqlx::query(LATEST_TELEMETRY_NETWORK_RATES_SQL)
                        .bind(client_id)
                        .bind(client_ids)
                        .bind(interface)
                        .bind(bucket_secs)
                        .bind(result_limit.map(|limit| limit as i64))
                        .bind(unrestricted_selection)
                        .bind(&all_client_ids)
                        .bind(&exact_client_ids)
                        .bind(&exact_interfaces)
                        .fetch_all(pool)
                        .await?
                };
                let mut result = rows
                    .into_iter()
                    .map(telemetry_network_rate_from_row)
                    .collect::<Result<Vec<_>>>()?;
                if bucket_secs.is_none()
                    && visibility == LatestNetworkRateVisibility::SingleVpsDetail
                {
                    let detail_client_id = client_id.ok_or_else(|| {
                        anyhow::anyhow!("the recent excluded-interface reader requires one VPS")
                    })?;
                    let excluded = sqlx::query(RECENT_EXCLUDED_NETWORK_TRANSITIONS_SQL)
                        .bind(detail_client_id)
                        .bind(interface)
                        .fetch_all(pool)
                        .await?;
                    result.extend(
                        excluded
                            .into_iter()
                            .map(telemetry_network_rate_from_row)
                            .collect::<Result<Vec<_>>>()?,
                    );
                    result.sort_by(|left, right| {
                        compare_timestamps_desc(&left.latest_observed_at, &right.latest_observed_at)
                            .then_with(|| left.client_id.cmp(&right.client_id))
                            .then_with(|| left.interface.cmp(&right.interface))
                    });
                }
                if let Some(limit) = result_limit {
                    result.truncate(limit);
                }
                Ok(result)
            }
        }
    }

    pub(crate) async fn list_telemetry_tunnels(
        &self,
        limit: i64,
        client_id: Option<&str>,
        interface: Option<&str>,
    ) -> Result<Vec<TelemetryTunnelView>> {
        self.list_telemetry_tunnels_matching(
            Some(limit.clamp(1, TELEMETRY_LIST_LIMIT_MAX) as usize),
            client_id,
            None,
            interface,
            false,
            None,
            None,
            None,
            TunnelCounterVisibility::AdmittedOnly,
        )
        .await
    }

    pub(crate) async fn list_declared_telemetry_tunnels_for_clients(
        &self,
        client_ids: &[String],
    ) -> Result<Vec<TelemetryTunnelView>> {
        if client_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.list_telemetry_tunnels_matching(
            None,
            None,
            Some(client_ids),
            None,
            false,
            None,
            None,
            None,
            TunnelCounterVisibility::AdmittedOnly,
        )
        .await
    }

    pub(crate) async fn list_telemetry_tunnels_for_vps_detail(
        &self,
        client_id: &str,
    ) -> Result<Vec<TelemetryTunnelView>> {
        self.list_telemetry_tunnels_matching(
            None,
            Some(client_id),
            None,
            None,
            false,
            None,
            None,
            None,
            TunnelCounterVisibility::SingleVpsDetail,
        )
        .await
    }

    async fn list_telemetry_tunnels_matching(
        &self,
        result_limit: Option<usize>,
        client_id: Option<&str>,
        client_ids: Option<&[String]>,
        interface: Option<&str>,
        alert_candidates_only: bool,
        severity: Option<&str>,
        start_unix: Option<u64>,
        end_unix: Option<u64>,
        counter_visibility: TunnelCounterVisibility,
    ) -> Result<Vec<TelemetryTunnelView>> {
        match self {
            Self::Postgres(pool) => {
                let rows = sqlx::query(
                    r#"
                    WITH visible_status AS MATERIALIZED (
                        SELECT id, status
                        FROM visible_clients
                    ), policy_clients AS MATERIALIZED (
                        SELECT visible.id
                        FROM visible_status visible
                        WHERE visible.status <> 'suspended'
                          AND ($1::TEXT IS NULL OR visible.id = $1)
                          AND ($2::TEXT[] IS NULL OR visible.id = ANY($2))
                    ), resolved_interface_policies AS MATERIALIZED (
                        SELECT policy.*
                        FROM public.resolve_telemetry_interface_policies(
                            CASE WHEN $9::BOOLEAN
                                 THEN ARRAY[]::TEXT[]
                                 ELSE ARRAY(
                                     SELECT client.id
                                     FROM policy_clients client
                                     ORDER BY client.id
                                 )
                            END
                        ) policy
                    )
                    SELECT
                        telemetry.client_id,
                        telemetry.observed_at::text AS observed_at,
                        telemetry.updated_at::text AS accepted_at,
                        telemetry.interface,
                        telemetry.kind AS kind,
                        current_plan_policy.ownership_mode,
                        CASE current_plan_policy.ownership_mode
                            WHEN 'external_observed' THEN 'observe_only_saved_plan'
                            WHEN 'agent_builtin' THEN 'managed_desired'
                            WHEN 'custom_adapter' THEN 'managed_desired'
                        END AS mutation_policy,
                        source,
                        operstate,
                        mtu,
                        link_type,
                        address,
                        CASE WHEN (
                                telemetry.counters_admitted_at_projection
                                AND interface_policy.admitted
                            ) OR (
                                $9::BOOLEAN
                                AND telemetry.observed_at >=
                                    clock_timestamp() - INTERVAL '15 minutes'
                            )
                            THEN telemetry.rx_bytes
                        END AS rx_bytes,
                        CASE WHEN (
                                telemetry.counters_admitted_at_projection
                                AND interface_policy.admitted
                            ) OR (
                                $9::BOOLEAN
                                AND telemetry.observed_at >=
                                    clock_timestamp() - INTERVAL '15 minutes'
                            )
                            THEN telemetry.tx_bytes
                        END AS tx_bytes,
                        traffic_source,
                        traffic_status,
                        traffic_reason,
                        traffic_checked_unix,
                        telemetry_plan_id,
                        telemetry_topology_identity_hash,
                        telemetry_runtime_evidence_identity_hash,
                        telemetry_plan_name,
                        current_plan_policy.ownership_mode
                            AS telemetry_plan_runtime_manager,
                        telemetry_endpoint_side,
                        telemetry_peer_client_id,
                        adapter_health,
                        latency_monitoring_enabled,
                        latency_status,
                        latency_reason,
                        latency_primary_family,
                        latency_target,
                        latency_checked_unix,
                        latency_avg_ms,
                        packet_loss_ratio,
                        latency_healthy_windows,
                        latency_missed_windows
                    FROM telemetry_current_tunnels telemetry
                    JOIN visible_status visible_client
                      ON visible_client.id = telemetry.client_id
                     AND visible_client.status <> 'suspended'
                    LEFT JOIN visible_status visible_peer
                      ON visible_peer.id = telemetry.telemetry_peer_client_id
                    LEFT JOIN resolved_interface_policies policy
                      ON policy.client_id = telemetry.client_id
                    CROSS JOIN LATERAL (
                        SELECT COALESCE(
                            telemetry.current_plan#>>'{runtime_control,manager}',
                            'agent_builtin'
                        ) AS ownership_mode
                        OFFSET 0
                    ) current_plan_policy
                    CROSS JOIN LATERAL (
                        SELECT CASE WHEN $9::BOOLEAN THEN FALSE ELSE
                            public.telemetry_interface_is_admitted_resolved(
                                policy.admission_mode,
                                policy.interface_patterns,
                                policy.managed_tunnel_interfaces,
                                'tunnel',
                                telemetry.interface
                            )
                        END AS admitted
                        OFFSET 0
                    ) interface_policy
                    WHERE ($1::TEXT IS NULL OR telemetry.client_id = $1)
                      AND ($2::TEXT[] IS NULL OR telemetry.client_id = ANY($2))
                      AND ($3::TEXT IS NULL OR telemetry.interface = $3)
                      AND visible_peer.status IS DISTINCT FROM 'suspended'
                      AND (
                        NOT $9::BOOLEAN
                        OR telemetry.observed_at >=
                            clock_timestamp() - INTERVAL '15 minutes'
                      )
                      AND (
                        $6::DOUBLE PRECISION IS NULL
                        OR telemetry.observed_at >= to_timestamp($6)
                      )
                      AND (
                        $7::DOUBLE PRECISION IS NULL
                        OR telemetry.observed_at <= to_timestamp($7)
                      )
                      AND (
                        NOT $4::BOOLEAN
                        OR (
                            ($5::TEXT IS NULL OR $5 = 'critical')
                            AND current_plan_policy.ownership_mode = 'custom_adapter'
                            AND jsonb_typeof(telemetry.adapter_health) = 'object'
                            AND jsonb_typeof(telemetry.adapter_health->'status') = 'string'
                            AND telemetry.adapter_health->'success'
                                IS DISTINCT FROM 'true'::jsonb
                        )
                        OR (
                            ($5::TEXT IS NULL OR $5 = 'warning')
                            AND telemetry.traffic_status IS NOT NULL
                            AND telemetry.traffic_status <> 'ok'
                        )
                    )
                    ORDER BY
                        CASE
                            WHEN $4::BOOLEAN
                             AND $5::TEXT IS NULL
                             AND current_plan_policy.ownership_mode = 'custom_adapter'
                             AND jsonb_typeof(telemetry.adapter_health) = 'object'
                             AND jsonb_typeof(telemetry.adapter_health->'status') = 'string'
                             AND telemetry.adapter_health->'success'
                                IS DISTINCT FROM 'true'::jsonb
                            THEN 0
                            ELSE 1
                        END ASC,
                        telemetry.observed_at DESC,
                        telemetry.client_id ASC,
                        telemetry.interface ASC
                    LIMIT $8
                    "#,
                )
                .bind(client_id)
                .bind(client_ids)
                .bind(interface)
                .bind(alert_candidates_only)
                .bind(severity)
                .bind(start_unix.map(|value| value as f64))
                .bind(end_unix.map(|value| value as f64))
                .bind(result_limit.map(|limit| limit as i64))
                .bind(counter_visibility == TunnelCounterVisibility::SingleVpsDetail)
                .fetch_all(pool)
                .await?;

                let mut records = rows
                    .into_iter()
                    .map(|row| {
                        let telemetry_plan_id = row.try_get("telemetry_plan_id")?;
                        let telemetry_plan_name = row.try_get("telemetry_plan_name")?;
                        Ok(TelemetryTunnelView {
                            client_id: row.try_get("client_id")?,
                            observed_at: row.try_get("observed_at")?,
                            accepted_at: row.try_get("accepted_at")?,
                            interface: row.try_get("interface")?,
                            kind: row.try_get("kind")?,
                            ownership_mode: row.try_get("ownership_mode")?,
                            mutation_policy: row.try_get("mutation_policy")?,
                            plan_id: telemetry_plan_id,
                            topology_identity_hash: row
                                .try_get("telemetry_topology_identity_hash")?,
                            runtime_evidence_identity_hash: row
                                .try_get("telemetry_runtime_evidence_identity_hash")?,
                            plan_name: telemetry_plan_name,
                            plan_runtime_manager: row.try_get("telemetry_plan_runtime_manager")?,
                            endpoint_side: row.try_get("telemetry_endpoint_side")?,
                            peer_client_id: row.try_get("telemetry_peer_client_id")?,
                            source: row.try_get("source")?,
                            operstate: row.try_get("operstate")?,
                            mtu: row.try_get("mtu")?,
                            link_type: row.try_get("link_type")?,
                            address: row.try_get("address")?,
                            rx_bytes: row.try_get("rx_bytes")?,
                            tx_bytes: row.try_get("tx_bytes")?,
                            traffic_source: row.try_get("traffic_source")?,
                            traffic_status: row.try_get("traffic_status")?,
                            traffic_reason: row.try_get("traffic_reason")?,
                            traffic_checked_unix: row.try_get("traffic_checked_unix")?,
                            adapter_health: parse_adapter_health(row.try_get("adapter_health")?),
                            latency_monitoring_enabled: row
                                .try_get("latency_monitoring_enabled")?,
                            latency_status: row.try_get("latency_status")?,
                            latency_reason: row.try_get("latency_reason")?,
                            latency_primary_family: row.try_get("latency_primary_family")?,
                            latency_target: row.try_get("latency_target")?,
                            latency_checked_unix: row.try_get("latency_checked_unix")?,
                            latency_avg_ms: row.try_get("latency_avg_ms")?,
                            packet_loss_ratio: row.try_get("packet_loss_ratio")?,
                            latency_healthy_windows: row.try_get("latency_healthy_windows")?,
                            latency_missed_windows: row.try_get("latency_missed_windows")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                if alert_candidates_only {
                    records.retain(|record| {
                        telemetry_tunnel_matches_alert_candidate(record, severity)
                    });
                }
                Ok(records)
            }
        }
    }
}

pub(crate) fn tunnel_adapter_health_is_degraded(tunnel: &TelemetryTunnelView) -> bool {
    tunnel.plan_runtime_manager.as_deref() == Some("custom_adapter")
        && tunnel
            .adapter_health
            .as_ref()
            .is_some_and(|health| !health.success)
}

fn telemetry_tunnel_matches_alert_candidate(
    tunnel: &TelemetryTunnelView,
    severity: Option<&str>,
) -> bool {
    ((severity.is_none() || severity == Some("critical"))
        && tunnel_adapter_health_is_degraded(tunnel))
        || ((severity.is_none() || severity == Some("warning"))
            && tunnel
                .traffic_status
                .as_deref()
                .is_some_and(|status| status != "ok"))
}

fn parse_adapter_health(
    value: Option<serde_json::Value>,
) -> Option<TelemetryTunnelAdapterHealthView> {
    let value = value?;
    if !value.is_object() {
        return None;
    }
    Some(TelemetryTunnelAdapterHealthView {
        status: value.get("status")?.as_str()?.to_string(),
        checked_unix: value
            .get("checked_unix")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        configured: value
            .get("configured")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        success: value
            .get("success")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        exit_code: value
            .get("exit_code")
            .and_then(|value| value.as_i64())
            .and_then(|value| i32::try_from(value).ok()),
        reason: value
            .get("reason")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        duration_ms: value
            .get("duration_ms")
            .and_then(|value| value.as_i64())
            .unwrap_or(0),
        command_sha256_hex: value
            .get("command_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        timed_out: value
            .get("timed_out")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        output_truncated: value
            .get("output_truncated")
            .and_then(|value| value.as_bool())
            .unwrap_or(false),
        stdout_sha256_hex: value
            .get("stdout_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        stderr_sha256_hex: value
            .get("stderr_sha256_hex")
            .and_then(|value| value.as_str())
            .map(str::to_string),
    })
}

fn telemetry_rollup_from_row(row: sqlx::postgres::PgRow) -> Result<TelemetryRollupView> {
    Ok(TelemetryRollupView {
        client_id: row.try_get("client_id")?,
        bucket_start: row.try_get("bucket_start")?,
        bucket_secs: row.try_get("bucket_secs")?,
        sample_count: row.try_get("sample_count")?,
        cpu_usage_sample_count: row.try_get("cpu_usage_sample_count")?,
        cpu_usage_avg: row.try_get("cpu_usage_avg")?,
        cpu_usage_max: row.try_get("cpu_usage_max")?,
        cpu_cores_max: row.try_get("cpu_cores_max")?,
        cpu_load_1_avg: row.try_get("cpu_load_1_avg")?,
        cpu_load_1_max: row.try_get("cpu_load_1_max")?,
        cpu_load_5_avg: row.try_get("cpu_load_5_avg")?,
        cpu_load_5_max: row.try_get("cpu_load_5_max")?,
        cpu_load_15_avg: row.try_get("cpu_load_15_avg")?,
        cpu_load_15_max: row.try_get("cpu_load_15_max")?,
        memory_total_bytes_max: row.try_get("memory_total_bytes_max")?,
        memory_available_bytes_avg: row.try_get("memory_available_bytes_avg")?,
        memory_available_bytes_min: row.try_get("memory_available_bytes_min")?,
        memory_used_ratio_avg: row.try_get("memory_used_ratio_avg")?,
        memory_used_ratio_max: row.try_get("memory_used_ratio_max")?,
        swap_sample_count: row.try_get("swap_sample_count")?,
        swap_total_bytes_max: row.try_get("swap_total_bytes_max")?,
        swap_available_bytes_avg: row.try_get("swap_available_bytes_avg")?,
        swap_available_bytes_min: row.try_get("swap_available_bytes_min")?,
        swap_used_ratio_avg: row.try_get("swap_used_ratio_avg")?,
        swap_used_ratio_max: row.try_get("swap_used_ratio_max")?,
        disk_sample_count: row.try_get("disk_sample_count")?,
        disk_total_bytes_max: row.try_get("disk_total_bytes_max")?,
        disk_available_bytes_avg: row.try_get("disk_available_bytes_avg")?,
        disk_available_bytes_min: row.try_get("disk_available_bytes_min")?,
        disk_used_ratio_avg: row.try_get("disk_used_ratio_avg")?,
        disk_used_ratio_max: row.try_get("disk_used_ratio_max")?,
        connections_sample_count: row.try_get("connections_sample_count")?,
        tcp_sockets_latest: row.try_get("tcp_sockets_latest")?,
        udp_sockets_latest: row.try_get("udp_sockets_latest")?,
        connections_observed_at: row.try_get("connections_observed_at")?,
        latest_observed_at: row.try_get("latest_observed_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn telemetry_network_rate_from_row(row: sqlx::postgres::PgRow) -> Result<TelemetryNetworkRateView> {
    Ok(TelemetryNetworkRateView {
        client_id: row.try_get("client_id")?,
        interface: row.try_get("interface")?,
        bucket_start: row.try_get("bucket_start")?,
        bucket_secs: row.try_get("bucket_secs")?,
        sample_count: row.try_get("sample_count")?,
        rx_bytes_avg: row.try_get("rx_bytes_avg")?,
        tx_bytes_avg: row.try_get("tx_bytes_avg")?,
        latest_observed_at: row.try_get("latest_observed_at")?,
        rx_bytes_delta: row.try_get("rx_bytes_delta")?,
        tx_bytes_delta: row.try_get("tx_bytes_delta")?,
        rx_bps_avg: row.try_get("rx_bps_avg")?,
        tx_bps_avg: row.try_get("tx_bps_avg")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn project_network_rate_selection(
    rows: Vec<TelemetryNetworkRateView>,
    selection: &NetworkRateInterfaceSelection,
) -> Vec<TelemetryNetworkRateView> {
    rows.into_iter()
        .filter(|row| selection.allows(&row.client_id, &row.interface))
        .collect()
}

#[cfg(test)]
pub(crate) fn aggregate_selected_network_history_oracle(
    rows: Vec<TelemetryNetworkRateView>,
) -> Vec<TelemetryNetworkRateView> {
    let mut points =
        std::collections::BTreeMap::<(String, String, i32), TelemetryNetworkRateView>::new();
    for mut row in rows {
        let key = (
            row.client_id.clone(),
            row.bucket_start.clone(),
            row.bucket_secs,
        );
        match points.entry(key) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                row.interface.clear();
                entry.insert(row);
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let point = entry.get_mut();
                point.sample_count = point.sample_count.max(row.sample_count);
                point.rx_bytes_avg = point.rx_bytes_avg.saturating_add(row.rx_bytes_avg);
                point.tx_bytes_avg = point.tx_bytes_avg.saturating_add(row.tx_bytes_avg);
                point.rx_bytes_delta = point.rx_bytes_delta.saturating_add(row.rx_bytes_delta);
                point.tx_bytes_delta = point.tx_bytes_delta.saturating_add(row.tx_bytes_delta);
                point.rx_bps_avg += row.rx_bps_avg;
                point.tx_bps_avg += row.tx_bps_avg;
                if row.latest_observed_at > point.latest_observed_at {
                    point.latest_observed_at = row.latest_observed_at;
                }
                if row.updated_at > point.updated_at {
                    point.updated_at = row.updated_at;
                }
            }
        }
    }
    points.into_values().collect()
}

fn normalized_dashboard_step_secs(step_secs: i32) -> i32 {
    step_secs.max(60).saturating_add(59) / 60 * 60
}

#[cfg(test)]
#[path = "tests_repository_telemetry_rollups.rs"]
mod fairness_tests;
