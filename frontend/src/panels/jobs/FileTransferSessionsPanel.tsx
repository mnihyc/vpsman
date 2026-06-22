import { Database, Download, FileArchive, RefreshCw, Upload } from "lucide-react";
import { useEffect, useState } from "react";
import type { ArtifactDownloadMode } from "../../artifactDownload";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  artifactLifecycleStatusBadgeClass,
  fileTransferSessionStatusBadgeClass,
} from "../../jobStatusPresentation";
import type {
  FileTransferHandoffRecord,
  FileTransferSessionRecord,
  FileTransferSourceArtifactRecord,
  UploadFileTransferSourceArtifactRequest,
} from "../../typesFileTransfer";
import { formatTime, shortHash, shortId } from "../../utils";

const MAX_SOURCE_ARTIFACT_BYTES = 16 * 1024 * 1024;

type HandoffReviewItem = {
  clientId: string;
  clientLabel: string;
  fileName: string;
  key: string;
  path: string;
  sessionId: string;
  evidenceReason: string | null;
  evidenceStatus: string;
  sha256Hex: string | null;
  sizeBytes: number | null;
};

type HandoffReviewSnapshot = {
  mode: ArtifactDownloadMode;
  transfers: HandoffReviewItem[];
};

export function FileTransferSessionsPanel({
  clientLabel,
  transfers,
  sources,
  loading,
  onCreateHandoff,
  onDownloadSource,
  onRefresh,
  onSaveHandoff,
  onUploadSource,
}: {
  clientLabel: (clientId: string) => string;
  transfers: FileTransferSessionRecord[];
  sources: FileTransferSourceArtifactRecord[];
  loading: boolean;
  onCreateHandoff: (clientId: string, sessionId: string) => Promise<FileTransferHandoffRecord>;
  onDownloadSource: (downloadPath: string) => Promise<Blob>;
  onRefresh: () => void;
  onSaveHandoff: (
    downloadPath: string,
    request: {
      expectedSha256Hex?: string | null;
      expectedSizeBytes?: number | null;
      fileName: string;
      mode: ArtifactDownloadMode;
    },
  ) => Promise<void>;
  onUploadSource: (request: UploadFileTransferSourceArtifactRequest) => Promise<FileTransferSourceArtifactRecord>;
}) {
  const [handoffPendingKey, setHandoffPendingKey] = useState<string | null>(null);
  const [handoffError, setHandoffError] = useState<string | null>(null);
  const [handoffDownloadMode, setHandoffDownloadMode] = useState<ArtifactDownloadMode>("browser-download");
  const [handoffProgress, setHandoffProgress] = useState<string | null>(null);
  const [handoffSnapshot, setHandoffSnapshot] = useState<HandoffReviewSnapshot | null>(null);
  const [selectedHandoffKeys, setSelectedHandoffKeys] = useState<string[]>([]);
  const [sourceError, setSourceError] = useState<string | null>(null);
  const [sourceFile, setSourceFile] = useState<File | null>(null);
  const [sourceInputKey, setSourceInputKey] = useState(0);
  const [sourceName, setSourceName] = useState("");
  const [sourcePending, setSourcePending] = useState(false);
  const [sourcePendingId, setSourcePendingId] = useState<string | null>(null);
  const [sourceSnapshot, setSourceSnapshot] = useState<{
    fileName: string;
    request: UploadFileTransferSourceArtifactRequest;
  } | null>(null);
  const handoffCandidates = transfers.filter(canCreateHandoff);
  const completedDownloads = transfers.filter(
    (transfer) => transfer.direction === "download" && transfer.status === "completed",
  );
  const unavailableCompletedDownloads = Math.max(0, completedDownloads.length - handoffCandidates.length);
  const selectedHandoffKeySet = new Set(selectedHandoffKeys);
  const selectedHandoffTransfers = handoffCandidates.filter((transfer) => selectedHandoffKeySet.has(transferKey(transfer)));
  const handoffBusy = handoffPendingKey !== null;
  const handoffSummary = handoffError ?? handoffProgress ?? `${transfers.length} resumable upload/download states`;
  const sourceColumns: ConsoleDataGridColumn<FileTransferSourceArtifactRecord>[] = [
    {
      cell: (source) => (
        <span className="historyPrimary">
          <strong>{source.name}</strong>
          <small>{shortHash(source.sha256_hex)}</small>
        </span>
      ),
      header: "Artifact",
      id: "artifact",
      searchValue: (source) => `${source.name} ${source.sha256_hex}`,
      sortValue: (source) => source.name,
    },
    {
      cell: (source) => (
        <span
          className={`sourceArtifactStatus status ${artifactLifecycleStatusBadgeClass(source.status)}`}
          title={artifactLifecycleStatusTitle(source.status)}
        >
          {source.status}
        </span>
      ),
      header: "Status",
      id: "status",
      searchValue: (source) => source.status,
      sortValue: (source) => source.status,
    },
    {
      cell: (source) => (
        <span className="sourceArtifactMeta historyPrimary">
          <strong>{formatBytes(source.size_bytes)}</strong>
          <small>{formatTime(source.created_at)}</small>
        </span>
      ),
      header: "Size",
      id: "size",
      searchValue: (source) => `${source.size_bytes} ${formatTime(source.created_at)}`,
      sortValue: (source) => source.size_bytes,
    },
    {
      cell: (source) => (
        <button
          aria-label={`Download source artifact ${source.name}`}
          className="sourceArtifactDownload iconButton"
          disabled={
            sourcePendingId === source.id ||
            source.status === "creating" ||
            source.status === "deleting"
          }
          onClick={(event) => {
            event.stopPropagation();
            void downloadSourceArtifact(source);
          }}
          title={
            source.status === "creating" || source.status === "deleting"
              ? artifactLifecycleStatusTitle(source.status)
              : "Download source artifact"
          }
          type="button"
        >
          <Download size={14} />
        </button>
      ),
      enableHiding: false,
      header: "Action",
      id: "action",
    },
  ];
  const transferColumns: ConsoleDataGridColumn<FileTransferSessionRecord>[] = [
    {
      cell: (transfer) => {
        const key = transferKey(transfer);
        const selectable = canCreateHandoff(transfer);
        const evidenceLabel = handoffEvidenceLabel(transfer);
        const evidenceTitle = handoffEvidenceTitle(transfer);
        return (
          <span className="rowSelectCell">
            {selectable ? (
              <input
                aria-label={`Select transfer handoff session ${shortId(transfer.session_id)}`}
                checked={selectedHandoffKeySet.has(key)}
                disabled={handoffBusy}
                onChange={(event) => toggleHandoffSelection(transfer, event.target.checked)}
                onClick={(event) => event.stopPropagation()}
                type="checkbox"
              />
            ) : (
              <small title={evidenceTitle}>{evidenceLabel}</small>
            )}
          </span>
        );
      },
      enableHiding: false,
      header: "Select",
      id: "select",
    },
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <strong>{transfer.direction}</strong>
          <small>
            {clientLabel(transfer.client_id)} / {shortId(transfer.session_id)}
          </small>
        </span>
      ),
      header: "Session",
      id: "session",
      searchValue: (transfer) => `${clientLabel(transfer.client_id)} ${transfer.client_id} ${transfer.session_id} ${transfer.direction}`,
      sortValue: (transfer) => `${transfer.direction}:${clientLabel(transfer.client_id)}`,
    },
    {
      cell: (transfer) => {
        const evidenceLabel = handoffEvidenceLabel(transfer);
        const evidenceTitle = handoffEvidenceTitle(transfer);
        return (
          <span className="historyPrimary">
            <span className={`status ${fileTransferSessionStatusBadgeClass(transfer.status)}`}>{transfer.status}</span>
            <small title={transfer.direction === "download" ? evidenceTitle : transfer.last_event}>
              {transfer.direction === "download" ? evidenceLabel : transfer.resumed ? "resumed" : transfer.last_event}
            </small>
          </span>
        );
      },
      header: "Status",
      id: "status",
      searchValue: (transfer) => `${transfer.status} ${transfer.last_event} ${handoffEvidenceLabel(transfer)}`,
      sortValue: (transfer) => transfer.status,
    },
    {
      cell: (transfer) => (
        <span className="transferProgressCell">
          <span>{formatTransferProgress(transfer)}</span>
          <span className="transferProgressTrack">
            <span style={{ width: `${Math.round((transfer.progress_ratio ?? 0) * 100)}%` }} />
          </span>
        </span>
      ),
      header: "Progress",
      id: "progress",
      searchValue: (transfer) => formatTransferProgress(transfer),
      sortValue: (transfer) => transfer.progress_ratio ?? 0,
    },
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <strong title={transfer.path}>{transfer.path}</strong>
          <small>{transfer.sha256_hex ? shortHash(transfer.sha256_hex) : transfer.last_command_type}</small>
        </span>
      ),
      header: "Path",
      id: "path",
      searchValue: (transfer) => `${transfer.path} ${transfer.sha256_hex ?? ""} ${transfer.last_command_type}`,
      sortValue: (transfer) => transfer.path,
    },
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <strong>{transfer.rate_limit_kbps ? `${transfer.rate_limit_kbps} kbps` : "unlimited"}</strong>
          <small>{formatChunkInfo(transfer)}</small>
        </span>
      ),
      header: "Rate",
      id: "rate",
      searchValue: (transfer) => `${transfer.rate_limit_kbps ?? "unlimited"} ${formatChunkInfo(transfer)}`,
      sortValue: (transfer) => transfer.rate_limit_kbps ?? 0,
    },
    {
      cell: (transfer) => formatTime(transfer.observed_at),
      header: "Observed",
      id: "observed",
      searchValue: (transfer) => formatTime(transfer.observed_at),
      sortValue: (transfer) => transfer.observed_at,
    },
    {
      cell: (transfer) => {
        const key = transferKey(transfer);
        const selectable = canCreateHandoff(transfer);
        const evidenceLabel = handoffEvidenceLabel(transfer);
        const evidenceTitle = handoffEvidenceTitle(transfer);
        return (
          <span className="rowActions">
            {selectable ? (
              <button
                aria-label={`Create transfer handoff session ${shortId(transfer.session_id)}`}
                className="iconButton"
                disabled={handoffPendingKey === key || handoffPendingKey === "bulk"}
                onClick={(event) => {
                  event.stopPropagation();
                  reviewHandoff(transfer);
                }}
                title={handoffReadyTitle(transfer)}
                type="button"
              >
                <Download size={14} />
              </button>
            ) : (
              <small title={evidenceTitle}>{evidenceLabel}</small>
            )}
          </span>
        );
      },
      enableHiding: false,
      header: "Action",
      id: "action",
    },
  ];

  useEffect(() => {
    setSourceSnapshot(null);
  }, [sourceFile, sourceName]);

  useEffect(() => {
    setHandoffSnapshot(null);
  }, [handoffDownloadMode, selectedHandoffKeys]);

  function reviewHandoff(transfer: FileTransferSessionRecord) {
    setHandoffError(null);
    setHandoffSnapshot({
      mode: handoffDownloadMode,
      transfers: [handoffReviewItem(transfer, clientLabel)],
    });
  }

  function reviewSelectedHandoffs() {
    if (selectedHandoffTransfers.length === 0) {
      return;
    }
    setHandoffError(null);
    setHandoffSnapshot({
      mode: handoffDownloadMode,
      transfers: selectedHandoffTransfers.map((transfer) => handoffReviewItem(transfer, clientLabel)),
    });
  }

  async function createAndDownloadReviewedHandoffs() {
    if (!handoffSnapshot || handoffSnapshot.transfers.length === 0) {
      return;
    }
    const pendingKey = handoffSnapshot.transfers.length === 1 ? handoffSnapshot.transfers[0].key : "bulk";
    const completedKeys = new Set<string>();
    setHandoffPendingKey(pendingKey);
    setHandoffError(null);
    setHandoffProgress(null);
    try {
      for (const [index, transfer] of handoffSnapshot.transfers.entries()) {
        setHandoffProgress(`Downloading ${index + 1}/${handoffSnapshot.transfers.length}: ${transfer.clientLabel}`);
        const handoff = await onCreateHandoff(transfer.clientId, transfer.sessionId);
        await onSaveHandoff(handoff.download_path, {
          expectedSha256Hex: handoff.sha256_hex,
          expectedSizeBytes: handoff.size_bytes,
          fileName: transfer.fileName,
          mode: handoffSnapshot.mode,
        });
        completedKeys.add(transfer.key);
      }
      setHandoffProgress(`Downloaded ${handoffSnapshot.transfers.length} transfer handoffs`);
      setHandoffSnapshot(null);
    } catch (error) {
      setHandoffError(error instanceof Error ? error.message : "Transfer handoff failed");
    } finally {
      setSelectedHandoffKeys((keys) => keys.filter((key) => !completedKeys.has(key)));
      setHandoffPendingKey(null);
    }
  }

  function setAllHandoffSelection(selected: boolean) {
    setSelectedHandoffKeys(selected ? handoffCandidates.map(transferKey) : []);
  }

  function toggleHandoffSelection(transfer: FileTransferSessionRecord, selected: boolean) {
    const key = transferKey(transfer);
    const validKeys = new Set(handoffCandidates.map(transferKey));
    setSelectedHandoffKeys((keys) => {
      const next = new Set(keys.filter((existingKey) => validKeys.has(existingKey)));
      if (selected) {
        next.add(key);
      } else {
        next.delete(key);
      }
      return Array.from(next);
    });
  }

  async function reviewSourceArtifact() {
    if (!sourceFile) {
      setSourceError("Choose a source artifact first");
      return;
    }
    if (sourceFile.size > MAX_SOURCE_ARTIFACT_BYTES) {
      setSourceError(`Source artifact must be ${formatBytes(MAX_SOURCE_ARTIFACT_BYTES)} or smaller`);
      return;
    }
    setSourcePending(true);
    setSourceError(null);
    try {
      const bytes = new Uint8Array(await sourceFile.arrayBuffer());
      const [sha256Hex, sourceBase64] = await Promise.all([sha256HexForBytes(bytes), base64ForBytes(bytes)]);
      setSourceSnapshot({
        fileName: sourceFile.name,
        request: {
          name: sourceName.trim() || sourceFile.name || undefined,
          source_base64: sourceBase64,
          sha256_hex: sha256Hex,
          size_bytes: bytes.byteLength,
          confirmed: true,
        },
      });
    } catch (error) {
      setSourceError(error instanceof Error ? error.message : "Source artifact review failed");
    } finally {
      setSourcePending(false);
    }
  }

  async function uploadSourceArtifact() {
    if (!sourceSnapshot) {
      setSourceError("Review source artifact before upload");
      return;
    }
    setSourcePending(true);
    setSourceError(null);
    try {
      await onUploadSource(sourceSnapshot.request);
      setSourceSnapshot(null);
      setSourceFile(null);
      setSourceInputKey((key) => key + 1);
      setSourceName("");
    } catch (error) {
      setSourceError(error instanceof Error ? error.message : "Source artifact upload failed");
    } finally {
      setSourcePending(false);
    }
  }

  async function downloadSourceArtifact(source: FileTransferSourceArtifactRecord) {
    setSourcePendingId(source.id);
    setSourceError(null);
    try {
      const blob = await onDownloadSource(source.download_path);
      saveBlob(blob, downloadFileName(source.name));
    } catch (error) {
      setSourceError(error instanceof Error ? error.message : "Source artifact download failed");
    } finally {
      setSourcePendingId(null);
    }
  }

  return (
    <div className="fleetPanel">
      <div className="sectionHeader">
        <div>
          <h2>File transfer sessions</h2>
          <span>{handoffSummary}</span>
        </div>
        <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
          <RefreshCw size={14} />
          <span>Refresh</span>
        </button>
      </div>
      <div className="sourceArtifactPanel">
        <div className="sectionSubheader">
          <div>
            <h3>Source artifacts</h3>
            <span>{sourceError ?? `${sources.length} object-store source records`}</span>
          </div>
        </div>
        <div className="sourceArtifactControls">
          <label>
            <span>Source file</span>
            <input
              key={sourceInputKey}
              onChange={(event) => {
                const file = event.target.files?.[0] ?? null;
                setSourceFile(file);
                setSourceName(file?.name ?? "");
                setSourceError(null);
              }}
              type="file"
            />
          </label>
          <label>
            <span>Artifact name</span>
            <input
              onChange={(event) => setSourceName(event.target.value)}
              placeholder={sourceFile?.name ?? "payload.bin"}
              type="text"
              value={sourceName}
            />
          </label>
          <button
            className="primaryAction"
            disabled={sourcePending || !sourceFile || loading}
            onClick={() => void reviewSourceArtifact()}
            type="button"
          >
            <Upload size={14} />
            <span>{sourcePending ? "Reviewing" : "Review source artifact"}</span>
          </button>
        </div>
        <ConfirmationPrompt
          confirmLabel="Upload source artifact"
          detail="Persists the reviewed source artifact with the computed SHA-256 and size."
          items={[
            { label: "Name", value: sourceSnapshot?.request.name ?? sourceSnapshot?.fileName ?? "-" },
            {
              label: "SHA-256",
              title: sourceSnapshot?.request.sha256_hex,
              value: sourceSnapshot ? shortHash(sourceSnapshot.request.sha256_hex) : "-",
            },
            { label: "Size", value: sourceSnapshot ? formatBytes(sourceSnapshot.request.size_bytes) : "-" },
          ]}
          onCancel={() => setSourceSnapshot(null)}
          onConfirm={() => void uploadSourceArtifact()}
          open={sourceSnapshot !== null}
          pending={sourcePending}
          title="Confirm source artifact upload"
        />
        <ConsoleDataGrid
          columns={sourceColumns}
          defaultPageSize={6}
          expandOnRowClick
          getRowId={(source) => source.id}
          itemLabel="artifacts"
          empty={
            <div className="sourceArtifactEmpty">
              <Database size={18} />
              <span>No source artifacts</span>
            </div>
          }
          renderExpandedRow={(source) => (
            <div className="consoleInlineDetailGrid">
              <span>Artifact ID</span>
              <strong>{source.id}</strong>
              <span>Name</span>
              <strong>{source.name}</strong>
              <span>SHA-256</span>
              <strong>{source.sha256_hex}</strong>
              <span>Size</span>
              <strong>{formatBytes(source.size_bytes)}</strong>
              <span>Status</span>
              <strong>{source.status}</strong>
              <span>Created</span>
              <strong>{formatTime(source.created_at)}</strong>
            </div>
          )}
          rows={sources}
          searchPlaceholder="Search source artifacts"
          selectable={false}
          storageKey="vpsman.jobs.fileTransferSources"
          title="Source artifacts"
        />
      </div>
      <div className="handoffBulkBar">
        <span className="historyPrimary">
          <strong>Download handoffs</strong>
          <small>
            {handoffCandidates.length} handoff ready, {unavailableCompletedDownloads} unavailable,{" "}
            {selectedHandoffTransfers.length} selected
          </small>
        </span>
        <span className="handoffBulkActions">
          <label className="handoffModeControl">
            <span>Save method</span>
            <select
              aria-label="Transfer handoff save method"
              disabled={handoffBusy}
              onChange={(event) => setHandoffDownloadMode(event.target.value as ArtifactDownloadMode)}
              value={handoffDownloadMode}
            >
              <option value="browser-download">Browser download</option>
              <option value="stream-to-file">Stream to file</option>
            </select>
          </label>
          <button
            className="secondaryAction compactAction"
            disabled={handoffBusy || handoffCandidates.length === 0}
            onClick={() => setAllHandoffSelection(true)}
            title={
              handoffCandidates.length === 0
                ? "No completed downloads currently have retained handoff evidence."
                : "Select every handoff-ready completed download."
            }
            type="button"
          >
            Select all
          </button>
          <button
            className="secondaryAction compactAction"
            disabled={handoffBusy || selectedHandoffKeys.length === 0}
            onClick={() => setAllHandoffSelection(false)}
            type="button"
          >
            Clear
          </button>
          <button
            className="primaryAction compactAction"
            disabled={handoffBusy || selectedHandoffTransfers.length === 0}
            onClick={() => reviewSelectedHandoffs()}
            title={
              selectedHandoffTransfers.length === 0
                ? "Select one or more handoff-ready downloads first."
                : "Review selected handoff downloads before saving."
            }
            type="button"
          >
            <Download size={14} />
            <span>{handoffBusy && handoffPendingKey === "bulk" ? "Downloading" : "Review selected handoffs"}</span>
          </button>
        </span>
      </div>
      <ConfirmationPrompt
        confirmLabel="Create and download handoffs"
        detail="Creates server-side transfer handoffs for the reviewed completed download sessions, then saves them using the selected method."
        items={[
          { label: "Save method", value: handoffSnapshot?.mode ?? "-" },
          { label: "Transfers", value: handoffSnapshot ? String(handoffSnapshot.transfers.length) : "-" },
          { label: "Sessions", value: handoffSnapshot ? handoffSessionSummary(handoffSnapshot.transfers) : "-" },
          {
            label: "Expected hashes",
            title: handoffSnapshot ? handoffFullHashSummary(handoffSnapshot.transfers) : undefined,
            value: handoffSnapshot ? handoffHashSummary(handoffSnapshot.transfers) : "-",
          },
          {
            label: "Evidence",
            title: handoffSnapshot ? handoffFullEvidenceSummary(handoffSnapshot.transfers) : undefined,
            value: handoffSnapshot ? handoffEvidenceSummary(handoffSnapshot.transfers) : "-",
          },
        ]}
        onCancel={() => setHandoffSnapshot(null)}
        onConfirm={() => void createAndDownloadReviewedHandoffs()}
        open={handoffSnapshot !== null}
        pending={handoffBusy}
        title="Confirm transfer handoff download"
      />
      <ConsoleDataGrid
        columns={transferColumns}
        defaultPageSize={8}
        expandOnRowClick
        getRowId={transferKey}
        itemLabel="transfers"
        empty={
          <div className="emptyState">
            <FileArchive size={22} />
            <strong>No file transfer sessions</strong>
            <span>Resumable upload and download status events populate this inventory.</span>
          </div>
        }
        renderExpandedRow={(transfer) => (
          <div className="consoleInlineDetailGrid">
            <span>Session ID</span>
            <strong>{transfer.session_id}</strong>
            <span>VPS</span>
            <strong>{clientLabel(transfer.client_id)}</strong>
            <span>Path</span>
            <strong>{transfer.path}</strong>
            <span>SHA-256</span>
            <strong>{transfer.sha256_hex ?? "Not reported"}</strong>
            <span>Progress</span>
            <strong>{formatTransferProgress(transfer)}</strong>
            <span>Handoff evidence</span>
            <strong>{handoffEvidenceTitle(transfer)}</strong>
            <span>Last event</span>
            <strong>{transfer.last_event}</strong>
          </div>
        )}
        rows={transfers}
        searchPlaceholder="Search transfers"
        selectable={false}
        storageKey="vpsman.jobs.fileTransferSessions"
        title="Transfer records"
      />
    </div>
  );
}

