-- Fleet alert triage bulk mutations use revision 0 to represent an absent
-- state row. Existing rows have already accepted at least one mutation, so
-- preserve that history as revision 1 during the upgrade.
ALTER TABLE fleet_alert_states
    ADD COLUMN revision BIGINT NOT NULL DEFAULT 0;

UPDATE fleet_alert_states
SET revision = 1;

ALTER TABLE fleet_alert_states
    ALTER COLUMN revision SET DEFAULT 1,
    ADD CONSTRAINT fleet_alert_states_revision_check CHECK (revision >= 0);

-- Current-cycle traffic accounting used to window every retained minute for
-- every selected stream on each read. Keep an exact, transactionally
-- maintained hourly ledger of those transitions instead. A transition belongs
-- to its later sample, matching the raw accounting oracle and the retention
-- rollup convention. The current partial hour remains a bounded raw read.
CREATE TABLE traffic_counter_hourly_usage (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    interface TEXT NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    rx_bytes BIGINT NOT NULL DEFAULT 0,
    tx_bytes BIGINT NOT NULL DEFAULT 0,
    rx_reset_count INTEGER NOT NULL DEFAULT 0,
    tx_reset_count INTEGER NOT NULL DEFAULT 0,
    sample_count INTEGER NOT NULL,
    first_observed_at TIMESTAMPTZ NOT NULL,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, source_kind, interface, bucket_start),
    CHECK (source_kind IN ('host', 'tunnel')),
    CHECK (length(interface) BETWEEN 1 AND 128),
    CHECK (
        bucket_start = date_bin(
            interval '1 hour',
            bucket_start,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        )
    ),
    CHECK (rx_bytes >= 0 AND tx_bytes >= 0),
    CHECK (rx_reset_count >= 0 AND tx_reset_count >= 0),
    CHECK (sample_count > 0),
    CHECK (
        first_observed_at >= bucket_start
        AND latest_observed_at < bucket_start + interval '1 hour'
        AND first_observed_at <= latest_observed_at
    )
);

-- Source and materialized revisions are advanced in the same transaction as a
-- raw mutation. Readers only use the hourly ledger when these match; a missing
-- or deliberately dirtied coverage row selects the original raw-LAG oracle.
CREATE TABLE traffic_counter_hourly_usage_streams (
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    source_kind TEXT NOT NULL,
    interface TEXT NOT NULL,
    source_revision BIGINT NOT NULL DEFAULT 0,
    materialized_revision BIGINT NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (client_id, source_kind, interface),
    CHECK (source_kind IN ('host', 'tunnel')),
    CHECK (length(interface) BETWEEN 1 AND 128),
    CHECK (source_revision >= 0),
    CHECK (materialized_revision >= 0),
    CHECK (materialized_revision <= source_revision)
);

