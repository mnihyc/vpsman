import { useEffect, useLayoutEffect, useMemo, useState, type FormEvent } from "react";
import { CheckCircle2, LockKeyhole, Play, ShieldCheck } from "lucide-react";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { ActionFeedback } from "../components/ActionFeedback";
import { FILE_TRANSFER_CHUNK_BYTES, readFilePushPayload, sha256Hex } from "../fileTransfer";
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
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
import {
  buildPrivilegeAssertion,
  canonicalTerminalInputPrivilegeIntent,
  canonicalJobPrivilegeIntent,
  operationPayloadHashHex,
  parseCommandArgv,
  textPayloadHashHex,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../privilege";
import { DEFAULT_JOB_BACKUP_PATHS, DEFAULT_TERMINAL_ARGV } from "../presets/jobOperationPresets";
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
  JobTargetRecord,
  JobTargetSelection,
  UpsertCommandTemplateRequest,
} from "../types";
import type { FileTransferSourceArtifactRecord } from "../typesFileTransfer";
import type {
  TerminalInputSubmitRequest,
  TerminalInputSubmitResponse,
  TerminalSessionRecord,
} from "../typesTerminal";
import { runPanelAction, shortId } from "../utils";
import { DispatchOptions, JobTargetSelector } from "./JobDispatchControls";
import { JobOperationEditor, OperationModeTabs } from "./jobs/JobOperationControls";
import { agentsMatchingExpression, parseSearchExpression } from "../searchExpression";
import { TargetImpactPreview, targetImpactModeForDispatch } from "./TargetImpactPreview";

const JOB_SELECTOR_STORAGE_KEY = "vpsman.jobDispatch.selectorExpression";

export type TerminalComposerAction = {
  action: TerminalAction;
  maxTimeoutSecs?: number;
  requestId: string;
  session: TerminalSessionRecord;
  terminalReplayFromSeq?: string;
  terminalUser?: string;
  terminalUserPolicy?: "fail" | "fallback";
};

function formatArgvForInput(argv: string[]): string {
  return argv.map(shellQuoteArg).join(" ");
}

function shellQuoteArg(value: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, `'\\''`)}'`;
}

function commandTypeForApi(operation: CreateJobRequest["operation"]): GeneratedJobCommandType {
  if (!operation) {
    return "shell_argv";
  }
  if (operation.type === "shell") {
    return operation.pty ? "shell_pty" : "shell_argv";
  }
  return JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
}

function displayGroupForOperation(operation: CreateJobRequest["operation"]): string | null {
  if (!operation) {
    return JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE.shell_argv;
  }
  return JOB_COMMAND_DISPLAY_GROUP_BY_COMMAND_TYPE[commandTypeForApi(operation)] ?? null;
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
      kind: "terminal_input";
      clientId: string;
      jobId: string;
      payloadHashHex: string;
      privilegeAssertion: PrivilegeAssertion;
      sessionId: string;
      text: string;
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
  };
}