function transferKey(transfer: FileTransferSessionRecord): string {
  return `${transfer.client_id}:${transfer.session_id}`;
}

function canCreateHandoff(transfer: FileTransferSessionRecord): boolean {
  return transfer.direction === "download" && transfer.status === "completed" && transfer.handoff_available;
}

function handoffReviewItem(
  transfer: FileTransferSessionRecord,
  clientLabel: (clientId: string) => string,
): HandoffReviewItem {
  return {
    clientId: transfer.client_id,
    clientLabel: clientLabel(transfer.client_id),
    fileName: downloadFileNameForTransfer(transfer, clientLabel),
    key: transferKey(transfer),
    path: transfer.path,
    sessionId: transfer.session_id,
    evidenceReason: transfer.handoff_unavailable_reason,
    evidenceStatus: transfer.handoff_evidence_status,
    sha256Hex: transfer.sha256_hex,
    sizeBytes: transfer.size_bytes,
  };
}

function handoffSessionSummary(transfers: HandoffReviewItem[]): string {
  const shown = transfers
    .slice(0, 3)
    .map((transfer) => `${transfer.clientLabel}/${shortId(transfer.sessionId)} ${transfer.path}`)
    .join(", ");
  return transfers.length > 3 ? `${shown}, +${transfers.length - 3} more` : shown;
}

