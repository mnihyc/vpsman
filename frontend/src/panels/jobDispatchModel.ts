import { parseFileMode, type FilePushPayload } from "../fileTransfer";
import { parseCommandArgv } from "../privilege";
import type { JobOperation } from "../types";
export {
  clampJobMaxTimeoutSecs,
  effectiveJobMaxTimeoutSecs,
  parseOptionalJobMaxTimeoutSecs,
  DEFAULT_MAX_JOB_TIMEOUT_SECS,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "../jobMaxTimeout";

export type DispatchMode =
  | "shell"
  | "shell_script"
  | "terminal_session"
  | "file_pull"
  | "file_push"
  | "file_transfer_upload"
  | "file_transfer_download"
  | "agent_update"
  | "agent_update_check"
  | "agent_update_activate"
  | "agent_update_rollback"
  | "backup"
  | "user_sessions"
  | "process_list"
  | "process_supervisor";

export type SupervisorAction = "start" | "stop" | "restart" | "status" | "logs";
export type TerminalAction = "open";
const MAX_SHELL_SCRIPT_BYTES = 16 * 1024;

export function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
}

export function buildOperation(
  mode: DispatchMode,
  commandText: string,
  shellPty: boolean,
  shellScript: string,
  terminalAction: TerminalAction,
  terminalSessionId: string,
  terminalArgv: string,
  terminalCwd: string,
  terminalUser: string,
  terminalUserPolicy: "fail" | "fallback",
  terminalCols: number,
  terminalRows: number,
  terminalReplayFromSeq: string,
  terminalIdleTimeoutSecs: number,
  terminalFlowWindowBytes: number,
  filePath: string,
  fileFollowSymlinks: boolean,
  processLimit: number,
  supervisorAction: SupervisorAction,
  supervisorName: string,
  supervisorArgv: string,
  supervisorCwd: string,
  supervisorEnv: string,
  supervisorLogBytes: number,
  updateArtifactUrl: string,
  updateSha256Hex: string,
  updateCheckVersionUrl: string,
  updateActivationSha256Hex: string,
  updateRestartAgent: boolean,
  updateRollbackSha256Hex: string,
  backupPathsText: string,
  backupIncludeConfig: boolean,
  backupFollowSymlinks: boolean,
  backupSkipMissingPaths: boolean,
  filePushPath: string,
  filePushMode: string,
  filePushPayload: FilePushPayload | null,
): JobOperation {
  if (mode === "shell_script") {
    const script = shellScript.trim();
    if (!script) {
      throw new Error("Shell script is empty");
    }
    if (new TextEncoder().encode(script).length > MAX_SHELL_SCRIPT_BYTES) {
      throw new Error("Shell script exceeds 16 KiB");
    }
    if (/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(script)) {
      throw new Error("Shell script contains unsupported control characters");
    }
    return { type: "shell_script", script };
  }
  if (mode === "terminal_session") {
    return buildTerminalOperation(
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
    );
  }
  if (mode === "file_pull") {
    if (!filePath.startsWith("/")) {
      throw new Error("File pull path must be absolute");
    }
    return { type: "file_pull", path: filePath, follow_symlinks: fileFollowSymlinks };
  }
  if (mode === "file_push") {
    if (!filePushPath.startsWith("/")) {
      throw new Error("File push path must be absolute");
    }
    if (!filePushPayload) {
      throw new Error("File push source is required");
    }
    const common = {
      path: filePushPath,
      mode: parseFileMode(filePushMode),
      size_bytes: filePushPayload.sizeBytes,
      sha256_hex: filePushPayload.sha256Hex,
      existing_policy: "replace" as const,
      ownership_policy: "fail" as const,
    };
    if (filePushPayload.chunks) {
      return {
        type: "file_push_chunked",
        ...common,
        chunks: filePushPayload.chunks,
      };
    }
    return {
      type: "file_push",
      ...common,
      data_base64: filePushPayload.dataBase64,
    };
  }
  if (mode === "file_transfer_upload") {
    throw new Error("Resumable upload is orchestrated by the browser transfer workflow");
  }
  if (mode === "file_transfer_download") {
    throw new Error("Resumable download is orchestrated by the browser transfer workflow");
  }
  if (mode === "user_sessions") {
    return { type: "user_sessions" };
  }
  if (mode === "agent_update") {
    if (!updateArtifactUrl.startsWith("https://")) {
      throw new Error("Agent update artifact URL must use https://");
    }
    const sha256Hex = updateSha256Hex.trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(sha256Hex)) {
      throw new Error("Agent update SHA-256 must be 64 hex characters");
    }
    return { type: "agent_update", artifact_url: updateArtifactUrl.trim(), sha256_hex: sha256Hex };
  }
  if (mode === "agent_update_check") {
    const versionUrl = updateCheckVersionUrl.trim();
    if (versionUrl && !versionUrl.startsWith("https://") && !versionUrl.startsWith("http://localhost") && !versionUrl.startsWith("http://127.0.0.1") && !versionUrl.startsWith("file://")) {
      throw new Error("Version manifest URL must use https://, localhost http://, or file://");
    }
    return versionUrl
      ? {
          type: "agent_update_check",
          version_url: versionUrl,
          activate: false,
          restart_agent: false,
        }
      : {
          type: "agent_update_check",
          activate: false,
          restart_agent: false,
        };
  }
  if (mode === "agent_update_activate") {
    const stagedSha256Hex = updateActivationSha256Hex.trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(stagedSha256Hex)) {
      throw new Error("Staged update SHA-256 must be 64 hex characters");
    }
    return updateRestartAgent
      ? { type: "agent_update_activate", staged_sha256_hex: stagedSha256Hex, restart_agent: true }
      : { type: "agent_update_activate", staged_sha256_hex: stagedSha256Hex };
  }
  if (mode === "agent_update_rollback") {
    const rollbackSha256Hex = updateRollbackSha256Hex.trim().toLowerCase();
    if (rollbackSha256Hex && !/^[0-9a-f]{64}$/.test(rollbackSha256Hex)) {
      throw new Error("Rollback update SHA-256 must be 64 hex characters");
    }
    return rollbackSha256Hex
      ? { type: "agent_update_rollback", rollback_sha256_hex: rollbackSha256Hex }
      : { type: "agent_update_rollback" };
  }
  if (mode === "process_list") {
    return { type: "process_list", limit: clampInteger(processLimit, 1, 512) };
  }
  if (mode === "backup") {
    const paths = parseBackupPaths(backupPathsText);
    if (!backupIncludeConfig && paths.length === 0) {
      throw new Error("Backup needs selected paths or config");
    }
    return {
      type: "backup",
      paths,
      include_config: backupIncludeConfig,
      follow_symlinks: backupFollowSymlinks,
      missing_path_policy: backupSkipMissingPaths ? "skip" : "fail",
    };
  }
  if (mode === "process_supervisor") {
    return buildSupervisorOperation(
      supervisorAction,
      supervisorName,
      supervisorArgv,
      supervisorCwd,
      supervisorEnv,
      supervisorLogBytes,
    );
  }
  const argv = parseCommandArgv(commandText);
  if (argv.length === 0) {
    throw new Error("Command argv is empty");
  }
  return { type: "shell", argv, pty: shellPty };
}

