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
    previewHash: string;
    request: HistoryRetentionPruneRequest;
    reviewedRows: number;
  } | null>(null);
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
        setPruneSnapshot({
          previewHash: preview.preview_hash,
          request: {
            ...pruneRequest(false),
            confirmed: true,
            preview_hash: preview.preview_hash,
          },
          reviewedRows: preview.domains.reduce(
            (sum, domain) => sum + domain.matched_rows,
            0,
          ),
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
            {
              label: "Review hash",
              value: pruneSnapshot ? `${pruneSnapshot.previewHash.slice(0, 12)}...` : "review required",
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