function handoffHashSummary(transfers: HandoffReviewItem[]): string {
  const hashes = transfers.map((transfer) => transfer.sha256Hex).filter((hash): hash is string => Boolean(hash));
  if (hashes.length === 0) {
    return "not reported";
  }
  const shown = hashes.slice(0, 3).map(shortHash).join(", ");
  return hashes.length > 3 ? `${shown}, +${hashes.length - 3} more` : shown;
}

function handoffFullHashSummary(transfers: HandoffReviewItem[]): string {
  const hashes = transfers.map((transfer) => transfer.sha256Hex).filter((hash): hash is string => Boolean(hash));
  return hashes.length > 0 ? hashes.join(", ") : "not reported";
}

function handoffEvidenceSummary(transfers: HandoffReviewItem[]): string {
  const statuses = new Map<string, number>();
  for (const transfer of transfers) {
    statuses.set(transfer.evidenceStatus, (statuses.get(transfer.evidenceStatus) ?? 0) + 1);
  }
  return Array.from(statuses.entries())
    .map(([status, count]) => `${count} ${handoffEvidenceStatusLabel(status)}`)
    .join(", ");
}

function handoffFullEvidenceSummary(transfers: HandoffReviewItem[]): string {
  return transfers
    .map((transfer) => {
      const reason = transfer.evidenceReason ? ` (${transfer.evidenceReason.replace(/_/g, " ")})` : "";
      return `${transfer.clientLabel}/${shortId(transfer.sessionId)}: ${handoffEvidenceStatusLabel(transfer.evidenceStatus)}${reason}`;
    })
    .join(", ");
}

