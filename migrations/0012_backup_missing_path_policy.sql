ALTER TABLE backup_requests
    ADD COLUMN missing_path_policy TEXT NOT NULL DEFAULT 'fail';

ALTER TABLE backup_requests
    ADD CONSTRAINT backup_requests_missing_path_policy_check
    CHECK (missing_path_policy IN ('fail', 'skip'));