export function parseBackupPaths(value: string): string[] {
  return Array.from(
    new Set(
      value
        .split(/[\n,]+/)
        .map((path) => path.trim())
        .filter((path) => path.length > 0 && path.startsWith("/")),
    ),
  );
}

export function operationCommandLabel(mode: DispatchMode, commandText: string): string {
  const labels: Record<Exclude<DispatchMode, "shell">, string> = {
    agent_update: "Install agent update",
    agent_update_activate: "Activate staged agent update",
    agent_update_check: "Check agent update",
    agent_update_rollback: "Rollback agent update",
    backup: "Create backup",
    file_pull: "Read remote file",
    file_push: "Write remote file",
    file_transfer_download: "Resumable download",
    file_transfer_upload: "Resumable upload",
    process_list: "List processes",
    process_supervisor: "Managed process",
    shell_script: "Run shell script",
    terminal_session: "Open terminal session",
    user_sessions: "List user sessions",
  };
  return mode === "shell" ? commandText.trim() : labels[mode];
}

export function terminalReady(_action: TerminalAction, sessionId: string, argv: string): boolean {
  if (!sessionId.trim()) {
    return false;
  }
  try {
    const parsed = parseCommandArgv(argv);
    return parsed.length > 0 && parsed[0].startsWith("/");
  } catch {
    return false;
  }
}

