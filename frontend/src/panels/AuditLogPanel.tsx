import { ClipboardList, Download, Scissors, ShieldCheck } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { CrudPager } from "../components/CrudPager";
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
  const [selectedDomain, setSelectedDomain] = useState("audit_logs");
  const selectedPolicy = useMemo(
    () => historyRetentionPolicies.find((policy) => policy.domain === selectedDomain) ?? historyRetentionPolicies[0],
    [historyRetentionPolicies, selectedDomain],
  );
  const [retentionDays, setRetentionDays] = useState("365");
  const [pruneLimit, setPruneLimit] = useState("1000");
  const [metadataOnly, setMetadataOnly] = useState(false);
  const [exportEnabled, setExportEnabled] = useState(true);

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
    void onPruneHistoryRetention({
      domain: selectedPolicy?.domain ?? selectedDomain,
      dry_run: dryRun,
      metadata_only: metadataOnly,
      confirmed: !dryRun,
    });
  };

  return (
    <section className="workspace singleColumn">
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
              Dry run
            </button>
            <button className="dangerAction" onClick={() => prune(false)} type="button">
              <Scissors size={16} />
              Prune
            </button>
          </div>
        </div>
        {historyPruneResult && (
          <div className="retentionResult">
            {historyPruneResult.domains.slice(0, 4).map((domain) => (
              <span key={domain.domain}>
                <strong>{domain.domain}</strong> {domain.status}: {domain.pruned_rows || domain.matched_rows} rows
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
        <CrudPager
          fields={[
            { label: "Action", value: (audit) => audit.action },
            { label: "Target", value: (audit) => audit.target },
            { label: "Operator", value: (audit) => metadataOperator(audit.metadata) ?? audit.actor_id },
            { label: "Command", value: (audit) => audit.command_hash },
            { label: "Created", value: (audit) => audit.created_at },
          ]}
          itemLabel="records"
          items={audits}
          pageSize={12}
          title="Audit records"
          empty={
            <div className="emptyState">
              <ClipboardList size={22} />
              <strong>No audit records</strong>
              <span>{error ?? "No audit records match the current search."}</span>
            </div>
          }
        >
          {(auditRows) => (
            <div className="table historyTable">
              <div className="historyRow heading auditHistoryGrid">
                <span>Action</span>
                <span>Target</span>
                <span>Operator</span>
                <span>Command</span>
                <span>Created</span>
              </div>
              {auditRows.map((audit) => (
                <div className="historyRow auditHistoryGrid" key={audit.id}>
                  <span className="historyPrimary">
                    <strong>{audit.action}</strong>
                    <small>{shortId(audit.id)}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{audit.target}</strong>
                    <small>{metadataPreview(audit.metadata)}</small>
                  </span>
                  <span>{metadataOperator(audit.metadata) ?? shortId(audit.actor_id)}</span>
                  <span className="monoValue">{audit.command_hash ? shortHash(audit.command_hash) : "-"}</span>
                  <span>{formatTime(audit.created_at)}</span>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>
    </section>
  );
}
