import { useMemo, useState } from "react";
import { Archive, Copy, ExternalLink } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import type {
  AgentUpdateReleaseRecord,
  BackupArtifactRecord,
} from "../../types";
import type { FileTransferSourceArtifactRecord } from "../../typesFileTransfer";
import { formatTime, shortHash, shortId } from "../../utils";

type JobArtifactsPanelProps = {
  agentUpdateReleases: AgentUpdateReleaseRecord[];
  backupArtifacts: BackupArtifactRecord[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
  onOpenAgentUpdates: () => void;
  onOpenBackupsArtifacts: () => void;
  onOpenTransfers: () => void;
};

type ArtifactInventoryRow = {
  actionLabel: string;
  createdAt: string;
  detailLabel: string;
  downloadPath: string | null;
  rawStatus: string;
  id: string;
  name: string;
  objectKey: string;
  relationLabel: string;
  sourceDetail: string;
  sha256Hex: string;
  sizeBytes: number | null;
  sourceWorkflow: string;
  type: string;
  verification:
    | "Ready"
    | "Upload incomplete"
    | "Verification failed"
    | "Expired";
  verificationDetail: string;
};

export function JobArtifactsPanel({
  agentUpdateReleases,
  backupArtifacts,
  fileTransferSources,
  onOpenAgentUpdates,
  onOpenBackupsArtifacts,
  onOpenTransfers,
}: JobArtifactsPanelProps) {
  const [typeFilter, setTypeFilter] = useState("all");
  const rows = buildArtifactInventoryRows({
    agentUpdateReleases,
    backupArtifacts,
    fileTransferSources,
  });
  const totalBytes = rows.reduce((sum, row) => sum + (row.sizeBytes ?? 0), 0);
  const artifactTypes = useMemo(
    () => Array.from(new Set(rows.map((row) => row.type))).sort(),
    [rows],
  );
  const visibleRows = useMemo(
    () =>
      typeFilter === "all"
        ? rows
        : rows.filter((row) => row.type === typeFilter),
    [rows, typeFilter],
  );
  const columns: ConsoleDataGridColumn<ArtifactInventoryRow>[] = [
    {
      cell: (row) => (
        <span className="historyPrimary">
          <strong title={row.name}>{row.name}</strong>
          <small>{row.detailLabel}</small>
        </span>
      ),
      header: "Artifact",
      id: "artifact",
      minSize: 180,
      searchValue: (row) => `${row.name} ${row.type} ${row.id}`,
      size: 240,
      sortValue: (row) => row.name,
    },
    {
      cell: (row) => (
        <span className="historyPrimary">
          <strong>{row.type}</strong>
          <small>{row.sourceDetail}</small>
        </span>
      ),
      header: "Type",
      id: "type",
      minSize: 150,
      searchValue: (row) => `${row.type} ${row.sourceDetail}`,
      size: 180,
      sortValue: (row) => row.type,
    },
    {
      cell: (row) => (
        <button
          className="linkButton"
          onClick={(event) => {
            event.stopPropagation();
            openSourceWorkflow(row, {
              onOpenAgentUpdates,
              onOpenBackupsArtifacts,
              onOpenTransfers,
            });
          }}
          type="button"
        >
          {row.sourceWorkflow}
        </button>
      ),
      header: "Source workflow",
      id: "source",
      minSize: 160,
      searchValue: (row) => row.sourceWorkflow,
      size: 190,
      sortValue: (row) => row.sourceWorkflow,
    },
    {
      cell: (row) => (
        <span className="historyPrimary">
          <strong>{row.relationLabel}</strong>
          <small>{row.id.replace(/^[^:]+:/, "")}</small>
        </span>
      ),
      header: "VPS / job",
      id: "relation",
      minSize: 150,
      searchValue: (row) => `${row.relationLabel} ${row.id}`,
      size: 190,
      sortValue: (row) => row.relationLabel,
    },
    {
      cell: (row) => formatTime(row.createdAt),
      header: "Created",
      id: "created",
      minSize: 130,
      searchValue: (row) => row.createdAt,
      size: 150,
      sortValue: (row) => row.createdAt,
    },
    {
      cell: (row) =>
        row.sizeBytes === null ? "Unknown" : formatBytes(row.sizeBytes),
      header: "Size",
      id: "size",
      minSize: 95,
      searchValue: (row) => row.sizeBytes ?? "",
      size: 110,
      sortValue: (row) => row.sizeBytes ?? -1,
    },
    {
      cell: (row) => (
        <span className="historyPrimary">
          <span
            className={`status ${artifactVerificationClass(row.verification)}`}
          >
            {row.verification}
          </span>
          <small>{row.verificationDetail}</small>
        </span>
      ),
      header: "Verification",
      id: "verification",
      minSize: 160,
      searchValue: (row) =>
        `${row.verification} ${row.verificationDetail} ${row.rawStatus}`,
      size: 190,
      sortValue: (row) => row.verification,
    },
    {
      cell: (row) => (
        <button
          className="secondaryAction compactAction"
          onClick={(event) => {
            event.stopPropagation();
            openSourceWorkflow(row, {
              onOpenAgentUpdates,
              onOpenBackupsArtifacts,
              onOpenTransfers,
            });
          }}
          type="button"
        >
          {row.actionLabel}
        </button>
      ),
      enableHiding: false,
      header: "Action",
      id: "action",
      minSize: 130,
      searchValue: (row) => row.actionLabel,
      size: 150,
      sortValue: (row) => row.actionLabel,
    },
  ];

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Job artifacts</h2>
            <span>
              Read-only cross-domain execution artifacts, separated from
              cleanup.
            </span>
          </div>
        </div>
        <section
          className="jobArtifactsSummary"
          aria-label="Job artifact inventory summary"
        >
          <div>
            <span>Artifact types</span>
            <strong>{artifactTypes.length}</strong>
            <small>backup, transfer, and update artifact types</small>
          </div>
          <div>
            <span>Records</span>
            <strong>{rows.length}</strong>
            <small>linked to source workflows</small>
          </div>
          <div>
            <span>Stored bytes</span>
            <strong>{formatBytes(totalBytes)}</strong>
            <small>known artifact sizes only</small>
          </div>
          <div>
            <span>Cleanup boundary</span>
            <strong>System / Maintenance</strong>
            <small>no destructive controls on this inventory page</small>
          </div>
        </section>
        <section
          className="jobArtifactSourceLinks"
          aria-label="Artifact source workflow links"
        >
          <button
            className="secondaryAction"
            onClick={onOpenBackupsArtifacts}
            type="button"
          >
            <Archive size={16} />
            Backups / Artifacts
          </button>
          <button
            className="secondaryAction"
            onClick={onOpenTransfers}
            type="button"
          >
            <ExternalLink size={16} />
            Remote Operations / Transfers
          </button>
          <button
            className="secondaryAction"
            onClick={onOpenAgentUpdates}
            type="button"
          >
            <ExternalLink size={16} />
            Automation / Agent updates
          </button>
        </section>
        <ConsoleDataGrid
          columns={columns}
          defaultColumnVisibility={{ created: false }}
          defaultPageSize={10}
          empty={
            <div className="emptyState">
              <Archive size={20} />
              <strong>No artifact records</strong>
              <span>
                Execution artifact records will appear here after source
                workflows create them.
              </span>
            </div>
          }
          getRowId={(row) => row.id}
          itemLabel="artifacts"
          renderExpandedRow={(row) => (
            <div className="consoleInlineDetailGrid artifactDetailGrid">
              <span>
                <strong>Object key / URL</strong>
                <span title={row.objectKey}>{row.objectKey}</span>
                <button
                  className="secondaryAction compactAction"
                  onClick={() => void copyText(row.objectKey)}
                  type="button"
                >
                  <Copy size={13} />
                  Copy
                </button>
              </span>
              <span>
                <strong>SHA-256</strong>
                <span title={row.sha256Hex}>{row.sha256Hex}</span>
                <button
                  className="secondaryAction compactAction"
                  onClick={() => void copyText(row.sha256Hex)}
                  type="button"
                >
                  <Copy size={13} />
                  Copy
                </button>
              </span>
              <span>
                <strong>Source</strong>
                <span>{row.sourceWorkflow}</span>
                <span>{row.relationLabel}</span>
              </span>
              <span>
                <strong>Verification evidence</strong>
                <span>{row.verificationDetail}</span>
                <span>Raw status: {row.rawStatus}</span>
              </span>
              <span>
                <strong>Download path</strong>
                <span>{row.downloadPath ?? "Handled by source workflow"}</span>
                {row.downloadPath ? (
                  <button
                    className="secondaryAction compactAction"
                    onClick={() => void copyText(row.downloadPath ?? "")}
                    type="button"
                  >
                    <Copy size={13} />
                    Copy
                  </button>
                ) : null}
              </span>
            </div>
          )}
          rows={visibleRows}
          searchPlaceholder="Search artifacts"
          selectable={false}
          storageKey="vpsman.grid.jobs.artifacts"
          toolbarActions={
            <label className="jobArtifactTypeFilter">
              <span>Type</span>
              <select
                aria-label="Artifact type filter"
                onChange={(event) => setTypeFilter(event.target.value)}
                value={typeFilter}
              >
                <option value="all">All types</option>
                {artifactTypes.map((type) => (
                  <option key={type} value={type}>
                    {type}
                  </option>
                ))}
              </select>
            </label>
          }
          title="Job artifact inventory"
        />
      </div>
    </section>
  );
}

