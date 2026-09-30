-- max_failures now counts tolerated failures: -1 disables failure-based pausing,
-- 0 pauses on the first failure, and N >= 0 pauses on failure N + 1.
-- Keep the existing upper bound of 100 tolerated failures.
ALTER TABLE public.schedules
    DROP CONSTRAINT schedules_max_failures_check,
    ALTER COLUMN max_failures SET DEFAULT -1;

ALTER TABLE public.schedules
    ADD CONSTRAINT schedules_max_failures_check
    CHECK (max_failures BETWEEN -1 AND 100);

COMMENT ON COLUMN public.schedules.max_failures IS
    'Failures tolerated before automatic pausing: -1 disables failure-based pausing; 0 pauses on the first failure; N >= 0 pauses on failure N + 1.';
