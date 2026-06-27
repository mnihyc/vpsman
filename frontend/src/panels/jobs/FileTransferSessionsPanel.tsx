import { AlertTriangle, Database, Download, ExternalLink, FileArchive, RefreshCw, RotateCcw, Upload, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { ArtifactDownloadMode } from "../../artifactDownload";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  artifactLifecycleStatusBadgeClass,
  fileTransferSessionStatusBadgeClass,
} from "../../jobStatusPresentation";
import type { JobDispatchPresetInput } from "../../jobDispatchPreset";
import type { AgentView } from "../../types";
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

type TransferRetryReviewSnapshot = {
  chunkEvidence: string;
  chunkSizeBytes: number;
  clientId: string;
  clientLabel: string;
  direction: string;
  failureReason: string;
  integrityPolicy: string;
  key: string;
  lastEvent: string;
  lastJobId: string;
  mode: "file_transfer_download" | "file_transfer_upload";
  path: string;
  progress: string;
  rateLimit: string;
  rateLimitKbps: number;
  retryGuidance: string;
  sessionId: string;
  status: string;
};

export function FileTransferSessionsPanel({
  agents,
  clientLabel,
  focusPath,
  transfers,
  sources,
  loading,
  onCreateHandoff,
  onDownloadSource,
  onOpenDispatchPreset,
  onOpenJobDetails,
  onRefresh,
  onSaveHandoff,
  onUploadSource,
}: {
  agents: AgentView[];
  clientLabel: (clientId: string) => string;
  focusPath?: string | null;
  transfers: FileTransferSessionRecord[];
  sources: FileTransferSourceArtifactRecord[];
  loading: boolean;
  onCreateHandoff: (clientId: string, sessionId: string) => Promise<FileTransferHandoffRecord>;
  onDownloadSource: (downloadPath: string) => Promise<Blob>;
  onOpenDispatchPreset?: (preset: JobDispatchPresetInput) => void;
  onOpenJobDetails?: (jobId: string) => void;
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
  const [retrySnapshot, setRetrySnapshot] = useState<TransferRetryReviewSnapshot | null>(null);
  const [selectedHandoffKeys, setSelectedHandoffKeys] = useState<string[]>([]);
  const [quickUploadTargetClientId, setQuickUploadTargetClientId] = useState(agents[0]?.id ?? "");
  const [quickUploadFile, setQuickUploadFile] = useState<File | null>(null);
  const [quickUploadPath, setQuickUploadPath] = useState("");
  const [quickUploadError, setQuickUploadError] = useState<string | null>(null);
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
  const uploadTransfers = transfers.filter((transfer) => transfer.direction === "upload");
  const downloadTransfers = transfers.filter((transfer) => transfer.direction === "download");
  const failedTransfers = transfers.filter(canReviewRetry);
  const completedDownloads = transfers.filter(
    (transfer) => transfer.direction === "download" && transfer.status === "completed",
  );
  const focusedTransfers = focusPath
    ? transfers.filter((transfer) => transfer.path === focusPath)
    : [];
  const focusedHandoffReady = focusedTransfers.filter(canCreateHandoff).length;
  const unavailableCompletedDownloads = Math.max(0, completedDownloads.length - handoffCandidates.length);
  const selectedHandoffKeySet = new Set(selectedHandoffKeys);
  const selectedHandoffTransfers = handoffCandidates.filter((transfer) => selectedHandoffKeySet.has(transferKey(transfer)));
  const handoffBusy = handoffPendingKey !== null;
  const handoffSummary =
    handoffError ??
    handoffProgress ??
    `${downloadTransfers.length} downloads, ${uploadTransfers.length} uploads tracked`;
  const sourceColumns: ConsoleDataGridColumn<FileTransferSourceArtifactRecord>[] = [
    {
      cell: (source) => (
        <span className="historyPrimary">
          <strong>{source.name}</strong>
          <small>SHA-256 {shortHash(source.sha256_hex)}</small>
        </span>
      ),
      header: "Source",
      id: "source",
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
          <small>Created {formatTime(source.created_at)}</small>
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
          aria-label={`Download reusable source ${source.name}`}
          className="sourceArtifactDownload secondaryAction compactAction"
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
              : "Download reusable upload source"
          }
          type="button"
        >
          <Download size={14} />
          <span>Download</span>
        </button>
      ),
      enableHiding: false,
      header: "Action",
      id: "action",
    },
  ];
  const transferColumns: ConsoleDataGridColumn<FileTransferSessionRecord>[] = [
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <strong>{transferDirectionLabel(transfer)}</strong>
          <small>{shortId(transfer.session_id)}</small>
        </span>
      ),
      header: "Direction",
      id: "direction",
      searchValue: (transfer) => `${transfer.direction} ${transfer.session_id}`,
      sortValue: (transfer) => `${transfer.direction}:${transfer.session_id}`,
    },
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <strong>{clientLabel(transfer.client_id)}</strong>
          <small title={transfer.client_id}>{transfer.client_id}</small>
        </span>
      ),
      header: "VPS",
      id: "vps",
      searchValue: (transfer) => `${clientLabel(transfer.client_id)} ${transfer.client_id}`,
      sortValue: (transfer) => clientLabel(transfer.client_id),
    },
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <strong title={transfer.path}>{transfer.path}</strong>
          <small>{transferPathRoleLabel(transfer)} · {transferIntegrityLabel(transfer)}</small>
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
          <strong>{transfer.size_bytes ? formatBytes(transfer.size_bytes) : "Not reported"}</strong>
          <small>{formatChunkInfo(transfer)}</small>
        </span>
      ),
      header: "Size",
      id: "size",
      searchValue: (transfer) => `${transfer.size_bytes ?? ""} ${formatChunkInfo(transfer)}`,
      sortValue: (transfer) => transfer.size_bytes ?? 0,
    },
    {
      cell: (transfer) => {
        return (
          <span className="transferProgressCell">
            <span>{formatTransferProgress(transfer)}</span>
            <span className="transferProgressTrack">
              <span style={{ width: `${Math.round((transfer.progress_ratio ?? 0) * 100)}%` }} />
            </span>
            <small>{formatTransferRateLimit(transfer.rate_limit_kbps)}</small>
          </span>
        );
      },
      header: "Progress/speed",
      id: "progress_speed",
      searchValue: (transfer) => `${formatTransferProgress(transfer)} ${formatTransferRateLimit(transfer.rate_limit_kbps)}`,
      sortValue: (transfer) => transfer.progress_ratio ?? 0,
    },
    {
      cell: (transfer) => (
        <span className="historyPrimary">
          <span className={`status ${fileTransferSessionStatusBadgeClass(transfer.status)}`}>{transferStateLabel(transfer)}</span>
          <small title={transferStateDetail(transfer)}>{transferStateDetail(transfer)}</small>
        </span>
      ),
      header: "State",
      id: "state",
      searchValue: (transfer) => `${transfer.status} ${transfer.last_event} ${handoffEvidenceLabel(transfer)} ${transferStateLabel(transfer)}`,
      sortValue: (transfer) => `${transfer.status}:${transfer.observed_at}`,
    },
    {
      cell: (transfer) => {
        const key = transferKey(transfer);
        const selectable = canCreateHandoff(transfer);
        const canRetry = canReviewRetry(transfer);
        const evidenceLabel = handoffEvidenceLabel(transfer);
        const evidenceTitle = handoffEvidenceTitle(transfer);
        return (
          <span className="rowActions">
            {selectable ? (
              <>
                <label className="transferRowSelect">
                  <input
                    aria-label={`Select ready download session ${shortId(transfer.session_id)}`}
                    checked={selectedHandoffKeySet.has(key)}
                    disabled={handoffBusy}
                    onChange={(event) => toggleHandoffSelection(transfer, event.target.checked)}
                    onClick={(event) => event.stopPropagation()}
                    type="checkbox"
                  />
                  <span>Select</span>
                </label>
              <button
                aria-label={`Ready to download session ${shortId(transfer.session_id)}`}
                className="secondaryAction compactAction"
                disabled={handoffPendingKey === key || handoffPendingKey === "bulk"}
                onClick={(event) => {
                  event.stopPropagation();
                  reviewHandoff(transfer);
                }}
                title={handoffReadyTitle(transfer)}
                type="button"
              >
                <Download size={14} />
                <span>Download</span>
              </button>
              </>
            ) : null}
            {canRetry ? (
              <button
                aria-label={`Retry transfer session ${shortId(transfer.session_id)}`}
                className="secondaryAction compactAction"
                onClick={(event) => {
                  event.stopPropagation();
                  setRetrySnapshot(retryReviewSnapshot(transfer, clientLabel));
                }}
                title="Review retry metadata and reopen the resumable transfer composer."
                type="button"
              >
                <RotateCcw size={14} />
                <span>Retry</span>
              </button>
            ) : null}
            {onOpenJobDetails ? (
              <button
                aria-label={`Open transfer job ${shortId(transfer.last_job_id)}`}
                className="secondaryAction compactAction"
                onClick={(event) => {
                  event.stopPropagation();
                  onOpenJobDetails(transfer.last_job_id);
                }}
                title="Open the last job that updated this transfer session"
                type="button"
              >
                <ExternalLink size={14} />
                <span>Job</span>
              </button>
            ) : null}
            {!selectable && !canRetry ? (
              <small title={evidenceTitle}>{evidenceLabel}</small>
            ) : null}
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

  useEffect(() => {
    if (quickUploadTargetClientId || agents.length === 0) {
      return;
    }
    setQuickUploadTargetClientId(agents[0].id);
  }, [agents, quickUploadTargetClientId]);

  function startQuickUpload() {
    if (!quickUploadFile) {
      setQuickUploadError("Choose a local file before uploading");
      return;
    }
    if (!quickUploadTargetClientId) {
      setQuickUploadError("Choose a VPS before uploading");
      return;
    }
    if (!quickUploadPath.startsWith("/")) {
      setQuickUploadError("Destination path must be absolute");
      return;
    }
    if (!onOpenDispatchPreset) {
      setQuickUploadError("Upload dispatch is unavailable on this surface");
      return;
    }
    setQuickUploadError(null);
    onOpenDispatchPreset({
      filePushMode: "0644",
      filePushPath: quickUploadPath,
      fileTransferChunkSize: 65536,
      fileTransferExistingPolicy: "skip",
      fileTransferMultiTargetPolicy: "same-offset",
      fileTransferRateLimit: 0,
      fileTransferResumeToken: "",
      fileTransferSessionId: "",
      fileTransferUploadFile: quickUploadFile,
      fileTransferUploadSourceKind: "local-file",
      maxTimeoutSecs: 300,
      mode: "file_transfer_upload",
      selectorExpression: `id:${quickUploadTargetClientId}`,
    });
  }

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
      setHandoffProgress(`Downloaded ${handoffSnapshot.transfers.length} selected files`);
      setHandoffSnapshot(null);
    } catch (error) {
      setHandoffError(error instanceof Error ? error.message : "Ready download failed");
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
      setSourceError("Choose a reusable source file first");
      return;
    }
    if (sourceFile.size > MAX_SOURCE_ARTIFACT_BYTES) {
      setSourceError(`Reusable source must be ${formatBytes(MAX_SOURCE_ARTIFACT_BYTES)} or smaller`);
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
      setSourceError(error instanceof Error ? error.message : "Reusable source review failed");
    } finally {
      setSourcePending(false);
    }
  }

  async function uploadSourceArtifact() {
    if (!sourceSnapshot) {
      setSourceError("Review reusable source before upload");
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
      setSourceError(error instanceof Error ? error.message : "Reusable source upload failed");
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
      setSourceError(error instanceof Error ? error.message : "Reusable source download failed");
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
      <div className="transferLifecycleSummary" aria-label="File transfer lifecycle summary">
        <span>
          <strong>Upload flow</strong>
          <small>Local file, VPS, destination, upload</small>
        </span>
        <span>
          <strong>Ready downloads</strong>
          <small>{handoffCandidates.length} ready, {unavailableCompletedDownloads} unavailable</small>
        </span>
        <span>
          <strong>Transfers</strong>
          <small>{downloadTransfers.length} downloads, {uploadTransfers.length} uploads</small>
        </span>
        <span className={failedTransfers.length > 0 ? "attention" : undefined}>
          <strong>Retries</strong>
          <small>{failedTransfers.length} failed sessions need metadata review</small>
        </span>
      </div>
      {focusPath && (
        <div className="transferFocusBanner" aria-label="Focused transfer path">
          <span>
            <strong>Focused from Files</strong>
            <small title={focusPath}>{focusPath}</small>
          </span>
          <span>
            <strong>{focusedTransfers.length} matching sessions</strong>
            <small>{focusedHandoffReady} ready to download</small>
          </span>
        </div>
      )}
      <section className="transferQuickUpload" aria-label="Default upload flow">
        <div className="transferWorkflowHeader">
          <div>
            <h3>Upload file</h3>
            <span>{"Choose local file -> Choose VPS -> Destination path -> Upload"}</span>
          </div>
          <Upload size={18} />
        </div>
        <div className="transferQuickUploadControls">
          <label>
            <span>Local file</span>
            <input
              aria-label="Transfer upload local file"
              onChange={(event) => {
                setQuickUploadFile(event.target.files?.[0] ?? null);
                setQuickUploadError(null);
              }}
              type="file"
            />
          </label>
          <label>
            <span>VPS</span>
            <VpsCombobox
              agents={agents}
              ariaLabel="Transfer upload target VPS"
              disabled={agents.length === 0}
              onChange={(value) => {
                setQuickUploadTargetClientId(value);
                setQuickUploadError(null);
              }}
              placeholder="Search target VPS"
              value={quickUploadTargetClientId}
            />
          </label>
          <label>
            <span>Destination path</span>
            <input
              aria-label="Transfer upload destination path"
              onChange={(event) => {
                setQuickUploadPath(event.target.value);
                setQuickUploadError(null);
              }}
              placeholder="/tmp/upload.bin"
              value={quickUploadPath}
            />
          </label>
          <button
            className="primaryAction compactAction"
            disabled={loading || !onOpenDispatchPreset}
            onClick={startQuickUpload}
            type="button"
          >
            <Upload size={14} />
            <span>Upload</span>
          </button>
        </div>
        <div className="transferQuickUploadStatus">
          <span>
            {quickUploadFile
              ? `${quickUploadFile.name} · ${formatBytes(quickUploadFile.size)}`
              : "No local file selected"}
          </span>
          <span>
            {quickUploadError ??
              `Target ${quickUploadTargetClientId ? clientLabel(quickUploadTargetClientId) : "not selected"}`}
          </span>
        </div>
      </section>
      <div className="handoffBulkBar">
        <span className="historyPrimary">
          <strong>Ready downloads</strong>
          <small>
            {handoffCandidates.length} ready to download, {unavailableCompletedDownloads} unavailable,{" "}
            {selectedHandoffTransfers.length} selected
          </small>
        </span>
        <span className="handoffBulkActions">
          <label className="handoffModeControl">
            <span>Save method</span>
            <select
              aria-label="Ready download save method"
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
                ? "No completed downloads currently have retained download evidence."
                : "Select every completed download that is ready to save."
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
                ? "Select one or more ready downloads first."
                : "Review selected downloads before saving."
            }
            type="button"
          >
            <Download size={14} />
            <span>{handoffBusy && handoffPendingKey === "bulk" ? "Downloading" : "Review selected downloads"}</span>
          </button>
        </span>
      </div>
      <ConfirmationPrompt
        confirmLabel="Download selected files"
        detail="Saves the reviewed completed download sessions using the selected method."
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
        title="Confirm ready download"
      />
      {retrySnapshot && (
        <section className="transferRetryReview" aria-label="Transfer retry review">
          <div className="transferRetryReviewHeader">
            <span>
              <AlertTriangle size={17} />
              <strong>Failed transfer retry review</strong>
            </span>
            <button
              aria-label="Close transfer retry review"
              className="iconButton"
              onClick={() => setRetrySnapshot(null)}
              title="Close retry review"
              type="button"
            >
              <X size={15} />
            </button>
          </div>
          <dl>
            <div>
              <dt>Target</dt>
              <dd title={retrySnapshot.clientId}>{retrySnapshot.clientLabel}</dd>
            </div>
            <div>
              <dt>Session</dt>
              <dd title={retrySnapshot.sessionId}>{shortId(retrySnapshot.sessionId)}</dd>
            </div>
            <div>
              <dt>Direction</dt>
              <dd>{retrySnapshot.direction}</dd>
            </div>
            <div>
              <dt>Status</dt>
              <dd>{retrySnapshot.status}</dd>
            </div>
            <div>
              <dt>Path</dt>
              <dd title={retrySnapshot.path}>{retrySnapshot.path}</dd>
            </div>
            <div>
              <dt>Progress</dt>
              <dd>{retrySnapshot.progress}</dd>
            </div>
            <div>
              <dt>Rate limit</dt>
              <dd>{retrySnapshot.rateLimit}</dd>
            </div>
            <div>
              <dt>Security</dt>
              <dd>{retrySnapshot.integrityPolicy}</dd>
            </div>
            <div>
              <dt>Chunk evidence</dt>
              <dd>{retrySnapshot.chunkEvidence}</dd>
            </div>
            <div>
              <dt>Failure reason</dt>
              <dd>{retrySnapshot.failureReason}</dd>
            </div>
            <div>
              <dt>Last event</dt>
              <dd>{retrySnapshot.lastEvent}</dd>
            </div>
            <div>
              <dt>Last job</dt>
              <dd title={retrySnapshot.lastJobId}>{shortId(retrySnapshot.lastJobId)}</dd>
            </div>
          </dl>
          <div className="transferRetryReviewActions">
            <span>{retrySnapshot.retryGuidance}</span>
            <button
              className="secondaryAction compactAction"
              disabled={!onOpenDispatchPreset}
              onClick={() => {
                onOpenDispatchPreset?.(retryDispatchPreset(retrySnapshot, "continue"));
                setRetrySnapshot(null);
              }}
              title={
                onOpenDispatchPreset
                  ? "Open Jobs / Dispatch with this session ID. Enter the original resume token before reviewing."
                  : "Retry dispatch is unavailable on this surface."
              }
              type="button"
            >
              <RotateCcw size={14} />
              <span>Continue in Dispatch</span>
            </button>
            <button
              className="primaryAction compactAction"
              disabled={!onOpenDispatchPreset}
              onClick={() => {
                onOpenDispatchPreset?.(retryDispatchPreset(retrySnapshot, "fresh"));
                setRetrySnapshot(null);
              }}
              title={
                onOpenDispatchPreset
                  ? "Open Jobs / Dispatch with the same target and path, but start a new transfer session."
                  : "Retry dispatch is unavailable on this surface."
              }
              type="button"
            >
              <Upload size={14} />
              <span>Start fresh in Dispatch</span>
            </button>
          </div>
        </section>
      )}
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
            <span>Direction</span>
            <strong>{transferDirectionLabel(transfer)}</strong>
            <span>VPS</span>
            <strong>{clientLabel(transfer.client_id)}</strong>
            <span>Path</span>
            <strong>{transfer.path}</strong>
            <span>Size</span>
            <strong>{transfer.size_bytes ? formatBytes(transfer.size_bytes) : "Not reported"}</strong>
            <span>SHA-256</span>
            <strong>{transfer.sha256_hex ?? "Not reported"}</strong>
            <span>Progress</span>
            <strong>{formatTransferProgress(transfer)}</strong>
            <span>Rate limit</span>
            <strong>{formatTransferRateLimit(transfer.rate_limit_kbps)}</strong>
            <span>Resume state</span>
            <strong>{transferResumeLabel(transfer)}</strong>
            <span>Security policy</span>
            <strong>{transferSecurityPolicyLabel(transfer)}</strong>
            <span>Retention expiry</span>
            <strong>Not reported by transfer API</strong>
            <span>Download evidence</span>
            <strong>{handoffEvidenceTitle(transfer)}</strong>
            <span>Failure reason</span>
            <strong>{transferFailureReason(transfer)}</strong>
            <span>Retry eligibility</span>
            <strong>{transferRetryEligibility(transfer)}</strong>
            <span>Last event</span>
            <strong>{transfer.last_event}</strong>
            <span>Last job</span>
            <strong>{transfer.last_job_id}</strong>
            <span>Last sequence</span>
            <strong>{transfer.last_seq}</strong>
            <span>Retained object</span>
            <strong title={transfer.handoff_object_key ?? undefined}>{transfer.handoff_object_key ?? "Not available"}</strong>
          </div>
        )}
        rows={transfers}
        searchPlaceholder="Search transfers"
        selectable={false}
        storageKey="vpsman.jobs.fileTransferSessions"
        title="Transfer sessions"
      />
      <details className="sourceArtifactAdvanced">
        <summary>
          <span>
            <strong>Advanced: reusable upload sources</strong>
            <small>{sourceError ?? `${sources.length} reusable sources`}</small>
          </span>
          <Database size={16} />
        </summary>
        <div className="sourceArtifactPanel">
          <div className="sectionSubheader">
            <div>
              <h3>Reusable upload sources</h3>
              <span>Optional object-store sources for repeated uploads.</span>
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
              <span>Reusable source name</span>
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
              <span>{sourcePending ? "Reviewing" : "Review reusable source"}</span>
            </button>
          </div>
          <ConfirmationPrompt
            confirmLabel="Upload reusable source"
            detail="Persists the reviewed reusable upload source with computed SHA-256 and size."
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
            title="Confirm reusable source upload"
          />
          <ConsoleDataGrid
            columns={sourceColumns}
            defaultPageSize={6}
            expandOnRowClick
            getRowId={(source) => source.id}
            itemLabel="sources"
            empty={
              <div className="sourceArtifactEmpty">
                <Database size={18} />
                <span>No reusable upload sources</span>
              </div>
            }
            renderExpandedRow={(source) => (
              <div className="consoleInlineDetailGrid">
                <span>Source ID</span>
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
                <span>Created by</span>
                <strong>{source.created_by ?? "System"}</strong>
                <span>Object key</span>
                <strong title={source.object_key}>{source.object_key}</strong>
                <span>Download path</span>
                <strong title={source.download_path}>{source.download_path}</strong>
                <span>Security policy</span>
                <strong>SHA-256 is computed before source persistence</strong>
              </div>
            )}
            rows={sources}
            searchPlaceholder="Search reusable sources"
            selectable={false}
            storageKey="vpsman.jobs.fileTransferSources"
            title="Reusable upload sources"
          />
        </div>
      </details>
    </div>
  );
}

