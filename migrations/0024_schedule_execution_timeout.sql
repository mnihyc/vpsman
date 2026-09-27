-- NULL preserves the configured schedule timeout for existing definitions.
-- Match MAX_CONFIGURABLE_JOB_TIMEOUT_SECS (seven days), not a new schedule cap.
ALTER TABLE public.schedules
    ADD COLUMN max_timeout_secs bigint
    CHECK (max_timeout_secs BETWEEN 1 AND 604800);

-- Event receipts freeze execution inputs at match time; later schedule edits
-- must not change an already accepted event's explicit timeout override.
ALTER TABLE public.schedule_event_receipts
    ADD COLUMN max_timeout_secs bigint
    CHECK (max_timeout_secs BETWEEN 1 AND 604800);
