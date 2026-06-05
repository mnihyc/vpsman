import { Archive, CalendarClock, GitBranch, RotateCcw } from "lucide-react";
import { CrudPager } from "../../components/CrudPager";
import type {
  BackupArtifactRecord,
  BackupPolicyRecord,
  BackupRequestRecord,
  MigrationLinkRecord,
  RestorePlanRecord,
} from "../../types";
import { formatTime, shortHash, shortId, statusClass } from "../../utils";

export function BackupHistoryTables({
  artifacts,
  backupPolicies,
  backups,
  clientLabel,
  error,
  migrationLinks,
  restorePlans,
}: {
  artifacts: BackupArtifactRecord[];
  backupPolicies: BackupPolicyRecord[];
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  error: string | null;
  migrationLinks: MigrationLinkRecord[];
  restorePlans: RestorePlanRecord[];
}) {
  if (error) {
    return (
      <div className="emptyState">
        <strong>{error}</strong>
        <span>Backup request history is unavailable.</span>
      </div>
    );
  }

  return (
    <>
      <BackupPoliciesTable policies={backupPolicies} />
      {backups.length === 0 ? (
        <div className="emptyState">
          <Archive size={22} />
          <strong>No backup requests</strong>
          <span>Accepted metadata-only requests will appear here.</span>
        </div>
      ) : (
        <BackupRequestsTable backups={backups} clientLabel={clientLabel} />
      )}
      <ArtifactHistoryTable artifacts={artifacts} clientLabel={clientLabel} />
      <RestorePlansTable clientLabel={clientLabel} restorePlans={restorePlans} />
      <MigrationLinksTable clientLabel={clientLabel} migrationLinks={migrationLinks} />
    </>
  );
}

