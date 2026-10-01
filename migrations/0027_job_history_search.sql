-- History searches include completed executions, unlike dispatch/active indexes.
-- Folded equality matches the search contract; date + ID support continuation.
CREATE INDEX jobs_history_type_created_idx
    ON jobs (lower(command_type), created_at DESC, id DESC);
CREATE INDEX jobs_history_status_created_idx
    ON jobs (lower(status), created_at DESC, id DESC);

-- Direct ID matching is case insensitive. Name matching first resolves the
-- current client, then joins its exact recorded ID without changing identity.
CREATE INDEX job_targets_history_folded_client_idx
    ON job_targets (lower(client_id), job_id);
CREATE INDEX job_targets_history_client_idx
    ON job_targets (client_id, job_id);
