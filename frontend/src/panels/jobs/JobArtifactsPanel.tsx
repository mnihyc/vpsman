import { Archive, ExternalLink } from "lucide-react";
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
  createdAt: string;
  domain: string;
  id: string;
  name: string;
  objectKey: string;
  relation: string;
  sha256Hex: string;
  sizeBytes: number | null;
  sourceWorkflow: string;
  status: string;
};

export function JobArtifactsPanel({
  agentUpdateReleases,
  backupArtifacts,
  fileTransferSources,
  onOpenAgentUpdates,
  onOpenBackupsArtifacts,
  onOpenTransfers,
}: JobArtifactsPanelProps) {
  const rows = buildArtifactInventoryRows({
    agentUpdateReleases,
    backupArtifacts,
    fileTransferSources,
  });
  const totalBytes = rows.reduce(
    (sum, row) => sum + (row.sizeBytes ?? 0),
    0,
  );
  const domains = new Set(rows.map((row) => row.domain));
  const columns: ConsoleDataGridColumn<ArtifactInventoryRow>[] = [
    {
      cell: (row) => (
        <span className="historyPrimary">
          <strong title={row.name}>{row.name}</strong>
          <small>{row.domain}</small>
        </span>
      ),
      header: "Artifact",
      id: "artifact",
      minSize: 180,
      searchValue: (row) => `${row.name} ${row.domain} ${row.id}`,
      size: 240,
      sortValue: (row) => row.name,
    },
    {
      cell: (row) => <span className="status">{row.status}</span>,
      header: "Status",
      id: "status",
      minSize: 100,
      searchValue: (row) => row.status,
      size: 120,
      sortValue: (row) => row.status,
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
        <span className="monoValue" title={row.objectKey}>
          {row.objectKey}
        </span>
      ),
      header: "Object / URL",
      id: "object",
      minSize: 180,
      searchValue: (row) => row.objectKey,
      size: 300,
      sortValue: (row) => row.objectKey,
    },
    {
      cell: (row) => (
        <span className="monoValue" title={row.sha256Hex}>
          {shortHash(row.sha256Hex)}
        </span>
      ),
      header: "SHA-256",
      id: "sha",
      minSize: 90,
      searchValue: (row) => row.sha256Hex,
      size: 105,
      sortValue: (row) => row.sha256Hex,
    },
    {
      cell: (row) =>
        row.sizeBytes === null ? "unknown" : formatBytes(row.sizeBytes),
      header: "Size",
      id: "size",
      minSize: 95,
      searchValue: (row) => row.sizeBytes ?? "",
      size: 110,
      sortValue: (row) => row.sizeBytes ?? -1,
    },
    {
      cell: (row) => formatTime(row.createdAt),
      header: "Created",
      id: "created",
      minSize: 130,
      searchValue: (row) => row.createdAt,
      size: 160,
      sortValue: (row) => row.createdAt,
    },
  ];

  return (
    <section className="workspace singleColumn">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Job artifacts</h2>
            <span>Read-only cross-domain execution artifacts, separated from cleanup.</span>
          </div>
        </div>
        <section className="jobArtifactsSummary" aria-label="Job artifact inventory summary">
          <div>
            <span>Domains</span>
            <strong>{domains.size}</strong>
            <small>backup, file transfer, and agent update sources</small>
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
        <section className="jobArtifactSourceLinks" aria-label="Artifact source workflow links">
          <button className="secondaryAction" onClick={onOpenBackupsArtifacts} type="button">
            <Archive size={16} />
            Backups / Artifacts
          </button>
          <button className="secondaryAction" onClick={onOpenTransfers} type="button">
            <ExternalLink size={16} />
            Remote Operations / Transfers
          </button>
          <button className="secondaryAction" onClick={onOpenAgentUpdates} type="button">
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
              <span>Execution artifact records will appear here after source workflows create them.</span>
            </div>
          }
          getRowId={(row) => row.id}
          itemLabel="artifacts"
          renderExpandedRow={(row) => (
            <div className="gridDetailLine">
              <strong>{row.sourceWorkflow}</strong>
              <span>{row.relation}</span>
              <span>{row.status}</span>
              <span>{row.objectKey}</span>
              <span>{shortHash(row.sha256Hex)}</span>
            </div>
          )}
          rows={rows}
          searchPlaceholder="Search artifacts"
          selectable={false}
          storageKey="vpsman.grid.jobs.artifacts"
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
    createdAt: artifact.created_at,
    domain: "backup",
    id: `backup:${artifact.id}`,
    name: shortId(artifact.id),
    objectKey: artifact.object_key,
    relation: artifact.client_id,
    sha256Hex: artifact.sha256_hex,
    sizeBytes: artifact.size_bytes,
    sourceWorkflow: "Backups / Artifacts",
    status: artifact.status,
  }));
  const transferRows = fileTransferSources.map((source) => ({
    createdAt: source.created_at,
    domain: "file_transfer_source",
    id: `file-transfer-source:${source.id}`,
    name: source.name,
    objectKey: source.object_key,
    relation: source.download_path,
    sha256Hex: source.sha256_hex,
    sizeBytes: source.size_bytes,
    sourceWorkflow: "Remote Operations / Transfers",
    status: source.status,
  }));
  const releaseRows = agentUpdateReleases.flatMap((release) => {
    const rows: ArtifactInventoryRow[] = [
      {
        createdAt: release.created_at,
        domain: "agent_update",
        id: `agent-update:${release.id}:artifact`,
        name: `${release.name} ${release.version}`,
        objectKey: release.artifact_url_sha256_hex
          ? `url hash ${shortHash(release.artifact_url_sha256_hex)}`
          : "artifact URL hash unavailable",
        relation: release.channel,
        sha256Hex: release.artifact_sha256_hex,
        sizeBytes: release.size_bytes,
        sourceWorkflow: "Automation / Agent updates",
        status: release.status,
      },
    ];
    if (release.rollback_artifact_sha256_hex) {
      rows.push({
        createdAt: release.created_at,
        domain: "agent_update_rollback",
        id: `agent-update:${release.id}:rollback`,
        name: `${release.name} ${release.version} rollback`,
        objectKey: release.rollback_artifact_url_sha256_hex
          ? `url hash ${shortHash(release.rollback_artifact_url_sha256_hex)}`
          : "rollback URL hash unavailable",
        relation: release.channel,
        sha256Hex: release.rollback_artifact_sha256_hex,
        sizeBytes: release.rollback_size_bytes,
        sourceWorkflow: "Automation / Agent updates",
        status: release.status,
      });
    }
    return rows;
  });
  return [...backupRows, ...transferRows, ...releaseRows].sort((left, right) =>
    right.createdAt.localeCompare(left.createdAt),
  );
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
