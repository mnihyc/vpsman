import { Archive, CalendarClock, GitBranch, RotateCcw } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import type {
  BackupArtifactRecord,
  BackupPolicyRecord,
  BackupRequestRecord,
  MigrationLinkRecord,
  RestorePlanRecord,
} from "../../types";
import {
  formatCompactTime,
  formatTime,
  shortHash,
  shortId,
  statusClass,
} from "../../utils";

export function BackupHistoryTables({
  activeSubpage,
  artifacts,
  backupPolicies,
  backups,
  clientLabel,
  error,
  migrationLinks,
  restorePlans,
}: {
  activeSubpage: string;
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

  if (activeSubpage === "policies") {
    return <BackupPoliciesTable policies={backupPolicies} />;
  }
  if (activeSubpage === "artifacts") {
    return (
      <ArtifactHistoryTable artifacts={artifacts} clientLabel={clientLabel} />
    );
  }
  if (activeSubpage === "restore") {
    return (
      <RestorePlansTable
        clientLabel={clientLabel}
        restorePlans={restorePlans}
      />
    );
  }
  if (activeSubpage === "migration") {
    return (
      <MigrationLinksTable
        clientLabel={clientLabel}
        migrationLinks={migrationLinks}
      />
    );
  }
  return <BackupRequestsTable backups={backups} clientLabel={clientLabel} />;
}

function BackupPoliciesTable({ policies }: { policies: BackupPolicyRecord[] }) {
  const columns: ConsoleDataGridColumn<BackupPolicyRecord>[] = [
    {
      id: "policy",
      header: "Policy",
      size: 220,
      sortValue: (policy) => policy.name,
      searchValue: (policy) =>
        `${policy.name} ${policy.schedule_id} ${policy.rotation_generation ?? ""}`,
      cell: (policy) => (
        <span className="historyPrimary">
          <strong>{policy.name}</strong>
          <small>
            {policy.rotation_generation ?? "default key generation"}
          </small>
        </span>
      ),
    },
    {
      id: "targets",
      header: "Targets",
      size: 130,
      sortValue: policyTargetLabel,
      searchValue: policyTargetLabel,
      cell: policyTargetLabel,
    },
    {
      id: "scope",
      header: "Scope",
      size: 130,
      sortValue: policyScopeLabel,
      searchValue: policyScopeLabel,
      cell: policyScopeLabel,
    },
    {
      id: "status",
      header: "Status",
      size: 120,
      sortValue: (policy) => (policy.enabled ? "enabled" : "disabled"),
      searchValue: (policy) => (policy.enabled ? "enabled" : "disabled"),
      cell: (policy) => (
        <span className={`status ${policy.enabled ? "ok" : "warn"}`}>
          {policy.enabled ? "enabled" : "disabled"}
        </span>
      ),
    },
    {
      id: "retention",
      header: "Retention",
      size: 130,
      sortValue: (policy) => policy.retention_days,
      searchValue: (policy) => `${policy.retention_days} ${policy.keep_last}`,
      cell: (policy) => `${policy.retention_days}d / ${policy.keep_last} kept`,
    },
    {
      id: "nextRun",
      header: "Next run",
      size: 170,
      sortValue: (policy) => policy.next_run_at,
      searchValue: (policy) => policy.next_run_at,
      cell: (policy) => formatTime(policy.next_run_at),
    },
  ];
  return (
    <GridSection
      title="Policies"
      summary="Scheduled backup selectors materialize as approval-required jobs"
    >
      <ConsoleDataGrid
        actions={[
          copyIdsAction<BackupPolicyRecord>(
            "Copy schedule IDs",
            (policy) => policy.schedule_id,
          ),
        ]}
        columns={columns}
        defaultPageSize={6}
        empty={
          <GridEmpty
            icon={<CalendarClock size={20} />}
            title="No backup policies"
            text="Saved policy schedules will appear here."
          />
        }
        getRowId={(policy) => policy.schedule_id}
        itemLabel="policies"
        renderExpandedRow={(policy) => (
          <div className="gridDetailLine">
            <strong>{policy.name}</strong>
            <span>{policyScopeLabel(policy)}</span>
            <span>{policyTargetLabel(policy)}</span>
            <span>{policy.retention_days}d retention</span>
          </div>
        )}
        rows={policies}
        storageKey="vpsman.grid.backups.policies"
        title="Backup policy records"
      />
    </GridSection>
  );
}

function BackupRequestsTable({
  backups,
  clientLabel,
}: {
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
}) {
  const columns: ConsoleDataGridColumn<BackupRequestRecord>[] = [
    {
      id: "request",
      header: "Request",
      size: 155,
      minSize: 135,
      sortValue: (backup) => backup.id,
      searchValue: (backup) => `${backup.id} ${backup.artifact_id ?? ""}`,
      cell: (backup) => (
        <span className="historyPrimary">
          <strong>{shortId(backup.id)}</strong>
          <small>
            {backup.artifact_id
              ? `artifact ${shortId(backup.artifact_id)}`
              : "metadata only"}
          </small>
        </span>
      ),
    },
    {
      id: "client",
      header: "Client",
      size: 190,
      minSize: 155,
      sortValue: (backup) => clientLabel(backup.client_id),
      searchValue: (backup) => clientLabel(backup.client_id),
      cell: (backup) => clientLabel(backup.client_id),
    },
    {
      id: "scope",
      header: "Scope",
      size: 90,
      minSize: 80,
      sortValue: backupScopeLabel,
      searchValue: backupScopeLabel,
      cell: backupScopeLabel,
    },
    {
      id: "status",
      header: "Status",
      size: 140,
      minSize: 120,
      sortValue: (backup) => backup.status,
      searchValue: (backup) => backup.status,
      cell: (backup) => (
        <span
          className={`status ${statusClass(backup.status)}`}
          title={backup.status}
        >
          {backupStatusLabel(backup.status)}
        </span>
      ),
    },
    {
      id: "hash",
      header: "Hash",
      size: 85,
      minSize: 80,
      sortValue: (backup) => backup.payload_hash,
      searchValue: (backup) => backup.payload_hash,
      cell: (backup) => (
        <span className="monoValue">{shortHash(backup.payload_hash)}</span>
      ),
    },
    {
      id: "created",
      header: "Created",
      size: 125,
      minSize: 115,
      sortValue: (backup) => backup.created_at,
      searchValue: (backup) => backup.created_at,
      cell: (backup) => formatCompactTime(backup.created_at),
    },
  ];
  return (
    <ConsoleDataGrid
      actions={[
        copyIdsAction<BackupRequestRecord>(
          "Copy request IDs",
          (backup) => backup.id,
        ),
        copyIdsAction<BackupRequestRecord>(
          "Copy payload hashes",
          (backup) => backup.payload_hash,
        ),
      ]}
      columns={columns}
      defaultColumnVisibility={{ created: false, hash: false }}
      defaultPageSize={8}
      empty={
        <GridEmpty
          icon={<Archive size={20} />}
          title="No backup requests"
          text="Accepted metadata-only requests will appear here."
        />
      }
      getRowId={(backup) => backup.id}
      itemLabel="requests"
      renderExpandedRow={(backup) => (
        <div className="gridDetailLine">
          <strong>{clientLabel(backup.client_id)}</strong>
          <span>{backupScopeLabel(backup)}</span>
          <span>{backupStatusLabel(backup.status)}</span>
          <span>{shortHash(backup.payload_hash)}</span>
          <span>{formatTime(backup.created_at)}</span>
        </div>
      )}
      rows={backups}
      storageKey="vpsman.grid.backups.requests"
      title="Backup request records"
    />
  );
}

function ArtifactHistoryTable({
  artifacts,
  clientLabel,
}: {
  artifacts: BackupArtifactRecord[];
  clientLabel: (clientId: string) => string;
}) {
  const columns: ConsoleDataGridColumn<BackupArtifactRecord>[] = [
    {
      id: "artifact",
      header: "Artifact",
      size: 150,
      minSize: 130,
      sortValue: (artifact) => artifact.id,
      searchValue: (artifact) => artifact.id,
      cell: (artifact) => (
        <span className="historyPrimary">
          <strong>{shortId(artifact.id)}</strong>
          <small>{formatBytes(artifact.size_bytes)}</small>
        </span>
      ),
    },
    {
      id: "client",
      header: "Client",
      size: 180,
      minSize: 150,
      sortValue: (artifact) => clientLabel(artifact.client_id),
      searchValue: (artifact) => clientLabel(artifact.client_id),
      cell: (artifact) => clientLabel(artifact.client_id),
    },
    {
      id: "object",
      header: "Object key",
      size: 300,
      minSize: 180,
      sortValue: (artifact) => artifact.object_key,
      searchValue: (artifact) => artifact.object_key,
      cell: (artifact) => (
        <span className="monoValue">{artifact.object_key}</span>
      ),
    },
    {
      id: "status",
      header: "Status",
      size: 115,
      minSize: 105,
      sortValue: (artifact) => (artifact.encrypted ? "encrypted" : "plaintext"),
      searchValue: (artifact) =>
        artifact.encrypted ? "encrypted" : "plaintext",
      cell: (artifact) => (
        <span className={`status ${artifact.encrypted ? "ok" : "warn"}`}>
          {artifact.encrypted ? "encrypted" : "plaintext"}
        </span>
      ),
    },
    {
      id: "hash",
      header: "Hash",
      size: 85,
      minSize: 80,
      sortValue: (artifact) => artifact.sha256_hex,
      searchValue: (artifact) => artifact.sha256_hex,
      cell: (artifact) => (
        <span className="monoValue">{shortHash(artifact.sha256_hex)}</span>
      ),
    },
    {
      id: "created",
      header: "Created",
      size: 125,
      minSize: 115,
      sortValue: (artifact) => artifact.created_at,
      searchValue: (artifact) => artifact.created_at,
      cell: (artifact) => formatCompactTime(artifact.created_at),
    },
  ];
  return (
    <GridSection
      title="Artifacts"
      summary="Encrypted artifact metadata linked to backup requests"
    >
      <ConsoleDataGrid
        actions={[
          copyIdsAction<BackupArtifactRecord>(
            "Copy artifact IDs",
            (artifact) => artifact.id,
          ),
          copyIdsAction<BackupArtifactRecord>(
            "Copy object keys",
            (artifact) => artifact.object_key,
          ),
        ]}
        columns={columns}
        defaultColumnVisibility={{ created: false, hash: false }}
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<Archive size={20} />}
            title="No artifacts"
            text="Recorded artifact metadata will appear here."
          />
        }
        getRowId={(artifact) => artifact.id}
        itemLabel="artifacts"
        renderExpandedRow={(artifact) => (
          <div className="gridDetailLine">
            <strong>{clientLabel(artifact.client_id)}</strong>
            <span>{formatBytes(artifact.size_bytes)}</span>
            <span>{artifact.object_key}</span>
            <span>{shortHash(artifact.sha256_hex)}</span>
            <span>{formatTime(artifact.created_at)}</span>
          </div>
        )}
        rows={artifacts}
        storageKey="vpsman.grid.backups.artifacts"
        title="Artifact records"
      />
    </GridSection>
  );
}

