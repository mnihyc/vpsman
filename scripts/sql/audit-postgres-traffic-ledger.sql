\set ON_ERROR_STOP on
\pset format unaligned
\pset tuples_only on
\pset pager off

-- This file is a psql program consumed by
-- scripts/audit-postgres-traffic-ledger.sh. The wrapper validates every
-- variable before passing it. Keep the audit free of temporary relations and
-- data-changing statements: the restricted smoke-test role and this explicit
-- read-only transaction are independent write barriers.
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL search_path = pg_catalog, public;
SET LOCAL lock_timeout TO :'audit_lock_timeout';
SET LOCAL statement_timeout TO :'audit_statement_timeout';
SET LOCAL idle_in_transaction_session_timeout TO :'audit_idle_timeout';
\if :audit_deep
-- Deep checks contain ordered/materialized scans that may spill even though
-- this program creates no TEMP relations. Keep the spill in one backend and
-- fail at a bounded per-process ceiling instead of consuming the filesystem.
SET LOCAL max_parallel_workers_per_gather TO 0;
SET LOCAL temp_file_limit TO '256MB';
\endif

SELECT
    current_setting('server_version_num')::integer >= 160000
        AS audit_server_is_pg16,
    to_regclass('public._sqlx_migrations') IS NOT NULL
        AS audit_has_migration_ledger,
    (
        to_regclass('public.clients') IS NOT NULL
        AND to_regclass('public.jobs') IS NOT NULL
        AND to_regclass('public.job_targets') IS NOT NULL
        AND to_regclass('public.job_outputs') IS NOT NULL
        AND to_regclass('public.history_retention_policies') IS NOT NULL
        AND to_regclass('public.traffic_counter_samples') IS NOT NULL
        AND to_regclass('public.traffic_counter_rollups') IS NOT NULL
    ) AS audit_has_core_schema,
    (
        to_regclass('public.traffic_counter_hourly_usage') IS NOT NULL
        AND to_regclass('public.traffic_counter_hourly_usage_streams') IS NOT NULL
    ) AS audit_has_hourly_schema
\gset

SELECT
    'INFO',
    'audit_context',
    1::bigint,
    jsonb_build_object(
        'mode', :'audit_mode',
        'server_version_num', current_setting('server_version_num')::integer,
        'transaction_read_only', current_setting('transaction_read_only'),
        'max_parallel_workers_per_gather',
            current_setting('max_parallel_workers_per_gather')::integer,
        'temp_file_limit', current_setting('temp_file_limit'),
        'snapshot_at', statement_timestamp()
    )::text;

SELECT
    'HARD',
    'transaction_read_only',
    (current_setting('transaction_read_only') <> 'on')::integer::bigint,
    jsonb_build_object(
        'expected', 'on',
        'actual', current_setting('transaction_read_only')
    )::text;

SELECT
    'HARD',
    'deep_resource_bounds',
    CASE
        WHEN :'audit_deep'::boolean
         AND (
             current_setting('max_parallel_workers_per_gather') <> '0'
             OR pg_size_bytes(current_setting('temp_file_limit')) <> 268435456
         ) THEN 1::bigint
        ELSE 0::bigint
    END,
    jsonb_build_object(
        'applies', :'audit_deep'::boolean,
        'max_parallel_workers_per_gather',
            current_setting('max_parallel_workers_per_gather')::integer,
        'temp_file_limit', current_setting('temp_file_limit'),
        'expected_temp_file_limit_bytes', 268435456,
        'meaning', 'sort, hash, and window spill is allowed only within this per-backend bound'
    )::text;

SELECT
    'WARN',
    'page_checksums_disabled',
    (COALESCE(current_setting('data_checksums', true), 'off') <> 'on')::integer::bigint,
    jsonb_build_object(
        'data_checksums', COALESCE(current_setting('data_checksums', true), 'unknown'),
        'meaning', 'logical checks cannot prove physical page integrity'
    )::text;

WITH required(name, relation_oid) AS (
    VALUES
        ('clients', to_regclass('public.clients')),
        ('jobs', to_regclass('public.jobs')),
        ('job_targets', to_regclass('public.job_targets')),
        ('job_outputs', to_regclass('public.job_outputs')),
        ('history_retention_policies', to_regclass('public.history_retention_policies')),
        ('traffic_counter_samples', to_regclass('public.traffic_counter_samples')),
        ('traffic_counter_rollups', to_regclass('public.traffic_counter_rollups'))
), missing AS (
    SELECT name
    FROM required
    WHERE relation_oid IS NULL
)
SELECT
    'HARD',
    'required_core_schema',
    count(*)::bigint,
    jsonb_build_object(
        'missing', COALESCE(jsonb_agg(name ORDER BY name), '[]'::jsonb)
    )::text
FROM missing;

\if :audit_has_migration_ledger
SELECT
    'HARD',
    'failed_sqlx_migrations',
    count(*)::bigint,
    jsonb_build_object(
        'versions', COALESCE(jsonb_agg(version ORDER BY version), '[]'::jsonb)
    )::text
FROM public._sqlx_migrations
WHERE NOT success;

WITH expected(version) AS (
    SELECT generate_series(1::bigint, 20::bigint)
), missing AS (
    SELECT expected.version
    FROM expected
    LEFT JOIN public._sqlx_migrations migration
      ON migration.version = expected.version
     AND migration.success
    WHERE migration.version IS NULL
), unexpected AS (
    SELECT migration.version
    FROM public._sqlx_migrations migration
    WHERE migration.version < 1 OR migration.version > 20
)
SELECT
    'HARD',
    'migration_release_range',
    ((SELECT count(*) FROM missing) + (SELECT count(*) FROM unexpected))::bigint,
    jsonb_build_object(
        'expected_first_version', 1,
        'expected_last_version', 20,
        'missing_versions', COALESCE(
            (SELECT jsonb_agg(version ORDER BY version) FROM missing),
            '[]'::jsonb
        ),
        'unexpected_versions', COALESCE(
            (SELECT jsonb_agg(version ORDER BY version) FROM unexpected),
            '[]'::jsonb
        ),
        'meaning', 'use the audit shipped with the exact target release; do not infer compatibility across a missing or newer migration range'
    )::text;

SELECT
    'HARD',
    'migration_0013_checksum',
    (count(*) FILTER (
        WHERE version = 13
          AND success
          AND encode(checksum, 'hex') =
              '0b36824089415bc2e83d4455295a3691497db1ea08f790cd7ae51a6582e04fec165f9696266c105145a871331a20dff9'
    ) <> 1)::integer::bigint,
    jsonb_build_object(
        'expected_version', 13,
        'expected_sha384',
            '0b36824089415bc2e83d4455295a3691497db1ea08f790cd7ae51a6582e04fec165f9696266c105145a871331a20dff9',
        'matching_rows', count(*) FILTER (
            WHERE version = 13
              AND success
              AND encode(checksum, 'hex') =
                  '0b36824089415bc2e83d4455295a3691497db1ea08f790cd7ae51a6582e04fec165f9696266c105145a871331a20dff9'
        )
    )::text
FROM public._sqlx_migrations;

SELECT
    CASE
        WHEN count(*) FILTER (WHERE version = 15) = 0 THEN 'WARN'
        ELSE 'HARD'
    END,
    'migration_0015_checksum',
    CASE
        WHEN count(*) FILTER (WHERE version = 15) = 0 THEN 1::bigint
        WHEN count(*) FILTER (WHERE version = 15) = 1
         AND count(*) FILTER (
            WHERE version = 15
              AND success
              AND encode(checksum, 'hex') =
                  '334ba5da8a9eb62bedc1c5d968b9e37da44ca7c0d7bd53e1a8e010a80fd7411c9710446905473de4169dae6c9974e81f'
        ) = 1 THEN 0::bigint
        ELSE 1::bigint
    END,
    jsonb_build_object(
        'expected_version', 15,
        'expected_sha384',
            '334ba5da8a9eb62bedc1c5d968b9e37da44ca7c0d7bd53e1a8e010a80fd7411c9710446905473de4169dae6c9974e81f',
        'present_rows', count(*) FILTER (WHERE version = 15),
        'matching_rows', count(*) FILTER (
            WHERE version = 15
              AND success
              AND encode(checksum, 'hex') =
                  '334ba5da8a9eb62bedc1c5d968b9e37da44ca7c0d7bd53e1a8e010a80fd7411c9710446905473de4169dae6c9974e81f'
        ),
        'state', CASE
            WHEN count(*) FILTER (WHERE version = 15) = 0 THEN 'not_applied'
            WHEN count(*) FILTER (WHERE version = 15) = 1
             AND count(*) FILTER (
                WHERE version = 15
                  AND success
                  AND encode(checksum, 'hex') =
                      '334ba5da8a9eb62bedc1c5d968b9e37da44ca7c0d7bd53e1a8e010a80fd7411c9710446905473de4169dae6c9974e81f'
            ) = 1 THEN 'applied_exactly'
            ELSE 'present_but_not_exact'
        END
    )::text
FROM public._sqlx_migrations;

SELECT
    'HARD',
    'migration_0017_checksum',
    (NOT (
        count(*) FILTER (WHERE version = 17) = 1
        AND count(*) FILTER (
            WHERE version = 17
              AND description = 'agent suspension'
              AND success
              AND encode(checksum, 'hex') =
                  'b1f367301f968e59b01ae4d16161753820a867e07bde1eb992bf3a9d2fb495ebef3bd4ccc55489e51430368eb7516145'
        ) = 1
    ))::integer::bigint,
    jsonb_build_object(
        'expected_version', 17,
        'expected_description', 'agent suspension',
        'expected_sha384',
            'b1f367301f968e59b01ae4d16161753820a867e07bde1eb992bf3a9d2fb495ebef3bd4ccc55489e51430368eb7516145',
        'present_rows', count(*) FILTER (WHERE version = 17),
        'matching_rows', count(*) FILTER (
            WHERE version = 17
              AND description = 'agent suspension'
              AND success
              AND encode(checksum, 'hex') =
                  'b1f367301f968e59b01ae4d16161753820a867e07bde1eb992bf3a9d2fb495ebef3bd4ccc55489e51430368eb7516145'
        ),
        'rewrite_required', false
    )::text
FROM public._sqlx_migrations;

SELECT
    'HARD',
    'migration_0018_checksum',
    (NOT (
        count(*) FILTER (WHERE version = 18) = 1
        AND count(*) FILTER (
            WHERE version = 18
              AND description = 'traffic counter import class stream index'
              AND success
              AND encode(checksum, 'hex') =
                  'f450567e725e9bc60456a4b5c2dab87de13ca4021f98de7fe214cb8907298f46564cebaa8c60b3d85e86f8830dd8bfe8'
        ) = 1
    ))::integer::bigint,
    jsonb_build_object(
        'expected_version', 18,
        'expected_description', 'traffic counter import class stream index',
        'expected_sha384',
            'f450567e725e9bc60456a4b5c2dab87de13ca4021f98de7fe214cb8907298f46564cebaa8c60b3d85e86f8830dd8bfe8',
        'present_rows', count(*) FILTER (WHERE version = 18),
        'matching_rows', count(*) FILTER (
            WHERE version = 18
              AND description = 'traffic counter import class stream index'
              AND success
              AND encode(checksum, 'hex') =
                  'f450567e725e9bc60456a4b5c2dab87de13ca4021f98de7fe214cb8907298f46564cebaa8c60b3d85e86f8830dd8bfe8'
        ),
        'no_transaction_migration', true,
        'rewrite_required', false
    )::text