function transferKey(transfer: FileTransferSessionRecord): string {
  return `${transfer.client_id}:${transfer.session_id}`;
}

function canCreateHandoff(transfer: FileTransferSessionRecord): boolean {
  return transfer.direction === "download" && transfer.status === "completed" && transfer.handoff_available;
}

function canReviewRetry(transfer: FileTransferSessionRecord): boolean {
  return transfer.status === "aborted" || transfer.status === "unknown";
}

function transferStateLabel(transfer: FileTransferSessionRecord): string {
  if (canCreateHandoff(transfer)) {
    return "Ready to download";
  }
  if (canReviewRetry(transfer)) {
    return "Retry";
  }
  if (transfer.status === "completed") {
    return "Completed";
  }
  if (transfer.status === "transferring") {
    return "In progress";
  }
  return transfer.status.replace(/_/g, " ");
}

function transferStateDetail(transfer: FileTransferSessionRecord): string {
  if (canCreateHandoff(transfer)) {
    return handoffEvidenceTitle(transfer);
  }
  if (canReviewRetry(transfer)) {
    return transferFailureReason(transfer);
  }
  if (transfer.status === "completed") {
    return transfer.direction === "download"
      ? handoffEvidenceTitle(transfer)
      : transferResumeLabel(transfer);
  }
  return transfer.last_event || transferResumeLabel(transfer);
}