function RestorePlansTable({
  restorePlans,
  clientLabel,
}: {
  restorePlans: RestorePlanRecord[];
  clientLabel: (clientId: string) => string;
}) {
  const columns: ConsoleDataGridColumn<RestorePlanRecord>[] = [
    {
      id: "plan",
      header: "Plan",
      size: 150,
      minSize: 130,
      sortValue: (plan) => plan.id,
      searchValue: (plan) => `${plan.id} ${plan.source_backup_request_id}`,
      cell: (plan) => (
        <span className="historyPrimary">
          <strong>{shortId(plan.id)}</strong>
          <small>{restoreScopeLabel(plan)}</small>
        </span>
      ),
    },
    {
      id: "source",
      header: "Source",
      size: 125,
      minSize: 110,
      sortValue: (plan) => plan.source_backup_request_id,
      searchValue: (plan) => plan.source_backup_request_id,
      cell: (plan) => shortId(plan.source_backup_request_id),
    },
    {
      id: "target",
      header: "Target",
      size: 190,
      minSize: 155,
      sortValue: (plan) => clientLabel(plan.target_client_id),
      searchValue: (plan) => clientLabel(plan.target_client_id),
      cell: (plan) => clientLabel(plan.target_client_id),
    },
    {
      id: "status",
      header: "Status",
      size: 120,
      minSize: 105,
      sortValue: (plan) => plan.status,
      searchValue: (plan) => plan.status,
      cell: (plan) => (
        <span
          className={`status ${statusClass(plan.status)}`}
          title={plan.status}
        >
          {backupStatusLabel(plan.status)}
        </span>
      ),
    },
    {
      id: "hash",
      header: "Hash",
      size: 85,
      minSize: 80,
      sortValue: (plan) => plan.payload_hash,
      searchValue: (plan) => plan.payload_hash,
      cell: (plan) => (
        <span className="monoValue">{shortHash(plan.payload_hash)}</span>
      ),
    },
    {
      id: "created",
      header: "Created",
      size: 125,
      minSize: 115,
      sortValue: (plan) => plan.created_at,
      searchValue: (plan) => plan.created_at,
      cell: (plan) => formatCompactTime(plan.created_at),
    },
  ];
  return (
    <GridSection
      title="Restore plans"
      summary="Proof-gated metadata plans, not executed restores"
    >
      <ConsoleDataGrid
        actions={[
          copyIdsAction<RestorePlanRecord>(
            "Copy restore plan IDs",
            (plan) => plan.id,
          ),
        ]}
        columns={columns}
        defaultColumnVisibility={{ created: false, hash: false }}
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<RotateCcw size={20} />}
            title="No restore plans"
            text="Plans will appear here after approval."
          />
        }
        getRowId={(plan) => plan.id}
        itemLabel="plans"
        renderExpandedRow={(plan) => (
          <div className="gridDetailLine">
            <strong>{clientLabel(plan.target_client_id)}</strong>
            <span>{restoreScopeLabel(plan)}</span>
            <span>{backupStatusLabel(plan.status)}</span>
            <span>{shortHash(plan.payload_hash)}</span>
            <span>{formatTime(plan.created_at)}</span>
          </div>
        )}
        rows={restorePlans}
        storageKey="vpsman.grid.backups.restorePlans"
        title="Restore plan records"
      />
    </GridSection>
  );
}

