import { Download, RefreshCw, ShieldCheck, Upload } from "lucide-react";
import { useMemo, useState } from "react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import { SearchExpressionInput } from "../../components/SearchExpressionInput";
import { useReviewGenerationGuard, waitForReviewRender } from "../../hooks/useReviewGenerationGuard";
import {
  buildBulkJobProgress,
  bulkProgressTimeoutMs,
  createJobTargetCount,
  targetPreflightUnavailable,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../../bulkJobProgress";
import {
  FILE_BROWSER_ARCHIVE_LIMIT_BYTES,
  buildWriteTextOperation,
  buildUploadOperation,
  decodedText,
  fileBrowserOperationLabel,
  mutatesFileSystem,
  normalizeAbsolutePath,
  parseLatestFileStatus,
  type FileDownloadManifestEntry,
  type FileOperationStatus,
} from "../../fileBrowser";
import { parseFileMode } from "../../fileTransfer";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../../privilege";
import { agentsMatchingExpression } from "../../searchExpression";
import {
  isJobTargetStatus,
  jobTargetStatusBadgeClass,
} from "../../jobStatusPresentation";
import type {
  AgentView,
  BulkResolveResponse,
  CreateJobRequest,
  CreateJobResponse,
  FileActionPolicy,
  FileExistingPolicy,
  FileOwnershipPolicy,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetSelection,
} from "../../types";
import { runPanelAction, shortId, statusClass } from "../../utils";

const SELECTOR_STORAGE_KEY = "vpsman.multiFile.selectorExpression";
const BULK_JOB_TIMEOUT_SECS = 60;
const BULK_OUTPUT_SUMMARY_POLL_INTERVAL_MS = 500;
const BULK_OUTPUT_SUMMARY_WAIT_MS = 5_000;

type MultiFileAction = "download_files" | "upload_file" | "copy" | "rename" | "delete" | "chmod" | "chown" | "mkdir" | "write_text";

type PendingBulkConfirmation = {
  jobId: string;
  operation: JobOperation;
  selectorExpression: string;
  targets: AgentView[];
};

export function MultiFileActionsPanel({
  agents,
  initialPath,
  loading,
  onCreateJob,
  onDownloadFileBundle,
  onLoadOutputs,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onResolveTargets,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  initialPath: string;
  loading: boolean;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadFileBundle: (jobId: string, clientIds: string[]) => Promise<Blob>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onResolveTargets: (selection: JobTargetSelection) => Promise<BulkResolveResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (value: PrivilegeMaterial | null) => void;
}) {
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const [selectorExpression, setSelectorExpression] = useState(() => localStorage.getItem(SELECTOR_STORAGE_KEY) ?? "id:*");
  const [path, setPath] = useState(initialPath || "/");
  const [newPath, setNewPath] = useState("");
  const [mode, setMode] = useState("0644");
  const [content, setContent] = useState("");
  const [recursive, setRecursive] = useState(false);
  const [overwrite, setOverwrite] = useState(false);
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const [uploadMode, setUploadMode] = useState("0644");
  const [uploadExistingPolicy, setUploadExistingPolicy] = useState<FileExistingPolicy>("skip");
  const [uploadOwnershipPolicy, setUploadOwnershipPolicy] = useState<FileOwnershipPolicy>("fail");
  const [uploadOwner, setUploadOwner] = useState("");
  const [uploadGroup, setUploadGroup] = useState("");
  const [policy, setPolicy] = useState<FileActionPolicy>("fail");
  const [action, setAction] = useState<MultiFileAction>("download_files");
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [pending, setPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingBulkConfirmation | null>(null);
  const [lastSummary, setLastSummary] = useState<BulkSummaryGroup[]>([]);
  const [lastOutputs, setLastOutputs] = useState<JobOutputRecord[]>([]);
  const [lastOperation, setLastOperation] = useState<JobOperation | null>(null);
  const [lastJobId, setLastJobId] = useState<string | null>(null);
  const [bulkProgress, setBulkProgress] = useState<BulkJobProgress | null>(null);
  const [lastRunProgress, setLastRunProgress] = useState<BulkJobProgress | null>(null);
  const localMatches = useMemo(() => agentsMatchingExpression(agents, selectorExpression), [agents, selectorExpression]);
  const agentById = useMemo(() => new Map(agents.map((agent) => [agent.id, agent])), [agents]);
  const downloadComparison = useMemo(
    () => (lastOperation?.type === "file_download" ? buildDownloadComparison(lastOutputs) : null),
    [lastOperation, lastOutputs],
  );
  const summary = actionError ?? reviewStatus ?? actionMessage ?? `${localMatches.length}/${agents.length} local matches`;

  function clearExecutionResults() {
    setBulkProgress(null);
    setLastRunProgress(null);
    setLastSummary([]);
    setLastOutputs([]);
    setLastOperation(null);
    setLastJobId(null);
    setActionMessage(null);
  }

  function clearPendingConfirmation() {
    setPendingConfirmation(null);
  }

  function invalidateBulkReview() {
    invalidateReviewGeneration();
    clearPendingConfirmation();
  }

  async function refreshPreview() {
    const reviewGeneration = captureReviewGeneration();
    const reviewSelectorExpression = selectorExpression.trim();
    setReviewStatus("Resolving bulk file targets");
    try {
      await waitForReviewRender();
      await runPanelAction(setPending, setActionError, async () => {
      const next = await onResolveTargets({
        selector_expression: reviewSelectorExpression,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPreview(next);
      clearPendingConfirmation();
      localStorage.setItem(SELECTOR_STORAGE_KEY, reviewSelectorExpression);
      setActionMessage(`${next.target_count} VPSs resolved`);
    });
    } finally {
      setReviewStatus(null);
    }
  }

  async function prepareBulkOperation() {
    const reviewGeneration = captureReviewGeneration();
    const reviewSelectorExpression = selectorExpression.trim();
    setReviewStatus("Preparing bulk file review");
    try {
      await waitForReviewRender();
      await runPanelAction(setPending, setActionError, async () => {
      const resolved = await onResolveTargets({
        selector_expression: reviewSelectorExpression,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPreview(resolved);
      if (resolved.targets.length === 0) {
        throw new Error("No VPSs match the selector");
      }
      const operation = await buildOperation();
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingConfirmation({
        jobId: crypto.randomUUID(),
        operation,
        selectorExpression: reviewSelectorExpression,
        targets: resolved.targets,
      });
    });
    } finally {
      setReviewStatus(null);
    }
  }

  async function buildOperation(): Promise<JobOperation> {
    const normalizedPath = normalizeAbsolutePath(path);
    if (action === "download_files") {
      return {
        type: "file_download",
        path: normalizedPath,
        max_bytes: FILE_BROWSER_ARCHIVE_LIMIT_BYTES,
        follow_symlinks: false,
      };
    }
    if (action === "upload_file") {
      if (!uploadFile) {
        throw new Error("Choose a file to upload");
      }
      return await buildUploadOperation(uploadFile, uploadDestinationPath(path, uploadFile.name), uploadMode, {
        existingPolicy: uploadExistingPolicy,
        owner: uploadOwner.trim() || null,
        group: uploadGroup.trim() || null,
        ownershipPolicy: uploadOwnershipPolicy,
      });
    }
    if (action === "chown") {
      return {
        type: "file_chown",
        path: normalizedPath,
        owner: uploadOwner.trim() || null,
        group: uploadGroup.trim() || null,
        recursive,
        ownership_policy: uploadOwnershipPolicy,
        policy,
      };
    }
    if (action === "chmod") {
      return {
        type: "file_chmod",
        path: normalizedPath,
        mode: parseMode(mode),
        recursive,
        follow_symlinks: false,
        policy,
      };
    }
    if (action === "delete") {
      return { type: "file_delete", path: normalizedPath, recursive, policy };
    }
    if (action === "copy") {
      if (!newPath.trim()) {
        throw new Error("Enter a destination path");
      }
      return {
        type: "file_copy",
        path: normalizedPath,
        new_path: normalizeAbsolutePath(newPath),
        overwrite,
        recursive,
        follow_symlinks: false,
        policy,
      };
    }
    if (action === "mkdir") {
      return { type: "file_mkdir", path: normalizedPath, mode: parseMode(mode), recursive, policy };
    }
    if (action === "rename") {
      if (!newPath.trim()) {
        throw new Error("Enter a destination path");
      }
      return {
        type: "file_rename",
        path: normalizedPath,
        new_path: normalizeAbsolutePath(newPath),
        overwrite,
        policy,
      };
    }
    return await buildWriteTextOperation({
      content,
      create: true,
      mode,
      path: normalizedPath,
      policy,
    });
  }

  async function executeBulkOperation(confirmation: PendingBulkConfirmation) {
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      const targetIds = confirmation.targets.map((target) => target.id);
      const built = await buildPrivilegeForJobOperation({
        clientIds: targetIds,
        commandType: confirmation.operation.type,
        operation: confirmation.operation,
        privilegeMaterial,
        selectorExpression: confirmation.selectorExpression,
        timeoutSecs: BULK_JOB_TIMEOUT_SECS,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRunProgress(null);
      const job = await onCreateJob({
        selector_expression: confirmation.selectorExpression,
        target_client_ids: confirmation.targets.map((target) => target.id),
        destructive: mutatesFileSystem(confirmation.operation),
        confirmed: true,
        command: confirmation.operation.type,
        argv: [],
        job_id: confirmation.jobId,
        operation: confirmation.operation,
        timeout_secs: BULK_JOB_TIMEOUT_SECS,
        force_unprivileged: false,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
      });
      let outputs: JobOutputRecord[] = [];
      let targets: JobTargetRecord[] = [];
      const targetCount = createJobTargetCount(job);
      let finalProgress = buildBulkJobProgress({
        jobId: job.job_id,
        targetCount,
        outputs,
        targetRecords: targets,
        targets: confirmation.targets,
      });
      setBulkProgress(finalProgress);
      try {
        const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
          targetCount,
          targets: confirmation.targets,
          onProgress: (progress) => {
            finalProgress = progress;
            setBulkProgress(progress);
          },
          timeoutMs: bulkProgressTimeoutMs(BULK_JOB_TIMEOUT_SECS),
        });
        targets = result.targets;
        if (result.timedOut) {
          throw new Error("Timed out waiting for bulk file action targets");
        }
        outputs = await loadOutputsForSummary(job.job_id, onLoadOutputs, targets);
        finalProgress = buildBulkJobProgress({
          jobId: job.job_id,
          targetCount,
          outputs,
          targetRecords: targets,
          targets: confirmation.targets,
        });
      } finally {
        setBulkProgress(null);
      }
      setLastOutputs(outputs);
      setLastOperation(confirmation.operation);
      setLastJobId(job.job_id);
      setLastRunProgress(finalProgress);
      setLastSummary(groupBulkOutputs(outputs, confirmation.operation.type, confirmation.targets, targets, agentById));
      setActionMessage(`${fileBrowserOperationLabel(confirmation.operation)} job ${shortId(job.job_id)} finished`);
    });
  }

  const visibleProgress = bulkProgress ?? lastRunProgress;
  const executionSummary = visibleProgress
    ? `Job ${shortId(visibleProgress.jobId)}`
    : lastSummary.length > 0
      ? `${lastSummary.length} grouped outcomes`
      : "Run a bulk action to aggregate output";

  return (
    <div className="fleetPanel multiFilePanel">
      <div className="sectionHeader">
        <div>
          <h2>Multi files</h2>
          <span>{summary}</span>
        </div>
        <button className="secondaryAction" disabled={pending || loading} onClick={() => void refreshPreview()} type="button">
          <RefreshCw size={14} />
          <span>Review targets</span>
        </button>
      </div>

      {!privilegeMaterial && (
        <div className="fileBrowserPrivilegeRow">
          <PrivilegeVaultBox
            lastPayloadHash={lastPayloadHash}
            onOpenUnlock={onOpenPrivilegeUnlock}
            onPrivilegeMaterialChange={setPrivilegeMaterial}
            privilegeMaterial={privilegeMaterial}
          />
        </div>
      )}

      <div className="multiFileLayout">
        <section className="multiFileComposer">
          <label>
            <span>Targets</span>
            <SearchExpressionInput
              agents={agents}
              ariaLabel="Multi file target selector"
              onChange={(value) => {
                setSelectorExpression(value);
                setPreview(null);
                invalidateBulkReview();
                localStorage.setItem(SELECTOR_STORAGE_KEY, value);
              }}
              placeholder="id:* && provider:example"
              showMatchCount
              value={selectorExpression}
              verification={localMatches.length > 0 ? "valid" : "neutral"}
            />
          </label>
          <div className="multiFilePrimaryActions">
            <button className={action === "download_files" ? "primaryAction" : "secondaryAction"} onClick={() => {
              setAction("download_files");
              invalidateBulkReview();
            }} type="button">
              <Download size={14} />
              <span>Download files</span>
            </button>
            <button className={action === "upload_file" ? "primaryAction" : "secondaryAction"} onClick={() => {
              setAction("upload_file");
              invalidateBulkReview();
            }} type="button">
              <Upload size={14} />
              <span>Upload files</span>
            </button>
          </div>
          <div className="multiFileGrid">
            <label>
              <span>{action === "upload_file" ? "Destination path" : "Path"}</span>
              <input
                aria-label={action === "upload_file" ? "Bulk file destination path" : "Bulk file path"}
                onChange={(event) => {
                  setPath(event.target.value);
                  invalidateBulkReview();
                }}
                value={path}
              />
            </label>
          </div>
          {action === "download_files" && (
            <div className="operationNote compactNote">
              <strong>Download files</strong>
              <span>Folders download as tar per VPS; the summary action returns one tar bundle.</span>
            </div>
          )}
          {action === "upload_file" && (
            <div className="bulkTransferGrid">
              <label className="wideField">
                <span>Source file</span>
                <input
                  aria-label="Bulk upload file"
                  onChange={(event) => {
                    setUploadFile(event.target.files?.[0] ?? null);
                    invalidateBulkReview();
                  }}
                  type="file"
                />
              </label>
              <label>
                <span>Mode</span>
                <input
                onChange={(event) => {
                  setUploadMode(event.target.value);
                  invalidateBulkReview();
                }}
                  value={uploadMode}
                />
              </label>
              <label>
                <span>Existing file</span>
                <select
                  onChange={(event) => {
                    setUploadExistingPolicy(event.target.value as FileExistingPolicy);
                    invalidateBulkReview();
                  }}
                  value={uploadExistingPolicy}
                >
                  <option value="skip">Skip</option>
                  <option value="replace">Replace</option>
                </select>
              </label>
              <label>
                <span>Owner</span>
                <input
                  onChange={(event) => {
                    setUploadOwner(event.target.value);
                    invalidateBulkReview();
                  }}
                  value={uploadOwner}
                />
              </label>
              <label>
                <span>Group</span>
                <input
                  onChange={(event) => {
                    setUploadGroup(event.target.value);
                    invalidateBulkReview();
                  }}
                  value={uploadGroup}
                />
              </label>
              <label>
                <span>Missing owner/group</span>
                <select
                  onChange={(event) => {
                    setUploadOwnershipPolicy(event.target.value as FileOwnershipPolicy);
                    invalidateBulkReview();
                  }}
                  value={uploadOwnershipPolicy}
                >
                  <option value="fail">Fail</option>
                  <option value="ignore">Ignore chown</option>
                </select>
              </label>
            </div>
          )}
          <details className="advancedFileActions">
            <summary>Advanced actions</summary>
            <label>
              <span>Action</span>
              <select onChange={(event) => {
                setAction(event.target.value as MultiFileAction);
                invalidateBulkReview();
              }} value={isAdvancedAction(action) ? action : ""}>
                <option disabled value="">Choose action</option>
                <option value="copy">Copy</option>
                <option value="rename">Move</option>
                <option value="delete">Delete path</option>
                <option value="chmod">Chmod</option>
                <option value="chown">Chown</option>
                <option value="mkdir">Create folder</option>
                <option value="write_text">Write text</option>
              </select>
            </label>
          </details>
          {usesFileActionPolicy(action) && (
            <label>
              <span>Policy</span>
              <select
                onChange={(event) => {
                  setPolicy(event.target.value as FileActionPolicy);
                  invalidateBulkReview();
                }}
                value={policy}
              >
                <option value="fail">Strict: fail on conflict</option>
                <option value="ensure">Converge: accept already-correct state</option>
                <option value="ignore">Skip missing/conflicting targets</option>
              </select>
            </label>
          )}
          {(action === "copy" || action === "rename") && (
            <label>
              <span>{action === "copy" ? "Destination path" : "New path"}</span>
              <input
                onChange={(event) => {
                  setNewPath(event.target.value);
                  invalidateBulkReview();
                }}
                placeholder="/etc/app.conf.next"
                value={newPath}
              />
            </label>
          )}
          {(action === "chmod" || action === "mkdir" || action === "write_text") && (
            <label>
              <span>Mode</span>
              <input
                onChange={(event) => {
                  setMode(event.target.value);
                  invalidateBulkReview();
                }}
                value={mode}
              />
            </label>
          )}
          {action === "chown" && (
            <div className="bulkTransferGrid">
              <label>
                <span>Owner</span>
                <input
                  onChange={(event) => {
                    setUploadOwner(event.target.value);
                    invalidateBulkReview();
                  }}
                  value={uploadOwner}
                />
              </label>
              <label>
                <span>Group</span>
                <input
                  onChange={(event) => {
                    setUploadGroup(event.target.value);
                    invalidateBulkReview();
                  }}
                  value={uploadGroup}
                />
              </label>
              <label>
                <span>Missing owner/group</span>
                <select
                  onChange={(event) => {
                    setUploadOwnershipPolicy(event.target.value as FileOwnershipPolicy);
                    invalidateBulkReview();
                  }}
                  value={uploadOwnershipPolicy}
                >
                  <option value="fail">Fail</option>
                  <option value="ignore">Ignore chown</option>
                </select>
              </label>
            </div>
          )}
          {action === "write_text" && (
            <label>
              <span>Content</span>
              <textarea
                onChange={(event) => {
                  setContent(event.target.value);
                  invalidateBulkReview();
                }}
                rows={8}
                value={content}
              />
            </label>
          )}
          {(action === "copy" || action === "chmod" || action === "chown" || action === "delete" || action === "mkdir" || action === "rename") && (
            <div className="multiFileGrid">
              {(action === "copy" || action === "chmod" || action === "chown" || action === "delete" || action === "mkdir") && (
                <label className="inlineCheck actionCheck">
                  <input
                    checked={recursive}
                    onChange={(event) => {
                      setRecursive(event.target.checked);
                      invalidateBulkReview();
                    }}
                    type="checkbox"
                  />
                  <span>{action === "mkdir" ? "Create missing parents" : "Recursive"}</span>
                </label>
              )}
              {(action === "copy" || action === "rename") && (
                <label className="inlineCheck actionCheck">
                  <input
                    checked={overwrite}
                    onChange={(event) => {
                      setOverwrite(event.target.checked);
                      invalidateBulkReview();
                    }}
                    type="checkbox"
                  />
                  <span>Overwrite destination</span>
                </label>
              )}
            </div>
          )}
          <button className={runBulkActionClass(action)} disabled={pending || !privilegeMaterial || loading} onClick={() => void prepareBulkOperation()} type="button">
            <ShieldCheck size={14} />
            <span>{runBulkActionLabel(action)}</span>
          </button>
        </section>

        <section className="bulkSummaryPane">
          <div className="sectionSubheader">
            <div>
              <h3>Execution summary</h3>
              <span>{executionSummary}</span>
            </div>
            {lastOperation?.type === "file_download" && lastOutputs.length > 0 && (
              <button className="secondaryAction compactAction" onClick={() => void downloadBulkBundle(onDownloadFileBundle, lastJobId, successfulDownloadClientIds(lastSummary))} type="button">
                <Download size={14} />
                <span>Download Archive</span>
              </button>
            )}
          </div>
          {visibleProgress && (
            <ExecutionResultPanel
              loading={bulkProgress !== null}
              onClearResults={clearExecutionResults}
              onOpenJobDetails={onOpenJobDetails}
              progress={visibleProgress}
            />
          )}
          {downloadComparison && <BulkDownloadComparisonView agentById={agentById} comparison={downloadComparison} />}
          <div className="bulkSummaryList">
            {lastSummary.length === 0 ? (
              <p className="emptyState">No bulk file results yet.</p>
            ) : (
              lastSummary.map((group) => (
                <details key={group.key} open>
                  <summary>
                    <span className={`status ${bulkSummaryStatusClass(group)}`}>{group.status}</span>
                    <strong>{vpsCountLabel(group.clientIds.length)}</strong>
                    <span>{group.reason || group.label}</span>
                  </summary>
                  {group.detail && <p className="bulkSummaryDetail">{group.detail}</p>}
                  {(group.sha256Hex || group.preview) && (
                    <div className="bulkEvidenceBox">
                      {group.sha256Hex && (
                        <div>
                          <span>Same hash</span>
                          <strong title={group.sha256Hex}>{shortId(group.sha256Hex)}</strong>
                        </div>
                      )}
                      {group.preview && (
                        <div className="bulkEvidencePreview">
                          <span>Content preview</span>
                          <pre className="bulkSummaryPreview">{group.preview}</pre>
                        </div>
                      )}
                    </div>
                  )}
                  <div className="bulkSummaryClients">
                    {group.clientIds.map((clientId) => (
                      <span key={clientId} title={clientId}>
                        {clientDisplayName(clientId, agentById)}
                      </span>
                    ))}
                  </div>
                </details>
              ))
            )}
          </div>
          {preview && lastSummary.length === 0 && (
            <div className="targetImpactPreview">
              <div className="targetImpactHeader">
                <strong>Target preview</strong>
                <span>{vpsCountLabel(preview.target_count)}</span>
              </div>
              <div className="bulkSummaryClients">
                {preview.targets.slice(0, 40).map((target) => (
                  <span key={target.id} title={target.id}>
                    {targetDisplayName(target)}
                  </span>
                ))}
              </div>
            </div>
          )}
        </section>
      </div>

      <ConfirmationPrompt
        confirmLabel={pendingConfirmation?.operation.type === "file_delete" ? "Delete path" : "Run bulk action"}
        detail={
          pendingConfirmation
            ? compactConfirmationDetail(pendingConfirmation.operation, pendingConfirmation.targets.length)
            : ""
        }
        items={pendingConfirmation ? confirmationItems(pendingConfirmation) : []}
        onCancel={() => setPendingConfirmation(null)}
        onConfirm={() => {
          const confirmation = pendingConfirmation;
          setPendingConfirmation(null);
          if (confirmation) {
            void executeBulkOperation(confirmation);
          }
        }}
        open={pendingConfirmation !== null}
        pending={pending}
        title="Confirm multi-file operation"
        tone={pendingConfirmation?.operation.type === "file_delete" ? "danger" : "normal"}
      />
    </div>
  );
}

function BulkDownloadComparisonView({
  agentById,
  comparison,
}: {
  agentById: Map<string, AgentView>;
  comparison: BulkDownloadComparison;
}) {
  return (
    <div className={`bulkDownloadComparison ${comparison.tone}`}>
      <div className="bulkDownloadComparisonHeader">
        <strong>{comparison.title}</strong>
        <span>{comparison.detail}</span>
      </div>
      {comparison.rows.length > 0 && (
        <div className="bulkComparisonRows">
          {comparison.rows.map((row) => (
            <div className="bulkComparisonRow" key={row.label}>
              <div className="bulkComparisonRowTitle">
                <strong title={row.label}>{row.label}</strong>
                {row.detail && <span>{row.detail}</span>}
              </div>
              <div className="bulkComparisonVariants">
                {row.variants.map((variant) => (
                  <div className="bulkComparisonVariant" key={`${row.label}:${variant.label}`}>
                    <span className="bulkComparisonVariantLabel" title={variant.label}>
                      {variant.label}
                    </span>
                    <div className="bulkSummaryClients bulkComparisonClients">
                      {variant.clientIds.map((clientId) => (
                        <span key={clientId} title={clientId}>
                          {clientDisplayName(clientId, agentById)}
                        </span>
                      ))}
                    </div>
                  </div>
                ))}
              </div>
            </div>
          ))}
          {comparison.extraRowCount > 0 && <p className="bulkSummaryDetail">{comparison.extraRowCount} more differing paths not shown.</p>}
        </div>
      )}
    </div>
  );
}

type BulkDownloadComparison = {
  detail: string;
  extraRowCount: number;
  rows: BulkDownloadComparisonRow[];
  title: string;
  tone: "same" | "contentDiscrepancy" | "hierarchyDiscrepancy";
};

type BulkDownloadComparisonRow = {
  detail?: string;
  label: string;
  variants: BulkDownloadComparisonVariant[];
};

type BulkDownloadComparisonVariant = {
  clientIds: string[];
  label: string;
};

type DownloadStatusRecord = {
  clientId: string;
  status: FileOperationStatus;
};

type BulkSummaryGroup = {
  clientIds: string[];
  detail: string;
  key: string;
  label: string;
  preview: string;
  reason: string;
  sha256Hex: string;
  status: string;
  type: string;
};

function groupBulkOutputs(
  outputs: JobOutputRecord[],
  expectedType: string,
  targets: AgentView[],
  targetRecords: JobTargetRecord[],
  agentById: Map<string, AgentView>,
): BulkSummaryGroup[] {
  const groups = new Map<string, BulkSummaryGroup>();
  const clientsWithStatus = new Set<string>();
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    const status = parseLatestFileStatus([output], expectedType);
    if (!status) {
      continue;
    }
    clientsWithStatus.add(output.client_id);
    const state = status.status ?? "completed";
    const reason = status.reason ?? "";
    const preview = statusPreview(status);
    const detail = statusDetail(status);
    const label = statusLabel(status);
    const sha256Hex = status.sha256_hex ?? "";
    const key = `${status.type}:${state}:${reason}:${status.path}:${sha256Hex}:${status.size_bytes ?? ""}:${preview}`;
    const group = groups.get(key) ?? {
      clientIds: [],
      detail,
      key,
      label,
      preview,
      reason,
      sha256Hex,
      status: state,
      type: status.type,
    };
    group.clientIds.push(output.client_id);
    groups.set(key, group);
  }
  const targetRecordByClient = new Map(targetRecords.map((target) => [target.client_id, target]));
  for (const target of targets) {
    if (clientsWithStatus.has(target.id)) {
      continue;
    }
    const targetRecord = targetRecordByClient.get(target.id);
    const agent = agentById.get(target.id) ?? target;
    const unavailable = targetPreflightUnavailable(agent);
    const state = unavailable ? "unavailable" : targetRecord?.status ?? "queued";
    const reason = unavailable ? agent.status : targetRecord?.message ?? targetRecord?.status ?? "waiting_for_output";
    const label = unavailable ? "Agent unavailable" : targetRecord?.status ? "Job target status" : "No file status";
    const detail = unavailable
      ? `Matched by selector; agent status ${agent.status}.`
      : targetRecord?.status
        ? `Job target ${targetRecord.status}; ${targetRecord.message ?? "structured file status not retrieved"}.`
        : "Waiting for job target or file status.";
    const key = `target:${state}:${reason}`;
    const group = groups.get(key) ?? {
      clientIds: [],
      detail,
      key,
      label,
      preview: "",
      reason,
      sha256Hex: "",
      status: state,
      type: expectedType,
    };
    group.clientIds.push(target.id);
    groups.set(key, group);
  }
  return Array.from(groups.values()).sort((left, right) => right.clientIds.length - left.clientIds.length);
}

function bulkSummaryStatusClass(group: BulkSummaryGroup): string {
  if (group.label === "Job target status" && isJobTargetStatus(group.status)) {
    return jobTargetStatusBadgeClass(group.status);
  }
  return statusClass(group.status);
}

function successfulDownloadClientIds(groups: BulkSummaryGroup[]): string[] {
  return groups.filter((group) => group.type === "file_download" && group.status === "completed").flatMap((group) => group.clientIds);
}

function buildDownloadComparison(outputs: JobOutputRecord[]): BulkDownloadComparison | null {
  const records = downloadStatusRecords(outputs);
  if (records.length < 2) {
    return null;
  }
  const sourceKindVariants = groupedDownloadVariants(
    records,
    (record) => record.status.source_kind ?? "unknown",
    (record) => `source ${record.status.source_kind ?? "unknown"}`,
  );
  if (sourceKindVariants.length > 1) {
    return {
      detail: "Targets did not return the same source kind.",
      extraRowCount: 0,
      rows: [{ label: "Source kind", variants: sourceKindVariants }],
      title: "Hierarchy differs",
      tone: "hierarchyDiscrepancy",
    };
  }
  if (records[0]?.status.source_kind === "directory") {
    return buildDirectoryDownloadComparison(records);
  }
  return buildRegularFileDownloadComparison(records);
}

function downloadStatusRecords(outputs: JobOutputRecord[]): DownloadStatusRecord[] {
  const records: DownloadStatusRecord[] = [];
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    const status = parseLatestFileStatus([output], "file_download");
    if (status?.status && status.status !== "completed") {
      continue;
    }
    if (status) {
      records.push({ clientId: output.client_id, status });
    }
  }
  return records;
}

function buildRegularFileDownloadComparison(records: DownloadStatusRecord[]): BulkDownloadComparison {
  const variants = groupedDownloadVariants(records, fileContentKey, (record) => fileContentLabel(record.status));
  if (variants.length <= 1) {
    const status = records[0]?.status;
    return {
      detail: `${records.length} VPSs match${status ? ` · ${fileContentLabel(status)}` : ""}`,
      extraRowCount: 0,
      rows: [],
      title: "Same file content",
      tone: "same",
    };
  }
  return {
    detail: "Same path, different file bytes.",
    extraRowCount: 0,
    rows: [{ label: records[0]?.status.path ?? "Selected file", variants }],
    title: "File content differs",
    tone: "contentDiscrepancy",
  };
}

function buildDirectoryDownloadComparison(records: DownloadStatusRecord[]): BulkDownloadComparison {
  const hierarchyVariants = groupedDownloadVariants(records, directoryHierarchyKey, (record) => directoryHierarchyLabel(record.status));
  if (hierarchyVariants.length > 1 || hierarchyVariants.some((variant) => variant.label.includes("missing"))) {
    return {
      detail: "Directory tree is not consistent across targets; compare hierarchy before trusting content hashes.",
      extraRowCount: 0,
      rows: [{ label: records[0]?.status.path ?? "Selected directory", variants: hierarchyVariants }],
      title: "Hierarchy differs",
      tone: "hierarchyDiscrepancy",
    };
  }
  const contentVariants = groupedDownloadVariants(records, directoryContentKey, (record) => directoryContentLabel(record.status));
  if (contentVariants.length <= 1) {
    const status = records[0]?.status;
    return {
      detail: `${records.length} VPSs match · ${directoryCountsLabel(status)} · ${hashLabel("content", status?.content_manifest_sha256_hex ?? status?.sha256_hex)}`,
      extraRowCount: 0,
      rows: [],
      title: "Same hierarchy and content",
      tone: "same",
    };
  }
  const exact = exactDirectoryFileDifferenceRows(records);
  if (exact) {
    return {
      detail: "Hierarchy matches; differing files are listed by relative path.",
      extraRowCount: exact.extraRowCount,
      rows: exact.rows,
      title: "File content differs",
      tone: "contentDiscrepancy",
    };
  }
  return {
    detail: "Hierarchy matches, but the manifest is unavailable or truncated; content differs by manifest hash.",
    extraRowCount: 0,
    rows: [{ label: records[0]?.status.path ?? "Selected directory", variants: contentVariants }],
    title: "Content manifest differs",
    tone: "contentDiscrepancy",
  };
}

function exactDirectoryFileDifferenceRows(records: DownloadStatusRecord[]): { extraRowCount: number; rows: BulkDownloadComparisonRow[] } | null {
  if (records.some((record) => record.status.manifest_truncated || !Array.isArray(record.status.manifest_entries))) {
    return null;
  }
  const manifests = records.map((record) => ({
    clientId: record.clientId,
    entries: new Map((record.status.manifest_entries ?? []).map((entry) => [entry.path, entry])),
  }));
  const paths = Array.from(
    new Set(
      manifests.flatMap((manifest) =>
        Array.from(manifest.entries.values())
          .filter((entry) => entry.kind === "file")
          .map((entry) => entry.path),
      ),
    ),
  ).sort((left, right) => left.localeCompare(right));
  const rows: BulkDownloadComparisonRow[] = [];
  for (const path of paths) {
    const variants = new Map<string, BulkDownloadComparisonVariant>();
    for (const manifest of manifests) {
      const entry = manifest.entries.get(path);
      const key = manifestEntryContentKey(entry);
      const variant = variants.get(key) ?? {
        clientIds: [],
        label: manifestEntryContentLabel(entry),
      };
      variant.clientIds.push(manifest.clientId);
      variants.set(key, variant);
    }
    if (variants.size > 1) {
      rows.push({
        detail: `${variants.size} variants`,
        label: path,
        variants: Array.from(variants.values()).sort((left, right) => right.clientIds.length - left.clientIds.length),
      });
    }
  }
  const visibleRows = rows.slice(0, 8);
  return { extraRowCount: Math.max(0, rows.length - visibleRows.length), rows: visibleRows };
}

function groupedDownloadVariants(
  records: DownloadStatusRecord[],
  keyFor: (record: DownloadStatusRecord) => string,
  labelFor: (record: DownloadStatusRecord) => string,
): BulkDownloadComparisonVariant[] {
  const groups = new Map<string, BulkDownloadComparisonVariant>();
  for (const record of records) {
    const key = keyFor(record);
    const variant = groups.get(key) ?? { clientIds: [], label: labelFor(record) };
    variant.clientIds.push(record.clientId);
    groups.set(key, variant);
  }
  return Array.from(groups.values()).sort((left, right) => right.clientIds.length - left.clientIds.length);
}

function fileContentKey(record: DownloadStatusRecord): string {
  return `${record.status.size_bytes ?? "unknown"}:${record.status.sha256_hex ?? "missing"}`;
}

function fileContentLabel(status: FileOperationStatus): string {
  return `${typeof status.size_bytes === "number" ? formatBytes(status.size_bytes) : "unknown size"} · ${hashLabel("sha256", status.sha256_hex)}`;
}

function directoryHierarchyKey(record: DownloadStatusRecord): string {
  return record.status.hierarchy_sha256_hex ?? "missing";
}

function directoryHierarchyLabel(status: FileOperationStatus): string {
  return `${directoryCountsLabel(status)} · ${hashLabel("hierarchy", status.hierarchy_sha256_hex)}`;
}

function directoryContentKey(record: DownloadStatusRecord): string {
  return record.status.content_manifest_sha256_hex ?? record.status.sha256_hex ?? "missing";
}

function directoryContentLabel(status: FileOperationStatus): string {
  return `${directoryCountsLabel(status)} · ${hashLabel("content", status.content_manifest_sha256_hex ?? status.sha256_hex)}`;
}

function directoryCountsLabel(status: FileOperationStatus | undefined): string {
  if (!status) {
    return "manifest unavailable";
  }
  const parts = [];
  if (typeof status.file_count === "number") {
    parts.push(`${status.file_count} files`);
  }
  if (typeof status.directory_count === "number") {
    parts.push(`${status.directory_count} dirs`);
  }
  return parts.length > 0 ? parts.join(" · ") : "manifest unavailable";
}

function manifestEntryContentKey(entry: FileDownloadManifestEntry | undefined): string {
  if (!entry) {
    return "missing";
  }
  return `${entry.kind ?? "unknown"}:${entry.size_bytes ?? "unknown"}:${entry.sha256_hex ?? "missing"}`;
}

function manifestEntryContentLabel(entry: FileDownloadManifestEntry | undefined): string {
  if (!entry) {
    return "missing";
  }
  return `${typeof entry.size_bytes === "number" ? formatBytes(entry.size_bytes) : "unknown size"} · ${hashLabel("sha256", entry.sha256_hex)}`;
}

function hashLabel(label: string, value: string | null | undefined): string {
  return `${label} ${value ? shortId(value) : "missing"}`;
}

function clientDisplayName(clientId: string, agentById: Map<string, AgentView>): string {
  const agent = agentById.get(clientId);
  return agent ? targetDisplayName(agent) : clientId;
}

function targetDisplayName(target: Pick<AgentView, "display_name" | "id">): string {
  const name = target.display_name?.trim();
  if (!name || name === target.id) {
    return target.id;
  }
  return name;
}

async function downloadBulkBundle(
  onDownloadFileBundle: (jobId: string, clientIds: string[]) => Promise<Blob>,
  jobId: string | null,
  clientIds: string[],
) {
  const uniqueClientIds = Array.from(new Set(clientIds));
  if (!jobId || uniqueClientIds.length === 0) {
    return;
  }
  const blob = await onDownloadFileBundle(jobId, uniqueClientIds);
  saveBlob(blob, `bulk-download-${uniqueClientIds.length}-targets.tar`);
}

function saveBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = name || "download.bin";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

function statusLabel(status: NonNullable<ReturnType<typeof parseLatestFileStatus>>): string {
  if (status.type === "file_download") {
    return status.archive ? "Folder tar" : "File download";
  }
  if (status.type === "file_push" || status.type === "file_push_chunked") {
    return "Uploaded file";
  }
  return status.type;
}

function statusDetail(status: NonNullable<ReturnType<typeof parseLatestFileStatus>>): string {
  const parts = [status.path];
  if (typeof status.size_bytes === "number") {
    parts.push(formatBytes(status.size_bytes));
  }
  if (status.sha256_hex) {
    parts.push(`sha256 ${shortId(status.sha256_hex)}`);
  }
  if ("mode" in status && typeof status.mode === "number") {
    parts.push(`mode ${formatMode(status.mode)}`);
  }
  if (status.overwrite_policy) {
    parts.push(`${status.overwrite_policy} existing`);
  }
  if (status.ownership_status) {
    parts.push(`ownership ${status.ownership_status}`);
  }
  return parts.join(" · ");
}

function statusPreview(status: NonNullable<ReturnType<typeof parseLatestFileStatus>>): string {
  if (status.type === "file_download") {
    return [status.source_kind, status.filename, status.content_type].filter(Boolean).join(" · ");
  }
  if (status.type === "file_read_text" && "content_base64" in status && typeof status.content_base64 === "string") {
    const text = decodedText(status as Parameters<typeof decodedText>[0]).trimEnd();
    if (text.length <= 600) {
      return text;
    }
    return `${text.slice(0, 600)}\n...`;
  }
  return "";
}

async function loadOutputsForSummary(
  jobId: string,
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>,
  targetRecords: JobTargetRecord[],
): Promise<JobOutputRecord[]> {
  const expectedStatusOutputs = targetRecords.filter((target) => target.status === "completed").length;
  let lastOutputs: JobOutputRecord[] = [];
  const deadline = Date.now() + BULK_OUTPUT_SUMMARY_WAIT_MS;
  while (Date.now() <= deadline) {
    try {
      lastOutputs = await onLoadOutputs(jobId);
    } catch {
      lastOutputs = [];
    }
    const statusOutputCount = lastOutputs.filter((output) => output.stream === "status" && output.done).length;
    if (expectedStatusOutputs === 0 || statusOutputCount >= expectedStatusOutputs) {
      return lastOutputs;
    }
    await new Promise((resolve) => window.setTimeout(resolve, BULK_OUTPUT_SUMMARY_POLL_INTERVAL_MS));
  }
  return lastOutputs;
}

function parseMode(value: string): number {
  const normalized = value.trim() || "0644";
  return parseFileMode(normalized);
}

function uploadDestinationPath(path: string, fileName: string): string {
  const trimmed = path.trim();
  if (trimmed.endsWith("/")) {
    return normalizeAbsolutePath(`${trimmed}${fileName}`);
  }
  return normalizeAbsolutePath(trimmed);
}

function usesFileActionPolicy(action: MultiFileAction): boolean {
  return action === "write_text" || action === "mkdir" || action === "copy" || action === "rename" || action === "chmod" || action === "chown" || action === "delete";
}

function isAdvancedAction(action: MultiFileAction): boolean {
  return !["download_files", "upload_file"].includes(action);
}

function runBulkActionClass(action: MultiFileAction): string {
  if (action === "delete") {
    return "dangerAction";
  }
  if (isAdvancedAction(action)) {
    return "secondaryAction";
  }
  return "primaryAction";
}

function runBulkActionLabel(action: MultiFileAction): string {
  switch (action) {
    case "download_files":
      return "Review download";
    case "upload_file":
      return "Review upload";
    case "copy":
      return "Review copy";
    case "rename":
      return "Review move";
    case "delete":
      return "Review delete";
    case "chmod":
      return "Review chmod";
    case "chown":
      return "Review chown";
    case "mkdir":
      return "Review create";
    case "write_text":
      return "Review write";
    default:
      return "Review action";
  }
}

function confirmationPolicyText(operation: JobOperation): string {
  if (operation.type === "file_download") {
    return "";
  }
  if (operation.type === "file_push" || operation.type === "file_push_chunked") {
    return `Upload policy: ${operation.existing_policy ?? "skip"} existing files; ownership ${operation.ownership_policy ?? "fail"}.`;
  }
  if (operation.type === "file_rename") {
    return `Policy: ${operation.policy}; destination ${operation.overwrite ? "may be atomically replaced when compatible" : "must not already exist"}.`;
  }
  if (operation.type === "file_copy") {
    return `Policy: ${operation.policy}; destination ${operation.overwrite ? "files may be overwritten; directories are merged" : "must not already exist"}.`;
  }
  if ("policy" in operation) {
    return `Policy: ${operation.policy}.`;
  }
  return "";
}

function compactConfirmationDetail(operation: JobOperation, targetCount: number): string {
  const policy = confirmationPolicyText(operation);
  return `${fileBrowserOperationLabel(operation)} on ${vpsCountLabel(targetCount)}${policy ? `. ${policy}` : ""}`;
}

function confirmationItems(confirmation: PendingBulkConfirmation): Array<{ label: string; value: string | number }> {
  const operation = confirmation.operation;
  const items: Array<{ label: string; value: string | number }> = [
    { label: "Selector", value: confirmation.selectorExpression },
    { label: "Targets", value: confirmation.targets.length },
    { label: "Operation", value: operation.type },
  ];
  if ("path" in operation) {
    items.push({ label: "Path", value: operation.path });
  }
  if ("new_path" in operation) {
    items.push({ label: "Destination", value: operation.new_path });
  }
  if ("recursive" in operation) {
    items.push({ label: "Recursive", value: operation.recursive ? "yes" : "no" });
  }
  if ("overwrite" in operation) {
    items.push({ label: "Overwrite", value: operation.overwrite ? "yes" : "no" });
  }
  if ("policy" in operation && (typeof operation.policy === "string" || typeof operation.policy === "undefined")) {
    items.push({ label: "Policy", value: operation.policy ?? "fail" });
  }
  if ("existing_policy" in operation) {
    items.push({ label: "Existing file", value: operation.existing_policy ?? "skip" });
  }
  if ("ownership_policy" in operation) {
    items.push({ label: "Owner/group", value: operation.ownership_policy ?? "fail" });
  }
  return items;
}

function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

function formatMode(mode: number): string {
  return `0${(mode & 0o777).toString(8)}`;
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}
