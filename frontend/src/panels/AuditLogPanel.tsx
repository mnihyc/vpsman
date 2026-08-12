import {
  ArrowLeft,
  ClipboardList,
  Download,
  ExternalLink,
  Filter,
  RefreshCw,
  RotateCcw,
  Scissors,
  ShieldCheck,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  auditActionLabel as sharedAuditActionLabel,
  auditEvidenceSearchText,
  auditMissingFieldLabel,
  auditSessionSearchText,
  presentAudit,
  type AuditEvidenceReference,
} from "../auditPresentation";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import type {
  AuditLogRecord,
  HistoryExportRecord,
  HistoryRetentionPolicyRecord,
  HistoryRetentionPolicyRequest,
  HistoryRetentionPruneRequest,
  HistoryRetentionPruneResponse,
  JsonValue,
} from "../types";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  shortHash,
} from "../utils";
import { useHistoryEntryState } from "../historyEntryState";
import { scrollIntoViewWithMotion } from "../motion";

type AuditFilterState = {
  action: string;
  actor: string;
  from: string;
  ip: string;
  privilege: string;
  resource: string;
  result: string;
  session: string;
  to: string;
};

const EMPTY_AUDIT_FILTERS: AuditFilterState = {
  action: "",
  actor: "",
  from: "",
  ip: "",
  privilege: "",
  resource: "",
  result: "",
  session: "",
  to: "",
};

