import { Activity, DatabaseBackup, Download, PackageCheck, Settings2, Upload } from "lucide-react";
import { FILE_TRANSFER_CHUNK_BYTES, MAX_CHUNKED_FILE_PUSH_BYTES } from "../../fileTransfer";
import {
  COMMAND_ARGV_PLACEHOLDER,
  FILE_PULL_PATH_PLACEHOLDER,
  JOB_BACKUP_PATHS_PLACEHOLDER,
  SUPERVISOR_COMMAND_PLACEHOLDER,
} from "../../presets/jobOperationPresets";
import {
  MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES,
  MAX_BROWSER_STREAMING_RESUMABLE_DOWNLOAD_BYTES,
  MAX_BROWSER_RESUMABLE_UPLOAD_BYTES,
} from "../../resumableFileTransfer";
import type { BrowserDownloadSinkMode, BrowserTransferMultiTargetPolicy } from "../../resumableFileTransfer";
import type { FileExistingPolicy } from "../../types";
import type { FileTransferSourceArtifactRecord } from "../../typesFileTransfer";
import type { DispatchMode, SupervisorAction, TerminalAction } from "../jobDispatchModel";
import { TerminalOperationControls } from "./TerminalOperationControls";

export function OperationModeTabs({
  mode,
  onModeChange,
}: {
  mode: DispatchMode;
  onModeChange: (mode: DispatchMode) => void;
}) {
  const modes: { label: string; mode: DispatchMode }[] = [
    { label: "Argv", mode: "shell" },
    { label: "Shell", mode: "shell_script" },
    { label: "Terminal", mode: "terminal_session" },
    { label: "File pull", mode: "file_pull" },
    { label: "File push", mode: "file_push" },
    { label: "Resumable upload", mode: "file_transfer_upload" },
    { label: "Resumable download", mode: "file_transfer_download" },
    { label: "Full config", mode: "hot_config" },
    { label: "Manual update", mode: "agent_update" },
    { label: "Check update", mode: "agent_update_check" },
    { label: "Activate", mode: "agent_update_activate" },
    { label: "Rollback", mode: "agent_update_rollback" },
    { label: "Backup", mode: "backup" },
    { label: "Sessions", mode: "user_sessions" },
    { label: "Processes", mode: "process_list" },
    { label: "Supervisor", mode: "process_supervisor" },
  ];

  return (
    <div className="segmented">
      {modes.map((item) => (
        <button
          className={mode === item.mode ? "selected" : ""}
          key={item.mode}
          onClick={() => onModeChange(item.mode)}
          type="button"
        >
          {item.label}
        </button>
      ))}
    </div>
  );
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

export function JobOperationEditor({
  backupIncludeConfig,
  backupPathsText,
  commandText,
  shellPty,
  terminalAction,
  terminalArgv,
  terminalCloseReason,
  terminalCols,
  terminalCwd,
  terminalUser,
  terminalUserPolicy,
  terminalFlowWindowBytes,
  terminalIdleTimeoutSecs,
  terminalInputSeq,
  terminalInputText,
  terminalReplayFromSeq,
  terminalRows,
  terminalSessionId,
  filePath,
  fileFollowSymlinks,
  filePushMode,
  filePushPath,
  fileTransferDownloadSink,
  fileTransferDownloadName,
  fileTransferChunkSize,
  fileTransferExistingPolicy,
  fileTransferMultiTargetPolicy,
  fileTransferRateLimit,
  fileTransferResumeToken,
  fileTransferSessionId,
  fileTransferSourceArtifactId,
  fileTransferSources,
  fileTransferUploadSourceKind,
  hotConfigToml,
  mode,
  processLimit,
  setBackupIncludeConfig,
  setBackupPathsText,
  setCommandText,
  setShellPty,
  setShellScript,
  setTerminalAction,
  setTerminalArgv,
  setTerminalCloseReason,
  setTerminalCols,
  setTerminalCwd,
  setTerminalUser,
  setTerminalUserPolicy,
  setTerminalFlowWindowBytes,
  setTerminalIdleTimeoutSecs,
  setTerminalInputSeq,
  setTerminalInputText,
  setTerminalReplayFromSeq,
  setTerminalRows,
  setTerminalSessionId,
  setFilePath,
  setFileFollowSymlinks,
  setFilePushMode,
  setFilePushPath,
  setFilePushSource,
  setFileTransferDownloadSink,
  setFileTransferDownloadName,
  setFileTransferChunkSize,
  setFileTransferExistingPolicy,
  setFileTransferMultiTargetPolicy,
  setFileTransferRateLimit,
  setFileTransferResumeToken,
  setFileTransferSessionId,
  setFileTransferSourceArtifactId,
  setFileTransferUploadSourceKind,
  setHotConfigToml,
  setProcessLimit,
  setSupervisorAction,
  setSupervisorArgv,
  setSupervisorCwd,
  setSupervisorEnv,
  setSupervisorLogBytes,
  setSupervisorName,
  setUpdateArtifactUrl,
  setUpdateCheckActivate,
  setUpdateCheckRestartAgent,
  setUpdateCheckVersionUrl,
  setUpdateActivationSha256Hex,
  setUpdateRestartAgent,
  setUpdateRollbackSha256Hex,
  setUpdateSha256Hex,
  supervisorAction,
  supervisorArgv,
  supervisorCwd,
  supervisorEnv,
  supervisorLogBytes,
  supervisorName,
  shellScript,
  updateArtifactUrl,
  updateCheckActivate,
  updateCheckRestartAgent,
  updateCheckVersionUrl,
  updateActivationSha256Hex,
  updateRestartAgent,
  updateRollbackSha256Hex,
  updateSha256Hex,
}: {
  backupIncludeConfig: boolean;
  backupPathsText: string;
  commandText: string;
  shellPty: boolean;
  terminalAction: TerminalAction;
  terminalArgv: string;
  terminalCloseReason: string;
  terminalCols: number;
  terminalCwd: string;
  terminalUser: string;
  terminalUserPolicy: "fail" | "fallback";
  terminalFlowWindowBytes: number;
  terminalIdleTimeoutSecs: number;
  terminalInputSeq: number;
  terminalInputText: string;
  terminalReplayFromSeq: string;
  terminalRows: number;
  terminalSessionId: string;
  filePath: string;
  fileFollowSymlinks: boolean;
  filePushMode: string;
  filePushPath: string;
  fileTransferDownloadSink: BrowserDownloadSinkMode;
  fileTransferDownloadName: string;
  fileTransferChunkSize: number;
  fileTransferExistingPolicy: FileExistingPolicy;
  fileTransferMultiTargetPolicy: BrowserTransferMultiTargetPolicy;
  fileTransferRateLimit: number;
  fileTransferResumeToken: string;
  fileTransferSessionId: string;
  fileTransferSourceArtifactId: string;
  fileTransferSources: FileTransferSourceArtifactRecord[];
  fileTransferUploadSourceKind: "local-file" | "source-artifact";
  hotConfigToml: string;
  mode: DispatchMode;
  processLimit: number;
  setBackupIncludeConfig: (value: boolean) => void;
  setBackupPathsText: (value: string) => void;
  setCommandText: (value: string) => void;
  setShellPty: (value: boolean) => void;
  setShellScript: (value: string) => void;
  setTerminalAction: (value: TerminalAction) => void;
  setTerminalArgv: (value: string) => void;
  setTerminalCloseReason: (value: string) => void;
  setTerminalCols: (value: number) => void;
  setTerminalCwd: (value: string) => void;
  setTerminalUser: (value: string) => void;
  setTerminalUserPolicy: (value: "fail" | "fallback") => void;
  setTerminalFlowWindowBytes: (value: number) => void;
  setTerminalIdleTimeoutSecs: (value: number) => void;
  setTerminalInputSeq: (value: number) => void;
  setTerminalInputText: (value: string) => void;
  setTerminalReplayFromSeq: (value: string) => void;
  setTerminalRows: (value: number) => void;
  setTerminalSessionId: (value: string) => void;
  setFilePath: (value: string) => void;
  setFileFollowSymlinks: (value: boolean) => void;
  setFilePushMode: (value: string) => void;
  setFilePushPath: (value: string) => void;
  setFilePushSource: (value: File | null) => void;
  setFileTransferDownloadSink: (value: BrowserDownloadSinkMode) => void;
  setFileTransferDownloadName: (value: string) => void;
  setFileTransferChunkSize: (value: number) => void;
  setFileTransferExistingPolicy: (value: FileExistingPolicy) => void;
  setFileTransferMultiTargetPolicy: (value: BrowserTransferMultiTargetPolicy) => void;
  setFileTransferRateLimit: (value: number) => void;
  setFileTransferResumeToken: (value: string) => void;
  setFileTransferSessionId: (value: string) => void;
  setFileTransferSourceArtifactId: (value: string) => void;
  setFileTransferUploadSourceKind: (value: "local-file" | "source-artifact") => void;
  setHotConfigToml: (value: string) => void;
  setProcessLimit: (value: number) => void;
  setSupervisorAction: (value: SupervisorAction) => void;
  setSupervisorArgv: (value: string) => void;
  setSupervisorCwd: (value: string) => void;
  setSupervisorEnv: (value: string) => void;
  setSupervisorLogBytes: (value: number) => void;
  setSupervisorName: (value: string) => void;
  setUpdateArtifactUrl: (value: string) => void;
  setUpdateCheckActivate: (value: boolean) => void;
  setUpdateCheckRestartAgent: (value: boolean) => void;
  setUpdateCheckVersionUrl: (value: string) => void;
  setUpdateActivationSha256Hex: (value: string) => void;
  setUpdateRestartAgent: (value: boolean) => void;
  setUpdateRollbackSha256Hex: (value: string) => void;
  setUpdateSha256Hex: (value: string) => void;
  supervisorAction: SupervisorAction;
  supervisorArgv: string;
  supervisorCwd: string;
  supervisorEnv: string;
  supervisorLogBytes: number;
  supervisorName: string;
  shellScript: string;
  updateArtifactUrl: string;
  updateCheckActivate: boolean;
  updateCheckRestartAgent: boolean;
  updateCheckVersionUrl: string;
  updateActivationSha256Hex: string;
  updateRestartAgent: boolean;
  updateRollbackSha256Hex: string;
  updateSha256Hex: string;
}) {
  if (mode === "shell") {
    return (
      <div className="compactOperation shellOperation">
        <label className="wideField">
          <span>Command argv</span>
          <textarea
            aria-label="Command argv"
            onChange={(event) => setCommandText(event.target.value)}
            placeholder={COMMAND_ARGV_PLACEHOLDER}
            rows={3}
            value={commandText}
          />
        </label>
        <label className="checkRow">
          <input
            checked={shellPty}
            onChange={(event) => setShellPty(event.target.checked)}
            type="checkbox"
          />
          <span>PTY</span>
        </label>
      </div>
    );
  }

  if (mode === "shell_script") {
    return (
      <label>
        <span>Shell script</span>
        <textarea
          aria-label="Shell script"
          onChange={(event) => setShellScript(event.target.value)}
          placeholder="set -eu&#10;hostname&#10;uptime"
          rows={5}
          value={shellScript}
        />
      </label>
    );
  }

  if (mode === "terminal_session") {
    return (
      <TerminalOperationControls
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
      />
    );
  }

  if (mode === "file_pull") {
    return (
      <div className="compactOperation filePathOperation">
        <label className="wideField">
          <span>Absolute path</span>
          <input
            aria-label="File pull path"
            onChange={(event) => setFilePath(event.target.value)}
            placeholder={FILE_PULL_PATH_PLACEHOLDER}
            value={filePath}
          />
        </label>
        <label
          className="checkLine inlineCheck fileOptionCheck"
          title="Disabled by default. Enable only when the reviewed path is intentionally a symlink and the target should be read."
        >
          <input
            checked={fileFollowSymlinks}
            onChange={(event) => setFileFollowSymlinks(event.target.checked)}
            type="checkbox"
          />
          <span>Follow symlinks</span>
        </label>
      </div>
    );
  }

  if (mode === "file_push") {
    return (
      <div className="operationNote compactOperation">
        <Upload size={18} />
        <div>
          <strong>File push</strong>
          <span>Privilege-unlocked, chunk-hashed, atomic agent write up to {MAX_CHUNKED_FILE_PUSH_BYTES} bytes</span>
        </div>
        <label className="wideField">
          <span>Source file</span>
          <input
            aria-label="File push source"
            onChange={(event) => setFilePushSource(event.target.files?.[0] ?? null)}
            type="file"
          />
        </label>
        <label className="wideField">
          <span>Remote path</span>
          <input
            aria-label="File push path"
            onChange={(event) => setFilePushPath(event.target.value)}
            placeholder="/tmp/vpsman-upload.txt"
            value={filePushPath}
          />
        </label>
        <label>
          <span>Mode</span>
          <input aria-label="File push mode" onChange={(event) => setFilePushMode(event.target.value)} value={filePushMode} />
        </label>
      </div>
    );
  }

  if (mode === "file_transfer_upload") {
    return (
      <div className="operationNote compactOperation">
        <Upload size={18} />
        <div>
          <strong>Resumable upload</strong>
          <span>Streamed ACK-tracked browser upload up to {formatBytes(MAX_BROWSER_RESUMABLE_UPLOAD_BYTES)}</span>
        </div>
        <label>
          <span>Source kind</span>
          <select
            aria-label="Resumable upload producer"
            onChange={(event) => setFileTransferUploadSourceKind(event.target.value as "local-file" | "source-artifact")}
            value={fileTransferUploadSourceKind}
          >
            <option value="local-file">Local file</option>
            <option value="source-artifact">Source artifact</option>
          </select>
        </label>
        {fileTransferUploadSourceKind === "source-artifact" ? (
          <label className="wideField">
            <span>Source artifact</span>
            <select
              aria-label="Resumable upload source artifact"
              onChange={(event) => setFileTransferSourceArtifactId(event.target.value)}
              value={fileTransferSourceArtifactId}
            >
              <option value="">Select artifact</option>
              {fileTransferSources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.name} · {formatBytes(source.size_bytes)}
                </option>
              ))}
            </select>
          </label>
        ) : (
        <label className="wideField">
          <span>Source file</span>
          <input
            aria-label="Resumable upload source"
            onChange={(event) => setFilePushSource(event.target.files?.[0] ?? null)}
            type="file"
          />
        </label>
        )}
        <label className="wideField">
          <span>Remote path</span>
          <input
            aria-label="Resumable upload path"
            onChange={(event) => setFilePushPath(event.target.value)}
            placeholder="/tmp/vpsman-large-upload.bin"
            value={filePushPath}
          />
        </label>
        <label>
          <span>Mode</span>
          <input
            aria-label="Resumable upload mode"
            onChange={(event) => setFilePushMode(event.target.value)}
            value={filePushMode}
          />
        </label>
        <label>
          <span>Chunk bytes</span>
          <input
            aria-label="Resumable upload chunk bytes"
            max={FILE_TRANSFER_CHUNK_BYTES}
            min={1}
            onChange={(event) => setFileTransferChunkSize(Number(event.target.value))}
            type="number"
            value={fileTransferChunkSize}
          />
        </label>
        <label>
          <span>Rate kbps</span>
          <input
            aria-label="Resumable upload rate limit"
            min={0}
            onChange={(event) => setFileTransferRateLimit(Number(event.target.value))}
            type="number"
            value={fileTransferRateLimit}
          />
        </label>
        <label>
          <span>Existing file</span>
          <select
            aria-label="Resumable upload existing-file policy"
            onChange={(event) => setFileTransferExistingPolicy(event.target.value as FileExistingPolicy)}
            value={fileTransferExistingPolicy}
          >
            <option value="skip">Skip</option>
            <option value="replace">Replace</option>
          </select>
        </label>
        <label>
          <span>Policy</span>
          <select
            aria-label="Resumable upload multi-target policy"
            onChange={(event) => setFileTransferMultiTargetPolicy(event.target.value as BrowserTransferMultiTargetPolicy)}
            value={fileTransferMultiTargetPolicy}
          >
            <option value="same-offset">same-offset</option>
            <option value="independent-offsets">independent-offsets</option>
          </select>
        </label>
        <label className="wideField">
          <span>Session</span>
          <input
            aria-label="Resumable upload session"
            onChange={(event) => setFileTransferSessionId(event.target.value)}
            placeholder="auto"
            value={fileTransferSessionId}
          />
        </label>
        <label className="wideField">
          <span>Resume token</span>
          <input
            aria-label="Resumable upload resume token"
            onChange={(event) => setFileTransferResumeToken(event.target.value)}
            placeholder="auto"
            value={fileTransferResumeToken}
          />
        </label>
      </div>
    );
  }

  if (mode === "file_transfer_download") {
    return (
      <div className="operationNote compactOperation fileTransferDownloadOperation">
        <div className="fileTransferOperationHeader">
          <Download size={18} />
          <div>
            <strong>Resumable download</strong>
            <span>
              Browser download up to {formatBytes(MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES)}; stream-to-file up to{" "}
              {formatBytes(MAX_BROWSER_STREAMING_RESUMABLE_DOWNLOAD_BYTES)}
            </span>
          </div>
        </div>
        <label className="wideField">
          <span>Remote path</span>
          <input
            aria-label="Resumable download path"
            onChange={(event) => setFilePath(event.target.value)}
            placeholder="/tmp/vpsman-large-download.bin"
            value={filePath}
          />
        </label>
        <label
          className="checkLine inlineCheck fileOptionCheck"
          title="Disabled by default. Enable only when the reviewed remote path is intentionally a symlink and the download should read its target."
        >
          <input
            checked={fileFollowSymlinks}
            onChange={(event) => setFileFollowSymlinks(event.target.checked)}
            type="checkbox"
          />
          <span>Follow symlinks</span>
        </label>
        <label className="wideField">
          <span>Browser filename</span>
          <input
            aria-label="Resumable download filename"
            onChange={(event) => setFileTransferDownloadName(event.target.value)}
            placeholder="auto from remote path"
            value={fileTransferDownloadName}
          />
        </label>
        <label>
          <span>Chunk bytes</span>
          <input
            aria-label="Resumable download chunk bytes"
            max={FILE_TRANSFER_CHUNK_BYTES}
            min={1}
            onChange={(event) => setFileTransferChunkSize(Number(event.target.value))}
            type="number"
            value={fileTransferChunkSize}
          />
        </label>
        <label>
          <span>Rate kbps</span>
          <input
            aria-label="Resumable download rate limit"
            min={0}
            onChange={(event) => setFileTransferRateLimit(Number(event.target.value))}
            type="number"
            value={fileTransferRateLimit}
          />
        </label>
        <label className="wideField">
          <span>Save method</span>
          <select
            aria-label="Resumable download save method"
            onChange={(event) => setFileTransferDownloadSink(event.target.value as BrowserDownloadSinkMode)}
            value={fileTransferDownloadSink}
          >
            <option value="browser-download">Browser download</option>
            <option value="stream-to-file">Stream to file</option>
          </select>
        </label>
        <label className="wideField">
          <span>Session</span>
          <input
            aria-label="Resumable download session"
            onChange={(event) => setFileTransferSessionId(event.target.value)}
            placeholder="auto"
            value={fileTransferSessionId}
          />
        </label>
        <label className="wideField">
          <span>Resume token</span>
          <input
            aria-label="Resumable download resume token"
            onChange={(event) => setFileTransferResumeToken(event.target.value)}
            placeholder="auto"
            value={fileTransferResumeToken}
          />
        </label>
      </div>
    );
  }

  if (mode === "user_sessions") {
    return (
      <div className="operationNote">
        <strong>User sessions</strong>
        <span>Source: w/who on selected VPSs</span>
      </div>
    );
  }

  if (mode === "hot_config") {
    return (
      <div className="operationNote compactOperation">
        <Settings2 size={18} />
        <div>
          <strong>Full agent config override</strong>
          <span>Replaces the agent config after validation and writes a rollback file</span>
        </div>
        <label className="wideField">
          <span>TOML</span>
          <textarea
            aria-label="Full override config TOML"
            onChange={(event) => setHotConfigToml(event.target.value)}
            placeholder={`client_id = "edge-a"\ndisplay_name = "edge-a"`}
            rows={6}
            value={hotConfigToml}
          />
        </label>
      </div>
    );
  }

  if (mode === "process_list") {
    return (
      <div className="operationNote compactOperation">
        <Activity size={18} />
        <div>
          <strong>Process snapshot</strong>
          <span>Privilege-unlocked process source sorted by RSS</span>
        </div>
        <label>
          <span>Limit</span>
          <input
            aria-label="Process list limit"
            max={512}
            min={1}
            onChange={(event) => setProcessLimit(Number(event.target.value))}
            type="number"
            value={processLimit}
          />
        </label>
      </div>
    );
  }

  if (mode === "backup") {
    return (
      <div className="operationNote compactOperation">
        <DatabaseBackup size={18} />
        <div>
          <strong>Encrypted backup</strong>
          <span>Agent encrypts selected regular files/config to its configured backup recipient key</span>
        </div>
        <label className="wideField">
          <span>Selected paths</span>
          <textarea
            aria-label="Backup selected paths"
            onChange={(event) => setBackupPathsText(event.target.value)}
            placeholder={JOB_BACKUP_PATHS_PLACEHOLDER}
            rows={4}
            value={backupPathsText}
          />
        </label>
        <label className="checkLine inlineCheck">
          <input
            checked={backupIncludeConfig}
            onChange={(event) => setBackupIncludeConfig(event.target.checked)}
            type="checkbox"
          />
          <span>Include agent config</span>
        </label>
      </div>
    );
  }

  if (mode === "agent_update") {
    return (
      <div className="operationNote compactOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Agent binary</strong>
          <span>HTTPS artifact staged side-by-side after SHA-256 verification</span>
        </div>
        <label className="wideField">
          <span>Artifact URL</span>
          <input
            aria-label="Agent update artifact URL"
            onChange={(event) => setUpdateArtifactUrl(event.target.value)}
            placeholder="https://updates.example/vpsman-agent"
            value={updateArtifactUrl}
          />
        </label>
        <label className="wideField">
          <span>SHA-256</span>
          <input
            aria-label="Agent update SHA-256"
            onChange={(event) => setUpdateSha256Hex(event.target.value)}
            placeholder="64 hex characters"
            value={updateSha256Hex}
          />
        </label>
      </div>
    );
  }

  if (mode === "agent_update_check") {
    return (
      <div className="operationNote compactOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Version manifest</strong>
          <span>Fetches version.json, uses its tag-pinned asset URL, verifies SHA256SUMS, then optionally activates</span>
        </div>
        <label className="wideField">
          <span>Manifest URL</span>
          <input
            aria-label="Agent update version manifest URL"
            onChange={(event) => setUpdateCheckVersionUrl(event.target.value)}
            placeholder="https://github.com/mnihyc/vpsman/releases/latest/download/version.json"
            value={updateCheckVersionUrl}
          />
        </label>
        <label className="checkRow">
          <input
            checked={updateCheckActivate}
            onChange={(event) => setUpdateCheckActivate(event.target.checked)}
            type="checkbox"
          />
          <span>Activate if newer</span>
        </label>
        <label className="checkRow">
          <input
            checked={updateCheckRestartAgent}
            disabled={!updateCheckActivate}
            onChange={(event) => setUpdateCheckRestartAgent(event.target.checked)}
            type="checkbox"
          />
          <span>Restart agent</span>
        </label>
      </div>
    );
  }

  if (mode === "agent_update_activate") {
    return (
      <div className="operationNote compactOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Activate staged agent</strong>
          <span>Promotes the verified side-by-side artifact and keeps rollback copy for restart recovery</span>
        </div>
        <label className="wideField">
          <span>Staged SHA-256</span>
          <input
            aria-label="Agent update staged SHA-256"
            onChange={(event) => setUpdateActivationSha256Hex(event.target.value)}
            placeholder="64 hex characters"
            value={updateActivationSha256Hex}
          />
        </label>
        <label className="checkRow">
          <input
            checked={updateRestartAgent}
            onChange={(event) => setUpdateRestartAgent(event.target.checked)}
            type="checkbox"
          />
          <span>Restart agent</span>
        </label>
      </div>
    );
  }

  if (mode === "agent_update_rollback") {
    return (
      <div className="operationNote compactOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Rollback agent</strong>
          <span>Restores the saved rollback binary and leaves restart under operator control</span>
        </div>
        <label className="wideField">
          <span>Rollback SHA-256</span>
          <input
            aria-label="Agent update rollback SHA-256"
            onChange={(event) => setUpdateRollbackSha256Hex(event.target.value)}
            placeholder="Optional 64 hex characters"
            value={updateRollbackSha256Hex}
          />
        </label>
      </div>
    );
  }

  if (mode === "process_supervisor") {
    return (
      <SupervisorEditor
        setSupervisorAction={setSupervisorAction}
        setSupervisorArgv={setSupervisorArgv}
        setSupervisorCwd={setSupervisorCwd}
        setSupervisorEnv={setSupervisorEnv}
        setSupervisorLogBytes={setSupervisorLogBytes}
        setSupervisorName={setSupervisorName}
        supervisorAction={supervisorAction}
        supervisorArgv={supervisorArgv}
        supervisorCwd={supervisorCwd}
        supervisorEnv={supervisorEnv}
        supervisorLogBytes={supervisorLogBytes}
        supervisorName={supervisorName}
      />
    );
  }

  return null;
}

