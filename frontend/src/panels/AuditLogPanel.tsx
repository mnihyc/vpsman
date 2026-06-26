import { ClipboardList, Download, Eye, Filter, RotateCcw, Scissors, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleDataGrid, type ConsoleDataGridColumn } from "../components/ConsoleDataGrid";
import type {
  AuditLogRecord,
  HistoryExportRecord,
  HistoryRetentionPolicyRecord,
  HistoryRetentionPolicyRequest,
  HistoryRetentionPruneRequest,
  HistoryRetentionPruneResponse,
  JsonValue,
} from "../types";
import { formatTime, metadataOperator, metadataPreview, shortHash, shortId } from "../utils";

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
  onPruneHistoryRetention: (request: HistoryRetentionPruneRequest) => Promise<HistoryRetentionPruneResponse>;
  onRefresh: () => void;
  onUpsertHistoryRetentionPolicy: (request: HistoryRetentionPolicyRequest) => Promise<void>;
}) {
  const auditSubpage = activeSubpage === "retention" ? "retention" : "events";
  const [selectedDomain, setSelectedDomain] = useState("audit_logs");
  const selectedPolicy = useMemo(
    () => historyRetentionPolicies.find((policy) => policy.domain === selectedDomain) ?? historyRetentionPolicies[0],
    [historyRetentionPolicies, selectedDomain],
  );
  const [retentionDays, setRetentionDays] = useState("365");
  const [pruneLimit, setPruneLimit] = useState("1000");
  const [metadataOnly, setMetadataOnly] = useState(false);
  const [exportEnabled, setExportEnabled] = useState(true);
  const [policyConfirmation, setPolicyConfirmation] =
    useState<HistoryRetentionPolicyRequest | null>(null);
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

  const exportDomains = useMemo(
    () =>
      historyRetentionPolicies
        .filter((policy) => policy.export_enabled)
        .map((policy) => policy.domain)
        .join(","),
    [historyRetentionPolicies],
  );
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
  const lastAuditTime = filteredAudits[0]?.created_at
    ? formatTime(filteredAudits[0].created_at)
    : "No visible events";
  const auditColumns = useMemo<ConsoleDataGridColumn<AuditLogRecord>[]>(
    () => [
      {
        id: "action",
        header: "Action",
        size: 190,
        minSize: 140,
        sortValue: (audit) => audit.action,
        searchValue: (audit) => `${audit.action} ${audit.id}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{audit.action}</strong>
            <small>{shortId(audit.id)}</small>
          </span>
        ),
      },
      {
        id: "target",
        header: "Target",
        size: 230,
        minSize: 150,
        sortValue: (audit) => audit.target,
        searchValue: (audit) => `${audit.target} ${metadataPreview(audit.metadata)}`,
        cell: (audit) => (
          <span className="historyPrimary">
            <strong>{audit.target}</strong>
            <small>{metadataPreview(audit.metadata)}</small>
          </span>
        ),
      },
      {
        id: "operator",
        header: "Operator",
        size: 150,
        sortValue: (audit) => metadataOperator(audit.metadata) ?? audit.actor_id,
        searchValue: (audit) => metadataOperator(audit.metadata) ?? audit.actor_id,
        cell: (audit) => metadataOperator(audit.metadata) ?? shortId(audit.actor_id),
      },
      {
        id: "command",
        header: "Command",
        size: 130,
        sortValue: (audit) => audit.command_hash ?? "",
        searchValue: (audit) => audit.command_hash ?? "",
        cell: (audit) => <span className="monoValue">{audit.command_hash ? shortHash(audit.command_hash) : "-"}</span>,
      },
      {
        id: "created",
        header: "Created",
        size: 170,
        sortValue: (audit) => audit.created_at,
        searchValue: (audit) => audit.created_at,
        cell: (audit) => formatTime(audit.created_at),
      },
    ],
    [],
  );

  function clearPolicyConfirmation() {
    setPolicyConfirmation(null);
  }

  function clearPruneConfirmation() {
    setPruneSnapshot(null);
    setPruneConfirmationOpen(false);
  }

  const submitPolicy = () => {
    setPolicyConfirmation({
      domain: selectedPolicy?.domain ?? selectedDomain,
      retention_days: Number(retentionDays),
      prune_limit: Number(pruneLimit),
      metadata_only: metadataOnly,
      export_enabled: exportEnabled,
      confirmed: true,
    });
  };

  const pruneRequest = (dryRun: boolean): HistoryRetentionPruneRequest => ({
      domain: selectedPolicy?.domain ?? selectedDomain,
      dry_run: dryRun,
      metadata_only: metadataOnly,
      confirmed: !dryRun,
    });

  const previewPrune = async (openConfirmation: boolean) => {
    try {
      const preview = await onPruneHistoryRetention({
        ...pruneRequest(true),
        confirmed: false,
        preview_hash: null,
      });
      if (openConfirmation) {
        const reviewedRows = totalMatchedRows(preview);
        const objectCount = totalObjectKeys(preview);
        const request = {
          ...pruneRequest(false),
          confirmed: true,
          preview_hash: preview.preview_hash ?? null,
        };
        setPruneSnapshot({
          effectLabel: formatPruneEffect(preview, request.metadata_only ?? metadataOnly),
          objectCount,
          previewHash: preview.preview_hash ?? null,
          request,
          reviewedRows,
        });
        setPruneConfirmationOpen(true);
      }
    } catch {
      // The data hook owns the visible error state for this panel.
    }
  };

  const confirmPolicy = async () => {
    if (!policyConfirmation) {
      return;
    }
    await onUpsertHistoryRetentionPolicy(policyConfirmation);
    setPolicyConfirmation(null);
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
  const pruneEffectLabel = historyPruneResult
    ? `${historyPruneResult.dry_run ? "Dry run" : "Applied"}: ${totalPrunedOrMatchedRows(historyPruneResult)} rows, ${totalObjectKeys(historyPruneResult)} objects`
    : "Preview required";
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
            <span>{error ?? (loading ? "Refreshing audit records" : "Operator and control-plane events")}</span>
          </div>
          <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
            Refresh
          </button>
        </div>
        <div className="auditCoverageOverview" aria-label="Audit coverage overview">
          <div className={`auditCoverageCard ${audits.length === 0 ? "attention" : "ready"}`}>
            <span>Audit records</span>
            <strong>{filteredAudits.length} visible / {audits.length} total</strong>
            <p>{audits.length === 0 ? "No records returned; capture coverage is not evidenced." : `Latest visible event: ${lastAuditTime}.`}</p>
          </div>
          <div className="auditCoverageCard">
            <span>Actor filter</span>
            <strong>{auditFilters.actor || "All actors"}</strong>
            <p>{auditActors.length > 0 ? `Known: ${auditActors.join(", ")}` : "No actor values available."}</p>
          </div>
          <div className="auditCoverageCard">
            <span>Action/resource filter</span>
            <strong>{auditFilters.action || "Any action"} / {auditFilters.resource || "any resource"}</strong>
            <p>Matches action names, targets, resource metadata, and payload text.</p>
          </div>
          <div className="auditCoverageCard">
            <span>Result/session/IP</span>
            <strong>{auditFilters.result || "Any result"} / {auditFilters.session || "any session"} / {auditFilters.ip || "any IP"}</strong>
            <p>Use for alert review and access-path reconstruction.</p>
          </div>
          <div className="auditCoverageCard">
            <span>Privilege/time</span>
            <strong>{auditFilters.privilege || "Any scope"} / {auditFilters.from || "start"} to {auditFilters.to || "now"}</strong>
            <p>Filters by privilege scope metadata and event creation date.</p>
          </div>
          <div className={`auditCoverageCard ${audits.length === 0 ? "attention" : ""}`}>
            <span>Capture health</span>
            <strong>{audits.length === 0 ? "API gap" : "Events present"}</strong>
            <p>{audits.length === 0 ? "A production control plane should not show an empty audit ledger." : "Table evidence is available for review."}</p>
          </div>
          <div className="auditCoverageContract">
            <div>
              <strong>Coverage contract</strong>
              <span>Audit must capture every security and operator workflow, not only table-visible rows.</span>
            </div>
            <div className="auditCoverageChips" aria-label="Expected audit workflows">
              {EXPECTED_AUDIT_WORKFLOWS.map((workflow) => (
                <span key={workflow}>{workflow}</span>
              ))}
            </div>
          </div>
        </div>
        <div className="auditFilterBar" aria-label="Audit event filters">
          <div className="auditFilterIntro">
            <Filter size={16} />
            <span>Filter audit evidence by actor, action, resource, result, time, source IP, session, and privilege scope.</span>
          </div>
          <label>
            <span>Actor</span>
            <input
              aria-label="Audit actor filter"
              placeholder="operator or actor ID"
              value={auditFilters.actor}
              onChange={(event) => updateAuditFilter("actor", event.target.value)}
            />
          </label>
          <label>
            <span>Action</span>
            <input
              aria-label="Audit action filter"
              placeholder="login, dispatch"
              value={auditFilters.action}
              onChange={(event) => updateAuditFilter("action", event.target.value)}
            />
          </label>
          <label>
            <span>Resource</span>
            <input
              aria-label="Audit resource filter"
              placeholder="target or object"
              value={auditFilters.resource}
              onChange={(event) => updateAuditFilter("resource", event.target.value)}
            />
          </label>
          <label>
            <span>Result</span>
            <input
              aria-label="Audit result filter"
              placeholder="allowed, failed"
              value={auditFilters.result}
              onChange={(event) => updateAuditFilter("result", event.target.value)}
            />
          </label>
          <label>
            <span>IP</span>
            <input
              aria-label="Audit IP filter"
              placeholder="source IP"
              value={auditFilters.ip}
              onChange={(event) => updateAuditFilter("ip", event.target.value)}
            />
          </label>
          <label>
            <span>Session</span>
            <input
              aria-label="Audit session filter"
              placeholder="session ID"
              value={auditFilters.session}
              onChange={(event) => updateAuditFilter("session", event.target.value)}
            />
          </label>
          <label>
            <span>Privilege scope</span>
            <input
              aria-label="Audit privilege scope filter"
              placeholder="audit:read"
              value={auditFilters.privilege}
              onChange={(event) => updateAuditFilter("privilege", event.target.value)}
            />
          </label>
          <label>
            <span>From</span>
            <input
              aria-label="Audit from date"
              type="date"
              value={auditFilters.from}
              onChange={(event) => updateAuditFilter("from", event.target.value)}
            />
          </label>
          <label>
            <span>To</span>
            <input
              aria-label="Audit to date"
              type="date"
              value={auditFilters.to}
              onChange={(event) => updateAuditFilter("to", event.target.value)}
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
              onSelect: (rows) => void copyText(rows.map((audit) => audit.id).join("\n")),
            },
            {
              label: "Copy command hashes",
              onSelect: (rows) => void copyText(rows.map((audit) => audit.command_hash).filter(Boolean).join("\n")),
            },
          ]}
          columns={auditColumns}
          defaultPageSize={12}
          empty={
            <div className="emptyState">
              <ClipboardList size={22} />
              <strong>{hasAuditFilters ? "No matching audit records" : "No audit records returned"}</strong>
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
          renderExpandedRow={(audit) => (
            <div className="gridDetailLine">
              <strong>{audit.action}</strong>
              <span>{audit.target}</span>
              <span>{metadataPreview(audit.metadata)}</span>
              <span>{audit.command_hash ? shortHash(audit.command_hash) : "no command hash"}</span>
            </div>
          )}
          rowActions={[
            {
              icon: <Eye size={14} />,
              label: "Details",
              onSelect: (rows) => {
                if (rows[0]) {
                  setSelectedAuditId(rows[0].id);
                }
              },
            },
          ]}
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
            <span>{historyRetentionPolicies.length} policy domains with export and prune controls</span>
          </div>
          <button className="secondaryAction" disabled={loading} onClick={() => void onExportHistory(exportDomains)} type="button">
            <Download size={16} />
            Export history
          </button>
        </div>
        <div className="retentionComplianceOverview" aria-label="History retention compliance overview">
          <div className="retentionComplianceCard ready">
            <span>Policy domains</span>
            <strong>{enabledPolicyCount} enabled / {historyRetentionPolicies.length} total</strong>
            <p>{exportPolicyCount} export-enabled domains in the current policy set.</p>
          </div>
          <div className={`retentionComplianceCard ${currentRecordLabel === "Not reported" || audits.length === 0 ? "attention" : ""}`}>
            <span>Current records</span>
            <strong>{currentRecordLabel}</strong>
            <p>{currentRecordDetail}</p>
          </div>
          <div className="retentionComplianceCard attention">
            <span>Storage size</span>
            <strong>Not reported</strong>
            <p>History storage bytes are not exposed by the retention API.</p>
          </div>
          <div className="retentionComplianceCard">
            <span>Last export</span>
            <strong>{historyExport ? formatTime(historyExport.generated_at) : "Not exported"}</strong>
            <p>{historyExport ? `${historyExport.domains.length} domains, JSON bundle, limit ${historyExport.limit}/domain.` : "Export creates a JSON bundle for export-enabled domains."}</p>
          </div>
          <div className="retentionComplianceCard">
            <span>Next prune</span>
            <strong>{historyPruneResult ? formatPruneStatus(historyPruneResult) : "Preview required"}</strong>
            <p>{selectedDomainLabel}: retain {retentionDays || selectedPolicy?.retention_days || "-"} days, limit {pruneLimit || selectedPolicy?.prune_limit || "-"} rows per run.</p>
          </div>
          <div className={`retentionComplianceCard ${metadataOnly ? "attention" : ""}`}>
            <span>Metadata only</span>
            <strong>{metadataOnly ? "Rows only" : "Rows and objects"}</strong>
            <p>{metadataOnly ? metadataOnlyExplanation(selectedDomainLabel) : "Cleanup may include retained object references when the backend returns object keys."}</p>
          </div>
          <div className="retentionComplianceCard">
            <span>Cleanup effect</span>
            <strong>{pruneEffectLabel}</strong>
            <p>{historyPruneResult ? formatPruneEffect(historyPruneResult, historyPruneResult.metadata_only_requested ?? metadataOnly) : "Run Preview prune before confirming cleanup."}</p>
          </div>
          <div className="retentionComplianceCard attention">
            <span>Compliance warning</span>
            <strong>{selectedDomainLabel}</strong>
            <p>{complianceWarning}</p>
          </div>
        </div>
        <div className="retentionReviewStrip" aria-label="History retention export scope">
          <div>
            <span>Export scope</span>
            <strong>{exportDomains || "No export-enabled domains"}</strong>
          </div>
          <div>
            <span>Format</span>
            <strong>JSON history bundle</strong>
          </div>
          <div>
            <span>Cleanup review</span>
            <strong>{historyPruneResult ? `${totalMatchedRows(historyPruneResult)} matched rows / ${totalObjectKeys(historyPruneResult)} objects` : "Preview required"}</strong>
          </div>
        </div>
        <div className="retentionBoundaryNotice" aria-label="Retention cleanup boundary">
          <strong>Evidence retention only</strong>
          <span>
            Prune preview applies to selected history domains and reports row/object impact before confirmation. System / Maintenance owns server artifact cleanup jobs.
          </span>
        </div>
        <div className="historyRetentionGrid">
          <label>
            <span>Domain</span>
            <select
              value={selectedPolicy?.domain ?? selectedDomain}
              onChange={(event) => {
                setSelectedDomain(event.target.value);
                clearPolicyConfirmation();
                clearPruneConfirmation();
              }}
            >
              {historyRetentionPolicies.map((policy) => (
                <option key={policy.domain} value={policy.domain}>
                  {policy.domain}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Retention days</span>
            <input
              min={1}
              max={3650}
              type="number"
              value={retentionDays}
              onChange={(event) => {
                setRetentionDays(event.target.value);
                clearPolicyConfirmation();
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
                clearPolicyConfirmation();
                clearPruneConfirmation();
              }}
            />
          </label>
          <label className="checkControl">
            <input
              checked={metadataOnly}
              type="checkbox"
              onChange={(event) => {
                setMetadataOnly(event.target.checked);
                clearPolicyConfirmation();
                clearPruneConfirmation();
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
                clearPolicyConfirmation();
              }}
            />
            <span>Export enabled</span>
          </label>
          <div className="retentionActions">
            <button className="secondaryAction" onClick={submitPolicy} type="button">
              <ShieldCheck size={16} />
              Save
            </button>
            <button className="secondaryAction" onClick={() => void previewPrune(false)} type="button">
              Preview prune
            </button>
            <button className="dangerAction" onClick={() => void previewPrune(true)} type="button">
              <Scissors size={16} />
              Review cleanup
            </button>
          </div>
        </div>
        <ConfirmationPrompt
          confirmLabel="Save retention"
          detail="Saves the selected domain retention policy exactly as reviewed."
          items={[
            { label: "Domain", value: policyConfirmation?.domain ?? selectedDomain },
            { label: "Retention days", value: policyConfirmation?.retention_days ?? retentionDays },
            { label: "Limit", value: policyConfirmation?.prune_limit ?? pruneLimit },
            { label: "Metadata only", value: policyConfirmation?.metadata_only ? "yes" : "no" },
            { label: "Export enabled", value: policyConfirmation?.export_enabled ? "yes" : "no" },
          ]}
          onCancel={clearPolicyConfirmation}
          onConfirm={() => void confirmPolicy()}
          open={policyConfirmation !== null}
          title="Confirm history retention policy"
        />
        <ConfirmationPrompt
          confirmLabel="Prune history"
          detail={
            pruneSnapshot?.request.metadata_only ?? metadataOnly
              ? "Deletes history metadata rows that match the selected domain, retention days, and prune limit."
              : "Deletes history rows and retained object files that match the selected domain, retention days, and prune limit."
          }
          items={[
            { label: "Domain", value: pruneSnapshot?.request.domain ?? selectedDomain },
            { label: "Retention days", value: retentionDays },
            { label: "Limit", value: pruneLimit },
            { label: "Metadata only", value: pruneSnapshot?.request.metadata_only ? "yes" : "no" },
            { label: "Reviewed rows", value: pruneSnapshot?.reviewedRows ?? 0 },
            { label: "Objects", value: pruneSnapshot?.objectCount ?? 0 },
            { label: "Effect", value: pruneSnapshot?.effectLabel ?? "review required" },
            {
              label: "Review hash",
              value: pruneSnapshot?.previewHash ? `${pruneSnapshot.previewHash.slice(0, 12)}...` : "not returned",
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
                <strong>{domain.domain}</strong> {domain.status}: {domain.pruned_rows || domain.matched_rows} rows, {domain.object_keys.length} objects
                {domain.object_delete_attempted ? ", object delete attempted" : ", metadata rows only"}
                {domain.object_delete_errors.length > 0 ? `, ${domain.object_delete_errors.length} delete error${domain.object_delete_errors.length === 1 ? "" : "s"}` : ""}
              </span>
            ))}
          </div>
        )}
        {historyExport && (
          <div className="retentionResult">
            <span>
              <strong>Export</strong> {historyExport.domains.length} domains as JSON at {formatTime(historyExport.generated_at)}; limit {historyExport.limit}/domain; scope {historyExport.domains.join(", ")}
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
    <section className="consoleDetailPanel auditEventDetailPanel" aria-label="Audit event detail">
      <div className="consoleDetailPanelHeader">
        <span>
          <strong>Audit event detail</strong>
          <small>{audit.action} · {formatTime(audit.created_at)}</small>
        </span>
        <button className="secondaryAction compactAction" onClick={onClose} type="button">
          Close
        </button>
      </div>
      <div className="consoleInlineDetailGrid">
        <span>
          <strong>Actor</strong>
          <span>{auditActor(audit) ?? "unknown"}</span>
        </span>
        <span>
          <strong>Target</strong>
          <span>{audit.target}</span>
        </span>
        <span>
          <strong>Result</strong>
          <span>{auditFilterText(audit, "result") || "not recorded"}</span>
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
          <span>{audit.command_hash ? shortHash(audit.command_hash) : "none"}</span>
        </span>
        <span>
          <strong>Event ID</strong>
          <span>{audit.id}</span>
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
  field:
    | "action"
    | "ip"
    | "privilege"
    | "resource"
    | "result"
    | "session",
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
      return metadataFieldText(metadata, [
        "client_ip",
        "ip",
        "remote_addr",
        "remote_ip",
        "request_ip",
        "source_ip",
      ]) ?? metadataJson;
    case "session":
      return metadataFieldText(metadata, [
        "gateway_session_id",
        "session",
        "session_id",
        "terminal_session_id",
      ]) ?? metadataJson;
    case "privilege":
      return metadataFieldText(metadata, [
        "capability",
        "permission",
        "permission_scope",
        "privilege_scope",
        "required_scope",
        "role",
        "scope",
      ]) ?? metadataJson;
  }
}

function metadataFieldText(
  metadata: JsonValue,
  keys: string[],
): string | null {
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

function totalMatchedRows(response: HistoryRetentionPruneResponse): number {
  return response.domains.reduce((sum, domain) => sum + domain.matched_rows, 0);
}

function totalPrunedRows(response: HistoryRetentionPruneResponse): number {
  return response.domains.reduce((sum, domain) => sum + domain.pruned_rows, 0);
}

function totalPrunedOrMatchedRows(
  response: HistoryRetentionPruneResponse,
): number {
  return response.dry_run ? totalMatchedRows(response) : totalPrunedRows(response);
}

function totalObjectKeys(response: HistoryRetentionPruneResponse): number {
  return response.domains.reduce(
    (sum, domain) => sum + domain.object_keys.length,
    0,
  );
}

function formatPruneStatus(response: HistoryRetentionPruneResponse): string {
  return `${response.dry_run ? "Dry run" : "Applied"} ${totalPrunedOrMatchedRows(response)} rows`;
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

function metadataOnlyExplanation(domain: string): string {
  if (domain === "audit_logs") {
    return "Audit logs metadata only preserves the event-ledger model: cleanup prunes database rows, not external object blobs.";
  }
  return "Metadata-only cleanup prunes history rows and leaves retained object blobs untouched.";
}
