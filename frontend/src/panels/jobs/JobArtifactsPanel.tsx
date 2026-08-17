import { useMemo, useState } from "react";
import { useByteCountFormatter } from "../../panelDisplay";
import { Archive, Copy, ExternalLink } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { ActionFeedback } from "../../components/ActionFeedback";
import { formatLowerBoundCount } from "../../constants";
import type {
  AgentUpdateReleaseRecord,
  BackupArtifactRecord,
} from "../../types";
import type { FileTransferSourceArtifactRecord } from "../../typesFileTransfer";
import { formatTime, shortHash, shortId } from "../../utils";

type JobArtifactsPanelProps = {
  agentUpdateReleases: AgentUpdateReleaseRecord[];
  agentUpdateReleasesTruncated: boolean;
  backupArtifacts: BackupArtifactRecord[];
  backupArtifactsTruncated: boolean;
  error: string | null;
  fileTransferSources: FileTransferSourceArtifactRecord[];
  fileTransferSourcesTruncated: boolean;
  loading: boolean;
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
  agentUpdateReleasesTruncated,
  backupArtifacts,
  backupArtifactsTruncated,
  error,
  fileTransferSources,
  fileTransferSourcesTruncated,
  loading,
  onOpenAgentUpdates,
  onOpenBackupsArtifacts,
  onOpenTransfers,
}: JobArtifactsPanelProps) {
  const formatBytes = useByteCountFormatter();
  const [typeFilter, setTypeFilter] = useState("all");
  const rowsTruncated =
    agentUpdateReleasesTruncated ||
    backupArtifactsTruncated ||
    fileTransferSourcesTruncated;
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
      cell: (row) => <span>{row.sourceWorkflow}</span>,
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
            title={row.verificationDetail}
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
        <ActionFeedback
          className="localActionFeedback"
          message={error ?? (loading ? "Refreshing artifact inventory" : null)}
          tone={error ? "danger" : "progress"}
        />
        <section
          className="jobArtifactsSummary"
          aria-label="Job artifact inventory summary"
        >
          <div
            title={`${formatLowerBoundCount(artifactTypes.length, rowsTruncated)} artifact types are represented${rowsTruncated ? " in loaded source pages" : ""}.`}
          >
            <span>Artifact types</span>
            <strong>
              {formatLowerBoundCount(artifactTypes.length, rowsTruncated)}
            </strong>
            <small>
              {rowsTruncated
                ? "types in loaded backup, transfer, and update pages"
                : "backup, transfer, and update artifact types"}
            </small>
          </div>
          <div
            title={`${formatLowerBoundCount(rows.length, rowsTruncated)} artifact records are linked to source workflows${rowsTruncated ? " in loaded pages" : ""}.`}
          >
            <span>Records</span>
            <strong>{formatLowerBoundCount(rows.length, rowsTruncated)}</strong>
            <small>
              linked to source workflows
              {rowsTruncated ? " in loaded pages" : ""}
            </small>
          </div>
          <div
            title={`${rowsTruncated ? "At least " : ""}${formatBytes(totalBytes)} is recorded across artifacts with known sizes.`}
          >
            <span>Stored bytes</span>
            <strong>
              {rowsTruncated ? "≥" : ""}
              {formatBytes(totalBytes)}
            </strong>
            <small>
              known artifact sizes only{rowsTruncated ? " in loaded pages" : ""}
            </small>
          </div>
          <div title="Artifact deletion is intentionally separated into System / Maintenance; this inventory is read-only.">
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
            Remote / Transfers
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
          actions={[
            {
              description: (rows) =>
                rows.length === 1
                  ? `${rows[0].actionLabel} for this artifact.`
                  : "Select exactly one artifact to open its source workflow.",
              disabled: (rows) => rows.length !== 1,
              label: "Open source workflow",
              onSelect: (rows) =>
                openSourceWorkflow(rows[0], {
                  onOpenAgentUpdates,
                  onOpenBackupsArtifacts,
                  onOpenTransfers,
                }),
            },
            {
              description: (rows) =>
                `Copy ${rows.length} artifact object key${rows.length === 1 ? "" : "s"}.`,
              icon: <Copy size={14} />,
              label: "Copy object keys",
              onSelect: (rows) =>
                void copyText(rows.map((row) => row.objectKey).join("\n")),
            },
            {
              description: (rows) =>
                `Copy ${rows.length} artifact SHA-256 value${rows.length === 1 ? "" : "s"}.`,
              icon: <Copy size={14} />,
              label: "Copy SHA-256",
              onSelect: (rows) =>
                void copyText(rows.map((row) => row.sha256Hex).join("\n")),
            },
            {
              description: (rows) => {
                const count = rows.filter((row) => row.downloadPath).length;
                return `Copy the ${count} available download path${count === 1 ? "" : "s"}.`;
              },
              disabled: (rows) => !rows.some((row) => row.downloadPath),
              icon: <Copy size={14} />,
              label: "Copy download paths",
              onSelect: (rows) =>
                void copyText(
                  rows
                    .flatMap((row) =>
                      row.downloadPath ? [row.downloadPath] : [],
                    )
                    .join("\n"),
                ),
            },
          ]}
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
          pageResetKey={typeFilter}
          renderExpandedRow={(row) => (
            <div className="consoleInlineDetailGrid artifactDetailGrid">
              <span>
                <strong>Object key / URL</strong>
                <span title={row.objectKey}>{row.objectKey}</span>
              </span>
              <span>
                <strong>SHA-256</strong>
                <span title={row.sha256Hex}>{row.sha256Hex}</span>
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
              </span>
            </div>
          )}
          rows={visibleRows}
          rowsTruncated={rowsTruncated}
          searchPlaceholder="Search artifacts"
          showMobileRowActions={false}
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
  if (row.sourceWorkflow === "Remote / Transfers") {
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
    sourceWorkflow: "Remote / Transfers",
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

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}
