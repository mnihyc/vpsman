ALTER TABLE jobs
  DROP CONSTRAINT IF EXISTS jobs_status_common_check;
ALTER TABLE jobs
  ADD CONSTRAINT jobs_status_common_check CHECK (status IN (
    'queued',
    'running',
    'dispatching',
    'completed',
    'partially_completed',
    'failed',
    'timed_out',
    'dispatch_failed',
    'degraded_unprivileged',
    'accepted',
    'rejected_authorization_required',
    'schedule_no_targets'
  ));

ALTER TABLE job_targets
  DROP CONSTRAINT IF EXISTS job_targets_status_common_check;
ALTER TABLE job_targets
  ADD CONSTRAINT job_targets_status_common_check CHECK (status IN (
    'queued',
    'dispatching',
    'accepted',
    'completed',
    'failed',
    'timed_out',
    'dispatch_failed',
    'degraded_unprivileged',
    'rejected_by_agent',
    'rejected_authorization_required'
  ));

ALTER TABLE gateway_sessions
  DROP CONSTRAINT IF EXISTS gateway_sessions_status_common_check;
ALTER TABLE gateway_sessions
  ADD CONSTRAINT gateway_sessions_status_common_check CHECK (status IN (
    'connected',
    'disconnected',
    'expired'
  ));