CREATE OR REPLACE FUNCTION refresh_traffic_counter_hourly_usage(
    changed_client_ids TEXT[],
    changed_source_kinds TEXT[],
    changed_interfaces TEXT[],
    changed_observed_at TIMESTAMPTZ[],
    rebuild_entire_streams BOOLEAN DEFAULT FALSE
) RETURNS VOID
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

    -- A missing stream marker may be a brand-new stream or an explicitly
    -- damaged cache. A mismatched revision is definitely incomplete. Rebuild
    -- the complete changed streams in either case before declaring them
    -- healthy; subsequent ordinary mutations may use the narrower repair.
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
    ), FALSE)
    INTO coverage_requires_rebuild
    FROM changed_streams changed
    JOIN clients ON clients.id = changed.client_id
    LEFT JOIN traffic_counter_hourly_usage_streams streams
      ON streams.client_id = changed.client_id
     AND streams.source_kind = changed.source_kind
     AND streams.interface = changed.interface;

    INSERT INTO traffic_counter_hourly_usage_streams (
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
            traffic_counter_hourly_usage_streams.source_revision + 1,
        updated_at = now();

    -- A large import or whole-stream epoch rewrite is cheaper and equally
    -- bounded when rebuilt with one ordered pass over each changed stream.
    -- Small live batches take the narrower changed/successor-hour path below.
    IF rebuild_entire_streams
       OR coverage_requires_rebuild
       OR array_length(changed_client_ids, 1) > 4096 THEN
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
        ), sequenced AS MATERIALIZED (
            SELECT
                sample.*,
                LAG(rx_bytes) OVER stream AS previous_rx_bytes,
                LAG(tx_bytes) OVER stream AS previous_tx_bytes,
                LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
                LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
                LAG(sample_source) OVER stream AS previous_sample_source
            FROM traffic_counter_samples sample
            JOIN changed_streams changed
              ON changed.client_id = sample.client_id
             AND changed.source_kind = sample.source_kind
             AND changed.interface = sample.interface
            WINDOW stream AS (
                PARTITION BY sample.client_id, sample.source_kind, sample.interface
                ORDER BY sample.observed_at
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
            date_bin(
                interval '1 hour',
                observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            ),
            COALESCE(SUM(CASE
                WHEN rx_counter_epoch = previous_rx_counter_epoch
                 AND rx_bytes >= previous_rx_bytes
                THEN rx_bytes - previous_rx_bytes ELSE 0 END
            ), 0)::bigint,
            COALESCE(SUM(CASE
                WHEN tx_counter_epoch = previous_tx_counter_epoch
                 AND tx_bytes >= previous_tx_bytes
                THEN tx_bytes - previous_tx_bytes ELSE 0 END
            ), 0)::bigint,
            COUNT(*) FILTER (
                WHERE previous_rx_counter_epoch IS NOT NULL
                  AND rx_counter_epoch <> previous_rx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::integer,
            COUNT(*) FILTER (
                WHERE previous_tx_counter_epoch IS NOT NULL
                  AND tx_counter_epoch <> previous_tx_counter_epoch
                  AND NOT (
                      previous_sample_source LIKE 'vnstat_import:%'
                      AND sample_source NOT LIKE 'vnstat_import:%'
                  )
            )::integer,
            COUNT(*)::integer,
            MIN(observed_at),
            MAX(observed_at),
            now()
        FROM sequenced
        GROUP BY
            client_id,
            source_kind,
            interface,
            date_bin(
                interval '1 hour',
                observed_at,
                TIMESTAMPTZ '1970-01-01 00:00:00+00'
            );

        UPDATE traffic_counter_hourly_usage_streams streams
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
            sample.sample_source
        FROM affected
        JOIN LATERAL (
            (
                SELECT
                    sample.observed_at,
                    sample.rx_bytes,
                    sample.tx_bytes,
                    sample.rx_counter_epoch,
                    sample.tx_counter_epoch,
                    sample.sample_source
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
                sample.sample_source
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
                WHEN rx_counter_epoch = previous_rx_counter_epoch
                 AND rx_bytes >= previous_rx_bytes
                THEN rx_bytes - previous_rx_bytes
                ELSE 0
            END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::bigint,
        COALESCE(SUM(
            CASE
                WHEN tx_counter_epoch = previous_tx_counter_epoch
                 AND tx_bytes >= previous_tx_bytes
                THEN tx_bytes - previous_tx_bytes
                ELSE 0
            END
        ) FILTER (WHERE observed_at >= bucket_start), 0)::bigint,
        COUNT(*) FILTER (
            WHERE observed_at >= bucket_start
              AND previous_rx_counter_epoch IS NOT NULL
              AND rx_counter_epoch <> previous_rx_counter_epoch
              AND NOT (
                  previous_sample_source LIKE 'vnstat_import:%'
                  AND sample_source NOT LIKE 'vnstat_import:%'
              )
        )::integer,
        COUNT(*) FILTER (
            WHERE observed_at >= bucket_start
              AND previous_tx_counter_epoch IS NOT NULL
              AND tx_counter_epoch <> previous_tx_counter_epoch
              AND NOT (
                  previous_sample_source LIKE 'vnstat_import:%'
                  AND sample_source NOT LIKE 'vnstat_import:%'
              )
        )::integer,
        COUNT(*) FILTER (WHERE observed_at >= bucket_start)::integer,
        MIN(observed_at) FILTER (WHERE observed_at >= bucket_start),
        MAX(observed_at) FILTER (WHERE observed_at >= bucket_start),
        now()
    FROM sequenced
    GROUP BY client_id, source_kind, interface, bucket_start
    HAVING COUNT(*) FILTER (WHERE observed_at >= bucket_start) > 0;

    UPDATE traffic_counter_hourly_usage_streams streams
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

CREATE OR REPLACE FUNCTION refresh_traffic_counter_hourly_usage_after_insert()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
    changed_count BIGINT;
BEGIN
    SELECT count(*) INTO changed_count FROM new_traffic_counter_samples;
    IF changed_count > 4096 THEN
        SELECT
            array_agg(client_id ORDER BY client_id, source_kind, interface),
            array_agg(source_kind ORDER BY client_id, source_kind, interface),
            array_agg(interface ORDER BY client_id, source_kind, interface),
            array_agg(observed_at ORDER BY client_id, source_kind, interface)
        INTO client_ids, source_kinds, interfaces, observed_values
        FROM (
            SELECT
                client_id,
                source_kind,
                interface,
                MIN(observed_at) AS observed_at
            FROM new_traffic_counter_samples
            GROUP BY client_id, source_kind, interface
        ) changed_streams;
        PERFORM refresh_traffic_counter_hourly_usage(
            client_ids, source_kinds, interfaces, observed_values, TRUE
        );
        RETURN NULL;
    END IF;
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
$$;

CREATE OR REPLACE FUNCTION refresh_traffic_counter_hourly_usage_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
    changed_count BIGINT;
BEGIN
    SELECT count(*) INTO changed_count
    FROM (
        SELECT client_id, source_kind, interface, observed_at
        FROM old_traffic_counter_samples
        UNION
        SELECT client_id, source_kind, interface, observed_at
        FROM new_traffic_counter_samples
    ) changed;
    IF changed_count > 4096 THEN
        WITH changed AS (
            SELECT client_id, source_kind, interface, observed_at
            FROM old_traffic_counter_samples
            UNION
            SELECT client_id, source_kind, interface, observed_at
            FROM new_traffic_counter_samples
        )
        SELECT
            array_agg(client_id ORDER BY client_id, source_kind, interface),
            array_agg(source_kind ORDER BY client_id, source_kind, interface),
            array_agg(interface ORDER BY client_id, source_kind, interface),
            array_agg(observed_at ORDER BY client_id, source_kind, interface)
        INTO client_ids, source_kinds, interfaces, observed_values
        FROM (
            SELECT
                client_id,
                source_kind,
                interface,
                MIN(observed_at) AS observed_at
            FROM changed
            GROUP BY client_id, source_kind, interface
        ) changed_streams;
        PERFORM refresh_traffic_counter_hourly_usage(
            client_ids, source_kinds, interfaces, observed_values, TRUE
        );
        RETURN NULL;
    END IF;
    WITH changed AS (
        SELECT client_id, source_kind, interface, observed_at
        FROM old_traffic_counter_samples
        UNION
        SELECT client_id, source_kind, interface, observed_at
        FROM new_traffic_counter_samples
    )
    SELECT
        array_agg(client_id ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(source_kind ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(interface ORDER BY client_id, source_kind, interface, observed_at),
        array_agg(observed_at ORDER BY client_id, source_kind, interface, observed_at)
    INTO client_ids, source_kinds, interfaces, observed_values
    FROM changed;
    PERFORM refresh_traffic_counter_hourly_usage(
        client_ids, source_kinds, interfaces, observed_values
    );
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION refresh_traffic_counter_hourly_usage_after_delete()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
DECLARE
    client_ids TEXT[];
    source_kinds TEXT[];
    interfaces TEXT[];
    observed_values TIMESTAMPTZ[];
    changed_count BIGINT;
BEGIN
    SELECT count(*) INTO changed_count FROM old_traffic_counter_samples;
    IF changed_count > 4096 THEN
        SELECT
            array_agg(client_id ORDER BY client_id, source_kind, interface),
            array_agg(source_kind ORDER BY client_id, source_kind, interface),
            array_agg(interface ORDER BY client_id, source_kind, interface),
            array_agg(observed_at ORDER BY client_id, source_kind, interface)
        INTO client_ids, source_kinds, interfaces, observed_values
        FROM (
            SELECT
                client_id,
                source_kind,
                interface,
                MIN(observed_at) AS observed_at
            FROM old_traffic_counter_samples
            GROUP BY client_id, source_kind, interface
        ) changed_streams;
        PERFORM refresh_traffic_counter_hourly_usage(
            client_ids, source_kinds, interfaces, observed_values, TRUE
        );
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

CREATE TRIGGER traffic_counter_hourly_usage_after_insert
AFTER INSERT ON traffic_counter_samples
REFERENCING NEW TABLE AS new_traffic_counter_samples
FOR EACH STATEMENT
EXECUTE FUNCTION refresh_traffic_counter_hourly_usage_after_insert();

CREATE TRIGGER traffic_counter_hourly_usage_after_update
AFTER UPDATE ON traffic_counter_samples
REFERENCING OLD TABLE AS old_traffic_counter_samples
            NEW TABLE AS new_traffic_counter_samples
FOR EACH STATEMENT
EXECUTE FUNCTION refresh_traffic_counter_hourly_usage_after_update();

CREATE TRIGGER traffic_counter_hourly_usage_after_delete
AFTER DELETE ON traffic_counter_samples
REFERENCING OLD TABLE AS old_traffic_counter_samples
FOR EACH STATEMENT
EXECUTE FUNCTION refresh_traffic_counter_hourly_usage_after_delete();

-- Deterministically seed the ledger before coverage is marked healthy. The
-- primary-key order supplies the raw window without changing accounting
-- semantics, including imported-to-live reset suppression.
WITH sequenced AS MATERIALIZED (
    SELECT
        sample.*,
        LAG(rx_bytes) OVER stream AS previous_rx_bytes,
        LAG(tx_bytes) OVER stream AS previous_tx_bytes,
        LAG(rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
        LAG(tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
        LAG(sample_source) OVER stream AS previous_sample_source
    FROM traffic_counter_samples sample
    WINDOW stream AS (
        PARTITION BY client_id, source_kind, interface
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
    latest_observed_at
)
SELECT
    client_id,
    source_kind,
    interface,
    date_bin(
        interval '1 hour',
        observed_at,
        TIMESTAMPTZ '1970-01-01 00:00:00+00'
    ) AS bucket_start,
    COALESCE(SUM(
        CASE
            WHEN rx_counter_epoch = previous_rx_counter_epoch
             AND rx_bytes >= previous_rx_bytes
            THEN rx_bytes - previous_rx_bytes
            ELSE 0
        END
    ), 0)::bigint,
    COALESCE(SUM(
        CASE
            WHEN tx_counter_epoch = previous_tx_counter_epoch
             AND tx_bytes >= previous_tx_bytes
            THEN tx_bytes - previous_tx_bytes
            ELSE 0
        END
    ), 0)::bigint,
    COUNT(*) FILTER (
        WHERE previous_rx_counter_epoch IS NOT NULL
          AND rx_counter_epoch <> previous_rx_counter_epoch
          AND NOT (
              previous_sample_source LIKE 'vnstat_import:%'
              AND sample_source NOT LIKE 'vnstat_import:%'
          )
    )::integer,
    COUNT(*) FILTER (
        WHERE previous_tx_counter_epoch IS NOT NULL
          AND tx_counter_epoch <> previous_tx_counter_epoch
          AND NOT (
              previous_sample_source LIKE 'vnstat_import:%'
              AND sample_source NOT LIKE 'vnstat_import:%'
          )
    )::integer,
    COUNT(*)::integer,
    MIN(observed_at),
    MAX(observed_at)
FROM sequenced
GROUP BY
    client_id,
    source_kind,
    interface,
    date_bin(
        interval '1 hour',
        observed_at,
        TIMESTAMPTZ '1970-01-01 00:00:00+00'
    );

INSERT INTO traffic_counter_hourly_usage_streams (
    client_id,
    source_kind,
    interface,
    source_revision,
    materialized_revision
)
SELECT DISTINCT client_id, source_kind, interface, 0, 0
FROM traffic_counter_samples;
