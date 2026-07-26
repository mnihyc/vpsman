ALTER TABLE history_retention_policies
    DROP CONSTRAINT history_retention_policies_domain_check;

ALTER TABLE history_retention_policies
    ADD CONSTRAINT history_retention_policies_domain_check
    CHECK (domain IN (
        'audit_logs',
        'system_metric_rollups',
        'telemetry_rollups',
        'telemetry_network_rates',
        'traffic_counter_samples',
        'job_outputs',
        'backup_artifacts',
        'network_observations',
        'topology_history',
        'client_status_history',
        'gateway_sessions'
    ));

ALTER TABLE history_retention_policies
    ADD CONSTRAINT history_retention_policies_traffic_counter_min_days_check
    CHECK (
        domain <> 'traffic_counter_samples'
        OR retention_days >= 32
    );