function buildTerminalOperation(
  _action: TerminalAction,
  sessionIdInput: string,
  argvInput: string,
  cwdInput: string,
  userInput: string,
  userPolicy: "fail" | "fallback",
  colsInput: number,
  rowsInput: number,
  replayFromSeqInput: string,
  idleTimeoutSecsInput: number,
  flowWindowBytesInput: number,
): JobOperation {
  const sessionId = sessionIdInput.trim();
  if (!/^[0-9a-fA-F-]{36}$/.test(sessionId)) {
    throw new Error("Terminal session id must be a UUID");
  }
  const argv = parseCommandArgv(argvInput);
  if (argv.length === 0 || !argv[0].startsWith("/")) {
    throw new Error("Terminal executable must be an absolute path");
  }
  const cwd = cwdInput.trim();
  const user = userInput.trim();
  const replayFromSeq = replayFromSeqInput.trim();
  return {
    type: "terminal_open",
    session_id: sessionId,
    argv,
    cwd: cwd ? cwd : null,
    user: user ? user : null,
    user_policy: userPolicy,
    cols: clampInteger(colsInput, 20, 240),
    rows: clampInteger(rowsInput, 5, 120),
    ...(replayFromSeq
      ? {
          replay_from_seq: clampInteger(
            Number(replayFromSeq),
            0,
            Number.MAX_SAFE_INTEGER,
          ),
        }
      : {}),
    idle_timeout_secs: clampInteger(idleTimeoutSecsInput, 10, 86400),
    flow_window_bytes: clampInteger(flowWindowBytesInput, 4096, 1024 * 1024),
  };
}

export function supervisorReady(action: SupervisorAction, name: string, argv: string): boolean {
  if (action === "status") {
    return true;
  }
  if (!name.trim()) {
    return false;
  }
  if (action !== "start") {
    return true;
  }
  try {
    return parseCommandArgv(argv).length > 0;
  } catch {
    return false;
  }
}

function buildSupervisorOperation(
  action: SupervisorAction,
  name: string,
  argvInput: string,
  cwdInput: string,
  envInput: string,
  logBytes: number,
): JobOperation {
  const cleanName = name.trim();
  if (action !== "status" && !cleanName) {
    throw new Error("Process name is required");
  }
  if (action === "status") {
    return { type: "process_status", name: cleanName || null };
  }
  if (action === "start") {
    const argv = parseCommandArgv(argvInput);
    if (argv.length === 0 || !argv[0].startsWith("/")) {
      throw new Error("Supervisor executable must be an absolute path");
    }
    const cwd = cwdInput.trim();
    return {
      type: "process_start",
      name: cleanName,
      argv,
      cwd: cwd ? cwd : null,
      env: parseEnvMap(envInput),
    };
  }
  if (action === "stop") {
    return { type: "process_stop", name: cleanName };
  }
  if (action === "restart") {
    return { type: "process_restart", name: cleanName };
  }
  return { type: "process_logs", name: cleanName, max_bytes: clampInteger(logBytes, 1, 524288) };
}

function parseEnvMap(input: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const rawLine of input.split("\n")) {
    const line = rawLine.trim();
    if (!line) {
      continue;
    }
    const separator = line.indexOf("=");
    if (separator <= 0) {
      throw new Error("Environment entries must be KEY=value");
    }
    env[line.slice(0, separator)] = line.slice(separator + 1);
  }
  return env;
}
