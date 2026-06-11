import { useEffect, useMemo, useState, type FormEvent } from "react";
import { CheckCircle2, LockKeyhole, Play, ShieldCheck } from "lucide-react";
import {
  acceptedDispatchTargetCount,
  formatTargetAvailabilitySummary,
  targetPreflightUnavailable,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../bulkJobProgress";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { readFilePushPayload, sha256Hex } from "../fileTransfer";
import {
  buildPrivilegeAssertion,
  canonicalJobPrivilegeIntent,
  operationPayloadHashHex,
  parseCommandArgv,
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
  clampInteger,
  operationCommandLabel,
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
  FileExistingPolicy,
  JobHistoryRecord,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetSelection,
  UpsertCommandTemplateRequest,
} from "../types";
import type { FileTransferSourceArtifactRecord } from "../typesFileTransfer";
import type { TerminalSessionRecord } from "../typesTerminal";
import { runPanelAction, shortId } from "../utils";
import { DispatchOptions, JobTargetSelector } from "./JobDispatchControls";
import { JobOperationEditor, OperationModeTabs } from "./jobs/JobOperationControls";
import { agentsMatchingExpression, parseSearchExpression } from "../searchExpression";
import { TargetImpactPreview, targetImpactModeForDispatch } from "./TargetImpactPreview";

const DEFAULT_UPDATE_VERSION_URL = "https://github.com/mnihyc/vpsman/releases/latest/download/version.json";
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

function commandTypeForApi(operation: CreateJobRequest["operation"]): string {
  if (!operation) {
    return "shell_argv";
  }
  if (operation.type === "shell") {
    return operation.pty ? "shell_pty" : "shell_argv";
  }
  return operation.type;
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
  terminalComposerAction,
  onCreateJob,
  onDownloadFileTransferSource,
  onDownloadOutputArtifact,
  onLoadJob,
  onLoadOutputs,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onResolveTargets,
  onUpsertCommandTemplate,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
  commandTemplates: CommandTemplateRecord[];
  terminalComposerAction?: TerminalComposerAction | null;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onDownloadOutputArtifact: (jobId: string, clientId: string, seq: number) => Promise<Blob>;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onResolveTargets: (selection: JobTargetSelection) => Promise<BulkResolveResponse>;
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
  const [terminalIdleTimeoutSecs, setTerminalIdleTimeoutSecs] = useState(1800);
  const [terminalFlowWindowBytes, setTerminalFlowWindowBytes] = useState(65536);
  const [terminalInputSeq, setTerminalInputSeq] = useState(1);
  const [terminalInputText, setTerminalInputText] = useState("");
  const [terminalCloseReason, setTerminalCloseReason] = useState("");
  const [filePath, setFilePath] = useState("");
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
  const [hotConfigToml, setHotConfigToml] = useState("");
  const [updateArtifactUrl, setUpdateArtifactUrl] = useState("");
  const [updateSha256Hex, setUpdateSha256Hex] = useState("");
  const [updateArtifactSignatureHex, setUpdateArtifactSignatureHex] = useState("");
  const [updateArtifactSigningKeyHex, setUpdateArtifactSigningKeyHex] = useState("");
  const [updateCheckVersionUrl, setUpdateCheckVersionUrl] = useState(DEFAULT_UPDATE_VERSION_URL);
  const [updateCheckActivate, setUpdateCheckActivate] = useState(true);
  const [updateCheckRestartAgent, setUpdateCheckRestartAgent] = useState(true);
  const [updateActivationSha256Hex, setUpdateActivationSha256Hex] = useState("");
  const [updateRestartAgent, setUpdateRestartAgent] = useState(false);
  const [updateRollbackSha256Hex, setUpdateRollbackSha256Hex] = useState("");
  const [backupPathsText, setBackupPathsText] = useState(DEFAULT_JOB_BACKUP_PATHS);
  const [backupIncludeConfig, setBackupIncludeConfig] = useState(true);
  const [processLimit, setProcessLimit] = useState(50);
  const [supervisorAction, setSupervisorAction] = useState<SupervisorAction>("status");
  const [supervisorName, setSupervisorName] = useState("");
  const [supervisorArgv, setSupervisorArgv] = useState("");
  const [supervisorCwd, setSupervisorCwd] = useState("");
  const [supervisorEnv, setSupervisorEnv] = useState("");
  const [supervisorLogBytes, setSupervisorLogBytes] = useState(65536);
  const [selectorExpression, setSelectorExpression] = useState(() => readLocalString(JOB_SELECTOR_STORAGE_KEY));
  const [timeoutSecs, setTimeoutSecs] = useState(30);
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [dispatchProgress, setDispatchProgress] = useState<BulkJobProgress | null>(null);
  const [lastDispatchProgress, setLastDispatchProgress] = useState<BulkJobProgress | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [transferProgress, setTransferProgress] = useState<ResumableUploadProgress | ResumableDownloadProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [dispatchPromptOpen, setDispatchPromptOpen] = useState(false);
  const [selectorVerification, setSelectorVerification] = useState<"checking" | "invalid" | "neutral" | "valid">("neutral");
  const [selectorVerificationMessage, setSelectorVerificationMessage] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const selectorParse = useMemo(() => parseSearchExpression(selectorExpression), [selectorExpression]);

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
    setTerminalIdleTimeoutSecs(session.idle_timeout_secs ?? 1800);
    setTerminalFlowWindowBytes(session.flow_window_bytes ?? 65536);
    setTerminalReplayFromSeq(
      terminalComposerAction.action === "open" || terminalComposerAction.action === "poll"
        ? String(session.output_retained_first_seq ?? session.output_first_seq ?? 0)
        : "",
    );
    setTerminalInputSeq((session.last_input_seq ?? 0) + 1);
    setTerminalInputText("");
    setTerminalCloseReason(session.close_reason ?? "operator");
    setSelectorExpression(`id:${session.client_id}`);
    setPreview(null);
    setActionError(null);
  }, [terminalComposerAction]);

  useEffect(() => {
    writeLocalString(JOB_SELECTOR_STORAGE_KEY, selectorExpression);
  }, [selectorExpression]);

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
  const updateSignatureReady =
    (!updateArtifactSignatureHex.trim() && !updateArtifactSigningKeyHex.trim()) ||
    (/^[0-9a-fA-F]{128}$/.test(updateArtifactSignatureHex.trim()) &&
      /^[0-9a-fA-F]{64}$/.test(updateArtifactSigningKeyHex.trim()));
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
                          /^[0-9a-fA-F]{64}$/.test(updateSha256Hex.trim()) &&
                          updateSignatureReady
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
  const selectedTargetCount = expressionTargets.length;
  const impactMode = targetImpactModeForDispatch(mode);
  const supportsForceUnprivileged = impactMode !== "generic";
  const operationNeedsConfirmation = operationRequiresConfirmation(mode);
  const impactTargets = preview?.targets ?? expressionTargets;
  const visibleDispatchProgress = dispatchProgress ?? lastDispatchProgress;
  const confirmationTargets = preview?.targets ?? expressionTargets;
  const dispatchConfirmationItems = [
    { label: "Operation", value: operationCommandLabel(mode, commandText) },
    { label: "Selector", value: selectorExpression.trim() || "-" },
    {
      label: "Targets",
      value: formatTargetAvailabilitySummary(confirmationTargets),
    },
    { label: "Timeout", value: `${clampInteger(timeoutSecs, 1, 3600)}s` },
    {
      label: "Privilege",
      value: privilegeMaterial ? "Unlocked locally" : "Locked",
    },
    {
      label: "Execution",
      value: forceUnprivileged ? "Forced best effort" : operationNeedsConfirmation ? "Privileged mutation" : "Standard",
    },
  ];
  const status =
    actionError ??
    (visibleDispatchProgress
      ? `Job ${shortId(visibleDispatchProgress.jobId)} result recorded`
      : lastJob
        ? `Job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} queued`
      : preview
        ? `${preview.target_count} resolved targets`
        : privilegeMaterial
          ? "Ready"
          : "Locked");

  function lockPrivilege() {
    setPrivilegeMaterial(null);
    setActionError(null);
  }

  function clearExecutionResults() {
    setDispatchProgress(null);
    setLastDispatchProgress(null);
    setLastJob(null);
    setTransferProgress(null);
  }

  async function previewTargets() {
    if (selectorParse.error) {
      setActionError(selectorParse.error);
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      setPreview(await onResolveTargets(targetSelection()));
    });
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
    if (!selectorExpression.trim() || selectedTargetCount === 0) {
      setActionError("Select at least one VPS or tag target");
      return;
    }
    if (!operationReady) {
      setActionError("Complete the selected operation before dispatching");
      return;
    }
    blurActiveElement();
    await runPanelAction(setPending, setActionError, async () => {
      const resolved = await onResolveTargets(targetSelection());
      if (!resolved.targets.length) {
        throw new Error("Target confirmation resolved no VPSs");
      }
      setPreview(resolved);
      setDispatchPromptOpen(true);
    });
  }

  function applyCommandTemplate(templateId: string) {
    setSelectedTemplateId(templateId);
    const template = commandTemplates.find((candidate) => candidate.id === templateId);
    if (!template) {
      return;
    }
    applyTemplateOperation(template.operation);
    setTemplateName(template.name);
    setTemplateScopeKind(template.scope_kind as "global" | "provider" | "tag" | "client");
    setTemplateScopeValue(template.scope_value ?? "");
    setActionError(null);
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
        setUpdateArtifactSignatureHex(operation.artifact_signature_hex ?? "");
        setUpdateArtifactSigningKeyHex(operation.artifact_signing_key_hex ?? "");
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

  async function saveCommandTemplate() {
    await runPanelAction(setTemplatePending, setActionError, async () => {
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
        terminalInputSeq,
        terminalInputText,
        terminalCloseReason,
        filePath,
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
        updateArtifactSignatureHex,
        updateArtifactSigningKeyHex,
        updateCheckVersionUrl,
        updateCheckActivate,
        updateCheckRestartAgent,
        updateActivationSha256Hex,
        updateRestartAgent,
        updateRollbackSha256Hex,
        backupPathsText,
        backupIncludeConfig,
        filePushPath,
        filePushMode,
        null,
      );
      await onUpsertCommandTemplate({
        name,
        scope_kind: templateScopeKind,
        scope_value: scopeValue,
        command_type: operationCommandLabel(mode, commandText),
        operation,
        defaults: {
          confirmed: operationNeedsConfirmation,
          destructive: operationNeedsConfirmation,
          force_unprivileged: supportsForceUnprivileged ? forceUnprivileged : false,
          timeout_secs: clampInteger(timeoutSecs, 1, 3600),
        },
        confirmed: true,
      });
    });
  }

  async function dispatchJobNow() {
    setDispatchPromptOpen(false);
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      const resolved = preview;
      if (!resolved?.targets.length) {
        throw new Error("Confirmed target snapshot is missing; review the targets again");
      }
      if (mode === "file_transfer_upload") {
        const clientIds = resolved.targets.map((target) => target.id);
        const uploadSourceFile =
          fileTransferUploadSourceKind === "source-artifact"
            ? await loadUploadSourceArtifactFile(
                fileTransferSources,
                fileTransferSourceArtifactId,
                onDownloadFileTransferSource,
              )
            : filePushSource;
        const commitJob = await runBrowserResumableUpload({
          clientIds,
          confirmed: true,
          createJob: onCreateJob,
          file: uploadSourceFile,
          loadJob: onLoadJob,
          loadOutputs: onLoadOutputs,
          modeText: filePushMode,
          multiTargetPolicy: fileTransferMultiTargetPolicy,
          existingPolicy: fileTransferExistingPolicy,
          path: filePushPath,
          privilegeMaterial,
          rateLimitKbps: fileTransferRateLimit,
          chunkSizeBytes: fileTransferChunkSize,
          resumeToken: fileTransferResumeToken,
          sessionId: fileTransferSessionId,
          timeoutSecs,
          onProgress: (progress) => {
            setTransferProgress(progress);
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastJob(commitJob);
        setLastPayloadHash(null);
        await trackDispatchProgress(commitJob, resolved.targets);
        return;
      }
      if (mode === "file_transfer_download") {
        const clientIds = resolved.targets.map((target) => target.id);
        const startJob = await runBrowserResumableDownload({
          clientIds,
          confirmed: true,
          createJob: onCreateJob,
          downloadName: fileTransferDownloadName,
          downloadSink: fileTransferDownloadSink,
          downloadOutputArtifact: onDownloadOutputArtifact,
          loadJob: onLoadJob,
          loadOutputs: onLoadOutputs,
          path: filePath,
          privilegeMaterial,
          rateLimitKbps: fileTransferRateLimit,
          chunkSizeBytes: fileTransferChunkSize,
          resumeToken: fileTransferResumeToken,
          sessionId: fileTransferSessionId,
          timeoutSecs,
          onProgress: (progress) => {
            setTransferProgress(progress);
            setFileTransferSessionId(progress.sessionId);
            setFileTransferResumeToken(progress.resumeToken);
          },
        });
        setLastJob(startJob);
        setLastPayloadHash(null);
        await trackDispatchProgress(startJob, resolved.targets);
        return;
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
        terminalInputSeq,
        terminalInputText,
        terminalCloseReason,
        filePath,
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
        updateArtifactSignatureHex,
        updateArtifactSigningKeyHex,
        updateCheckVersionUrl,
        updateCheckActivate,
        updateCheckRestartAgent,
        updateActivationSha256Hex,
        updateRestartAgent,
        updateRollbackSha256Hex,
        backupPathsText,
        backupIncludeConfig,
        filePushPath,
        filePushMode,
        filePushPayload,
      );
      const clientIds = resolved.targets.map((target) => target.id);
      const payloadHashHex = await operationPayloadHashHex(operation);
      const commandType = commandTypeForApi(operation);
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalJobPrivilegeIntent({
          selectorExpression,
          commandType,
          operationPayloadHash: payloadHashHex,
          resolvedTargets: clientIds,
          timeoutSecs: clampInteger(timeoutSecs, 1, 3600),
          forceUnprivileged: supportsForceUnprivileged ? forceUnprivileged : false,
          privileged: true,
        }),
        privilegeMaterial,
      });
      const nextJob = await onCreateJob({
        job_id: crypto.randomUUID(),
        selector_expression: selectorExpression.trim(),
        target_client_ids: clientIds,
        destructive: operationNeedsConfirmation,
        confirmed: operationNeedsConfirmation,
        command: commandType,
        argv: mode === "shell" && operation.type === "shell" ? operation.argv : [],
        operation,
        timeout_secs: clampInteger(timeoutSecs, 1, 3600),
        force_unprivileged: supportsForceUnprivileged ? forceUnprivileged : false,
        privileged: true,
        privilege_assertion: privilegeAssertion,
        reconnect_policy: {
          duplicate_delivery: "ignore_completed",
          resume_outputs: true,
        },
      });
      setLastJob(nextJob);
      setLastPayloadHash(payloadHashHex);
      await trackDispatchProgress(nextJob, resolved.targets);
    });
  }

  async function trackDispatchProgress(job: CreateJobResponse, targets: AgentView[]) {
    const accepted = acceptedDispatchTargetCount(job.target_count, targets);
    setLastDispatchProgress(null);
    setDispatchProgress({
      accepted,
      completed: 0,
      doing: accepted,
      expected: targets.length,
      failed: 0,
      jobId: job.job_id,
      retrieved: 0,
      unavailable: targets.filter(targetPreflightUnavailable).length,
    });
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        acceptedTargets: job.target_count,
        onProgress: setDispatchProgress,
        targets,
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
              aria-label="Saved command template"
              onChange={(event) => applyCommandTemplate(event.target.value)}
              value={selectedTemplateId}
            >
              <option value="">Select saved template</option>
              {commandTemplates.map((template) => (
                <option key={template.id} value={template.id}>
                  {template.name} · {template.scope_kind}
                  {template.scope_value ? `:${template.scope_value}` : ""}
                </option>
              ))}
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
          <button
            className="secondaryAction"
            disabled={templatePending}
            onClick={() => void saveCommandTemplate()}
            type="button"
          >
            Save template
          </button>
        </div>
        <OperationModeTabs mode={mode} onModeChange={setMode} />
        <JobOperationEditor
          commandText={commandText}
          shellPty={shellPty}
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
          terminalInputSeq={terminalInputSeq}
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
          setTerminalInputSeq={setTerminalInputSeq}
          setTerminalInputText={setTerminalInputText}
          setTerminalReplayFromSeq={setTerminalReplayFromSeq}
          setTerminalRows={setTerminalRows}
          setTerminalSessionId={setTerminalSessionId}
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
          setUpdateArtifactSignatureHex={setUpdateArtifactSignatureHex}
          setUpdateArtifactSigningKeyHex={setUpdateArtifactSigningKeyHex}
          setUpdateArtifactUrl={setUpdateArtifactUrl}
          setUpdateCheckActivate={setUpdateCheckActivate}
          setUpdateCheckRestartAgent={setUpdateCheckRestartAgent}
          setUpdateCheckVersionUrl={setUpdateCheckVersionUrl}
          setUpdateActivationSha256Hex={setUpdateActivationSha256Hex}
          setUpdateRestartAgent={setUpdateRestartAgent}
          setUpdateRollbackSha256Hex={setUpdateRollbackSha256Hex}
          setUpdateSha256Hex={setUpdateSha256Hex}
          setBackupIncludeConfig={setBackupIncludeConfig}
          setBackupPathsText={setBackupPathsText}
          supervisorAction={supervisorAction}
          supervisorArgv={supervisorArgv}
          supervisorCwd={supervisorCwd}
          supervisorEnv={supervisorEnv}
          supervisorLogBytes={supervisorLogBytes}
          supervisorName={supervisorName}
          updateArtifactSignatureHex={updateArtifactSignatureHex}
          updateArtifactSigningKeyHex={updateArtifactSigningKeyHex}
          updateArtifactUrl={updateArtifactUrl}
          updateCheckActivate={updateCheckActivate}
          updateCheckRestartAgent={updateCheckRestartAgent}
          updateCheckVersionUrl={updateCheckVersionUrl}
          updateActivationSha256Hex={updateActivationSha256Hex}
          updateRestartAgent={updateRestartAgent}
          updateRollbackSha256Hex={updateRollbackSha256Hex}
          updateSha256Hex={updateSha256Hex}
          backupIncludeConfig={backupIncludeConfig}
          backupPathsText={backupPathsText}
          shellScript={shellScript}
        />
        <JobTargetSelector
          agents={agents}
          selectorExpression={selectorExpression}
          setSelectorExpression={(value) => {
            setSelectorExpression(value);
            setPreview(null);
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
          detail={`${operationCommandLabel(mode, commandText)} on ${vpsCountLabel(confirmationTargets.length)}.`}
          items={dispatchConfirmationItems}
          onCancel={() => setDispatchPromptOpen(false)}
          onConfirm={() => void dispatchJobNow()}
          open={dispatchPromptOpen}
          pending={pending}
          title="Confirm job dispatch"
          tone={operationNeedsConfirmation ? "danger" : "normal"}
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
              disabled={pending || selectedTargetCount === 0}
              onClick={previewTargets}
              type="button"
            >
              <CheckCircle2 size={17} />
              Review targets
            </button>
            <button
              className="primaryAction"
              disabled={pending || !operationReady || selectedTargetCount === 0 || !privilegeMaterial}
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

function operationRequiresConfirmation(mode: DispatchMode): boolean {
  return (
    mode === "file_push" ||
    mode === "file_transfer_upload" ||
    mode === "file_transfer_download" ||
    mode === "hot_config" ||
    mode === "agent_update" ||
    mode === "agent_update_check" ||
    mode === "agent_update_activate" ||
    mode === "agent_update_rollback" ||
    mode === "backup"
  );
}

function blurActiveElement() {
  if (document.activeElement instanceof HTMLElement) {
    document.activeElement.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    document.activeElement.blur();
  }
}