function MigrationLinksTable({
  migrationLinks,
  clientLabel,
}: {
  migrationLinks: MigrationLinkRecord[];
  clientLabel: (clientId: string) => string;
}) {
  const columns: ConsoleDataGridColumn<MigrationLinkRecord>[] = [
    {
      id: "link",
      header: "Link",
      size: 150,
      minSize: 130,
      sortValue: (link) => link.id,
      searchValue: (link) => `${link.id} ${link.restore_plan_id}`,
      cell: (link) => (
        <span className="historyPrimary">
          <strong>{shortId(link.id)}</strong>
          <small>plan {shortId(link.restore_plan_id)}</small>
        </span>
      ),
    },
    {
      id: "source",
      header: "Source",
      size: 180,
      minSize: 145,
      sortValue: (link) => clientLabel(link.source_client_id),
      searchValue: (link) => clientLabel(link.source_client_id),
      cell: (link) => clientLabel(link.source_client_id),
    },
    {
      id: "target",
      header: "Target",
      size: 180,
      minSize: 145,
      sortValue: (link) => clientLabel(link.target_client_id),
      searchValue: (link) => clientLabel(link.target_client_id),
      cell: (link) => clientLabel(link.target_client_id),
    },
    {
      id: "status",
      header: "Status",
      size: 120,
      minSize: 105,
      sortValue: (link) => link.status,
      searchValue: (link) => link.status,
      cell: (link) => (
        <span
          className={`status ${statusClass(link.status)}`}
          title={link.status}
        >
          {backupStatusLabel(link.status)}
        </span>
      ),
    },
    {
      id: "scope",
      header: "Scope",
      size: 110,
      minSize: 90,
      sortValue: migrationScopeLabel,
      searchValue: migrationScopeLabel,
      cell: migrationScopeLabel,
    },
    {
      id: "created",
      header: "Created",
      size: 125,
      minSize: 115,
      sortValue: (link) => link.created_at,
      searchValue: (link) => link.created_at,
      cell: (link) => formatCompactTime(link.created_at),
    },
  ];
  return (
    <GridSection
      title="Migration links"
      summary="Restore plans mapped to replacement VPS identities"
    >
      <ConsoleDataGrid
        actions={[
          copyIdsAction<MigrationLinkRecord>(
            "Copy migration link IDs",
            (link) => link.id,
          ),
        ]}
        columns={columns}
        defaultColumnVisibility={{ created: false }}
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<GitBranch size={20} />}
            title="No migration links"
            text="Accepted migration links will appear here."
          />
        }
        getRowId={(link) => link.id}
        itemLabel="links"
        renderExpandedRow={(link) => (
          <div className="gridDetailLine">
            <strong>
              {clientLabel(link.source_client_id)} to{" "}
              {clientLabel(link.target_client_id)}
            </strong>
            <span>{migrationScopeLabel(link)}</span>
            <span>{backupStatusLabel(link.status)}</span>
            <span>{formatTime(link.created_at)}</span>
          </div>
        )}
        rows={migrationLinks}
        storageKey="vpsman.grid.backups.migrations"
        title="Migration link records"
      />
    </GridSection>
  );
}

