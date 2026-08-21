-- traffic_cycle_usage was a prototype cycle-summary store that has never been
-- read or written by the current repository.  Current accounting is sourced
-- from traffic_counter_samples, traffic_counter_rollups, and the revisioned
-- hourly ledger.  Retire only this exact table (without CASCADE) so an
-- unexpected external dependency fails the migration instead of being
-- silently removed.
DROP TABLE IF EXISTS public.traffic_cycle_usage;