function handoffReadyTitle(transfer: FileTransferSessionRecord): string {
  if (transfer.handoff_evidence_status === "artifact_available") {
    return "Review handoff download from the retained server artifact.";
  }
  return "Review handoff download rebuilt from retained chunk outputs.";
}

function handoffEvidenceLabel(transfer: FileTransferSessionRecord): string {
  return handoffEvidenceStatusLabel(transfer.handoff_evidence_status);
}

function handoffEvidenceStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    artifact_available: "Stored artifact",
    retained_outputs_available: "Retained outputs",
    retained_outputs_pruned: "Evidence pruned",
    retained_outputs_incomplete: "Incomplete evidence",
    retained_outputs_conflict: "Conflicting chunks",
    missing_final_metadata: "Missing metadata",
    not_completed: "Not completed",
    not_applicable: "No handoff",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function handoffEvidenceTitle(transfer: FileTransferSessionRecord): string {
  const reason = transfer.handoff_unavailable_reason
    ? ` Reason: ${transfer.handoff_unavailable_reason.replace(/_/g, " ")}.`
    : "";
  switch (transfer.handoff_evidence_status) {
    case "artifact_available":
      return "A retained server-side handoff artifact exists for this completed download.";
    case "retained_outputs_available":
      return "Retained chunk output evidence is complete and can rebuild a server-side handoff artifact.";
    case "retained_outputs_pruned":
      return `The completed download remains visible, but the retained chunk outputs needed for a new handoff were pruned.${reason}`;
    case "retained_outputs_incomplete":
      return `The completed download remains visible, but retained chunk output evidence is incomplete.${reason}`;
    case "retained_outputs_conflict":
      return `The completed download remains visible, but duplicate chunk metadata conflicts and handoff is disabled.${reason}`;
    case "missing_final_metadata":
      return `The completed download is missing final size or SHA-256 metadata required for verified handoff.${reason}`;
    case "not_completed":
      return "Handoff is available after the download session completes.";
    case "not_applicable":
      return "Upload sessions do not create download handoff artifacts.";
    default:
      return `${handoffEvidenceStatusLabel(transfer.handoff_evidence_status)}.${reason}`;
  }
}

