-- Promotion replaces history rather than removing it. Only terminal deletion
-- owners publish the existing orphan-cleanup wake, in their delete transaction.
-- Keep the independent Ping bounds and dashboard maintenance triggers intact.
DROP TRIGGER telemetry_ping_rollups_retention_delete
ON public.telemetry_ping_rollups;
