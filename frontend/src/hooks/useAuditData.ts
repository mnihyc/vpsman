import { useCallback, useState } from "react";
import { apiGet, apiPost, isApiUnauthorized } from "../api";
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
  const [historyRetentionPolicies, setHistoryRetentionPolicies] = useState<HistoryRetentionPolicyRecord[]>([]);
  const [historyPruneResult, setHistoryPruneResult] = useState<HistoryRetentionPruneResponse | null>(null);
  const [historyExport, setHistoryExport] = useState<HistoryExportRecord | null>(null);
  const [auditError, setAuditError] = useState<string | null>(null);
  const [auditLoading, setAuditLoading] = useState(false);

  const handleAuditError = useCallback(
    (error: unknown, fallback: string) => {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setAudits([]);
        setAuditError("Operator login required");
        return;
      }
      setAuditError(error instanceof Error ? error.message : fallback);
    },
    [onUnauthorized],
  );

  const loadAudits = useCallback(async () => {
    setAuditLoading(true);
    setAuditError(null);
    try {
      const [auditRows, retentionRows] = await Promise.all([
        apiGet<AuditLogRecord[]>("/api/v1/audit?limit=1000", apiToken),
        apiGet<HistoryRetentionPolicyRecord[]>("/api/v1/history/retention-policies", apiToken),
      ]);
      setAudits(auditRows);
      setHistoryRetentionPolicies(retentionRows);
    } catch (error) {
      handleAuditError(error, "Audit log unavailable");
    } finally {
      setAuditLoading(false);
    }
  }, [apiToken, handleAuditError]);

  const upsertHistoryRetentionPolicy = useCallback(
    async (request: HistoryRetentionPolicyRequest) => {
      setAuditError(null);
      try {
        await apiPost<HistoryRetentionPolicyRecord>("/api/v1/history/retention-policies", apiToken, request);
        await loadAudits();
      } catch (error) {
        handleAuditError(error, "History retention policy update failed");
      }
    },
    [apiToken, handleAuditError, loadAudits],
  );

  const pruneHistoryRetention = useCallback(
    async (request: HistoryRetentionPruneRequest) => {
      setAuditError(null);
      try {
        const response = await apiPost<HistoryRetentionPruneResponse>(
          "/api/v1/history/retention-prune",
          apiToken,
          request,
        );
        setHistoryPruneResult(response);
        await loadAudits();
      } catch (error) {
        handleAuditError(error, "History retention prune failed");
      }
    },
    [apiToken, handleAuditError, loadAudits],
  );

  const loadHistoryExport = useCallback(
    async (domains = "audit_logs,job_outputs,backup_artifacts,network_observations,topology_history") => {
      setAuditError(null);
      try {
        setHistoryExport(
          await apiGet<HistoryExportRecord>(
            `/api/v1/history/export?limit=1000&domains=${encodeURIComponent(domains)}`,
            apiToken,
          ),
        );
      } catch (error) {
        handleAuditError(error, "History export unavailable");
      }
    },
    [apiToken, handleAuditError],
  );

  return {
    auditError,
    auditLoading,
    audits,
    historyExport,
    historyPruneResult,
    historyRetentionPolicies,
    loadAudits,
    loadHistoryExport,
    pruneHistoryRetention,
    upsertHistoryRetentionPolicy,
  };
}