async function loadUploadSourceArtifactFile(
  sources: FileTransferSourceArtifactRecord[],
  sourceArtifactId: string,
  downloadSource: (downloadPath: string) => Promise<Blob>,
): Promise<File> {
  const artifact = sources.find((source) => source.id === sourceArtifactId);
  if (!artifact) {
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

export function JobDispatchPanel({
  agents,
  fileTransferSources,
  commandTemplates,
  dispatchPreset,
  fixedMode,
  surface = "jobs",
  terminalComposerAction,
  onDispatchPresetApplied,
  onCreateJob,
  onCreateJobApproval,
  onDownloadFileTransferSource,
  onDownloadOutputChunk,
  onOpenJobsDispatch,
  onOpenRemoteTerminal,
  onLoadJob,
  onLoadOutputs,
  onLoadTargets,
  onSubmitTerminalInput,
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
  commandTemplates: CommandTemplateRecord[];
  dispatchPreset?: JobDispatchPreset | null;
  fixedMode?: DispatchMode;
  surface?: "jobs" | "terminal";
  terminalComposerAction?: TerminalComposerAction | null;
  onDispatchPresetApplied?: () => void;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateJobApproval?: (
    request: CreateJobApprovalRequest,
  ) => Promise<JobApprovalRecord>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onDownloadOutputChunk: (jobId: string, clientId: string, seq: number) => Promise<Blob>;
  onOpenJobsDispatch?: () => void;
  onOpenRemoteTerminal?: () => void;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onSubmitTerminalInput: (
    clientId: string,
    sessionId: string,
    request: TerminalInputSubmitRequest,
  ) => Promise<TerminalInputSubmitResponse>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onApprovalRequested?: (approval: JobApprovalRecord) => void;
  onResolveTargets: (selection: JobTargetSelection) => Promise<BulkResolveResponse>;
  onDeleteCommandTemplate: (
    templateId: string,
    request: DeleteCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  onUpsertCommandTemplate: (request: UpsertCommandTemplateRequest) => Promise<CommandTemplateRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [mode, setModeState] = useState<DispatchMode>(fixedMode ?? "shell");
  const [commandText, setCommandText] = useState("");
  const [shellPty, setShellPty] = useState(false);
  const [shellScript, setShellScript] = useState("");
  const [terminalAction, setTerminalAction] = useState<TerminalAction>("open");
  const [terminalSessionId, setTerminalSessionId] = useState<string>(() => crypto.randomUUID());
  const [terminalArgv, setTerminalArgv] = useState(DEFAULT_TERMINAL_ARGV);
  const [terminalCwd, setTerminalCwd] = useState("");
  const [terminalUser, setTerminalUser] = useState("");
  const [terminalUserPolicy, setTerminalUserPolicy] = useState<"fail" | "fallback">("fail");
  const [terminalCols, setTerminalCols] = useState(120);
  const [terminalRows, setTerminalRows] = useState(40);
  const [terminalReplayFromSeq, setTerminalReplayFromSeq] = useState("");
  const [terminalIdleTimeoutSecs, setTerminalIdleTimeoutSecs] = useState(3600);
  const [terminalFlowWindowBytes, setTerminalFlowWindowBytes] = useState(65536);
  const [terminalInputText, setTerminalInputText] = useState("");
  const [terminalCloseReason, setTerminalCloseReason] = useState("");
  const [filePath, setFilePath] = useState("");
  const [fileFollowSymlinks, setFileFollowSymlinks] = useState(false);
  const [filePushPath, setFilePushPath] = useState("");
  const [filePushMode, setFilePushMode] = useState("0644");
  const [filePushSource, setFilePushSource] = useState<File | null>(null);
  const [fileTransferUploadSourceKind, setFileTransferUploadSourceKind] = useState<"local-file" | "source-artifact">(
    "local-file",
  );
  const [fileTransferSourceArtifactId, setFileTransferSourceArtifactId] = useState("");
  const [fileTransferSessionId, setFileTransferSessionId] = useState("");
  const [fileTransferResumeToken, setFileTransferResumeToken] = useState("");
  const [fileTransferDownloadName, setFileTransferDownloadName] = useState("");
  const [fileTransferDownloadSink, setFileTransferDownloadSink] = useState<BrowserDownloadSinkMode>("browser-download");
  const [fileTransferChunkSize, setFileTransferChunkSize] = useState(65536);
  const [fileTransferRateLimit, setFileTransferRateLimit] = useState(0);
  const [fileTransferExistingPolicy, setFileTransferExistingPolicy] = useState<FileExistingPolicy>("skip");
  const [fileTransferMultiTargetPolicy, setFileTransferMultiTargetPolicy] =
    useState<BrowserTransferMultiTargetPolicy>("same-offset");
  const [selectedTemplateId, setSelectedTemplateId] = useState("");
  const [templateName, setTemplateName] = useState("");
  const [templateScopeKind, setTemplateScopeKind] = useState<"global" | "provider" | "tag" | "client">("global");
  const [templateScopeValue, setTemplateScopeValue] = useState("");
  const [templatePending, setTemplatePending] = useState(false);
  const [templateConfirmation, setTemplateConfirmation] = useState<"save" | "save-copy" | "delete" | null>(null);
  const [templateSaveSnapshot, setTemplateSaveSnapshot] = useState<{
    request: UpsertCommandTemplateRequest;
    title: string;
  } | null>(null);
  const [deleteTemplateSnapshot, setDeleteTemplateSnapshot] =
    useState<CommandTemplateRecord | null>(null);
  const [updateArtifactUrl, setUpdateArtifactUrl] = useState("");
  const [updateSha256Hex, setUpdateSha256Hex] = useState("");
  const [updateCheckVersionUrl, setUpdateCheckVersionUrl] = useState(DEFAULT_UPDATE_VERSION_URL);
  const [updateActivationSha256Hex, setUpdateActivationSha256Hex] = useState("");
  const [updateRestartAgent, setUpdateRestartAgent] = useState(false);
  const [updateRollbackSha256Hex, setUpdateRollbackSha256Hex] = useState("");
  const [backupPathsText, setBackupPathsText] = useState(DEFAULT_JOB_BACKUP_PATHS);
  const [backupIncludeConfig, setBackupIncludeConfig] = useState(true);
  const [backupFollowSymlinks, setBackupFollowSymlinks] = useState(false);
  const [backupSkipMissingPaths, setBackupSkipMissingPaths] = useState(false);
  const [processLimit, setProcessLimit] = useState(50);
  const [supervisorAction, setSupervisorAction] = useState<SupervisorAction>("status");
  const [supervisorName, setSupervisorName] = useState("");
  const [supervisorArgv, setSupervisorArgv] = useState("");
  const [supervisorCwd, setSupervisorCwd] = useState("");
  const [supervisorEnv, setSupervisorEnv] = useState("");
  const [supervisorLogBytes, setSupervisorLogBytes] = useState(65536);
  const [selectorExpression, setSelectorExpression] = useState(() =>
    visibleDispatchSelector(readLocalString(JOB_SELECTOR_STORAGE_KEY)),
  );
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState("");
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [dispatchProgress, setDispatchProgress] = useState<BulkJobProgress | null>(null);
  const [lastDispatchProgress, setLastDispatchProgress] = useState<BulkJobProgress | null>(null);
  const [lastDispatchContext, setLastDispatchContext] = useState<string | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [transferProgress, setTransferProgress] = useState<ResumableUploadProgress | ResumableDownloadProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [dispatchPromptOpen, setDispatchPromptOpen] = useState(false);
  const [dispatchConfirmation, setDispatchConfirmation] = useState<DispatchConfirmationSnapshot | null>(null);
  const [dispatchReviewIntent, setDispatchReviewIntent] = useState<
    "dispatch" | "approval"
  >("dispatch");
  const [approvalRequestReason, setApprovalRequestReason] = useState("");
  const [selectorVerification, setSelectorVerification] = useState<"checking" | "invalid" | "neutral" | "valid">("neutral");
  const [selectorVerificationMessage, setSelectorVerificationMessage] = useState<string | null>(null);
  const [selectorVerificationError, setSelectorVerificationError] =
    useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const normalizedSelectorExpression = normalizedDispatchSelector(selectorExpression);
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
    if (!dispatchPreset) {
      return;
    }
    if (fixedMode && dispatchPreset.mode !== fixedMode) {
      onDispatchPresetApplied?.();
      return;
    }
    setModeState(fixedMode ?? dispatchPreset.mode);
    if (dispatchPreset.selectorExpression !== undefined) {
      setSelectorExpression(visibleDispatchSelector(dispatchPreset.selectorExpression));
    }
    if (dispatchPreset.commandTemplateId) {
      applyCommandTemplate(dispatchPreset.commandTemplateId);
    }
    if (dispatchPreset.maxTimeoutSecs !== undefined) {
      setMaxTimeoutSecs(String(clampJobMaxTimeoutSecs(dispatchPreset.maxTimeoutSecs)));
    } else if (dispatchPreset.mode === "agent_update_activate" || dispatchPreset.mode === "agent_update_rollback") {
      setMaxTimeoutSecs("60");
    } else if (dispatchPreset.mode.startsWith("agent_update")) {
      setMaxTimeoutSecs("300");
    }
    if (dispatchPreset.mode === "agent_update") {
      setUpdateArtifactUrl(dispatchPreset.updateArtifactUrl ?? "");
      setUpdateSha256Hex(dispatchPreset.updateSha256Hex ?? "");
    }
    if (dispatchPreset.mode === "agent_update_check") {
      setUpdateCheckVersionUrl(dispatchPreset.updateCheckVersionUrl ?? DEFAULT_UPDATE_VERSION_URL);
    }
    if (dispatchPreset.mode === "agent_update_activate") {
      setUpdateActivationSha256Hex(dispatchPreset.updateActivationSha256Hex ?? "");
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
        setMaxTimeoutSecs(dispatchPreset.supervisorAction === "logs" ? "30" : "60");
      }
    }
    if (dispatchPreset.mode === "file_transfer_upload") {
      setFilePushPath(dispatchPreset.filePushPath ?? "");
      setFilePushMode(dispatchPreset.filePushMode ?? "0644");
      setFilePushSource(dispatchPreset.fileTransferUploadFile ?? null);
      setFileTransferUploadSourceKind(dispatchPreset.fileTransferUploadSourceKind ?? "local-file");
      setFileTransferSourceArtifactId(dispatchPreset.fileTransferSourceArtifactId ?? "");
      setFileTransferSessionId(dispatchPreset.fileTransferSessionId ?? "");
      setFileTransferResumeToken(dispatchPreset.fileTransferResumeToken ?? "");
      setFileTransferChunkSize(
        clampInteger(dispatchPreset.fileTransferChunkSize ?? 65536, 1, FILE_TRANSFER_CHUNK_BYTES),
      );
      setFileTransferRateLimit(
        clampInteger(dispatchPreset.fileTransferRateLimit ?? 0, 0, MAX_FILE_TRANSFER_RATE_LIMIT_KBPS),
      );
      setFileTransferExistingPolicy(dispatchPreset.fileTransferExistingPolicy ?? "skip");
      setFileTransferMultiTargetPolicy(dispatchPreset.fileTransferMultiTargetPolicy ?? "same-offset");
    }
    if (dispatchPreset.mode === "file_transfer_download") {
      setFilePath(dispatchPreset.filePath ?? "");
      setFileFollowSymlinks(dispatchPreset.fileFollowSymlinks ?? false);
      setFileTransferDownloadName(dispatchPreset.fileTransferDownloadName ?? "");
      setFileTransferDownloadSink(dispatchPreset.fileTransferDownloadSink ?? "browser-download");
      setFileTransferSessionId(dispatchPreset.fileTransferSessionId ?? "");
      setFileTransferResumeToken(dispatchPreset.fileTransferResumeToken ?? "");
      setFileTransferChunkSize(
        clampInteger(dispatchPreset.fileTransferChunkSize ?? 65536, 1, FILE_TRANSFER_CHUNK_BYTES),
      );
      setFileTransferRateLimit(
        clampInteger(dispatchPreset.fileTransferRateLimit ?? 0, 0, MAX_FILE_TRANSFER_RATE_LIMIT_KBPS),
      );
    }
    setPreview(null);
    clearDispatchReview();
    setActionError(null);
    clearExecutionResults();
    onDispatchPresetApplied?.();
  }, [commandTemplates, dispatchPreset, onDispatchPresetApplied]);

  useEffect(() => {
    if (!terminalComposerAction) {
      return;
    }
    const session = terminalComposerAction.session;
    setMode("terminal_session");
    setTerminalAction(terminalComposerAction.action);
    setTerminalSessionId(session.session_id);
    setTerminalArgv(session.argv.length > 0 ? formatArgvForInput(session.argv) : DEFAULT_TERMINAL_ARGV);
    setTerminalCwd(session.cwd ?? "");
    setTerminalUser(terminalComposerAction.terminalUser ?? "");
    setTerminalUserPolicy(terminalComposerAction.terminalUserPolicy ?? "fail");
    setTerminalCols(session.cols ?? 120);
    setTerminalRows(session.rows ?? 40);
    setTerminalIdleTimeoutSecs(session.idle_timeout_secs ?? 3600);
    setTerminalFlowWindowBytes(session.flow_window_bytes ?? 65536);
    setTerminalReplayFromSeq(
      terminalComposerAction.terminalReplayFromSeq !== undefined
        ? terminalComposerAction.terminalReplayFromSeq
        : terminalComposerAction.action === "open" || terminalComposerAction.action === "poll"
        ? String(session.output_retained_first_seq ?? session.output_first_seq ?? 0)
        : "",
    );
    if (terminalComposerAction.maxTimeoutSecs !== undefined) {
      setMaxTimeoutSecs(String(clampJobMaxTimeoutSecs(terminalComposerAction.maxTimeoutSecs)));
    }
    if (terminalComposerAction.action === "input") {
      setMaxTimeoutSecs("30");
    }
    setTerminalInputText("");
    setTerminalCloseReason(session.close_reason ?? "operator");
    setSelectorExpression(`id:${session.client_id}`);
    setPreview(null);
    clearDispatchReview();
    setActionError(null);
  }, [terminalComposerAction]);

  useEffect(() => {
    writeLocalString(JOB_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

  useEffect(() => {
    setTemplateSaveSnapshot(null);
    setDeleteTemplateSnapshot(null);
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
    terminalCloseReason,
    terminalCols,
    terminalCwd,
    terminalFlowWindowBytes,
    terminalIdleTimeoutSecs,
    terminalInputText,
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
          setSelectorVerificationMessage(`${response.target_count}/${agents.length}`);
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
  }, [agents.length, mode, normalizedSelectorExpression, onResolveTargets, selectorParse.error]);

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
    (fileTransferUploadSourceKind === "local-file" ? !!filePushSource : !!fileTransferSourceArtifactId);
  const fileTransferDownloadReady = filePath.startsWith("/");
  const backupReady = backupIncludeConfig || parseBackupPaths(backupPathsText).length > 0;
  const operationReady =
    mode === "shell"
      ? parsedArgv.length > 0
      : mode === "shell_script"
        ? shellScript.trim().length > 0
        : mode === "terminal_session"
          ? terminalReady(terminalAction, terminalSessionId, terminalArgv, terminalInputText)
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
                            ? /^[0-9a-fA-F]{64}$/.test(updateActivationSha256Hex.trim())
                            : mode === "agent_update_rollback"
                              ? (!updateRollbackSha256Hex.trim() ||
                                  /^[0-9a-fA-F]{64}$/.test(updateRollbackSha256Hex.trim()))
                              : mode === "process_supervisor"
                                ? supervisorReady(supervisorAction, supervisorName, supervisorArgv)
                                : mode === "backup"
                                  ? backupReady
                                  : true;
  const expressionTargets = useMemo(
    () => (selectorParse.error ? [] : agentsMatchingExpression(agents, normalizedSelectorExpression)),
    [agents, normalizedSelectorExpression, selectorParse.error],
  );
  const impactMode = targetImpactModeForDispatch(mode);
  const supportsForceUnprivileged = impactMode !== "generic";
  const operationNeedsConfirmation = generatedConfirmationRequiredForMode(mode, supervisorAction, terminalAction);
  const approvalRequestSupported = Boolean(
    !terminalSurface &&
      onCreateJobApproval &&
      mode !== "file_transfer_upload" &&
      mode !== "file_transfer_download",
  );
  const impactTargets = preview?.targets ?? expressionTargets;
  const activeDispatchConfirmation = dispatchPromptOpen ? dispatchConfirmation : null;
  const dispatchConfirmationSelector =
    activeDispatchConfirmation?.selectorExpression ?? normalizedSelectorExpression;
  const dispatchConfirmationTargets =
    activeDispatchConfirmation?.targets ?? preview?.targets ?? expressionTargets;
  const dispatchConfirmationMaxTimeoutSecs =
    activeDispatchConfirmation?.maxTimeoutSecs ?? effectiveJobMaxTimeoutSecs(maxTimeoutSecs);
  const dispatchConfirmationForceUnprivileged =
    activeDispatchConfirmation?.forceUnprivileged ??
    (supportsForceUnprivileged ? forceUnprivileged : false);
  const dispatchConfirmationOperationLabel =
    activeDispatchConfirmation?.operationLabel ?? operationCommandLabel(mode, commandText);
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
  const selectedTemplate = commandTemplates.find((template) => template.id === selectedTemplateId) ?? null;
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
      activeDispatchConfirmation?.kind === "job" ? activeDispatchConfirmation.operation : undefined,
    ),
    ...(dispatchConfirmationFollowSymlinks === null
      ? []
      : [
          {
            label: "Symlinks",
            value: dispatchConfirmationFollowSymlinks ? "Follow targets" : "Do not follow",
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

  function lockPrivilege() {
    setPrivilegeMaterial(null);
    setActionError(null);
    clearDispatchReview();
  }

  function clearExecutionResults() {
    setDispatchProgress(null);
    setLastDispatchProgress(null);
    setLastDispatchContext(null);
    setLastJob(null);
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

  async function previewTargets() {
    if (selectorParse.error) {
      setActionError(selectorParse.error);
      return;
    }
    const reviewGeneration = captureReviewGeneration();
    const selection = targetSelection();
    setReviewStatus("Resolving dispatch targets");
    try {
      await runPanelAction(setPending, setActionError, async () => {
        await waitForReviewRender();
        const resolved = await onResolveTargets(selection);
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        setPreview(resolved);
      });
    } finally {
      if (isReviewGenerationCurrent(reviewGeneration)) {
        setReviewStatus(null);
      }
    }
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
        const snapshot = await buildDispatchConfirmationSnapshot(resolved.targets);
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

  async function buildDispatchConfirmationSnapshot(targets: AgentView[]): Promise<DispatchConfirmationSnapshot> {
    if (!privilegeMaterial) {
      throw new Error("Privilege unlock is locked");
    }
    const selector = normalizedSelectorExpression;
    const maxTimeoutOverride = parseOptionalJobMaxTimeoutSecs(maxTimeoutSecs);
    const maxTimeout = maxTimeoutOverride ?? effectiveJobMaxTimeoutSecs(maxTimeoutSecs);
    const frozenForceUnprivileged = supportsForceUnprivileged ? forceUnprivileged : false;
    const operationLabel = operationCommandLabel(mode, commandText);
    const base = {
      forceUnprivileged: frozenForceUnprivileged,
      operationLabel,
      selectorExpression: selector,
      targets,
      maxTimeoutSecs: maxTimeout,
      maxTimeoutOverrideSecs: maxTimeoutOverride,
    };
    if (mode === "file_transfer_upload") {
      const uploadSourceFile =
        fileTransferUploadSourceKind === "source-artifact"
          ? await loadUploadSourceArtifactFile(
              fileTransferSources,
              fileTransferSourceArtifactId,
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
    if (mode === "terminal_session" && terminalAction === "input") {
      if (targets.length !== 1) {
        throw new Error("Terminal input requires exactly one resolved VPS");
      }
      const sessionId = terminalSessionId.trim();
      if (!/^[0-9a-fA-F-]{36}$/.test(sessionId)) {
        throw new Error("Terminal session id must be a UUID");
      }
      if (!terminalInputText) {
        throw new Error("Terminal input is empty");
      }
      const clientId = targets[0].id;
      const payloadHashHex = await textPayloadHashHex(terminalInputText);
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalTerminalInputPrivilegeIntent({
          clientId,
          sessionId,
          inputPayloadHash: payloadHashHex,
          maxTimeoutSecs: maxTimeout,
          confirmed: true,
        }),
        privilegeMaterial,
      });
      return {
        ...base,
        clientId,
        jobId: crypto.randomUUID(),
        kind: "terminal_input",
        operationLabel: "Send terminal input",
        payloadHashHex,
        privilegeAssertion,
        sessionId,
        text: terminalInputText,
      };
    }
    const filePushPayload = mode === "file_push" ? await readFilePushPayload(filePushSource) : null;
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
      terminalInputText,
      terminalCloseReason,
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
    const privilegeAssertion = await buildPrivilegeAssertion({
      intent: canonicalJobPrivilegeIntent({
        selectorExpression: selector,
        commandType,
        operationPayloadHash: payloadHashHex,
        resolvedTargets: clientIds,
        maxTimeoutSecs: maxTimeout,
        forceUnprivileged: frozenForceUnprivileged,
        privileged: true,
      }),
      privilegeMaterial,
    });
    return {
      ...base,
      argv: mode === "shell" && operation.type === "shell" ? operation.argv : [],
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

  function applyCommandTemplate(templateId: string) {
    setSelectedTemplateId(templateId);
    const template = commandTemplates.find((candidate) => candidate.id === templateId);
    if (!template) {
      return;
    }
    if (!terminalSurface && template.operation.type === "terminal_open") {
      setSelectedTemplateId("");
      setActionError("Open terminal sessions from Remote / Terminal. Jobs / Dispatch stays focused on generic command, file, backup, update, session, and process dispatch.");
      return;
    }
    applyTemplateOperation(template.operation);
    applyTemplateDefaults(template.defaults);
    setTemplateName(template.built_in ? `${template.name} copy` : template.name);
    setTemplateScopeKind(template.scope_kind as "global" | "provider" | "tag" | "client");
    setTemplateScopeValue(template.scope_value ?? "");
    setTemplateConfirmation(null);
    setActionError(null);
  }

  function applyTemplateDefaults(defaults: CommandTemplateRecord["defaults"]) {
    if (!defaults || typeof defaults !== "object" || Array.isArray(defaults)) {
      return;
    }
    if (typeof defaults.max_timeout_secs === "number") {
      setMaxTimeoutSecs(String(clampJobMaxTimeoutSecs(defaults.max_timeout_secs)));
    }
    if (typeof defaults.force_unprivileged === "boolean") {
      setForceUnprivileged(defaults.force_unprivileged);
    }
  }

  function applyTemplateOperation(operation: CommandTemplateRecord["operation"]) {
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
        setTerminalAction("open");
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
        setUpdateCheckVersionUrl(operation.version_url ?? DEFAULT_UPDATE_VERSION_URL);
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
        setActionError(`Template operation ${operation.type} is not editable in this composer yet`);
    }
  }

  function commandTemplateRequest(): UpsertCommandTemplateRequest {
    const name = templateName.trim();
    if (!name) {
      throw new Error("Template name is required");
    }
    const scopeValue = templateScopeKind === "global" ? null : templateScopeValue.trim();
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
      terminalInputText,
      terminalCloseReason,
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
        force_unprivileged: supportsForceUnprivileged ? forceUnprivileged : false,
        ...(maxTimeoutOverride !== undefined ? { max_timeout_secs: maxTimeoutOverride } : {}),
      },
      confirmed: true,
    };
  }

  async function reviewCommandTemplateSave() {
    await runPanelAction(setTemplatePending, setActionError, async () => {
      const request = commandTemplateRequest();
      setTemplateSaveSnapshot({
        request,
        title: selectedTemplate?.built_in
          ? "Save built-in as user template"
          : "Save command template",
      });
      setTemplateConfirmation(selectedTemplate?.built_in ? "save-copy" : "save");
    });
  }

  async function saveCommandTemplate() {
    const snapshot = templateSaveSnapshot;
    if (!snapshot) {
      setActionError("Review template before saving");
      return;
    }
    await runPanelAction(setTemplatePending, setActionError, async () => {
      const saved = await onUpsertCommandTemplate(snapshot.request);
      setSelectedTemplateId(saved.id);
      setTemplateName(saved.name);
      setTemplateScopeKind(saved.scope_kind as "global" | "provider" | "tag" | "client");
      setTemplateScopeValue(saved.scope_value ?? "");
      setTemplateConfirmation(null);
      setTemplateSaveSnapshot(null);
    });
  }

  async function deleteSelectedCommandTemplate() {
    if (!deleteTemplateSnapshot || deleteTemplateSnapshot.built_in) {
      return;
    }
    await runPanelAction(setTemplatePending, setActionError, async () => {
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
    setDispatchPromptOpen(false);
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      const confirmed = dispatchConfirmation;
      if (!confirmed?.targets.length) {
        throw new Error("Confirmed target snapshot is missing; review the targets again");
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
            setTransferProgress(progress);
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastJob(commitJob);
        setLastPayloadHash(null);
        await trackDispatchProgress(commitJob, confirmed.targets, confirmed.maxTimeoutSecs);
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
            setTransferProgress(progress);
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastJob(startJob);
        setLastPayloadHash(null);
        await trackDispatchProgress(startJob, confirmed.targets, confirmed.maxTimeoutSecs);
        return;
      }
      if (confirmed.kind === "terminal_input") {
        const response = await onSubmitTerminalInput(confirmed.clientId, confirmed.sessionId, {
          job_id: confirmed.jobId,
          text: confirmed.text,
          max_timeout_secs: confirmed.maxTimeoutSecs,
          confirmed: true,
          privilege_assertion: confirmed.privilegeAssertion,
        });
        setLastJob(response.job);
        setLastPayloadHash(confirmed.payloadHashHex);
        await trackDispatchProgress(response.job, confirmed.targets, confirmed.maxTimeoutSecs);
        return;
      }
      const nextJob = await onCreateJob(
        jobRequestFromConfirmation(confirmed, confirmed.destructive),
      );
      setLastJob(nextJob);
      setLastPayloadHash(confirmed.payloadHashHex);
      await trackDispatchProgress(nextJob, confirmed.targets, confirmed.maxTimeoutSecs);
    });
  }

  async function requestJobApproval() {
    setDispatchPromptOpen(false);
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
      setDispatchConfirmation(null);
      setApprovalRequestReason("");
      onApprovalRequested?.(approval);
    });
  }

  async function trackDispatchProgress(job: CreateJobResponse, targets: AgentView[], jobMaxTimeoutSecs?: number) {
    const targetCount = createJobTargetCount(job);
    const boundedJobTimeoutSecs = clampJobMaxTimeoutSecs(jobMaxTimeoutSecs ?? effectiveJobMaxTimeoutSecs(maxTimeoutSecs));
    setLastDispatchProgress(null);
    setDispatchProgress(buildBulkJobProgress({
      jobId: job.job_id,
      targetCount,
      targetRecords: [],
      targets,
      maxTimeoutSecs: boundedJobTimeoutSecs,
    }));
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
    <section className={`fleetPanel commandComposer ${terminalSurface ? "terminalCommandComposer" : ""}`.trim()}>
      <div className="sectionHeader">
        <div>
          <h2>{terminalSurface ? "Terminal review composer" : "Dispatch command"}</h2>
          <span>{dispatchHeaderStatus}</span>
        </div>
        <div className="headerActionStack">
          {privilegeMaterial ? (
            <button className="secondaryAction" onClick={lockPrivilege} type="button">
              <LockKeyhole size={17} />
              Lock
            </button>
          ) : (
            <ShieldCheck size={20} />
          )}
        </div>
      </div>

      <form className="dispatchForm" onSubmit={submitJob}>
        <ActionFeedback
          className="localActionFeedback"
          message={dispatchFeedbackMessage}
          tone={dispatchFeedbackTone}
        />
        {!terminalSurface && (
          <>
            <div className="templateToolbar" aria-label="Command template controls">
              <div className="templateToolbarPrimary">
                <label>
                  <span>Template</span>
                  <select
                    aria-label="Template selector"
                    onChange={(event) => applyCommandTemplate(event.target.value)}
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
                              {template.scope_value ? `:${template.scope_value}` : ""}
                            </option>
                          ))}
                        </optgroup>
                      </>
                    )}
                  </select>
                </label>
                <span className="templateToolbarStatus">
                  {selectedTemplate ? `${selectedTemplate.scope_kind}${selectedTemplate.scope_value ? `:${selectedTemplate.scope_value}` : ""}` : "Optional"}
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
                      onChange={(event) => setTemplateScopeKind(event.target.value as typeof templateScopeKind)}
                      value={templateScopeKind}
                    >
                      <option value="global">Global</option>
                      <option value="provider">Provider</option>
                      <option value="tag">Tag</option>
                      <option value="client">Client</option>
                    </select>
                  </label>
                  <label>
                    <span>Scope value</span>
                    <input
                      aria-label="Command template scope value"
                      disabled={templateScopeKind === "global"}
                      onChange={(event) => setTemplateScopeValue(event.target.value)}
                      placeholder={templateScopeKind}
                      value={templateScopeKind === "global" ? "" : templateScopeValue}
                    />
                  </label>
                  <div className="templateToolbarActions">
                    <button
                      className="secondaryAction"
                      disabled={templatePending}
                      onClick={() => void reviewCommandTemplateSave()}
                      type="button"
                    >
                      {selectedTemplate?.built_in ? "Review copy" : "Review save"}
                    </button>
                    <button
                      className="secondaryAction dangerAction"
                      disabled={templatePending || !selectedTemplate || selectedTemplate.built_in}
                      onClick={() => {
                        if (!selectedTemplate || selectedTemplate.built_in) {
                          return;
                        }
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
            </div>
            <ConfirmationPrompt
              confirmLabel={templateSaveSnapshot?.title ?? "Save template"}
              detail={
                templateConfirmation === "save-copy"
                  ? "Creates a user-defined command template. The built-in template remains unchanged."
                  : "Saves the reviewed command template request exactly as shown."
              }
              items={[
                { label: "Template", value: templateSaveSnapshot?.request.name ?? "-" },
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
              items={[
                { label: "Template", value: deleteTemplateSnapshot?.name ?? "-" },
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
              open={templateConfirmation === "delete" && deleteTemplateSnapshot !== null}
              pending={templatePending}
              title="Confirm template delete"
              tone="danger"
            />
          </>
        )}
        {fixedMode ? (
          <div className="dispatchModeNotice" aria-label="Dispatch mode boundary">
            <strong>{focusedModeBoundary?.label}</strong>
            <span>{focusedModeBoundary?.detail}</span>
            {onOpenJobsDispatch ? (
              <button className="secondaryAction compactAction" onClick={onOpenJobsDispatch} type="button">
                Jobs / Dispatch
              </button>
            ) : null}
          </div>
        ) : (
          <>
            <div className="dispatchModeNotice" aria-label="Dispatch mode boundary">
              <strong>Advanced dispatch</strong>
              <span>Terminal open and resume start in Remote / Terminal.</span>
              {onOpenRemoteTerminal ? (
                <button className="secondaryAction compactAction" onClick={onOpenRemoteTerminal} type="button">
                  Remote terminal
                </button>
              ) : null}
            </div>
            <OperationModeTabs includeTerminal={false} mode={mode} onModeChange={setMode} />
          </>
        )}
        <JobOperationEditor
          commandText={commandText}
          shellPty={shellPty}
          fileFollowSymlinks={fileFollowSymlinks}
          filePath={filePath}
          terminalAction={terminalAction}
          terminalArgv={terminalArgv}
          terminalCloseReason={terminalCloseReason}
          terminalCols={terminalCols}
          terminalCwd={terminalCwd}
          terminalUser={terminalUser}
          terminalUserPolicy={terminalUserPolicy}
          terminalFlowWindowBytes={terminalFlowWindowBytes}
          terminalIdleTimeoutSecs={terminalIdleTimeoutSecs}
          terminalInputText={terminalInputText}
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
          fileTransferUploadSourceKind={fileTransferUploadSourceKind}
          fileTransferRateLimit={fileTransferRateLimit}
          fileTransferResumeToken={fileTransferResumeToken}
          fileTransferSessionId={fileTransferSessionId}
          mode={mode}
          processLimit={processLimit}
          setCommandText={setCommandText}
          setShellPty={setShellPty}
          setShellScript={setShellScript}
          setTerminalAction={setTerminalAction}
          setTerminalArgv={setTerminalArgv}
          setTerminalCloseReason={setTerminalCloseReason}
          setTerminalCols={setTerminalCols}
          setTerminalCwd={setTerminalCwd}
          setTerminalUser={setTerminalUser}
          setTerminalUserPolicy={setTerminalUserPolicy}
          setTerminalFlowWindowBytes={setTerminalFlowWindowBytes}
          setTerminalIdleTimeoutSecs={setTerminalIdleTimeoutSecs}
          setTerminalInputText={setTerminalInputText}
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
          forceUnprivileged={supportsForceUnprivileged ? forceUnprivileged : false}
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
                  onChange={(event) => setForceUnprivileged(event.target.checked)}
                  type="checkbox"
                />
                <span>Force unprivileged best effort</span>
              </label>
            )}
            <DispatchOptions
              setMaxTimeoutSecs={setMaxTimeoutSecs}
              maxTimeoutSecs={maxTimeoutSecs}
            />
            <div className="dispatchOptionNote">
              <strong>Rollout controls</strong>
              <span>Per-job controls stored here are timeout and privilege mode. Fleet concurrency uses the system dispatcher policy; canary and stop-after-failure are not recorded by this job request.</span>
            </div>
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
            dispatchReviewIntent === "dispatch" && dispatchConfirmationDestructive
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
            context={lastDispatchContext ? `Dispatch: ${lastDispatchContext}` : undefined}
            loading={dispatchProgress !== null}
            onClearResults={clearExecutionResults}
            onOpenJobDetails={onOpenJobDetails}
            progress={visibleDispatchProgress}
          />
        )}

        {!dispatchPromptOpen && (
          <div className="dispatchActions">
            <button
              className="secondaryAction"
              disabled={pending}
              onClick={previewTargets}
              type="button"
            >
              <CheckCircle2 size={17} />
              Refresh target preview
            </button>
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
            aria-label={transferProgress.event === "downloaded" ? "Resumable download progress" : "Resumable upload progress"}
          >
            <strong>
              {transferProgress.event === "downloaded"
                ? "Download complete"
                : transferProgress.event === "committed"
                  ? "Upload complete"
                  : "Transfer in progress"}
            </strong>
            <span>
              {transferProgress.nextOffset}/{transferProgress.sizeBytes} bytes · session {shortId(transferProgress.sessionId)}
              {"multiTargetPolicy" in transferProgress ? ` · ${transferProgress.multiTargetPolicy}` : ""}
              {"downloadSink" in transferProgress ? ` · ${transferProgress.downloadSink}` : ""}
            </span>
          </div>
        )}
      </form>

      <PrivilegeVaultBox
        lastPayloadHash={lastPayloadHash}
        onOpenUnlock={onOpenPrivilegeUnlock}
        onPrivilegeMaterialChange={setPrivilegeMaterial}
        privilegeMaterial={privilegeMaterial}
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
        value: operation.rollback_sha256_hex ?? "Agent-managed previous artifact",
      },
    ];
  }
  if (operation.type.startsWith("terminal_")) {
    return [
      {
        label: "Session",
        value: "session_id" in operation ? operation.session_id : "Not reported",
      },
      { label: "Effect", value: jobOperationLabel(operation, operation.type) },
    ];
  }
  if (!operation.type.startsWith("process_")) {
    return [];
  }
  const processName =
    "name" in operation && typeof operation.name === "string" && operation.name.trim()
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
    terminal_close: "Close terminal session",
    terminal_input: "Send terminal input",
    terminal_open: "Open or attach terminal session",
    terminal_poll: "Poll terminal output",
    terminal_resize: "Resize terminal session",
  };
  return labels[operation.type] ?? fallback;
}

function generatedConfirmationRequiredForMode(
  mode: DispatchMode,
  supervisorAction: SupervisorAction,
  terminalAction: TerminalAction,
): boolean {
  const terminalOperationType = {
    close: "terminal_close",
    input: "terminal_input",
    open: "terminal_open",
    poll: "terminal_poll",
    resize: "terminal_resize",
  } satisfies Record<TerminalAction, keyof typeof JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE>;
  const operationType =
    mode === "terminal_session"
      ? terminalOperationType[terminalAction]
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

function operationUsesDangerTone(
  operation: JobOperation | undefined,
): boolean {
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
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    document.activeElement.blur();
  }
}
