import { useEffect, useMemo, useState, type FormEvent } from "react";
import { CheckCircle2, LockKeyhole, Play, ShieldCheck } from "lucide-react";
import { ProofVaultBox } from "../components/ProofVaultBox";
import { readFilePushPayload, sha256Hex } from "../fileTransfer";
import { buildEnvelopesForOperation, deriveSuperKeyHex, parseCommandArgv, type ProofMaterial } from "../proof";
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
  JobHistoryRecord,
  JobOutputRecord,
  JobTargetSelection,
  UpsertCommandTemplateRequest,
  ResourcePoolView,
  TagView,
} from "../types";
import type { FileTransferSourceArtifactRecord } from "../typesFileTransfer";
import type { TerminalSessionRecord } from "../typesTerminal";
import { runPanelAction, shortId } from "../utils";
import { DispatchOptions, JobTargetSelector } from "./JobDispatchControls";
import { JobOperationEditor, OperationModeTabs } from "./jobs/JobOperationControls";
import {
  resolveAgentsById,
  TargetImpactPreview,
  targetImpactModeForDispatch,
} from "./TargetImpactPreview";

export type TerminalComposerAction = {
  action: TerminalAction;
  requestId: string;
  session: TerminalSessionRecord;
};

function randomSaltHex(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function formatArgvForInput(argv: string[]): string {
  return argv.map(shellQuoteArg).join(" ");
}

function shellQuoteArg(value: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(value)) {
    return value;
  }
  return `'${value.replace(/'/g, `'\\''`)}'`;
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
  onResolveTargets,
  onUpsertCommandTemplate,
  pools,
  tags,
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
  onResolveTargets: (selection: JobTargetSelection) => Promise<BulkResolveResponse>;
  onUpsertCommandTemplate: (request: UpsertCommandTemplateRequest) => Promise<CommandTemplateRecord>;
  pools: ResourcePoolView[];
  tags: TagView[];
}) {
  const [mode, setMode] = useState<DispatchMode>("shell");
  const [commandText, setCommandText] = useState("");
  const [shellPty, setShellPty] = useState(false);
  const [shellScript, setShellScript] = useState("");
  const [terminalAction, setTerminalAction] = useState<TerminalAction>("open");
  const [terminalSessionId, setTerminalSessionId] = useState<string>(() => crypto.randomUUID());
  const [terminalArgv, setTerminalArgv] = useState(DEFAULT_TERMINAL_ARGV);
  const [terminalCwd, setTerminalCwd] = useState("");
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
  const [fileTransferMultiTargetPolicy, setFileTransferMultiTargetPolicy] =
    useState<BrowserTransferMultiTargetPolicy>("same-offset");
  const [selectedTemplateId, setSelectedTemplateId] = useState("");
  const [templateName, setTemplateName] = useState("");
  const [templateScopeKind, setTemplateScopeKind] = useState<"global" | "provider" | "pool" | "tag" | "client">("global");
  const [templateScopeValue, setTemplateScopeValue] = useState("");
  const [templatePending, setTemplatePending] = useState(false);
  const [hotConfigToml, setHotConfigToml] = useState("");
  const [rotationPassword, setRotationPassword] = useState("");
  const [rotationSaltHex, setRotationSaltHex] = useState(() => randomSaltHex());
  const [rotationGeneration, setRotationGeneration] = useState("");
  const [updateArtifactUrl, setUpdateArtifactUrl] = useState("");
  const [updateSha256Hex, setUpdateSha256Hex] = useState("");
  const [updateArtifactSignatureHex, setUpdateArtifactSignatureHex] = useState("");
  const [updateArtifactSigningKeyHex, setUpdateArtifactSigningKeyHex] = useState("");
  const [updateCanaryCount, setUpdateCanaryCount] = useState(1);
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
  const [selectedClients, setSelectedClients] = useState<string[]>([]);
  const [selectedPools, setSelectedPools] = useState<string[]>([]);
  const [selectedTags, setSelectedTags] = useState<string[]>([]);
  const [tagMode, setTagMode] = useState<"any" | "all">("any");
  const [timeoutSecs, setTimeoutSecs] = useState(30);
  const [proofTtlSecs, setProofTtlSecs] = useState(300);
  const [destructive, setDestructive] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [proofMaterial, setProofMaterial] = useState<ProofMaterial | null>(null);
  const [preview, setPreview] = useState<BulkResolveResponse | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [transferProgress, setTransferProgress] = useState<ResumableUploadProgress | ResumableDownloadProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

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
    setSelectedClients([session.client_id]);
    setSelectedPools([]);
    setSelectedTags([]);
    setPreview(null);
    setActionError(null);
  }, [terminalComposerAction]);

  const parsedArgv = useMemo(() => {
    try {
      return parseCommandArgv(commandText);
    } catch {
      return [];
    }
  }, [commandText]);

  const filePullReady = filePath.startsWith("/");
  const filePushReady = filePushPath.startsWith("/") && !!filePushSource && confirmed;
  const fileTransferUploadReady =
    filePushPath.startsWith("/") &&
    confirmed &&
    (fileTransferUploadSourceKind === "local-file" ? !!filePushSource : !!fileTransferSourceArtifactId);
  const fileTransferDownloadReady = filePath.startsWith("/") && confirmed;
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
                    ? hotConfigToml.trim().length > 0 && confirmed
                    : mode === "auth_rotate"
                      ? rotationPassword.length > 0 &&
                        rotationSaltHex.trim().length > 0 &&
                        rotationSaltHex.trim().length % 2 === 0 &&
                        /^[0-9a-fA-F]+$/.test(rotationSaltHex.trim()) &&
                        confirmed
                      : mode === "agent_update"
                        ? updateArtifactUrl.startsWith("https://") &&
                          /^[0-9a-fA-F]{64}$/.test(updateSha256Hex.trim()) &&
                          updateSignatureReady &&
                          confirmed
                        : mode === "agent_update_activate"
                          ? /^[0-9a-fA-F]{64}$/.test(updateActivationSha256Hex.trim()) && confirmed
                          : mode === "agent_update_rollback"
                            ? (!updateRollbackSha256Hex.trim() ||
                                /^[0-9a-fA-F]{64}$/.test(updateRollbackSha256Hex.trim())) &&
                              confirmed
                            : mode === "process_supervisor"
                              ? supervisorReady(supervisorAction, supervisorName, supervisorArgv)
                              : mode === "backup"
                                ? backupReady && confirmed
                                : true;
  const selectedTargetCount = selectedClients.length + selectedPools.length + selectedTags.length;
  const impactMode = targetImpactModeForDispatch(mode);
  const supportsForceUnprivileged = impactMode !== "generic";
  const impactTargets = preview?.targets ?? resolveAgentsById(agents, selectedClients);
  const status =
    actionError ??
    (lastJob
      ? `Job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.accepted_targets} accepted`
      : preview
        ? `${preview.target_count} resolved targets`
        : proofMaterial
          ? "Proof unlocked"
          : "Proof locked");

  function lockProof() {
    setProofMaterial(null);
    setActionError(null);
  }

  async function previewTargets() {
    await runPanelAction(setPending, setActionError, async () => {
      setPreview(await onResolveTargets(targetSelection()));
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
    setTemplateScopeKind(template.scope_kind as "global" | "provider" | "pool" | "tag" | "client");
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
        "",
        rotationGeneration,
        updateArtifactUrl,
        updateSha256Hex,
        updateArtifactSignatureHex,
        updateArtifactSigningKeyHex,
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
          confirmed,
          destructive,
          force_unprivileged: supportsForceUnprivileged ? forceUnprivileged : false,
          timeout_secs: clampInteger(timeoutSecs, 1, 3600),
        },
        confirmed: true,
      });
    });
  }

  async function submitJob(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      setTransferProgress(null);
      if (mode === "file_transfer_upload") {
        const resolved = await onResolveTargets(targetSelection());
        setPreview(resolved);
        if (resolved.confirmation_required) {
          throw new Error("Resumable upload requires confirmation");
        }
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
          confirmed,
          createJob: onCreateJob,
          file: uploadSourceFile,
          loadJob: onLoadJob,
          loadOutputs: onLoadOutputs,
          modeText: filePushMode,
          multiTargetPolicy: fileTransferMultiTargetPolicy,
          path: filePushPath,
          proofMaterial,
          proofTtlSecs,
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
        return;
      }
      if (mode === "file_transfer_download") {
        const resolved = await onResolveTargets(targetSelection());
        setPreview(resolved);
        if (resolved.confirmation_required) {
          throw new Error("Resumable download requires confirmation");
        }
        const clientIds = resolved.targets.map((target) => target.id);
        const startJob = await runBrowserResumableDownload({
          clientIds,
          confirmed,
          createJob: onCreateJob,
          downloadName: fileTransferDownloadName,
          downloadSink: fileTransferDownloadSink,
          downloadOutputArtifact: onDownloadOutputArtifact,
          loadJob: onLoadJob,
          loadOutputs: onLoadOutputs,
          path: filePath,
          proofMaterial,
          proofTtlSecs,
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
        return;
      }
      const filePushPayload = mode === "file_push" ? await readFilePushPayload(filePushSource) : null;
      const rotationProofKeyHex =
        mode === "auth_rotate" ? await deriveSuperKeyHex(rotationPassword, rotationSaltHex) : "";
      const operation = buildOperation(
        mode,
        commandText,
        shellPty,
        shellScript,
        terminalAction,
        terminalSessionId,
        terminalArgv,
        terminalCwd,
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
        rotationProofKeyHex,
        rotationGeneration,
        updateArtifactUrl,
        updateSha256Hex,
        updateArtifactSignatureHex,
        updateArtifactSigningKeyHex,
        updateActivationSha256Hex,
        updateRestartAgent,
        updateRollbackSha256Hex,
        backupPathsText,
        backupIncludeConfig,
        filePushPath,
        filePushMode,
        filePushPayload,
      );
      if (
        (mode === "hot_config" ||
          mode === "agent_update" ||
          mode === "agent_update_activate" ||
          mode === "agent_update_rollback" ||
          mode === "auth_rotate" ||
          mode === "file_push" ||
          mode === "backup") &&
        !confirmed
      ) {
        throw new Error(`${operationCommandLabel(mode, commandText)} dispatch requires confirmation`);
      }
      const resolved = await onResolveTargets(targetSelection());
      setPreview(resolved);
      if (resolved.confirmation_required) {
        throw new Error("Destructive dispatch requires confirmation");
      }
      const clientIds = resolved.targets.map((target) => target.id);
      const built = await buildEnvelopesForOperation({
        clientIds,
        operation,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      const nextJob = await onCreateJob({
        clients: selectedClients,
        pools: selectedPools,
        tags: selectedTags,
        tag_mode: tagMode,
        destructive,
        confirmed,
        command: operationCommandLabel(mode, commandText),
        argv: mode === "shell" && operation.type === "shell" ? operation.argv : [],
        operation,
        timeout_secs: clampInteger(timeoutSecs, 1, 3600),
        canary_count: clampInteger(updateCanaryCount, 0, 10000) || null,
        force_unprivileged: supportsForceUnprivileged ? forceUnprivileged : false,
        privileged: true,
        idempotency_key: `panel:${mode}:${built.payloadHashHex.slice(0, 16)}:${clientIds.join(".").slice(0, 72)}`,
        reconnect_policy: {
          duplicate_delivery: "ignore_completed",
          resume_outputs: true,
          cancel_on_disconnect: false,
        },
        envelope: null,
        envelopes: built.envelopes,
      });
      setLastJob(nextJob);
      setLastPayloadHash(built.payloadHashHex);
    });
  }

  function targetSelection(): JobTargetSelection {
    return {
      clients: selectedClients,
      pools: selectedPools,
      tags: selectedTags,
      tag_mode: tagMode,
      destructive,
      confirmed,
    };
  }

  return (
    <section className="fleetPanel commandComposer">
      <div className="sectionHeader">
        <div>
          <h2>Dispatch command</h2>
          <span>{status}</span>
        </div>
        {proofMaterial ? (
          <button className="secondaryAction" onClick={lockProof} type="button">
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
              placeholder="pool-health-check"
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
              <option value="pool">Pool</option>
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
              placeholder={templateScopeKind === "pool" ? "hetzner-fsn1" : templateScopeKind}
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
          rotationGeneration={rotationGeneration}
          rotationPassword={rotationPassword}
          rotationSaltHex={rotationSaltHex}
          setCommandText={setCommandText}
          setShellPty={setShellPty}
          setShellScript={setShellScript}
          setTerminalAction={setTerminalAction}
          setTerminalArgv={setTerminalArgv}
          setTerminalCloseReason={setTerminalCloseReason}
          setTerminalCols={setTerminalCols}
          setTerminalCwd={setTerminalCwd}
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
          setFileTransferMultiTargetPolicy={setFileTransferMultiTargetPolicy}
          setFileTransferRateLimit={setFileTransferRateLimit}
          setFileTransferResumeToken={setFileTransferResumeToken}
          setFileTransferSessionId={setFileTransferSessionId}
          setHotConfigToml={setHotConfigToml}
          setProcessLimit={setProcessLimit}
          setRotationGeneration={setRotationGeneration}
          setRotationPassword={setRotationPassword}
          setRotationSaltHex={setRotationSaltHex}
          setSupervisorAction={setSupervisorAction}
          setSupervisorArgv={setSupervisorArgv}
          setSupervisorCwd={setSupervisorCwd}
          setSupervisorEnv={setSupervisorEnv}
          setSupervisorLogBytes={setSupervisorLogBytes}
          setSupervisorName={setSupervisorName}
          setUpdateArtifactSignatureHex={setUpdateArtifactSignatureHex}
          setUpdateArtifactSigningKeyHex={setUpdateArtifactSigningKeyHex}
          setUpdateArtifactUrl={setUpdateArtifactUrl}
          setUpdateActivationSha256Hex={setUpdateActivationSha256Hex}
          setUpdateRestartAgent={setUpdateRestartAgent}
          setUpdateRollbackSha256Hex={setUpdateRollbackSha256Hex}
          setUpdateCanaryCount={setUpdateCanaryCount}
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
          updateActivationSha256Hex={updateActivationSha256Hex}
          updateRestartAgent={updateRestartAgent}
          updateRollbackSha256Hex={updateRollbackSha256Hex}
          updateCanaryCount={updateCanaryCount}
          updateSha256Hex={updateSha256Hex}
          backupIncludeConfig={backupIncludeConfig}
          backupPathsText={backupPathsText}
          shellScript={shellScript}
        />
        <JobTargetSelector
          agents={agents}
          pools={pools}
          selectedClients={selectedClients}
          selectedPools={selectedPools}
          selectedTags={selectedTags}
          setSelectedClients={setSelectedClients}
          setSelectedPools={setSelectedPools}
          setSelectedTags={setSelectedTags}
          setTagMode={setTagMode}
          tagMode={tagMode}
          tags={tags}
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
          canaryCount={updateCanaryCount}
          confirmed={confirmed}
          destructive={destructive}
          proofTtlSecs={proofTtlSecs}
          setCanaryCount={setUpdateCanaryCount}
          setConfirmed={setConfirmed}
          setDestructive={setDestructive}
          setProofTtlSecs={setProofTtlSecs}
          setTimeoutSecs={setTimeoutSecs}
          timeoutSecs={timeoutSecs}
        />

        <div className="dispatchActions">
          <button className="secondaryAction" disabled={pending || selectedTargetCount === 0} onClick={previewTargets} type="button">
            <CheckCircle2 size={17} />
            Preview
          </button>
          <button
            className="primaryAction"
            disabled={pending || !operationReady || selectedTargetCount === 0 || !proofMaterial}
            type="submit"
          >
            <Play size={17} />
            Dispatch
          </button>
        </div>
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

      <ProofVaultBox lastPayloadHash={lastPayloadHash} onProofMaterialChange={setProofMaterial} proofMaterial={proofMaterial} />
    </section>
  );
}
