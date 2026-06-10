import { useCallback, useState } from "react";
import { apiDelete, apiGet, apiPost, apiPut, buildListPath, isApiUnauthorized } from "../api";
import type {
  CreateScheduleRequest,
  DeferScheduleRequest,
  SchedulePrivilegeMutationRequest,
  ScheduleRecord,
  UpdateScheduleRequest,
  UpdateScheduleTargetsRequest,
} from "../types";

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
      setSchedules(
        await apiGet<ScheduleRecord[]>(
          buildListPath("/api/v1/schedules", { limit: 1000, sort: "next_run_at", dir: "asc" }),
          apiToken,
        ),
      );
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

  const updateSchedule = useCallback(
    async (scheduleId: string, request: UpdateScheduleRequest) => {
      await apiPut<ScheduleRecord>(`/api/v1/schedules/${scheduleId}`, apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const updateScheduleTargets = useCallback(
    async (scheduleId: string, request: UpdateScheduleTargetsRequest) => {
      await apiPost<ScheduleRecord>(`/api/v1/schedules/${scheduleId}/targets`, apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const enableSchedule = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      await apiPost<ScheduleRecord>(`/api/v1/schedules/${scheduleId}/enable`, apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const disableSchedule = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      await apiPost<ScheduleRecord>(`/api/v1/schedules/${scheduleId}/disable`, apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const deferSchedule = useCallback(
    async (scheduleId: string, request: DeferScheduleRequest) => {
      await apiPost<ScheduleRecord>(`/api/v1/schedules/${scheduleId}/defer`, apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const applyScheduleNow = useCallback(
    async (scheduleId: string) => {
      await apiPost(`/api/v1/schedules/${scheduleId}/apply-now`, apiToken, {});
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const deleteSchedule = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      await apiDelete(`/api/v1/schedules/${scheduleId}`, apiToken, request);
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  return {
    createSchedule,
    updateSchedule,
    updateScheduleTargets,
    enableSchedule,
    disableSchedule,
    deferSchedule,
    applyScheduleNow,
    deleteSchedule,
    loadSchedules,
    schedules,
    schedulesError,
    schedulesLoading,
  };
}