function GridSection({
  children,
  summary,
  title,
}: {
  children: React.ReactNode;
  summary: string;
  title: string;
}) {
  return (
    <div className="restoreHistorySection">
      <div className="sectionHeader compact">
        <h2>{title}</h2>
        <span>{summary}</span>
      </div>
      {children}
    </div>
  );
}

function GridEmpty({
  icon,
  text,
  title,
}: {
  icon: React.ReactNode;
  text: string;
  title: string;
}) {
  return (
    <div className="emptyState compactEmpty">
      {icon}
      <strong>{title}</strong>
      <span>{text}</span>
    </div>
  );
}

function copyIdsAction<T>(label: string, value: (row: T) => string) {
  return {
    label,
    onSelect: (rows: T[]) => void copyText(rows.map(value).join("\n")),
  };
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function backupScopeLabel(backup: BackupRequestRecord): string {
  const scopes = [];
  if (backup.include_config) {
    scopes.push("config");
  }
  if (backup.paths.length > 0) {
    scopes.push(
      `${backup.paths.length} path${backup.paths.length === 1 ? "" : "s"}`,
    );
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function policyTargetLabel(policy: BackupPolicyRecord): string {
  const parts = [];
  if (policy.clients.length > 0) {
    parts.push(
      `${policy.clients.length} client${policy.clients.length === 1 ? "" : "s"}`,
    );
  }
  if (policy.tags.length > 0) {
    parts.push(
      `${policy.tags.length} tag${policy.tags.length === 1 ? "" : "s"}`,
    );
  }
  return parts.length > 0 ? parts.join(" + ") : "none";
}

function policyScopeLabel(policy: BackupPolicyRecord): string {
  const scopes = [];
  if (policy.include_config) {
    scopes.push("config");
  }
  if (policy.paths.length > 0) {
    scopes.push(
      `${policy.paths.length} path${policy.paths.length === 1 ? "" : "s"}`,
    );
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function restoreScopeLabel(plan: RestorePlanRecord): string {
  const scopes = [];
  if (plan.include_config) {
    scopes.push("config");
  }
  if (plan.paths.length > 0) {
    scopes.push(
      `${plan.paths.length} path${plan.paths.length === 1 ? "" : "s"}`,
    );
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function migrationScopeLabel(link: MigrationLinkRecord): string {
  const scopes = [];
  if (link.include_config) {
    scopes.push("config");
  }
  if (link.paths.length > 0) {
    scopes.push(
      `${link.paths.length} path${link.paths.length === 1 ? "" : "s"}`,
    );
  }
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function backupStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    artifact_metadata_recorded: "artifact ready",
    linked_metadata_only: "linked",
    planned_metadata_only: "planned",
  };
  return labels[status] ?? status.replace(/_/g, " ");
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
