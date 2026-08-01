import { useCallback, useRef, useState } from "react";
import { apiGet, apiPost, buildListPath, isApiUnauthorized } from "../api";
import { HISTORY_DETAIL_LIMIT } from "../constants";
import type {
  AuditLogRecord,
  HistoryExportRecord,
  HistoryRetentionPolicyRecord,
  HistoryRetentionPolicyRequest,
  HistoryRetentionPruneRequest,
  HistoryRetentionPruneResponse,
} from "../types";

export function useAuditData(apiToken: string, onUnauthorized: () => void) {
  const [audits, setAudits] = useState<AuditLogRecord[]>([]);
  const [auditsTruncated, setAuditsTruncated] = useState(false);
  const [historyRetentionPolicies, setHistoryRetentionPolicies] = useState<HistoryRetentionPolicyRecord[]>([]);
  const [historyPruneResult, setHistoryPruneResult] = useState<HistoryRetentionPruneResponse | null>(null);
  const [historyExport, setHistoryExport] = useState<HistoryExportRecord | null>(null);
  const [auditError, setAuditError] = useState<string | null>(null);
  const [auditLoading, setAuditLoading] = useState(false);
  const [auditEvidenceAvailable, setAuditEvidenceAvailable] = useState(false);
  const auditLoadGeneration = useRef(0);
  const historyExportLoadGeneration = useRef(0);
  const historyPruneMutationGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const handleAuditUnauthorized = useCallback(() => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    onUnauthorized();
    setAudits([]);
    setAuditsTruncated(false);
    setHistoryRetentionPolicies([]);
    setAuditError("Operator login required");
  }, [apiToken, onUnauthorized]);

  const handleAuditError = useCallback(
    (error: unknown, fallback: string) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (isApiUnauthorized(error)) {
        handleAuditUnauthorized();
        return;
      }
      setAuditError(error instanceof Error ? error.message : fallback);
    },
    [apiToken, handleAuditUnauthorized],
  );

  const loadAudits = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = auditLoadGeneration.current + 1;
    auditLoadGeneration.current = generation;
    setAuditLoading(true);
    setAuditError(null);
    try {
      const [auditResult, retentionResult] = await Promise.allSettled([
        apiGet<AuditLogRecord[]>(buildListPath("/api/v1/audit", { limit: HISTORY_DETAIL_LIMIT, sort: "created_at", dir: "desc" }), apiToken),
        apiGet<HistoryRetentionPolicyRecord[]>("/api/v1/history/retention-policies", apiToken),
      ]);
      if (
        auditLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      const results = [auditResult, retentionResult];
      if (
        results.some(
          (result) =>
            result.status === "rejected" &&
            isApiUnauthorized(result.reason),
        )
      ) {
        onUnauthorized();
        setAuditEvidenceAvailable(false);
        setAudits([]);
        setAuditsTruncated(false);
        setHistoryRetentionPolicies([]);
        setAuditError("Operator login required");
        return;
      }
      if (auditResult.status === "fulfilled") {
        setAudits(auditResult.value);
        setAuditsTruncated(auditResult.value.length >= HISTORY_DETAIL_LIMIT);
      }
      setAuditEvidenceAvailable(auditResult.status === "fulfilled");
      if (retentionResult.status === "fulfilled") {
        setHistoryRetentionPolicies(retentionResult.value);
      }
      setAuditError(
        unavailableAuditSources([auditResult, retentionResult]),
      );
    } finally {
      if (
        auditLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setAuditLoading(false);
      }
    }
  }, [apiToken, onUnauthorized]);

  const loadAuditEvent = useCallback(
    async (auditId: string): Promise<AuditLogRecord | null> => {
      const normalizedId = auditId.trim();
      if (!normalizedId || currentApiToken.current !== apiToken) {
        return null;
      }
      try {
        const record = await apiGet<AuditLogRecord>(
          `/api/v1/audit/${encodeURIComponent(normalizedId)}`,
          apiToken,
        );
        if (currentApiToken.current !== apiToken) {
          return null;
        }
        return record;
      } catch (error) {
        if (isApiUnauthorized(error)) {
          handleAuditUnauthorized();
        }
        throw error;
      }
    },
    [apiToken, handleAuditUnauthorized],
  );

  const upsertHistoryRetentionPolicy = useCallback(
    async (request: HistoryRetentionPolicyRequest) => {
      setAuditError(null);
      try {
        await apiPost<HistoryRetentionPolicyRecord>("/api/v1/history/retention-policies", apiToken, request);
        if (currentApiToken.current !== apiToken) {
          return;
        }
        await loadAudits();
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        handleAuditError(error, "History retention policy update failed");
        throw error;
      }
    },
    [apiToken, handleAuditError, loadAudits],
  );

  const pruneHistoryRetention = useCallback(
    async (request: HistoryRetentionPruneRequest) => {
      const operationGeneration =
        historyPruneMutationGeneration.current + 1;
      historyPruneMutationGeneration.current = operationGeneration;
      setAuditError(null);
      try {
        const response = await apiPost<HistoryRetentionPruneResponse>(
          "/api/v1/history/retention-prune",
          apiToken,
          request,
        );
        if (
          currentApiToken.current !== apiToken ||
          historyPruneMutationGeneration.current !== operationGeneration
        ) {
          return response;
        }
        setHistoryPruneResult(response);
        await loadAudits();
        return response;
      } catch (error) {
        if (
          currentApiToken.current !== apiToken ||
          historyPruneMutationGeneration.current !== operationGeneration
        ) {
          throw error;
        }
        handleAuditError(error, "History retention prune failed");
        throw error;
      }
    },
    [apiToken, handleAuditError, loadAudits],
  );

  const loadHistoryExport = useCallback(
    async (
      domains = "audit_logs,system_metric_rollups,telemetry_rollups,telemetry_network_rates,traffic_counter_samples,job_outputs,backup_artifacts,network_observations,topology_history,client_status_history,gateway_sessions",
    ) => {
      if (currentApiToken.current !== apiToken) {
        throw new Error("Operator session changed; retry the history export");
      }
      const generation = historyExportLoadGeneration.current + 1;
      historyExportLoadGeneration.current = generation;
      setAuditError(null);
      try {
        const response = await apiGet<HistoryExportRecord>(
          `/api/v1/history/export?limit=1000&domains=${encodeURIComponent(domains)}`,
          apiToken,
        );
        if (
          currentApiToken.current !== apiToken ||
          historyExportLoadGeneration.current !== generation
        ) {
          return response;
        }
        setHistoryExport(response);
        return response;
      } catch (error) {
        if (
          currentApiToken.current !== apiToken ||
          historyExportLoadGeneration.current !== generation
        ) {
          throw error;
        }
        handleAuditError(error, "History export unavailable");
        throw error;
      }
    },
    [apiToken, handleAuditError],
  );

  const clearAudits = useCallback(() => {
    auditLoadGeneration.current += 1;
    historyExportLoadGeneration.current += 1;
    historyPruneMutationGeneration.current += 1;
    currentApiToken.current = "";
    setAudits([]);
    setAuditsTruncated(false);
    setHistoryRetentionPolicies([]);
    setHistoryPruneResult(null);
    setHistoryExport(null);
    setAuditError(null);
    setAuditLoading(false);
    setAuditEvidenceAvailable(false);
  }, []);

  return {
    auditError,
    auditEvidenceAvailable,
    auditLoading,
    audits,
    auditsTruncated,
    clearAudits,
    historyExport,
    historyPruneResult,
    historyRetentionPolicies,
    loadAudits,
    loadAuditEvent,
    loadHistoryExport,
    pruneHistoryRetention,
    upsertHistoryRetentionPolicy,
  };
}

function unavailableAuditSources(
  results: readonly PromiseSettledResult<unknown>[],
): string | null {
  const labels = ["audit log", "history retention policies"] as const;
  const failures = results.flatMap((result, index) =>
    result.status === "rejected" ? [labels[index]] : [],
  );
  return failures.length > 0
    ? `Some audit sources are unavailable: ${failures.join(", ")}`
    : null;
}
