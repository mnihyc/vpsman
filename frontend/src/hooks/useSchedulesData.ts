import { useCallback, useState } from "react";
import { apiGet, apiPost, isApiUnauthorized } from "../api";
import type { CreateScheduleRequest, ScheduleRecord } from "../types";

export function useSchedulesData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
) {
  const [schedules, setSchedules] = useState<ScheduleRecord[]>([]);
  const [schedulesError, setSchedulesError] = useState<string | null>(null);
  const [schedulesLoading, setSchedulesLoading] = useState(false);

  const loadSchedules = useCallback(async () => {
    setSchedulesLoading(true);
    setSchedulesError(null);
    try {
      setSchedules(await apiGet<ScheduleRecord[]>("/api/v1/schedules", apiToken));
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setSchedules([]);
        setSchedulesError("Operator login required");
        return;
      }
      setSchedulesError(error instanceof Error ? error.message : "Schedules unavailable");
    } finally {
      setSchedulesLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const createSchedule = useCallback(
    async (request: CreateScheduleRequest) => {
      await apiPost<ScheduleRecord>("/api/v1/schedules", apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  return {
    createSchedule,
    loadSchedules,
    schedules,
    schedulesError,
    schedulesLoading,
  };
}
