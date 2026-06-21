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
import { readFilePushPayload, sha256Hex } from "../fileTransfer";
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
  type BrowserDownloadSinkMode,
  type BrowserTransferMultiTargetPolicy,
  type ResumableDownloadProgress,
  type ResumableUploadProgress,
} from "../resumableFileTransfer";
import {
  buildOperation,
  clampCommandTimeoutSecs,
  clampInteger,
  effectiveCommandTimeoutSecs,
  operationCommandLabel,
  parseOptionalCommandTimeoutSecs,
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
  CreateJobRequest,
  CreateJobResponse,
  DeleteCommandTemplateRequest,
  FileExistingPolicy,
  JobHistoryRecord,
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
  requestId: string;
  session: TerminalSessionRecord;
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

type DispatchConfirmationSnapshot = {
  operationLabel: string;
  forceUnprivileged: boolean;
  selectorExpression: string;
  targets: AgentView[];
  timeoutSecs: number;
  timeoutOverrideSecs?: number;
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

async function loadUploadSourceArtifactFile(
  sources: FileTransferSourceArtifactRecord[],
  sourceArtifactId: string,
  downloadSource: (downloadPath: string) => Promise<Blob>,
): Promise<File> {
  const artifact = sources.find((source) => source.id === sourceArtifactId);
  if (!artifact) {
    throw new Error("Select a source artifact");
  }
  const blob = await downloadSource(artifact.download_path);
  const bytes = new Uint8Array(await blob.arrayBuffer());
  if (bytes.byteLength !== artifact.size_bytes) {
    throw new Error(`Source artifact size mismatch for ${artifact.name}`);
  }
  const actualSha256Hex = await sha256Hex(bytes);
  if (actualSha256Hex !== artifact.sha256_hex) {
    throw new Error(`Source artifact SHA-256 mismatch for ${artifact.name}`);
  }
  return new File([bytes], artifact.name || "source-artifact.bin", {
    type: blob.type || "application/octet-stream",
  });
}

export function JobDispatchPanel({
  agents,
  fileTransferSources,
  commandTemplates,
  dispatchPreset,
  terminalComposerAction,
  onDispatchPresetApplied,
  onCreateJob,
  onDownloadFileTransferSource,
  onDownloadOutputChunk,
  onLoadJob,
  onLoadOutputs,
  onLoadTargets,
  onSubmitTerminalInput,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
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
  terminalComposerAction?: TerminalComposerAction | null;
  onDispatchPresetApplied?: () => void;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onDownloadOutputChunk: (jobId: string, clientId: string, seq: number) => Promise<Blob>;
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
  onResolveTargets: (selection: JobTargetSelection) => Promise<BulkResolveResponse>;
  onDeleteCommandTemplate: (
    templateId: string,
    request: DeleteCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  onUpsertCommandTemplate: (request: UpsertCommandTemplateRequest) => Promise<CommandTemplateRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
}) {
  const [mode, setMode] = useState<DispatchMode>("shell");
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
  const [hotConfigToml, setHotConfigToml] = useState("");
  const [updateArtifactUrl, setUpdateArtifactUrl] = useState("");
  const [updateSha256Hex, setUpdateSha256Hex] = useState("");
  const [updateCheckVersionUrl, setUpdateCheckVersionUrl] = useState(DEFAULT_UPDATE_VERSION_URL);
  const [updateCheckActivate, setUpdateCheckActivate] = useState(true);
  const [updateCheckRestartAgent, setUpdateCheckRestartAgent] = useState(true);
  const [updateActivationSha256Hex, setUpdateActivationSha256Hex] = useState("");
  const [updateRestartAgent, setUpdateRestartAgent] = useState(false);
  const [updateRollbackSha256Hex, setUpdateRollbackSha256Hex] = useState("");
  const [backupPathsText, setBackupPathsText] = useState(DEFAULT_JOB_BACKUP_PATHS);
  const [backupIncludeConfig, setBackupIncludeConfig] = useState(true);
  const [backupFollowSymlinks, setBackupFollowSymlinks] = useState(false);
  const [processLimit, setProcessLimit] = useState(50);
  const [supervisorAction, setSupervisorAction] = useState<SupervisorAction>("status");
  const [supervisorName, setSupervisorName] = useState("");
  const [supervisorArgv, setSupervisorArgv] = useState("");
  const [supervisorCwd, setSupervisorCwd] = useState("");
  const [supervisorEnv, setSupervisorEnv] = useState("");
  const [supervisorLogBytes, setSupervisorLogBytes] = useState(65536);
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(JOB_SELECTOR_STORAGE_KEY));
  const [timeoutSecs, setTimeoutSecs] = useState("");
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [dispatchProgress, setDispatchProgress] = useState<BulkJobProgress | null>(null);
  const [lastDispatchProgress, setLastDispatchProgress] = useState<BulkJobProgress | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [transferProgress, setTransferProgress] = useState<ResumableUploadProgress | ResumableDownloadProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [dispatchPromptOpen, setDispatchPromptOpen] = useState(false);
  const [dispatchConfirmation, setDispatchConfirmation] = useState<DispatchConfirmationSnapshot | null>(null);
  const [selectorVerification, setSelectorVerification] = useState<"checking" | "invalid" | "neutral" | "valid">("neutral");
  const [selectorVerificationMessage, setSelectorVerificationMessage] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);

  useEffect(() => {
    if (!dispatchPreset) {
      return;
    }
    setMode(dispatchPreset.mode);
    if (dispatchPreset.selectorExpression !== undefined) {
      setSelectorExpression(dispatchPreset.selectorExpression);
    }
    if (dispatchPreset.timeoutSecs !== undefined) {
      setTimeoutSecs(String(clampCommandTimeoutSecs(dispatchPreset.timeoutSecs)));
    } else if (dispatchPreset.mode === "agent_update_activate" || dispatchPreset.mode === "agent_update_rollback") {
      setTimeoutSecs("60");
    } else if (dispatchPreset.mode.startsWith("agent_update")) {
      setTimeoutSecs("300");
    }
    if (dispatchPreset.mode === "agent_update") {
      setUpdateArtifactUrl(dispatchPreset.updateArtifactUrl ?? "");
      setUpdateSha256Hex(dispatchPreset.updateSha256Hex ?? "");
    }
    if (dispatchPreset.mode === "agent_update_check") {
      setUpdateCheckVersionUrl(dispatchPreset.updateCheckVersionUrl ?? DEFAULT_UPDATE_VERSION_URL);
      setUpdateCheckActivate(dispatchPreset.updateCheckActivate ?? true);
      setUpdateCheckRestartAgent(dispatchPreset.updateCheckRestartAgent ?? true);
    }
    if (dispatchPreset.mode === "agent_update_activate") {
      setUpdateActivationSha256Hex(dispatchPreset.updateActivationSha256Hex ?? "");
      setUpdateRestartAgent(dispatchPreset.updateRestartAgent ?? true);
    }
    if (dispatchPreset.mode === "agent_update_rollback") {
      setUpdateRollbackSha256Hex(dispatchPreset.updateRollbackSha256Hex ?? "");
    }
    setPreview(null);
    clearDispatchReview();
    setActionError(null);
    clearExecutionResults();
    onDispatchPresetApplied?.();
  }, [dispatchPreset, onDispatchPresetApplied]);

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
    setTerminalUser("");
    setTerminalUserPolicy("fail");
    setTerminalCols(session.cols ?? 120);
    setTerminalRows(session.rows ?? 40);
    setTerminalIdleTimeoutSecs(session.idle_timeout_secs ?? 3600);
    setTerminalFlowWindowBytes(session.flow_window_bytes ?? 65536);
    setTerminalReplayFromSeq(
      terminalComposerAction.action === "open" || terminalComposerAction.action === "poll"
        ? String(session.output_retained_first_seq ?? session.output_first_seq ?? 0)
        : "",
    );
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
    hotConfigToml,
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
    timeoutSecs,
    updateActivationSha256Hex,
    updateArtifactUrl,
    updateCheckActivate,
    updateCheckRestartAgent,
    updateCheckVersionUrl,
    updateRestartAgent,
    updateRollbackSha256Hex,
    updateSha256Hex,
    invalidateReviewGeneration,
  ]);

  useEffect(() => {
    if (!selectorExpression.trim()) {
      setSelectorVerification("neutral");
      setSelectorVerificationMessage(null);
      setPreview(null);
      return;
    }
    if (selectorParse.error) {
      setSelectorVerification("invalid");
      setSelectorVerificationMessage("Invalid");
      setPreview(null);
      return;
    }
    let disposed = false;
    setSelectorVerification("checking");
    setSelectorVerificationMessage("Checking");
    const timeout = window.setTimeout(() => {
      void onResolveTargets({
        selector_expression: selectorExpression.trim(),
      })
        .then((response) => {
          if (disposed) {
            return;
          }
          setPreview(response);
          setSelectorVerification("valid");
          setSelectorVerificationMessage(`${response.target_count}/${agents.length}`);
        })
        .catch(() => {
          if (disposed) {
            return;
          }
          setPreview(null);
          setSelectorVerification("invalid");
          setSelectorVerificationMessage("Invalid");
        });
    }, 300);
    return () => {
      disposed = true;
      window.clearTimeout(timeout);
    };
  }, [agents.length, mode, onResolveTargets, selectorExpression, selectorParse.error]);

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
                  : mode === "hot_config"
                    ? hotConfigToml.trim().length > 0
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
    () => (selectorParse.error ? [] : agentsMatchingExpression(agents, selectorExpression)),
    [agents, selectorExpression, selectorParse.error],
  );
  const impactMode = targetImpactModeForDispatch(mode);
  const supportsForceUnprivileged = impactMode !== "generic";
  const operationNeedsConfirmation = generatedConfirmationRequiredForMode(mode, supervisorAction, terminalAction);
  const impactTargets = preview?.targets ?? expressionTargets;
  const activeDispatchConfirmation = dispatchPromptOpen ? dispatchConfirmation : null;
  const dispatchConfirmationSelector =
    activeDispatchConfirmation?.selectorExpression ?? selectorExpression.trim();
  const dispatchConfirmationTargets =
    activeDispatchConfirmation?.targets ?? preview?.targets ?? expressionTargets;
  const dispatchConfirmationTimeoutSecs =
    activeDispatchConfirmation?.timeoutSecs ?? effectiveCommandTimeoutSecs(timeoutSecs);
  const dispatchConfirmationForceUnprivileged =
    activeDispatchConfirmation?.forceUnprivileged ??
    (supportsForceUnprivileged ? forceUnprivileged : false);
  const dispatchConfirmationOperationLabel =
    activeDispatchConfirmation?.operationLabel ?? operationCommandLabel(mode, commandText);
  const dispatchConfirmationDestructive =
    activeDispatchConfirmation?.kind === "job"
      ? activeDispatchConfirmation.destructive
      : operationNeedsConfirmation;
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
    { label: "Timeout", value: `${dispatchConfirmationTimeoutSecs}s` },
    {
      label: "Privilege",
      value: privilegeMaterial ? "Unlocked locally" : "Locked",
    },
    {
      label: "Execution",
      value: dispatchConfirmationForceUnprivileged ? "Forced best effort" : operationNeedsConfirmation ? "Privileged mutation" : "Standard",
    },
  ];
  const status =
    actionError ??
    reviewStatus ??
    (visibleDispatchProgress
      ? `Job ${shortId(visibleDispatchProgress.jobId)} result recorded`
      : lastJob
        ? `Job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} targets`
      : preview
        ? `${preview.target_count} resolved targets`
        : privilegeMaterial
          ? "Ready"
          : "Locked");

  function lockPrivilege() {
    setPrivilegeMaterial(null);
    setActionError(null);
    clearDispatchReview();
  }

  function clearExecutionResults() {
    setDispatchProgress(null);
    setLastDispatchProgress(null);
    setLastJob(null);
    setTransferProgress(null);
  }

  function clearDispatchReview() {
    invalidateReviewGeneration();
    setDispatchPromptOpen(false);
    setDispatchConfirmation(null);
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
    setActionError(null);
    if (!privilegeMaterial) {
      setActionError("Privilege unlock is locked");
      return;
    }
    if (selectorParse.error) {
      setActionError(selectorParse.error);
      return;
    }
    if (!selectorExpression.trim()) {
      setActionError("Select at least one VPS or tag target");
      return;
    }
    if (!operationReady) {
      setActionError("Complete the selected operation before dispatching");
      return;
    }
    blurActiveElement();
    const reviewGeneration = captureReviewGeneration();
    const selection = targetSelection();
    setReviewStatus("Preparing dispatch review");
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
    const selector = selectorExpression.trim();
    const timeoutOverride = parseOptionalCommandTimeoutSecs(timeoutSecs);
    const timeout = timeoutOverride ?? effectiveCommandTimeoutSecs(timeoutSecs);
    const frozenForceUnprivileged = supportsForceUnprivileged ? forceUnprivileged : false;
    const operationLabel = operationCommandLabel(mode, commandText);
    const base = {
      forceUnprivileged: frozenForceUnprivileged,
      operationLabel,
      selectorExpression: selector,
      targets,
      timeoutSecs: timeout,
      timeoutOverrideSecs: timeoutOverride,
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
      return {
        ...base,
        chunkSizeBytes: fileTransferChunkSize,
        existingPolicy: fileTransferExistingPolicy,
        file: uploadSourceFile,
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
          timeoutSecs: timeout,
          confirmed: true,
        }),
        privilegeMaterial,
      });
      return {
        ...base,
        clientId,
        jobId: crypto.randomUUID(),
        kind: "terminal_input",
        operationLabel: "terminal_input",
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
      hotConfigToml,
      updateArtifactUrl,
      updateSha256Hex,
      updateCheckVersionUrl,
      updateCheckActivate,
      updateCheckRestartAgent,
      updateActivationSha256Hex,
      updateRestartAgent,
      updateRollbackSha256Hex,
      backupPathsText,
      backupIncludeConfig,
      backupFollowSymlinks,
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
        timeoutSecs: timeout,
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
    if (typeof defaults.timeout_secs === "number") {
      setTimeoutSecs(String(clampCommandTimeoutSecs(defaults.timeout_secs)));
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
        setUpdateCheckActivate(operation.activate ?? true);
        setUpdateCheckRestartAgent(operation.restart_agent ?? true);
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
      hotConfigToml,
      updateArtifactUrl,
      updateSha256Hex,
      updateCheckVersionUrl,
      updateCheckActivate,
      updateCheckRestartAgent,
      updateActivationSha256Hex,
      updateRestartAgent,
      updateRollbackSha256Hex,
      backupPathsText,
      backupIncludeConfig,
      backupFollowSymlinks,
      filePushPath,
      filePushMode,
      null,
    );
    const timeoutOverride = parseOptionalCommandTimeoutSecs(timeoutSecs);
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
        ...(timeoutOverride !== undefined ? { timeout_secs: timeoutOverride } : {}),
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
          timeoutSecs: confirmed.timeoutSecs,
          timeoutOverrideSecs: confirmed.timeoutOverrideSecs,
          onProgress: (progress) => {
            setTransferProgress(progress);
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastJob(commitJob);
        setLastPayloadHash(null);
        await trackDispatchProgress(commitJob, confirmed.targets, confirmed.timeoutSecs);
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
          timeoutSecs: confirmed.timeoutSecs,
          timeoutOverrideSecs: confirmed.timeoutOverrideSecs,
          onProgress: (progress) => {
            setTransferProgress(progress);
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastJob(startJob);
        setLastPayloadHash(null);
        await trackDispatchProgress(startJob, confirmed.targets, confirmed.timeoutSecs);
        return;
      }
      if (confirmed.kind === "terminal_input") {
        const response = await onSubmitTerminalInput(confirmed.clientId, confirmed.sessionId, {
          job_id: confirmed.jobId,
          text: confirmed.text,
          ...(confirmed.timeoutOverrideSecs !== undefined ? { timeout_secs: confirmed.timeoutOverrideSecs } : {}),
          confirmed: true,
          privilege_assertion: confirmed.privilegeAssertion,
        });
        setLastJob(response.job);
        setLastPayloadHash(confirmed.payloadHashHex);
        await trackDispatchProgress(response.job, confirmed.targets, confirmed.timeoutSecs);
        return;
      }
      const clientIds = confirmed.targets.map((target) => target.id);
      const nextJob = await onCreateJob({
        job_id: confirmed.jobId,
        selector_expression: confirmed.selectorExpression,
        target_client_ids: clientIds,
        destructive: confirmed.destructive,
        confirmed: confirmed.destructive,
        command: confirmed.commandType,
        argv: confirmed.argv,
        operation: confirmed.operation,
        ...(confirmed.timeoutOverrideSecs !== undefined ? { timeout_secs: confirmed.timeoutOverrideSecs } : {}),
        force_unprivileged: confirmed.forceUnprivileged,
        privileged: true,
        privilege_assertion: confirmed.privilegeAssertion,
      });
      setLastJob(nextJob);
      setLastPayloadHash(confirmed.payloadHashHex);
      await trackDispatchProgress(nextJob, confirmed.targets, confirmed.timeoutSecs);
    });
  }

  async function trackDispatchProgress(job: CreateJobResponse, targets: AgentView[], jobTimeoutSecs?: number) {
    const targetCount = createJobTargetCount(job);
    const boundedJobTimeoutSecs = clampCommandTimeoutSecs(jobTimeoutSecs ?? effectiveCommandTimeoutSecs(timeoutSecs));
    setLastDispatchProgress(null);
    setDispatchProgress(buildBulkJobProgress({
      jobId: job.job_id,
      targetCount,
      targetRecords: [],
      targets,
      timeoutSecs: boundedJobTimeoutSecs,
    }));
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        onProgress: setDispatchProgress,
        targetCount,
        targets,
        timeoutSecs: boundedJobTimeoutSecs,
      });
      setLastDispatchProgress(result.progress);
    } finally {
      setDispatchProgress(null);
    }
  }

  function targetSelection(): JobTargetSelection {
    return {
      selector_expression: selectorExpression.trim(),
    };
  }

  return (
    <section className="fleetPanel commandComposer">
      <div className="sectionHeader">
        <div>
          <h2>Dispatch command</h2>
          <span>{status}</span>
        </div>
        {privilegeMaterial ? (
          <button className="secondaryAction" onClick={lockPrivilege} type="button">
            <LockKeyhole size={17} />
            Lock
          </button>
        ) : (
          <ShieldCheck size={20} />
        )}
      </div>

      <form className="dispatchForm" onSubmit={submitJob}>
        <div className="templateToolbar" aria-label="Command template controls">
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
                    ────────── User-defined templates ──────────
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
        <OperationModeTabs mode={mode} onModeChange={setMode} />
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
          hotConfigToml={hotConfigToml}
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
          setHotConfigToml={setHotConfigToml}
          setProcessLimit={setProcessLimit}
          setSupervisorAction={setSupervisorAction}
          setSupervisorArgv={setSupervisorArgv}
          setSupervisorCwd={setSupervisorCwd}
          setSupervisorEnv={setSupervisorEnv}
          setSupervisorLogBytes={setSupervisorLogBytes}
          setSupervisorName={setSupervisorName}
          setUpdateArtifactUrl={setUpdateArtifactUrl}
          setUpdateCheckActivate={setUpdateCheckActivate}
          setUpdateCheckRestartAgent={setUpdateCheckRestartAgent}
          setUpdateCheckVersionUrl={setUpdateCheckVersionUrl}
          setUpdateActivationSha256Hex={setUpdateActivationSha256Hex}
          setUpdateRestartAgent={setUpdateRestartAgent}
          setUpdateRollbackSha256Hex={setUpdateRollbackSha256Hex}
          setUpdateSha256Hex={setUpdateSha256Hex}
          setBackupIncludeConfig={setBackupIncludeConfig}
          setBackupFollowSymlinks={setBackupFollowSymlinks}
          setBackupPathsText={setBackupPathsText}
          supervisorAction={supervisorAction}
          supervisorArgv={supervisorArgv}
          supervisorCwd={supervisorCwd}
          supervisorEnv={supervisorEnv}
          supervisorLogBytes={supervisorLogBytes}
          supervisorName={supervisorName}
          updateArtifactUrl={updateArtifactUrl}
          updateCheckActivate={updateCheckActivate}
          updateCheckRestartAgent={updateCheckRestartAgent}
          updateCheckVersionUrl={updateCheckVersionUrl}
          updateActivationSha256Hex={updateActivationSha256Hex}
          updateRestartAgent={updateRestartAgent}
          updateRollbackSha256Hex={updateRollbackSha256Hex}
          updateSha256Hex={updateSha256Hex}
          backupIncludeConfig={backupIncludeConfig}
          backupFollowSymlinks={backupFollowSymlinks}
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
          setTimeoutSecs={setTimeoutSecs}
          timeoutSecs={timeoutSecs}
        />

        <ConfirmationPrompt
          confirmLabel="Dispatch job"
          detail={`${dispatchConfirmationOperationLabel} on ${vpsCountLabel(dispatchConfirmationTargets.length)}.`}
          items={dispatchConfirmationItems}
          onCancel={() => {
            setDispatchPromptOpen(false);
            setDispatchConfirmation(null);
          }}
          onConfirm={() => void dispatchJobNow()}
          open={dispatchPromptOpen}
          pending={pending}
          title="Confirm job dispatch"
          tone={dispatchConfirmationDestructive ? "danger" : "normal"}
        />

        {visibleDispatchProgress && (
          <ExecutionResultPanel
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
              disabled={pending || !selectorExpression.trim()}
              onClick={previewTargets}
              type="button"
            >
              <CheckCircle2 size={17} />
              Review targets
            </button>
            <button
              className="primaryAction"
              disabled={pending || !operationReady || !selectorExpression.trim() || !privilegeMaterial}
              type="submit"
            >
              <Play size={17} />
              Review dispatch
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

function blurActiveElement() {
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    document.activeElement.blur();
  }
}