export function AuditLogPanel({
  activeSubpage,
  audits,
  auditsTruncated,
  error,
  historyExport,
  historyPruneResult,
  historyRetentionPolicies,
  loading,
  onExportHistory,
  onCloseAuditEvent,
  onLoadAuditEvent,
  onOpenAuditEvent,
  onOpenEvidence,
  onPruneHistoryRetention,
  onRefresh,
  onUpsertHistoryRetentionPolicy,
}: {
  activeSubpage: string;
  audits: AuditLogRecord[];
  auditsTruncated: boolean;
  error: string | null;
  historyExport: HistoryExportRecord | null;
  historyPruneResult: HistoryRetentionPruneResponse | null;
  historyRetentionPolicies: HistoryRetentionPolicyRecord[];
  loading: boolean;
  onExportHistory: (domains?: string) => Promise<HistoryExportRecord>;
  onCloseAuditEvent: () => void;
  onLoadAuditEvent: (auditId: string) => Promise<AuditLogRecord | null>;
  onOpenAuditEvent: (auditId: string) => void;
  onOpenEvidence: (reference: AuditEvidenceReference) => void;
  onPruneHistoryRetention: (
    request: HistoryRetentionPruneRequest,
  ) => Promise<HistoryRetentionPruneResponse>;
  onRefresh: () => void;
  onUpsertHistoryRetentionPolicy: (
    request: HistoryRetentionPolicyRequest,
  ) => Promise<void>;
}) {
  const auditSubpage = activeSubpage === "retention" ? "retention" : "events";
  const auditFeedbackMessage =
    error ?? (loading ? "Refreshing audit records" : null);
  const selectedAuditId = activeSubpage.startsWith("events:id:")
    ? activeSubpage.slice("events:id:".length).trim()
    : null;
  const selectedAuditFromList = selectedAuditId
    ? (audits.find((audit) => audit.id === selectedAuditId) ?? null)
    : null;
  const [routedAudit, setRoutedAudit] = useState<AuditLogRecord | null>(null);
  const [routedAuditError, setRoutedAuditError] = useState<string | null>(null);
  const [routedAuditLoading, setRoutedAuditLoading] = useState(false);
  const routedAuditLoadGeneration = useRef(0);
  const auditFeedbackRef = useRef<HTMLDivElement | null>(null);
  const retentionFeedbackRef = useRef<HTMLDivElement | null>(null);
  const selectedAudit =
    selectedAuditFromList ??
    (routedAudit?.id === selectedAuditId ? routedAudit : null);
  const [selectedDomain, setSelectedDomain] = useState("audit_logs");
  const selectedPolicy = useMemo(
    () =>
      historyRetentionPolicies.find(
        (policy) => policy.domain === selectedDomain,
      ) ?? historyRetentionPolicies[0],
    [historyRetentionPolicies, selectedDomain],
  );
  const minimumRetentionDays =
    selectedPolicy?.domain === "traffic_counter_samples" ? 32 : 1;
  const selectedUsesTieredHorizon = isTieredMonitoringDomain(
    selectedPolicy?.domain,
  );
  const [retentionDays, setRetentionDays] = useState("365");
  const [pruneLimit, setPruneLimit] = useState("1000");
  const [metadataOnly, setMetadataOnly] = useState(false);
  const [exportEnabled, setExportEnabled] = useState(true);
  const [pruneSnapshot, setPruneSnapshot] = useState<{
    effectLabel: string;
    objectCount: number;
    previewHash: string | null;
    request: HistoryRetentionPruneRequest;
    reviewedRows: number;
  } | null>(null);
  const [pruneConfirmationOpen, setPruneConfirmationOpen] = useState(false);
  const [retentionStatus, setRetentionStatus] = useState<string | null>(null);
  const [retentionStatusTone, setRetentionStatusTone] =
    useState<ActionFeedbackTone>("info");
  const [auditFilters, setAuditFilters] =
    useHistoryEntryState<AuditFilterState>(
      "audit.events.filters",
      EMPTY_AUDIT_FILTERS,
    );

  useEffect(() => {
    if (!selectedPolicy) {
      return;
    }
    setSelectedDomain(selectedPolicy.domain);
    setRetentionDays(String(selectedPolicy.retention_days));
    setPruneLimit(String(selectedPolicy.prune_limit));
    setMetadataOnly(selectedPolicy.metadata_only);
    setExportEnabled(selectedPolicy.export_enabled);
  }, [selectedPolicy]);

  useEffect(() => {
    const generation = routedAuditLoadGeneration.current + 1;
    routedAuditLoadGeneration.current = generation;
    if (!selectedAuditId || selectedAuditFromList) {
      setRoutedAudit(null);
      setRoutedAuditError(null);
      setRoutedAuditLoading(false);
      return;
    }
    let active = true;
    setRoutedAudit(null);
    setRoutedAuditError(null);
    setRoutedAuditLoading(true);
    void onLoadAuditEvent(selectedAuditId)
      .then((record) => {
        if (active && routedAuditLoadGeneration.current === generation) {
          setRoutedAudit(record);
        }
      })
      .catch((loadError: unknown) => {
        if (active && routedAuditLoadGeneration.current === generation) {
          setRoutedAuditError(
            loadError instanceof Error
              ? loadError.message
              : "Audit event lookup failed",
          );
        }
      })
      .finally(() => {
        if (active && routedAuditLoadGeneration.current === generation) {
          setRoutedAuditLoading(false);
        }
      });
    return () => {
      active = false;
    };
  }, [onLoadAuditEvent, selectedAuditFromList, selectedAuditId]);

  const enabledPolicyCount = useMemo(
    () => historyRetentionPolicies.filter((policy) => policy.enabled).length,
    [historyRetentionPolicies],
  );
  const exportPolicyCount = useMemo(
    () =>
      historyRetentionPolicies.filter((policy) => policy.export_enabled).length,
    [historyRetentionPolicies],
  );
  const hasAuditFilters = useMemo(
    () => Object.values(auditFilters).some((value) => value.trim().length > 0),
    [auditFilters],
  );
  const filteredAudits = useMemo(
    () => audits.filter((audit) => auditMatchesFilters(audit, auditFilters)),
    [audits, auditFilters],
  );
  const auditActors = useMemo(
    () =>
      Array.from(
        new Set(
          audits
            .map((audit) => auditActor(audit))
            .filter((value): value is string => Boolean(value)),
        ),
      ),
    [audits],
  );
  const latestVisibleAudit = useMemo(
    () => latestAuditRecord(filteredAudits),
    [filteredAudits],
  );
  const relatedAuditCount = useMemo(
    () =>
      audits.filter(
        (audit) => presentAudit(audit).evidenceReferences.length > 0,
      ).length,
    [audits],
  );
  const activeFilterCount = useMemo(
    () =>
      Object.values(auditFilters).filter((value) => value.trim().length > 0)
        .length,
    [auditFilters],
  );
  const lastAuditTime = latestVisibleAudit?.created_at
    ? formatFullTime(latestVisibleAudit.created_at)
    : "No visible events";
  const auditColumns = useMemo<ConsoleDataGridColumn<AuditLogRecord>[]>(
    () => [
      {
        id: "time",
        header: "Time",
        size: 170,
        minSize: 130,
        sortValue: (audit) => audit.created_at,
        searchValue: (audit) =>
          `${audit.created_at} ${formatFullTime(audit.created_at)}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong title={formatFullTime(audit.created_at)}>
              {formatCompactTime(audit.created_at)}
            </strong>
            <small>{formatFullTime(audit.created_at)}</small>
          </span>
        ),
      },
      {
        id: "operator",
        header: "Actor",
        size: 170,
        minSize: 150,
        sortValue: (audit) => auditActor(audit),
        searchValue: (audit) =>
          `${auditActor(audit)} ${audit.actor_id ?? ""} ${presentAudit(audit).actorDetail}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{auditActor(audit)}</strong>
            <small title={audit.actor_id ?? undefined}>
              {auditActorDetail(audit)}
            </small>
          </span>
        ),
      },
      {
        id: "action",
        header: "Action",
        mobilePrimary: true,
        size: 190,
        minSize: 150,
        sortValue: (audit) => auditActionLabel(audit.action),
        searchValue: (audit) =>
          `${audit.action} ${auditActionLabel(audit.action)} ${audit.id}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{auditActionLabel(audit.action)}</strong>
            <small>{auditActionDetail(audit)}</small>
          </span>
        ),
      },
      {
        id: "target",
        header: "Target",
        size: 210,
        minSize: 150,
        sortValue: (audit) => audit.target,
        searchValue: (audit) =>
          `${audit.target} ${auditTargetLabel(audit)} ${jsonText(audit.metadata)}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{auditTargetLabel(audit)}</strong>
            <small title={audit.target}>{auditTargetDetail(audit)}</small>
          </span>
        ),
      },
      {
        id: "result",
        header: "Outcome",
        mobileState: true,
        size: 140,
        minSize: 110,
        sortValue: (audit) => auditResultLabel(audit),
        searchValue: (audit) => auditFilterText(audit, "result"),
        cell: (audit) => (
          <ConsoleStatusBadge tone={auditResultTone(audit)}>
            {auditResultLabel(audit)}
          </ConsoleStatusBadge>
        ),
      },
      {
        id: "related",
        header: "Evidence",
        size: 210,
        minSize: 150,
        sortValue: (audit) => auditRelatedEvidenceLabel(audit),
        searchValue: (audit) => auditRelatedEvidenceSearch(audit),
        cell: (audit) => (
          <span className="historyPrimary">
            <strong title={auditRelatedEvidenceFullDetail(audit)}>
              {auditRelatedEvidenceLabel(audit)}
            </strong>
            <small title={auditRelatedEvidenceFullDetail(audit)}>
              {auditRelatedEvidenceDetail(audit)}
            </small>
          </span>
        ),
      },
    ],
    [],
  );

  function clearPruneConfirmation() {
    setPruneSnapshot(null);
    setPruneConfirmationOpen(false);
  }

  function clearRetentionReviewFeedback() {
    clearPruneConfirmation();
    setRetentionStatus(null);
  }

  useEffect(() => {
    if (!auditFeedbackMessage || auditSubpage !== "events") return;
    const frame = window.requestAnimationFrame(() => {
      if (auditFeedbackRef.current) {
        scrollIntoViewWithMotion(auditFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [auditFeedbackMessage, auditSubpage]);

  useEffect(() => {
    if (!(error ?? retentionStatus) || auditSubpage !== "retention") return;
    const frame = window.requestAnimationFrame(() => {
      if (retentionFeedbackRef.current) {
        scrollIntoViewWithMotion(retentionFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [auditSubpage, error, retentionStatus]);

  const submitPolicy = async () => {
    if (
      minimumRetentionDays > 1 &&
      Number(retentionDays) < minimumRetentionDays
    ) {
      setRetentionStatus(
        `Traffic accounting counters require at least ${minimumRetentionDays} retention days to preserve a complete monthly cycle`,
      );
      setRetentionStatusTone("danger");
      return;
    }
    setRetentionStatus("Saving history retention policy");
    setRetentionStatusTone("progress");
    try {
      await onUpsertHistoryRetentionPolicy({
        domain: selectedPolicy?.domain ?? selectedDomain,
        retention_days: Number(retentionDays),
        prune_limit: Number(pruneLimit),
        metadata_only: metadataOnly,
        export_enabled: exportEnabled,
        confirmed: true,
      });
      setRetentionStatus(`Saved ${selectedDomainName} retention policy`);
      setRetentionStatusTone("success");
    } catch (actionError) {
      setRetentionStatus(
        actionError instanceof Error
          ? actionError.message
          : "History retention policy update failed",
      );
      setRetentionStatusTone("danger");
    }
  };

  const pruneRequest = (dryRun: boolean): HistoryRetentionPruneRequest => ({
    domain: selectedPolicy?.domain ?? selectedDomain,
    dry_run: dryRun,
    metadata_only: metadataOnly,
    confirmed: !dryRun,
  });

  const previewPrune = async () => {
    setRetentionStatus(`Previewing ${selectedDomainName} cleanup`);
    setRetentionStatusTone("progress");
    try {
      const preview = await onPruneHistoryRetention({
        ...pruneRequest(true),
        confirmed: false,
        preview_hash: null,
      });
      const reviewedRows = totalMatchedRows(preview);
      const objectCount = totalObjectKeys(preview);
      const request = {
        ...pruneRequest(false),
        confirmed: true,
        preview_hash: preview.preview_hash ?? null,
      };
      setPruneSnapshot({
        effectLabel: formatPruneEffect(
          preview,
          request.metadata_only ?? metadataOnly,
        ),
        objectCount,
        previewHash: preview.preview_hash ?? null,
        request,
        reviewedRows,
      });
      setPruneConfirmationOpen(false);
      setRetentionStatus(
        reviewedRows > 0
          ? `Cleanup preview matched ${reviewedRows} row${reviewedRows === 1 ? "" : "s"}`
          : "Cleanup preview matched no rows; deletion is not needed",
      );
      setRetentionStatusTone(reviewedRows > 0 ? "warning" : "success");
    } catch (actionError) {
      setRetentionStatus(
        actionError instanceof Error
          ? actionError.message
          : "History cleanup preview failed",
      );
      setRetentionStatusTone("danger");
    }
  };

  const confirmPrune = async () => {
    if (!pruneSnapshot) {
      return;
    }
    setRetentionStatus(
      `Deleting ${pruneSnapshot.reviewedRows} reviewed history row${pruneSnapshot.reviewedRows === 1 ? "" : "s"}`,
    );
    setRetentionStatusTone("progress");
    try {
      await onPruneHistoryRetention(pruneSnapshot.request);
      setRetentionStatus(`Deleted reviewed ${selectedDomainName} history rows`);
      setRetentionStatusTone("success");
      clearPruneConfirmation();
    } catch (actionError) {
      setRetentionStatus(
        actionError instanceof Error
          ? actionError.message
          : "History cleanup failed",
      );
      setRetentionStatusTone("danger");
    }
  };

  const exportSelectedHistory = async () => {
    setRetentionStatus(`Exporting ${selectedDomainName} history`);
    setRetentionStatusTone("progress");
    try {
      const exported = await onExportHistory(selectedDomainLabel);
      const blob = new Blob([JSON.stringify(exported, null, 2)], {
        type: "application/json",
      });
      const href = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = href;
      anchor.download = `vpsman-history-${selectedDomainLabel.replace(/[^a-z0-9_-]+/gi, "-")}.json`;
      anchor.click();
      URL.revokeObjectURL(href);
      setRetentionStatus(`Downloaded ${selectedDomainName} history export`);
      setRetentionStatusTone("success");
    } catch (actionError) {
      setRetentionStatus(
        actionError instanceof Error
          ? actionError.message
          : "History export failed",
      );
      setRetentionStatusTone("danger");
    }
  };

  const updateAuditFilter = (key: keyof AuditFilterState, value: string) => {
    setAuditFilters((current) => ({ ...current, [key]: value }));
  };

  const selectedDomainLabel = selectedPolicy?.domain ?? selectedDomain;
  const selectedDomainName = historyDomainLabel(selectedDomainLabel);
  const selectedDomainDescription =
    historyDomainDescription(selectedDomainLabel);
  const currentRecordLabel =
    selectedDomainLabel === "audit_logs"
      ? `${audits.length} audit records`
      : "Not reported";
  const currentRecordDetail =
    selectedDomainLabel === "audit_logs"
      ? audits.length === 0
        ? "No audit rows are loaded; privileged activity is not evidenced in the table."
        : "Visible audit rows only; the full retained total is unavailable."
      : "Current row count is unavailable for this history domain.";
  const cleanupReviewLabel = pruneSnapshot
    ? `${pruneSnapshot.reviewedRows} matched rows / ${pruneSnapshot.objectCount} objects`
    : historyPruneResult
      ? `${totalMatchedRows(historyPruneResult)} matched rows / ${totalObjectKeys(historyPruneResult)} objects`
      : "Preview required";
  const cleanupEffectLabel =
    pruneSnapshot?.effectLabel ??
    (historyPruneResult
      ? formatPruneEffect(
          historyPruneResult,
          historyPruneResult.metadata_only_requested ?? metadataOnly,
        )
      : "Preview selected domain before delete.");
  const policyUpdatedLabel = selectedPolicy?.updated_at
    ? formatFullTime(selectedPolicy.updated_at)
    : "Not reported";
  const complianceWarning =
    selectedDomainLabel === "audit_logs" && audits.length === 0
      ? "No audit events are visible for privileged control-plane workflows."
      : "Compliance-grade record totals and storage size are unavailable.";
  const retryRoutedAudit = async () => {
    if (!selectedAuditId) return;
    const generation = routedAuditLoadGeneration.current + 1;
    routedAuditLoadGeneration.current = generation;
    setRoutedAuditLoading(true);
    setRoutedAuditError(null);
    try {
      const record = await onLoadAuditEvent(selectedAuditId);
      if (routedAuditLoadGeneration.current === generation) {
        setRoutedAudit(record);
      }
    } catch (loadError) {
      if (routedAuditLoadGeneration.current === generation) {
        setRoutedAuditError(
          loadError instanceof Error
            ? loadError.message
            : "Audit event lookup failed",
        );
      }
    } finally {
      if (routedAuditLoadGeneration.current === generation) {
        setRoutedAuditLoading(false);
      }
    }
  };

  return (
    <section className="workspace singleColumn">
      {auditSubpage === "events" && selectedAuditId && (
        <div className="fleetPanel auditEventRoutePanel">
          <div className="sectionHeader">
            <div>
              <h2>Audit event</h2>
              <span title={selectedAuditId}>Event ID {selectedAuditId}</span>
            </div>
            <button
              className="secondaryAction compactAction"
              onClick={onCloseAuditEvent}
              type="button"
            >
              <ArrowLeft size={15} />
              Audit events
            </button>
          </div>
          {selectedAudit ? (
            <AuditEventDetailPanel
              audit={selectedAudit}
              onOpenEvidence={onOpenEvidence}
            />
          ) : routedAuditLoading || loading ? (
            <div className="emptyState" aria-live="polite">
              <ClipboardList size={22} />
              <strong>Loading audit event</strong>
              <span>Looking up the exact event ID.</span>
            </div>
          ) : (
            <div className="emptyState">
              <ClipboardList size={22} />
              <strong>Audit event is unavailable</strong>
              <span>
                {routedAuditError ??
                  "The exact event ID was not returned. It may have been pruned."}
              </span>
              <button
                className="secondaryAction compactAction"
                onClick={() => void retryRoutedAudit()}
                type="button"
              >
                Refresh event
              </button>
            </div>
          )}
        </div>
      )}
      {auditSubpage === "events" && (
        <div className="fleetPanel" hidden={Boolean(selectedAuditId)}>
          <div className="sectionHeader">
            <div>
              <h2>Audit log</h2>
              <span>Operator and control-plane events</span>
            </div>
          </div>
          <ActionFeedback
            className="localActionFeedback"
            message={auditFeedbackMessage}
            ref={auditFeedbackRef}
            tone={error ? "danger" : "progress"}
          />
          <div className="auditEventSummary" aria-label="Audit event summary">
            <div
              className="auditEventMetric"
              title="Count after applying the current audit filters; an API result limit keeps a truncated total as a lower bound."
            >
              <span>Visible events</span>
              <strong>
                {hasAuditFilters
                  ? `${filteredAudits.length} / ${auditsTruncated ? "≥" : ""}${audits.length}`
                  : `${auditsTruncated ? "≥" : ""}${audits.length}`}
              </strong>
              <p>
                {hasAuditFilters
                  ? `${activeFilterCount} active filters${auditsTruncated ? "; more matches may exist" : ""}`
                  : auditsTruncated
                    ? "All loaded events; more may exist"
                    : "All returned events"}
              </p>
            </div>
            <div
              className="auditEventMetric"
              title={
                latestVisibleAudit
                  ? `Latest visible audit event: ${formatFullTime(latestVisibleAudit.created_at)}.`
                  : "No audit events match the current filters."
              }
            >
              <span>Latest visible</span>
              <strong>
                {latestVisibleAudit
                  ? formatCompactTime(latestVisibleAudit.created_at)
                  : "No events"}
              </strong>
              <p>{lastAuditTime}</p>
            </div>
            <div
              className="auditEventMetric"
              title="Links are derived from audit metadata and open the corresponding job, terminal, session, or schedule evidence."
            >
              <span>Related evidence</span>
              <strong>{relatedAuditCount} linked</strong>
              <p>Job, terminal, session, or schedule references in metadata.</p>
            </div>
            <div
              className="auditEventMetric"
              title="Distinct non-empty actor identifiers in the loaded audit event set."
            >
              <span>Known actors</span>
              <strong>{auditActors.length || "None"}</strong>
              <p>
                {auditActors.length > 0
                  ? auditActors.slice(0, 3).join(", ")
                  : "No actor values available."}
              </p>
            </div>
          </div>
          <div className="auditFilterBar" aria-label="Audit event filters">
            <div className="auditFilterIntro">
              <Filter size={16} />
              <span>
                Filter audit evidence by actor, action, resource, outcome, time,
                source IP, session, and privilege scope.
              </span>
            </div>
            <label>
              <span>Actor</span>
              <input
                aria-label="Audit actor filter"
                placeholder="operator or actor ID"
                value={auditFilters.actor}
                onChange={(event) =>
                  updateAuditFilter("actor", event.target.value)
                }
              />
            </label>
            <label>
              <span>Action</span>
              <input
                aria-label="Audit action filter"
                placeholder="login, dispatch"
                value={auditFilters.action}
                onChange={(event) =>
                  updateAuditFilter("action", event.target.value)
                }
              />
            </label>
            <label>
              <span>Resource</span>
              <input
                aria-label="Audit resource filter"
                placeholder="target or object"
                value={auditFilters.resource}
                onChange={(event) =>
                  updateAuditFilter("resource", event.target.value)
                }
              />
            </label>
            <label>
              <span>Outcome</span>
              <input
                aria-label="Audit result filter"
                placeholder="allowed, failed"
                value={auditFilters.result}
                onChange={(event) =>
                  updateAuditFilter("result", event.target.value)
                }
              />
            </label>
            <label>
              <span>IP</span>
              <input
                aria-label="Audit IP filter"
                placeholder="source IP"
                value={auditFilters.ip}
                onChange={(event) =>
                  updateAuditFilter("ip", event.target.value)
                }
              />
            </label>
            <label>
              <span>Session</span>
              <input
                aria-label="Audit session filter"
                placeholder="session ID"
                value={auditFilters.session}
                onChange={(event) =>
                  updateAuditFilter("session", event.target.value)
                }
              />
            </label>
            <label>
              <span>Privilege scope</span>
              <input
                aria-label="Audit privilege scope filter"
                placeholder="audit:read"
                value={auditFilters.privilege}
                onChange={(event) =>
                  updateAuditFilter("privilege", event.target.value)
                }
              />
            </label>
            <label>
              <span>From</span>
              <input
                aria-label="Audit from date"
                type="date"
                value={auditFilters.from}
                onChange={(event) =>
                  updateAuditFilter("from", event.target.value)
                }
              />
            </label>
            <label>
              <span>To</span>
              <input
                aria-label="Audit to date"
                type="date"
                value={auditFilters.to}
                onChange={(event) =>
                  updateAuditFilter("to", event.target.value)
                }
              />
            </label>
            <div className="auditFilterActions">
              <button
                className="secondaryAction"
                data-tooltip-disabled-reason="No audit filters are active."
                disabled={!hasAuditFilters}
                onClick={() => setAuditFilters(EMPTY_AUDIT_FILTERS)}
                type="button"
              >
                <RotateCcw size={16} />
                Clear
              </button>
            </div>
          </div>
          <ConsoleDataGrid
            actions={[
              {
                label: "Copy audit IDs",
                onSelect: (rows) =>
                  void copyText(rows.map((audit) => audit.id).join("\n")),
              },
              {
                label: "Copy command hashes",
                onSelect: (rows) =>
                  void copyText(
                    rows
                      .map((audit) => audit.command_hash)
                      .filter(Boolean)
                      .join("\n"),
                  ),
              },
            ]}
            columns={auditColumns}
            defaultPageSize={12}
            empty={
              <div className="emptyState">
                <ClipboardList size={22} />
                <strong>
                  {hasAuditFilters
                    ? "No matching audit records"
                    : "No audit records returned"}
                </strong>
                <span>
                  {hasAuditFilters
                    ? auditsTruncated
                      ? "Clear filters or broaden the time window; more records may exist outside the loaded audit page."
                      : "Clear filters or broaden the time window to inspect available events."
                    : "Expected login, unlock, dispatch, file, key, backup, topology, and system events are not evidenced by the API response."}
                </span>
              </div>
            }
            getRowId={(audit) => audit.id}
            itemLabel="records"
            expandOnRowClick
            renderExpandedRow={(audit) => (
              <AuditEventDetailPanel
                audit={audit}
                onOpenDedicated={() => onOpenAuditEvent(audit.id)}
                onOpenEvidence={onOpenEvidence}
              />
            )}
            rowActions={[
              {
                expandRow: true,
                label: "Details",
                onSelect: () => undefined,
              },
            ]}
            rows={filteredAudits}
            rowsTruncated={auditsTruncated}
            singleExpandedRow
            storageKey="vpsman.grid.audit.events"
            title="Audit records"
            toolbarActions={
              <button
                className="secondaryAction compactAction"
                data-tooltip-disabled-reason="Audit records are already refreshing."
                disabled={loading}
                onClick={onRefresh}
                type="button"
              >
                <RefreshCw size={15} />
                Refresh
              </button>
            }
          />
        </div>
      )}
      {auditSubpage === "retention" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>History retention</h2>
              <span>
                Domain policy, export, and cleanup for retained control-plane
                history
              </span>
            </div>
            <button
              className="secondaryAction"
              data-tooltip-disabled-reason="History-retention data is already refreshing."
              disabled={loading}
              onClick={onRefresh}
              type="button"
            >
              Refresh
            </button>
          </div>
          <ActionFeedback
            className="localActionFeedback"
            message={error ?? retentionStatus}
            ref={retentionFeedbackRef}
            tone={error ? "danger" : retentionStatusTone}
          />
          <p className="retentionPanelNote">
            <strong>Monitoring lifecycle.</strong> Accepted resource, network,
            and Ping samples remain exact for 7 days. Their retained history,
            system metrics, and automatic tunnel reachability use fixed age
            tiers: 1m through 2d, 5m through 8d, 30m through 31d, 1h through
            91d, 3h through 181d, 6h through 366d, then 1d. Missing fine detail
            is not fabricated. Traffic counters remain exact through 32d, then
            use 1h/3h/6h/1d transition tiers. For tiered domains, Retention days
            is the final history horizon, not the exact-row duration.
          </p>
          <div
            className="retentionSummaryStrip"
            aria-label="History retention summary"
          >
            <div>
              <span>Policy domains</span>
              <strong>
                {enabledPolicyCount} enabled / {historyRetentionPolicies.length}
              </strong>
            </div>
            <div>
              <span>Export enabled</span>
              <strong>{exportPolicyCount} domains</strong>
            </div>
            <div>
              <span>Selected domain</span>
              <strong>{selectedDomainName}</strong>
            </div>
            <div>
              <span>Last export</span>
              <strong>
                {historyExport
                  ? `${historyExport.domains.length} domain${historyExport.domains.length === 1 ? "" : "s"}`
                  : "Not exported"}
              </strong>
            </div>
            <div>
              <span>Cleanup review</span>
              <strong>{cleanupReviewLabel}</strong>
            </div>
          </div>

          <section
            className="retentionPolicyTable"
            aria-label="History retention policy table"
          >
            <div className="retentionPolicyHeader" aria-hidden="true">
              <span>Domain</span>
              <span>Retention days</span>
              <span>Metadata only</span>
              <span>Export enabled</span>
            </div>
            {historyRetentionPolicies.map((policy) => {
              const selected = policy.domain === selectedDomainLabel;
              return (
                <button
                  aria-pressed={selected}
                  className={`retentionPolicyRow ${selected ? "selected" : ""}`}
                  key={policy.domain}
                  onClick={() => {
                    setSelectedDomain(policy.domain);
                    clearRetentionReviewFeedback();
                  }}
                  type="button"
                >
                  <span className="retentionDomainCell">
                    <strong>{historyDomainLabel(policy.domain)}</strong>
                    <small>{historyDomainDescription(policy.domain)}</small>
                  </span>
                  <span className="retentionPolicyValue">
                    <small>
                      {isTieredMonitoringDomain(policy.domain)
                        ? "Final horizon"
                        : "Retention days"}
                    </small>
                    <strong>{policy.retention_days} days</strong>
                  </span>
                  <span className="retentionPolicyValue">
                    <small>Metadata only</small>
                    <strong>{policy.metadata_only ? "Yes" : "No"}</strong>
                  </span>
                  <span className="retentionPolicyValue">
                    <small>Export enabled</small>
                    <strong>{policy.export_enabled ? "Yes" : "No"}</strong>
                  </span>
                </button>
              );
            })}
          </section>

          <div className="retentionWorkflowGrid">
            <section
              className="retentionWorkflowPanel"
              aria-label="Selected retention domain editor"
            >
              <div className="retentionWorkflowHeader">
                <span>
                  <strong>{selectedDomainName}</strong>
                  <small>{selectedDomainDescription}</small>
                </span>
              </div>
              <label>
                <span>Domain</span>
                <select
                  value={selectedPolicy?.domain ?? selectedDomain}
                  onChange={(event) => {
                    setSelectedDomain(event.target.value);
                    clearRetentionReviewFeedback();
                  }}
                >
                  {historyRetentionPolicies.map((policy) => (
                    <option key={policy.domain} value={policy.domain}>
                      {historyDomainLabel(policy.domain)}
                    </option>
                  ))}
                </select>
              </label>
              <div className="retentionFieldGrid">
                <label>
                  <span>
                    {selectedUsesTieredHorizon
                      ? "Final retention horizon"
                      : "Retention days"}
                  </span>
                  <input
                    min={minimumRetentionDays}
                    max={3650}
                    type="number"
                    value={retentionDays}
                    onChange={(event) => {
                      setRetentionDays(event.target.value);
                      clearRetentionReviewFeedback();
                    }}
                  />
                  {minimumRetentionDays > 1 && (
                    <small>
                      Minimum {minimumRetentionDays} days preserves the active
                      monthly cycle.
                    </small>
                  )}
                  {selectedUsesTieredHorizon && (
                    <small>
                      Exact-row duration and intermediate tier cutovers are
                      fixed; this value controls the last retained day.
                    </small>
                  )}
                </label>
                <label>
                  <span>Prune limit</span>
                  <input
                    min={1}
                    max={100000}
                    type="number"
                    value={pruneLimit}
                    onChange={(event) => {
                      setPruneLimit(event.target.value);
                      clearRetentionReviewFeedback();
                    }}
                  />
                  <small>
                    Maximum retained-history rows removed per cleanup pass.
                  </small>
                </label>
              </div>
              <label className="checkControl">
                <input
                  checked={metadataOnly}
                  type="checkbox"
                  onChange={(event) => {
                    setMetadataOnly(event.target.checked);
                    clearRetentionReviewFeedback();
                  }}
                />
                <span>Metadata only</span>
              </label>
              <label className="checkControl">
                <input
                  checked={exportEnabled}
                  type="checkbox"
                  onChange={(event) => {
                    setExportEnabled(event.target.checked);
                    clearRetentionReviewFeedback();
                  }}
                />
                <span>Export enabled</span>
              </label>
              <button
                className="secondaryAction"
                onClick={() => void submitPolicy()}
                type="button"
              >
                <ShieldCheck size={16} />
                Save policy
              </button>
            </section>

            <section
              className="retentionWorkflowPanel"
              aria-label="History retention cleanup workflow"
            >
              <div className="retentionWorkflowHeader">
                <span>
                  <strong>Cleanup</strong>
                  <small>
                    Choose domain and cutoff, preview impact, then delete the
                    reviewed rows.
                  </small>
                </span>
              </div>
              <div className="retentionFactGrid">
                <span>
                  <strong>Domain</strong>
                  <small>{selectedDomainName}</small>
                </span>
                <span>
                  <strong>Cleanup cutoff</strong>
                  <small
                    data-tooltip-empty-reason="No cleanup cutoff is configured for the selected history domain."
                    title={
                      retentionDays.trim()
                        ? undefined
                        : "No cleanup cutoff is configured for the selected history domain."
                    }
                  >
                    Older than {retentionDays || "-"} days
                  </small>
                </span>
                <span>
                  <strong>Cleanup review</strong>
                  <small>{cleanupReviewLabel}</small>
                </span>
                <span>
                  <strong>Effect</strong>
                  <small>{cleanupEffectLabel}</small>
                </span>
              </div>
              <p className="retentionPanelNote">
                <strong>Evidence retention only.</strong> History cleanup
                affects selected history evidence. System / Maintenance handles
                server artifact cleanup jobs.
              </p>
              <div className="retentionActions">
                <button
                  className="secondaryAction"
                  onClick={() => void previewPrune()}
                  type="button"
                >
                  Preview cleanup
                </button>
                <button
                  className="secondaryAction dangerAction"
                  data-tooltip-disabled-reason={
                    !pruneSnapshot
                      ? "Preview cleanup before deleting retained history."
                      : "The cleanup preview found no retained history rows to delete."
                  }
                  disabled={!pruneSnapshot || pruneSnapshot.reviewedRows === 0}
                  onClick={() => setPruneConfirmationOpen(true)}
                  title={
                    !pruneSnapshot
                      ? "Preview cleanup first"
                      : pruneSnapshot.reviewedRows === 0
                        ? "No reviewed rows match; deletion is not needed"
                        : `Review deletion of ${pruneSnapshot.reviewedRows} matched rows`
                  }
                  type="button"
                >
                  <Scissors size={16} />
                  Delete reviewed rows
                </button>
              </div>
            </section>

            <section
              className="retentionWorkflowPanel"
              aria-label="History retention export scope"
            >
              <div className="retentionWorkflowHeader">
                <span>
                  <strong>Export</strong>
                  <small>
                    Export selected domain as a JSON history bundle.
                  </small>
                </span>
              </div>
              <div className="retentionFactGrid">
                <span>
                  <strong>Export scope</strong>
                  <small>{selectedDomainName}</small>
                </span>
                <span>
                  <strong>Time range</strong>
                  <small>All retained records</small>
                </span>
                <span>
                  <strong>Format</strong>
                  <small>JSON history bundle</small>
                </span>
                <span>
                  <strong>Last export</strong>
                  <small>
                    {historyExport
                      ? formatTime(historyExport.generated_at)
                      : "Not exported"}
                  </small>
                </span>
              </div>
              <button
                className="secondaryAction"
                data-tooltip-disabled-reason="History export is disabled by the selected retention policy."
                disabled={!exportEnabled}
                onClick={() => void exportSelectedHistory()}
                type="button"
              >
                <Download size={16} />
                Export history
              </button>
            </section>
          </div>

          <details
            className="retentionDiagnostics"
            aria-label="Retention diagnostics"
          >
            <summary>Diagnostics</summary>
            <div className="retentionFactGrid">
              <span>
                <strong>Raw domain</strong>
                <small>{selectedDomainLabel}</small>
              </span>
              <span>
                <strong>Current records</strong>
                <small>
                  {currentRecordLabel}: {currentRecordDetail}
                </small>
              </span>
              <span>
                <strong>Storage size</strong>
                <small>Unavailable.</small>
              </span>
              <span>
                <strong>Policy updated</strong>
                <small>{policyUpdatedLabel}</small>
              </span>
              <span>
                <strong>Export scope</strong>
                <small>
                  Domain and row limit are supported; custom date windows are
                  unavailable.
                </small>
              </span>
              <span>
                <strong>Compliance note</strong>
                <small>{complianceWarning}</small>
              </span>
            </div>
          </details>

          <ConfirmationPrompt
            confirmLabel="Prune history"
            detail={
              (pruneSnapshot?.request.metadata_only ?? metadataOnly)
                ? "Deletes history metadata rows that match the selected domain, retention days, and prune limit."
                : "Deletes history rows and retained object files that match the selected domain, retention days, and prune limit."
            }
            error={
              retentionStatusTone === "danger"
                ? (retentionStatus ?? undefined)
                : undefined
            }
            items={[
              {
                label: "Domain",
                value: historyDomainLabel(
                  pruneSnapshot?.request.domain ?? selectedDomain,
                ),
              },
              { label: "Retention days", value: retentionDays },
              { label: "Limit", value: pruneLimit },
              {
                label: "Metadata only",
                value: pruneSnapshot?.request.metadata_only ? "yes" : "no",
              },
              {
                label: "Reviewed rows",
                value: pruneSnapshot?.reviewedRows ?? 0,
              },
              { label: "Objects", value: pruneSnapshot?.objectCount ?? 0 },
              {
                label: "Effect",
                value: pruneSnapshot?.effectLabel ?? "review required",
              },
              {
                label: "Review hash",
                value: pruneSnapshot?.previewHash
                  ? `${pruneSnapshot.previewHash.slice(0, 12)}...`
                  : "not returned",
                title: pruneSnapshot?.previewHash ?? "not returned",
              },
            ]}
            onCancel={clearPruneConfirmation}
            onConfirm={() => void confirmPrune()}
            open={pruneConfirmationOpen && pruneSnapshot !== null}
            title="Confirm history prune"
            tone="danger"
          />
          {historyPruneResult && (
            <div className="retentionResult">
              {historyPruneResult.domains.slice(0, 4).map((domain) => (
                <span key={domain.domain}>
                  <strong>{historyDomainLabel(domain.domain)}</strong>{" "}
                  {historyPruneStatusLabel(domain.status)}:{" "}
                  {domain.pruned_rows || domain.matched_rows} rows,{" "}
                  {domain.object_keys.length} objects
                  {domain.object_delete_attempted
                    ? ", object delete attempted"
                    : ", metadata rows only"}
                  {domain.object_delete_errors.length > 0
                    ? `, ${domain.object_delete_errors.length} delete error${domain.object_delete_errors.length === 1 ? "" : "s"}`
                    : ""}
                </span>
              ))}
            </div>
          )}
          {historyExport && (
            <div className="retentionResult">
              <span>
                <strong>Export</strong> {historyExport.domains.length} domain
                {historyExport.domains.length === 1 ? "" : "s"} as JSON at{" "}
                {formatTime(historyExport.generated_at)}; limit{" "}
                {historyExport.limit}/domain; scope{" "}
                {historyExport.domains.map(historyDomainLabel).join(", ")}
              </span>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function AuditEventDetailPanel({
  audit,
  onOpenDedicated,
  onOpenEvidence,
}: {
  audit: AuditLogRecord;
  onOpenDedicated?: () => void;
  onOpenEvidence: (reference: AuditEvidenceReference) => void;
}) {
  const presentation = presentAudit(audit);
  return (
    <div className="auditEventDetailPanel" aria-label="Audit event detail">
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>Audit event detail</strong>
          <small>
            {auditActionLabel(audit.action)} ·{" "}
            {formatFullTime(audit.created_at)}
          </small>
        </span>
        {onOpenDedicated && (
          <button
            className="secondaryAction compactAction"
            onClick={onOpenDedicated}
            type="button"
          >
            Open event
            <ExternalLink size={14} />
          </button>
        )}
      </div>
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Exact time</strong>
          <span>{formatFullTime(audit.created_at)}</span>
        </span>
        <span>
          <strong>Event</strong>
          <span>{presentation.actionLabel}</span>
        </span>
        <span>
          <strong>Actor</strong>
          <span>
            {presentation.actorLabel} · {presentation.actorDetail}
          </span>
        </span>
        <span>
          <strong>Origin</strong>
          <span>{presentation.originLabel}</span>
        </span>
        <span>
          <strong>Target</strong>
          <span>
            {presentation.targetLabel} · {presentation.targetDetail}
          </span>
        </span>
        <span>
          <strong>Outcome</strong>
          <span>{presentation.outcomeLabel}</span>
        </span>
        <span>
          <strong>Evidence</strong>
          <span>{auditRelatedEvidenceFullDetail(audit)}</span>
        </span>
        <span>
          <strong>Source IP</strong>
          <span>
            {presentation.sourceIp ?? auditMissingFieldLabel("request")}
          </span>
        </span>
        <span>
          <strong>Operator session</strong>
          <span>
            {presentation.operatorSessionId ??
              auditMissingFieldLabel("operator")}
          </span>
        </span>
        <span>
          <strong>Terminal session</strong>
          <span>
            {presentation.terminalSessionId ??
              auditMissingFieldLabel("terminal")}
          </span>
        </span>
        <span>
          <strong>Gateway session</strong>
          <span>
            {presentation.gatewaySessionId ?? auditMissingFieldLabel("gateway")}
          </span>
        </span>
        <span>
          <strong>Privilege scope</strong>
          <span>
            {presentation.privilege ?? auditMissingFieldLabel("privilege")}
          </span>
        </span>
        {presentation.executionPrivilege && (
          <span>
            <strong>Execution privilege</strong>
            <span>{presentation.executionPrivilege}</span>
          </span>
        )}
        <span>
          <strong>User agent</strong>
          <span>
            {presentation.userAgent ?? auditMissingFieldLabel("request")}
          </span>
        </span>
      </div>
      {presentation.evidenceReferences.some((reference) =>
        auditEvidenceHasDestination(reference),
      ) && (
        <div
          className="consoleInlineDetailActions"
          aria-label="Related audit evidence links"
        >
          {presentation.evidenceReferences
            .filter(auditEvidenceHasDestination)
            .map((reference) => (
              <button
                className="secondaryAction compactAction"
                key={`${reference.kind}:${reference.value}`}
                onClick={() => onOpenEvidence(reference)}
                title={reference.detail}
                type="button"
              >
                Open {reference.kind.toLowerCase()}
                <ExternalLink size={14} />
              </button>
            ))}
        </div>
      )}
      <details
        className="auditEventAdvanced"
        title="Raw persisted action, target, identifiers, and metadata for this audit event."
      >
        <summary>Advanced event data</summary>
        <div className="consoleInlineDetailGrid">
          <span>
            <strong>Command hash</strong>
            <span title={audit.command_hash ?? undefined}>
              {audit.command_hash
                ? shortHash(audit.command_hash)
                : "Not supplied for this event"}
            </span>
          </span>
          <span>
            <strong>Event ID</strong>
            <span>{audit.id}</span>
          </span>
          <span>
            <strong>Raw action</strong>
            <span>{audit.action}</span>
          </span>
          <span>
            <strong>Raw target</strong>
            <span>{audit.target}</span>
          </span>
        </div>
        <pre className="auditEventMetadata">{jsonText(audit.metadata)}</pre>
      </details>
    </div>
  );
}

function auditEvidenceHasDestination(
  reference: AuditEvidenceReference,
): boolean {
  return (
    reference.kind === "Job" &&
    /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(reference.value)
  );
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function auditActionLabel(action: string): string {
  return sharedAuditActionLabel(action);
}

function auditActionDetail(audit: AuditLogRecord): string {
  return presentAudit(audit).actionDetail;
}

function auditTargetLabel(audit: AuditLogRecord): string {
  return presentAudit(audit).targetLabel;
}

function auditTargetDetail(audit: AuditLogRecord): string {
  return presentAudit(audit).targetDetail;
}

function auditActorDetail(audit: AuditLogRecord): string {
  return presentAudit(audit).actorDetail;
}

function auditResultLabel(audit: AuditLogRecord): string {
  return presentAudit(audit).outcomeLabel;
}

function auditResultTone(
  audit: AuditLogRecord,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  return presentAudit(audit).outcomeTone;
}

function auditRelatedEvidenceLabel(audit: AuditLogRecord): string {
  return presentAudit(audit).evidenceLabel;
}

function auditRelatedEvidenceDetail(audit: AuditLogRecord): string {
  return presentAudit(audit).evidenceDetail;
}

function auditRelatedEvidenceFullDetail(audit: AuditLogRecord): string {
  const references = presentAudit(audit).evidenceReferences;
  if (references.length === 0) {
    return "This audit row is the complete event record; no related record ID was supplied.";
  }
  return references.map((reference) => reference.detail).join(" · ");
}

function auditRelatedEvidenceSearch(audit: AuditLogRecord): string {
  return auditEvidenceSearchText(audit);
}

function titleCase(value: string): string {
  return value
    .split(/\s+/)
    .filter(Boolean)
    .map((part) => `${part.charAt(0).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function auditMatchesFilters(
  audit: AuditLogRecord,
  filters: AuditFilterState,
): boolean {
  const checks: Array<[string, string]> = [
    [filters.actor, auditActorFilterText(audit)],
    [filters.action, auditFilterText(audit, "action")],
    [filters.resource, auditFilterText(audit, "resource")],
    [filters.result, auditFilterText(audit, "result")],
    [filters.ip, auditFilterText(audit, "ip")],
    [filters.session, auditFilterText(audit, "session")],
    [filters.privilege, auditFilterText(audit, "privilege")],
  ];
  if (checks.some(([query, value]) => !textMatches(query, value))) {
    return false;
  }
  const createdAt = Date.parse(audit.created_at);
  if (!Number.isNaN(createdAt)) {
    const from = parseFilterDate(filters.from, "start");
    if (from !== null && createdAt < from) {
      return false;
    }
    const to = parseFilterDate(filters.to, "end");
    if (to !== null && createdAt > to) {
      return false;
    }
  }
  return true;
}

function latestAuditRecord(audits: AuditLogRecord[]): AuditLogRecord | null {
  let latestRecord: AuditLogRecord | null = null;
  let latestTime = Number.NEGATIVE_INFINITY;
  for (const audit of audits) {
    const createdAt = Date.parse(audit.created_at);
    if (!Number.isNaN(createdAt) && createdAt > latestTime) {
      latestRecord = audit;
      latestTime = createdAt;
    }
  }
  return latestRecord;
}

function auditActor(audit: AuditLogRecord): string {
  return presentAudit(audit).actorLabel;
}

function auditActorFilterText(audit: AuditLogRecord): string {
  const presentation = presentAudit(audit);
  return [audit.actor_id, presentation.actorLabel, presentation.actorDetail]
    .filter((value): value is string => Boolean(value))
    .join(" ");
}

function auditFilterText(
  audit: AuditLogRecord,
  field: "action" | "ip" | "privilege" | "resource" | "result" | "session",
): string {
  const presentation = presentAudit(audit);
  switch (field) {
    case "action":
      return [
        audit.action,
        presentation.actionLabel,
        presentation.actionDetail,
      ].join(" ");
    case "resource":
      return [
        audit.target,
        presentation.targetLabel,
        presentation.targetDetail,
      ].join(" ");
    case "result":
      return presentation.outcomeLabel;
    case "ip":
      return presentation.sourceIp ?? "";
    case "session":
      return auditSessionSearchText(audit);
    case "privilege":
      return presentation.privilege ?? "";
  }
}

function jsonText(value: JsonValue): string {
  try {
    return JSON.stringify(value);
  } catch {
    return "";
  }
}

function textMatches(query: string, value: string): boolean {
  const normalized = query.trim().toLowerCase();
  if (!normalized) {
    return true;
  }
  return value.toLowerCase().includes(normalized);
}

function parseFilterDate(
  value: string,
  boundary: "end" | "start",
): number | null {
  if (!value) {
    return null;
  }
  const suffix = boundary === "start" ? "T00:00:00.000Z" : "T23:59:59.999Z";
  const time = Date.parse(`${value}${suffix}`);
  return Number.isNaN(time) ? null : time;
}

function historyDomainLabel(domain: string | null | undefined): string {
  const labels: Record<string, string> = {
    audit_logs: "Audit logs",
    backup_artifacts: "Backup artifacts",
    client_status_history: "VPS lifecycle",
    gateway_sessions: "Gateway sessions",
    job_outputs: "Job outputs",
    network_observations: "Network observations",
    system_metric_rollups: "System metrics",
    telemetry_network_rates: "Long-term network history",
    telemetry_ping_rollups: "Long-term Ping history",
    telemetry_rollups: "Long-term resource history",
    telemetry_samples: "High-resolution monitoring samples",
    traffic_counter_samples: "Long-term traffic counters",
    topology_history: "Topology history",
  };
  return domain
    ? (labels[domain] ?? titleCase(domain.replace(/_/g, " ")))
    : "Selected domain";
}

function historyDomainDescription(domain: string | null | undefined): string {
  const descriptions: Record<string, string> = {
    audit_logs: "Operator and control-plane event ledger",
    backup_artifacts: "Backup metadata and retained artifact references",
    client_status_history: "VPS connection and lifecycle history",
    gateway_sessions: "Gateway connection session history",
    job_outputs: "Command output and retained job evidence",
    network_observations:
      "Exact manual, speed, and status evidence; automatic reachability is exact through 2d, then retained in fixed age tiers",
    system_metric_rollups:
      "Control-plane capacity history in fixed 1m/5m/30m/1h/3h/6h/1d age tiers",
    telemetry_network_rates:
      "Authoritative RX/TX rate history in fixed 1m/5m/30m/1h/3h/6h/1d age tiers",
    telemetry_ping_rollups:
      "Authoritative Ping history by target generation in fixed 1m/5m/30m/1h/3h/6h/1d age tiers",
    telemetry_rollups:
      "Authoritative CPU, load, memory, and disk history in fixed 1m/5m/30m/1h/3h/6h/1d age tiers",
    telemetry_samples:
      "Accepted exact resource, network, traffic, and Ping source samples retained for 7 days",
    traffic_counter_samples:
      "Authoritative traffic counters exact through 32d, then retained at 1h/3h/6h/1d transition tiers",
    topology_history: "Topology graph and trend history",
  };
  return domain
    ? (descriptions[domain] ?? "Retained history domain")
    : "Retained history domain";
}

function isTieredMonitoringDomain(domain: string | null | undefined): boolean {
  return [
    "network_observations",
    "system_metric_rollups",
    "telemetry_network_rates",
    "telemetry_ping_rollups",
    "telemetry_rollups",
    "traffic_counter_samples",
  ].includes(domain ?? "");
}

function historyPruneStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    disabled: "Disabled",
    dry_run: "Preview",
    object_delete_failed: "Object delete failed",
    pruned: "Deleted",
  };
  return labels[status] ?? titleCase(status.replace(/_/g, " "));
}

function totalMatchedRows(response: HistoryRetentionPruneResponse): number {
  return response.domains.reduce((sum, domain) => sum + domain.matched_rows, 0);
}

function totalPrunedRows(response: HistoryRetentionPruneResponse): number {
  return response.domains.reduce((sum, domain) => sum + domain.pruned_rows, 0);
}

function totalPrunedOrMatchedRows(
  response: HistoryRetentionPruneResponse,
): number {
  return response.dry_run
    ? totalMatchedRows(response)
    : totalPrunedRows(response);
}

function totalObjectKeys(response: HistoryRetentionPruneResponse): number {
  return response.domains.reduce(
    (sum, domain) => sum + domain.object_keys.length,
    0,
  );
}

function formatPruneEffect(
  response: HistoryRetentionPruneResponse,
  metadataOnly: boolean | null,
): string {
  const rows = totalPrunedOrMatchedRows(response);
  const objects = totalObjectKeys(response);
  if (response.dry_run) {
    return metadataOnly
      ? `Would delete ${rows} metadata rows; retained object files stay untouched.`
      : `Would delete ${rows} rows and review ${objects} retained object keys.`;
  }
  return metadataOnly
    ? `Deleted ${rows} metadata rows; retained object files stayed untouched.`
    : `Deleted ${rows} rows and attempted cleanup for ${objects} retained object keys.`;
}
