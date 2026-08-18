import {
  Activity,
  DatabaseBackup,
  Download,
  PackageCheck,
  Upload,
} from "lucide-react";
import { useByteCountFormatter } from "../../panelDisplay";
import {
  FILE_TRANSFER_CHUNK_BYTES,
  MAX_CHUNKED_FILE_PUSH_BYTES,
} from "../../fileTransfer";
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
  MAX_FILE_TRANSFER_RATE_LIMIT_KBPS,
} from "../../resumableFileTransfer";
import type {
  BrowserDownloadSinkMode,
  BrowserTransferMultiTargetPolicy,
} from "../../resumableFileTransfer";
import type { FileExistingPolicy } from "../../types";
import type { FileTransferSourceArtifactRecord } from "../../typesFileTransfer";
import type { DispatchMode, SupervisorAction } from "../jobDispatchModel";
import {
  ArgvInspector,
  ExactPayloadInspector,
} from "../../components/ExactPayloadInspector";
import { TerminalOperationControls } from "./TerminalOperationControls";

export function OperationModeTabs({
  includeTerminal = true,
  mode,
  onModeChange,
}: {
  includeTerminal?: boolean;
  mode: DispatchMode;
  onModeChange: (mode: DispatchMode) => void;
}) {
  const groups = operationModeGroups(includeTerminal);
  const activeGroup =
    groups.find((group) => group.modes.some((item) => item.mode === mode)) ??
    groups[0];

  return (
    <div className="operationSelector" aria-label="Dispatch operation selector">
      <label className="operationMobileSelect">
        <span>Operation</span>
        <select
          aria-label="Dispatch operation"
          onChange={(event) => onModeChange(event.target.value as DispatchMode)}
          value={mode}
        >
          {groups.map((group) => (
            <optgroup key={group.id} label={group.label}>
              {group.modes.map((item) => (
                <option key={item.mode} value={item.mode}>
                  {item.label}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
      </label>
      <div
        className="operationGroupTabs"
        aria-label="Dispatch operation groups"
      >
        {groups.map((group) => (
          <button
            aria-pressed={group.id === activeGroup.id}
            className={group.id === activeGroup.id ? "selected" : ""}
            key={group.id}
            onClick={() => onModeChange(group.modes[0].mode)}
            type="button"
          >
            {group.label}
          </button>
        ))}
      </div>
      <div
        className="operationChoiceStrip"
        aria-label={`${activeGroup.label} operations`}
      >
        {activeGroup.modes.map((item) => (
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
    </div>
  );
}

type OperationModeChoice = { label: string; mode: DispatchMode };

type OperationModeGroup = {
  id: string;
  label: string;
  modes: OperationModeChoice[];
};

function operationModeGroups(includeTerminal: boolean): OperationModeGroup[] {
  const groups: OperationModeGroup[] = [
    {
      id: "command",
      label: "Command",
      modes: [
        { label: "Argv", mode: "shell" },
        { label: "Shell", mode: "shell_script" },
      ],
    },
    {
      id: "files",
      label: "Files",
      modes: [
        { label: "File pull", mode: "file_pull" },
        { label: "File push", mode: "file_push" },
        { label: "Resumable upload", mode: "file_transfer_upload" },
        { label: "Resumable download", mode: "file_transfer_download" },
      ],
    },
    {
      id: "update",
      label: "Update",
      modes: [
        { label: "Manual update", mode: "agent_update" },
        { label: "Check update", mode: "agent_update_check" },
        { label: "Activate", mode: "agent_update_activate" },
        { label: "Rollback", mode: "agent_update_rollback" },
      ],
    },
    {
      id: "backup",
      label: "Backup",
      modes: [{ label: "Backup", mode: "backup" }],
    },
    {
      id: "network",
      label: "Network",
      modes: [
        {
          label: "Import vnStat history",
          mode: "network_traffic_import_vnstat",
        },
      ],
    },
    {
      id: "process",
      label: "Process",
      modes: [
        { label: "Sessions", mode: "user_sessions" },
        { label: "Processes", mode: "process_list" },
        { label: "Supervisor", mode: "process_supervisor" },
      ],
    },
  ];
  if (includeTerminal) {
    groups.splice(1, 0, {
      id: "terminal",
      label: "Terminal",
      modes: [{ label: "Terminal", mode: "terminal_session" }],
    });
  }
  return groups;
}

export function JobOperationEditor({
  backupIncludeConfig,
  backupFollowSymlinks,
  backupSkipMissingPaths,
  backupPathsText,
  commandArgv,
  commandArgvError,
  commandText,
  shellPty,
  terminalArgv,
  terminalCols,
  terminalCwd,
  terminalUser,
  terminalUserPolicy,
  terminalFlowWindowBytes,
  terminalIdleTimeoutSecs,
  terminalReplayFromSeq,
  terminalRows,
  terminalSessionId,
  filePath,
  fileFollowSymlinks,
  filePushMode,
  filePushPath,
  filePushSource,
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
  fileTransferSourcesTruncated,
  fileTransferUploadSourceKind,
  mode,
  networkTrafficImportInterfacesText,
  networkTrafficImportStartDate,
  processLimit,
  setBackupIncludeConfig,
  setBackupFollowSymlinks,
  setBackupSkipMissingPaths,
  setBackupPathsText,
  setCommandText,
  setShellPty,
  setShellScript,
  setTerminalArgv,
  setTerminalCols,
  setTerminalCwd,
  setTerminalUser,
  setTerminalUserPolicy,
  setTerminalFlowWindowBytes,
  setTerminalIdleTimeoutSecs,
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
  setNetworkTrafficImportInterfacesText,
  setNetworkTrafficImportStartDate,
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
  backupFollowSymlinks: boolean;
  backupSkipMissingPaths: boolean;
  backupPathsText: string;
  commandArgv: string[];
  commandArgvError: string | null;
  commandText: string;
  shellPty: boolean;
  terminalArgv: string;
  terminalCols: number;
  terminalCwd: string;
  terminalUser: string;
  terminalUserPolicy: "fail" | "fallback";
  terminalFlowWindowBytes: number;
  terminalIdleTimeoutSecs: number;
  terminalReplayFromSeq: string;
  terminalRows: number;
  terminalSessionId: string;
  filePath: string;
  fileFollowSymlinks: boolean;
  filePushMode: string;
  filePushPath: string;
  filePushSource: File | null;
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
  fileTransferSourcesTruncated: boolean;
  fileTransferUploadSourceKind: "local-file" | "source-artifact";
  mode: DispatchMode;
  networkTrafficImportInterfacesText: string;
  networkTrafficImportStartDate: string;
  processLimit: number;
  setBackupIncludeConfig: (value: boolean) => void;
  setBackupFollowSymlinks: (value: boolean) => void;
  setBackupSkipMissingPaths: (value: boolean) => void;
  setBackupPathsText: (value: string) => void;
  setCommandText: (value: string) => void;
  setShellPty: (value: boolean) => void;
  setShellScript: (value: string) => void;
  setTerminalArgv: (value: string) => void;
  setTerminalCols: (value: number) => void;
  setTerminalCwd: (value: string) => void;
  setTerminalUser: (value: string) => void;
  setTerminalUserPolicy: (value: "fail" | "fallback") => void;
  setTerminalFlowWindowBytes: (value: number) => void;
  setTerminalIdleTimeoutSecs: (value: number) => void;
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
  setFileTransferMultiTargetPolicy: (
    value: BrowserTransferMultiTargetPolicy,
  ) => void;
  setFileTransferRateLimit: (value: number) => void;
  setFileTransferResumeToken: (value: string) => void;
  setFileTransferSessionId: (value: string) => void;
  setFileTransferSourceArtifactId: (value: string) => void;
  setFileTransferUploadSourceKind: (
    value: "local-file" | "source-artifact",
  ) => void;
  setNetworkTrafficImportInterfacesText: (value: string) => void;
  setNetworkTrafficImportStartDate: (value: string) => void;
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
  const formatBytes = useByteCountFormatter();
  if (mode === "shell") {
    return (
      <div className="commandPayloadEditor">
        <div className="compactOperation shellOperation">
          <label
            className="wideField"
            title="Command and arguments submitted to each selected VPS."
          >
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
        <ArgvInspector
          ariaLabel="Dispatch argv elements"
          argv={commandArgv}
          elementDetail={(_value, index) =>
            index === 0
              ? "Executable value · passed directly"
              : "Exact argument · passed directly"
          }
          error={commandArgvError}
          footer="One row is one exact argument. Quotes only group authoring text and are not sent; values are never split or reparsed after this preview."
          help="Whitespace separates authoring values unless quotes or backslash escaping group them. The resulting ordered argv is passed directly to the executable, without a shell or a second parse."
          title="Parsed direct argv"
        />
      </div>
    );
  }

  if (mode === "shell_script") {
    const submittedScript = shellScript.trim();
    const scriptBytes = new TextEncoder().encode(submittedScript).length;
    const scriptLines = submittedScript
      ? submittedScript.split(/\r\n|\r|\n/).length
      : 0;
    return (
      <div className="commandPayloadEditor">
        <label title="Shell script executed on each selected VPS.">
          <span>Shell script</span>
          <textarea
            aria-label="Shell script"
            onChange={(event) => setShellScript(event.target.value)}
            placeholder="set -eu&#10;hostname&#10;uptime"
            rows={5}
            value={shellScript}
          />
        </label>
        <ExactPayloadInspector
          ariaLabel="Submitted script payload"
          exactValue={JSON.stringify(submittedScript)}
          exactValueLabel="Exact submitted script JSON value"
          footer="Outer whitespace is trimmed before review. The script remains one value; its target-specific configured shell argv prefix is not guessed by this composer."
          help="The exact submitted script is appended as one value to each target agent's configured shell-script argv prefix. This composer does not infer a shell executable or environment."
          items={[
            {
              detail: submittedScript
                ? "One exact script value · interpreted by the target's configured shell"
                : "Enter a script to preview the submitted value.",
              label: "script",
              value: submittedScript || "No script yet",
            },
          ]}
          summary={`${scriptBytes} UTF-8 ${scriptBytes === 1 ? "byte" : "bytes"} · ${scriptLines} ${scriptLines === 1 ? "line" : "lines"}`}
          title="Exact shell payload"
        />
      </div>
    );
  }

  if (mode === "terminal_session") {
    return (
      <TerminalOperationControls
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
      />
    );
  }

  if (mode === "file_pull") {
    return (
      <div className="compactOperation filePathOperation">
        <label
          className="wideField"
          title="SHA-256 digest of the staged agent artifact; enter exactly 64 hexadecimal characters."
        >
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
          <span>
            Privilege-unlocked, chunk-hashed, atomic agent write up to{" "}
            {MAX_CHUNKED_FILE_PUSH_BYTES} bytes
          </span>
        </div>
        <label className="wideField">
          <span>Source file</span>
          <input
            aria-label="File push source"
            onChange={(event) =>
              setFilePushSource(event.target.files?.[0] ?? null)
            }
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
          <input
            aria-label="File push mode"
            onChange={(event) => setFilePushMode(event.target.value)}
            value={filePushMode}
          />
        </label>
      </div>
    );
  }

  if (mode === "file_transfer_upload") {
    return (
      <div className="operationNote compactOperation fileTransferUploadOperation">
        <div className="fileTransferOperationHeader">
          <Upload size={18} />
          <div>
            <strong>Resumable upload</strong>
            <span>
              Streamed ACK-tracked browser upload up to{" "}
              {formatBytes(MAX_BROWSER_RESUMABLE_UPLOAD_BYTES)}
            </span>
          </div>
        </div>
        <label>
          <span>Source kind</span>
          <select
            aria-label="Resumable upload producer"
            onChange={(event) =>
              setFileTransferUploadSourceKind(
                event.target.value as "local-file" | "source-artifact",
              )
            }
            value={fileTransferUploadSourceKind}
          >
            <option value="local-file">Local file</option>
            <option value="source-artifact">Source artifact</option>
          </select>
        </label>
        {fileTransferUploadSourceKind === "source-artifact" ? (
          <label className="wideField transferPrimaryField">
            <span>Resumable upload source artifact</span>
            <select
              aria-label="Resumable upload source artifact"
              onChange={(event) =>
                setFileTransferSourceArtifactId(event.target.value)
              }
              value={fileTransferSourceArtifactId}
            >
              <option value="">
                {fileTransferSources.length === 0
                  ? "No source artifacts available"
                  : "Select artifact"}
              </option>
              {fileTransferSources.map((source) => (
                <option key={source.id} value={source.id}>
                  {source.name} · {formatBytes(source.size_bytes)}
                </option>
              ))}
            </select>
            {fileTransferSourcesTruncated && (
              <small>
                {fileTransferSources.length} source artifacts loaded; older
                artifacts may not appear.
              </small>
            )}
          </label>
        ) : (
          <div className="wideField transferPrimaryField dispatchFileSourceField">
            <span>Source file</span>
            <div className="dispatchFileSourceControl">
              <span
                className="dispatchSelectedFile"
                title={filePushSource?.name}
              >
                {filePushSource
                  ? `${filePushSource.name} · ${formatBytes(filePushSource.size)}`
                  : "No local file selected"}
              </span>
              <label className="secondaryAction compactAction dispatchFilePicker">
                <Upload size={14} />
                <span>{filePushSource ? "Replace" : "Choose file"}</span>
                <input
                  aria-label="Resumable upload source"
                  onChange={(event) =>
                    setFilePushSource(event.target.files?.[0] ?? null)
                  }
                  type="file"
                />
              </label>
            </div>
          </div>
        )}
        <label className="wideField transferPrimaryField">
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
            onChange={(event) =>
              setFileTransferChunkSize(Number(event.target.value))
            }
            type="number"
            value={fileTransferChunkSize}
          />
        </label>
        <label title="Transfer rate cap in Mbps. Use 0 for no cap.">
          <span>Rate limit Mbps</span>
          <input
            aria-label="Resumable upload rate limit Mbps"
            max={MAX_FILE_TRANSFER_RATE_LIMIT_KBPS / 1_000}
            min={0}
            onChange={(event) =>
              setFileTransferRateLimit(
                Math.round(Number(event.target.value) * 1_000),
              )
            }
            step={0.001}
            type="number"
            value={fileTransferRateLimit / 1_000}
          />
        </label>
        <label>
          <span>Existing file</span>
          <select
            aria-label="Resumable upload existing-file policy"
            onChange={(event) =>
              setFileTransferExistingPolicy(
                event.target.value as FileExistingPolicy,
              )
            }
            value={fileTransferExistingPolicy}
          >
            <option value="skip">Skip</option>
            <option value="replace">Replace</option>
          </select>
        </label>
        <label title="Choose whether every target must resume from one shared byte offset or may resume independently.">
          <span>Multi-VPS resume</span>
          <select
            aria-label="Resumable upload multi-target policy"
            onChange={(event) =>
              setFileTransferMultiTargetPolicy(
                event.target.value as BrowserTransferMultiTargetPolicy,
              )
            }
            value={fileTransferMultiTargetPolicy}
          >
            <option value="same-offset">Shared offset (all targets)</option>
            <option value="independent-offsets">Independent offsets</option>
          </select>
        </label>
        <label className="wideField transferIdentityField">
          <span>Session</span>
          <input
            aria-label="Resumable upload session"
            onChange={(event) => setFileTransferSessionId(event.target.value)}
            placeholder="auto"
            value={fileTransferSessionId}
          />
        </label>
        <label
          className="wideField transferIdentityField"
          title="Optional stable token used to resume an interrupted upload; leave blank to generate it automatically."
        >
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
              Browser download up to{" "}
              {formatBytes(MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES)};
              stream-to-file up to{" "}
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
            onChange={(event) =>
              setFileTransferDownloadName(event.target.value)
            }
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
            onChange={(event) =>
              setFileTransferChunkSize(Number(event.target.value))
            }
            type="number"
            value={fileTransferChunkSize}
          />
        </label>
        <label title="Transfer rate cap in Mbps. Use 0 for no cap.">
          <span>Rate limit Mbps</span>
          <input
            aria-label="Resumable download rate limit Mbps"
            max={MAX_FILE_TRANSFER_RATE_LIMIT_KBPS / 1_000}
            min={0}
            onChange={(event) =>
              setFileTransferRateLimit(
                Math.round(Number(event.target.value) * 1_000),
              )
            }
            step={0.001}
            type="number"
            value={fileTransferRateLimit / 1_000}
          />
        </label>
        <label className="wideField">
          <span>Save method</span>
          <select
            aria-label="Resumable download save method"
            onChange={(event) =>
              setFileTransferDownloadSink(
                event.target.value as BrowserDownloadSinkMode,
              )
            }
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
        <label
          className="wideField"
          title="Optional stable token used to resume an interrupted download; leave blank to generate it automatically."
        >
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

  if (mode === "network_traffic_import_vnstat") {
    return (
      <div className="operationNote compactOperation">
        <Activity size={18} />
        <div>
          <strong>Import retained vnStat traffic</strong>
          <span>
            The agent reads vnStat once. After its output is stored, the target
            stays running while the API durably backfills synthetic minute
            samples up to each interface&apos;s first live counter sample. A
            restart resumes pending server-side imports. A rerun replaces prior
            vnStat-imported samples for those interfaces.
          </span>
        </div>
        <label
          className="wideField"
          title="Comma-separated host interfaces to import, such as eth0, ens3. Leave blank to import every interface reported by vnStat."
        >
          <span>Host interfaces</span>
          <textarea
            aria-label="vnStat import host interfaces"
            onChange={(event) =>
              setNetworkTrafficImportInterfacesText(event.target.value)
            }
            placeholder="eth0, ens3"
            rows={2}
            value={networkTrafficImportInterfacesText}
          />
        </label>
        <label>
          <span>Start date (UTC)</span>
          <input
            aria-label="vnStat import start date"
            onChange={(event) =>
              setNetworkTrafficImportStartDate(event.target.value)
            }
            type="date"
            value={networkTrafficImportStartDate}
          />
        </label>
        <span className="operationHint">
          There is no fixed lookback limit. A selected date earlier than an
          interface&apos;s vnStat history is clamped independently to that
          interface&apos;s latest continuous retained coverage. The operation
          also requires that coverage to reach an existing live agent sample.
          Aggregate bytes are preserved; minute-level distribution is
          reconstructed from vnStat&apos;s retained resolutions.
        </span>
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
      <div className="operationNote compactOperation backupOperation">
        <DatabaseBackup size={18} />
        <div>
          <strong>Backup artifact</strong>
          <span>
            Agent packages selected regular files and config into a plain tar
            artifact
          </span>
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
        <div
          className="backupOptionStrip"
          aria-label="Backup collection options"
        >
          <label className="checkLine inlineCheck">
            <input
              checked={backupIncludeConfig}
              onChange={(event) => setBackupIncludeConfig(event.target.checked)}
              type="checkbox"
            />
            <span>Agent config</span>
          </label>
          <label
            className="checkLine inlineCheck"
            title="Default is off. Enable only when the backup should archive symlink target contents."
          >
            <input
              checked={backupFollowSymlinks}
              onChange={(event) =>
                setBackupFollowSymlinks(event.target.checked)
              }
              type="checkbox"
            />
            <span>Follow symlinks</span>
          </label>
          <label
            className="checkLine inlineCheck"
            title="Continue only when a selected root does not exist on a target VPS. Unreadable paths and collection errors still fail that target."
          >
            <input
              checked={backupSkipMissingPaths}
              onChange={(event) =>
                setBackupSkipMissingPaths(event.target.checked)
              }
              type="checkbox"
            />
            <span>Skip missing roots</span>
          </label>
        </div>
      </div>
    );
  }

  if (mode === "agent_update") {
    return (
      <div className="operationNote compactOperation agentUpdateOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Agent binary</strong>
          <span>
            HTTPS artifact staged side-by-side after SHA-256 verification
          </span>
        </div>
        <label
          className="wideField"
          title="HTTPS URL used to download the agent binary before SHA-256 verification."
        >
          <span>Artifact URL</span>
          <input
            aria-label="Agent update artifact URL"
            onChange={(event) => setUpdateArtifactUrl(event.target.value)}
            placeholder="https://updates.example/vpsman-agent"
            value={updateArtifactUrl}
          />
        </label>
        <label
          className="wideField"
          title="Expected SHA-256 digest for the downloaded agent binary, as 64 hexadecimal characters."
        >
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
      <div className="operationNote compactOperation agentUpdateOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Version manifest</strong>
          <span>
            Checks version.json and selects the matching architecture-specific
            artifact for each VPS
          </span>
        </div>
        <label
          className="wideField"
          title="HTTPS URL of version.json used to select the architecture-specific agent artifact."
        >
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
            onChange={(event) => {
              const activate = event.target.checked;
              setUpdateCheckActivate(activate);
              if (!activate) {
                setUpdateCheckRestartAgent(false);
              }
            }}
            type="checkbox"
          />
          <span>Activate if newer</span>
        </label>
        <label className="checkRow">
          <input
            checked={updateCheckRestartAgent}
            disabled={!updateCheckActivate}
            onChange={(event) =>
              setUpdateCheckRestartAgent(event.target.checked)
            }
            type="checkbox"
          />
          <span>Restart agent</span>
        </label>
        <div className="operationSafetyNote" role="note">
          Each VPS selects and verifies its own architecture-specific artifact.
          When enabled, activation uses that VPS-specific staged SHA-256; one
          shared SHA-256 is not required.
        </div>
      </div>
    );
  }

  if (mode === "agent_update_activate") {
    return (
      <div className="operationNote compactOperation agentUpdateOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Activate staged agent</strong>
          <span>
            Promotes the verified side-by-side artifact and keeps rollback copy
            for restart recovery
          </span>
        </div>
        <label className="wideField">
          <span>Staged SHA-256</span>
          <input
            aria-label="Agent update staged SHA-256"
            onChange={(event) =>
              setUpdateActivationSha256Hex(event.target.value)
            }
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
      <div className="operationNote compactOperation agentUpdateOperation">
        <PackageCheck size={18} />
        <div>
          <strong>Rollback agent</strong>
          <span>
            Restores the saved rollback binary and leaves restart under operator
            control
          </span>
        </div>
        <label
          className="wideField"
          title="Optional SHA-256 digest of the rollback artifact; enter exactly 64 hexadecimal characters when specified."
        >
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
        <span>
          Start, inspect, restart, stop, or tail vpsman-launched processes
        </span>
      </div>
      <label>
        <span>Action</span>
        <select
          aria-label="Supervisor action"
          onChange={(event) =>
            setSupervisorAction(event.target.value as SupervisorAction)
          }
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
          <label
            className="wideField"
            title="Command and arguments used to start the managed process."
          >
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
          <label
            className="wideField"
            title="Environment entries for the managed process, one KEY=value pair per line."
          >
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
            onChange={(event) =>
              setSupervisorLogBytes(Number(event.target.value))
            }
            type="number"
            value={supervisorLogBytes}
          />
        </label>
      )}
    </div>
  );
}
