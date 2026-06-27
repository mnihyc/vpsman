import {
  ClipboardList,
  Download,
  Filter,
  RotateCcw,
  Scissors,
  ShieldCheck,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
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
  metadataOperator,
  shortHash,
  shortId,
} from "../utils";

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

const EXPECTED_AUDIT_WORKFLOWS = [
  "login",
  "privilege unlock",
  "config read/write",
  "command dispatch",
  "file edit",
  "terminal input",
  "key import/revoke",
  "backup/restore",
  "topology update",
  "system config change",
];

type AuditWorkflowCoverage = {
  covered: string[];
  missing: string[];
  relatedCount: number;
};

export function AuditLogPanel({
  activeSubpage,
  audits,
  error,
  historyExport,
  historyPruneResult,
  historyRetentionPolicies,
  loading,
  onExportHistory,
  onPruneHistoryRetention,
  onRefresh,
  onUpsertHistoryRetentionPolicy,
}: {
  activeSubpage: string;
  audits: AuditLogRecord[];
  error: string | null;
  historyExport: HistoryExportRecord | null;
  historyPruneResult: HistoryRetentionPruneResponse | null;
  historyRetentionPolicies: HistoryRetentionPolicyRecord[];
  loading: boolean;
  onExportHistory: (domains?: string) => Promise<void>;
  onPruneHistoryRetention: (
    request: HistoryRetentionPruneRequest,
  ) => Promise<HistoryRetentionPruneResponse>;
  onRefresh: () => void;
  onUpsertHistoryRetentionPolicy: (
    request: HistoryRetentionPolicyRequest,
  ) => Promise<void>;
}) {
  const auditSubpage = activeSubpage === "retention" ? "retention" : "events";
  const [selectedDomain, setSelectedDomain] = useState("audit_logs");
  const selectedPolicy = useMemo(
    () =>
      historyRetentionPolicies.find(
        (policy) => policy.domain === selectedDomain,
      ) ?? historyRetentionPolicies[0],
    [historyRetentionPolicies, selectedDomain],
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
  const [auditFilters, setAuditFilters] =
    useState<AuditFilterState>(EMPTY_AUDIT_FILTERS);
  const [selectedAuditId, setSelectedAuditId] = useState<string | null>(null);

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
  const selectedAudit = useMemo(
    () => filteredAudits.find((audit) => audit.id === selectedAuditId) ?? null,
    [filteredAudits, selectedAuditId],
  );
  const auditActors = useMemo(
    () =>
      Array.from(
        new Set(
          audits
            .map((audit) => auditActor(audit))
            .filter((value): value is string => Boolean(value)),
        ),
      ).slice(0, 3),
    [audits],
  );
  const latestVisibleAudit = useMemo(
    () => latestAuditRecord(filteredAudits),
    [filteredAudits],
  );
  const auditWorkflowCoverage = useMemo(
    () => summarizeAuditWorkflowCoverage(audits),
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
        header: "Operator",
        size: 170,
        minSize: 150,
        sortValue: (audit) => auditActor(audit) ?? "",
        searchValue: (audit) =>
          `${auditActor(audit) ?? ""} ${audit.actor_id ?? ""} ${auditMetadataValue(audit, ["operator_role", "role"]) ?? ""}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{auditActor(audit) ?? "Unknown operator"}</strong>
            <small>{auditOperatorDetail(audit)}</small>
          </span>
        ),
      },
      {
        id: "action",
        header: "Action",
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
            <small>{auditTargetDetail(audit)}</small>
          </span>
        ),
      },
      {
        id: "result",
        header: "Result",
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
        header: "Related job/session",
        size: 210,
        minSize: 150,
        sortValue: (audit) => auditRelatedEvidenceLabel(audit),
        searchValue: (audit) => auditRelatedEvidenceSearch(audit),
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{auditRelatedEvidenceLabel(audit)}</strong>
            <small>{auditRelatedEvidenceDetail(audit)}</small>
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

  const submitPolicy = async () => {
    try {
      await onUpsertHistoryRetentionPolicy({
        domain: selectedPolicy?.domain ?? selectedDomain,
        retention_days: Number(retentionDays),
        prune_limit: Number(pruneLimit),
        metadata_only: metadataOnly,
        export_enabled: exportEnabled,
        confirmed: true,
      });
    } catch {
      // The data hook owns the visible error state for this panel.
    }
  };

  const pruneRequest = (dryRun: boolean): HistoryRetentionPruneRequest => ({
    domain: selectedPolicy?.domain ?? selectedDomain,
    dry_run: dryRun,
    metadata_only: metadataOnly,
    confirmed: !dryRun,
  });

  const previewPrune = async () => {
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
    } catch {
      // The data hook owns the visible error state for this panel.
    }
  };

  const confirmPrune = async () => {
    if (!pruneSnapshot) {
      return;
    }
    try {
      await onPruneHistoryRetention(pruneSnapshot.request);
      clearPruneConfirmation();
    } catch {
      // The data hook owns the visible error state for this panel.
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
        ? "Audit API returned no rows; privileged activity is not evidenced in the table."
        : "Visible audit rows only; retention total is not exposed separately."
      : "Retention API does not expose current rows for this history domain.";
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
      : "Record totals and storage size need backend evidence for compliance-grade retention.";

  return (
    <section className="workspace singleColumn">
      {auditSubpage === "events" && (
        <div className="fleetPanel">
          <div className="sectionHeader">
            <div>
              <h2>Audit log</h2>
              <span>
                {error ??
                  (loading
                    ? "Refreshing audit records"
                    : "Operator and control-plane events")}
              </span>
            </div>
            <button
              className="secondaryAction"
              disabled={loading}
              onClick={onRefresh}
              type="button"
            >
              Refresh
            </button>
          </div>
          <div className="auditEventSummary" aria-label="Audit event summary">
            <div className="auditEventMetric">
              <span>Visible events</span>
              <strong>
                {filteredAudits.length} / {audits.length}
              </strong>
              <p>
                {hasAuditFilters
                  ? `${activeFilterCount} active filters`
                  : "All returned events"}
              </p>
            </div>
            <div className="auditEventMetric">
              <span>Latest visible</span>
              <strong>
                {latestVisibleAudit
                  ? formatCompactTime(latestVisibleAudit.created_at)
                  : "No events"}
              </strong>
              <p>{lastAuditTime}</p>
            </div>
            <div className="auditEventMetric">
              <span>Related evidence</span>
              <strong>{auditWorkflowCoverage.relatedCount} linked</strong>
              <p>Job, terminal, session, or schedule references in metadata.</p>
            </div>
            <div className="auditEventMetric">
              <span>Known operators</span>
              <strong>{auditActors.length || "None"}</strong>
              <p>
                {auditActors.length > 0
                  ? auditActors.join(", ")
                  : "No actor values available."}
              </p>
            </div>
            <div
              className={`auditCoverageNotice ${auditWorkflowCoverage.missing.length > 0 ? "attention" : "ready"}`}
              aria-label="Audit coverage warning"
            >
              <strong>
                {auditWorkflowCoverage.missing.length > 0
                  ? "Coverage warning"
                  : "Coverage sample present"}
              </strong>
              <span>
                {auditWorkflowCoverage.missing.length > 0
                  ? `${auditWorkflowCoverage.covered.length}/${EXPECTED_AUDIT_WORKFLOWS.length} expected workflow families are visible. Missing: ${auditWorkflowCoverage.missing.join(", ")}.`
                  : `All ${EXPECTED_AUDIT_WORKFLOWS.length} expected workflow families are represented in the returned rows.`}
              </span>
            </div>
          </div>
          <div className="auditFilterBar" aria-label="Audit event filters">
            <div className="auditFilterIntro">
              <Filter size={16} />
              <span>
                Filter audit evidence by actor, action, resource, result, time,
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
              <span>Result</span>
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
                  {error ??
                    (hasAuditFilters
                      ? "Clear filters or broaden the time window to inspect available events."
                      : "Expected login, unlock, dispatch, file, key, backup, topology, and system events are not evidenced by the API response.")}
                </span>
              </div>
            }
            getRowId={(audit) => audit.id}
            itemLabel="records"
            onOpenRow={(audit) => setSelectedAuditId(audit.id)}
            openRowLabel="View audit"
            openRowTitle={(audit) => `Show details for audit record ${audit.id}.`}
            rows={filteredAudits}
            storageKey="vpsman.grid.audit.events"
            title="Audit records"
          />
          {selectedAudit ? (
            <AuditEventDetailPanel
              audit={selectedAudit}
              onClose={() => setSelectedAuditId(null)}
            />
          ) : null}
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
              disabled={loading}
              onClick={onRefresh}
              type="button"
            >
              Refresh
            </button>
          </div>
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
                    clearPruneConfirmation();
                  }}
                  type="button"
                >
                  <span className="retentionDomainCell">
                    <strong>{historyDomainLabel(policy.domain)}</strong>
                    <small>{historyDomainDescription(policy.domain)}</small>
                  </span>
                  <span className="retentionPolicyValue">
                    <small>Retention days</small>
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
                    clearPruneConfirmation();
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
                  <span>Retention days</span>
                  <input
                    min={1}
                    max={3650}
                    type="number"
                    value={retentionDays}
                    onChange={(event) => {
                      setRetentionDays(event.target.value);
                      clearPruneConfirmation();
                    }}
                  />
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
                      clearPruneConfirmation();
                    }}
                  />
                </label>
              </div>
              <label className="checkControl">
                <input
                  checked={metadataOnly}
                  type="checkbox"
                  onChange={(event) => {
                    setMetadataOnly(event.target.checked);
                    clearPruneConfirmation();
                  }}
                />
                <span>Metadata only</span>
              </label>
              <label className="checkControl">
                <input
                  checked={exportEnabled}
                  type="checkbox"
                  onChange={(event) => setExportEnabled(event.target.checked)}
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
                  <small>Older than {retentionDays || "-"} days</small>
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
                  className="dangerAction"
                  disabled={!pruneSnapshot}
                  onClick={() => setPruneConfirmationOpen(true)}
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
                disabled={!exportEnabled}
                onClick={() => void onExportHistory(selectedDomainLabel)}
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
                <small>Not reported by the retention API.</small>
              </span>
              <span>
                <strong>Policy updated</strong>
                <small>{policyUpdatedLabel}</small>
              </span>
              <span>
                <strong>API export window</strong>
                <small>
                  Domain and limit are supported; custom date-window export is
                  not exposed.
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
  onClose,
}: {
  audit: AuditLogRecord;
  onClose: () => void;
}) {
  return (
    <section
      className="consoleDetailPanel auditEventDetailPanel"
      aria-label="Audit event detail"
    >
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>Audit event detail</strong>
          <small>
            {auditActionLabel(audit.action)} ·{" "}
            {formatFullTime(audit.created_at)}
          </small>
        </span>
        <button
          className="secondaryAction compactAction"
          onClick={onClose}
          type="button"
        >
          Close
        </button>
      </div>
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Exact time</strong>
          <span>{formatFullTime(audit.created_at)}</span>
        </span>
        <span>
          <strong>Actor</strong>
          <span>{auditActor(audit) ?? "unknown"}</span>
        </span>
        <span>
          <strong>Action</strong>
          <span>{auditActionLabel(audit.action)}</span>
        </span>
        <span>
          <strong>Target</strong>
          <span>{auditTargetLabel(audit)}</span>
        </span>
        <span>
          <strong>Result</strong>
          <span>{auditResultLabel(audit)}</span>
        </span>
        <span>
          <strong>Related evidence</strong>
          <span>{auditRelatedEvidenceFullDetail(audit)}</span>
        </span>
        <span>
          <strong>Source IP</strong>
          <span>{auditFilterText(audit, "ip") || "not recorded"}</span>
        </span>
        <span>
          <strong>Session</strong>
          <span>{auditFilterText(audit, "session") || "not recorded"}</span>
        </span>
        <span>
          <strong>Privilege</strong>
          <span>{auditFilterText(audit, "privilege") || "not recorded"}</span>
        </span>
        <span>
          <strong>Command hash</strong>
          <span>
            {audit.command_hash ? shortHash(audit.command_hash) : "none"}
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
    </section>
  );
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function summarizeAuditWorkflowCoverage(
  audits: AuditLogRecord[],
): AuditWorkflowCoverage {
  const covered = new Set<string>();
  for (const audit of audits) {
    for (const workflow of auditWorkflowFamilies(audit)) {
      covered.add(workflow);
    }
  }
  return {
    covered: EXPECTED_AUDIT_WORKFLOWS.filter((workflow) =>
      covered.has(workflow),
    ),
    missing: EXPECTED_AUDIT_WORKFLOWS.filter(
      (workflow) => !covered.has(workflow),
    ),
    relatedCount: audits.filter(
      (audit) => auditRelatedEvidenceLabel(audit) !== "No linked evidence",
    ).length,
  };
}

function auditWorkflowFamilies(audit: AuditLogRecord): string[] {
  const haystack = `${audit.action} ${audit.target} ${jsonText(
    audit.metadata,
  )}`.toLowerCase();
  const workflows = new Set<string>();
  if (haystack.includes("login") || haystack.includes("operator_auth")) {
    workflows.add("login");
  }
  if (haystack.includes("privilege_unlock") || haystack.includes("privilege")) {
    workflows.add("privilege unlock");
  }
  if (
    haystack.includes("config") ||
    haystack.includes("source_template") ||
    haystack.includes("suite_config")
  ) {
    workflows.add("config read/write");
  }
  if (
    haystack.includes("job.") ||
    haystack.includes("dispatch") ||
    haystack.includes("command")
  ) {
    workflows.add("command dispatch");
  }
  if (haystack.includes("file")) {
    workflows.add("file edit");
  }
  if (haystack.includes("terminal")) {
    workflows.add("terminal input");
  }
  if (haystack.includes("key") || haystack.includes("totp")) {
    workflows.add("key import/revoke");
  }
  if (
    haystack.includes("backup") ||
    haystack.includes("restore") ||
    haystack.includes("migration")
  ) {
    workflows.add("backup/restore");
  }
  if (
    haystack.includes("network") ||
    haystack.includes("ospf") ||
    haystack.includes("topology") ||
    haystack.includes("tunnel")
  ) {
    workflows.add("topology update");
  }
  if (
    haystack.includes("system") ||
    haystack.includes("runtime_config") ||
    haystack.includes("suite_config")
  ) {
    workflows.add("system config change");
  }
  return Array.from(workflows);
}

function auditActionLabel(action: string): string {
  const known: Record<string, string> = {
    "job.dispatch_requested": "Job dispatch requested",
    "operator.login": "Operator login",
    privilege_unlock: "Privilege unlock",
    "terminal.close": "Terminal closed",
    "terminal.input": "Terminal input",
    "terminal.open": "Terminal opened",
  };
  return known[action] ?? titleCase(action.replace(/[._:/-]+/g, " "));
}

function auditActionDetail(audit: AuditLogRecord): string {
  const commandType = auditMetadataValue(audit, ["command_type"]);
  const targetCount = auditMetadataValue(audit, ["target_count"]);
  const privileged = auditMetadataValue(audit, ["privileged"]);
  const details = [
    commandType ? titleCase(commandType.replace(/_/g, " ")) : null,
    targetCount
      ? `${targetCount} target${targetCount === "1" ? "" : "s"}`
      : null,
    privileged === "true" ? "privileged" : null,
  ].filter((value): value is string => Boolean(value));
  if (details.length > 0) {
    return details.join(" · ");
  }
  if (audit.action.startsWith("terminal.")) {
    return "Terminal session event";
  }
  if (audit.action.includes("privilege")) {
    return "Privilege access event";
  }
  if (audit.action.includes("dispatch")) {
    return "Dispatch event";
  }
  return "Audit event";
}

function auditTargetLabel(audit: AuditLogRecord): string {
  if (audit.target === "access/privilege-vault") {
    return "Privilege vault";
  }
  if (audit.target === "auth:login") {
    return "Authentication";
  }
  if (audit.target.startsWith("api:/api/v1/jobs")) {
    return "Jobs API";
  }
  if (audit.target.startsWith("terminal:")) {
    const clientId = auditMetadataValue(audit, ["client_id"]);
    return clientId ? `Terminal: ${clientId}` : "Terminal session";
  }
  if (audit.target.startsWith("client:")) {
    return `VPS: ${audit.target.slice("client:".length)}`;
  }
  return titleCase(audit.target.replace(/[._:/-]+/g, " "));
}

function auditTargetDetail(audit: AuditLogRecord): string {
  const clientId = auditMetadataValue(audit, ["client_id"]);
  const targetCount = auditMetadataValue(audit, ["target_count"]);
  if (targetCount) {
    return `${targetCount} resolved target${targetCount === "1" ? "" : "s"}`;
  }
  if (clientId) {
    return clientId;
  }
  if (audit.target === "access/privilege-vault") {
    return "access control";
  }
  if (audit.target === "auth:login") {
    return "operator access";
  }
  return shortId(audit.target);
}

function auditOperatorDetail(audit: AuditLogRecord): string {
  const role = auditMetadataValue(audit, ["operator_role", "role"]);
  const ip = auditMetadataValue(audit, [
    "client_ip",
    "ip",
    "remote_addr",
    "remote_ip",
    "request_ip",
    "source_ip",
  ]);
  if (role && ip) {
    return `${role} · ${ip}`;
  }
  if (role) {
    return role;
  }
  if (ip) {
    return ip;
  }
  return audit.actor_id ? shortId(audit.actor_id) : "system";
}

function auditResultLabel(audit: AuditLogRecord): string {
  const raw =
    auditMetadataValue(audit, [
      "decision",
      "error",
      "outcome",
      "result",
      "status",
    ]) ?? defaultAuditResult(audit.action);
  return titleCase(raw.replace(/_/g, " "));
}

function defaultAuditResult(action: string): string {
  if (action.endsWith("_requested") || action.includes(".dispatch")) {
    return "requested";
  }
  if (action.startsWith("terminal.")) {
    return "recorded";
  }
  return "recorded";
}

function auditResultTone(
  audit: AuditLogRecord,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  const value = auditResultLabel(audit).toLowerCase();
  if (
    value.includes("fail") ||
    value.includes("error") ||
    value.includes("denied") ||
    value.includes("reject")
  ) {
    return "critical";
  }
  if (value.includes("warning") || value.includes("stale")) {
    return "warning";
  }
  if (
    value.includes("success") ||
    value.includes("accepted") ||
    value.includes("complete") ||
    value.includes("applied")
  ) {
    return "ok";
  }
  if (value.includes("requested") || value.includes("recorded")) {
    return "info";
  }
  return "neutral";
}

function auditRelatedEvidenceLabel(audit: AuditLogRecord): string {
  const references = auditRelatedEvidenceReferences(audit);
  if (references.length === 0) {
    return "No linked evidence";
  }
  return references
    .slice(0, 2)
    .map((reference) => reference.label)
    .join(" · ");
}

function auditRelatedEvidenceDetail(audit: AuditLogRecord): string {
  const references = auditRelatedEvidenceReferences(audit);
  if (references.length === 0) {
    return "No job, terminal, session, or schedule reference";
  }
  if (references.length === 1) {
    return references[0].label;
  }
  return `${references.length} links: ${references
    .map((reference) => reference.kind)
    .join(", ")}`;
}

function auditRelatedEvidenceFullDetail(audit: AuditLogRecord): string {
  const references = auditRelatedEvidenceReferences(audit);
  if (references.length === 0) {
    return "No job, terminal, session, or schedule reference";
  }
  return references.map((reference) => reference.detail).join(" · ");
}

function auditRelatedEvidenceSearch(audit: AuditLogRecord): string {
  return auditRelatedEvidenceReferences(audit)
    .flatMap((reference) => [
      reference.label,
      reference.detail,
      reference.value,
    ])
    .join(" ");
}

function auditRelatedEvidenceReferences(audit: AuditLogRecord): Array<{
  detail: string;
  kind: string;
  label: string;
  value: string;
}> {
  const refs: Array<{
    detail: string;
    kind: string;
    label: string;
    value: string;
  }> = [];
  const pushRef = (kind: string, value: string | null) => {
    if (!value || refs.some((reference) => reference.value === value)) {
      return;
    }
    refs.push({
      detail: `${kind} ${value}`,
      kind,
      label: `${kind} ${shortId(value)}`,
      value,
    });
  };
  pushRef("Job", auditMetadataValue(audit, ["activation_job_id", "job_id"]));
  pushRef("Terminal", auditMetadataValue(audit, ["terminal_session_id"]));
  pushRef(
    "Session",
    auditMetadataValue(audit, [
      "gateway_session_id",
      "operator_session_id",
      "session",
      "session_id",
    ]),
  );
  pushRef("Schedule", auditMetadataValue(audit, ["source_schedule_id"]));
  return refs;
}

function auditMetadataValue(
  audit: AuditLogRecord,
  keys: string[],
): string | null {
  const values = collectMetadataValues(
    audit.metadata,
    new Set(keys.map((key) => key.toLowerCase())),
  );
  return values.find((value) => value && value !== "null") ?? null;
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
    [filters.actor, auditActor(audit) ?? ""],
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

function auditActor(audit: AuditLogRecord): string | null {
  return (
    metadataOperator(audit.metadata) ??
    metadataFieldText(audit.metadata, [
      "actor",
      "actor_id",
      "operator",
      "operator_username",
      "user",
      "user_id",
      "username",
    ]) ??
    audit.actor_id
  );
}

function auditFilterText(
  audit: AuditLogRecord,
  field: "action" | "ip" | "privilege" | "resource" | "result" | "session",
): string {
  const metadata = audit.metadata;
  const metadataJson = jsonText(metadata);
  switch (field) {
    case "action":
      return [
        audit.action,
        metadataFieldText(metadata, [
          "action",
          "command",
          "command_type",
          "event",
          "event_kind",
          "operation",
          "type",
        ]),
        metadataJson,
      ].join(" ");
    case "resource":
      return [
        audit.target,
        metadataFieldText(metadata, [
          "client_id",
          "domain",
          "object_id",
          "resource",
          "resource_id",
          "target",
          "target_id",
          "vps_id",
        ]),
        metadataJson,
      ].join(" ");
    case "result":
      return [
        metadataFieldText(metadata, [
          "decision",
          "error",
          "exit_code",
          "outcome",
          "result",
          "status",
        ]),
        metadataJson,
      ].join(" ");
    case "ip":
      return (
        metadataFieldText(metadata, [
          "client_ip",
          "ip",
          "remote_addr",
          "remote_ip",
          "request_ip",
          "source_ip",
        ]) ?? metadataJson
      );
    case "session":
      return (
        metadataFieldText(metadata, [
          "gateway_session_id",
          "session",
          "session_id",
          "terminal_session_id",
        ]) ?? metadataJson
      );
    case "privilege":
      return (
        metadataFieldText(metadata, [
          "capability",
          "permission",
          "permission_scope",
          "privilege_scope",
          "required_scope",
          "role",
          "scope",
        ]) ?? metadataJson
      );
  }
}

function metadataFieldText(metadata: JsonValue, keys: string[]): string | null {
  const values = collectMetadataValues(metadata, new Set(keys));
  return values.length > 0 ? values.join(" ") : null;
}

function collectMetadataValues(
  value: JsonValue,
  keys: Set<string>,
  values: string[] = [],
): string[] {
  if (Array.isArray(value)) {
    value.forEach((entry) => collectMetadataValues(entry, keys, values));
    return values;
  }
  if (value && typeof value === "object") {
    Object.entries(value).forEach(([key, entry]) => {
      if (keys.has(key.toLowerCase())) {
        values.push(jsonScalarText(entry));
      }
      collectMetadataValues(entry, keys, values);
    });
  }
  return values;
}

function jsonScalarText(value: JsonValue): string {
  if (value === null) {
    return "null";
  }
  if (typeof value === "object") {
    return jsonText(value);
  }
  return String(value);
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
    telemetry_network_rates: "Network traffic rates",
    telemetry_rollups: "Telemetry rollups",
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
    network_observations: "Tunnel health, probe, and observation history",
    system_metric_rollups: "Control-plane capacity rollups",
    telemetry_network_rates: "Per-VPS network rate history",
    telemetry_rollups: "Per-VPS telemetry rollups",
    topology_history: "Topology graph and trend history",
  };
  return domain
    ? (descriptions[domain] ?? "Retained history domain")
    : "Retained history domain";
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