FROM public._sqlx_migrations;

SELECT
    'HARD',
    'migration_0019_checksum',
    (NOT (
        count(*) FILTER (WHERE version = 19) = 1
        AND count(*) FILTER (
            WHERE version = 19
              AND description = 'traffic import same shape update'
              AND success
              AND encode(checksum, 'hex') =
                  'aa39b2f44989f2e6337d4eea2b98065a41dc150d676655d46d1d767992d3df0c3e3ff2d7e50b589481a993fe6e691ac8'
        ) = 1
    ))::integer::bigint,
    jsonb_build_object(
        'expected_version', 19,
        'expected_description', 'traffic import same shape update',
        'expected_sha384',
            'aa39b2f44989f2e6337d4eea2b98065a41dc150d676655d46d1d767992d3df0c3e3ff2d7e50b589481a993fe6e691ac8',
        'present_rows', count(*) FILTER (WHERE version = 19),
        'matching_rows', count(*) FILTER (
            WHERE version = 19
              AND description = 'traffic import same shape update'
              AND success
              AND encode(checksum, 'hex') =
                  'aa39b2f44989f2e6337d4eea2b98065a41dc150d676655d46d1d767992d3df0c3e3ff2d7e50b589481a993fe6e691ac8'
        ),
        'transactional_migration', true,
        'rewrite_required', false
    )::text
FROM public._sqlx_migrations;

SELECT
    'HARD',
    'migration_0020_checksum',
    (NOT (
        count(*) FILTER (WHERE version = 20) = 1
        AND count(*) FILTER (
            WHERE version = 20
              AND description = 'retire unused traffic cycle usage'
              AND success
              AND encode(checksum, 'hex') =
                  '89d9b86df9fb4c8f5004a7688f22e50a20a09843b456c35742f44b403e377e73bd7cdb561b1388c25276ec97cb201f75'
        ) = 1
    ))::integer::bigint,
    jsonb_build_object(
        'expected_version', 20,
        'expected_description', 'retire unused traffic cycle usage',
        'expected_sha384',
            '89d9b86df9fb4c8f5004a7688f22e50a20a09843b456c35742f44b403e377e73bd7cdb561b1388c25276ec97cb201f75',
        'present_rows', count(*) FILTER (WHERE version = 20),
        'matching_rows', count(*) FILTER (
            WHERE version = 20
              AND description = 'retire unused traffic cycle usage'
              AND success
              AND encode(checksum, 'hex') =
                  '89d9b86df9fb4c8f5004a7688f22e50a20a09843b456c35742f44b403e377e73bd7cdb561b1388c25276ec97cb201f75'
        ),
        'transactional_migration', true,
        'rewrite_required', false
    )::text
FROM public._sqlx_migrations;

SELECT
    'HARD',
    'retired_traffic_cycle_usage_absent',
    (to_regclass('public.traffic_cycle_usage') IS NOT NULL)::integer::bigint,
    jsonb_build_object(
        'relation', to_regclass('public.traffic_cycle_usage'),
        'expected', 'absent; current accounting uses the revisioned traffic ledger'
    )::text;

SELECT (
    count(*) FILTER (WHERE version = 15) = 1
    AND count(*) FILTER (
        WHERE version = 15
          AND success
          AND encode(checksum, 'hex') =
              '334ba5da8a9eb62bedc1c5d968b9e37da44ca7c0d7bd53e1a8e010a80fd7411c9710446905473de4169dae6c9974e81f'
    ) = 1
) AS audit_migration_0015_applied_exact
FROM public._sqlx_migrations
\gset

SELECT
    'HARD',
    'migration_0016_checksum',
    (NOT (
        count(*) FILTER (WHERE version = 16) = 1
        AND count(*) FILTER (
            WHERE version = 16
              AND success
              AND encode(checksum, 'hex') =
                  '6b5644e07b7ac9bb56a0df90755d9f2b8a25598ec6846d02d798060941703c073435a593051739b0d87361af45147b1d'
        ) = 1
    ))::integer::bigint,
    jsonb_build_object(
        'expected_version', 16,
        'expected_sha384',
            '6b5644e07b7ac9bb56a0df90755d9f2b8a25598ec6846d02d798060941703c073435a593051739b0d87361af45147b1d',
        'present_rows', count(*) FILTER (WHERE version = 16),
        'matching_rows', count(*) FILTER (
            WHERE version = 16
              AND success
              AND encode(checksum, 'hex') =
                  '6b5644e07b7ac9bb56a0df90755d9f2b8a25598ec6846d02d798060941703c073435a593051739b0d87361af45147b1d'
        ),
        'rewrite_required', false
    )::text
FROM public._sqlx_migrations;
\else
SELECT
    'HARD',
    'migration_ledger_missing',
    1::bigint,
    jsonb_build_object('relation', 'public._sqlx_migrations')::text;
\set audit_migration_0015_applied_exact false
\endif

WITH expected_columns(column_name, type_oid) AS (
    VALUES
        ('suspended_at', 'timestamptz'::regtype::oid),
        ('suspended_by', 'uuid'::regtype::oid),
        ('suspended_reason', 'text'::regtype::oid),
        ('suspended_from_status', 'text'::regtype::oid)
), actual_columns AS (
    SELECT attribute.attname AS column_name,
           attribute.atttypid AS type_oid,
           attribute.attnotnull
    FROM pg_attribute attribute
    WHERE attribute.attrelid = to_regclass('public.clients')
      AND attribute.attnum > 0
      AND NOT attribute.attisdropped
), missing_or_wrong_columns AS (
    SELECT expected.column_name
    FROM expected_columns expected
    LEFT JOIN actual_columns actual USING (column_name)
    WHERE actual.column_name IS NULL
       OR actual.type_oid <> expected.type_oid
       OR actual.attnotnull
), required_constraints(constraint_name, relation_name, required_tokens) AS (
    VALUES
        ('clients_status_check', 'clients', ARRAY['suspended']::text[]),
        ('clients_suspended_reason_check', 'clients',
            ARRAY['suspended_reason', '240']::text[]),
        ('clients_suspension_state_check', 'clients',
            ARRAY['suspended_at', 'suspended_from_status', 'never',
                  'disconnected', 'offline', 'stale']::text[]),
        ('clients_suspended_by_fkey', 'clients',
            ARRAY['suspended_by', 'operators']::text[]),
        ('client_status_history_from_check', 'client_status_history',
            ARRAY['from_status', 'suspended']::text[]),
        ('client_status_history_to_check', 'client_status_history',
            ARRAY['to_status', 'suspended']::text[])
), assessed_constraints AS (
    SELECT
        required.constraint_name,
        required.relation_name,
        constraint_catalog.oid IS NOT NULL AS present,
        COALESCE(constraint_catalog.convalidated, false) AS validated,
        CASE
            WHEN constraint_catalog.oid IS NULL THEN false
            ELSE NOT EXISTS (
                SELECT 1
                FROM unnest(required.required_tokens) token
                WHERE position(token IN lower(
                    pg_get_constraintdef(constraint_catalog.oid, true)
                )) = 0
            )
        END AS tokens_exact,
        CASE WHEN constraint_catalog.oid IS NULL THEN NULL
             ELSE pg_get_constraintdef(constraint_catalog.oid, true)
        END AS definition
    FROM required_constraints required
    LEFT JOIN pg_constraint constraint_catalog
      ON constraint_catalog.conname = required.constraint_name
     AND constraint_catalog.conrelid = to_regclass(
            'public.' || required.relation_name
         )
), required_view_columns(column_name) AS (
    VALUES ('suspended_at'), ('suspended_by'),
           ('suspended_reason'), ('suspended_from_status')
), missing_view_columns AS (
    SELECT required.column_name
    FROM required_view_columns required
    LEFT JOIN information_schema.columns actual
      ON actual.table_schema = 'public'
     AND actual.table_name = 'visible_clients'
     AND actual.column_name = required.column_name
    WHERE actual.column_name IS NULL
)
SELECT
    'HARD',
    'migration_0017_suspension_catalog_contract',
    (
        (SELECT count(*) FROM missing_or_wrong_columns)
        + (SELECT count(*) FROM assessed_constraints
           WHERE NOT present OR NOT validated OR NOT tokens_exact)
        + (SELECT count(*) FROM missing_view_columns)
        + CASE WHEN COALESCE((
              SELECT relation.relkind = 'v'
              FROM pg_class relation
              JOIN pg_namespace namespace
                ON namespace.oid = relation.relnamespace
              WHERE namespace.nspname = 'public'
                AND relation.relname = 'visible_clients'
          ), false) THEN 0 ELSE 1 END
    )::bigint,
    jsonb_build_object(
        'expected_columns', ARRAY[
            'suspended_at:timestamptz', 'suspended_by:uuid',
            'suspended_reason:text', 'suspended_from_status:text'
        ],
        'missing_or_wrong_columns', COALESCE(
            (SELECT jsonb_agg(column_name ORDER BY column_name)
             FROM missing_or_wrong_columns),
            '[]'::jsonb
        ),
        'constraints', COALESCE(
            (SELECT jsonb_agg(jsonb_build_object(
                'name', constraint_name,
                'relation', relation_name,
                'present', present,
                'validated', validated,
                'required_tokens_present', tokens_exact,
                'definition', definition
            ) ORDER BY relation_name, constraint_name)
            FROM assessed_constraints),
            '[]'::jsonb
        ),
        'missing_visible_clients_columns', COALESCE(
            (SELECT jsonb_agg(column_name ORDER BY column_name)
             FROM missing_view_columns),
            '[]'::jsonb
        ),
        'rewrite_required', false
    )::text;

