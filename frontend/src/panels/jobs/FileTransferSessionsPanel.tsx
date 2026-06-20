import { Database, Download, FileArchive, RefreshCw, Upload } from "lucide-react";
import { useEffect, useState } from "react";
import type { ArtifactDownloadMode } from "../../artifactDownload";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { CrudPager } from "../../components/CrudPager";
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
  const selectedHandoffKeySet = new Set(selectedHandoffKeys);
  const selectedHandoffTransfers = handoffCandidates.filter((transfer) => selectedHandoffKeySet.has(transferKey(transfer)));
  const handoffBusy = handoffPendingKey !== null;
  const handoffSummary = handoffError ?? handoffProgress ?? `${transfers.length} resumable upload/download states`;

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
        <CrudPager
          fields={[
            { label: "Name", value: (source) => source.name },
            { label: "Hash", value: (source) => source.sha256_hex },
            { label: "Status", value: (source) => source.status },
            { label: "Size", value: (source) => source.size_bytes },
            { label: "Created", value: (source) => source.created_at },
          ]}
          itemLabel="artifacts"
          items={sources}
          pageSize={6}
          title="Source artifacts"
          empty={
            <div className="sourceArtifactEmpty">
              <Database size={18} />
              <span>No source artifacts</span>
            </div>
          }
        >
          {(sourceRows) => (
            <div className="sourceArtifactList">
              {sourceRows.map((source) => (
                <div className="sourceArtifactRow" key={source.id}>
                  <span className="historyPrimary">
                    <strong>{source.name}</strong>
                    <small>{shortHash(source.sha256_hex)}</small>
                  </span>
                  <span
                    className={`sourceArtifactStatus status ${artifactLifecycleStatusBadgeClass(source.status)}`}
                    title={artifactLifecycleStatusTitle(source.status)}
                  >
                    {source.status}
                  </span>
                  <span className="sourceArtifactMeta historyPrimary">
                    <strong>{formatBytes(source.size_bytes)}</strong>
                    <small>{formatTime(source.created_at)}</small>
                  </span>
                  <button
                    aria-label={`Download source artifact ${source.name}`}
                    className="sourceArtifactDownload iconButton"
                    disabled={
                      sourcePendingId === source.id ||
                      source.status === "creating" ||
                      source.status === "deleting"
                    }
                    onClick={() => void downloadSourceArtifact(source)}
                    title={
                      source.status === "creating" || source.status === "deleting"
                        ? artifactLifecycleStatusTitle(source.status)
                        : "Download source artifact"
                    }
                    type="button"
                  >
                    <Download size={14} />
                  </button>
                </div>
              ))}
            </div>
          )}
        </CrudPager>
      </div>
      <div className="handoffBulkBar">
        <span className="historyPrimary">
          <strong>Download handoffs</strong>
          <small>
            {handoffCandidates.length} completed downloads available, {selectedHandoffTransfers.length} selected
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
        ]}
        onCancel={() => setHandoffSnapshot(null)}
        onConfirm={() => void createAndDownloadReviewedHandoffs()}
        open={handoffSnapshot !== null}
        pending={handoffBusy}
        title="Confirm transfer handoff download"
      />
      <CrudPager
        fields={[
          { label: "VPS", value: (transfer) => clientLabel(transfer.client_id) },
          { label: "Session", value: (transfer) => transfer.session_id },
          { label: "Direction", value: (transfer) => transfer.direction },
          { label: "Status", value: (transfer) => `${transfer.status} ${transfer.last_event}` },
          { label: "Path", value: (transfer) => transfer.path },
          { label: "Hash", value: (transfer) => transfer.sha256_hex },
        ]}
        itemLabel="transfers"
        items={transfers}
        pageSize={8}
        title="Transfer records"
        empty={
          <div className="emptyState">
            <FileArchive size={22} />
            <strong>No file transfer sessions</strong>
            <span>Resumable upload and download status events populate this inventory.</span>
          </div>
        }
      >
        {(transferRows) => (
          <div className="table historyTable">
            <div className="historyRow heading fileTransferGrid">
              <span>Select</span>
              <span>Session</span>
              <span>Status</span>
              <span>Progress</span>
              <span>Path</span>
              <span>Rate</span>
              <span>Observed</span>
              <span>Action</span>
            </div>
            {transferRows.map((transfer) => {
              const key = transferKey(transfer);
              const selectable = canCreateHandoff(transfer);
              return (
                <div className="historyRow fileTransferGrid" key={key}>
                  <span className="rowSelectCell">
                    {selectable ? (
                      <input
                        aria-label={`Select transfer handoff session ${shortId(transfer.session_id)}`}
                        checked={selectedHandoffKeySet.has(key)}
                        disabled={handoffBusy}
                        onChange={(event) => toggleHandoffSelection(transfer, event.target.checked)}
                        type="checkbox"
                      />
                    ) : (
                      <small title="Handoff is available only after a completed inbound transfer">No handoff</small>
                    )}
                  </span>
                  <span className="historyPrimary">
                    <strong>{transfer.direction}</strong>
                    <small>{clientLabel(transfer.client_id)} / {shortId(transfer.session_id)}</small>
                  </span>
                  <span className="historyPrimary">
                    <span className={`status ${fileTransferSessionStatusBadgeClass(transfer.status)}`}>{transfer.status}</span>
                    <small>{transfer.resumed ? "resumed" : transfer.last_event}</small>
                  </span>
                  <span className="transferProgressCell">
                    <span>{formatTransferProgress(transfer)}</span>
                    <span className="transferProgressTrack">
                      <span style={{ width: `${Math.round((transfer.progress_ratio ?? 0) * 100)}%` }} />
                    </span>
                  </span>
                  <span className="historyPrimary">
                    <strong title={transfer.path}>{transfer.path}</strong>
                    <small>{transfer.sha256_hex ? shortHash(transfer.sha256_hex) : transfer.last_command_type}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{transfer.rate_limit_kbps ? `${transfer.rate_limit_kbps} kbps` : "unlimited"}</strong>
                    <small>{formatChunkInfo(transfer)}</small>
                  </span>
                  <span>{formatTime(transfer.observed_at)}</span>
                  <span className="rowActions">
                    {selectable ? (
                      <button
                        aria-label={`Create transfer handoff session ${shortId(transfer.session_id)}`}
                        className="iconButton"
                        disabled={handoffPendingKey === key || handoffPendingKey === "bulk"}
                        onClick={() => reviewHandoff(transfer)}
                        title="Review server-side transfer handoff download"
                        type="button"
                      >
                        <Download size={14} />
                      </button>
                    ) : (
                      <small>{transfer.handoff_object_key ? "stored" : "no handoff"}</small>
                    )}
                  </span>
                </div>
              );
            })}
          </div>
        )}
      </CrudPager>
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