function SupervisorEditor({
  setSupervisorAction,
  setSupervisorArgv,
  setSupervisorCwd,
  setSupervisorEnv,
  setSupervisorLogBytes,
  setSupervisorName,
  supervisorAction,
  supervisorArgv,
  supervisorCwd,
  supervisorEnv,
  supervisorLogBytes,
  supervisorName,
}: {
  setSupervisorAction: (value: SupervisorAction) => void;
  setSupervisorArgv: (value: string) => void;
  setSupervisorCwd: (value: string) => void;
  setSupervisorEnv: (value: string) => void;
  setSupervisorLogBytes: (value: number) => void;
  setSupervisorName: (value: string) => void;
  supervisorAction: SupervisorAction;
  supervisorArgv: string;
  supervisorCwd: string;
  supervisorEnv: string;
  supervisorLogBytes: number;
  supervisorName: string;
}) {
  return (
    <div className="operationNote supervisorOperation">
      <Activity size={18} />
      <div>
        <strong>Managed process</strong>
        <span>Start, inspect, restart, stop, or tail vpsman-launched processes</span>
      </div>
      <label>
        <span>Action</span>
        <select
          aria-label="Supervisor action"
          onChange={(event) => setSupervisorAction(event.target.value as SupervisorAction)}
          value={supervisorAction}
        >
          <option value="status">Status</option>
          <option value="start">Start</option>
          <option value="stop">Stop</option>
          <option value="restart">Restart</option>
          <option value="logs">Logs</option>
        </select>
      </label>
      <label>
        <span>Name</span>
        <input
          aria-label="Supervisor process name"
          onChange={(event) => setSupervisorName(event.target.value)}
          placeholder="edge-worker"
          value={supervisorName}
        />
      </label>
      {supervisorAction === "start" && (
        <>
          <label className="wideField">
            <span>Command argv</span>
            <textarea
              aria-label="Supervisor command argv"
              onChange={(event) => setSupervisorArgv(event.target.value)}
              placeholder={SUPERVISOR_COMMAND_PLACEHOLDER}
              rows={2}
              value={supervisorArgv}
            />
          </label>
          <label>
            <span>CWD</span>
            <input
              aria-label="Supervisor cwd"
              onChange={(event) => setSupervisorCwd(event.target.value)}
              placeholder="/opt/app"
              value={supervisorCwd}
            />
          </label>
          <label className="wideField">
            <span>Env</span>
            <textarea
              aria-label="Supervisor environment"
              onChange={(event) => setSupervisorEnv(event.target.value)}
              placeholder="KEY=value"
              rows={2}
              value={supervisorEnv}
            />
          </label>
        </>
      )}
      {supervisorAction === "logs" && (
        <label>
          <span>Bytes</span>
          <input
            aria-label="Supervisor log bytes"
            max={524288}
            min={1}
            onChange={(event) => setSupervisorLogBytes(Number(event.target.value))}
            type="number"
            value={supervisorLogBytes}
          />
        </label>
      )}
    </div>
  );
}