WITH function_catalog AS (
    SELECT
        procedure_catalog.oid AS function_oid,
        function_language.lanname AS language_name,
        procedure_catalog.prokind,
        procedure_catalog.prorettype,
        procedure_catalog.proretset,
        procedure_catalog.pronargs,
        procedure_catalog.pronargdefaults,
        procedure_catalog.proargnames,
        pg_get_expr(procedure_catalog.proargdefaults, 0)
            AS argument_defaults,
        procedure_catalog.provolatile,
        procedure_catalog.proparallel,
        procedure_catalog.prosecdef,
        procedure_catalog.proleakproof,
        procedure_catalog.proisstrict,
        encode(
            sha256(convert_to(procedure_catalog.prosrc, 'UTF8')),
            'hex'
        ) AS source_sha256
    FROM pg_proc procedure_catalog
    JOIN pg_language function_language
      ON function_language.oid = procedure_catalog.prolang
    WHERE procedure_catalog.oid = to_regprocedure(
        'public.refresh_traffic_counter_hourly_usage(text[],text[],text[],timestamptz[],boolean)'
    )
), assessed AS (
    SELECT
        function_catalog.*,
        (
            language_name = 'plpgsql'
            AND prokind = 'f'
            AND prorettype = 'pg_catalog.void'::regtype
            AND NOT proretset
            AND pronargs = 5
            AND pronargdefaults = 1
            AND argument_defaults = 'false'
            AND proargnames = ARRAY[
                'changed_client_ids',
                'changed_source_kinds',
                'changed_interfaces',
                'changed_observed_at',
                'rebuild_entire_streams'
            ]::text[]
            AND provolatile = 'v'
            AND proparallel = 'u'
            AND NOT prosecdef
            AND NOT proleakproof
            AND NOT proisstrict
            AND source_sha256 =
                'd88de80aa8c8788af1d44007201b9a618927a204fb1edd688231cabcc95fbbc9'
        ) AS contract_exact
    FROM function_catalog
)
SELECT
    'HARD',
    'migration_0016_streaming_function_contract',
    (NOT COALESCE(bool_or(contract_exact), false))::integer::bigint,
    jsonb_build_object(
        'expected_signature',
            'public.refresh_traffic_counter_hourly_usage(text[],text[],text[],timestamptz[],boolean)',
        'expected_source_sha256',
            'd88de80aa8c8788af1d44007201b9a618927a204fb1edd688231cabcc95fbbc9',
        'available', count(*) = 1,
        'matching_contract_rows', count(*) FILTER (WHERE contract_exact),
        'actual_argument_defaults', max(argument_defaults),
        'actual_source_sha256', max(source_sha256),
        'purpose', 'narrow per-stream whole-ledger refresh without a retained-row rewrite',
        'rewrite_required', false
    )::text
FROM assessed;

WITH named_index AS (
    SELECT
        index_relation.oid AS index_oid,
        indexed_relation.oid AS table_oid,
        index_access_method.amname AS access_method,
        index_catalog.indisvalid,
        index_catalog.indisready,
        index_catalog.indislive,
        index_catalog.indisunique,
        index_catalog.indisprimary,
        index_catalog.indisexclusion,
        index_catalog.indnkeyatts,
        index_catalog.indnatts,
        index_catalog.indpred IS NULL AS has_no_predicate,
        index_catalog.indexprs IS NULL AS has_no_expressions,
        ARRAY(
            SELECT indexed_attribute.attname::text
            FROM unnest(index_catalog.indkey) WITH ORDINALITY
                index_key(attribute_number, position)
            LEFT JOIN pg_attribute indexed_attribute
              ON indexed_attribute.attrelid = index_catalog.indrelid
             AND indexed_attribute.attnum = index_key.attribute_number
            ORDER BY index_key.position
        ) AS indexed_columns,
        ARRAY(
            SELECT index_option.option_value
            FROM unnest(index_catalog.indoption) WITH ORDINALITY
                index_option(option_value, position)
            ORDER BY index_option.position
        ) AS index_options,
        regexp_replace(
            pg_get_indexdef(index_relation.oid),
            '[[:space:]]+',
            ' ',
            'g'
        ) AS actual_definition
    FROM pg_class index_relation
    JOIN pg_namespace index_namespace
      ON index_namespace.oid = index_relation.relnamespace
    JOIN pg_index index_catalog
      ON index_catalog.indexrelid = index_relation.oid
    JOIN pg_class indexed_relation
      ON indexed_relation.oid = index_catalog.indrelid
    JOIN pg_am index_access_method
      ON index_access_method.oid = index_relation.relam
    WHERE index_namespace.nspname = 'public'
      AND index_relation.relname =
          'telemetry_network_rates_client_effective_idx'
), assessed AS (
    SELECT
        named_index.*,
        (
            table_oid = to_regclass('public.telemetry_network_rates')
            AND access_method = 'btree'
            AND indisvalid
            AND indisready
            AND indislive
            AND NOT indisunique
            AND NOT indisprimary
            AND NOT indisexclusion
            AND indnkeyatts = 4
            AND indnatts = 5
            AND has_no_predicate
            AND has_no_expressions
            AND indexed_columns = ARRAY[
                'client_id',
                'interface',
                'latest_observed_at',
                'bucket_start',
                'bucket_secs'
            ]::text[]
            -- btree indoption: ASC NULLS LAST = 0; DESC NULLS FIRST = 3.
            AND index_options = ARRAY[0, 0, 3, 3]::smallint[]
            AND actual_definition =
                'CREATE INDEX telemetry_network_rates_client_effective_idx ON public.telemetry_network_rates USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC) INCLUDE (bucket_secs)'
        ) AS contract_exact
    FROM named_index
)
SELECT
    CASE
        WHEN :'audit_migration_0015_applied_exact'::boolean THEN 'HARD'
        ELSE 'WARN'
    END,
    'migration_0015_index_contract',
    (NOT COALESCE(bool_or(contract_exact), false))::integer::bigint,
    jsonb_build_object(
        'migration_applied_exactly',
            :'audit_migration_0015_applied_exact'::boolean,
        'expected_index', 'telemetry_network_rates_client_effective_idx',
        'expected_definition',
            'CREATE INDEX telemetry_network_rates_client_effective_idx ON public.telemetry_network_rates USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC) INCLUDE (bucket_secs)',
        'matching_contract_rows', count(*) FILTER (WHERE contract_exact),
        'actual', COALESCE(
            jsonb_agg(jsonb_build_object(
                'valid', indisvalid,
                'ready', indisready,
                'live', indislive,
                'access_method', access_method,
                'key_attributes', indnkeyatts,
                'total_attributes', indnatts,
                'columns', indexed_columns,
                'options', index_options,
                'definition', actual_definition
            )) FILTER (WHERE index_oid IS NOT NULL),
            '[]'::jsonb
        ),
        'rewrite_required', false
    )::text
FROM assessed;

WITH named_relation AS (
    SELECT
        relation.oid AS relation_oid,
        relation.relkind::text AS relkind,
        indexed_relation.oid AS table_oid,
        access_method.amname AS access_method,
        COALESCE(index_catalog.indisvalid, false) AS indisvalid,
        COALESCE(index_catalog.indisready, false) AS indisready,
        COALESCE(index_catalog.indislive, false) AS indislive,
        COALESCE(index_catalog.indisunique, false) AS indisunique,
        COALESCE(index_catalog.indisprimary, false) AS indisprimary,
        COALESCE(index_catalog.indisexclusion, false) AS indisexclusion,
        CASE WHEN relation.relkind = 'i'
             THEN pg_get_indexdef(relation.oid)
             ELSE NULL
        END AS actual_definition
    FROM pg_class relation
    JOIN pg_namespace namespace
      ON namespace.oid = relation.relnamespace
    LEFT JOIN pg_index index_catalog
      ON index_catalog.indexrelid = relation.oid
    LEFT JOIN pg_class indexed_relation
      ON indexed_relation.oid = index_catalog.indrelid
    LEFT JOIN pg_am access_method
      ON access_method.oid = relation.relam
    WHERE namespace.nspname = 'public'
      AND relation.relname =
          'traffic_counter_samples_import_class_stream_idx'
), assessed AS (
    SELECT
        named_relation.*,
        (
            relkind = 'i'
            AND table_oid = to_regclass('public.traffic_counter_samples')
            AND access_method = 'btree'
            AND indisvalid
            AND indisready
            AND indislive
            AND NOT indisunique
            AND NOT indisprimary
            AND NOT indisexclusion
            AND actual_definition =
                'CREATE INDEX traffic_counter_samples_import_class_stream_idx ON public.traffic_counter_samples USING btree (client_id, source_kind, interface, ((sample_source ~~ ''vnstat_import:%''::text)), observed_at)'
        ) AS contract_exact,
        (
            relkind = 'i'
            AND table_oid = to_regclass('public.traffic_counter_samples')
            AND access_method = 'btree'
            AND actual_definition =
                'CREATE INDEX traffic_counter_samples_import_class_stream_idx ON public.traffic_counter_samples USING btree (client_id, source_kind, interface, ((sample_source ~~ ''vnstat_import:%''::text)), observed_at)'
        ) AS definition_exact
    FROM named_relation
)
SELECT
    'HARD',
    'migration_0018_import_class_index_contract',
    (NOT COALESCE(bool_or(contract_exact), false))::integer::bigint,
    jsonb_build_object(
        'expected_index',
            'public.traffic_counter_samples_import_class_stream_idx',
        'expected_definition',
            'CREATE INDEX traffic_counter_samples_import_class_stream_idx ON public.traffic_counter_samples USING btree (client_id, source_kind, interface, ((sample_source ~~ ''vnstat_import:%''::text)), observed_at)',
        'matching_contract_rows', count(*) FILTER (WHERE contract_exact),
        'catalog_state', CASE
            WHEN count(*) = 0 THEN
                'missing_recoverable_by_current_startup'
            WHEN bool_or(contract_exact) THEN 'usable'
            WHEN bool_or(definition_exact) THEN
                'exact_invalid_recoverable_by_current_startup'
            ELSE 'wrong_same_name_operator_action_required'
        END,
        'actual', COALESCE(
            jsonb_agg(jsonb_build_object(
                'relkind', relkind,
                'table', table_oid::regclass::text,
                'access_method', access_method,
                'valid', indisvalid,
                'ready', indisready,
                'live', indislive,
                'unique', indisunique,
                'primary', indisprimary,
                'exclusion', indisexclusion,
                'definition', actual_definition
            )) FILTER (WHERE relation_oid IS NOT NULL),
            '[]'::jsonb
        ),
        'recovery',
            'restart one current API or worker binary; it repairs only a missing or exact invalid migration-owned index under the startup advisory lock and fails closed for a wrong same-name object',
        'rewrite_required', false
    )::text
FROM assessed;

