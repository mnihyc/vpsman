ALTER TABLE job_targets
    ADD COLUMN capability_degraded_reason TEXT,
    ADD COLUMN capability_degraded_hint TEXT,
    ADD CONSTRAINT job_targets_capability_degraded_pair_check CHECK (
        (capability_degraded_reason IS NULL) = (capability_degraded_hint IS NULL)
    ),
    ADD CONSTRAINT job_targets_capability_degraded_reason_check CHECK (
        capability_degraded_reason IS NULL
        OR length(trim(capability_degraded_reason)) BETWEEN 1 AND 256
    ),
    ADD CONSTRAINT job_targets_capability_degraded_hint_check CHECK (
        capability_degraded_hint IS NULL
        OR length(trim(capability_degraded_hint)) BETWEEN 1 AND 2048
    );

DO $$
DECLARE
    candidate RECORD;
    payload JSONB;
BEGIN
    FOR candidate IN
        SELECT
            output.job_id,
            output.client_id,
            output.data,
            job.command_type
        FROM job_outputs AS output
        JOIN job_targets AS target
          ON target.job_id = output.job_id
         AND target.client_id = output.client_id
        JOIN jobs AS job ON job.id = target.job_id
        WHERE target.status = 'skipped'
          AND target.capability_degraded_reason IS NULL
          AND COALESCE(target.completed_at, target.started_at) IS NOT NULL
          AND output.stream = 'status'
          AND output.storage = 'inline'
        ORDER BY output.job_id, output.client_id, output.seq
    LOOP
        BEGIN
            payload := convert_from(candidate.data, 'UTF8')::jsonb;
        EXCEPTION WHEN OTHERS THEN
            CONTINUE;
        END;

        IF payload->>'type' = 'capability_degraded'
           AND payload->>'status' = 'skipped'
           AND payload->>'client_id' = candidate.client_id
           AND (
                payload->>'command_type' = candidate.command_type
                OR candidate.command_type = 'scheduled_' || payload->>'command_type'
           )
           AND length(trim(COALESCE(payload->>'reason', ''))) BETWEEN 1 AND 256
           AND length(trim(COALESCE(payload->>'hint', ''))) BETWEEN 1 AND 2048
        THEN
            UPDATE job_targets
            SET capability_degraded_reason = trim(payload->>'reason'),
                capability_degraded_hint = trim(payload->>'hint')
            WHERE job_id = candidate.job_id
              AND client_id = candidate.client_id
              AND capability_degraded_reason IS NULL;
        END IF;
    END LOOP;
END
$$;

CREATE INDEX policy_alerts_fleet_priority_idx
    ON policy_alerts (
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        observed_at DESC,
        id DESC
    );

CREATE INDEX policy_alerts_client_fleet_priority_idx
    ON policy_alerts (
        client_id,
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        observed_at DESC,
        id DESC
    );

CREATE INDEX jobs_fleet_alert_candidates_idx
    ON jobs (
        (CASE WHEN status = 'partial_success' THEN 1 ELSE 0 END),
        (COALESCE(completed_at, created_at)) DESC,
        id DESC
    )
    WHERE status IN (
        'failed',
        'agent_timeout',
        'control_timeout',
        'partial_success',
        'rejected',
        'canceled'
    );

CREATE INDEX jobs_active_dashboard_idx
    ON jobs (created_at DESC, id DESC)
    WHERE status IN ('queued', 'running');

CREATE INDEX backup_requests_failed_client_idx
    ON backup_requests (client_id, created_at DESC, id DESC)
    WHERE status = 'execution_failed';

CREATE INDEX job_targets_capability_degraded_idx
    ON job_targets (
        (COALESCE(completed_at, started_at)) DESC,
        job_id DESC,
        client_id
    )
    WHERE capability_degraded_reason IS NOT NULL;

CREATE INDEX job_targets_client_capability_degraded_idx
    ON job_targets (
        client_id,
        (COALESCE(completed_at, started_at)) DESC,
        job_id DESC
    )
    WHERE capability_degraded_reason IS NOT NULL;

CREATE INDEX backup_artifacts_client_idx
    ON backup_artifacts (client_id);

CREATE INDEX restore_plans_source_client_idx
    ON restore_plans (source_client_id);

CREATE INDEX network_observations_client_observed_idx
    ON network_observations (client_id, observed_at DESC, id DESC);

CREATE INDEX network_observations_peer_client_observed_idx
    ON network_observations (peer_client_id, observed_at DESC, id DESC);

CREATE INDEX network_observations_plan_kind_observed_idx
    ON network_observations (plan_id, kind, observed_at DESC, id DESC)
    WHERE kind IN ('network_probe', 'network_speed_test');