function retryReviewSnapshot(
  transfer: FileTransferSessionRecord,
  clientLabel: (clientId: string) => string,
): TransferRetryReviewSnapshot {
  const mode = transfer.direction === "upload" ? "file_transfer_upload" : "file_transfer_download";
  return {
    chunkEvidence: formatChunkInfo(transfer),
    chunkSizeBytes: transfer.chunk_size_bytes ?? 65536,
    clientId: transfer.client_id,
    clientLabel: clientLabel(transfer.client_id),
    direction: transferDirectionLabel(transfer),
    failureReason: transferFailureReason(transfer),
    integrityPolicy: transferSecurityPolicyLabel(transfer),
    key: transferKey(transfer),
    lastEvent: transfer.last_event,
    lastJobId: transfer.last_job_id,
    mode,
    path: transfer.path,
    progress: formatTransferProgress(transfer),
    rateLimit: formatTransferRateLimit(transfer.rate_limit_kbps),
    rateLimitKbps: transfer.rate_limit_kbps ?? 0,
    retryGuidance:
      mode === "file_transfer_download"
        ? "Continue requires the original resume token for this session. Start fresh uses the same target, path, chunk size, and rate cap with a new session."
        : "Continue requires the original resume token and the same source payload. Start fresh uses the same target, destination path, chunk size, and rate cap with a new session.",
    sessionId: transfer.session_id,
    status: transfer.status,
  };
}

