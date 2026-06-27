import { Archive, CalendarClock, Download, ExternalLink, GitBranch, RotateCcw } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  artifactLifecycleStatusBadgeClass,
  backupRequestStatusBadgeClass,
  migrationLinkStatusBadgeClass,
  restorePlanStatusBadgeClass,
} from "../../jobStatusPresentation";
import type {
  BackupArtifactRecord,
  BackupPolicyRecord,
  BackupRequestRecord,
  MigrationLinkRecord,
  RestorePlanRecord,
} from "../../types";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  shortHash,
  shortId,
} from "../../utils";

export function BackupHistoryTables({
  activeSubpage,
  artifacts,
  backupPolicies,
  backups,
  clientLabel,
  error,
  migrationLinks,
  onDownloadArtifact,
  onOpenRequestArtifact,
  onPlanRestoreSource,
  onRestoreArtifact,
  onRetryBackup,
  restorePlans,
}: {
  activeSubpage: string;
  artifacts: BackupArtifactRecord[];
  backupPolicies: BackupPolicyRecord[];
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  error: string | null;
  migrationLinks: MigrationLinkRecord[];
  onDownloadArtifact?: (
    artifact: BackupArtifactRecord,
    backup: BackupRequestRecord | null,
  ) => void;
  onOpenRequestArtifact?: (backup: BackupRequestRecord) => void;
  onPlanRestoreSource?: (backup: BackupRequestRecord) => void;
  onRestoreArtifact?: (
    artifact: BackupArtifactRecord,
    backup: BackupRequestRecord | null,
  ) => void;
  onRetryBackup?: (backup: BackupRequestRecord) => void;
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
      <ArtifactHistoryTable
        artifacts={artifacts}
        backups={backups}
        clientLabel={clientLabel}
        onDownloadArtifact={onDownloadArtifact}
        onRestoreArtifact={onRestoreArtifact}
      />
    );
  }
  if (activeSubpage === "restore") {
    return (
      <RestoreSourcesTable
        artifacts={artifacts}
        backups={backups}
        clientLabel={clientLabel}
        onPlanRestoreSource={onPlanRestoreSource}
        restorePlans={restorePlans}
      />
    );
  }
  if (activeSubpage === "migration") {
    return (
      <MigrationLinksTable
        artifacts={artifacts}
        backups={backups}
        clientLabel={clientLabel}
        migrationLinks={migrationLinks}
        restorePlans={restorePlans}
      />
    );
  }
  return (
    <BackupRequestsTable
      artifacts={artifacts}
      backups={backups}
      clientLabel={clientLabel}
      onOpenRequestArtifact={onOpenRequestArtifact}
      onRetryBackup={onRetryBackup}
    />
  );
}

