-- Run-on changes the reviewed definition for future alert edges. Rearm existing
-- event schedules like an explicit definition edit; cron keeps its old behavior.
ALTER TABLE public.schedules
    ADD COLUMN run_on text NOT NULL DEFAULT 'all_at_once',
    ADD CONSTRAINT schedules_run_on_check CHECK (
        run_on IN ('triggered_only', 'all_at_once')
        AND (trigger_kind = 'event' OR run_on = 'all_at_once')
    );

UPDATE public.schedules
SET run_on = 'triggered_only',
    definition_revision = definition_revision + 1,
    event_armed_at = clock_timestamp(),
    failure_count = 0,
    last_error = NULL,
    updated_at = clock_timestamp()
WHERE trigger_kind = 'event';

-- Accepted work belongs to its old snapshot, not the edited definition above.
-- Preserve every existing receipt/job's full reviewed target set. Deploy API
-- and worker together: new writers must capture effective targets explicitly;
-- no default or legacy fallback may silently widen/narrow an accepted receipt.
ALTER TABLE public.schedule_event_receipts
    ADD COLUMN run_on text NOT NULL DEFAULT 'all_at_once',
    ADD COLUMN effective_target_client_ids text[],
    ADD CONSTRAINT schedule_event_receipts_run_on_check CHECK (
        run_on IN ('triggered_only', 'all_at_once')
    );

UPDATE public.schedule_event_receipts
SET effective_target_client_ids = fixed_target_client_ids;

ALTER TABLE public.schedule_event_receipts
    ALTER COLUMN effective_target_client_ids SET NOT NULL;
