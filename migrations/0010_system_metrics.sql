-- Control-plane system metric rollups.

-- Tables.

CREATE TABLE public.system_metric_rollups (
    metric text NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    bucket_secs integer NOT NULL,
    sample_count integer NOT NULL,
    value_sum double precision DEFAULT 0 NOT NULL,
    avg_value double precision NOT NULL,
    max_value double precision NOT NULL,
    latest_value double precision NOT NULL,
    latest_observed_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT system_metric_rollups_bucket_secs_check CHECK (
        bucket_secs = ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400])
    ),
    CONSTRAINT system_metric_rollups_bucket_start_check CHECK (
        bucket_start = date_trunc('minute', bucket_start)
        AND mod(extract(epoch FROM bucket_start)::bigint, bucket_secs) = 0
    ),
    CONSTRAINT system_metric_rollups_check CHECK (((latest_observed_at >= bucket_start) AND (latest_observed_at < (bucket_start + make_interval(secs => (bucket_secs)::double precision))))),
    CONSTRAINT system_metric_rollups_metric_check CHECK (((length(TRIM(BOTH FROM metric)) >= 1) AND (length(TRIM(BOTH FROM metric)) <= 128))),
    CONSTRAINT system_metric_rollups_sample_count_check CHECK ((sample_count > 0)),
    CONSTRAINT system_metric_rollups_pkey PRIMARY KEY (bucket_secs, bucket_start, metric)
);



-- Indexes.

CREATE INDEX system_metric_rollups_history_export_idx ON public.system_metric_rollups USING btree (
    bucket_start DESC,
    metric,
    bucket_secs
) INCLUDE (
    sample_count,
    value_sum,
    avg_value,
    max_value,
    latest_value,
    latest_observed_at
);



CREATE INDEX system_metric_rollups_retention_idx ON public.system_metric_rollups USING btree (
    bucket_secs,
    bucket_start
);



-- Triggers.

CREATE TRIGGER system_metric_rollups_due_events_insert
AFTER INSERT ON public.system_metric_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'system_metric_rollups'
);



CREATE TRIGGER system_metric_rollups_due_events_update
AFTER UPDATE ON public.system_metric_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'system_metric_rollups'
);