function BackupPoliciesTable({ policies }: { policies: BackupPolicyRecord[] }) {
  const columns: ConsoleDataGridColumn<BackupPolicyRecord>[] = [
    {
      id: "policy",
      header: "Name",
      size: 180,
      minSize: 150,
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
      size: 170,
      minSize: 140,
      sortValue: (policy) => policy.target_client_ids.length,
      searchValue: (policy) =>
        `${policyTargetLabel(policy)} ${policy.selector_expression} ${policy.target_client_ids.join(" ")}`,
      cell: (policy) => (
        <span className="historyPrimary">
          <strong>{policyTargetCountLabel(policy)}</strong>
          <small title={policy.selector_expression}>
            {policy.selector_expression || "no selector"}
          </small>
        </span>
      ),
    },
    {
      id: "frequency",
      header: "Frequency",
      size: 170,
      minSize: 140,
      sortValue: (policy) => policy.cron_expr,
      searchValue: (policy) =>
        `${policy.cron_expr} ${describeCronExpression(policy.cron_expr)} ${policy.timezone}`,
      cell: (policy) => (
        <span className="historyPrimary">
          <strong>{describeCronExpression(policy.cron_expr)}</strong>
          <small>
            {policy.cron_expr} · {policy.timezone}
          </small>
        </span>
      ),
    },
    {
      id: "nextRun",
      header: "Next run",
      size: 135,
      minSize: 115,
      sortValue: (policy) => policy.next_run_at,
      searchValue: (policy) =>
        `${policy.next_run_at} ${policyNextRunLabel(policy.next_run_at)}`,
      cell: (policy) => {
        const nextRun = policyNextRunState(policy.next_run_at);
        return (
          <span className="historyPrimary">
            <strong
              className={`status ${nextRun.tone}`}
              title={formatFullTime(policy.next_run_at)}
            >
              {nextRun.label}
            </strong>
            <small>{nextRun.detail}</small>
          </span>
        );
      },
    },
    {
      id: "retention",
      header: "Retention",
      size: 120,
      minSize: 105,
      sortValue: (policy) => policy.retention_days,
      searchValue: (policy) => `${policy.retention_days} ${policy.keep_last}`,
      cell: (policy) => (
        <span className="historyPrimary">
          <strong>{policy.retention_days}d</strong>
          <small>{policy.keep_last} kept</small>
        </span>
      ),
    },
    {
      id: "lastResult",
      header: "Last result",
      size: 150,
      minSize: 125,
      sortValue: (policy) => policy.last_run_at ?? "",
      searchValue: (policy) =>
        `${policy.last_run_at ?? ""} ${policy.last_error ?? ""} ${policy.failure_count}`,
      cell: (policy) => {
        const result = policyLastResult(policy);
        return (
          <span className="historyPrimary">
            <strong className={`status ${result.tone}`} title={result.title}>
              {result.label}
            </strong>
            <small>{result.detail}</small>
          </span>
        );
      },
    },
    {
      id: "state",
      header: "State",
      size: 135,
      minSize: 115,
      sortValue: (policy) => policyState(policy).label,
      searchValue: (policy) =>
        `${policyState(policy).label} ${policyState(policy).detail} ${policy.catch_up_policy} ${policy.max_failures}`,
      cell: (policy) => {
        const state = policyState(policy);
        return (
          <span className="historyPrimary">
            <strong className={`status ${state.tone}`}>{state.label}</strong>
            <small>{state.detail}</small>
          </span>
        );
      },
    },
  ];
  return (
    <GridSection
      title="Scheduled backup policies"
      summary="Enabled policies run automatically on their UTC cadence; prune is separate retention maintenance."
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
            title="No scheduled backups"
            text="Create a policy for automatic backups, or use Back up now in Requests for a one-time backup."
          />
        }
        getRowId={(policy) => policy.schedule_id}
        itemLabel="policies"
        renderExpandedRow={(policy) => (
          <div className="gridDetailLine">
            <strong>{policy.name}</strong>
            <span>{policyTargetLabel(policy)}</span>
            <span>{policyScopeLabel(policy)}</span>
            <span>{describeCronExpression(policy.cron_expr)}</span>
            <span>
              {policy.next_runs.length} next run{policy.next_runs.length === 1 ? "" : "s"} reported
            </span>
            <span>{policy.retention_days}d retention</span>
            <span>
              {policy.catch_up_policy}; retry {formatDuration(policy.retry_delay_secs * 1000)}
            </span>
            <span title={policy.schedule_id}>schedule {shortId(policy.schedule_id)}</span>
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
  artifacts,
  backups,
  clientLabel,
  onOpenRequestArtifact,
  onRetryBackup,
}: {
  artifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  onOpenRequestArtifact?: (backup: BackupRequestRecord) => void;
  onRetryBackup?: (backup: BackupRequestRecord) => void;
}) {
  const artifactForBackup = (backup: BackupRequestRecord) =>
    backup.artifact_id
      ? artifacts.find((artifact) => artifact.id === backup.artifact_id) ?? null
      : null;
  const canOpenArtifact = (backup: BackupRequestRecord) =>
    Boolean(backup.artifact_id) && !backupRequestNeedsAttention(backup.status);
  const rowActions: ConsoleDataGridAction<BackupRequestRecord>[] = [
    {
      description: ([backup]) =>
        backup
          ? "Open the artifact inventory for this backup request."
          : "Open artifact",
      disabled: ([backup]) => !backup || !canOpenArtifact(backup) || !onOpenRequestArtifact,
      icon: <ExternalLink size={15} />,
      label: "Open artifact",
      onSelect: ([backup]) => {
        if (backup && canOpenArtifact(backup)) {
          onOpenRequestArtifact?.(backup);
        }
      },
    },
    {
      description: ([backup]) =>
        backup
          ? "Prefill the backup request workflow from this request."
          : "Retry backup request",
      disabled: ([backup]) => !backup || canOpenArtifact(backup) || !onRetryBackup,
      icon: <RotateCcw size={15} />,
      label: "Retry",
      onSelect: ([backup]) => {
        if (backup && !canOpenArtifact(backup)) {
          onRetryBackup?.(backup);
        }
      },
    },
  ];
  const columns: ConsoleDataGridColumn<BackupRequestRecord>[] = [
    {
      id: "vps",
      header: "VPS",
      size: 155,
      minSize: 135,
      sortValue: (backup) => clientLabel(backup.client_id),
      searchValue: (backup) => clientLabel(backup.client_id),
      cell: (backup) => (
        <span title={clientLabel(backup.client_id)}>
          {clientLabel(backup.client_id)}
        </span>
      ),
    },
    {
      id: "paths",
      header: "Paths",
      size: 175,
      minSize: 145,
      sortValue: backupScopeLabel,
      searchValue: (backup) =>
        `${backupScopeLabel(backup)} ${backup.paths.join(" ")}`,
      cell: (backup) => (
        <span className="historyPrimary">
          <strong>{backupPathSummaryLabel(backup)}</strong>
          <small title={`${backupSymlinkLabel(backup)}; ${backup.paths.join(", ")}`}>
            {backup.paths[0]
              ? `${backupSymlinkLabel(backup)} · ${backup.paths[0]}`
              : backupSymlinkLabel(backup)}
          </small>
        </span>
      ),
    },
    {
      id: "status",
      header: "State",
      size: 150,
      minSize: 130,
      sortValue: (backup) => backup.status,
      searchValue: (backup) =>
        `${backup.status} ${backupStatusLabel(backup.status)}`,
      cell: (backup) => {
        const state = backupStateParts(backup.status);
        return (
          <span className="historyPrimary">
            <strong
              className={`status ${backupRequestStatusBadgeClass(backup.status)}`}
              title={backupStatusLabel(backup.status)}
            >
              {state.label}
            </strong>
            <small>{state.detail}</small>
          </span>
        );
      },
    },
    {
      id: "size",
      header: "Size",
      size: 80,
      minSize: 70,
      sortValue: (backup) => artifactForBackup(backup)?.size_bytes ?? -1,
      searchValue: (backup) => {
        const artifact = artifactForBackup(backup);
        return artifact ? formatBytes(artifact.size_bytes) : "not reported";
      },
      cell: (backup) => {
        const artifact = artifactForBackup(backup);
        return artifact ? formatBytes(artifact.size_bytes) : "not reported";
      },
    },
    {
      id: "started",
      header: "Started",
      size: 90,
      minSize: 80,
      sortValue: (backup) => backup.created_at,
      searchValue: (backup) => backup.created_at,
      cell: (backup) => (
        <time dateTime={backup.created_at} title={formatFullTime(backup.created_at)}>
          {formatCompactTime(backup.created_at)}
        </time>
      ),
    },
    {
      id: "duration",
      header: "Duration",
      size: 90,
      minSize: 80,
      sortValue: () => "not reported",
      searchValue: () => "duration not reported",
      cell: () => "not reported",
    },
    {
      id: "artifact",
      header: "Artifact",
      size: 125,
      minSize: 110,
      sortValue: (backup) => backup.artifact_id ?? "",
      searchValue: (backup) => {
        const artifact = artifactForBackup(backup);
        return `${backup.artifact_id ?? ""} ${artifact?.status ?? ""} ${artifact ? artifactVerificationLabel(artifact.status) : "no package"}`;
      },
      cell: (backup) => {
        const artifact = artifactForBackup(backup);
        return (
          <span className="historyPrimary">
            <strong title={backup.artifact_id ?? ""}>
              {backup.artifact_id ? shortId(backup.artifact_id) : "No package"}
            </strong>
            <small>
              {artifact
                ? artifactVerificationShortLabel(artifact.status)
                : backup.artifact_id
                  ? "metadata only"
                  : "not recorded"}
            </small>
          </span>
        );
      },
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
      id: "action",
      header: "Action",
      size: 145,
      minSize: 135,
      sortValue: (backup) =>
        canOpenArtifact(backup) ? "open artifact" : "retry",
      searchValue: () => "open artifact retry",
      cell: (backup) => (
        canOpenArtifact(backup) ? (
          <button
            className="secondaryAction compactAction"
            disabled={!onOpenRequestArtifact}
            onClick={(event) => {
              event.stopPropagation();
              onOpenRequestArtifact?.(backup);
            }}
            title="Open the artifact inventory for this backup request."
            type="button"
          >
            <ExternalLink size={15} />
            <span>Open artifact</span>
          </button>
        ) : (
          <button
            className="secondaryAction compactAction"
            disabled={!onRetryBackup}
            onClick={(event) => {
              event.stopPropagation();
              onRetryBackup?.(backup);
            }}
            title="Prefill the backup request workflow from this request."
            type="button"
          >
            <RotateCcw size={15} />
            <span>Retry</span>
          </button>
        )
      ),
    },
  ];
  return (
    <GridSection
      title="Backup request history"
      summary="Request IDs, source jobs, hashes, and raw artifact metadata stay in row details."
    >
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
        defaultColumnVisibility={{ hash: false }}
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<Archive size={20} />}
            title="No backup requests"
            text="Backup requests will appear here after review."
          />
        }
        getRowId={(backup) => backup.id}
        itemLabel="requests"
        renderExpandedRow={(backup) => {
          const artifact = artifactForBackup(backup);
          return (
            <div className="gridDetailLine">
              <strong>{clientLabel(backup.client_id)}</strong>
              <span title={backup.id}>request {shortId(backup.id)}</span>
              <span>{backup.actor_id ? `actor ${shortId(backup.actor_id)}` : "requester not reported"}</span>
              <span>{backup.source_job_id ? `job ${shortId(backup.source_job_id)}` : "source job not reported"}</span>
              <span>{backup.source_schedule_id ? `schedule ${shortId(backup.source_schedule_id)}` : "manual or source not reported"}</span>
              <span>{backupStatusLabel(backup.status)}</span>
              <span>{artifact ? `${formatBytes(artifact.size_bytes)} package` : "artifact size not reported"}</span>
              <span>{shortHash(backup.payload_hash)}</span>
              <span>{formatTime(backup.created_at)}</span>
              {backup.note && <span>{backup.note}</span>}
            </div>
          );
        }}
        rowActions={rowActions}
        rows={backups}
        storageKey="vpsman.grid.backups.requests"
        title="Backup request records"
      />
    </GridSection>
  );
}

