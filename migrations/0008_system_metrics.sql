CREATE TABLE system_metric_rollups (
    metric TEXT NOT NULL,
    bucket_start TIMESTAMPTZ NOT NULL,
    bucket_secs INTEGER NOT NULL,
    sample_count INTEGER NOT NULL,
    avg_value DOUBLE PRECISION NOT NULL,
    max_value DOUBLE PRECISION NOT NULL,
    latest_value DOUBLE PRECISION NOT NULL,
    latest_observed_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (metric, bucket_secs, bucket_start),
    CHECK (length(trim(metric)) BETWEEN 1 AND 128),
    CHECK (bucket_secs > 0),
    CHECK (sample_count > 0)
);

CREATE INDEX system_metric_rollups_latest_idx
    ON system_metric_rollups (bucket_secs, bucket_start DESC, metric);
