import { useCallback, useRef, useState } from "react";
import {
  apiGet,
  apiPost,
  buildListPath,
  isApiUnauthorized,
  LatestReadConsumer,
} from "../api";
import { HISTORY_DETAIL_LIMIT } from "../constants";
import {
  snapshotSourceAvailable,
  snapshotSourceError,
  type SnapshotSource,
} from "../homeSnapshot";
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
  const [historyRetentionPolicies, setHistoryRetentionPolicies] = useState<
    HistoryRetentionPolicyRecord[]
  >([]);
  const [historyPruneResult, setHistoryPruneResult] =
    useState<HistoryRetentionPruneResponse | null>(null);
  const [historyExport, setHistoryExport] =
    useState<HistoryExportRecord | null>(null);
  const [auditError, setAuditError] = useState<string | null>(null);
  const [auditLoading, setAuditLoading] = useState(false);
  const [auditEvidenceAvailable, setAuditEvidenceAvailable] = useState(false);
  // The operation fence owns only the shared loading indicator. Each source
  // has its own revision and consumer so a newer audit-log read cannot cancel
  // or overwrite the retention-policy projection (and vice versa).
  const auditLoadOperationGeneration = useRef(0);
  const auditLogLoadGeneration = useRef(0);
  const retentionPolicyLoadGeneration = useRef(0);
  const auditLogLoadConsumer = useRef(
    new LatestReadConsumer<AuditLogRecord[]>(),
  );
  const retentionPolicyLoadConsumer = useRef(
    new LatestReadConsumer<HistoryRetentionPolicyRecord[]>(),
  );
  const homeAuditHydrationRef = useRef<{
    auditLogGeneration: number;
    operationGeneration: number;
  } | null>(null);
  const auditProjectionFailuresRef = useRef<AuditProjectionFailures>({
    auditLog: null,
    retentionPolicies: null,
  });
  const historyExportLoadGeneration = useRef(0);
  const historyPruneMutationGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const handleAuditUnauthorized = useCallback(() => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    auditLoadOperationGeneration.current += 1;
    auditLogLoadGeneration.current += 1;
    retentionPolicyLoadGeneration.current += 1;
    auditLogLoadConsumer.current.discardPending([]);
    retentionPolicyLoadConsumer.current.discardPending([]);
    homeAuditHydrationRef.current = null;
    auditProjectionFailuresRef.current = {
      auditLog: null,
      retentionPolicies: null,
    };
    currentApiToken.current = "";
    onUnauthorized();
    setAuditEvidenceAvailable(false);
    setAudits([]);
    setAuditsTruncated(false);
    setHistoryRetentionPolicies([]);
    setAuditError("Operator login required");
    setAuditLoading(false);
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

  const loadAudits = useCallback((): Promise<void> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve();
    }
    const operationGeneration = ++auditLoadOperationGeneration.current;
    const auditLogGeneration = ++auditLogLoadGeneration.current;
    const retentionPolicyGeneration =
      ++retentionPolicyLoadGeneration.current;
    auditProjectionFailuresRef.current = {
      auditLog: null,
      retentionPolicies: null,
    };
    setAuditLoading(true);
    setAuditError(null);
    return (async () => {
      try {
        const [auditResult, retentionResult] = await Promise.allSettled([
          auditLogLoadConsumer.current.enqueue(() =>
            apiGet<AuditLogRecord[]>(
              buildListPath("/api/v1/audit", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
          ),
          retentionPolicyLoadConsumer.current.enqueue(() =>
            apiGet<HistoryRetentionPolicyRecord[]>(
              "/api/v1/history/retention-policies",
              apiToken,
            ),
          ),
        ]);
        if (currentApiToken.current !== apiToken) {
          return;
        }
        const auditLogIsCurrent =
          auditLogLoadGeneration.current === auditLogGeneration;
        const retentionPolicyIsCurrent =
          retentionPolicyLoadGeneration.current === retentionPolicyGeneration;
        if (
          (auditLogIsCurrent &&
            auditResult.status === "rejected" &&
            isApiUnauthorized(auditResult.reason)) ||
          (retentionPolicyIsCurrent &&
            retentionResult.status === "rejected" &&
            isApiUnauthorized(retentionResult.reason))
        ) {
          handleAuditUnauthorized();
          return;
        }
        if (auditLogIsCurrent) {
          if (auditResult.status === "fulfilled") {
            setAudits(auditResult.value);
            setAuditsTruncated(
              auditResult.value.length >= HISTORY_DETAIL_LIMIT,
            );
          }
          setAuditEvidenceAvailable(auditResult.status === "fulfilled");
          auditProjectionFailuresRef.current.auditLog =
            auditResult.status === "rejected"
              ? { kind: "aggregate", label: "audit log" }
              : null;
        }
        if (retentionPolicyIsCurrent) {
          if (retentionResult.status === "fulfilled") {
            setHistoryRetentionPolicies(retentionResult.value);
          }
          auditProjectionFailuresRef.current.retentionPolicies =
            retentionResult.status === "rejected"
              ? { kind: "aggregate", label: "history retention policies" }
              : null;
        }
        setAuditError(
          formatAuditProjectionFailures(auditProjectionFailuresRef.current),
        );
      } finally {
        if (
          auditLoadOperationGeneration.current === operationGeneration &&
          currentApiToken.current === apiToken
        ) {
          setAuditLoading(false);
        }
      }
    })();
  }, [apiToken, handleAuditUnauthorized]);

  const loadAuditLogs = useCallback((): Promise<void> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve();
    }
    const operationGeneration = ++auditLoadOperationGeneration.current;
    const auditLogGeneration = ++auditLogLoadGeneration.current;
    auditProjectionFailuresRef.current.auditLog = null;
    setAuditLoading(true);
    setAuditError(
      formatAuditProjectionFailures(auditProjectionFailuresRef.current),
    );
    return (async () => {
      try {
        const records = await auditLogLoadConsumer.current.enqueue(() =>
          apiGet<AuditLogRecord[]>(
            buildListPath("/api/v1/audit", {
              limit: HISTORY_DETAIL_LIMIT,
              sort: "created_at",
              dir: "desc",
            }),
            apiToken,
          ),
        );
        if (
          auditLogLoadGeneration.current !== auditLogGeneration ||
          currentApiToken.current !== apiToken
        ) {
          return;
        }
        setAudits(records);
        setAuditsTruncated(records.length >= HISTORY_DETAIL_LIMIT);
        setAuditEvidenceAvailable(true);
        auditProjectionFailuresRef.current.auditLog = null;
        setAuditError(
          formatAuditProjectionFailures(auditProjectionFailuresRef.current),
        );
      } catch (error) {
        if (
          auditLogLoadGeneration.current !== auditLogGeneration ||
          currentApiToken.current !== apiToken
        ) {
          return;
        }
        setAuditEvidenceAvailable(false);
        if (isApiUnauthorized(error)) {
          handleAuditUnauthorized();
        } else {
          auditProjectionFailuresRef.current.auditLog = {
            kind: "message",
            message:
              error instanceof Error ? error.message : "Audit log unavailable",
          };
          setAuditError(
            formatAuditProjectionFailures(auditProjectionFailuresRef.current),
          );
        }
      } finally {
        if (
          auditLoadOperationGeneration.current === operationGeneration &&
          currentApiToken.current === apiToken
        ) {
          setAuditLoading(false);
        }
      }
    })();
  }, [apiToken, handleAuditUnauthorized]);

  const beginHomeAuditHydration = useCallback(() => {
    const operationGeneration = ++auditLoadOperationGeneration.current;
    const auditLogGeneration = ++auditLogLoadGeneration.current;
    homeAuditHydrationRef.current = {
      auditLogGeneration,
      operationGeneration,
    };
    auditProjectionFailuresRef.current.auditLog = null;
    setAuditError(
      formatAuditProjectionFailures(auditProjectionFailuresRef.current),
    );
    setAuditLoading(true);
    return operationGeneration;
  }, []);

  const hydrateHomeAudit = useCallback(
    (generation: number, source: SnapshotSource<AuditLogRecord[]>) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      const hydration = homeAuditHydrationRef.current;
      if (
        hydration?.operationGeneration !== generation ||
        auditLogLoadGeneration.current !== hydration.auditLogGeneration
      ) {
        return;
      }
      if (snapshotSourceAvailable(source)) {
        setAudits(source.data);
        setAuditsTruncated(source.data.length >= HISTORY_DETAIL_LIMIT);
      }
      setAuditEvidenceAvailable(snapshotSourceAvailable(source));
      const sourceFailure = snapshotSourceError("Audit log", source);
      auditProjectionFailuresRef.current.auditLog = sourceFailure
        ? { kind: "message", message: sourceFailure }
        : null;
      setAuditError(
        formatAuditProjectionFailures(auditProjectionFailuresRef.current),
      );
      if (auditLoadOperationGeneration.current === generation) {
        setAuditLoading(false);
      }
    },
    [apiToken],
  );

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
        const policy = await apiPost<HistoryRetentionPolicyRecord>(
          "/api/v1/history/retention-policies",
          apiToken,
          request,
        );
        if (currentApiToken.current !== apiToken) {
          return;
        }
        retentionPolicyLoadGeneration.current += 1;
        auditProjectionFailuresRef.current.retentionPolicies = null;
        setHistoryRetentionPolicies((current) =>
          current.some((stored) => stored.domain === policy.domain)
            ? current.map((stored) =>
                stored.domain === policy.domain ? policy : stored,
              )
            : [policy, ...current],
        );
        setAuditError(
          formatAuditProjectionFailures(auditProjectionFailuresRef.current),
        );
        await loadAuditLogs();
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        handleAuditError(error, "History retention policy update failed");
        throw error;
      }
    },
    [apiToken, handleAuditError, loadAuditLogs],
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
        await loadAuditLogs();
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
    [apiToken, handleAuditError, loadAuditLogs],
  );

  const loadHistoryExport = useCallback(
    async (
      domains = "audit_logs,system_metric_rollups,telemetry_samples,telemetry_rollups,telemetry_network_rates,telemetry_ping_rollups,traffic_counter_samples,job_outputs,backup_artifacts,network_observations,topology_history,client_status_history,gateway_sessions",
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
    auditLoadOperationGeneration.current += 1;
    auditLogLoadGeneration.current += 1;
    retentionPolicyLoadGeneration.current += 1;
    auditLogLoadConsumer.current.discardPending([]);
    retentionPolicyLoadConsumer.current.discardPending([]);
    homeAuditHydrationRef.current = null;
    auditProjectionFailuresRef.current = {
      auditLog: null,
      retentionPolicies: null,
    };
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
    beginHomeAuditHydration,
    auditEvidenceAvailable,
    auditLoading,
    audits,
    auditsTruncated,
    clearAudits,
    historyExport,
    historyPruneResult,
    historyRetentionPolicies,
    hydrateHomeAudit,
    loadAuditLogs,
    loadAudits,
    loadAuditEvent,
    loadHistoryExport,
    pruneHistoryRetention,
    upsertHistoryRetentionPolicy,
  };
}

type AuditProjectionFailure =
  | { kind: "aggregate"; label: string }
  | { kind: "message"; message: string };

type AuditProjectionFailures = {
  auditLog: AuditProjectionFailure | null;
  retentionPolicies: AuditProjectionFailure | null;
};

function formatAuditProjectionFailures(
  failures: AuditProjectionFailures,
): string | null {
  const current = [failures.auditLog, failures.retentionPolicies].filter(
    (failure): failure is AuditProjectionFailure => failure !== null,
  );
  if (current.length === 0) {
    return null;
  }
  const aggregateLabels = current.flatMap((failure) =>
    failure.kind === "aggregate" ? [failure.label] : [],
  );
  const messages = current.flatMap((failure) =>
    failure.kind === "message" ? [failure.message] : [],
  );
  if (aggregateLabels.length > 0) {
    messages.push(
      `Some audit sources are unavailable: ${aggregateLabels.join(", ")}`,
    );
  }
  return messages.join("; ");
}