function ArtifactHistoryTable({
  artifacts,
  backups,
  clientLabel,
  onDownloadArtifact,
  onRestoreArtifact,
}: {
  artifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  clientLabel: (clientId: string) => string;
  onDownloadArtifact?: (
    artifact: BackupArtifactRecord,
    backup: BackupRequestRecord | null,
  ) => void;
  onRestoreArtifact?: (
    artifact: BackupArtifactRecord,
    backup: BackupRequestRecord | null,
  ) => void;
}) {
  const backupForArtifact = (artifact: BackupArtifactRecord) =>
    backups.find((backup) => backup.artifact_id === artifact.id) ?? null;
  const columns: ConsoleDataGridColumn<BackupArtifactRecord>[] = [
    {
      id: "artifact",
      header: "Artifact",
      size: 170,
      minSize: 145,
      sortValue: (artifact) => artifact.id,
      searchValue: (artifact) => {
        const backup = backupForArtifact(artifact);
        return `${artifact.id} ${artifact.object_key} ${artifact.sha256_hex} ${backup?.id ?? ""}`;
      },
      cell: (artifact) => (
        <span className="historyPrimary">
          <strong title={artifact.id}>{shortId(artifact.id)}</strong>
          <small>
            {backupForArtifact(artifact)
              ? `request ${shortId(backupForArtifact(artifact)?.id ?? "")}`
              : "unlinked package"}
          </small>
        </span>
      ),
    },
    {
      id: "client",
      header: "VPS",
      size: 180,
      minSize: 150,
      sortValue: (artifact) => clientLabel(artifact.client_id),
      searchValue: (artifact) => clientLabel(artifact.client_id),
      cell: (artifact) => (
        <span title={clientLabel(artifact.client_id)}>
          {clientLabel(artifact.client_id)}
        </span>
      ),
    },
    {
      id: "created",
      header: "Created",
      size: 125,
      minSize: 115,
      sortValue: (artifact) => artifact.created_at,
      searchValue: (artifact) => artifact.created_at,
      cell: (artifact) => (
        <time dateTime={artifact.created_at} title={formatFullTime(artifact.created_at)}>
          {formatCompactTime(artifact.created_at)}
        </time>
      ),
    },
    {
      id: "size",
      header: "Size",
      size: 100,
      minSize: 90,
      sortValue: (artifact) => artifact.size_bytes,
      searchValue: (artifact) => artifact.size_bytes,
      cell: (artifact) => formatBytes(artifact.size_bytes),
    },
    {
      id: "verification",
      header: "Verification",
      size: 155,
      minSize: 130,
      sortValue: (artifact) => artifactVerificationLabel(artifact.status),
      searchValue: (artifact) =>
        `${artifact.status} ${artifactVerificationLabel(artifact.status)}`,
      cell: (artifact) => (
        <span
          className={`status ${artifactLifecycleStatusBadgeClass(artifact.status)}`}
          title={artifactLifecycleStatusTitle(artifact.status)}
        >
          {artifactVerificationLabel(artifact.status)}
        </span>
      ),
    },
    {
      id: "retention",
      header: "Retention",
      size: 165,
      minSize: 130,
      sortValue: (artifact) => (backupForArtifact(artifact) ? "linked" : "unlinked"),
      searchValue: (artifact) =>
        backupForArtifact(artifact)
          ? "linked backup request retention expiry not reported"
          : "unlinked retention expiry not reported",
      cell: (artifact) => {
        const backup = backupForArtifact(artifact);
        return (
          <span className="historyPrimary">
            <strong>{backup ? "Linked" : "Unlinked"}</strong>
            <small>expiry not reported</small>
          </span>
        );
      },
    },
    {
      id: "restore",
      header: "Restore",
      size: 115,
      minSize: 105,
      sortValue: (artifact) => (backupForArtifact(artifact) ? "restore" : "disabled"),
      searchValue: () => "restore",
      cell: (artifact) => {
        const backup = backupForArtifact(artifact);
        return (
          <button
            className="secondaryAction compactAction"
            disabled={!backup || !onRestoreArtifact}
            onClick={(event) => {
              event.stopPropagation();
              onRestoreArtifact?.(artifact, backup);
            }}
            title={
              backup
                ? "Open the restore workflow with this artifact source selected."
                : "Restore requires a linked backup request."
            }
            type="button"
          >
            <RotateCcw size={15} />
            <span>Restore</span>
          </button>
        );
      },
    },
    {
      id: "download",
      header: "Download",
      size: 150,
      minSize: 130,
      sortValue: (artifact) => (backupForArtifact(artifact) ? "download" : "disabled"),
      searchValue: () => "download package transfer package",
      cell: (artifact) => {
        const backup = backupForArtifact(artifact);
        return (
          <button
            className="secondaryAction compactAction"
            disabled={!backup || !onDownloadArtifact}
            onClick={(event) => {
              event.stopPropagation();
              onDownloadArtifact?.(artifact, backup);
            }}
            title={
              backup
                ? "Download the backup artifact package in this browser."
                : "Download requires a linked backup request."
            }
            type="button"
          >
            <Download size={15} />
            <span>Download</span>
          </button>
        );
      },
    },
  ];
  return (
    <GridSection
      title="Artifact inventory"
      summary="Restore and download actions stay on the package row; object lineage is in details."
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
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<Archive size={20} />}
            title="No artifacts"
            text="Backup packages will appear here after upload or transfer package creation."
          />
        }
        getRowId={(artifact) => artifact.id}
        itemLabel="artifacts"
        renderExpandedRow={(artifact) => (
          <div className="gridDetailLine">
            <strong title={clientLabel(artifact.client_id)}>
              {clientLabel(artifact.client_id)}
            </strong>
            <span title={backupForArtifact(artifact)?.id ?? ""}>
              request {backupForArtifact(artifact) ? shortId(backupForArtifact(artifact)?.id ?? "") : "unlinked"}
            </span>
            <span title={String(artifact.size_bytes)}>
              {formatBytes(artifact.size_bytes)}
            </span>
            <span title={artifact.object_key}>{artifact.object_key}</span>
            <span title={artifact.sha256_hex}>
              {shortHash(artifact.sha256_hex)}
            </span>
            <span title={artifactLifecycleStatusTitle(artifact.status)}>
              raw {artifact.status}
            </span>
            <span title={artifact.created_at}>
              {formatTime(artifact.created_at)}
            </span>
          </div>
        )}
        rows={artifacts}
        storageKey="vpsman.grid.backups.artifacts"
        title="Artifact inventory records"
      />
    </GridSection>
  );
}

