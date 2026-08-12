CREATE TABLE system_metric_rollups (
    metric TEXT NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    value_sum DOUBLE PRECISION NOT NULL DEFAULT 0,
    avg_value DOUBLE PRECISION NOT NULL,
    max_value DOUBLE PRECISION NOT NULL,
    latest_value DOUBLE PRECISION NOT NULL,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (metric, bucket_secs, bucket_start),
    CHECK (length(trim(metric)) BETWEEN 1 AND 128),
    CHECK (bucket_secs >= 60 AND bucket_secs % 60 = 0),
    CHECK (bucket_start = date_trunc('minute', bucket_start)),
    CHECK (sample_count > 0),
    CHECK (
        latest_observed_at >= bucket_start
        AND latest_observed_at < bucket_start + make_interval(secs => bucket_secs)
    )
);

CREATE INDEX system_metric_rollups_latest_idx
    ON system_metric_rollups (bucket_secs, bucket_start DESC, metric);

CREATE INDEX system_metric_rollups_retention_idx
    ON system_metric_rollups (bucket_start);
