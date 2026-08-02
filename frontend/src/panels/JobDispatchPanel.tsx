import {
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
} from "react";
import { LockKeyhole, Play, ShieldCheck } from "lucide-react";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { PrivilegeLockPrompt } from "../components/PrivilegeLockPrompt";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { ActionFeedback } from "../components/ActionFeedback";
import { VpsCombobox } from "../components/VpsCombobox";
import {
  FILE_TRANSFER_CHUNK_BYTES,
  readFilePushPayload,
  sha256Hex,
} from "../fileTransfer";
import {
  JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE,
  JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE,
  JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
  type GeneratedJobCommandType,
} from "../generated/protocolContracts";
import {
  DEFAULT_UPDATE_VERSION_URL,
  type JobDispatchPreset,
} from "../jobDispatchPreset";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../hooks/useReviewGenerationGuard";
import { useHistoryEntryState } from "../historyEntryState";
import { scrollIntoViewWithMotion } from "../motion";
import {
  buildPrivilegeAssertion,
  canonicalJobPrivilegeIntent,
  operationPayloadHashHex,
  parseCommandArgv,
  rolloutPolicyHashHex,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../privilege";
import {
  DEFAULT_JOB_BACKUP_PATHS,
  DEFAULT_TERMINAL_ARGV,
} from "../presets/jobOperationPresets";
import {
  runBrowserResumableDownload,
  runBrowserResumableUpload,
  MAX_FILE_TRANSFER_RATE_LIMIT_KBPS,
  type BrowserDownloadSinkMode,
  type BrowserTransferMultiTargetPolicy,
  type ResumableDownloadProgress,
  type ResumableUploadProgress,
} from "../resumableFileTransfer";
import {
  buildOperation,
  clampJobMaxTimeoutSecs,
  clampInteger,
  effectiveJobMaxTimeoutSecs,
  operationCommandLabel,
  parseOptionalJobMaxTimeoutSecs,
  parseBackupPaths,
  supervisorReady,
  terminalReady,
  type DispatchMode,
  type SupervisorAction,
  type TerminalAction,
} from "./jobDispatchModel";
import type {
  AgentView,
  BulkResolveResponse,
  CommandTemplateRecord,
  CreateJobApprovalRequest,
  CreateJobRequest,
  CreateJobResponse,
  DeleteCommandTemplateRequest,
  FileExistingPolicy,
  JobHistoryRecord,
  JobApprovalRecord,
  JobOperation,
  JobOutputRecord,
  JobRolloutPolicy,
  JobTargetRecord,
  JobTargetSelection,
  UpsertCommandTemplateRequest,
} from "../types";
import type { FileTransferSourceArtifactRecord } from "../typesFileTransfer";
import { runPanelAction, shortId } from "../utils";
import { DispatchOptions, JobTargetSelector } from "./JobDispatchControls";
import {
  JobOperationEditor,
  OperationModeTabs,
} from "./jobs/JobOperationControls";
import {
  agentsMatchingExpression,
  parseSearchExpression,
} from "../searchExpression";
import {
  TargetImpactPreview,
  targetImpactModeForDispatch,
} from "./TargetImpactPreview";

const JOB_SELECTOR_STORAGE_KEY = "vpsman.jobDispatch.selectorExpression";

function formatArgvForInput(argv: string[]): string {
  return argv.map(shellQuoteArg).join(" ");
}

function shellQuoteArg(value: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

function commandTypeForApi(
  operation: CreateJobRequest["operation"],
): GeneratedJobCommandType {
  if (!operation) {
    return "shell_argv";
  }
  if (operation.type === "shell") {
    return operation.pty ? "shell_pty" : "shell_argv";
  }
  return JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
}

function displayGroupForOperation(
  operation: CreateJobRequest["operation"],
): string | null {
  if (!operation) {
    return JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE.shell_argv;
  }
  return (
    JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE[commandTypeForApi(operation)] ??
    null
  );
}

function readLocalString(key: string): string {
  if (typeof window === "undefined") {
    return "";
  }
  try {
    return window.localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function writeLocalString(key: string, value: string) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    if (value.trim()) {
      window.localStorage.setItem(key, value);
    } else {
      window.localStorage.removeItem(key);
    }
  } catch {
    // Browser-local selector persistence must never block dispatch.
  }
}

function visibleDispatchSelector(value: string): string {
  const trimmed = value.trim();
  if (trimmed === "id:*" || trimmed === "*") {
    return "";
  }
  return value;
}

function normalizedDispatchSelector(value: string): string {
  return value.trim() || "id:*";
}

type DispatchConfirmationSnapshot = {
  operationLabel: string;
  forceUnprivileged: boolean;
  selectorExpression: string;
  targets: AgentView[];
  maxTimeoutSecs: number;
  maxTimeoutOverrideSecs?: number;
  rollout: JobRolloutPolicy | null;
} & (
  | {
      kind: "job";
      argv: string[];
      commandType: GeneratedJobCommandType;
      destructive: boolean;
      jobId: string;
      operation: CreateJobRequest["operation"];
      payloadHashHex: string;
      privilegeAssertion: PrivilegeAssertion;
    }
  | {
      kind: "transfer_upload";
      chunkSizeBytes: number;
      existingPolicy: FileExistingPolicy;
      file: File | null;
      fileSha256Hex: string;
      modeText: string;
      multiTargetPolicy: BrowserTransferMultiTargetPolicy;
      path: string;
      privilegeMaterial: PrivilegeMaterial;
      rateLimitKbps: number;
      resumeToken: string;
      sessionId: string;
    }
  | {
      kind: "transfer_download";
      chunkSizeBytes: number;
      downloadName: string;
      downloadSink: BrowserDownloadSinkMode;
      followSymlinks: boolean;
      path: string;
      privilegeMaterial: PrivilegeMaterial;
      rateLimitKbps: number;
      resumeToken: string;
      sessionId: string;
    }
);

function jobRequestFromConfirmation(
  snapshot: Extract<DispatchConfirmationSnapshot, { kind: "job" }>,
  confirmed: boolean,
): CreateJobRequest {
  return {
    job_id: snapshot.jobId,
    selector_expression: snapshot.selectorExpression,
    target_client_ids: snapshot.targets.map((target) => target.id),
    destructive: snapshot.destructive,
    confirmed,
    command: snapshot.commandType,
    argv: snapshot.argv,
    operation: snapshot.operation,
    ...(snapshot.maxTimeoutOverrideSecs !== undefined
      ? { max_timeout_secs: snapshot.maxTimeoutOverrideSecs }
      : {}),
    force_unprivileged: snapshot.forceUnprivileged,
    privileged: true,
    privilege_assertion: snapshot.privilegeAssertion,
    rollout: snapshot.rollout,
  };
}

async function loadUploadSourceArtifactFile(
  sources: FileTransferSourceArtifactRecord[],
  sourceArtifactId: string,
  sourcesTruncated: boolean,
  downloadSource: (downloadPath: string) => Promise<Blob>,
): Promise<File> {
  const artifact = sources.find((source) => source.id === sourceArtifactId);
  if (!artifact) {
    if (sourceArtifactId && sourcesTruncated) {
      throw new Error(
        "The selected reusable source is not in the loaded source page; older artifacts may exist. Select a loaded source before review.",
      );
    }
    throw new Error("Select a reusable source");
  }
  const blob = await downloadSource(artifact.download_path);
  const bytes = new Uint8Array(await blob.arrayBuffer());
  if (bytes.byteLength !== artifact.size_bytes) {
    throw new Error(`Reusable source size mismatch for ${artifact.name}`);
  }
  const actualSha256Hex = await sha256Hex(bytes);
  if (actualSha256Hex !== artifact.sha256_hex) {
    throw new Error(`Reusable source SHA-256 mismatch for ${artifact.name}`);
  }
  return new File([bytes], artifact.name || "reusable-source.bin", {
    type: blob.type || "application/octet-stream",
  });
}

function useDispatchHistoryState<T>(
  slot: string,
  initial: T | (() => T),
  enabled: boolean,
) {
  return useHistoryEntryState(`jobs.dispatch.${slot}`, initial, enabled);
}

type VisibleTransferProgress =
  | Omit<ResumableUploadProgress, "resumeToken">
  | Omit<ResumableDownloadProgress, "resumeToken">;

function visibleTransferProgress(
  progress: ResumableUploadProgress | ResumableDownloadProgress,
): VisibleTransferProgress {
  const { resumeToken: _resumeToken, ...visible } = progress;
  return visible;
}

export function JobDispatchPanel({
  agents,
  fileTransferSources,
  fileTransferSourcesTruncated,
  commandTemplates,
  commandTemplatesTruncated,
  dispatchPreset,
  fixedMode,
  surface = "jobs",
  onDispatchPresetApplied,
  onCreateJob,
  onCreateJobApproval,
  onDownloadFileTransferSource,
  onDownloadOutputChunk,
  onOpenJobsDispatch,
  onOpenRollout,
  onOpenRemoteTerminal,
  onLoadJob,
  onLoadOutputs,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onApprovalRequested,
  onResolveTargets,
  onDeleteCommandTemplate,
  onUpsertCommandTemplate,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
  fileTransferSourcesTruncated: boolean;
  commandTemplates: CommandTemplateRecord[];
  commandTemplatesTruncated: boolean;
  dispatchPreset?: JobDispatchPreset | null;
  fixedMode?: DispatchMode;
  surface?: "jobs" | "terminal";
  onDispatchPresetApplied?: () => void;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateJobApproval?: (
    request: CreateJobApprovalRequest,
  ) => Promise<JobApprovalRecord>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onDownloadOutputChunk: (
    jobId: string,
    clientId: string,
    seq: number,
  ) => Promise<Blob>;
  onOpenJobsDispatch?: () => void;
  onOpenRollout?: (jobId: string) => void;
  onOpenRemoteTerminal?: () => void;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onApprovalRequested?: (approval: JobApprovalRecord) => void;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
  onDeleteCommandTemplate: (
    templateId: string,
    request: DeleteCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  onUpsertCommandTemplate: (
    request: UpsertCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => Promise<void>;
}) {
  const appliedDispatchPresetRequestId = useRef<string | null>(null);
  // Only the main composer restores operator-safe drafts/results. File
  // objects, resume tokens, terminal input, privilege material, reviews, and
  // errors use ordinary component state and never enter history memory.
  const preserveHistoryState = surface === "jobs" && fixedMode === undefined;
  const [mode, setModeState] = useDispatchHistoryState<DispatchMode>(
    "mode",
    fixedMode ?? "shell",
    preserveHistoryState,
  );
  const [commandText, setCommandText] = useDispatchHistoryState(
    "commandText",
    "",
    preserveHistoryState,
  );
  const [shellPty, setShellPty] = useDispatchHistoryState(
    "shellPty",
    false,
    preserveHistoryState,
  );
  const [shellScript, setShellScript] = useDispatchHistoryState(
    "shellScript",
    "",
    preserveHistoryState,
  );
  // Terminal workflows use their dedicated surface, so these values never
  // belong to the Jobs composer history snapshot.
  const terminalAction: TerminalAction = "open";
  const [terminalSessionId, setTerminalSessionId] = useState<string>(() =>
    crypto.randomUUID(),
  );
  const [terminalArgv, setTerminalArgv] = useState(DEFAULT_TERMINAL_ARGV);
  const [terminalCwd, setTerminalCwd] = useState("");
  const [terminalUser, setTerminalUser] = useState("");
  const [terminalUserPolicy, setTerminalUserPolicy] = useState<
    "fail" | "fallback"
  >("fail");
  const [terminalCols, setTerminalCols] = useState(120);
  const [terminalRows, setTerminalRows] = useState(40);
  const [terminalReplayFromSeq, setTerminalReplayFromSeq] = useState("");
  const [terminalIdleTimeoutSecs, setTerminalIdleTimeoutSecs] = useState(3600);
  const [terminalFlowWindowBytes, setTerminalFlowWindowBytes] = useState(65536);
  const [filePath, setFilePath] = useDispatchHistoryState(
    "filePath",
    "",
    preserveHistoryState,
  );
  const [fileFollowSymlinks, setFileFollowSymlinks] = useDispatchHistoryState(
    "fileFollowSymlinks",
    false,
    preserveHistoryState,
  );
  const [filePushPath, setFilePushPath] = useDispatchHistoryState(
    "filePushPath",
    "",
    preserveHistoryState,
  );
  const [filePushMode, setFilePushMode] = useDispatchHistoryState(
    "filePushMode",
    "0644",
    preserveHistoryState,
  );
  const [filePushSource, setFilePushSource] = useState<File | null>(null);
  const [fileTransferUploadSourceKind, setFileTransferUploadSourceKind] =
    useDispatchHistoryState<"local-file" | "source-artifact">(
      "fileTransferUploadSourceKind",
      "local-file",
      preserveHistoryState,
    );
  const [fileTransferSourceArtifactId, setFileTransferSourceArtifactId] =
    useDispatchHistoryState(
      "fileTransferSourceArtifactId",
      "",
      preserveHistoryState,
    );
  const [fileTransferSessionId, setFileTransferSessionId] = useState("");
  const [fileTransferResumeToken, setFileTransferResumeToken] = useState("");
  const [fileTransferDownloadName, setFileTransferDownloadName] =
    useDispatchHistoryState(
      "fileTransferDownloadName",
      "",
      preserveHistoryState,
    );
  const [fileTransferDownloadSink, setFileTransferDownloadSink] =
    useDispatchHistoryState<BrowserDownloadSinkMode>(
      "fileTransferDownloadSink",
      "browser-download",
      preserveHistoryState,
    );
  const [fileTransferChunkSize, setFileTransferChunkSize] =
    useDispatchHistoryState(
      "fileTransferChunkSize",
      65536,
      preserveHistoryState,
    );
  const [fileTransferRateLimit, setFileTransferRateLimit] =
    useDispatchHistoryState("fileTransferRateLimit", 0, preserveHistoryState);
  const [fileTransferExistingPolicy, setFileTransferExistingPolicy] =
    useDispatchHistoryState<FileExistingPolicy>(
      "fileTransferExistingPolicy",
      "skip",
      preserveHistoryState,
    );
  const [fileTransferMultiTargetPolicy, setFileTransferMultiTargetPolicy] =
    useDispatchHistoryState<BrowserTransferMultiTargetPolicy>(
      "fileTransferMultiTargetPolicy",
      "same-offset",
      preserveHistoryState,
    );
  const [selectedTemplateId, setSelectedTemplateId] = useDispatchHistoryState(
    "selectedTemplateId",
    "",
    preserveHistoryState,
  );
  const [templateName, setTemplateName] = useDispatchHistoryState(
    "templateName",
    "",
    preserveHistoryState,
  );
  const [templateScopeKind, setTemplateScopeKind] = useDispatchHistoryState<
    "global" | "provider" | "tag" | "client"
  >("templateScopeKind", "global", preserveHistoryState);
  const [templateScopeValue, setTemplateScopeValue] = useDispatchHistoryState(
    "templateScopeValue",
    "",
    preserveHistoryState,
  );
  const [templatePending, setTemplatePending] = useState(false);
  const [templateError, setTemplateError] = useState<string | null>(null);
  const [templateConfirmation, setTemplateConfirmation] = useState<
    "save" | "save-copy" | "delete" | null
  >(null);
  const [templateSaveSnapshot, setTemplateSaveSnapshot] = useState<{
    request: UpsertCommandTemplateRequest;
    title: string;
  } | null>(null);
  const [deleteTemplateSnapshot, setDeleteTemplateSnapshot] =
    useState<CommandTemplateRecord | null>(null);
  const templateFeedbackRef = useRef<HTMLDivElement | null>(null);
  const [updateArtifactUrl, setUpdateArtifactUrl] = useDispatchHistoryState(
    "updateArtifactUrl",
    "",
    preserveHistoryState,
  );
  const [updateSha256Hex, setUpdateSha256Hex] = useDispatchHistoryState(
    "updateSha256Hex",
    "",
    preserveHistoryState,
  );
  const [updateCheckVersionUrl, setUpdateCheckVersionUrl] =
    useDispatchHistoryState(
      "updateCheckVersionUrl",
      DEFAULT_UPDATE_VERSION_URL,
      preserveHistoryState,
    );
  const [updateActivationSha256Hex, setUpdateActivationSha256Hex] =
    useDispatchHistoryState(
      "updateActivationSha256Hex",
      "",
      preserveHistoryState,
    );
  const [updateRestartAgent, setUpdateRestartAgent] = useDispatchHistoryState(
    "updateRestartAgent",
    false,
    preserveHistoryState,
  );
  const [updateRollbackSha256Hex, setUpdateRollbackSha256Hex] =
    useDispatchHistoryState(
      "updateRollbackSha256Hex",
      "",
      preserveHistoryState,
    );
  const [backupPathsText, setBackupPathsText] = useDispatchHistoryState(
    "backupPathsText",
    DEFAULT_JOB_BACKUP_PATHS,
    preserveHistoryState,
  );
  const [backupIncludeConfig, setBackupIncludeConfig] = useDispatchHistoryState(
    "backupIncludeConfig",
    true,
    preserveHistoryState,
  );
  const [backupFollowSymlinks, setBackupFollowSymlinks] =
    useDispatchHistoryState(
      "backupFollowSymlinks",
      false,
      preserveHistoryState,
    );
  const [backupSkipMissingPaths, setBackupSkipMissingPaths] =
    useDispatchHistoryState(
      "backupSkipMissingPaths",
      false,
      preserveHistoryState,
    );
  const [processLimit, setProcessLimit] = useDispatchHistoryState(
    "processLimit",
    50,
    preserveHistoryState,
  );
  const [supervisorAction, setSupervisorAction] =
    useDispatchHistoryState<SupervisorAction>(
      "supervisorAction",
      "status",
      preserveHistoryState,
    );
  const [supervisorName, setSupervisorName] = useDispatchHistoryState(
    "supervisorName",
    "",
    preserveHistoryState,
  );
  const [supervisorArgv, setSupervisorArgv] = useDispatchHistoryState(
    "supervisorArgv",
    "",
    preserveHistoryState,
  );
  const [supervisorCwd, setSupervisorCwd] = useDispatchHistoryState(
    "supervisorCwd",
    "",
    preserveHistoryState,
  );
  const [supervisorEnv, setSupervisorEnv] = useDispatchHistoryState(
    "supervisorEnv",
    "",
    preserveHistoryState,
  );
  const [supervisorLogBytes, setSupervisorLogBytes] = useDispatchHistoryState(
    "supervisorLogBytes",
    65536,
    preserveHistoryState,
  );
  const [selectorExpression, setSelectorExpression] = useDispatchHistoryState(
    "selectorExpression",
    () => visibleDispatchSelector(readLocalString(JOB_SELECTOR_STORAGE_KEY)),
    preserveHistoryState,
  );
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useDispatchHistoryState(
    "maxTimeoutSecs",
    "",
    preserveHistoryState,
  );
  const [forceUnprivileged, setForceUnprivileged] = useDispatchHistoryState(
    "forceUnprivileged",
    false,
    preserveHistoryState,
  );
  const [rolloutEnabled, setRolloutEnabled] = useDispatchHistoryState(
    "rolloutEnabled",
    false,
    preserveHistoryState,
  );
  const [rolloutCanaryClientId, setRolloutCanaryClientId] =
    useDispatchHistoryState("rolloutCanaryClientId", "", preserveHistoryState);
  const [rolloutBatchSize, setRolloutBatchSize] = useDispatchHistoryState(
    "rolloutBatchSize",
    "5",
    preserveHistoryState,
  );
  const [rolloutMaxFailures, setRolloutMaxFailures] = useDispatchHistoryState(
    "rolloutMaxFailures",
    "0",
    preserveHistoryState,
  );
  const [rolloutPauseAfterCanary, setRolloutPauseAfterCanary] =
    useDispatchHistoryState(
      "rolloutPauseAfterCanary",
      true,
      preserveHistoryState,
    );
  const [rolloutBatchDelaySecs, setRolloutBatchDelaySecs] =
    useDispatchHistoryState("rolloutBatchDelaySecs", "0", preserveHistoryState);
  // The target preview is refreshed from the restored selector; caching it
  // would briefly expose stale target resolution after Back.
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [dispatchProgress, setDispatchProgress] =
    useState<BulkJobProgress | null>(null);
  const [lastDispatchProgress, setLastDispatchProgress] =
    useDispatchHistoryState<BulkJobProgress | null>(
      "lastDispatchProgress",
      null,
      preserveHistoryState,
    );
  const [lastDispatchContext, setLastDispatchContext] = useDispatchHistoryState<
    string | null
  >("lastDispatchContext", null, preserveHistoryState);
  const [lastPayloadHash, setLastPayloadHash] = useDispatchHistoryState<
    string | null
  >("lastPayloadHash", null, preserveHistoryState);
  const [lastRolloutJobId, setLastRolloutJobId] = useDispatchHistoryState<
    string | null
  >("lastRolloutJobId", null, preserveHistoryState);
  const [transferProgress, setTransferProgress] =
    useDispatchHistoryState<VisibleTransferProgress | null>(
      "transferProgress",
      null,
      preserveHistoryState,
    );
  const [actionError, setActionError] = useState<string | null>(null);
  const [dispatchPromptOpen, setDispatchPromptOpen] = useState(false);
  const [lockPromptOpen, setLockPromptOpen] = useState(false);
  const [lockPending, setLockPending] = useState(false);
  const [dispatchConfirmation, setDispatchConfirmation] =
    useState<DispatchConfirmationSnapshot | null>(null);
  const [dispatchReviewIntent, setDispatchReviewIntent] = useState<
    "dispatch" | "approval"
  >("dispatch");
  const [approvalRequestReason, setApprovalRequestReason] = useState("");
  const [selectorVerification, setSelectorVerification] = useState<
    "checking" | "invalid" | "neutral" | "valid"
  >("neutral");
  const [selectorVerificationMessage, setSelectorVerificationMessage] =
    useState<string | null>(null);
  const [selectorVerificationError, setSelectorVerificationError] = useState<
    string | null
  >(null);
  const [pending, setPending] = useState(false);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const normalizedSelectorExpression =
    normalizedDispatchSelector(selectorExpression);
  const selectorParse = useMemo(
    () => parseSearchExpression(normalizedSelectorExpression),
    [normalizedSelectorExpression],
  );
  const terminalSurface = surface === "terminal";

  function setMode(nextMode: DispatchMode) {
    setModeState(fixedMode ?? nextMode);
  }

  useEffect(() => {
    if (fixedMode) {
      setModeState(fixedMode);
    }
  }, [fixedMode]);

  useEffect(() => {
    if (!terminalSurface && !fixedMode && mode === "terminal_session") {
      setModeState("shell");
    }
  }, [fixedMode, mode, terminalSurface]);

  useEffect(() => {
    if (
      !dispatchPreset ||
      appliedDispatchPresetRequestId.current === dispatchPreset.requestId
    ) {
      return;
    }
    if (
      dispatchPreset.commandTemplateId &&
      !commandTemplates.some(
        (template) => template.id === dispatchPreset.commandTemplateId,
      )
    ) {
      if (commandTemplatesTruncated) {
        appliedDispatchPresetRequestId.current = dispatchPreset.requestId;
        setActionError(
          "The requested command template is not in the loaded template page; older templates may exist. Select a loaded template before review.",
        );
        onDispatchPresetApplied?.();
      }
      return;
    }
    appliedDispatchPresetRequestId.current = dispatchPreset.requestId;
    if (fixedMode && dispatchPreset.mode !== fixedMode) {
      onDispatchPresetApplied?.();
      return;
    }
    setModeState(fixedMode ?? dispatchPreset.mode);
    if (dispatchPreset.selectorExpression !== undefined) {
      setSelectorExpression(
        visibleDispatchSelector(dispatchPreset.selectorExpression),
      );
    }
    if (dispatchPreset.commandTemplateId) {
      applyCommandTemplate(dispatchPreset.commandTemplateId);
    }
    if (dispatchPreset.maxTimeoutSecs !== undefined) {
      setMaxTimeoutSecs(
        String(clampJobMaxTimeoutSecs(dispatchPreset.maxTimeoutSecs)),
      );
    } else if (
      dispatchPreset.mode === "agent_update_activate" ||
      dispatchPreset.mode === "agent_update_rollback"
    ) {
      setMaxTimeoutSecs("60");
    } else if (dispatchPreset.mode.startsWith("agent_update")) {
      setMaxTimeoutSecs("300");
    }
    if (dispatchPreset.mode === "agent_update") {
      setUpdateArtifactUrl(dispatchPreset.updateArtifactUrl ?? "");
      setUpdateSha256Hex(dispatchPreset.updateSha256Hex ?? "");
    }
    if (dispatchPreset.mode === "agent_update_check") {
      setUpdateCheckVersionUrl(
        dispatchPreset.updateCheckVersionUrl ?? DEFAULT_UPDATE_VERSION_URL,
      );
    }
    if (dispatchPreset.mode === "agent_update_activate") {
      setUpdateActivationSha256Hex(
        dispatchPreset.updateActivationSha256Hex ?? "",
      );
      setUpdateRestartAgent(dispatchPreset.updateRestartAgent ?? true);
    }
    if (dispatchPreset.mode === "agent_update_rollback") {
      setUpdateRollbackSha256Hex(dispatchPreset.updateRollbackSha256Hex ?? "");
    }
    if (dispatchPreset.mode === "process_supervisor") {
      setSupervisorAction(dispatchPreset.supervisorAction ?? "status");
      setSupervisorName(dispatchPreset.supervisorName ?? "");
      setSupervisorArgv(dispatchPreset.supervisorArgv ?? "");
      setSupervisorCwd(dispatchPreset.supervisorCwd ?? "");
      setSupervisorEnv(dispatchPreset.supervisorEnv ?? "");
      setSupervisorLogBytes(dispatchPreset.supervisorLogBytes ?? 65536);
      if (dispatchPreset.maxTimeoutSecs === undefined) {
        setMaxTimeoutSecs(
          dispatchPreset.supervisorAction === "logs" ? "30" : "60",
        );
      }
    }
    if (dispatchPreset.mode === "file_transfer_upload") {
      setFilePushPath(dispatchPreset.filePushPath ?? "");
      setFilePushMode(dispatchPreset.filePushMode ?? "0644");
      setFilePushSource(dispatchPreset.fileTransferUploadFile ?? null);
      setFileTransferUploadSourceKind(
        dispatchPreset.fileTransferUploadSourceKind ?? "local-file",
      );
      setFileTransferSourceArtifactId(
        dispatchPreset.fileTransferSourceArtifactId ?? "",
      );
      setFileTransferSessionId(dispatchPreset.fileTransferSessionId ?? "");
      setFileTransferResumeToken(dispatchPreset.fileTransferResumeToken ?? "");
      setFileTransferChunkSize(
        clampInteger(
          dispatchPreset.fileTransferChunkSize ?? 65536,
          1,
          FILE_TRANSFER_CHUNK_BYTES,
        ),
      );
      setFileTransferRateLimit(
        clampInteger(
          dispatchPreset.fileTransferRateLimit ?? 0,
          0,
          MAX_FILE_TRANSFER_RATE_LIMIT_KBPS,
        ),
      );
      setFileTransferExistingPolicy(
        dispatchPreset.fileTransferExistingPolicy ?? "skip",
      );
      setFileTransferMultiTargetPolicy(
        dispatchPreset.fileTransferMultiTargetPolicy ?? "same-offset",
      );
    }
    if (dispatchPreset.mode === "file_transfer_download") {
      setFilePath(dispatchPreset.filePath ?? "");
      setFileFollowSymlinks(dispatchPreset.fileFollowSymlinks ?? false);
      setFileTransferDownloadName(
        dispatchPreset.fileTransferDownloadName ?? "",
      );
      setFileTransferDownloadSink(
        dispatchPreset.fileTransferDownloadSink ?? "browser-download",
      );
      setFileTransferSessionId(dispatchPreset.fileTransferSessionId ?? "");
      setFileTransferResumeToken(dispatchPreset.fileTransferResumeToken ?? "");
      setFileTransferChunkSize(
        clampInteger(
          dispatchPreset.fileTransferChunkSize ?? 65536,
          1,
          FILE_TRANSFER_CHUNK_BYTES,
        ),
      );
      setFileTransferRateLimit(
        clampInteger(
          dispatchPreset.fileTransferRateLimit ?? 0,
          0,
          MAX_FILE_TRANSFER_RATE_LIMIT_KBPS,
        ),
      );
    }
    setPreview(null);
    clearDispatchReview();
    setActionError(null);
    clearExecutionResults();
    onDispatchPresetApplied?.();
  }, [
    commandTemplates,
    commandTemplatesTruncated,
    dispatchPreset,
    onDispatchPresetApplied,
  ]);

  useEffect(() => {
    writeLocalString(JOB_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  useEffect(() => {
    setTemplateSaveSnapshot(null);
    setDeleteTemplateSnapshot(null);
    setTemplateError(null);
    setTemplateConfirmation((current) =>
      current === "save" || current === "save-copy" || current === "delete"
        ? null
        : current,
    );
  }, [selectedTemplateId, templateName, templateScopeKind, templateScopeValue]);

  useLayoutEffect(() => {
    invalidateReviewGeneration();
    setDispatchPromptOpen(false);
    setDispatchConfirmation(null);
    setReviewStatus(null);
    setTemplateSaveSnapshot(null);
    setTemplateError(null);
    setTemplateConfirmation((current) =>
      current === "save" || current === "save-copy" ? null : current,
    );
  }, [
    backupIncludeConfig,
    backupFollowSymlinks,
    backupSkipMissingPaths,
    backupPathsText,
    commandText,
    fileFollowSymlinks,
    filePath,
    filePushMode,
    filePushPath,
    filePushSource,
    fileTransferChunkSize,
    fileTransferDownloadName,
    fileTransferDownloadSink,
    fileTransferExistingPolicy,
    fileTransferMultiTargetPolicy,
    fileTransferRateLimit,
    fileTransferResumeToken,
    fileTransferSessionId,
    fileTransferSourceArtifactId,
    fileTransferUploadSourceKind,
    forceUnprivileged,
    mode,
    privilegeMaterial,
    processLimit,
    rolloutBatchDelaySecs,
    rolloutBatchSize,
    rolloutCanaryClientId,
    rolloutEnabled,
    rolloutMaxFailures,
    rolloutPauseAfterCanary,
    selectorExpression,
    shellPty,
    shellScript,
    supervisorAction,
    supervisorArgv,
    supervisorCwd,
    supervisorEnv,
    supervisorLogBytes,
    supervisorName,
    terminalAction,
    terminalArgv,
    terminalCols,
    terminalCwd,
    terminalFlowWindowBytes,
    terminalIdleTimeoutSecs,
    terminalReplayFromSeq,
    terminalRows,
    terminalSessionId,
    terminalUser,
    terminalUserPolicy,
    maxTimeoutSecs,
    updateActivationSha256Hex,
    updateArtifactUrl,
    updateCheckVersionUrl,
    updateRestartAgent,
    updateRollbackSha256Hex,
    updateSha256Hex,
    invalidateReviewGeneration,
  ]);

  useEffect(() => {
    if (!templateError) return;
    const frame = window.requestAnimationFrame(() => {
      if (templateFeedbackRef.current) {
        scrollIntoViewWithMotion(templateFeedbackRef.current, {
          block: "nearest",
        });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [templateError]);

  useEffect(() => {
    if (selectorParse.error) {
      setSelectorVerification("invalid");
      setSelectorVerificationMessage("Invalid");
      setSelectorVerificationError(null);
      setPreview(null);
      return;
    }
    let disposed = false;
    setSelectorVerification("checking");
    setSelectorVerificationMessage("Checking");
    setSelectorVerificationError(null);
    const timeout = window.setTimeout(() => {
      void onResolveTargets({
        selector_expression: normalizedSelectorExpression,
      })
        .then((response) => {
          if (disposed) {
            return;
          }
          setPreview(response);
          setSelectorVerification("valid");
          setSelectorVerificationMessage(
            `${response.target_count}/${agents.length}`,
          );
          setSelectorVerificationError(null);
        })
        .catch((error) => {
          if (disposed) {
            return;
          }
          setPreview(null);
          setSelectorVerification("invalid");
          setSelectorVerificationMessage("Unavailable");
          setSelectorVerificationError(
            `Target verification could not complete: ${
              error instanceof Error
                ? error.message
                : "the browser returned no failure detail. Check API availability and retry."
            }`,
          );
        });
    }, 300);
    return () => {
      disposed = true;
      window.clearTimeout(timeout);
    };
  }, [
    agents.length,
    mode,
    normalizedSelectorExpression,
    onResolveTargets,
    selectorParse.error,
  ]);

  const parsedArgv = useMemo(() => {
    try {
      return parseCommandArgv(commandText);
    } catch {
      return [];
    }
  }, [commandText]);

  const filePullReady = filePath.startsWith("/");
  const filePushReady = filePushPath.startsWith("/") && !!filePushSource;
  const fileTransferUploadReady =
    filePushPath.startsWith("/") &&
    (fileTransferUploadSourceKind === "local-file"
      ? !!filePushSource
      : !!fileTransferSourceArtifactId);
  const fileTransferDownloadReady = filePath.startsWith("/");
  const backupReady =
    backupIncludeConfig || parseBackupPaths(backupPathsText).length > 0;
  const operationReady =
    mode === "shell"
      ? parsedArgv.length > 0
      : mode === "shell_script"
        ? shellScript.trim().length > 0
        : mode === "terminal_session"
          ? terminalReady(terminalAction, terminalSessionId, terminalArgv)
          : mode === "file_pull"
            ? filePullReady
            : mode === "file_push"
              ? filePushReady
              : mode === "file_transfer_upload"
                ? fileTransferUploadReady
                : mode === "file_transfer_download"
                  ? fileTransferDownloadReady
                  : mode === "agent_update"
                    ? updateArtifactUrl.startsWith("https://") &&
                      /^[0-9a-fA-F]{64}$/.test(updateSha256Hex.trim())
                    : mode === "agent_update_check"
                      ? updateCheckVersionUrl.trim().length > 0
                      : mode === "agent_update_activate"
                        ? /^[0-9a-fA-F]{64}$/.test(
                            updateActivationSha256Hex.trim(),
                          )
                        : mode === "agent_update_rollback"
                          ? !updateRollbackSha256Hex.trim() ||
                            /^[0-9a-fA-F]{64}$/.test(
                              updateRollbackSha256Hex.trim(),
                            )
                          : mode === "process_supervisor"
                            ? supervisorReady(
                                supervisorAction,
                                supervisorName,
                                supervisorArgv,
                              )
                            : mode === "backup"
                              ? backupReady
                              : true;
  const expressionTargets = useMemo(
    () =>
      selectorParse.error
        ? []
        : agentsMatchingExpression(agents, normalizedSelectorExpression),
    [agents, normalizedSelectorExpression, selectorParse.error],
  );
  const impactMode = targetImpactModeForDispatch(mode);
  const supportsForceUnprivileged = impactMode !== "generic";
  const operationNeedsConfirmation = generatedConfirmationRequiredForMode(
    mode,
    supervisorAction,
  );
  const approvalRequestSupported = Boolean(
    !terminalSurface &&
    onCreateJobApproval &&
    mode !== "file_transfer_upload" &&
    mode !== "file_transfer_download",
  );
  const impactTargets = preview?.targets ?? expressionTargets;
  const rolloutUnavailableReason = rolloutUnsupportedReason(
    terminalSurface,
    fixedMode,
    mode,
  );
  const activeDispatchConfirmation = dispatchPromptOpen
    ? dispatchConfirmation
    : null;
  const dispatchConfirmationSelector =
    activeDispatchConfirmation?.selectorExpression ??
    normalizedSelectorExpression;
  const dispatchConfirmationTargets =
    activeDispatchConfirmation?.targets ??
    preview?.targets ??
    expressionTargets;
  const dispatchConfirmationMaxTimeoutSecs =
    activeDispatchConfirmation?.maxTimeoutSecs ??
    effectiveJobMaxTimeoutSecs(maxTimeoutSecs);
  const dispatchConfirmationForceUnprivileged =
    activeDispatchConfirmation?.forceUnprivileged ??
    (supportsForceUnprivileged ? forceUnprivileged : false);
  const dispatchConfirmationOperationLabel =
    activeDispatchConfirmation?.operationLabel ??
    operationCommandLabel(mode, commandText);
  const dispatchConfirmationTargetNames = dispatchTargetIdentitySummary(
    dispatchConfirmationTargets,
  );
  const focusedModeBoundary = fixedMode
    ? fixedModeBoundaryCopy(fixedMode)
    : null;
  const dispatchConfirmationDestructive =
    activeDispatchConfirmation?.kind === "job"
      ? operationUsesDangerTone(activeDispatchConfirmation.operation)
      : activeDispatchConfirmation?.kind === "transfer_upload"
        ? activeDispatchConfirmation.existingPolicy === "replace"
        : false;
  const dispatchConfirmationFollowSymlinks =
    activeDispatchConfirmation?.kind === "transfer_download"
      ? activeDispatchConfirmation.followSymlinks
      : activeDispatchConfirmation?.kind === "job" &&
          activeDispatchConfirmation.operation?.type === "file_pull"
        ? activeDispatchConfirmation.operation.follow_symlinks
        : null;
  const selectedTemplate =
    commandTemplates.find((template) => template.id === selectedTemplateId) ??
    null;
  const builtinTemplates = useMemo(
    () => commandTemplates.filter((template) => template.built_in),
    [commandTemplates],
  );
  const userTemplates = useMemo(
    () => commandTemplates.filter((template) => !template.built_in),
    [commandTemplates],
  );
  const visibleDispatchProgress = dispatchProgress ?? lastDispatchProgress;
  const dispatchConfirmationItems = [
    { label: "Operation", value: dispatchConfirmationOperationLabel },
    {
      label: "Submission",
      value:
        dispatchReviewIntent === "approval"
          ? "Approval queue; no execution until approved"
          : "Dispatch immediately after confirmation",
    },
    ...transferReviewItems(activeDispatchConfirmation),
    ...operationReviewItems(
      activeDispatchConfirmation?.kind === "job"
        ? activeDispatchConfirmation.operation
        : undefined,
    ),
    ...rolloutReviewItems(activeDispatchConfirmation),
    ...(dispatchConfirmationFollowSymlinks === null
      ? []
      : [
          {
            label: "Symlinks",
            value: dispatchConfirmationFollowSymlinks
              ? "Follow targets"
              : "Do not follow",
          },
        ]),
    { label: "Selector", value: dispatchConfirmationSelector || "-" },
    {
      label: "Targets",
      value: formatTargetAvailabilitySummary(dispatchConfirmationTargets),
    },
    {
      label: "Resolved VPS",
      title: dispatchConfirmationTargetNames.full,
      value: dispatchConfirmationTargetNames.visible,
    },
    { label: "Max timeout", value: `${dispatchConfirmationMaxTimeoutSecs}s` },
    {
      label: "Privilege",
      value: privilegeMaterial ? "Unlocked locally" : "Locked",
    },
    {
      label: "Execution",
      value: dispatchConfirmationForceUnprivileged
        ? "Forced best effort"
        : operationNeedsConfirmation
          ? "Protected operation"
          : "Standard",
    },
  ];
  const dispatchHeaderStatus = privilegeMaterial ? "Ready" : "Locked";
  const dispatchFeedbackMessage =
    actionError ?? selectorVerificationError ?? reviewStatus;
  const dispatchFeedbackTone =
    actionError || selectorVerificationError ? "danger" : "progress";

  async function lockPrivilege() {
    setLockPending(true);
    setActionError(null);
    try {
      await setPrivilegeMaterial(null);
      setLockPromptOpen(false);
      clearDispatchReview();
    } catch (error) {
      setActionError(
        error instanceof Error ? error.message : "Privilege lock failed",
      );
    } finally {
      setLockPending(false);
    }
  }

  function clearExecutionResults() {
    setDispatchProgress(null);
    setLastDispatchProgress(null);
    setLastDispatchContext(null);
    setLastRolloutJobId(null);
    setTransferProgress(null);
  }

  function clearDispatchReview() {
    invalidateReviewGeneration();
    setDispatchPromptOpen(false);
    setDispatchConfirmation(null);
    setDispatchReviewIntent("dispatch");
    setApprovalRequestReason("");
    setReviewStatus(null);
  }

  async function submitJob(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await prepareJobReview("dispatch");
  }

  async function prepareJobReview(intent: "dispatch" | "approval") {
    setActionError(null);
    if (intent === "approval" && !approvalRequestSupported) {
      setActionError("This operation must be dispatched directly");
      return;
    }
    if (!privilegeMaterial) {
      setActionError("Privilege unlock is locked");
      return;
    }
    if (selectorParse.error) {
      setActionError(selectorParse.error);
      return;
    }
    if (!operationReady) {
      setActionError("Complete the selected operation before dispatching");
      return;
    }
    blurActiveElement();
    const reviewGeneration = captureReviewGeneration();
    const selection = targetSelection();
    setReviewStatus(
      intent === "approval"
        ? "Preparing approval request"
        : "Preparing dispatch confirmation",
    );
    try {
      await runPanelAction(setPending, setActionError, async () => {
        await waitForReviewRender();
        const resolved = await onResolveTargets(selection);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        if (!resolved.targets.length) {
          throw new Error("Target confirmation resolved no VPSs");
        }
        const snapshot = await buildDispatchConfirmationSnapshot(
          resolved.targets,
        );
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(resolved);
        setDispatchConfirmation(snapshot);
        setDispatchReviewIntent(intent);
        setApprovalRequestReason("");
        setDispatchPromptOpen(true);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
  }

  async function buildDispatchConfirmationSnapshot(
    targets: AgentView[],
  ): Promise<DispatchConfirmationSnapshot> {
    if (!privilegeMaterial) {
      throw new Error("Privilege unlock is locked");
    }
    const selector = normalizedSelectorExpression;
    const maxTimeoutOverride = parseOptionalJobMaxTimeoutSecs(maxTimeoutSecs);
    const maxTimeout =
      maxTimeoutOverride ?? effectiveJobMaxTimeoutSecs(maxTimeoutSecs);
    const frozenForceUnprivileged = supportsForceUnprivileged
      ? forceUnprivileged
      : false;
    const operationLabel = operationCommandLabel(mode, commandText);
    const rollout = reviewedRolloutPolicy(targets);
    const base = {
      forceUnprivileged: frozenForceUnprivileged,
      operationLabel,
      selectorExpression: selector,
      targets,
      maxTimeoutSecs: maxTimeout,
      maxTimeoutOverrideSecs: maxTimeoutOverride,
      rollout,
    };
    if (mode === "file_transfer_upload") {
      const uploadSourceFile =
        fileTransferUploadSourceKind === "source-artifact"
          ? await loadUploadSourceArtifactFile(
              fileTransferSources,
              fileTransferSourceArtifactId,
              fileTransferSourcesTruncated,
              onDownloadFileTransferSource,
            )
          : filePushSource;
      if (!uploadSourceFile) {
        throw new Error("Choose an upload source before review");
      }
      const uploadSourceBytes = new Uint8Array(
        await uploadSourceFile.arrayBuffer(),
      );
      return {
        ...base,
        chunkSizeBytes: fileTransferChunkSize,
        existingPolicy: fileTransferExistingPolicy,
        file: uploadSourceFile,
        fileSha256Hex: await sha256Hex(uploadSourceBytes),
        kind: "transfer_upload",
        modeText: filePushMode,
        multiTargetPolicy: fileTransferMultiTargetPolicy,
        path: filePushPath,
        privilegeMaterial,
        rateLimitKbps: fileTransferRateLimit,
        resumeToken: fileTransferResumeToken,
        sessionId: fileTransferSessionId,
      };
    }
    if (mode === "file_transfer_download") {
      return {
        ...base,
        chunkSizeBytes: fileTransferChunkSize,
        downloadName: fileTransferDownloadName,
        downloadSink: fileTransferDownloadSink,
        followSymlinks: fileFollowSymlinks,
        kind: "transfer_download",
        path: filePath,
        privilegeMaterial,
        rateLimitKbps: fileTransferRateLimit,
        resumeToken: fileTransferResumeToken,
        sessionId: fileTransferSessionId,
      };
    }
    const filePushPayload =
      mode === "file_push" ? await readFilePushPayload(filePushSource) : null;
    const operation = buildOperation(
      mode,
      commandText,
      shellPty,
      shellScript,
      terminalAction,
      terminalSessionId,
      terminalArgv,
      terminalCwd,
      terminalUser,
      terminalUserPolicy,
      terminalCols,
      terminalRows,
      terminalReplayFromSeq,
      terminalIdleTimeoutSecs,
      terminalFlowWindowBytes,
      filePath,
      fileFollowSymlinks,
      processLimit,
      supervisorAction,
      supervisorName,
      supervisorArgv,
      supervisorCwd,
      supervisorEnv,
      supervisorLogBytes,
      updateArtifactUrl,
      updateSha256Hex,
      updateCheckVersionUrl,
      updateActivationSha256Hex,
      updateRestartAgent,
      updateRollbackSha256Hex,
      backupPathsText,
      backupIncludeConfig,
      backupFollowSymlinks,
      backupSkipMissingPaths,
      filePushPath,
      filePushMode,
      filePushPayload,
    );
    const clientIds = targets.map((target) => target.id);
    const payloadHashHex = await operationPayloadHashHex(operation);
    const commandType = commandTypeForApi(operation);
    if (rollout && operation.type === "network_speed_test") {
      throw new Error(
        "Staged rollout is unsupported for a coordinated network speed test.",
      );
    }
    const rolloutPolicyHash = await rolloutPolicyHashHex(rollout);
    const privilegeAssertion = await buildPrivilegeAssertion({
      intent: canonicalJobPrivilegeIntent({
        selectorExpression: selector,
        commandType,
        operationPayloadHash: payloadHashHex,
        rolloutPolicyHash,
        resolvedTargets: clientIds,
        maxTimeoutSecs: maxTimeout,
        forceUnprivileged: frozenForceUnprivileged,
        privileged: true,
      }),
      privilegeMaterial,
    });
    return {
      ...base,
      argv:
        mode === "shell" && operation.type === "shell" ? operation.argv : [],
      commandType,
      destructive: operationNeedsConfirmation,
      jobId: crypto.randomUUID(),
      kind: "job",
      operation,
      operationLabel: jobOperationLabel(operation, operationLabel),
      payloadHashHex,
      privilegeAssertion,
    };
  }

  function reviewedRolloutPolicy(
    targets: AgentView[],
  ): JobRolloutPolicy | null {
    if (!rolloutEnabled) return null;
    if (rolloutUnavailableReason) {
      throw new Error(rolloutUnavailableReason);
    }
    if (targets.length < 2) {
      throw new Error("Staged rollout requires at least two resolved VPSs.");
    }
    const canary = rolloutCanaryClientId.trim();
    if (!canary || !targets.some((target) => target.id === canary)) {
      throw new Error(
        "Select one canary from the current resolved target scope.",
      );
    }
    return {
      batch_delay_secs: parseRolloutInteger(
        rolloutBatchDelaySecs,
        "Inter-stage delay",
        0,
        86_400,
      ),
      batch_size: parseRolloutInteger(rolloutBatchSize, "Batch size", 1, 100),
      canary_client_ids: [canary],
      max_failures: parseRolloutInteger(
        rolloutMaxFailures,
        "Tolerated failures",
        0,
        100,
      ),
      pause_after_canary: rolloutPauseAfterCanary,
    };
  }

  function applyCommandTemplate(templateId: string) {
    setSelectedTemplateId(templateId);
    const template = commandTemplates.find(
      (candidate) => candidate.id === templateId,
    );
    if (!template) {
      return;
    }
    if (!terminalSurface && template.operation.type === "terminal_open") {
      setSelectedTemplateId("");
      setActionError(
        "Open terminal sessions from Remote / Terminal. Jobs / Dispatch stays focused on generic command, file, backup, update, session, and process dispatch.",
      );
      return;
    }
    applyTemplateOperation(template.operation);
    applyTemplateDefaults(template.defaults);
    setTemplateName(
      template.built_in ? `${template.name} copy` : template.name,
    );
    setTemplateScopeKind(
      template.scope_kind as "global" | "provider" | "tag" | "client",
    );
    setTemplateScopeValue(template.scope_value ?? "");
    setTemplateConfirmation(null);
    setActionError(null);
  }

  function applyTemplateDefaults(defaults: CommandTemplateRecord["defaults"]) {
    if (!defaults || typeof defaults !== "object" || Array.isArray(defaults)) {
      return;
    }
    if (typeof defaults.max_timeout_secs === "number") {
      setMaxTimeoutSecs(
        String(clampJobMaxTimeoutSecs(defaults.max_timeout_secs)),
      );
    }
    if (typeof defaults.force_unprivileged === "boolean") {
      setForceUnprivileged(defaults.force_unprivileged);
    }
  }

  function applyTemplateOperation(
    operation: CommandTemplateRecord["operation"],
  ) {
    switch (operation.type) {
      case "shell":
        setMode("shell");
        setCommandText(formatArgvForInput(operation.argv));
        setShellPty(Boolean(operation.pty));
        return;
      case "shell_script":
        setMode("shell_script");
        setShellScript(operation.script);
        return;
      case "terminal_open":
        setMode("terminal_session");
        setTerminalSessionId(crypto.randomUUID());
        setTerminalArgv(formatArgvForInput(operation.argv));
        setTerminalCwd(operation.cwd ?? "");
        setTerminalUser(operation.user ?? "");
        setTerminalUserPolicy(operation.user_policy ?? "fail");
        setTerminalCols(operation.cols);
        setTerminalRows(operation.rows);
        setTerminalIdleTimeoutSecs(operation.idle_timeout_secs);
        setTerminalFlowWindowBytes(operation.flow_window_bytes);
        return;
      case "backup":
        setMode("backup");
        setBackupPathsText(operation.paths.join("\n"));
        setBackupIncludeConfig(operation.include_config);
        setBackupFollowSymlinks(operation.follow_symlinks);
        setBackupSkipMissingPaths(operation.missing_path_policy === "skip");
        return;
      case "file_pull":
        setMode("file_pull");
        setFilePath(operation.path);
        setFileFollowSymlinks(operation.follow_symlinks);
        return;
      case "user_sessions":
        setMode("user_sessions");
        return;
      case "process_list":
        setMode("process_list");
        setProcessLimit(operation.limit);
        return;
      case "agent_update":
        setMode("agent_update");
        setUpdateArtifactUrl(operation.artifact_url);
        setUpdateSha256Hex(operation.sha256_hex);
        return;
      case "agent_update_check":
        setMode("agent_update_check");
        setUpdateCheckVersionUrl(
          operation.version_url ?? DEFAULT_UPDATE_VERSION_URL,
        );
        return;
      case "agent_update_activate":
        setMode("agent_update_activate");
        setUpdateActivationSha256Hex(operation.staged_sha256_hex);
        setUpdateRestartAgent(Boolean(operation.restart_agent));
        return;
      case "agent_update_rollback":
        setMode("agent_update_rollback");
        setUpdateRollbackSha256Hex(operation.rollback_sha256_hex ?? "");
        return;
      default:
        setActionError(
          `Template operation ${operation.type} is not editable in this composer yet`,
        );
    }
  }

  function commandTemplateRequest(): UpsertCommandTemplateRequest {
    const name = templateName.trim();
    if (!name) {
      throw new Error("Template name is required");
    }
    const scopeValue =
      templateScopeKind === "global" ? null : templateScopeValue.trim();
    if (templateScopeKind !== "global" && !scopeValue) {
      throw new Error("Template scope value is required");
    }
    const operation = buildOperation(
      mode,
      commandText,
      shellPty,
      shellScript,
      terminalAction,
      terminalSessionId,
      terminalArgv,
      terminalCwd,
      terminalUser,
      terminalUserPolicy,
      terminalCols,
      terminalRows,
      terminalReplayFromSeq,
      terminalIdleTimeoutSecs,
      terminalFlowWindowBytes,
      filePath,
      fileFollowSymlinks,
      processLimit,
      supervisorAction,
      supervisorName,
      supervisorArgv,
      supervisorCwd,
      supervisorEnv,
      supervisorLogBytes,
      updateArtifactUrl,
      updateSha256Hex,
      updateCheckVersionUrl,
      updateActivationSha256Hex,
      updateRestartAgent,
      updateRollbackSha256Hex,
      backupPathsText,
      backupIncludeConfig,
      backupFollowSymlinks,
      backupSkipMissingPaths,
      filePushPath,
      filePushMode,
      null,
    );
    const maxTimeoutOverride = parseOptionalJobMaxTimeoutSecs(maxTimeoutSecs);
    return {
      name,
      scope_kind: templateScopeKind,
      scope_value: scopeValue,
      display_group: displayGroupForOperation(operation),
      operation,
      defaults: {
        confirmed: operationNeedsConfirmation,
        destructive: operationNeedsConfirmation,
        force_unprivileged: supportsForceUnprivileged
          ? forceUnprivileged
          : false,
        ...(maxTimeoutOverride !== undefined
          ? { max_timeout_secs: maxTimeoutOverride }
          : {}),
      },
      confirmed: true,
    };
  }

  async function reviewCommandTemplateSave() {
    await runPanelAction(setTemplatePending, setTemplateError, async () => {
      const request = commandTemplateRequest();
      setTemplateSaveSnapshot({
        request,
        title: selectedTemplate?.built_in
          ? "Save built-in as user template"
          : "Save command template",
      });
      setTemplateConfirmation(
        selectedTemplate?.built_in ? "save-copy" : "save",
      );
    });
  }

  async function saveCommandTemplate() {
    const snapshot = templateSaveSnapshot;
    if (!snapshot) {
      setTemplateError("Review template before saving");
      return;
    }
    await runPanelAction(setTemplatePending, setTemplateError, async () => {
      const saved = await onUpsertCommandTemplate(snapshot.request);
      setSelectedTemplateId(saved.id);
      setTemplateName(saved.name);
      setTemplateScopeKind(
        saved.scope_kind as "global" | "provider" | "tag" | "client",
      );
      setTemplateScopeValue(saved.scope_value ?? "");
      setTemplateConfirmation(null);
      setTemplateSaveSnapshot(null);
    });
  }

  async function deleteSelectedCommandTemplate() {
    if (!deleteTemplateSnapshot || deleteTemplateSnapshot.built_in) {
      return;
    }
    await runPanelAction(setTemplatePending, setTemplateError, async () => {
      await onDeleteCommandTemplate(deleteTemplateSnapshot.id, {
        confirmed: true,
        reviewed_name: deleteTemplateSnapshot.name,
      });
      setSelectedTemplateId("");
      setTemplateConfirmation(null);
      setDeleteTemplateSnapshot(null);
    });
  }

  async function dispatchJobNow() {
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      const confirmed = dispatchConfirmation;
      if (!confirmed?.targets.length) {
        throw new Error(
          "Confirmed target snapshot is missing; review the targets again",
        );
      }
      setLastDispatchContext(confirmed.operationLabel);
      if (confirmed.kind === "transfer_upload") {
        const clientIds = confirmed.targets.map((target) => target.id);
        const commitJob = await runBrowserResumableUpload({
          clientIds,
          confirmed: true,
          createJob: onCreateJob,
          file: confirmed.file,
          loadJob: onLoadJob,
          loadOutputs: onLoadOutputs,
          modeText: confirmed.modeText,
          multiTargetPolicy: confirmed.multiTargetPolicy,
          existingPolicy: confirmed.existingPolicy,
          path: confirmed.path,
          privilegeMaterial: confirmed.privilegeMaterial,
          rateLimitKbps: confirmed.rateLimitKbps,
          chunkSizeBytes: confirmed.chunkSizeBytes,
          resumeToken: confirmed.resumeToken,
          sessionId: confirmed.sessionId,
          maxTimeoutSecs: confirmed.maxTimeoutSecs,
          maxTimeoutOverrideSecs: confirmed.maxTimeoutOverrideSecs,
          onProgress: (progress) => {
            setTransferProgress(visibleTransferProgress(progress));
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastPayloadHash(null);
        await trackDispatchProgress(
          commitJob,
          confirmed.targets,
          confirmed.maxTimeoutSecs,
        );
        setDispatchPromptOpen(false);
        return;
      }
      if (confirmed.kind === "transfer_download") {
        const clientIds = confirmed.targets.map((target) => target.id);
        const startJob = await runBrowserResumableDownload({
          clientIds,
          confirmed: true,
          createJob: onCreateJob,
          downloadName: confirmed.downloadName,
          downloadSink: confirmed.downloadSink,
          downloadOutputChunk: onDownloadOutputChunk,
          followSymlinks: confirmed.followSymlinks,
          loadJob: onLoadJob,
          loadOutputs: onLoadOutputs,
          path: confirmed.path,
          privilegeMaterial: confirmed.privilegeMaterial,
          rateLimitKbps: confirmed.rateLimitKbps,
          chunkSizeBytes: confirmed.chunkSizeBytes,
          resumeToken: confirmed.resumeToken,
          sessionId: confirmed.sessionId,
          maxTimeoutSecs: confirmed.maxTimeoutSecs,
          maxTimeoutOverrideSecs: confirmed.maxTimeoutOverrideSecs,
          onProgress: (progress) => {
            setTransferProgress(visibleTransferProgress(progress));
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastPayloadHash(null);
        await trackDispatchProgress(
          startJob,
          confirmed.targets,
          confirmed.maxTimeoutSecs,
        );
        setDispatchPromptOpen(false);
        return;
      }
      const nextJob = await onCreateJob(
        jobRequestFromConfirmation(confirmed, confirmed.destructive),
      );
      setLastPayloadHash(confirmed.payloadHashHex);
      if (confirmed.rollout) {
        setLastRolloutJobId(nextJob.job_id);
        const targetRecords = await onLoadTargets(nextJob.job_id);
        setLastDispatchProgress(
          buildBulkJobProgress({
            jobId: nextJob.job_id,
            maxTimeoutSecs: confirmed.maxTimeoutSecs,
            targetCount: createJobTargetCount(nextJob),
            targetRecords,
            targets: confirmed.targets,
          }),
        );
        setReviewStatus(
          `Staged rollout accepted with ${confirmed.rollout.canary_client_ids.length} canary and ${confirmed.rollout.batch_size}-VPS batches.`,
        );
      } else {
        await trackDispatchProgress(
          nextJob,
          confirmed.targets,
          confirmed.maxTimeoutSecs,
        );
      }
      setDispatchPromptOpen(false);
    });
  }

  async function requestJobApproval() {
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      if (!onCreateJobApproval) {
        throw new Error("Approval requests are unavailable");
      }
      const confirmed = dispatchConfirmation;
      if (confirmed?.kind !== "job" || !confirmed.targets.length) {
        throw new Error(
          "Confirmed job snapshot is missing; review the targets again",
        );
      }
      const approval = await onCreateJobApproval({
        job: jobRequestFromConfirmation(confirmed, true),
        reason: approvalRequestReason.trim() || null,
      });
      setDispatchPromptOpen(false);
      setDispatchConfirmation(null);
      setApprovalRequestReason("");
      onApprovalRequested?.(approval);
    });
  }

  async function trackDispatchProgress(
    job: CreateJobResponse,
    targets: AgentView[],
    jobMaxTimeoutSecs?: number,
  ) {
    const targetCount = createJobTargetCount(job);
    const boundedJobTimeoutSecs = clampJobMaxTimeoutSecs(
      jobMaxTimeoutSecs ?? effectiveJobMaxTimeoutSecs(maxTimeoutSecs),
    );
    setLastDispatchProgress(null);
    setDispatchProgress(
      buildBulkJobProgress({
        jobId: job.job_id,
        targetCount,
        targetRecords: [],
        targets,
        maxTimeoutSecs: boundedJobTimeoutSecs,
      }),
    );
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        onLoadOutputs,
        onProgress: setDispatchProgress,
        targetCount,
        targets,
        maxTimeoutSecs: boundedJobTimeoutSecs,
      });
      setLastDispatchProgress(result.progress);
    } finally {
      setDispatchProgress(null);
    }
  }

  function targetSelection(): JobTargetSelection {
    return {
      selector_expression: normalizedSelectorExpression,
    };
  }

  return (
    <section
      className={`fleetPanel commandComposer ${terminalSurface ? "terminalCommandComposer" : ""}`.trim()}
    >
      <div className="sectionHeader">
        <div>
          <h2>
            {terminalSurface ? "Terminal review composer" : "Dispatch command"}
          </h2>
          <span>{dispatchHeaderStatus}</span>
        </div>
        <div className="headerActionStack">
          {privilegeMaterial ? (
            <button
              className="secondaryAction"
              onClick={() => setLockPromptOpen(true)}
              type="button"
            >
              <LockKeyhole size={17} />
              Lock
            </button>
          ) : (
            <ShieldCheck size={20} />
          )}
        </div>
      </div>

      <form className="dispatchForm" onSubmit={submitJob}>
        {!terminalSurface && (
          <>
            <div
              className="templateToolbar"
              aria-label="Command template controls"
            >
              <div className="templateToolbarPrimary">
                <label>
                  <span>Template</span>
                  <select
                    aria-label="Template selector"
                    onChange={(event) =>
                      applyCommandTemplate(event.target.value)
                    }
                    value={selectedTemplateId}
                  >
                    <option value="">Select template</option>
                    {builtinTemplates.length > 0 && (
                      <optgroup label="Built-in templates">
                        {builtinTemplates.map((template) => (
                          <option key={template.id} value={template.id}>
                            {template.name}
                          </option>
                        ))}
                      </optgroup>
                    )}
                    {userTemplates.length > 0 && (
                      <>
                        <option disabled value="__user_template_separator">
                          User-defined templates
                        </option>
                        <optgroup label="User-defined templates">
                          {userTemplates.map((template) => (
                            <option key={template.id} value={template.id}>
                              {template.name} · {template.scope_kind}
                              {template.scope_value
                                ? `:${template.scope_value}`
                                : ""}
                            </option>
                          ))}
                        </optgroup>
                      </>
                    )}
                  </select>
                </label>
                <span className="templateToolbarStatus">
                  {selectedTemplate
                    ? `${selectedTemplate.scope_kind}${selectedTemplate.scope_value ? `:${selectedTemplate.scope_value}` : ""}`
                    : "Optional"}
                  {commandTemplatesTruncated
                    ? ` · ${commandTemplates.length} templates loaded; older templates may not appear`
                    : ""}
                </span>
              </div>
              <details className="templateManageDrawer">
                <summary>Manage templates</summary>
                <div className="templateManageGrid">
                  <label>
                    <span>Name</span>
                    <input
                      aria-label="Command template name"
                      onChange={(event) => setTemplateName(event.target.value)}
                      placeholder="provider-health-check"
                      value={templateName}
                    />
                  </label>
                  <label>
                    <span>Scope</span>
                    <select
                      aria-label="Command template scope"
                      onChange={(event) => {
                        setTemplateScopeKind(
                          event.target.value as typeof templateScopeKind,
                        );
                        setTemplateScopeValue("");
                      }}
                      value={templateScopeKind}
                    >
                      <option value="global">Global</option>
                      <option value="provider">Provider</option>
                      <option value="tag">Tag</option>
                      <option value="client">Client</option>
                    </select>
                  </label>
                  {templateScopeKind === "client" ? (
                    <label>
                      <span>Scope VPS</span>
                      <VpsCombobox
                        agents={agents}
                        ariaLabel="Command template scope VPS"
                        onChange={setTemplateScopeValue}
                        placeholder="Search VPS name or ID"
                        value={templateScopeValue}
                      />
                    </label>
                  ) : (
                    <label>
                      <span>Scope value</span>
                      <input
                        aria-label="Command template scope value"
                        disabled={templateScopeKind === "global"}
                        onChange={(event) =>
                          setTemplateScopeValue(event.target.value)
                        }
                        placeholder={templateScopeKind}
                        value={
                          templateScopeKind === "global"
                            ? ""
                            : templateScopeValue
                        }
                      />
                    </label>
                  )}
                  <div className="templateToolbarActions">
                    <button
                      className="secondaryAction"
                      disabled={templatePending}
                      onClick={() => void reviewCommandTemplateSave()}
                      type="button"
                    >
                      {selectedTemplate?.built_in
                        ? "Review copy"
                        : "Review save"}
                    </button>
                    <button
                      className="secondaryAction dangerAction"
                      disabled={
                        templatePending ||
                        !selectedTemplate ||
                        selectedTemplate.built_in
                      }
                      onClick={() => {
                        if (!selectedTemplate || selectedTemplate.built_in) {
                          return;
                        }
                        setTemplateError(null);
                        setDeleteTemplateSnapshot(selectedTemplate);
                        setTemplateConfirmation("delete");
                      }}
                      type="button"
                    >
                      Delete
                    </button>
                  </div>
                </div>
              </details>
              <ActionFeedback
                className="localActionFeedback"
                message={templateError}
                ref={templateFeedbackRef}
                tone="danger"
              />
            </div>
            <ConfirmationPrompt
              confirmLabel={templateSaveSnapshot?.title ?? "Save template"}
              detail={
                templateConfirmation === "save-copy"
                  ? "Creates a user-defined command template. The built-in template remains unchanged."
                  : "Saves the reviewed command template request exactly as shown."
              }
              error={templateError}
              items={[
                {
                  label: "Template",
                  value: templateSaveSnapshot?.request.name ?? "-",
                },
                {
                  label: "Scope",
                  value: templateSaveSnapshot
                    ? templateSaveSnapshot.request.scope_kind === "global"
                      ? "global"
                      : `${templateSaveSnapshot.request.scope_kind}:${templateSaveSnapshot.request.scope_value ?? "-"}`
                    : "-",
                },
                {
                  label: "Operation",
                  value: templateSaveSnapshot?.request.operation.type ?? "-",
                },
              ]}
              onCancel={() => {
                setTemplateConfirmation(null);
                setTemplateSaveSnapshot(null);
              }}
              onConfirm={() => void saveCommandTemplate()}
              open={
                (templateConfirmation === "save-copy" ||
                  templateConfirmation === "save") &&
                templateSaveSnapshot !== null
              }
              pending={templatePending}
              title="Confirm command template save"
            />
            <ConfirmationPrompt
              confirmLabel="Delete template"
              detail="Deletes this user-defined command template. Built-in templates cannot be deleted."
              error={templateError}
              items={[
                {
                  label: "Template",
                  value: deleteTemplateSnapshot?.name ?? "-",
                },
                {
                  label: "Scope",
                  value: deleteTemplateSnapshot
                    ? deleteTemplateSnapshot.scope_value
                      ? `${deleteTemplateSnapshot.scope_kind}:${deleteTemplateSnapshot.scope_value}`
                      : deleteTemplateSnapshot.scope_kind
                    : "-",
                },
              ]}
              onCancel={() => {
                setTemplateConfirmation(null);
                setDeleteTemplateSnapshot(null);
              }}
              onConfirm={() => void deleteSelectedCommandTemplate()}
              open={
                templateConfirmation === "delete" &&
                deleteTemplateSnapshot !== null
              }
              pending={templatePending}
              title="Confirm template delete"
              tone="danger"
            />
          </>
        )}
        {fixedMode ? (
          <div
            className="dispatchModeNotice"
            aria-label="Dispatch mode boundary"
          >
            <strong>{focusedModeBoundary?.label}</strong>
            <span>{focusedModeBoundary?.detail}</span>
            {onOpenJobsDispatch ? (
              <button
                className="secondaryAction compactAction"
                onClick={onOpenJobsDispatch}
                type="button"
              >
                Jobs / Dispatch
              </button>
            ) : null}
          </div>
        ) : (
          <>
            <div
              className="dispatchModeNotice"
              aria-label="Dispatch mode boundary"
            >
              <strong>Advanced dispatch</strong>
              <span>Terminal open and resume start in Remote / Terminal.</span>
              {onOpenRemoteTerminal ? (
                <button
                  className="secondaryAction compactAction"
                  onClick={onOpenRemoteTerminal}
                  type="button"
                >
                  Remote terminal
                </button>
              ) : null}
            </div>
            <OperationModeTabs
              includeTerminal={false}
              mode={mode}
              onModeChange={setMode}
            />
          </>
        )}
        <JobOperationEditor
          commandText={commandText}
          shellPty={shellPty}
          fileFollowSymlinks={fileFollowSymlinks}
          filePath={filePath}
          terminalArgv={terminalArgv}
          terminalCols={terminalCols}
          terminalCwd={terminalCwd}
          terminalUser={terminalUser}
          terminalUserPolicy={terminalUserPolicy}
          terminalFlowWindowBytes={terminalFlowWindowBytes}
          terminalIdleTimeoutSecs={terminalIdleTimeoutSecs}
          terminalReplayFromSeq={terminalReplayFromSeq}
          terminalRows={terminalRows}
          terminalSessionId={terminalSessionId}
          filePushMode={filePushMode}
          filePushPath={filePushPath}
          filePushSource={filePushSource}
          fileTransferDownloadSink={fileTransferDownloadSink}
          fileTransferDownloadName={fileTransferDownloadName}
          fileTransferChunkSize={fileTransferChunkSize}
          fileTransferExistingPolicy={fileTransferExistingPolicy}
          fileTransferMultiTargetPolicy={fileTransferMultiTargetPolicy}
          fileTransferSourceArtifactId={fileTransferSourceArtifactId}
          fileTransferSources={fileTransferSources}
          fileTransferSourcesTruncated={fileTransferSourcesTruncated}
          fileTransferUploadSourceKind={fileTransferUploadSourceKind}
          fileTransferRateLimit={fileTransferRateLimit}
          fileTransferResumeToken={fileTransferResumeToken}
          fileTransferSessionId={fileTransferSessionId}
          mode={mode}
          processLimit={processLimit}
          setCommandText={setCommandText}
          setShellPty={setShellPty}
          setShellScript={setShellScript}
          setTerminalArgv={setTerminalArgv}
          setTerminalCols={setTerminalCols}
          setTerminalCwd={setTerminalCwd}
          setTerminalUser={setTerminalUser}
          setTerminalUserPolicy={setTerminalUserPolicy}
          setTerminalFlowWindowBytes={setTerminalFlowWindowBytes}
          setTerminalIdleTimeoutSecs={setTerminalIdleTimeoutSecs}
          setTerminalReplayFromSeq={setTerminalReplayFromSeq}
          setTerminalRows={setTerminalRows}
          setTerminalSessionId={setTerminalSessionId}
          setFileFollowSymlinks={setFileFollowSymlinks}
          setFilePath={setFilePath}
          setFilePushMode={setFilePushMode}
          setFilePushPath={setFilePushPath}
          setFilePushSource={setFilePushSource}
          setFileTransferSourceArtifactId={setFileTransferSourceArtifactId}
          setFileTransferUploadSourceKind={setFileTransferUploadSourceKind}
          setFileTransferDownloadSink={setFileTransferDownloadSink}
          setFileTransferDownloadName={setFileTransferDownloadName}
          setFileTransferChunkSize={setFileTransferChunkSize}
          setFileTransferExistingPolicy={setFileTransferExistingPolicy}
          setFileTransferMultiTargetPolicy={setFileTransferMultiTargetPolicy}
          setFileTransferRateLimit={setFileTransferRateLimit}
          setFileTransferResumeToken={setFileTransferResumeToken}
          setFileTransferSessionId={setFileTransferSessionId}
          setProcessLimit={setProcessLimit}
          setSupervisorAction={setSupervisorAction}
          setSupervisorArgv={setSupervisorArgv}
          setSupervisorCwd={setSupervisorCwd}
          setSupervisorEnv={setSupervisorEnv}
          setSupervisorLogBytes={setSupervisorLogBytes}
          setSupervisorName={setSupervisorName}
          setUpdateArtifactUrl={setUpdateArtifactUrl}
          setUpdateCheckVersionUrl={setUpdateCheckVersionUrl}
          setUpdateActivationSha256Hex={setUpdateActivationSha256Hex}
          setUpdateRestartAgent={setUpdateRestartAgent}
          setUpdateRollbackSha256Hex={setUpdateRollbackSha256Hex}
          setUpdateSha256Hex={setUpdateSha256Hex}
          setBackupIncludeConfig={setBackupIncludeConfig}
          setBackupFollowSymlinks={setBackupFollowSymlinks}
          setBackupSkipMissingPaths={setBackupSkipMissingPaths}
          setBackupPathsText={setBackupPathsText}
          supervisorAction={supervisorAction}
          supervisorArgv={supervisorArgv}
          supervisorCwd={supervisorCwd}
          supervisorEnv={supervisorEnv}
          supervisorLogBytes={supervisorLogBytes}
          supervisorName={supervisorName}
          updateArtifactUrl={updateArtifactUrl}
          updateCheckVersionUrl={updateCheckVersionUrl}
          updateActivationSha256Hex={updateActivationSha256Hex}
          updateRestartAgent={updateRestartAgent}
          updateRollbackSha256Hex={updateRollbackSha256Hex}
          updateSha256Hex={updateSha256Hex}
          backupIncludeConfig={backupIncludeConfig}
          backupFollowSymlinks={backupFollowSymlinks}
          backupSkipMissingPaths={backupSkipMissingPaths}
          backupPathsText={backupPathsText}
          shellScript={shellScript}
        />
        <JobTargetSelector
          agents={agents}
          selectorExpression={selectorExpression}
          setSelectorExpression={(value) => {
            setSelectorExpression(value);
            setPreview(null);
            setDispatchConfirmation(null);
          }}
          verification={selectorVerification}
          verificationMessage={selectorVerificationMessage}
        />
        <TargetImpactPreview
          forceUnprivileged={
            supportsForceUnprivileged ? forceUnprivileged : false
          }
          mode={impactMode}
          targets={impactTargets}
        />
        <details className="dispatchExecutionOptions">
          <summary>Execution options</summary>
          <div className="dispatchExecutionOptionsGrid">
            {supportsForceUnprivileged && (
              <label className="checkLine">
                <input
                  aria-label="Force unprivileged job best effort"
                  checked={forceUnprivileged}
                  onChange={(event) =>
                    setForceUnprivileged(event.target.checked)
                  }
                  type="checkbox"
                />
                <span>Force unprivileged best effort</span>
              </label>
            )}
            <DispatchOptions
              setMaxTimeoutSecs={setMaxTimeoutSecs}
              maxTimeoutSecs={maxTimeoutSecs}
            />
            {!terminalSurface && !fixedMode && (
              <div className="dispatchOptionNote rolloutControls">
                <label
                  className="checkLine"
                  title={
                    rolloutUnavailableReason ??
                    "Release one reviewed canary before bounded fleet batches"
                  }
                >
                  <input
                    checked={rolloutEnabled}
                    disabled={Boolean(rolloutUnavailableReason)}
                    onChange={(event) =>
                      setRolloutEnabled(event.target.checked)
                    }
                    type="checkbox"
                  />
                  <span>Staged rollout</span>
                </label>
                {rolloutEnabled && !rolloutUnavailableReason ? (
                  <div className="rolloutControlGrid">
                    <label>
                      <span>Canary VPS</span>
                      <VpsCombobox
                        agents={impactTargets}
                        ariaLabel="Rollout canary VPS"
                        onChange={setRolloutCanaryClientId}
                        placeholder="Search resolved VPSs"
                        value={rolloutCanaryClientId}
                      />
                    </label>
                    <RolloutNumberField
                      label="Batch size"
                      max={100}
                      min={1}
                      onChange={setRolloutBatchSize}
                      value={rolloutBatchSize}
                    />
                    <RolloutNumberField
                      label="Tolerated failures"
                      max={100}
                      min={0}
                      onChange={setRolloutMaxFailures}
                      value={rolloutMaxFailures}
                    />
                    <RolloutNumberField
                      label="Stage delay (seconds)"
                      max={86_400}
                      min={0}
                      onChange={setRolloutBatchDelaySecs}
                      value={rolloutBatchDelaySecs}
                    />
                    <label className="checkLine">
                      <input
                        checked={rolloutPauseAfterCanary}
                        onChange={(event) =>
                          setRolloutPauseAfterCanary(event.target.checked)
                        }
                        type="checkbox"
                      />
                      <span>Pause after canary</span>
                    </label>
                  </div>
                ) : (
                  <span>
                    {rolloutUnavailableReason ??
                      "Normal dispatch releases all resolved targets through the dispatcher."}
                  </span>
                )}
              </div>
            )}
          </div>
        </details>

        <ConfirmationPrompt
          confirmLabel={
            dispatchReviewIntent === "approval"
              ? "Request approval"
              : "Dispatch job"
          }
          detail={
            dispatchReviewIntent === "approval"
              ? `Queues ${dispatchConfirmationOperationLabel} on ${vpsCountLabel(dispatchConfirmationTargets.length)} for review. Nothing runs until a pending request is approved.`
              : `${dispatchConfirmationOperationLabel} on ${vpsCountLabel(dispatchConfirmationTargets.length)}.`
          }
          error={actionError}
          items={dispatchConfirmationItems}
          onCancel={clearDispatchReview}
          onConfirm={() =>
            void (dispatchReviewIntent === "approval"
              ? requestJobApproval()
              : dispatchJobNow())
          }
          open={dispatchPromptOpen}
          pending={pending}
          title={
            dispatchReviewIntent === "approval"
              ? "Confirm approval request"
              : "Confirm job dispatch"
          }
          tone={
            dispatchReviewIntent === "dispatch" &&
            dispatchConfirmationDestructive
              ? "danger"
              : "normal"
          }
        >
          {dispatchReviewIntent === "approval" ? (
            <label className="confirmationTypedInput">
              <span>Request reason (optional)</span>
              <textarea
                aria-label="Approval request reason"
                disabled={pending}
                maxLength={1024}
                onChange={(event) =>
                  setApprovalRequestReason(event.target.value)
                }
                placeholder="Maintenance window, incident, or change reference"
                rows={3}
                value={approvalRequestReason}
              />
            </label>
          ) : null}
        </ConfirmationPrompt>

        {!dispatchPromptOpen && visibleDispatchProgress && (
          <ExecutionResultPanel
            context={
              lastDispatchContext
                ? `Dispatch: ${lastDispatchContext}`
                : undefined
            }
            loading={dispatchProgress !== null}
            onClearResults={clearExecutionResults}
            onOpenJobDetails={onOpenJobDetails}
            progress={visibleDispatchProgress}
          />
        )}

        {!dispatchPromptOpen && lastRolloutJobId && onOpenRollout && (
          <div className="dispatchResultActions">
            <button
              className="secondaryAction compactAction"
              onClick={() => onOpenRollout(lastRolloutJobId)}
              type="button"
            >
              <ShieldCheck size={14} />
              <span>Open staged rollout</span>
            </button>
          </div>
        )}

        {!dispatchPromptOpen && (
          <ActionFeedback
            className="localActionFeedback"
            message={dispatchFeedbackMessage}
            tone={dispatchFeedbackTone}
          />
        )}
        {!dispatchPromptOpen && (
          <div className="dispatchActions">
            {approvalRequestSupported ? (
              <button
                className="secondaryAction"
                disabled={pending || !operationReady || !privilegeMaterial}
                onClick={() => void prepareJobReview("approval")}
                title="Queue the frozen job request for approval without dispatching it"
                type="button"
              >
                <ShieldCheck size={17} />
                Request approval
              </button>
            ) : null}
            <button
              className="primaryAction"
              disabled={pending || !operationReady || !privilegeMaterial}
              type="submit"
            >
              <Play size={17} />
              Dispatch
            </button>
          </div>
        )}
        {transferProgress && (
          <div
            className="transferProgress"
            aria-label={
              transferProgress.event === "downloaded"
                ? "Resumable download progress"
                : "Resumable upload progress"
            }
          >
            <strong>
              {transferProgress.event === "downloaded"
                ? "Download complete"
                : transferProgress.event === "committed"
                  ? "Upload complete"
                  : "Transfer in progress"}
            </strong>
            <span>
              {transferProgress.nextOffset}/{transferProgress.sizeBytes} bytes ·
              session {shortId(transferProgress.sessionId)}
              {"multiTargetPolicy" in transferProgress
                ? ` · ${transferProgress.multiTargetPolicy}`
                : ""}
              {"downloadSink" in transferProgress
                ? ` · ${transferProgress.downloadSink}`
                : ""}
            </span>
          </div>
        )}
      </form>

      {!privilegeMaterial && (
        <PrivilegeVaultBox
          lastPayloadHash={lastPayloadHash}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={setPrivilegeMaterial}
          privilegeMaterial={privilegeMaterial}
        />
      )}
      <PrivilegeLockPrompt
        error={actionError}
        onCancel={() => setLockPromptOpen(false)}
        onConfirm={() => void lockPrivilege()}
        open={lockPromptOpen}
        pending={lockPending}
      />
    </section>
  );
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function dispatchTargetIdentitySummary(targets: AgentView[]): {
  full: string;
  visible: string;
} {
  if (targets.length === 0) {
    return { full: "No VPS resolved", visible: "No VPS resolved" };
  }
  const labels = targets.map(
    (target) => `${target.display_name.trim() || "Unnamed VPS"} (${target.id})`,
  );
  const visibleLimit = 8;
  return {
    full: labels.join(", "),
    visible:
      labels.length > visibleLimit
        ? `${labels.slice(0, visibleLimit).join(", ")} +${labels.length - visibleLimit} more`
        : labels.join(", "),
  };
}

function operationReviewItems(
  operation: CreateJobRequest["operation"] | undefined,
): Array<{ label: string; title?: string; value: string }> {
  if (!operation) {
    return [];
  }
  if (operation.type === "agent_update_check") {
    return [
      { label: "Effect", value: "Check and stage verified artifact only" },
      { label: "Activation", value: "No" },
      { label: "Agent restart", value: "No" },
      { label: "Manifest", value: operation.version_url ?? "Agent default" },
    ];
  }
  if (operation.type === "agent_update_activate") {
    return [
      { label: "Staged SHA-256", value: operation.staged_sha256_hex },
      { label: "Agent restart", value: operation.restart_agent ? "Yes" : "No" },
    ];
  }
  if (operation.type === "agent_update_rollback") {
    return [
      {
        label: "Rollback artifact",
        value:
          operation.rollback_sha256_hex ?? "Agent-managed previous artifact",
      },
    ];
  }
  if (operation.type.startsWith("terminal_")) {
    return [
      {
        label: "Session",
        value:
          "session_id" in operation ? operation.session_id : "Not reported",
      },
      { label: "Effect", value: jobOperationLabel(operation, operation.type) },
    ];
  }
  if (!operation.type.startsWith("process_")) {
    return [];
  }
  const processName =
    "name" in operation &&
    typeof operation.name === "string" &&
    operation.name.trim()
      ? operation.name
      : "All supervised processes";
  const effectByType: Record<string, string> = {
    process_logs: "Read retained stdout/stderr logs",
    process_restart: "Restart supervised process",
    process_start: "Start supervised process",
    process_status: "Refresh process status",
    process_stop: "Stop supervised process",
  };
  const items: Array<{ label: string; title?: string; value: string }> = [
    { label: "Process", value: processName },
    { label: "Effect", value: effectByType[operation.type] ?? operation.type },
  ];
  if (operation.type === "process_start") {
    const command = formatArgvForInput(operation.argv);
    const environmentNames = Object.keys(operation.env).sort();
    const environment =
      environmentNames.length > 0
        ? `${environmentNames.join(", ")} (values hidden)`
        : "No overrides";
    items.push(
      { label: "Command argv", title: command, value: command },
      {
        label: "Working directory",
        title: operation.cwd ?? "Agent default",
        value: operation.cwd ?? "Agent default",
      },
      { label: "Environment", title: environment, value: environment },
    );
  }
  if (operation.type === "process_logs") {
    items.push({ label: "Log bytes", value: String(operation.max_bytes) });
  }
  return items;
}

function transferReviewItems(snapshot: DispatchConfirmationSnapshot | null) {
  if (snapshot?.kind === "transfer_upload") {
    return [
      { label: "Local source", value: snapshot.file?.name ?? "Not selected" },
      {
        label: "Source size",
        value: snapshot.file
          ? formatTransferBytes(snapshot.file.size)
          : "Not reported",
      },
      {
        label: "Source SHA-256",
        title: snapshot.fileSha256Hex,
        value: `${snapshot.fileSha256Hex.slice(0, 12)}...${snapshot.fileSha256Hex.slice(-8)}`,
      },
      { label: "Destination", title: snapshot.path, value: snapshot.path },
      {
        label: "Existing path",
        value:
          snapshot.existingPolicy === "replace"
            ? "Replace the existing file"
            : "Skip upload if the file already exists",
      },
      { label: "File mode", value: snapshot.modeText },
      {
        label: "Multi-VPS resume",
        value:
          snapshot.multiTargetPolicy === "same-offset"
            ? "Shared offset across all targets"
            : "Independent offset per target",
      },
      {
        label: "Transfer limits",
        value: `${formatTransferBytes(snapshot.chunkSizeBytes)} chunks · ${formatTransferRate(snapshot.rateLimitKbps)}`,
      },
    ];
  }
  if (snapshot?.kind === "transfer_download") {
    return [
      { label: "Remote source", title: snapshot.path, value: snapshot.path },
      {
        label: "Local file",
        title: snapshot.downloadName,
        value: snapshot.downloadName,
      },
      { label: "Save method", value: snapshot.downloadSink },
      {
        label: "Transfer limits",
        value: `${formatTransferBytes(snapshot.chunkSizeBytes)} chunks · ${formatTransferRate(snapshot.rateLimitKbps)}`,
      },
    ];
  }
  return [];
}

function rolloutReviewItems(snapshot: DispatchConfirmationSnapshot | null) {
  if (snapshot?.kind !== "job") return [];
  if (!snapshot.rollout) {
    return [{ label: "Delivery", value: "Standard dispatcher" }];
  }
  const canaryId = snapshot.rollout.canary_client_ids[0] ?? "";
  const canary = snapshot.targets.find((target) => target.id === canaryId);
  return [
    { label: "Delivery", value: "Staged rollout" },
    {
      label: "Canary",
      title: canaryId,
      value: canary?.display_name || canaryId,
    },
    {
      label: "Batches",
      value: `${snapshot.rollout.batch_size} VPS · ${snapshot.rollout.batch_delay_secs}s delay`,
    },
    {
      label: "Safety pause",
      value: `${snapshot.rollout.max_failures} failures tolerated · ${snapshot.rollout.pause_after_canary ? "pause after canary" : "continue after canary"}`,
    },
  ];
}

function rolloutUnsupportedReason(
  terminalSurface: boolean,
  fixedMode: DispatchMode | undefined,
  mode: DispatchMode,
): string | null {
  if (terminalSurface || fixedMode) {
    return "Staged rollout is available from Jobs / Dispatch.";
  }
  if (mode === "file_transfer_upload" || mode === "file_transfer_download") {
    return "Browser-managed resumable transfers do not support staged rollout.";
  }
  return null;
}

function parseRolloutInteger(
  value: string,
  label: string,
  min: number,
  max: number,
) {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) {
    throw new Error(`${label} must be between ${min} and ${max}.`);
  }
  return parsed;
}

function RolloutNumberField({
  label,
  max,
  min,
  onChange,
  value,
}: {
  label: string;
  max: number;
  min: number;
  onChange: (value: string) => void;
  value: string;
}) {
  return (
    <label>
      <span>{label}</span>
      <input
        aria-label={label}
        inputMode="numeric"
        max={max}
        min={min}
        onChange={(event) => onChange(event.target.value)}
        step={1}
        type="number"
        value={value}
      />
    </label>
  );
}

function formatTransferBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${Math.round(value / 1024)} KiB`;
  }
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}

function formatTransferRate(value: number): string {
  return value > 0 ? `${value} KiB/s cap` : "no rate cap";
}

function fixedModeBoundaryCopy(mode: DispatchMode): {
  detail: string;
  label: string;
} {
  if (mode === "terminal_session") {
    return {
      detail:
        "This focused composer controls one terminal workflow from Remote / Terminal. Other operations remain in Jobs / Dispatch.",
      label: "Terminal session mode",
    };
  }
  if (mode === "file_transfer_upload" || mode === "file_transfer_download") {
    return {
      detail:
        "This focused composer reviews one resumable transfer from Remote / Transfers. Other operations remain in Jobs / Dispatch.",
      label: "File transfer mode",
    };
  }
  if (mode === "process_supervisor" || mode === "process_list") {
    return {
      detail:
        "This focused composer reviews one process operation from Remote / Processes. Other operations remain in Jobs / Dispatch.",
      label: "Process operation mode",
    };
  }
  return {
    detail:
      "This focused composer keeps the selected operation fixed. Other operations remain in Jobs / Dispatch.",
    label: "Focused operation mode",
  };
}

function jobOperationLabel(
  operation: NonNullable<CreateJobRequest["operation"]>,
  fallback: string,
): string {
  const labels: Partial<
    Record<NonNullable<CreateJobRequest["operation"]>["type"], string>
  > = {
    agent_update_activate: "Activate staged agent update",
    agent_update_check: "Check agent update",
    agent_update_rollback: "Rollback agent update",
    backup: "Run backup",
    terminal_open: "Open terminal session",
  };
  return labels[operation.type] ?? fallback;
}

function generatedConfirmationRequiredForMode(
  mode: DispatchMode,
  supervisorAction: SupervisorAction,
): boolean {
  const operationType =
    mode === "terminal_session"
      ? "terminal_open"
      : mode === "file_transfer_upload"
        ? "file_transfer_start"
        : mode === "file_transfer_download"
          ? "file_transfer_download_start"
          : mode === "process_supervisor"
            ? supervisorAction === "start"
              ? "process_start"
              : supervisorAction === "stop"
                ? "process_stop"
                : supervisorAction === "restart"
                  ? "process_restart"
                  : supervisorAction === "logs"
                    ? "process_logs"
                    : "process_status"
            : mode;
  return JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE[operationType];
}

function operationUsesDangerTone(operation: JobOperation | undefined): boolean {
  if (!operation) {
    return false;
  }

  switch (operation.type) {
    case "agent_update_activate":
    case "agent_update_rollback":
    case "file_delete":
    case "network_routing_apply":
    case "restore_rollback":
      return true;
    case "agent_update_check":
      return Boolean(operation.activate || operation.restart_agent);
    case "file_copy":
    case "file_rename":
      return Boolean(operation.overwrite);
    case "file_push":
    case "file_push_chunked":
    case "file_transfer_start":
      return operation.existing_policy === "replace";
    case "restore":
      return !operation.dry_run;
    default:
      return false;
  }
}

function blurActiveElement() {
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.dispatchEvent(
      new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }),
    );
    document.activeElement.blur();
  }
}