function RestoreSourcesTable({
  artifacts,
  backups,
  clientLabel,
  onPlanRestoreSource,
  restorePlans,
}: {
  artifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  restorePlans: RestorePlanRecord[];
  clientLabel: (clientId: string) => string;
  onPlanRestoreSource?: (backup: BackupRequestRecord) => void;
}) {
  const artifactForBackup = (backup: BackupRequestRecord) =>
    backup.artifact_id
      ? artifacts.find((artifact) => artifact.id === backup.artifact_id) ?? null
      : null;
  const latestPlanForBackup = (backup: BackupRequestRecord) =>
    restorePlans
      .filter((plan) => plan.source_backup_request_id === backup.id)
      .sort((left, right) => right.created_at.localeCompare(left.created_at))[0] ??
    null;
  const columns: ConsoleDataGridColumn<BackupRequestRecord>[] = [
    {
      id: "artifact",
      header: "Artifact",
      size: 180,
      minSize: 150,
      sortValue: (backup) => artifactForBackup(backup)?.id ?? backup.artifact_id ?? "",
      searchValue: (backup) => {
        const artifact = artifactForBackup(backup);
        return `${backup.id} ${backup.artifact_id ?? ""} ${artifact?.object_key ?? ""} ${artifact?.sha256_hex ?? ""}`;
      },
      cell: (backup) => {
        const artifact = artifactForBackup(backup);
        return (
          <span className="historyPrimary">
            <strong title={artifact?.id ?? backup.artifact_id ?? ""}>
              {artifact
                ? shortId(artifact.id)
                : backup.artifact_id
                  ? shortId(backup.artifact_id)
                  : "No artifact"}
            </strong>
            <small>
              {clientLabel(backup.client_id)} · request {shortId(backup.id)}
            </small>
          </span>
        );
      },
    },
    {
      id: "readiness",
      header: "Readiness",
      size: 165,
      minSize: 135,
      sortValue: (backup) =>
        restoreSourceReadiness(backup, artifactForBackup(backup)).sort,
      searchValue: (backup) => {
        const readiness = restoreSourceReadiness(backup, artifactForBackup(backup));
        return `${readiness.label} ${readiness.detail}`;
      },
      cell: (backup) => {
        const readiness = restoreSourceReadiness(backup, artifactForBackup(backup));
        return (
          <span className="historyPrimary">
            <strong className={`status ${readiness.tone}`} title={readiness.title}>
              {readiness.label}
            </strong>
            <small>{readiness.detail}</small>
          </span>
        );
      },
    },
    {
      id: "destination",
      header: "Destination",
      size: 185,
      minSize: 150,
      sortValue: (backup) =>
        latestPlanForBackup(backup)
          ? clientLabel(latestPlanForBackup(backup)?.target_client_id ?? "")
          : "",
      searchValue: (backup) => {
        const plan = latestPlanForBackup(backup);
        return `${plan?.target_client_id ?? ""} ${plan ? clientLabel(plan.target_client_id) : "choose destination"}`;
      },
      cell: (backup) => {
        const plan = latestPlanForBackup(backup);
        return (
          <span className="historyPrimary">
            <strong>
              {plan ? clientLabel(plan.target_client_id) : "Choose destination"}
            </strong>
            <small>{plan?.destination_root ?? "set in restore drawer"}</small>
          </span>
        );
      },
    },
    {
      id: "behavior",
      header: "Path behavior",
      size: 165,
      minSize: 135,
      sortValue: (backup) =>
        restoreSourceScopeLabel(backup.include_config, backup.paths),
      searchValue: (backup) =>
        `${restoreSourceScopeLabel(backup.include_config, backup.paths)} ${backup.paths.join(" ")}`,
      cell: (backup) => (
        <span className="historyPrimary">
          <strong>
            {restoreSourceScopeLabel(backup.include_config, backup.paths)}
          </strong>
          <small>
            {backup.paths.length > 0
              ? backup.paths.slice(0, 2).join(", ")
              : backup.include_config
                ? "agent config included"
                : "metadata only"}
          </small>
        </span>
      ),
    },
    {
      id: "draft",
      header: "Draft restore",
      size: 150,
      minSize: 125,
      sortValue: (backup) => latestPlanForBackup(backup)?.created_at ?? "",
      searchValue: (backup) => {
        const plan = latestPlanForBackup(backup);
        return `${plan?.id ?? ""} ${plan?.status ?? "no draft restore"}`;
      },
      cell: (backup) => {
        const plan = latestPlanForBackup(backup);
        return (
          <span className="historyPrimary">
            <strong
              className={`status ${
                plan ? restorePlanStatusBadgeClass(plan.status) : "neutral"
              }`}
              title={plan?.status ?? "No draft restore has been saved."}
            >
              {plan ? backupStatusLabel(plan.status) : "No draft"}
            </strong>
            <small>
              {plan ? formatCompactTime(plan.created_at) : "save if interrupted"}
            </small>
          </span>
        );
      },
    },
    {
      id: "action",
      header: "Action",
      size: 125,
      minSize: 115,
      sortValue: () => "restore",
      searchValue: () => "restore choose artifact draft restore",
      cell: (backup) => {
        const readiness = restoreSourceReadiness(backup, artifactForBackup(backup));
        return (
          <button
            className="secondaryAction compactAction"
            disabled={!onPlanRestoreSource}
            onClick={(event) => {
              event.stopPropagation();
              onPlanRestoreSource?.(backup);
            }}
            title={
              readiness.tone === "ok"
                ? "Choose this artifact and continue to destination/path behavior."
                : "Choose this source, but verify the artifact before live restore."
            }
            type="button"
          >
            <RotateCcw size={15} />
            <span>Restore</span>
          </button>
        );
      },
    },
  ];
  return (
    <GridSection
      title="Restore sources"
      summary="Choose artifact, destination and path behavior, then confirm a dry run or live restore."
    >
      <ConsoleDataGrid
        actions={[
          copyIdsAction<BackupRequestRecord>(
            "Copy source backup IDs",
            (backup) => backup.id,
          ),
        ]}
        columns={columns}
        defaultColumnVisibility={{ draft: true }}
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<RotateCcw size={20} />}
            title="No restore sources"
            text="Create or upload a backup artifact before planning a restore."
          />
        }
        getRowId={(backup) => backup.id}
        itemLabel="sources"
        renderExpandedRow={(backup) => {
          const artifact = artifactForBackup(backup);
          const plan = latestPlanForBackup(backup);
          const readiness = restoreSourceReadiness(backup, artifact);
          return (
            <div className="gridDetailLine">
              <strong title={backup.id}>request {shortId(backup.id)}</strong>
              <span title={clientLabel(backup.client_id)}>
                source {clientLabel(backup.client_id)}
              </span>
              <span title={readiness.title}>{readiness.label}</span>
              <span title={artifact?.object_key ?? ""}>
                {artifact?.object_key ?? "no object key"}
              </span>
              <span title={artifact?.sha256_hex ?? ""}>
                {artifact ? shortHash(artifact.sha256_hex) : "no SHA-256"}
              </span>
              <span title={plan?.id ?? ""}>
                draft {plan ? shortId(plan.id) : "none"}
              </span>
              <span title={plan?.payload_hash ?? ""}>
                {plan ? shortHash(plan.payload_hash) : "no draft hash"}
              </span>
            </div>
          );
        }}
        rows={backups}
        storageKey="vpsman.grid.backups.restoreSources"
        title="Restore source records"
      />
    </GridSection>
  );
}