function retryDispatchPreset(
  retry: TransferRetryReviewSnapshot,
  intent: "continue" | "fresh",
): JobDispatchPresetInput {
  const base = {
    fileTransferChunkSize: retry.chunkSizeBytes,
    fileTransferRateLimit: retry.rateLimitKbps,
    fileTransferResumeToken: "",
    fileTransferSessionId: intent === "continue" ? retry.sessionId : "",
    maxTimeoutSecs: 300,
    selectorExpression: `id:${retry.clientId}`,
  };
  if (retry.mode === "file_transfer_upload") {
    return {
      ...base,
      filePushMode: "0644",
      filePushPath: retry.path,
      fileTransferExistingPolicy: "skip",
      fileTransferMultiTargetPolicy: "same-offset",
      fileTransferUploadSourceKind: "local-file",
      mode: "file_transfer_upload",
    };
  }
  return {
    ...base,
    fileFollowSymlinks: false,
    filePath: retry.path,
    fileTransferDownloadName: downloadFileName(retry.path),
    fileTransferDownloadSink: "browser-download",
    mode: "file_transfer_download",
  };
}

function transferRetryEligibility(transfer: FileTransferSessionRecord): string {
  if (canReviewRetry(transfer)) {
    return "Review metadata and reopen the resumable transfer composer";
  }
  if (transfer.status === "completed") {
    return "Completed transfer does not need retry";
  }
  return "Retry waits for a failed or unknown terminal state";
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
    return "Review download from the retained server file.";
  }
  return "Review download rebuilt from retained chunk outputs.";
}