WITH function_catalog AS (
    SELECT
        procedure_catalog.oid AS function_oid,
        function_language.lanname AS language_name,
        procedure_catalog.prokind,
        procedure_catalog.prorettype,
        procedure_catalog.proretset,
        procedure_catalog.pronargs,
        procedure_catalog.pronargdefaults,
        procedure_catalog.proargnames,
        procedure_catalog.provolatile,
        procedure_catalog.proparallel,
        procedure_catalog.prosecdef,
        procedure_catalog.proleakproof,
        procedure_catalog.proisstrict,
        encode(
            sha256(convert_to(procedure_catalog.prosrc, 'UTF8')),
            'hex'
        ) AS source_sha256
    FROM pg_proc procedure_catalog
    JOIN pg_language function_language
      ON function_language.oid = procedure_catalog.prolang
    WHERE procedure_catalog.oid = to_regprocedure(
        'public.refresh_traffic_counter_hourly_usage_after_update()'
    )
), assessed_function AS (
    SELECT
        function_catalog.*,
        (
            language_name = 'plpgsql'
            AND prokind = 'f'
            AND prorettype = 'pg_catalog.trigger'::regtype
            AND NOT proretset
            AND pronargs = 0
            AND pronargdefaults = 0
            AND proargnames IS NULL
            AND provolatile = 'v'
            AND proparallel = 'u'
            AND NOT prosecdef
            AND NOT proleakproof
            AND NOT proisstrict
            AND source_sha256 =
                'f739a35af6a49d85770b9c06de11e12c2fbe2d68320c86ca9e6d953ffe6fcab5'
        ) AS contract_exact
    FROM function_catalog
), named_trigger AS (
    SELECT
        trigger_catalog.oid AS trigger_oid,
        trigger_catalog.tgrelid AS table_oid,
        trigger_catalog.tgfoid AS function_oid,
        trigger_catalog.tgenabled,
        trigger_catalog.tgisinternal,
        trigger_catalog.tgtype,
        trigger_catalog.tgoldtable,
        trigger_catalog.tgnewtable,
        trigger_catalog.tgnargs,
        trigger_catalog.tgqual IS NULL AS has_no_when_clause,
        pg_get_triggerdef(trigger_catalog.oid) AS actual_definition
    FROM pg_trigger trigger_catalog
    WHERE trigger_catalog.tgrelid =
            to_regclass('public.traffic_counter_samples')
      AND trigger_catalog.tgname =
            'traffic_counter_hourly_usage_after_update'
), assessed_trigger AS (
    SELECT
        named_trigger.*,
        (
            table_oid = to_regclass('public.traffic_counter_samples')
            AND function_oid = to_regprocedure(
                'public.refresh_traffic_counter_hourly_usage_after_update()'
            )
            AND tgenabled = 'O'
            AND NOT tgisinternal
            -- tgtype 16 is an AFTER UPDATE, FOR EACH STATEMENT trigger.
            AND tgtype = 16
            AND tgoldtable = 'old_traffic_counter_samples'
            AND tgnewtable = 'new_traffic_counter_samples'
            AND tgnargs = 0
            AND has_no_when_clause
        ) AS contract_exact
    FROM named_trigger
)
SELECT
    'HARD',
    'migration_0019_import_update_trigger_contract',
    (
        CASE WHEN (SELECT count(*) FILTER (WHERE contract_exact)
                   FROM assessed_function) = 1
             THEN 0 ELSE 1 END
        + CASE WHEN (SELECT count(*) FILTER (WHERE contract_exact)
                     FROM assessed_trigger) = 1
               THEN 0 ELSE 1 END
    )::bigint,
    jsonb_build_object(
        'expected_function',
            'public.refresh_traffic_counter_hourly_usage_after_update()',
        'expected_source_sha256',
            'f739a35af6a49d85770b9c06de11e12c2fbe2d68320c86ca9e6d953ffe6fcab5',
        'function_available', (SELECT count(*) = 1 FROM assessed_function),
        'matching_function_rows',
            (SELECT count(*) FILTER (WHERE contract_exact)
             FROM assessed_function),
        'actual_source_sha256',
            (SELECT max(source_sha256) FROM assessed_function),
        'expected_trigger',
            'public.traffic_counter_samples.traffic_counter_hourly_usage_after_update',
        'trigger_available', (SELECT count(*) = 1 FROM assessed_trigger),
        'matching_trigger_rows',
            (SELECT count(*) FILTER (WHERE contract_exact)
             FROM assessed_trigger),
        'actual_trigger', COALESCE(
            (SELECT jsonb_agg(jsonb_build_object(
                'enabled', tgenabled,
                'internal', tgisinternal,
                'tgtype', tgtype,
                'old_transition_table', tgoldtable,
                'new_transition_table', tgnewtable,
                'argument_count', tgnargs,
                'has_no_when_clause', has_no_when_clause,
                'definition', actual_definition
            ) ORDER BY trigger_oid)
            FROM assessed_trigger),
            '[]'::jsonb
        ),
        'rewrite_required', false
    )::text;

SELECT
    'WARN',
    'long_running_client_transactions',
    count(*)::bigint,
    jsonb_build_object(
        'threshold_seconds', 300,
        'max_age_seconds', COALESCE(
            max(extract(epoch FROM clock_timestamp() - xact_start))::bigint,
            0
        )
    )::text
FROM pg_stat_activity
WHERE pid <> pg_backend_pid()
  AND backend_type = 'client backend'
  AND xact_start IS NOT NULL
  AND xact_start < clock_timestamp() - interval '5 minutes';

\if :audit_has_core_schema
SELECT
    'INFO',
    'traffic_relation_sizes',
    count(*)::bigint,
    jsonb_build_object(
        'database_bytes', pg_database_size(current_database()),
        'relations', COALESCE(
            jsonb_agg(
                jsonb_build_object(
                    'relation', stats.relname,
                    'heap_bytes', pg_relation_size(stats.relid),
                    'index_bytes', pg_indexes_size(stats.relid),
                    'total_bytes', pg_total_relation_size(stats.relid),
                    'estimated_live_rows', stats.n_live_tup,
                    'estimated_dead_rows', stats.n_dead_tup,
                    'last_autovacuum', stats.last_autovacuum,
                    'last_autoanalyze', stats.last_autoanalyze
                ) ORDER BY stats.relname
            ),
            '[]'::jsonb
        )
    )::text
FROM pg_stat_user_tables stats
WHERE stats.schemaname = 'public'
  AND stats.relname IN (
      'traffic_counter_samples',
      'traffic_counter_rollups',
      'traffic_counter_hourly_usage',
      'traffic_counter_hourly_usage_streams'
  );

WITH bloated AS (
    SELECT
        relname,
        n_live_tup,
        n_dead_tup,
        CASE
            WHEN n_live_tup + n_dead_tup = 0 THEN 0
            ELSE round(100.0 * n_dead_tup / (n_live_tup + n_dead_tup), 2)
        END AS dead_percent
    FROM pg_stat_user_tables
    WHERE schemaname = 'public'
      AND relname IN (
          'traffic_counter_samples',
          'traffic_counter_rollups',
          'traffic_counter_hourly_usage'
      )
      AND n_dead_tup >= 100000
      AND n_dead_tup * 5 > GREATEST(n_live_tup + n_dead_tup, 1)
)
SELECT
    'WARN',
    'traffic_relation_bloat_estimate',
    count(*)::bigint,
    jsonb_build_object(
        'threshold', 'at least 100000 dead tuples and over 20 percent estimated dead',
        'relations', COALESCE(
            jsonb_agg(
                jsonb_build_object(
                    'relation', relname,
                    'estimated_live_rows', n_live_tup,
                    'estimated_dead_rows', n_dead_tup,
                    'estimated_dead_percent', dead_percent
                ) ORDER BY relname
            ),
            '[]'::jsonb
        )
    )::text
FROM bloated;

SELECT
    'INFO',
    'traffic_row_counts',
    1::bigint,
    jsonb_build_object(
        'raw_rows', (SELECT count(*) FROM public.traffic_counter_samples),
        'raw_streams', (
            SELECT count(*)
            FROM (
                SELECT DISTINCT client_id, source_kind, interface
                FROM public.traffic_counter_samples
            ) streams
        ),
        'rollup_rows', (SELECT count(*) FROM public.traffic_counter_rollups),
        'import_raw_rows', (
            SELECT count(*)
            FROM public.traffic_counter_samples
            WHERE sample_source LIKE 'vnstat_import:%'
        ),
        'import_rollup_rows', (
            SELECT count(*)
            FROM public.traffic_counter_rollups
            WHERE origin_kind = 'vnstat_import'
        )
    )::text;

WITH cutoff AS (
    SELECT
        (date_trunc('day', current_timestamp AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
            - interval '32 days' AS raw_cutoff
), old_rows AS (
    SELECT sample.client_id, sample.source_kind, sample.interface
    FROM public.traffic_counter_samples sample
    CROSS JOIN cutoff
    WHERE sample.observed_at < cutoff.raw_cutoff
      AND NOT sample.inbound_promoted
), examples AS (
    SELECT DISTINCT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias
    FROM old_rows
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'WARN',
    'raw_retention_backlog',
    (SELECT count(*) FROM old_rows)::bigint,
    jsonb_build_object(
        'cutoff', (SELECT raw_cutoff FROM cutoff),
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(stream_alias ORDER BY stream_alias) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH stream_state AS (
    SELECT
        client_id,
        source_kind,
        interface,
        min(observed_at) AS first_observed_at,
        min(observed_at) FILTER (WHERE inbound_promoted) AS promoted_at,
        count(*) FILTER (WHERE inbound_promoted) AS promoted_count
    FROM public.traffic_counter_samples
    GROUP BY client_id, source_kind, interface
), multiple AS (
    SELECT * FROM stream_state WHERE promoted_count > 1
), misplaced AS (
    SELECT *
    FROM stream_state
    WHERE promoted_count = 1
      AND promoted_at IS DISTINCT FROM first_observed_at
), multiple_examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias
    FROM multiple
    ORDER BY stream_alias
    LIMIT 10
), misplaced_examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias
    FROM misplaced
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'raw_promoted_boundary_count',
    (SELECT count(*) FROM multiple)::bigint,
    jsonb_build_object(
        'expected_maximum_per_stream', 1,
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(stream_alias ORDER BY stream_alias) FROM multiple_examples),
            '[]'::jsonb
        )
    )::text
UNION ALL
SELECT
    'HARD',
    'raw_promoted_boundary_position',
    (SELECT count(*) FROM misplaced)::bigint,
    jsonb_build_object(
        'expected', 'the sole promoted boundary is the earliest retained raw row',
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(stream_alias ORDER BY stream_alias) FROM misplaced_examples),
            '[]'::jsonb
        )
    )::text;