function MigrationLinksTable({
  artifacts,
  backups,
  migrationLinks,
  clientLabel,
  restorePlans,
}: {
  artifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  migrationLinks: MigrationLinkRecord[];
  clientLabel: (clientId: string) => string;
  restorePlans: RestorePlanRecord[];
}) {
  const sourceBackupForLink = (link: MigrationLinkRecord) =>
    backups.find((backup) => backup.id === link.source_backup_request_id) ??
    null;
  const artifactForBackup = (backup: BackupRequestRecord | null) =>
    backup?.artifact_id
      ? artifacts.find((artifact) => artifact.id === backup.artifact_id) ?? null
      : null;
  const restorePlanForLink = (link: MigrationLinkRecord) =>
    restorePlans.find((plan) => plan.id === link.restore_plan_id) ?? null;
  const columns: ConsoleDataGridColumn<MigrationLinkRecord>[] = [
    {
      id: "mapping",
      header: "Mapping",
      size: 145,
      minSize: 130,
      sortValue: (link) => link.id,
      searchValue: (link) => `${link.id} ${link.restore_plan_id}`,
      cell: (link) => (
        <span className="historyPrimary">
          <strong>{shortId(link.id)}</strong>
          <small>draft {shortId(link.restore_plan_id)}</small>
        </span>
      ),
    },
    {
      id: "sourceArtifact",
      header: "Source artifact",
      size: 210,
      minSize: 165,
      sortValue: (link) => clientLabel(link.source_client_id),
      searchValue: (link) => {
        const backup = sourceBackupForLink(link);
        const artifact = artifactForBackup(backup);
        return `${clientLabel(link.source_client_id)} ${backup?.id ?? ""} ${backup?.artifact_id ?? ""} ${artifact?.status ?? ""}`;
      },
      cell: (link) => {
        const backup = sourceBackupForLink(link);
        const artifact = artifactForBackup(backup);
        const readiness = backup
          ? restoreSourceReadiness(backup, artifact)
          : {
            label: "Unverified package",
            title: "Source backup request is not visible in the current backup records.",
          };
        return (
          <span className="historyPrimary">
            <strong>{clientLabel(link.source_client_id)}</strong>
            <small title={readiness.title}>{readiness.label}</small>
          </span>
        );
      },
    },
    {
      id: "replacement",
      header: "Replacement VPS",
      size: 180,
      minSize: 145,
      sortValue: (link) => clientLabel(link.target_client_id),
      searchValue: (link) => clientLabel(link.target_client_id),
      cell: (link) => (
        <span className="historyPrimary">
          <strong>{clientLabel(link.target_client_id)}</strong>
          <small>{link.destination_root ?? "restore path not recorded"}</small>
        </span>
      ),
    },
    {
      id: "behavior",
      header: "Path behavior",
      size: 150,
      minSize: 125,
      sortValue: migrationScopeLabel,
      searchValue: migrationScopeLabel,
      cell: migrationScopeLabel,
    },
    {
      id: "cutover",
      header: "Cutover state",
      size: 145,
      minSize: 125,
      sortValue: (link) => link.status,
      searchValue: (link) => `${link.status} ${backupStatusLabel(link.status)} ${link.note ?? ""}`,
      cell: (link) => (
        <span className="historyPrimary">
          <strong
            className={`status ${migrationLinkStatusBadgeClass(link.status)}`}
            title={link.status}
          >
            {backupStatusLabel(link.status)}
          </strong>
          <small>{link.note ?? "no cutover notes"}</small>
        </span>
      ),
    },
    {
      id: "created",
      header: "Created",
      size: 125,
      minSize: 115,
      sortValue: (link) => link.created_at,
      searchValue: (link) => link.created_at,
      cell: (link) => (
        <time dateTime={link.created_at} title={formatFullTime(link.created_at)}>
          {formatCompactTime(link.created_at)}
        </time>
      ),
    },
  ];
  return (
    <GridSection
      title="Migration mappings"
      summary="Source VPS/artifact to replacement VPS relationships; identity, service checks, and cutover evidence stay in details."
    >
      <ConsoleDataGrid
        actions={[
          copyIdsAction<MigrationLinkRecord>(
            "Copy migration mapping IDs",
            (link) => link.id,
          ),
        ]}
        columns={columns}
        defaultColumnVisibility={{ created: false }}
        defaultPageSize={8}
        empty={
          <GridEmpty
            icon={<GitBranch size={20} />}
            title="No migration mappings"
            text="Create a mapping after a draft restore defines the source artifact and replacement VPS."
          />
        }
        getRowId={(link) => link.id}
        itemLabel="mappings"
        renderExpandedRow={(link) => (
          <div className="gridDetailLine">
            <strong>
              {clientLabel(link.source_client_id)} to{" "}
              {clientLabel(link.target_client_id)}
            </strong>
            <span title={link.restore_plan_id}>
              draft {shortId(link.restore_plan_id)}
            </span>
            <span>{migrationScopeLabel(link)}</span>
            <span>{backupStatusLabel(link.status)}</span>
            <span>{link.destination_root ?? "no restore path"}</span>
            <span>{link.note ?? "no cutover notes"}</span>
            <span>
              restore draft {restorePlanForLink(link) ? "visible" : "not visible"}
            </span>
            <span>{formatTime(link.created_at)}</span>
          </div>
        )}
        rows={migrationLinks}
        storageKey="vpsman.grid.backups.migrations"
        title="Migration mapping records"
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
  scopes.push(backup.follow_symlinks ? "follows symlinks" : "no symlink follow");
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function backupPathSummaryLabel(backup: BackupRequestRecord): string {
  const scopes = [];
  if (backup.include_config) {
    scopes.push("config");
  }
  if (backup.paths.length > 0) {
    scopes.push(
      `${backup.paths.length} path${backup.paths.length === 1 ? "" : "s"}`,
    );
  }
  return scopes.length > 0 ? scopes.join(" + ") : "No data scope";
}

function backupSymlinkLabel(backup: BackupRequestRecord): string {
  return backup.follow_symlinks ? "follow symlinks" : "no symlink follow";
}

function backupRequestNeedsAttention(status: string): boolean {
  return /fail|error|cancel|lost|timeout|expired|rejected/.test(status.toLowerCase());
}

function backupStateParts(status: string): { detail: string; label: string } {
  const states: Record<string, { detail: string; label: string }> = {
    artifact_metadata_recorded: {
      detail: "content not verified",
      label: "Recorded",
    },
    artifact_uploaded: {
      detail: "package uploaded",
      label: "Uploaded",
    },
    completed: { detail: "backup completed", label: "Completed" },
    failed: { detail: "retry available", label: "Failed" },
    planned_metadata_only: {
      detail: "metadata only",
      label: "Planned",
    },
    requested: { detail: "waiting for artifact", label: "Requested" },
    restored: { detail: "restore evidence exists", label: "Restored" },
    running: { detail: "in progress", label: "Running" },
  };
  return states[status] ?? {
    detail: "raw status retained in details",
    label: backupStatusLabel(status),
  };
}

function policyTargetLabel(policy: BackupPolicyRecord): string {
  const ids = Array.isArray(policy.target_client_ids)
    ? policy.target_client_ids
    : [];
  if (ids.length === 0) {
    return "no fixed targets";
  }
  const preview = ids.slice(0, 3).join(", ");
  return `${ids.length} VPS${ids.length === 1 ? "" : "s"}${preview ? ` · ${preview}` : ""}`;
}

function policyTargetCountLabel(policy: BackupPolicyRecord): string {
  const count = Array.isArray(policy.target_client_ids)
    ? policy.target_client_ids.length
    : 0;
  return count === 0 ? "No fixed targets" : `${count} VPS${count === 1 ? "" : "s"}`;
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
  scopes.push(policy.follow_symlinks ? "follows symlinks" : "no symlink follow");
  return scopes.length > 0 ? scopes.join(" + ") : "empty";
}

function describeCronExpression(expr: string): string {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) {
    return "Invalid schedule";
  }
  const [minute, hour, dom, month, dow] = fields;
  if (
    minute.startsWith("*/") &&
    hour === "*" &&
    dom === "*" &&
    month === "*" &&
    dow === "*"
  ) {
    const interval = Number(minute.slice(2));
    return Number.isInteger(interval) && interval > 0
      ? `Every ${interval} minutes`
      : "Custom cron schedule";
  }
  if (hour === "*" && dom === "*" && month === "*" && dow === "*") {
    return `Hourly at minute ${minute}`;
  }
  if (dom === "*" && month === "*" && dow === "*") {
    return `Daily at ${timeLabel(hour, minute)} UTC`;
  }
  if (dom === "*" && month === "*" && dow !== "*") {
    return `Weekly at ${timeLabel(hour, minute)} UTC`;
  }
  if (month === "*" && dow === "*") {
    return `Monthly on day ${dom}`;
  }
  return "Custom cron schedule";
}

function timeLabel(hour: string, minute: string): string {
  return `${hour.padStart(2, "0")}:${minute.padStart(2, "0")}`;
}

function formatDuration(valueMs: number): string {
  const minutes = Math.max(1, Math.round(valueMs / 60000));
  if (minutes < 60) {
    return `${minutes}m`;
  }
  const hours = Math.round(minutes / 60);
  if (hours < 48) {
    return `${hours}h`;
  }
  return `${Math.round(hours / 24)}d`;
}

function policyNextRunLabel(value: string): string {
  return policyNextRunState(value).label;
}

function policyNextRunState(value: string): {
  detail: string;
  label: string;
  tone: "info" | "neutral" | "warn";
} {
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) {
    return { detail: "next run unavailable", label: "Unknown", tone: "neutral" };
  }
  const ageMs = timestamp - Date.now();
  if (ageMs < 0) {
    return { detail: formatCompactTime(value), label: "Overdue", tone: "warn" };
  }
  return { detail: formatCompactTime(value), label: "Scheduled", tone: "info" };
}