function handoffEvidenceLabel(transfer: FileTransferSessionRecord): string {
  return handoffEvidenceStatusLabel(transfer.handoff_evidence_status);
}

function handoffEvidenceStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    artifact_available: "Ready to download",
    retained_outputs_available: "Ready from retained output",
    retained_outputs_pruned: "Evidence pruned",
    retained_outputs_incomplete: "Incomplete evidence",
    retained_outputs_conflict: "Conflicting chunks",
    missing_final_metadata: "Missing metadata",
    not_completed: "Not completed",
    not_applicable: "Upload complete",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function handoffEvidenceTitle(transfer: FileTransferSessionRecord): string {
  const reason = transfer.handoff_unavailable_reason
    ? ` Reason: ${transfer.handoff_unavailable_reason.replace(/_/g, " ")}.`
    : "";
  switch (transfer.handoff_evidence_status) {
    case "artifact_available":
      return "A retained server file exists for this completed download.";
    case "retained_outputs_available":
      return "Retained chunk output evidence is complete and can rebuild the download.";
    case "retained_outputs_pruned":
      return `The completed download remains visible, but the retained chunk outputs needed for a new download were pruned.${reason}`;
    case "retained_outputs_incomplete":
      return `The completed download remains visible, but retained chunk output evidence is incomplete.${reason}`;
    case "retained_outputs_conflict":
      return `The completed download remains visible, but duplicate chunk metadata conflicts and ready download is disabled.${reason}`;
    case "missing_final_metadata":
      return `The completed download is missing final size or SHA-256 metadata required for verified download.${reason}`;
    case "not_completed":
      return "Download is available after the session completes.";
    case "not_applicable":
      return "Upload sessions do not create ready-download files.";
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

function transferDirectionLabel(transfer: FileTransferSessionRecord): string {
  return transfer.direction === "upload" ? "Upload to VPS" : "Download from VPS";
}

function transferPathRoleLabel(transfer: FileTransferSessionRecord): string {
  return transfer.direction === "upload" ? "Destination path" : "Source path";
}

function transferIntegrityLabel(transfer: FileTransferSessionRecord): string {
  return transfer.sha256_hex ? `SHA-256 ${shortHash(transfer.sha256_hex)}` : "Checksum not reported";
}

function transferResumeLabel(transfer: FileTransferSessionRecord): string {
  if (transfer.resumed === true) {
    return "Resumed session";
  }
  if (transfer.resumed === false) {
    return "Fresh session";
  }
  return "Resume state unknown";
}

function transferSecurityPolicyLabel(transfer: FileTransferSessionRecord): string {
  if (!transfer.sha256_hex) {
    return "Checksum not reported by session";
  }
  return transfer.direction === "download"
    ? "SHA-256 is checked before ready download"
    : "SHA-256 is recorded for upload integrity";
}

function transferFailureReason(transfer: FileTransferSessionRecord): string {
  if (transfer.status === "aborted") {
    return transfer.handoff_unavailable_reason
      ? transfer.handoff_unavailable_reason.replace(/_/g, " ")
      : "Session aborted";
  }
  if (transfer.status === "unknown") {
    return transfer.handoff_unavailable_reason
      ? transfer.handoff_unavailable_reason.replace(/_/g, " ")
      : "Last state unknown";
  }
  if (transfer.direction === "download" && transfer.handoff_unavailable_reason) {
    return transfer.handoff_unavailable_reason.replace(/_/g, " ");
  }
  return "No failure reported";
}

function formatTransferRateLimit(kbps: number | null): string {
  if (!kbps || kbps <= 0) {
    return "No transfer cap";
  }
  if (kbps >= 1000) {
    return `${formatRateNumber(kbps / 1000)} Mbps cap`;
  }
  return `${kbps.toLocaleString()} Kbps cap`;
}

function formatRateNumber(value: number): string {
  return value.toLocaleString(undefined, {
    maximumFractionDigits: value < 10 ? 1 : 0,
  });
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