WITH bounds AS (
    SELECT
        (date_trunc('day', current_timestamp AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
            - interval '32 days' AS raw_cutoff,
        date_trunc('minute', current_timestamp) AS current_minute
), oversized AS (
    SELECT sample.client_id, sample.source_kind, sample.interface, count(*) AS row_count
    FROM public.traffic_counter_samples sample
    CROSS JOIN bounds
    GROUP BY sample.client_id, sample.source_kind, sample.interface,
             bounds.raw_cutoff, bounds.current_minute
    HAVING count(*) >
        floor(extract(epoch FROM (bounds.current_minute - bounds.raw_cutoff)) / 60)::bigint + 2
), examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias,
        row_count
    FROM oversized
    ORDER BY row_count DESC, stream_alias
    LIMIT 10
)
SELECT
    'WARN',
    'raw_stream_row_bound',
    (SELECT count(*) FROM oversized)::bigint,
    jsonb_build_object(
        'expected', 'no more than the 32-day minute tail plus one predecessor',
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(
                jsonb_build_object('stream', stream_alias, 'rows', row_count)
                ORDER BY row_count DESC, stream_alias
            ) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH malformed AS (
    SELECT client_id, source_kind, interface
    FROM public.traffic_counter_samples
    WHERE sample_source LIKE 'vnstat_import:%'
      AND sample_source !~
          '^vnstat_import:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
), examples AS (
    SELECT DISTINCT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias
    FROM malformed
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'import_sample_source_format',
    (SELECT count(*) FROM malformed)::bigint,
    jsonb_build_object(
        'expected', 'vnstat_import:<job UUID>',
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(stream_alias ORDER BY stream_alias) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH imported AS (
    SELECT
        sample.client_id,
        sample.source_kind,
        sample.interface,
        substring(sample.sample_source FROM '^vnstat_import:(.*)$')::uuid AS job_id
    FROM public.traffic_counter_samples sample
    WHERE sample.sample_source ~
        '^vnstat_import:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$'
), bad AS (
    SELECT imported.*
    FROM imported
    LEFT JOIN public.jobs job ON job.id = imported.job_id
    LEFT JOIN public.job_targets target
      ON target.job_id = imported.job_id
     AND target.client_id = imported.client_id
    WHERE job.id IS NULL
       OR job.command_type <> 'network_traffic_import_vnstat'
       OR target.job_id IS NULL
       OR target.status NOT IN ('dispatching', 'running', 'completed')
), examples AS (
    SELECT DISTINCT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias
    FROM bad
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'import_sample_job_lineage',
    (SELECT count(*) FROM bad)::bigint,
    jsonb_build_object(
        'expected', 'each imported raw row maps to the same client on an applied or retry-pending vnStat job target',
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(stream_alias ORDER BY stream_alias) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH mixed AS (
    SELECT client_id, source_kind, interface, count(DISTINCT sample_source) AS import_jobs
    FROM public.traffic_counter_samples
    WHERE sample_source LIKE 'vnstat_import:%'
    GROUP BY client_id, source_kind, interface
    HAVING count(DISTINCT sample_source) > 1
), examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias,
        import_jobs
    FROM mixed
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'multiple_import_jobs_per_raw_stream',
    (SELECT count(*) FROM mixed)::bigint,
    jsonb_build_object(
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(
                jsonb_build_object('stream', stream_alias, 'job_count', import_jobs)
                ORDER BY stream_alias
            ) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH import_targets AS (
    SELECT
        job.id AS job_id,
        job.created_at,
        job.max_timeout_secs,
        target.client_id,
        target.status,
        target.message,
        target.completed_at
    FROM public.jobs job
    JOIN public.job_targets target ON target.job_id = job.id
    WHERE job.command_type = 'network_traffic_import_vnstat'
), active AS (
    SELECT * FROM import_targets WHERE completed_at IS NULL
), stalled AS (
    SELECT active.*
    FROM active
    WHERE created_at < clock_timestamp()
            - make_interval(secs => GREATEST(max_timeout_secs, 60)::integer)
            - interval '10 minutes'
       OR EXISTS (
            SELECT 1
            FROM public.job_outputs final_output
            WHERE final_output.job_id = active.job_id
              AND final_output.client_id = active.client_id
              AND final_output.done
       )
), failed AS (
    SELECT *
    FROM import_targets
    WHERE completed_at IS NOT NULL
      AND status <> 'completed'
), missing_outputs AS (
    SELECT target.*
    FROM import_targets target
    WHERE target.status = 'completed'
      AND NOT EXISTS (
          SELECT 1
          FROM public.job_outputs output
          WHERE output.job_id = target.job_id
            AND output.client_id = target.client_id
      )
), partial_without_final AS (
    SELECT target.*
    FROM import_targets target
    WHERE target.status = 'completed'
      AND EXISTS (
          SELECT 1
          FROM public.job_outputs output
          WHERE output.job_id = target.job_id
            AND output.client_id = target.client_id
      )
      AND NOT EXISTS (
          SELECT 1
          FROM public.job_outputs output
          WHERE output.job_id = target.job_id
            AND output.client_id = target.client_id
            AND output.done
      )
), bad_summary AS (
    SELECT target.*
    FROM import_targets target
    WHERE target.status = 'completed'
      AND COALESCE(target.message, '') !~
          '[0-9]+ RX bytes, [0-9]+ TX bytes'
      AND EXISTS (
          SELECT 1
          FROM public.job_outputs output
          WHERE output.job_id = target.job_id
            AND output.client_id = target.client_id
            AND output.done
      )
)
SELECT
    'INFO',
    'active_vnstat_import_targets',
    (SELECT count(*) FROM active)::bigint,
    jsonb_build_object('meaning', 'do not dispatch a duplicate while a durable target is active')::text
UNION ALL
SELECT
    'WARN',
    'stalled_or_finalizer_pending_vnstat_import_targets',
    (SELECT count(*) FROM stalled)::bigint,
    jsonb_build_object('meaning', 'deploy the fixed finalizer and let it retry; do not delete traffic rows')::text
UNION ALL
SELECT
    'WARN',
    'failed_vnstat_import_targets',
    (SELECT count(*) FROM failed)::bigint,
    jsonb_build_object(
        'atomicity', 'a failed attempt does not expose a partial per-client import',
        'invalid_contract_failures', (
            SELECT count(*) FROM failed
            WHERE message LIKE 'network_traffic_import_invalid:%'
        )
    )::text
UNION ALL
SELECT
    'WARN',
    'completed_import_outputs_unavailable',
    (SELECT count(*) FROM missing_outputs)::bigint,
    jsonb_build_object(
        'meaning', 'logical traffic may be healthy, but exact completed-output replay is no longer available'
    )::text
UNION ALL
SELECT
    'HARD',
    'completed_import_final_output_missing',
    (SELECT count(*) FROM partial_without_final)::bigint,
    jsonb_build_object(
        'expected', 'a completed target with retained chunks has exactly one done output'
    )::text
UNION ALL
SELECT
    'HARD',
    'completed_import_summary_contract',
    (SELECT count(*) FROM bad_summary)::bigint,
    jsonb_build_object(
        'expected', 'the committed server summary retains parseable RX and TX totals for conservation'
    )::text;

WITH import_outputs AS (
    SELECT
        job.id AS job_id,
        target.client_id,
        target.completed_at,
        output.seq,
        output.stream,
        output.data,
        output.storage,
        output.object_key,
        output.data_sha256_hex,
        output.data_size_bytes,
        output.exit_code,
        output.done
    FROM public.jobs job
    JOIN public.job_targets target ON target.job_id = job.id
    JOIN public.job_outputs output
      ON output.job_id = target.job_id
     AND output.client_id = target.client_id
    WHERE job.command_type = 'network_traffic_import_vnstat'
), metadata_bad AS (
    SELECT *
    FROM import_outputs
    WHERE storage <> 'inline'
       OR object_key IS NOT NULL
       OR data_size_bytes IS DISTINCT FROM octet_length(data)::bigint
       OR data_sha256_hex IS NULL
       OR data_sha256_hex !~ '^[0-9a-f]{64}$'
       OR data_sha256_hex IS DISTINCT FROM encode(sha256(data), 'hex')
), sequence_state AS (
    SELECT
        job_id,
        client_id,
        count(*) AS output_count,
        min(seq) AS minimum_seq,
        max(seq) AS maximum_seq,
        count(*) FILTER (WHERE done) AS final_count,
        min(seq) FILTER (WHERE done) AS final_seq,
        bool_or(stream <> 'status') AS wrong_stream
    FROM import_outputs
    GROUP BY job_id, client_id
), sequence_bad AS (
    SELECT *
    FROM sequence_state
    WHERE final_count > 0
      AND (
          final_count <> 1
          OR minimum_seq <> 0
          OR final_seq <> maximum_seq
          OR output_count <> maximum_seq::bigint + 1
          OR wrong_stream
      )
)
SELECT
    'HARD',
    'import_output_storage_integrity',
    (SELECT count(*) FROM metadata_bad)::bigint,
    jsonb_build_object(
        'expected', 'status outputs remain inline with matching size and SHA-256 metadata'
    )::text
UNION ALL
SELECT
    'HARD',
    'import_output_sequence_integrity',
    (SELECT count(*) FROM sequence_bad)::bigint,
    jsonb_build_object(
        'expected', 'one contiguous status sequence 0..final_seq with exactly one final row'
    )::text;

WITH finer_overlap AS (
    SELECT coarse.client_id, coarse.source_kind, coarse.interface
    FROM public.traffic_counter_rollups coarse
    WHERE EXISTS (
        SELECT 1
        FROM public.traffic_counter_rollups finer
        WHERE finer.client_id = coarse.client_id
          AND finer.source_kind = coarse.source_kind
          AND finer.interface = coarse.interface
          AND finer.origin_kind = coarse.origin_kind
          AND finer.bucket_secs < coarse.bucket_secs
          AND finer.bucket_start
                < coarse.bucket_start + make_interval(secs => coarse.bucket_secs)
          AND finer.bucket_start + make_interval(secs => finer.bucket_secs)
                > coarse.bucket_start
        OFFSET 0
    )
), exact_overlap AS (
    SELECT rollup.client_id, rollup.source_kind, rollup.interface
    FROM public.traffic_counter_rollups rollup
    WHERE EXISTS (
        SELECT 1
        FROM public.traffic_counter_samples exact
        WHERE exact.client_id = rollup.client_id
          AND exact.source_kind = rollup.source_kind
          AND exact.interface = rollup.interface
          AND NOT exact.inbound_promoted
          AND (CASE WHEN exact.sample_source LIKE 'vnstat_import:%'
                    THEN 'vnstat_import' ELSE 'live' END) = rollup.origin_kind
          AND exact.observed_at >= rollup.bucket_start
          AND exact.observed_at
                < rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
        OFFSET 0
    )
), cross_origin AS (
    SELECT live.client_id, live.source_kind, live.interface
    FROM public.traffic_counter_rollups live
    WHERE live.origin_kind = 'live'
      AND EXISTS (
          SELECT 1
          FROM public.traffic_counter_rollups imported
          WHERE imported.client_id = live.client_id
            AND imported.source_kind = live.source_kind
            AND imported.interface = live.interface
            AND imported.origin_kind = 'vnstat_import'
            AND imported.first_observed_at <= live.latest_observed_at
            AND imported.latest_observed_at >= live.first_observed_at
          OFFSET 0
      )
)
SELECT
    'WARN',
    'rollup_finer_overlap',
    (SELECT count(*) FROM finer_overlap)::bigint,
    jsonb_build_object(
        'meaning', 'readers de-overlap finer tiers; inspect retention conflict logs before any repair'
    )::text
UNION ALL
SELECT
    'WARN',
    'rollup_exact_overlap',
    (SELECT count(*) FROM exact_overlap)::bigint,
    jsonb_build_object(
        'meaning', 'readers suppress same-origin rollups where exact non-promoted rows overlap'
    )::text
UNION ALL
SELECT
    'HARD',
    'rollup_cross_origin_observation_overlap',
    (SELECT count(*) FROM cross_origin)::bigint,
    jsonb_build_object(
        'expected', 'live and vnStat-import observation ranges do not overlap within one stream'
    )::text;

WITH policy AS (
    SELECT
        COALESCE(
            (SELECT enabled FROM public.history_retention_policies
             WHERE domain = 'traffic_counter_samples'),
            true
        ) AS enabled,
        COALESCE(
            (SELECT retention_days FROM public.history_retention_policies
             WHERE domain = 'traffic_counter_samples'),
            3650
        ) AS retention_days
), backlog AS (
    SELECT rollup.client_id, rollup.source_kind, rollup.interface
    FROM public.traffic_counter_rollups rollup
    CROSS JOIN policy
    WHERE policy.enabled
      AND rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
            < (date_trunc('day', current_timestamp AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                - make_interval(days => policy.retention_days)
)
SELECT
    'WARN',
    'rollup_retention_backlog',
    count(*)::bigint,
    jsonb_build_object('meaning', 'ordinary bounded retention has not yet pruned these terminal buckets')::text
FROM backlog;

\if :audit_has_hourly_schema
WITH components(name, present) AS (
    VALUES
        (
            'refresh_function',
            to_regprocedure(
                'public.refresh_traffic_counter_hourly_usage(text[],text[],text[],timestamp with time zone[],boolean)'
            ) IS NOT NULL
        ),
        (
            'insert_trigger',
            EXISTS (
                SELECT 1 FROM pg_trigger
                WHERE tgrelid = 'public.traffic_counter_samples'::regclass
                  AND tgname = 'traffic_counter_hourly_usage_after_insert'
                  AND tgenabled <> 'D'
            )
        ),
        (
            'update_trigger',
            EXISTS (
                SELECT 1 FROM pg_trigger
                WHERE tgrelid = 'public.traffic_counter_samples'::regclass
                  AND tgname = 'traffic_counter_hourly_usage_after_update'
                  AND tgenabled <> 'D'
            )
        ),
        (
            'delete_trigger',
            EXISTS (
                SELECT 1 FROM pg_trigger
                WHERE tgrelid = 'public.traffic_counter_samples'::regclass
                  AND tgname = 'traffic_counter_hourly_usage_after_delete'
                  AND tgenabled <> 'D'
            )
        )
), missing AS (
    SELECT name FROM components WHERE NOT present
)
SELECT
    'HARD',
    'hourly_ledger_maintenance_components',
    count(*)::bigint,
    jsonb_build_object(
        'missing_or_disabled', COALESCE(jsonb_agg(name ORDER BY name), '[]'::jsonb)
    )::text
FROM missing;

WITH raw_streams AS (
    SELECT DISTINCT client_id, source_kind, interface
    FROM public.traffic_counter_samples
), missing AS (
    SELECT raw.*
    FROM raw_streams raw
    LEFT JOIN public.traffic_counter_hourly_usage_streams coverage
      USING (client_id, source_kind, interface)
    WHERE coverage.client_id IS NULL
), examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias
    FROM missing
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'hourly_coverage_marker_missing',
    (SELECT count(*) FROM missing)::bigint,
    jsonb_build_object(
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(stream_alias ORDER BY stream_alias) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH mismatched AS (
    SELECT client_id, source_kind, interface, source_revision, materialized_revision
    FROM public.traffic_counter_hourly_usage_streams
    WHERE source_revision <> materialized_revision
), examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias,
        source_revision,
        materialized_revision
    FROM mismatched
    ORDER BY stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'hourly_revision_mismatch',
    (SELECT count(*) FROM mismatched)::bigint,
    jsonb_build_object(
        'affected_stream_examples', COALESCE(
            (SELECT jsonb_agg(
                jsonb_build_object(
                    'stream', stream_alias,
                    'source_revision', source_revision,
                    'materialized_revision', materialized_revision
                ) ORDER BY stream_alias
            ) FROM examples),
            '[]'::jsonb
        )
    )::text;

WITH orphaned AS (
    SELECT DISTINCT usage.client_id, usage.source_kind, usage.interface
    FROM public.traffic_counter_hourly_usage usage
    WHERE NOT EXISTS (
        SELECT 1
        FROM public.traffic_counter_samples raw
        WHERE raw.client_id = usage.client_id
          AND raw.source_kind = usage.source_kind
          AND raw.interface = usage.interface
    )
)
SELECT
    'HARD',
    'hourly_usage_without_raw_stream',
    count(*)::bigint,
    jsonb_build_object(
        'expected', 'a stream with no retained raw rows has no materialized hourly buckets'
    )::text
FROM orphaned;

SELECT
    'INFO',
    'hourly_ledger_counts',
    1::bigint,
    jsonb_build_object(
        'hourly_rows', (SELECT count(*) FROM public.traffic_counter_hourly_usage),
        'coverage_streams', (
            SELECT count(*) FROM public.traffic_counter_hourly_usage_streams
        ),
        'healthy_coverage_streams', (
            SELECT count(*)
            FROM public.traffic_counter_hourly_usage_streams
            WHERE source_revision = materialized_revision
        ),
        'empty_coverage_streams', (
            SELECT count(*)
            FROM public.traffic_counter_hourly_usage_streams coverage
            WHERE NOT EXISTS (
                SELECT 1
                FROM public.traffic_counter_samples raw
                WHERE raw.client_id = coverage.client_id
                  AND raw.source_kind = coverage.source_kind
                  AND raw.interface = coverage.interface
            )
        )
    )::text;
\else
SELECT
    'HARD',
    'hourly_ledger_schema_missing',
    1::bigint,
    jsonb_build_object(
        'required_tables', jsonb_build_array(
            'traffic_counter_hourly_usage',
            'traffic_counter_hourly_usage_streams'
        ),
        'required_migration', 13
    )::text;
\endif

\if :audit_deep
WITH sequenced AS NOT MATERIALIZED (
    SELECT
        sample.rx_bytes,
        sample.tx_bytes,
        sample.rx_counter_epoch,
        sample.tx_counter_epoch,
        sample.sample_source,
        -- The lookup index is ordered by observed_at DESC. lead() in that
        -- order is the same older predecessor that lag() in ASC order would
        -- return, without requiring a full-dataset order reversal.
        lead(sample.rx_bytes) OVER stream AS previous_rx_bytes,
        lead(sample.tx_bytes) OVER stream AS previous_tx_bytes,
        lead(sample.rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
        lead(sample.tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
        lead(sample.sample_source) OVER stream AS previous_sample_source
    FROM public.traffic_counter_samples sample
    WINDOW stream AS (
        PARTITION BY sample.client_id, sample.source_kind, sample.interface
        ORDER BY sample.observed_at DESC
    )
), finding_counts AS (
    SELECT
        count(*) FILTER (WHERE
            (rx_bytes < previous_rx_bytes
             AND rx_counter_epoch = previous_rx_counter_epoch)
            OR (tx_bytes < previous_tx_bytes
                AND tx_counter_epoch = previous_tx_counter_epoch)
        )::bigint AS decrease_same_epoch,
        count(*) FILTER (WHERE
            previous_sample_source LIKE 'vnstat_import:%'
            AND sample_source NOT LIKE 'vnstat_import:%'
            AND (rx_counter_epoch = previous_rx_counter_epoch
                 OR tx_counter_epoch = previous_tx_counter_epoch)
        )::bigint AS imported_live_same_epoch
    FROM sequenced
)
SELECT
    'WARN',
    'counter_decrease_without_epoch_change',
    finding_counts.decrease_same_epoch,
    jsonb_build_object(
        'meaning', 'the accounting oracle contributes zero for these suspicious transitions'
    )::text
FROM finding_counts
UNION ALL
SELECT
    'HARD',
    'import_to_live_epoch_boundary',
    finding_counts.imported_live_same_epoch,
    jsonb_build_object(
        'expected', 'both directions change epoch at every vnStat-import to live transition'
    )::text
FROM finding_counts;

\if :audit_has_hourly_schema
WITH sequenced AS NOT MATERIALIZED (
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
        lead(sample.rx_bytes) OVER stream AS previous_rx_bytes,
        lead(sample.tx_bytes) OVER stream AS previous_tx_bytes,
        lead(sample.rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
        lead(sample.tx_counter_epoch) OVER stream AS previous_tx_counter_epoch,
        lead(sample.sample_source) OVER stream AS previous_sample_source
    FROM public.traffic_counter_samples sample
    WINDOW stream AS (
        PARTITION BY sample.client_id, sample.source_kind, sample.interface
        ORDER BY sample.observed_at DESC
    )
), expected AS NOT MATERIALIZED (
    SELECT
        client_id,
        source_kind,
        interface,
        date_bin(
            interval '1 hour',
            observed_at,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        ) AS bucket_start,
        COALESCE(sum(CASE
            WHEN rx_counter_epoch = previous_rx_counter_epoch
             AND rx_bytes >= previous_rx_bytes
            THEN rx_bytes - previous_rx_bytes ELSE 0 END
        ), 0)::bigint AS rx_bytes,
        COALESCE(sum(CASE
            WHEN tx_counter_epoch = previous_tx_counter_epoch
             AND tx_bytes >= previous_tx_bytes
            THEN tx_bytes - previous_tx_bytes ELSE 0 END
        ), 0)::bigint AS tx_bytes,
        count(*) FILTER (
            WHERE previous_rx_counter_epoch IS NOT NULL
              AND rx_counter_epoch <> previous_rx_counter_epoch
              AND NOT (
                  previous_sample_source LIKE 'vnstat_import:%'
                  AND sample_source NOT LIKE 'vnstat_import:%'
              )
        )::integer AS rx_reset_count,
        count(*) FILTER (
            WHERE previous_tx_counter_epoch IS NOT NULL
              AND tx_counter_epoch <> previous_tx_counter_epoch
              AND NOT (
                  previous_sample_source LIKE 'vnstat_import:%'
                  AND sample_source NOT LIKE 'vnstat_import:%'
              )
        )::integer AS tx_reset_count,
        count(*)::integer AS sample_count,
        min(observed_at) AS first_observed_at,
        max(observed_at) AS latest_observed_at
    FROM sequenced
    GROUP BY
        client_id,
        source_kind,
        interface,
        date_bin(
            interval '1 hour',
            observed_at,
            TIMESTAMPTZ '1970-01-01 00:00:00+00'
        )
), mismatched AS MATERIALIZED (
    SELECT
        COALESCE(expected.client_id, actual.client_id) AS client_id,
        COALESCE(expected.source_kind, actual.source_kind) AS source_kind,
        COALESCE(expected.interface, actual.interface) AS interface,
        COALESCE(expected.bucket_start, actual.bucket_start) AS bucket_start,
        CASE
            WHEN expected.client_id IS NULL THEN 'unexpected_materialized_bucket'
            WHEN actual.client_id IS NULL THEN 'missing_materialized_bucket'
            ELSE 'field_mismatch'
        END AS mismatch_kind
    FROM expected
    FULL JOIN public.traffic_counter_hourly_usage actual
      USING (client_id, source_kind, interface, bucket_start)
    WHERE expected.client_id IS NULL
       OR actual.client_id IS NULL
       OR expected.rx_bytes IS DISTINCT FROM actual.rx_bytes
       OR expected.tx_bytes IS DISTINCT FROM actual.tx_bytes
       OR expected.rx_reset_count IS DISTINCT FROM actual.rx_reset_count
       OR expected.tx_reset_count IS DISTINCT FROM actual.tx_reset_count
       OR expected.sample_count IS DISTINCT FROM actual.sample_count
       OR expected.first_observed_at IS DISTINCT FROM actual.first_observed_at
       OR expected.latest_observed_at IS DISTINCT FROM actual.latest_observed_at
), examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/' || source_kind || '/' || interface
            ELSE 'stream-' || left(md5(
                :'audit_identity_salt' || ':stream:' || client_id || ':' ||
                source_kind || ':' || interface
            ), 20)
        END AS stream_alias,
        bucket_start,
        mismatch_kind
    FROM mismatched
    ORDER BY bucket_start, stream_alias
    LIMIT 10
)
SELECT
    'HARD',
    'hourly_usage_parity',
    (SELECT count(*) FROM mismatched)::bigint,
    jsonb_build_object(
        'compared_fields', jsonb_build_array(
            'rx_bytes', 'tx_bytes', 'rx_reset_count', 'tx_reset_count',
            'sample_count', 'first_observed_at', 'latest_observed_at'
        ),
        'affected_bucket_examples', COALESCE(
            (SELECT jsonb_agg(
                jsonb_build_object(
                    'stream', stream_alias,
                    'bucket_start', bucket_start,
                    'kind', mismatch_kind
                ) ORDER BY bucket_start, stream_alias
            ) FROM examples),
            '[]'::jsonb
        )
    )::text;
\endif

\if :audit_server_is_pg16
WITH import_outputs AS MATERIALIZED (
    SELECT
        job.id AS job_id,
        target.client_id,
        target.status AS target_status,
        output.seq,
        output.done,
        output.data,
        CASE
            WHEN convert_from(output.data, 'UTF8') IS JSON OBJECT
            THEN convert_from(output.data, 'UTF8')::jsonb
            ELSE NULL
        END AS document
    FROM public.jobs job
    JOIN public.job_targets target ON target.job_id = job.id
    JOIN public.job_outputs output
      ON output.job_id = target.job_id
     AND output.client_id = target.client_id
    WHERE job.command_type = 'network_traffic_import_vnstat'
      AND output.stream = 'status'
      AND output.storage = 'inline'
), finals AS MATERIALIZED (
    SELECT * FROM import_outputs WHERE done
), bad_finals AS (
    SELECT *
    FROM finals
    WHERE document IS NULL
       OR document->>'type' IS DISTINCT FROM 'network_traffic_import_vnstat'
       OR document->>'status' IS DISTINCT FROM 'collected'
       OR jsonb_typeof(document->'batch_count') IS DISTINCT FROM 'number'
       OR document->>'batch_count' IS DISTINCT FROM seq::text
       OR jsonb_typeof(document->'bucket_count') IS DISTINCT FROM 'number'
       OR jsonb_typeof(document->'requested_start_unix') IS DISTINCT FROM 'number'
       OR jsonb_typeof(document->'interfaces') IS DISTINCT FROM 'array'
       OR jsonb_array_length(CASE
            WHEN jsonb_typeof(document->'interfaces') = 'array'
            THEN document->'interfaces' ELSE '[]'::jsonb
          END) = 0
       OR jsonb_typeof(document->'sources') IS DISTINCT FROM 'array'
       OR jsonb_array_length(CASE
            WHEN jsonb_typeof(document->'sources') = 'array'
            THEN document->'sources' ELSE '[]'::jsonb
          END) <> jsonb_array_length(CASE
            WHEN jsonb_typeof(document->'interfaces') = 'array'
            THEN document->'interfaces' ELSE '[]'::jsonb
          END)
       OR EXISTS (
            SELECT 1
            FROM jsonb_array_elements(CASE
                WHEN jsonb_typeof(document->'interfaces') = 'array'
                THEN document->'interfaces' ELSE '[]'::jsonb
            END) interface(value)
            WHERE jsonb_typeof(interface.value) IS DISTINCT FROM 'string'
       )
       OR EXISTS (
            SELECT 1
            FROM jsonb_array_elements(CASE
                WHEN jsonb_typeof(document->'sources') = 'array'
                THEN document->'sources' ELSE '[]'::jsonb
            END) source(value)
            WHERE jsonb_typeof(source.value) IS DISTINCT FROM 'object'
               OR jsonb_typeof(source.value->'interface') IS DISTINCT FROM 'string'
               OR jsonb_typeof(source.value->'retained_start_unix') IS DISTINCT FROM 'number'
       )
       OR (
            SELECT count(DISTINCT source.value->>'interface')
            FROM jsonb_array_elements(CASE
                WHEN jsonb_typeof(document->'sources') = 'array'
                THEN document->'sources' ELSE '[]'::jsonb
            END) source(value)
       ) <> jsonb_array_length(CASE
            WHEN jsonb_typeof(document->'sources') = 'array'
            THEN document->'sources' ELSE '[]'::jsonb
       END)
       OR (
            SELECT count(DISTINCT (interface.value #>> '{}'))
            FROM jsonb_array_elements(CASE
                WHEN jsonb_typeof(document->'interfaces') = 'array'
                THEN document->'interfaces' ELSE '[]'::jsonb
            END) interface(value)
       ) <> jsonb_array_length(CASE
            WHEN jsonb_typeof(document->'interfaces') = 'array'
            THEN document->'interfaces' ELSE '[]'::jsonb
       END)
       OR EXISTS (
            SELECT 1
            FROM jsonb_array_elements(CASE
                WHEN jsonb_typeof(document->'interfaces') = 'array'
                THEN document->'interfaces' ELSE '[]'::jsonb
            END) interface(value)
            WHERE NOT EXISTS (
                SELECT 1
                FROM jsonb_array_elements(CASE
                    WHEN jsonb_typeof(document->'sources') = 'array'
                    THEN document->'sources' ELSE '[]'::jsonb
                END) source(value)
                WHERE source.value->>'interface' = interface.value #>> '{}'
            )
       )
), bad_batches AS (
    SELECT output.*
    FROM import_outputs output
    JOIN finals final
      ON final.job_id = output.job_id
     AND final.client_id = output.client_id
    WHERE output.seq < final.seq
      AND (
          output.done
          OR output.document IS NULL
          OR output.document->>'type' IS DISTINCT FROM 'network_traffic_import_vnstat_batch'
          OR jsonb_typeof(output.document->'batch_index') IS DISTINCT FROM 'number'
          OR output.document->>'batch_index' IS DISTINCT FROM output.seq::text
          OR jsonb_typeof(output.document->'buckets') IS DISTINCT FROM 'array'
      )
), valid_batches AS MATERIALIZED (
    SELECT output.*
    FROM import_outputs output
    JOIN finals final
      ON final.job_id = output.job_id
     AND final.client_id = output.client_id
    WHERE output.seq < final.seq
      AND NOT output.done
      AND output.document->>'type' = 'network_traffic_import_vnstat_batch'
      AND jsonb_typeof(output.document->'buckets') = 'array'
), bad_buckets AS (
    SELECT bucket
    FROM valid_batches batch
    CROSS JOIN LATERAL jsonb_array_elements(batch.document->'buckets') bucket
    WHERE jsonb_typeof(bucket) IS DISTINCT FROM 'object'
       OR jsonb_typeof(bucket->'interface') IS DISTINCT FROM 'string'
       OR jsonb_typeof(bucket->'start_unix') IS DISTINCT FROM 'number'
       OR jsonb_typeof(bucket->'duration_secs') IS DISTINCT FROM 'number'
       OR jsonb_typeof(bucket->'rx_bytes') IS DISTINCT FROM 'number'
       OR jsonb_typeof(bucket->'tx_bytes') IS DISTINCT FROM 'number'
)
SELECT
    'HARD',
    'import_final_output_contract',
    (SELECT count(*) FROM bad_finals)::bigint,
    jsonb_build_object('expected', 'validated collected-result status JSON')::text
UNION ALL
SELECT
    'HARD',
    'import_batch_output_contract',
    (SELECT count(*) FROM bad_batches)::bigint,
    jsonb_build_object('expected', 'validated ordered batch status JSON')::text
UNION ALL
SELECT
    'HARD',
    'import_bucket_output_contract',
    (SELECT count(*) FROM bad_buckets)::bigint,
    jsonb_build_object('expected', 'typed vnStat bucket fields')::text;

WITH final_rows AS MATERIALIZED (
    SELECT
        job.id AS job_id,
        job.created_at,
        target.client_id,
        target.completed_at,
        target.message,
        output.seq AS final_seq,
        CASE
            WHEN convert_from(output.data, 'UTF8') IS JSON OBJECT
            THEN convert_from(output.data, 'UTF8')::jsonb
            ELSE NULL
        END AS document
    FROM public.jobs job
    JOIN public.job_targets target ON target.job_id = job.id
    JOIN public.job_outputs output
      ON output.job_id = target.job_id
     AND output.client_id = target.client_id
    WHERE job.command_type = 'network_traffic_import_vnstat'
      AND target.status = 'completed'
      AND output.done
      AND output.stream = 'status'
      AND output.storage = 'inline'
), contract_finals AS MATERIALIZED (
    SELECT
        final.*,
        (
            final.document IS NOT NULL
            AND final.document->>'type' IS NOT DISTINCT FROM
                'network_traffic_import_vnstat'
            AND final.document->>'status' IS NOT DISTINCT FROM 'collected'
            AND jsonb_typeof(final.document->'requested_start_unix')
                IS NOT DISTINCT FROM 'number'
            AND jsonb_typeof(final.document->'interfaces')
                IS NOT DISTINCT FROM 'array'
            AND jsonb_array_length(CASE
                WHEN jsonb_typeof(final.document->'interfaces') = 'array'
                THEN final.document->'interfaces' ELSE '[]'::jsonb
            END) > 0
            AND jsonb_typeof(final.document->'sources')
                IS NOT DISTINCT FROM 'array'
            AND jsonb_array_length(CASE
                WHEN jsonb_typeof(final.document->'sources') = 'array'
                THEN final.document->'sources' ELSE '[]'::jsonb
            END) = jsonb_array_length(CASE
                WHEN jsonb_typeof(final.document->'interfaces') = 'array'
                THEN final.document->'interfaces' ELSE '[]'::jsonb
            END)
            AND NOT EXISTS (
                SELECT 1
                FROM jsonb_array_elements(CASE
                    WHEN jsonb_typeof(final.document->'interfaces') = 'array'
                    THEN final.document->'interfaces' ELSE '[]'::jsonb
                END) interface(value)
                WHERE jsonb_typeof(interface.value) IS DISTINCT FROM 'string'
            )
            AND NOT EXISTS (
                SELECT 1
                FROM jsonb_array_elements(CASE
                    WHEN jsonb_typeof(final.document->'sources') = 'array'
                    THEN final.document->'sources' ELSE '[]'::jsonb
                END) source(value)
                WHERE jsonb_typeof(source.value) IS DISTINCT FROM 'object'
                   OR jsonb_typeof(source.value->'interface')
                        IS DISTINCT FROM 'string'
                   OR jsonb_typeof(source.value->'retained_start_unix')
                        IS DISTINCT FROM 'number'
            )
            AND (
                SELECT count(DISTINCT source.value->>'interface')
                FROM jsonb_array_elements(CASE
                    WHEN jsonb_typeof(final.document->'sources') = 'array'
                    THEN final.document->'sources' ELSE '[]'::jsonb
                END) source(value)
            ) = jsonb_array_length(CASE
                WHEN jsonb_typeof(final.document->'sources') = 'array'
                THEN final.document->'sources' ELSE '[]'::jsonb
            END)
            AND (
                SELECT count(DISTINCT (interface.value #>> '{}'))
                FROM jsonb_array_elements(CASE
                    WHEN jsonb_typeof(final.document->'interfaces') = 'array'
                    THEN final.document->'interfaces' ELSE '[]'::jsonb
                END) interface(value)
            ) = jsonb_array_length(CASE
                WHEN jsonb_typeof(final.document->'interfaces') = 'array'
                THEN final.document->'interfaces' ELSE '[]'::jsonb
            END)
            AND NOT EXISTS (
                SELECT 1
                FROM jsonb_array_elements(CASE
                    WHEN jsonb_typeof(final.document->'interfaces') = 'array'
                    THEN final.document->'interfaces' ELSE '[]'::jsonb
                END) interface(value)
                WHERE NOT EXISTS (
                    SELECT 1
                    FROM jsonb_array_elements(CASE
                        WHEN jsonb_typeof(final.document->'sources') = 'array'
                        THEN final.document->'sources' ELSE '[]'::jsonb
                    END) source(value)
                    WHERE source.value->>'interface' = interface.value #>> '{}'
                )
            )
        ) AS contract_valid
    FROM final_rows final
), valid_finals AS MATERIALIZED (
    SELECT final.*
    FROM contract_finals final
    WHERE final.contract_valid IS TRUE
      AND final.message ~ '[0-9]+ RX bytes, [0-9]+ TX bytes'
      AND (
          SELECT count(*)
          FROM public.job_outputs output
          WHERE output.job_id = final.job_id
            AND output.client_id = final.client_id
            AND output.seq BETWEEN 0 AND final.final_seq
      ) = final.final_seq::bigint + 1
), expanded AS MATERIALIZED (
    SELECT
        final.*,
        interface.value AS interface,
        row_number() OVER (
            PARTITION BY final.client_id, interface.value
            ORDER BY final.completed_at DESC NULLS LAST,
                     final.created_at DESC,
                     final.job_id DESC
        ) AS interface_rank
    FROM valid_finals final
    CROSS JOIN LATERAL jsonb_array_elements_text(final.document->'interfaces') interface(value)
), partial_replacements AS MATERIALIZED (
    SELECT
        job_id,
        client_id,
        count(*) AS interface_count,
        count(*) FILTER (WHERE interface_rank = 1) AS current_interface_count
    FROM expanded
    GROUP BY job_id, client_id
    HAVING count(*) FILTER (WHERE interface_rank = 1) > 0
       AND count(*) FILTER (WHERE interface_rank = 1) < count(*)
), all_interfaces_latest AS MATERIALIZED (
    SELECT
        job_id,
        client_id,
        max(created_at) AS created_at,
        max(completed_at) AS completed_at,
        max(message) AS message,
        max(document::text)::jsonb AS document,
        count(*) AS interface_count
    FROM expanded
    GROUP BY job_id, client_id
    HAVING bool_and(interface_rank = 1)
), policy AS (
    SELECT
        COALESCE(
            (SELECT enabled FROM public.history_retention_policies
             WHERE domain = 'traffic_counter_samples'),
            true
        ) AS enabled,
        COALESCE(
            (SELECT retention_days FROM public.history_retention_policies
             WHERE domain = 'traffic_counter_samples'),
            3650
        ) AS retention_days
), candidate_state AS MATERIALIZED (
    SELECT
        latest.*,
        match.captures,
        policy.enabled AND EXISTS (
            SELECT 1
            FROM jsonb_array_elements(latest.document->'sources') source
            WHERE jsonb_typeof(source->'retained_start_unix') = 'number'
              AND GREATEST(
                    (latest.document->>'requested_start_unix')::numeric,
                    (source->>'retained_start_unix')::numeric
                  ) < extract(epoch FROM (
                    (date_trunc('day', current_timestamp AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                    - make_interval(days => policy.retention_days)
                  ))::numeric
        ) AS clipped_by_retention,
        EXISTS (
            SELECT 1
            FROM public.jobs active_job
            JOIN public.job_targets active_target ON active_target.job_id = active_job.id
            WHERE active_job.command_type = 'network_traffic_import_vnstat'
              AND active_target.client_id = latest.client_id
              AND active_target.completed_at IS NULL
        ) AS active_import
    FROM all_interfaces_latest latest
    CROSS JOIN policy
    CROSS JOIN LATERAL regexp_match(
        latest.message,
        '([0-9]+) RX bytes, ([0-9]+) TX bytes'
    ) match(captures)
), candidates AS MATERIALIZED (
    SELECT *
    FROM candidate_state
    WHERE NOT clipped_by_retention
      AND NOT active_import
), selected AS MATERIALIZED (
    SELECT
        candidate.job_id,
        candidate.client_id,
        candidate.captures,
        interface.value AS interface
    FROM candidates candidate
    CROSS JOIN LATERAL jsonb_array_elements_text(candidate.document->'interfaces') interface(value)
), raw_sequenced AS NOT MATERIALIZED (
    SELECT
        selected.job_id,
        selected.client_id,
        sample.rx_bytes,
        sample.tx_bytes,
        sample.rx_counter_epoch,
        sample.tx_counter_epoch,
        sample.sample_source,
        sample.inbound_promoted,
        lead(sample.rx_bytes) OVER stream AS previous_rx_bytes,
        lead(sample.tx_bytes) OVER stream AS previous_tx_bytes,
        lead(sample.rx_counter_epoch) OVER stream AS previous_rx_counter_epoch,
        lead(sample.tx_counter_epoch) OVER stream AS previous_tx_counter_epoch
    FROM selected
    JOIN public.traffic_counter_samples sample
      ON sample.client_id = selected.client_id
     AND sample.source_kind = 'host'
     AND sample.interface = selected.interface
    WINDOW stream AS (
        PARTITION BY selected.job_id, selected.client_id, selected.interface
        ORDER BY sample.observed_at DESC
    )
), raw_usage AS MATERIALIZED (
    SELECT
        selected.job_id,
        selected.client_id,
        COALESCE(sum(CASE
            WHEN raw.sample_source = 'vnstat_import:' || selected.job_id::text
             AND NOT raw.inbound_promoted
             AND raw.rx_counter_epoch = raw.previous_rx_counter_epoch
             AND raw.rx_bytes >= raw.previous_rx_bytes
            THEN raw.rx_bytes - raw.previous_rx_bytes ELSE 0 END
        ), 0)::numeric AS rx_bytes,
        COALESCE(sum(CASE
            WHEN raw.sample_source = 'vnstat_import:' || selected.job_id::text
             AND NOT raw.inbound_promoted
             AND raw.tx_counter_epoch = raw.previous_tx_counter_epoch
             AND raw.tx_bytes >= raw.previous_tx_bytes
            THEN raw.tx_bytes - raw.previous_tx_bytes ELSE 0 END
        ), 0)::numeric AS tx_bytes
    FROM (SELECT DISTINCT job_id, client_id FROM selected) selected
    LEFT JOIN raw_sequenced raw
      ON raw.job_id = selected.job_id
     AND raw.client_id = selected.client_id
    GROUP BY selected.job_id, selected.client_id
), effective_rollups AS NOT MATERIALIZED (
    SELECT
        selected.job_id,
        selected.client_id,
        rollup.rx_bytes,
        rollup.tx_bytes
    FROM selected
    JOIN public.traffic_counter_rollups rollup
      ON rollup.client_id = selected.client_id
     AND rollup.source_kind = 'host'
     AND rollup.interface = selected.interface
     AND rollup.origin_kind = 'vnstat_import'
    WHERE NOT EXISTS (
        SELECT 1
        FROM public.traffic_counter_rollups finer
        WHERE finer.client_id = rollup.client_id
          AND finer.source_kind = rollup.source_kind
          AND finer.interface = rollup.interface
          AND finer.origin_kind = rollup.origin_kind
          AND finer.bucket_secs < rollup.bucket_secs
          AND finer.bucket_start
                < rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
          AND finer.bucket_start + make_interval(secs => finer.bucket_secs)
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
          AND exact.sample_source LIKE 'vnstat_import:%'
          AND exact.observed_at >= rollup.bucket_start
          AND exact.observed_at
                < rollup.bucket_start + make_interval(secs => rollup.bucket_secs)
        OFFSET 0
    )
), rollup_usage AS MATERIALIZED (
    SELECT
        selected.job_id,
        selected.client_id,
        COALESCE(sum(rollup.rx_bytes), 0)::numeric AS rx_bytes,
        COALESCE(sum(rollup.tx_bytes), 0)::numeric AS tx_bytes
    FROM (SELECT DISTINCT job_id, client_id FROM selected) selected
    LEFT JOIN effective_rollups rollup
      ON rollup.job_id = selected.job_id
     AND rollup.client_id = selected.client_id
    GROUP BY selected.job_id, selected.client_id
), compared AS MATERIALIZED (
    SELECT
        candidate.job_id,
        candidate.client_id,
        candidate.captures[1]::numeric AS expected_rx,
        candidate.captures[2]::numeric AS expected_tx,
        raw.rx_bytes + rollup.rx_bytes AS actual_rx,
        raw.tx_bytes + rollup.tx_bytes AS actual_tx
    FROM candidates candidate
    JOIN raw_usage raw
      ON raw.job_id = candidate.job_id
     AND raw.client_id = candidate.client_id
    JOIN rollup_usage rollup
      ON rollup.job_id = candidate.job_id
     AND rollup.client_id = candidate.client_id
), mismatched AS (
    SELECT *
    FROM compared
    WHERE expected_rx <> actual_rx OR expected_tx <> actual_tx
), examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/job-' || job_id::text
            ELSE 'target-' || left(md5(
                :'audit_identity_salt' || ':target:' || client_id || ':' || job_id::text
            ), 20)
        END AS target_alias
    FROM mismatched
    ORDER BY target_alias
    LIMIT 10
), partial_examples AS (
    SELECT
        CASE WHEN :'audit_show_identities'::boolean
            THEN client_id || '/job-' || job_id::text
            ELSE 'target-' || left(md5(
                :'audit_identity_salt' || ':target:' || client_id || ':' || job_id::text
            ), 20)
        END AS target_alias,
        current_interface_count,
        interface_count
    FROM partial_replacements
    ORDER BY target_alias
    LIMIT 10
)
SELECT
    'HARD',
    'import_conservation',
    (SELECT count(*) FROM mismatched)::bigint,
    jsonb_build_object(
        'checked_targets', (SELECT count(*) FROM compared),
        'affected_target_examples', COALESCE(
            (SELECT jsonb_agg(target_alias ORDER BY target_alias) FROM examples),
            '[]'::jsonb
        ),
        'expected_source', 'server-recorded atomic import summary',
        'actual_source', 'effective de-overlapped imported raw and rollup ledger'
    )::text
UNION ALL
SELECT
    'WARN',
    'import_conservation_skipped_by_retention',
    (SELECT count(*) FILTER (WHERE clipped_by_retention)
     FROM candidate_state)::bigint,
    jsonb_build_object(
        'meaning', 'the configured final-retention cutoff can legitimately remove part of the original imported total'
    )::text
UNION ALL
SELECT
    'WARN',
    'conservation_skipped_by_partial_replacement',
    (SELECT count(*) FROM partial_replacements)::bigint,
    jsonb_build_object(
        'skipped_job_targets', (SELECT count(*) FROM partial_replacements),
        'skipped_target_examples', COALESCE(
            (SELECT jsonb_agg(
                jsonb_build_object(
                    'target', target_alias,
                    'current_interfaces', current_interface_count,
                    'original_interfaces', interface_count
                ) ORDER BY target_alias
            ) FROM partial_examples),
            '[]'::jsonb
        ),
        'meaning', 'a later import replaced only part of this job target; its aggregate server summary cannot validate the still-current interfaces alone'
    )::text;
\else
SELECT
    'HARD',
    'postgres_16_required_for_deep_import_audit',
    1::bigint,
    jsonb_build_object(
        'server_version_num', current_setting('server_version_num')::integer
    )::text;
\endif
\endif
\endif

COMMIT;