async function sha256HexForBytes(bytes: Uint8Array): Promise<string> {
  const normalized = new Uint8Array(bytes.byteLength);
  normalized.set(bytes);
  const digest = await window.crypto.subtle.digest("SHA-256", normalized.buffer);
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function base64ForBytes(bytes: Uint8Array): Promise<string> {
  const chunkSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < bytes.byteLength; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return window.btoa(binary);
}

function formatTransferProgress(transfer: FileTransferSessionRecord): string {
  const size = transfer.size_bytes;
  if (!size || size <= 0) {
    return `${formatBytes(transfer.progress_bytes)} transferred`;
  }
  const pct = Math.round((transfer.progress_ratio ?? 0) * 100);
  return `${formatBytes(transfer.progress_bytes)} / ${formatBytes(size)} (${pct}%)`;
}

function formatChunkInfo(transfer: FileTransferSessionRecord): string {
  const configured = transfer.chunk_size_bytes ? formatBytes(transfer.chunk_size_bytes) : "auto";
  const last = transfer.last_chunk_size_bytes ? formatBytes(transfer.last_chunk_size_bytes) : "-";
  return `chunk ${configured}, last ${last}`;
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

function formatBytes(value: number): string {
  if (value >= 1024 * 1024 * 1024) {
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GiB`;
  }
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${value} B`;
}

function downloadFileName(path: string): string {
  const name = path.split("/").filter(Boolean).pop() ?? "vpsman-transfer.bin";
  return sanitizeFileName(name, "vpsman-transfer.bin");
}

function downloadFileNameForTransfer(transfer: FileTransferSessionRecord, clientLabel: (clientId: string) => string): string {
  return sanitizeFileName(
    `${clientLabel(transfer.client_id)}-${shortId(transfer.session_id)}-${downloadFileName(transfer.path)}`,
    "vpsman-transfer.bin",
  );
}

function sanitizeFileName(value: string, fallback: string): string {
  return value.replace(/[\\/\u0000-\u001f\u007f]+/g, "_").slice(0, 160) || fallback;
}

function saveBlob(blob: Blob, fileName: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