function openSourceWorkflow(
  row: ArtifactInventoryRow,
  links: Pick<
    JobArtifactsPanelProps,
    "onOpenAgentUpdates" | "onOpenBackupsArtifacts" | "onOpenTransfers"
  >,
) {
  if (row.sourceWorkflow === "Backups / Artifacts") {
    links.onOpenBackupsArtifacts();
    return;
  }
  if (row.sourceWorkflow === "Remote Operations / Transfers") {
    links.onOpenTransfers();
    return;
  }
  links.onOpenAgentUpdates();
}

function buildArtifactInventoryRows({
  agentUpdateReleases,
  backupArtifacts,
  fileTransferSources,
}: {
  agentUpdateReleases: AgentUpdateReleaseRecord[];
  backupArtifacts: BackupArtifactRecord[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
}): ArtifactInventoryRow[] {
  const backupRows = backupArtifacts.map((artifact) => ({
    actionLabel: "Open backup",
    createdAt: artifact.created_at,
    detailLabel: `Backup ${shortId(artifact.id)}`,
    downloadPath: null,
    id: `backup:${artifact.id}`,
    name: `Backup artifact ${shortId(artifact.id)}`,
    objectKey: artifact.object_key,
    rawStatus: artifact.status,
    relationLabel: artifact.client_id,
    sha256Hex: artifact.sha256_hex,
    sizeBytes: artifact.size_bytes,
    sourceDetail: "Backup request output",
    sourceWorkflow: "Backups / Artifacts",
    type: "Backup artifact",
    ...artifactVerification(artifact.status),
  }));
  const transferRows = fileTransferSources.map((source) => ({
    actionLabel: "Open transfers",
    createdAt: source.created_at,
    detailLabel: source.name,
    downloadPath: source.download_path,
    id: `file-transfer-source:${source.id}`,
    name: source.name,
    objectKey: source.object_key,
    rawStatus: source.status,
    relationLabel: source.created_by
      ? `Operator ${shortId(source.created_by)}`
      : "Uploaded source",
    sha256Hex: source.sha256_hex,
    sizeBytes: source.size_bytes,
    sourceDetail: "Reusable upload source",
    sourceWorkflow: "Remote Operations / Transfers",
    type: "Transfer package",
    ...artifactVerification(source.status),
  }));
  const releaseRows = agentUpdateReleases.flatMap((release) => {
    const rows: ArtifactInventoryRow[] = [
      {
        actionLabel: "Open update",
        createdAt: release.created_at,
        detailLabel: `${release.channel} channel`,
        downloadPath: null,
        id: `agent-update:${release.id}:artifact`,
        name: `${release.name} ${release.version}`,
        objectKey: release.artifact_url_sha256_hex
          ? `url hash ${shortHash(release.artifact_url_sha256_hex)}`
          : "artifact URL hash unavailable",
        rawStatus: release.status,
        relationLabel: `Release ${shortId(release.id)}`,
        sha256Hex: release.artifact_sha256_hex,
        sizeBytes: release.size_bytes,
        sourceDetail: "Primary agent update",
        sourceWorkflow: "Automation / Agent updates",
        type: "Agent update bundle",
        ...artifactVerification(release.status),
      },
    ];
    if (release.rollback_artifact_sha256_hex) {
      rows.push({
        actionLabel: "Open update",
        createdAt: release.created_at,
        detailLabel: `${release.channel} channel`,
        downloadPath: null,
        id: `agent-update:${release.id}:rollback`,
        name: `${release.name} ${release.version} rollback`,
        objectKey: release.rollback_artifact_url_sha256_hex
          ? `url hash ${shortHash(release.rollback_artifact_url_sha256_hex)}`
          : "rollback URL hash unavailable",
        rawStatus: release.status,
        relationLabel: `Release ${shortId(release.id)}`,
        sha256Hex: release.rollback_artifact_sha256_hex,
        sizeBytes: release.rollback_size_bytes,
        sourceDetail: "Rollback bundle",
        sourceWorkflow: "Automation / Agent updates",
        type: "Agent rollback bundle",
        ...artifactVerification(release.status),
      });
    }
    return rows;
  });
  return [...backupRows, ...transferRows, ...releaseRows].sort((left, right) =>
    right.createdAt.localeCompare(left.createdAt),
  );
}

function artifactVerification(
  status: string,
): Pick<ArtifactInventoryRow, "verification" | "verificationDetail"> {
  const normalized = status.toLowerCase();
  if (
    normalized.includes("expired") ||
    normalized.includes("deleted") ||
    normalized.includes("pruned")
  ) {
    return {
      verification: "Expired",
      verificationDetail: "Artifact reference is no longer usable",
    };
  }
  if (
    normalized.includes("failed") ||
    normalized.includes("mismatch") ||
    normalized.includes("invalid")
  ) {
    return {
      verification: "Verification failed",
      verificationDetail: "Hash, upload, or publication check failed",
    };
  }
  if (
    normalized.includes("creating") ||
    normalized.includes("upload") ||
    normalized.includes("pending") ||
    normalized.includes("partial")
  ) {
    return {
      verification: "Upload incomplete",
      verificationDetail: "Source workflow has not finished recording bytes",
    };
  }
  return {
    verification: "Ready",
    verificationDetail: "Recorded with SHA-256 evidence",
  };
}

function artifactVerificationClass(
  verification: ArtifactInventoryRow["verification"],
): string {
  switch (verification) {
    case "Ready":
      return "ok";
    case "Upload incomplete":
      return "warn";
    case "Verification failed":
      return "warn";
    case "Expired":
      return "neutral";
    default:
      return "neutral";
  }
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }
  return `${size >= 10 || unitIndex === 0 ? size.toFixed(0) : size.toFixed(1)} ${units[unitIndex]}`;
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}