function policyLastResult(policy: BackupPolicyRecord): {
  detail: string;
  label: string;
  title: string;
  tone: "neutral" | "ok" | "warn";
} {
  if (policy.last_error) {
    return {
      detail: policy.last_error,
      label: "Failed",
      title: policy.last_error,
      tone: "warn",
    };
  }
  if (policy.failure_count > 0) {
    return {
      detail: `${policy.failure_count}/${policy.max_failures} failures`,
      label: "Failures",
      title: `${policy.failure_count} failure${policy.failure_count === 1 ? "" : "s"}`,
      tone: "warn",
    };
  }
  if (policy.last_run_at) {
    return {
      detail: formatCompactTime(policy.last_run_at),
      label: "Succeeded",
      title: formatFullTime(policy.last_run_at),
      tone: "ok",
    };
  }
  return {
    detail: "no run recorded",
    label: "No run yet",
    title: "No backup run has been recorded for this policy.",
    tone: "neutral",
  };
}

function policyState(policy: BackupPolicyRecord): {
  detail: string;
  label: string;
  tone: "neutral" | "ok" | "warn";
} {
  if (!policy.enabled) {
    return { detail: "manual only", label: "Paused", tone: "neutral" };
  }
  if (policy.failure_count >= policy.max_failures) {
    return { detail: "failure limit reached", label: "Blocked", tone: "warn" };
  }
  return { detail: "runs automatically", label: "Automatic", tone: "ok" };
}