function BackupPoliciesTable({ policies }: { policies: BackupPolicyRecord[] }) {
  return (
    <div className="restoreHistorySection">
      <div className="sectionHeader compact">
        <h2>Policies</h2>
        <span>Scheduled backup selectors materialize as approval-required jobs</span>
      </div>
      <CrudPager
        fields={[
          { label: "Policy", value: (policy) => `${policy.name} ${policy.schedule_id}` },
          { label: "Targets", value: (policy) => policyTargetLabel(policy) },
          { label: "Scope", value: (policy) => policyScopeLabel(policy) },
          { label: "Status", value: (policy) => (policy.enabled ? "enabled" : "disabled") },
          { label: "Retention", value: (policy) => `${policy.retention_days} ${policy.keep_last}` },
        ]}
        itemLabel="policies"
        items={policies}
        pageSize={6}
        title="Backup policy records"
        empty={
          <div className="emptyState compactEmpty">
            <CalendarClock size={20} />
            <strong>No backup policies</strong>
            <span>Saved policy schedules will appear here.</span>
          </div>
        }
      >
        {(policyRows) => (
          <div className="historyTable">
            <div className="historyRow heading backupHistoryGrid">
              <span>Policy</span>
              <span>Targets</span>
              <span>Scope</span>
              <span>Status</span>
              <span>Retention</span>
              <span>Next run</span>
            </div>
            {policyRows.map((policy) => (
              <div className="historyRow backupHistoryGrid" key={policy.schedule_id}>
                <span className="historyPrimary">
                  <strong>{policy.name}</strong>
                  <small>{policy.rotation_generation ?? "default key generation"}</small>
                </span>
                <span>{policyTargetLabel(policy)}</span>
                <span>{policyScopeLabel(policy)}</span>
                <span className={`status ${policy.enabled ? "ok" : "warn"}`}>{policy.enabled ? "enabled" : "disabled"}</span>
                <span>{policy.retention_days}d / {policy.keep_last} kept</span>
                <span>{formatTime(policy.next_run_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
    </div>
  );
}

function BackupRequestsTable({ backups, clientLabel }: { backups: BackupRequestRecord[]; clientLabel: (clientId: string) => string }) {
  return (
    <CrudPager
      fields={[
        { label: "Request", value: (backup) => `${backup.id} ${backup.artifact_id ?? ""}` },
        { label: "Client", value: (backup) => clientLabel(backup.client_id) },
        { label: "Scope", value: (backup) => backupScopeLabel(backup) },
        { label: "Status", value: (backup) => backup.status },
        { label: "Hash", value: (backup) => backup.payload_hash },
      ]}
      itemLabel="requests"
      items={backups}
      pageSize={8}
      title="Backup request records"
      empty={<div className="emptyState compactEmpty">No backup requests match the current search.</div>}
    >
      {(backupRows) => (
        <div className="historyTable">
          <div className="historyRow heading backupHistoryGrid">
            <span>Request</span>
            <span>Client</span>
            <span>Scope</span>
            <span>Status</span>
            <span>Hash</span>
            <span>Created</span>
          </div>
          {backupRows.map((backup) => (
            <div className="historyRow backupHistoryGrid" key={backup.id}>
              <span className="historyPrimary">
                <strong>{shortId(backup.id)}</strong>
                <small>{backup.artifact_id ? `artifact ${shortId(backup.artifact_id)}` : "metadata only"}</small>
              </span>
              <span>{clientLabel(backup.client_id)}</span>
              <span>{backupScopeLabel(backup)}</span>
              <span className={`status ${statusClass(backup.status)}`}>{backup.status}</span>
              <span className="monoValue">{shortHash(backup.payload_hash)}</span>
              <span>{formatTime(backup.created_at)}</span>
            </div>
          ))}
        </div>
      )}
    </CrudPager>
  );
}

function ArtifactHistoryTable({ artifacts, clientLabel }: { artifacts: BackupArtifactRecord[]; clientLabel: (clientId: string) => string }) {
  return (
    <div className="restoreHistorySection">
      <div className="sectionHeader compact">
        <h2>Artifacts</h2>
        <span>Encrypted artifact metadata linked to backup requests</span>
      </div>
      <CrudPager
        fields={[
          { label: "Artifact", value: (artifact) => artifact.id },
          { label: "Client", value: (artifact) => clientLabel(artifact.client_id) },
          { label: "Object key", value: (artifact) => artifact.object_key },
          { label: "Status", value: (artifact) => (artifact.encrypted ? "encrypted" : "plaintext") },
          { label: "Hash", value: (artifact) => artifact.sha256_hex },
        ]}
        itemLabel="artifacts"
        items={artifacts}
        pageSize={8}
        title="Artifact records"
        empty={
          <div className="emptyState compactEmpty">
            <Archive size={20} />
            <strong>No artifacts</strong>
            <span>Recorded artifact metadata will appear here.</span>
          </div>
        }
      >
        {(artifactRows) => (
          <div className="historyTable">
            <div className="historyRow heading backupHistoryGrid">
              <span>Artifact</span>
              <span>Client</span>
              <span>Object key</span>
              <span>Status</span>
              <span>Hash</span>
              <span>Created</span>
            </div>
            {artifactRows.map((artifact) => (
              <div className="historyRow backupHistoryGrid" key={artifact.id}>
                <span className="historyPrimary">
                  <strong>{shortId(artifact.id)}</strong>
                  <small>{formatBytes(artifact.size_bytes)}</small>
                </span>
                <span>{clientLabel(artifact.client_id)}</span>
                <span className="monoValue">{artifact.object_key}</span>
                <span className={`status ${artifact.encrypted ? "ok" : "warn"}`}>
                  {artifact.encrypted ? "encrypted" : "plaintext"}
                </span>
                <span className="monoValue">{shortHash(artifact.sha256_hex)}</span>
                <span>{formatTime(artifact.created_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
    </div>
  );
}

function RestorePlansTable({ restorePlans, clientLabel }: { restorePlans: RestorePlanRecord[]; clientLabel: (clientId: string) => string }) {
  return (
    <div className="restoreHistorySection">
      <div className="sectionHeader compact">
        <h2>Restore plans</h2>
        <span>Proof-gated metadata plans, not executed restores</span>
      </div>
      <CrudPager
        fields={[
          { label: "Plan", value: (plan) => plan.id },
          { label: "Source", value: (plan) => plan.source_backup_request_id },
          { label: "Target", value: (plan) => clientLabel(plan.target_client_id) },
          { label: "Status", value: (plan) => plan.status },
          { label: "Hash", value: (plan) => plan.payload_hash },
          { label: "Scope", value: (plan) => restoreScopeLabel(plan) },
        ]}
        itemLabel="plans"
        items={restorePlans}
        pageSize={8}
        title="Restore plan records"
        empty={
          <div className="emptyState compactEmpty">
            <RotateCcw size={20} />
            <strong>No restore plans</strong>
            <span>Plans will appear here after approval.</span>
          </div>
        }
      >
        {(planRows) => (
          <div className="historyTable">
            <div className="historyRow heading backupHistoryGrid">
              <span>Plan</span>
              <span>Source</span>
              <span>Target</span>
              <span>Status</span>
              <span>Hash</span>
              <span>Created</span>
            </div>
            {planRows.map((plan) => (
              <div className="historyRow backupHistoryGrid" key={plan.id}>
                <span className="historyPrimary">
                  <strong>{shortId(plan.id)}</strong>
                  <small>{restoreScopeLabel(plan)}</small>
                </span>
                <span>{shortId(plan.source_backup_request_id)}</span>
                <span>{clientLabel(plan.target_client_id)}</span>
                <span className={`status ${statusClass(plan.status)}`}>{plan.status}</span>
                <span className="monoValue">{shortHash(plan.payload_hash)}</span>
                <span>{formatTime(plan.created_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
    </div>
  );
}

function MigrationLinksTable({ migrationLinks, clientLabel }: { migrationLinks: MigrationLinkRecord[]; clientLabel: (clientId: string) => string }) {
  return (
    <div className="restoreHistorySection">
      <div className="sectionHeader compact">
        <h2>Migration links</h2>
        <span>Restore plans mapped to replacement VPS identities</span>
      </div>
      <CrudPager
        fields={[
          { label: "Link", value: (link) => `${link.id} ${link.restore_plan_id}` },
          { label: "Source", value: (link) => clientLabel(link.source_client_id) },
          { label: "Target", value: (link) => clientLabel(link.target_client_id) },
          { label: "Status", value: (link) => link.status },
          { label: "Scope", value: (link) => migrationScopeLabel(link) },
        ]}
        itemLabel="links"
        items={migrationLinks}
        pageSize={8}
        title="Migration link records"
        empty={
          <div className="emptyState compactEmpty">
            <GitBranch size={20} />
            <strong>No migration links</strong>
            <span>Accepted migration links will appear here.</span>
          </div>
        }
      >
        {(linkRows) => (
          <div className="historyTable">
            <div className="historyRow heading backupHistoryGrid">
              <span>Link</span>
              <span>Source</span>
              <span>Target</span>
              <span>Status</span>
              <span>Scope</span>
              <span>Created</span>
            </div>
            {linkRows.map((link) => (
              <div className="historyRow backupHistoryGrid" key={link.id}>
                <span className="historyPrimary">
                  <strong>{shortId(link.id)}</strong>
                  <small>plan {shortId(link.restore_plan_id)}</small>
                </span>
                <span>{clientLabel(link.source_client_id)}</span>
                <span>{clientLabel(link.target_client_id)}</span>
                <span className={`status ${statusClass(link.status)}`}>{link.status}</span>
                <span>{migrationScopeLabel(link)}</span>
                <span>{formatTime(link.created_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
    </div>
  );
}

function backupScopeLabel(backup: BackupRequestRecord): string {
  const scopes = [];
  if (backup.include_config) {
    scopes.push("config");
  }
  if (backup.paths.length > 0) {
    scopes.push(`${backup.paths.length} path${backup.paths.length === 1 ? "" : "s"}`);
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function policyTargetLabel(policy: BackupPolicyRecord): string {
  const parts = [];
  if (policy.clients.length > 0) {
    parts.push(`${policy.clients.length} client${policy.clients.length === 1 ? "" : "s"}`);
  }
  if (policy.tags.length > 0) {
    parts.push(`${policy.tags.length} tag${policy.tags.length === 1 ? "" : "s"}`);
  }
  return parts.length > 0 ? parts.join(" + ") : "none";
}

function policyScopeLabel(policy: BackupPolicyRecord): string {
  const scopes = [];
  if (policy.include_config) {
    scopes.push("config");
  }
  if (policy.paths.length > 0) {
    scopes.push(`${policy.paths.length} path${policy.paths.length === 1 ? "" : "s"}`);
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function restoreScopeLabel(plan: RestorePlanRecord): string {
  const scopes = [];
  if (plan.include_config) {
    scopes.push("config");
  }
  if (plan.paths.length > 0) {
    scopes.push(`${plan.paths.length} path${plan.paths.length === 1 ? "" : "s"}`);
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function migrationScopeLabel(link: MigrationLinkRecord): string {
  const scopes = [];
  if (link.include_config) {
    scopes.push("config");
  }
  if (link.paths.length > 0) {
    scopes.push(`${link.paths.length} path${link.paths.length === 1 ? "" : "s"}`);
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  if (value < 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  }
  return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
}
