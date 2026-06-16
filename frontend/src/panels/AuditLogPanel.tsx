import { ClipboardList, Download, Scissors, ShieldCheck } from "lucide-react";
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
} from "../types";
import { formatTime, metadataOperator, metadataPreview, shortHash, shortId } from "../utils";

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
  onPruneHistoryRetention: (request: HistoryRetentionPruneRequest) => Promise<void>;
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
  const [pruneConfirmationOpen, setPruneConfirmationOpen] = useState(false);

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

  const submitPolicy = () => {
    void onUpsertHistoryRetentionPolicy({
      domain: selectedPolicy?.domain ?? selectedDomain,
      retention_days: Number(retentionDays),
      prune_limit: Number(pruneLimit),
      metadata_only: metadataOnly,
      export_enabled: exportEnabled,
      confirmed: true,
    });
  };

  const prune = (dryRun: boolean) => {
    if (!dryRun && !pruneConfirmationOpen) {
      setPruneConfirmationOpen(true);
      return;
    }
    void onPruneHistoryRetention({
      domain: selectedPolicy?.domain ?? selectedDomain,
      dry_run: dryRun,
      metadata_only: metadataOnly,
      confirmed: !dryRun,
    });
    if (!dryRun) {
      setPruneConfirmationOpen(false);
    }
  };

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
              <strong>No audit records</strong>
              <span>{error ?? "No audit records match the current search."}</span>
            </div>
          }
          getRowId={(audit) => audit.id}
          itemLabel="records"
          renderExpandedRow={(audit) => (
            <div className="gridDetailLine">
              <strong>{audit.action}</strong>
              <span>{audit.target}</span>
              <span>{metadataPreview(audit.metadata)}</span>
              <span>{audit.command_hash ? shortHash(audit.command_hash) : "no command hash"}</span>
            </div>
          )}
          rows={audits}
          storageKey="vpsman.grid.audit.events"
          title="Audit records"
        />
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
            Export
          </button>
        </div>
        <div className="historyRetentionGrid">
          <label>
            <span>Domain</span>
            <select value={selectedPolicy?.domain ?? selectedDomain} onChange={(event) => setSelectedDomain(event.target.value)}>
              {historyRetentionPolicies.map((policy) => (
                <option key={policy.domain} value={policy.domain}>
                  {policy.domain}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Retention days</span>
            <input min={1} max={3650} type="number" value={retentionDays} onChange={(event) => setRetentionDays(event.target.value)} />
          </label>
          <label>
            <span>Prune limit</span>
            <input min={1} max={100000} type="number" value={pruneLimit} onChange={(event) => setPruneLimit(event.target.value)} />
          </label>
          <label className="checkControl">
            <input checked={metadataOnly} type="checkbox" onChange={(event) => setMetadataOnly(event.target.checked)} />
            <span>Metadata only</span>
          </label>
          <label className="checkControl">
            <input checked={exportEnabled} type="checkbox" onChange={(event) => setExportEnabled(event.target.checked)} />
            <span>Export enabled</span>
          </label>
          <div className="retentionActions">
            <button className="secondaryAction" onClick={submitPolicy} type="button">
              <ShieldCheck size={16} />
              Save
            </button>
            <button className="secondaryAction" onClick={() => prune(true)} type="button">
              Review prune
            </button>
            <button className="dangerAction" onClick={() => prune(false)} type="button">
              <Scissors size={16} />
              Review prune
            </button>
          </div>
        </div>
        <ConfirmationPrompt
          confirmLabel="Prune history"
          detail={
            metadataOnly
              ? "Deletes history metadata rows that match the selected domain, retention days, and prune limit."
              : "Deletes history rows and retained object files that match the selected domain, retention days, and prune limit."
          }
          items={[
            { label: "Domain", value: selectedPolicy?.domain ?? selectedDomain },
            { label: "Retention days", value: retentionDays },
            { label: "Limit", value: pruneLimit },
            { label: "Metadata only", value: metadataOnly ? "yes" : "no" },
          ]}
          onCancel={() => setPruneConfirmationOpen(false)}
          onConfirm={() => prune(false)}
          open={pruneConfirmationOpen}
          title="Confirm history prune"
          tone="danger"
        />
        {historyPruneResult && (
          <div className="retentionResult">
            {historyPruneResult.domains.slice(0, 4).map((domain) => (
              <span key={domain.domain}>
                <strong>{domain.domain}</strong> {domain.status}: {domain.pruned_rows || domain.matched_rows} rows
                {domain.object_delete_errors.length > 0 ? `, ${domain.object_delete_errors.length} delete error${domain.object_delete_errors.length === 1 ? "" : "s"}` : ""}
              </span>
            ))}
          </div>
        )}
        {historyExport && (
          <div className="retentionResult">
            <span>
              <strong>Export</strong> {historyExport.domains.join(", ")} at {formatTime(historyExport.generated_at)}
            </span>
          </div>
        )}
      </div>
      )}
    </section>
  );
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}
