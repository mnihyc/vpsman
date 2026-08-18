import { useCallback, useRef, useState } from "react";
import {
  apiDelete,
  apiGet,
  apiPost,
  apiPut,
  buildListPath,
  isApiUnauthorized,
} from "../api";
import { HISTORY_DETAIL_LIMIT } from "../constants";
import {
  snapshotSourceAvailable,
  snapshotSourceError,
  type SnapshotSource,
} from "../homeSnapshot";
import type {
  CreateJobResponse,
  CreateScheduleRequest,
  DeferScheduleRequest,
  EventScheduleTemplatePreviewRequest,
  EventScheduleTemplatePreviewResponse,
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
  const [schedulesTruncated, setSchedulesTruncated] = useState(false);
  const [schedulesError, setSchedulesError] = useState<string | null>(null);
  const [schedulesLoading, setSchedulesLoading] = useState(false);
  const [schedulesEvidenceAvailable, setSchedulesEvidenceAvailable] =
    useState(false);
  const schedulesLoadGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const loadSchedules = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = schedulesLoadGeneration.current + 1;
    schedulesLoadGeneration.current = generation;
    setSchedulesLoading(true);
    setSchedulesError(null);
    try {
      const records = await apiGet<ScheduleRecord[]>(
        buildListPath("/api/v1/schedules", {
          limit: HISTORY_DETAIL_LIMIT,
          sort: "next_run_at",
          dir: "asc",
        }),
        apiToken,
      );
      if (
        schedulesLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setSchedules(records);
      setSchedulesTruncated(records.length >= HISTORY_DETAIL_LIMIT);
      setSchedulesEvidenceAvailable(true);
    } catch (error) {
      if (
        schedulesLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setSchedulesEvidenceAvailable(false);
        setSchedules([]);
        setSchedulesTruncated(false);
        setSchedulesError("Operator login required");
        return;
      }
      setSchedulesEvidenceAvailable(false);
      setSchedulesError(
        error instanceof Error ? error.message : "Schedules unavailable",
      );
    } finally {
      if (
        schedulesLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setSchedulesLoading(false);
      }
    }
  }, [apiToken, onUnauthorized]);

  const beginHomeSchedulesHydration = useCallback(() => {
    setSchedulesLoading(true);
    return ++schedulesLoadGeneration.current;
  }, []);

  const hydrateHomeSchedules = useCallback(
    (generation: number, source: SnapshotSource<ScheduleRecord[]>) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (schedulesLoadGeneration.current !== generation) {
        return;
      }
      if (snapshotSourceAvailable(source)) {
        setSchedules(source.data);
        setSchedulesTruncated(source.data.length >= HISTORY_DETAIL_LIMIT);
      }
      setSchedulesEvidenceAvailable(snapshotSourceAvailable(source));
      setSchedulesError(snapshotSourceError("Schedules", source));
      setSchedulesLoading(false);
    },
    [apiToken],
  );

  const createSchedule = useCallback(
    async (request: CreateScheduleRequest) => {
      await apiPost<ScheduleRecord>("/api/v1/schedules", apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const previewEventScheduleTemplate = useCallback(
    async (request: EventScheduleTemplatePreviewRequest) =>
      apiPost<EventScheduleTemplatePreviewResponse>(
        "/api/v1/schedules/preview-event-template",
        apiToken,
        request,
      ),
    [apiToken],
  );

  const updateSchedule = useCallback(
    async (scheduleId: string, request: UpdateScheduleRequest) => {
      await apiPut<ScheduleRecord>(
        `/api/v1/schedules/${scheduleId}`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const updateScheduleTargets = useCallback(
    async (scheduleId: string, request: UpdateScheduleTargetsRequest) => {
      await apiPost<ScheduleRecord>(
        `/api/v1/schedules/${scheduleId}/targets`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const enableSchedule = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      await apiPost<ScheduleRecord>(
        `/api/v1/schedules/${scheduleId}/enable`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const disableSchedule = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      await apiPost<ScheduleRecord>(
        `/api/v1/schedules/${scheduleId}/disable`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const deferSchedule = useCallback(
    async (scheduleId: string, request: DeferScheduleRequest) => {
      await apiPost<ScheduleRecord>(
        `/api/v1/schedules/${scheduleId}/defer`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const applyScheduleNow = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      const response = await apiPost<CreateJobResponse>(
        `/api/v1/schedules/${scheduleId}/apply-now`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const deleteSchedule = useCallback(
    async (scheduleId: string, request: SchedulePrivilegeMutationRequest) => {
      await apiDelete(`/api/v1/schedules/${scheduleId}`, apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return;
      }
      await Promise.all([loadSchedules(), onAuditChanged()]);
    },
    [apiToken, loadSchedules, onAuditChanged],
  );

  const clearSchedules = useCallback(() => {
    schedulesLoadGeneration.current += 1;
    currentApiToken.current = "";
    setSchedules([]);
    setSchedulesTruncated(false);
    setSchedulesError(null);
    setSchedulesLoading(false);
    setSchedulesEvidenceAvailable(false);
  }, []);

  return {
    createSchedule,
    previewEventScheduleTemplate,
    beginHomeSchedulesHydration,
    clearSchedules,
    updateSchedule,
    updateScheduleTargets,
    enableSchedule,
    disableSchedule,
    deferSchedule,
    applyScheduleNow,
    deleteSchedule,
    loadSchedules,
    hydrateHomeSchedules,
    schedules,
    schedulesTruncated,
    schedulesError,
    schedulesEvidenceAvailable,
    schedulesLoading,
  };
}