function restoreSourceReadiness(
  backup: BackupRequestRecord,
  artifact: BackupArtifactRecord | null,
): {
  detail: string;
  label: string;
  sort: string;
  title: string;
  tone: "neutral" | "ok" | "warn";
} {
  if (artifact?.status === "active") {
    return {
      detail: `${formatBytes(artifact.size_bytes)} staged package record`,
      label: "Available package",
      sort: "0-available",
      title: artifactLifecycleStatusTitle(artifact.status),
      tone: "ok",
    };
  }
  if (artifact) {
    return {
      detail: artifactVerificationShortLabel(artifact.status),
      label: "Unverified package",
      sort: `1-${artifact.status}`,
      title: artifactLifecycleStatusTitle(artifact.status),
      tone: "warn",
    };
  }
  if (backup.artifact_id) {
    return {
      detail: "artifact metadata unavailable",
      label: "Unverified package",
      sort: "2-metadata-gap",
      title: "This backup references an artifact ID, but the artifact record is not visible.",
      tone: "warn",
    };
  }
  return {
    detail: "upload or transfer package first",
    label: "No artifact",
    sort: "3-no-artifact",
    title: "This backup request has no artifact record and cannot run a live restore yet.",
    tone: "warn",
  };
}

function restoreSourceScopeLabel(includeConfig: boolean, paths: string[]): string {
  const scopes = [];
  if (includeConfig) {
    scopes.push("config restore");
  }
  if (paths.length > 0) {
    scopes.push(`${paths.length} path${paths.length === 1 ? "" : "s"}`);
  }
  return scopes.length > 0 ? scopes.join(" + ") : "metadata only";
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
    accepted: "Accepted",
    artifact_metadata_recorded: "Artifact recorded; content not verified",
    artifact_uploaded: "Artifact uploaded",
    completed: "Completed",
    failed: "Failed",
    linked_metadata_only: "linked",
    planned_metadata_only: "planned",
    requested: "Requested",
    restored: "Restored",
    running: "Running",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function artifactLifecycleStatusTitle(status: string): string {
  const descriptions: Record<string, string> = {
    active: "Object bytes are owned by this artifact and available.",
    creating: "Artifact ownership is being prepared.",
    deleting: "Object deletion is in progress; metadata remains visible until deletion finishes.",
    delete_failed: "Object deletion failed; metadata remains visible and cleanup can be retried.",
    tombstoned: "Metadata was retained as a tombstone.",
    deleted: "Object bytes were deleted.",
  };
  return descriptions[status] ?? status.replace(/_/g, " ");
}

function artifactVerificationLabel(status: string): string {
  const labels: Record<string, string> = {
    active: "Available package",
    creating: "Preparing package",
    deleting: "Deleting package",
    delete_failed: "Delete failed",
    tombstoned: "Tombstone only",
    deleted: "Deleted",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function artifactVerificationShortLabel(status: string): string {
  const labels: Record<string, string> = {
    active: "Available",
    creating: "Preparing",
    deleting: "Deleting",
    delete_failed: "Delete failed",
    tombstoned: "Tombstone",
    deleted: "Deleted",
  };
  return labels[status] ?? artifactVerificationLabel(status);
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
