import * as ContextMenu from "@radix-ui/react-context-menu";
import {
  ChevronDown,
  ChevronRight,
  Copy,
  Download,
  File,
  FilePlus2,
  Folder,
  FolderPlus,
  RefreshCw,
  Save,
  Scissors,
  ShieldCheck,
  Trash2,
  Upload,
  UserRound,
} from "lucide-react";
import { basicSetup, EditorView } from "codemirror";
import type { Extension } from "@codemirror/state";
import { javascript } from "@codemirror/lang-javascript";
import { markdown } from "@codemirror/lang-markdown";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import { VpsCombobox } from "../../components/VpsCombobox";
import {
  FILE_BROWSER_ARCHIVE_LIMIT_BYTES,
  FILE_BROWSER_LIST_LIMIT,
  FILE_BROWSER_TEXT_LIMIT_BYTES,
  buildUploadOperation,
  buildWriteTextOperation,
  decodedText,
  fileBrowserOperationLabel,
  fileName,
  joinPath,
  mutatesFileSystem,
  normalizeAbsolutePath,
  parentPath,
  parseFileListStatus,
  parseFileReadTextStatus,
  parseLatestFileStatus,
  safeNormalizeAbsolutePath,
  type FileBrowserEntry,
} from "../../fileBrowser";
import { base64ToBytes, parseFileMode } from "../../fileTransfer";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../../privilege";
import { selectorExpressionForClientIds } from "../../searchExpression";
import { targetRecordTerminal } from "../../bulkJobProgress";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  FileExistingPolicy,
  FileOwnershipPolicy,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
} from "../../types";
import { formatTime, runPanelAction, shortId } from "../../utils";

const STORAGE_KEY = "vpsman.fileBrowser.state";
const DEFAULT_MODE = "0644";
const DEFAULT_DIR_MODE = "0755";

type BrowserState = {
  path: string;
  showHidden: boolean;
  targetClientId: string;
  targetExpression?: string;
};

type PendingConfirmation = {
  detail: string;
  operation: JobOperation;
  refreshPath?: string;
  selectorExpression: string;
  target: AgentView | null;
  title: string;
};

type FileCommandPopover = "chmod" | "chown" | "create" | "rename" | "upload" | null;
type BrowserClipboard = { intent: "copy" | "move"; path: string } | null;

export function FileBrowserPanel({
  agents,
  loading,
  onCreateJob,
  onLoadOutputs,
  onLoadTargets,
  onOpenMultiFiles,
  onOpenPrivilegeUnlock,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  agents: AgentView[];
  loading: boolean;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenMultiFiles?: (path: string) => void;
  onOpenPrivilegeUnlock: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (value: PrivilegeMaterial | null) => void;
}) {
  const saved = readBrowserState();
  const [targetClientId, setTargetClientId] = useState(saved.targetClientId || agents[0]?.id || "");
  const [pathInput, setPathInput] = useState(saved.path || "/");
  const [currentPath, setCurrentPath] = useState(safeNormalizeAbsolutePath(saved.path || "/"));
  const [showHidden, setShowHidden] = useState(saved.showHidden);
  const [entriesByPath, setEntriesByPath] = useState<Record<string, FileBrowserEntry[]>>({});
  const [metadataByPath, setMetadataByPath] = useState<Record<string, FileBrowserEntry>>({});
  const [expandedPaths, setExpandedPaths] = useState<Record<string, boolean>>({ "/": true });
  const [selectedPath, setSelectedPath] = useState(safeNormalizeAbsolutePath(saved.path || "/"));
  const [editorPath, setEditorPath] = useState<string | null>(null);
  const [editorContent, setEditorContent] = useState("");
  const [editorSavedContent, setEditorSavedContent] = useState("");
  const [editorSha256Hex, setEditorSha256Hex] = useState<string | null>(null);
  const [editorMode, setEditorMode] = useState(DEFAULT_MODE);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [staleDirectoryPath, setStaleDirectoryPath] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [pendingConfirmation, setPendingConfirmation] = useState<PendingConfirmation | null>(null);
  const [activeCommand, setActiveCommand] = useState<FileCommandPopover>(null);
  const [browserClipboard, setBrowserClipboard] = useState<BrowserClipboard>(null);
  const [newName, setNewName] = useState("");
  const [createType, setCreateType] = useState<"directory" | "file">("file");
  const [createMode, setCreateMode] = useState(DEFAULT_MODE);
  const [createContent, setCreateContent] = useState("");
  const [renamePathValue, setRenamePathValue] = useState("");
  const [recursive, setRecursive] = useState(false);
  const [followSymlinks, setFollowSymlinks] = useState(false);
  const [chmodMode, setChmodMode] = useState(DEFAULT_MODE);
  const [chownOwner, setChownOwner] = useState("");
  const [chownGroup, setChownGroup] = useState("");
  const [uploadMode, setUploadMode] = useState(DEFAULT_MODE);
  const [uploadExistingPolicy, setUploadExistingPolicy] = useState<FileExistingPolicy>("skip");
  const [uploadOwnershipPolicy, setUploadOwnershipPolicy] = useState<FileOwnershipPolicy>("fail");
  const [uploadOwner, setUploadOwner] = useState("");
  const [uploadGroup, setUploadGroup] = useState("");
  const [uploadDestination, setUploadDestination] = useState("");
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const selectedAgent = useMemo(
    () => agents.find((agent) => agent.id === targetClientId) ?? null,
    [agents, targetClientId],
  );
  const selectedEntry = metadataByPath[selectedPath];
  const locationCommandDisabled = !selectedEntry || pending || !privilegeMaterial;
  const selectedPathCommandDisabled = !selectedEntry || selectedPath === "/" || pending || !privilegeMaterial;
  const editorDirty = editorContent !== editorSavedContent;
  const currentEntries = entriesByPath[currentPath] ?? [];
  const staleMessage = staleDirectoryPath ? `Directory ${staleDirectoryPath} changed; refresh to update listing` : null;
  const targetSummary = selectedAgent ? `target ${targetNameId(selectedAgent)}` : "No target VPS";
  const summary = actionError ?? actionMessage ?? staleMessage ?? (privilegeMaterial ? `${targetSummary} · ${currentEntries.length} entries loaded` : "Locked");
  const editorStateText = editorPath ? `${editorContent.length} chars${editorDirty ? " · unsaved" : ""}` : "Select a text file to edit";
  const editorStatusText = actionError ?? (actionMessage ? `${actionMessage}${staleDirectoryPath ? " · refresh available" : ""}` : staleMessage) ?? editorStateText;

  useEffect(() => {
    if (!targetClientId && agents[0]?.id) {
      setTargetClientId(agents[0].id);
    }
  }, [agents, targetClientId]);

  useEffect(() => {
    writeBrowserState({ path: currentPath, targetClientId, showHidden });
  }, [currentPath, targetClientId, showHidden]);

  useLayoutEffect(() => {
    setPendingConfirmation(null);
  }, [
    activeCommand,
    browserClipboard,
    chmodMode,
    chownGroup,
    chownOwner,
    createContent,
    createMode,
    createType,
    currentPath,
    editorContent,
    editorMode,
    editorPath,
    followSymlinks,
    newName,
    pathInput,
    recursive,
    renamePathValue,
    selectedPath,
    targetClientId,
    uploadDestination,
    uploadExistingPolicy,
    uploadFile,
    uploadGroup,
    uploadMode,
    uploadOwner,
    uploadOwnershipPolicy,
  ]);

  function selectTargetClientId(value: string) {
    setTargetClientId(value);
    setEntriesByPath({});
    setMetadataByPath({});
    setExpandedPaths({ "/": true });
    setSelectedPath(currentPath);
    setEditorPath(null);
    setEditorContent("");
    setEditorSavedContent("");
    setEditorSha256Hex(null);
    setPendingConfirmation(null);
    setActionError(null);
    setActionMessage(null);
    setStaleDirectoryPath(null);
  }

  async function runFileJob(operation: JobOperation, options: { expectedType?: string } = {}) {
    if (!selectedAgent) {
      throw new Error("Choose a VPS first");
    }
    if (!privilegeMaterial) {
      throw new Error("Privilege unlock is locked");
    }
    const timeoutSecs = operation.type === "file_download" ? 90 : 30;
    const selectorExpression = selectorExpressionForClientIds([selectedAgent.id]);
    const built = await buildPrivilegeForJobOperation({
      clientIds: [selectedAgent.id],
      commandType: operation.type,
      operation,
      privilegeMaterial,
      selectorExpression,
      timeoutSecs,
    });
    setLastPayloadHash(built.payloadHashHex);
    const destructive = mutatesFileSystem(operation);
    const job = await onCreateJob({
      selector_expression: selectorExpression,
      target_client_ids: [selectedAgent.id],
      destructive,
      confirmed: true,
      command: operation.type,
      argv: [],
      job_id: crypto.randomUUID(),
      operation,
      timeout_secs: timeoutSecs,
      force_unprivileged: false,
      privileged: true,
      privilege_assertion: built.privilegeAssertion,
    });
    const outputs = await waitForOutputs(
      job.job_id,
      onLoadOutputs,
      onLoadTargets,
      options.expectedType,
    );
    return { job, outputs };
  }

  async function loadDirectory(path: string = pathInput, announce = true) {
    await runPanelAction(setPending, setActionError, async () => {
      await fetchDirectory(path, announce);
    });
  }

  async function fetchDirectory(path: string, announce = true) {
    const normalized = normalizeAbsolutePath(path);
    const { outputs } = await runFileJob(
      { type: "file_list_dir", path: normalized, offset: 0, limit: FILE_BROWSER_LIST_LIMIT, show_hidden: showHidden },
      { expectedType: "file_list_dir" },
    );
    const status = parseFileListStatus(outputs);
    if (!status) {
      throw new Error("Directory listing did not return structured output");
    }
    setCurrentPath(status.path);
    setPathInput(status.path);
    setSelectedPath(status.path);
    setExpandedPaths((current) => ({ ...current, [status.path]: true }));
    setEntriesByPath((current) => ({ ...current, [status.path]: status.entries }));
    setMetadataByPath((current) => {
      const next = { ...current, [status.path]: status.metadata };
      for (const entry of status.entries) {
        next[entry.path] = entry;
      }
      return next;
    });
    setStaleDirectoryPath((current) => (current === status.path ? null : current));
    if (announce) {
      const totalText = status.truncated_by_scan_cap
        ? `${status.visible_entries_scanned ?? status.scanned_entries ?? status.entries.length}+ scanned`
        : String(status.total_entries ?? status.entries.length);
      setActionMessage(`Loaded ${status.entries.length}/${totalText} entries from ${status.path}`);
    }
  }

  async function openTextFile(path: string) {
    const normalized = normalizeAbsolutePath(path);
    await runPanelAction(setPending, setActionError, async () => {
      const { outputs } = await runFileJob(
        {
          type: "file_read_text",
          path: normalized,
          max_bytes: FILE_BROWSER_TEXT_LIMIT_BYTES,
          follow_symlinks: followSymlinks,
        },
        { expectedType: "file_read_text" },
      );
      const status = parseFileReadTextStatus(outputs);
      if (!status) {
        throw new Error("File read did not return structured text output");
      }
      const content = decodedText(status);
      setEditorPath(status.path);
      setEditorContent(content);
      setEditorSavedContent(content);
      setEditorSha256Hex(status.sha256_hex);
      setEditorMode(formatMode(status.metadata.mode));
      setSelectedPath(status.path);
      setMetadataByPath((current) => ({ ...current, [status.path]: status.metadata }));
      setActionMessage(`Opened ${status.path}`);
    });
  }

  async function saveEditor(force = false) {
    try {
      if (!editorPath) {
        return;
      }
      const operation = await buildWriteTextOperation({
        content: editorContent,
        create: false,
        expectedSha256Hex: force ? null : editorSha256Hex,
        mode: editorMode,
        path: editorPath,
        policy: "fail",
      });
      confirmOperation(
        operation,
        "Save file",
        force || !editorSha256Hex
          ? `Save changes to ${editorPath}. No base hash is available, so this writes without optimistic conflict protection.`
          : `Save changes to ${editorPath}. If the file changed on the VPS, the save will be rejected.`,
        parentPath(editorPath),
      );
    } catch (error) {
      reportActionError(error);
    }
  }

  async function executeConfirmedOperation(operation: JobOperation, refreshPath?: string) {
    await runPanelAction(setPending, setActionError, async () => {
      const { outputs } = await runFileJob(operation, {
        expectedType: operation.type,
      });
      const status = parseLatestFileStatus(outputs, operation.type);
      if (operation.type === "file_write_text") {
        setEditorSavedContent(reviewedTextContent(operation));
        if (status?.sha256_hex) {
          setEditorSha256Hex(status.sha256_hex);
        }
      }
      if (refreshPath) {
        setStaleDirectoryPath(refreshPath);
      }
      setActionMessage(`${fileBrowserOperationLabel(operation)} completed`);
    });
  }

  function confirmOperation(operation: JobOperation, title: string, detail: string, refreshPath?: string) {
    setPendingConfirmation({
      operation,
      title,
      detail,
      refreshPath,
      selectorExpression: selectedAgent ? selectorExpressionForClientIds([selectedAgent.id]) : "",
      target: selectedAgent,
    });
  }

  function reportActionError(error: unknown) {
    setActionError(actionErrorMessage(error));
  }

  async function createFile() {
    try {
      const name = newName.trim();
      if (!name) {
        setActionError("Enter a new file name");
        return;
      }
      const destination = joinPath(selectedEntry?.is_dir ? selectedPath : currentPath, name);
      const operation = await buildWriteTextOperation({
        content: createContent,
        create: true,
        mode: createMode || DEFAULT_MODE,
        path: destination,
        policy: "fail",
      });
      confirmOperation(operation, "Write text", `Write text to ${destination} on ${selectedAgent ? targetNameId(selectedAgent) : "selected VPS"}.`, parentPath(destination));
    } catch (error) {
      reportActionError(error);
    }
  }

  function createFolder() {
    try {
      const name = newName.trim();
      if (!name) {
        setActionError("Enter a new folder name");
        return;
      }
      const destination = joinPath(selectedEntry?.is_dir ? selectedPath : currentPath, name);
      confirmOperation(
        { type: "file_mkdir", path: destination, mode: parseMode(createMode || DEFAULT_DIR_MODE), recursive, policy: "fail" },
        "Create folder",
        `Create ${destination}${recursive ? " and missing parents" : ""}.`,
        parentPath(destination),
      );
    } catch (error) {
      reportActionError(error);
    }
  }

  async function submitCreate() {
    if (createType === "directory") {
      createFolder();
      return;
    }
    await createFile();
  }

  function renameSelected() {
    try {
      if (!selectedPath || selectedPath === "/") {
        setActionError("Choose a file or folder to rename");
        return;
      }
      const newPath = normalizeAbsolutePath(renamePathValue || joinPath(parentPath(selectedPath), `${fileName(selectedPath)}-renamed`));
      confirmOperation(
        { type: "file_rename", path: selectedPath, new_path: newPath, overwrite: false, policy: "fail" },
        "Rename path",
        `Rename ${selectedPath} to ${newPath}.`,
        parentPath(selectedPath),
      );
    } catch (error) {
      reportActionError(error);
    }
  }

  function deleteSelected(path = selectedPath) {
    if (!path || path === "/") {
      setActionError("Choose a file or folder to delete");
      return;
    }
    confirmOperation(
      { type: "file_delete", path, recursive, policy: "fail" },
      "Delete path",
      `Delete ${path}${recursive ? " recursively" : ""}. This cannot be undone from the panel.`,
      parentPath(path),
    );
  }

  function chmodSelected() {
    try {
      if (!selectedPath || selectedPath === "/") {
        setActionError("Choose a file or folder to chmod");
        return;
      }
      confirmOperation(
        {
          type: "file_chmod",
          path: selectedPath,
          mode: parseMode(chmodMode),
          recursive,
          follow_symlinks: followSymlinks,
          policy: "fail",
        },
        "Change mode",
        `Apply mode ${chmodMode} to ${selectedPath}${recursive ? " recursively" : ""}.`,
        parentPath(selectedPath),
      );
    } catch (error) {
      reportActionError(error);
    }
  }

  function chownSelected() {
    if (!selectedPath || selectedPath === "/") {
      setActionError("Choose a file or folder to chown");
      return;
    }
    confirmOperation(
      {
        type: "file_chown",
        path: selectedPath,
        owner: chownOwner.trim() || null,
        group: chownGroup.trim() || null,
        recursive,
        ownership_policy: "fail",
        policy: "fail",
      },
      "Change owner",
      `Apply owner/group to ${selectedPath}${recursive ? " recursively" : ""}.`,
      parentPath(selectedPath),
    );
  }

  function copyForPaste(path: string, intent: "copy" | "move") {
    setBrowserClipboard({ path, intent });
    setActionMessage(`${intent === "copy" ? "Copied" : "Marked to move"} ${path}`);
  }

  function pasteInto(path: string) {
    if (!browserClipboard) {
      setActionError("Copy or move a file/folder first");
      return;
    }
    const destinationFolder = metadataByPath[path]?.is_dir || path === "/" ? path : parentPath(path);
    const destination = joinPath(destinationFolder, fileName(browserClipboard.path));
    const operation: JobOperation =
      browserClipboard.intent === "copy"
        ? {
            type: "file_copy",
            path: browserClipboard.path,
            new_path: destination,
            recursive: true,
            follow_symlinks: followSymlinks,
            overwrite: false,
            policy: "fail",
          }
        : { type: "file_rename", path: browserClipboard.path, new_path: destination, overwrite: false, policy: "fail" };
    confirmOperation(
      operation,
      browserClipboard.intent === "copy" ? "Paste copy" : "Paste move",
      `${browserClipboard.intent === "copy" ? "Copy" : "Move"} ${browserClipboard.path} to ${destination}.`,
      destinationFolder,
    );
  }

  async function uploadSelectedFile(file: File | null = uploadFile) {
    try {
      if (!file) {
        setActionError("Choose a file to upload");
        return;
      }
      const folder = selectedEntry?.is_dir ? selectedPath : currentPath;
      const destinationInput = uploadDestination.trim();
      const destination = normalizeAbsolutePath(
        destinationInput
          ? destinationInput.endsWith("/")
            ? `${destinationInput}${file.name}`
            : destinationInput
          : joinPath(folder, file.name),
      );
      const operation = await buildUploadOperation(file, destination, uploadMode, {
        existingPolicy: uploadExistingPolicy,
        owner: uploadOwner.trim() || null,
        group: uploadGroup.trim() || null,
        ownershipPolicy: uploadOwnershipPolicy,
      });
      confirmOperation(
        operation,
        "Upload file",
        `Upload ${file.name} to ${destination} with ${uploadExistingPolicy} existing-file policy.`,
        folder,
      );
    } catch (error) {
      reportActionError(error);
    }
  }

  async function downloadSelected(path = selectedPath) {
    if (!path) {
      return;
    }
    await runPanelAction(setPending, setActionError, async () => {
      const operation: JobOperation = {
        type: "file_download",
        path,
        max_bytes: FILE_BROWSER_ARCHIVE_LIMIT_BYTES,
        follow_symlinks: followSymlinks,
      };
      const { outputs } = await runFileJob(operation, {
        expectedType: operation.type,
      });
      const status = parseLatestFileStatus(outputs, operation.type);
      const bytes = concatenateStdout(outputs);
      const blob = new Blob([arrayBufferForBytes(bytes)], {
        type: status?.content_type ?? "application/octet-stream",
      });
      saveBlob(blob, status?.filename ?? fileName(path));
      setActionMessage(`Downloaded ${path}`);
    });
  }

  async function handleEntryOpen(entry: FileBrowserEntry) {
    setSelectedPath(entry.path);
    if (entry.is_dir) {
      if (entriesByPath[entry.path]) {
        setExpandedPaths((current) => ({ ...current, [entry.path]: !current[entry.path] }));
        setCurrentPath(entry.path);
        setPathInput(entry.path);
        return;
      }
      await loadDirectory(entry.path);
      return;
    }
    await openTextFile(entry.path);
  }

  function copySelectedPath() {
    void navigator.clipboard?.writeText(selectedPath);
    setActionMessage(`Copied ${selectedPath}`);
  }

  return (
    <div
      className="fleetPanel fileBrowserPanel"
      onDragOver={(event) => {
        event.preventDefault();
      }}
      onDrop={(event) => {
        event.preventDefault();
        const file = event.dataTransfer.files?.[0] ?? null;
        if (file) {
          setUploadFile(file);
          void uploadSelectedFile(file);
        }
      }}
    >
      <div className="sectionHeader">
        <div>
          <h2>File browser</h2>
          <span>{summary}</span>
        </div>
        <div className="fileBrowserHeaderActions">
          <VpsCombobox
            agents={agents}
            ariaLabel="File browser target VPS"
            onChange={selectTargetClientId}
            placeholder="Search file browser VPS"
            value={targetClientId}
          />
        </div>
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

      <div className="filePathBar">
        <button className="iconButton" disabled={pending || currentPath === "/" || !privilegeMaterial} onClick={() => void loadDirectory(parentPath(currentPath))} title="Parent directory" type="button">
          <ChevronRight className="rotate180" size={15} />
        </button>
        <button
          className="secondaryAction compactAction pathRefreshAction"
          disabled={pending || loading || !privilegeMaterial}
          onClick={() => void loadDirectory(staleDirectoryPath ?? pathInput)}
          title={staleDirectoryPath ? `Refresh changed directory ${staleDirectoryPath}` : "Refresh directory"}
          type="button"
        >
          <RefreshCw size={14} />
          <span>Refresh</span>
        </button>
        <input
          aria-label="Remote path"
          onChange={(event) => setPathInput(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              void loadDirectory(pathInput);
            }
          }}
          value={pathInput}
        />
        <label className="inlineCheck tightCheck">
          <input checked={showHidden} onChange={(event) => setShowHidden(event.target.checked)} type="checkbox" />
          <span>Hidden</span>
        </label>
        <button className="secondaryAction" disabled={!selectedPath} onClick={copySelectedPath} type="button">
          <Copy size={14} />
          <span>Copy path</span>
        </button>
        {onOpenMultiFiles && (
          <button className="secondaryAction" disabled={!selectedPath} onClick={() => onOpenMultiFiles(selectedPath)} type="button">
            <ShieldCheck size={14} />
            <span>Multi files</span>
          </button>
        )}
      </div>

      <div className="fileBrowserWorkspace">
        <aside className="fileTreePane">
          <div className="fileTreeToolbar">
            <button className="secondaryAction compactAction" disabled={pending || !privilegeMaterial} onClick={() => void loadDirectory("/")} type="button">
              /
            </button>
            <button className="secondaryAction compactAction" disabled={!selectedPath || pending || !privilegeMaterial} onClick={() => void downloadSelected()} type="button">
              <Download size={13} />
              <span>Download</span>
            </button>
          </div>
          <div className="fileTree" role="tree">
            <TreeNode
              depth={0}
              entriesByPath={entriesByPath}
              expandedPaths={expandedPaths}
              metadataByPath={metadataByPath}
              onChmod={(path) => {
                setSelectedPath(path);
                setActiveCommand("chmod");
              }}
              onChown={(path) => {
                setSelectedPath(path);
                setActiveCommand("chown");
              }}
              onCopyIntent={copyForPaste}
              onDelete={(path) => {
                setSelectedPath(path);
                deleteSelected(path);
              }}
              onDownload={(path) => void downloadSelected(path)}
              onOpenEntry={(entry) => void handleEntryOpen(entry)}
              onPaste={pasteInto}
              onRename={(path) => {
                setSelectedPath(path);
                setActiveCommand("rename");
              }}
              onSelectPath={(path) => setSelectedPath(path)}
              path="/"
              selectedPath={selectedPath}
              setExpandedPaths={setExpandedPaths}
            />
          </div>
        </aside>

        <main className="fileEditorPane">
          <div className="fileEditorToolbar">
            <div>
              <strong>{editorPath ?? selectedPath}</strong>
              <span>{editorStatusText}</span>
            </div>
            <div className="fileEditorActions">
              <label>
                <span>Mode</span>
                <input onChange={(event) => setEditorMode(event.target.value)} value={editorMode} />
              </label>
              <button className="primaryAction" disabled={!editorPath || !editorDirty || pending || !privilegeMaterial} onClick={() => void saveEditor()} type="button">
                <Save size={14} />
                <span>Review save</span>
              </button>
            </div>
          </div>
          <CodeMirrorTextEditor onChange={setEditorContent} path={editorPath ?? selectedPath} value={editorContent} />
        </main>

        <aside className="fileDetailsPane">
          <div className="sectionSubheader">
            <div>
              <h3>Actions</h3>
              <span title={selectedAgent?.id}>{selectedAgent ? targetNameId(selectedAgent) : "No VPS selected"}</span>
            </div>
          </div>
          <div className="fileActionStack">
            <div className="fileDetailsToolbar" aria-label="Selected file actions">
              <button aria-label="Download selected" className="iconButton" disabled={selectedPathCommandDisabled} onClick={() => void downloadSelected()} title="Download selected" type="button">
                <Download size={15} />
              </button>
              <button
                aria-label="Upload here"
                className="iconButton"
                disabled={locationCommandDisabled}
                onClick={() => {
                  setUploadDestination(joinPath(selectedEntry?.is_dir ? selectedPath : currentPath, uploadFile?.name ?? ""));
                  setActiveCommand("upload");
                }}
                title="Upload here"
                type="button"
              >
                <Upload size={15} />
              </button>
              <button aria-label="Move or rename selected" className="iconButton" disabled={selectedPathCommandDisabled} onClick={() => setActiveCommand("rename")} title="Move or rename selected" type="button">
                <Scissors size={15} />
              </button>
              <button aria-label="Create file or folder" className="iconButton" disabled={locationCommandDisabled} onClick={() => setActiveCommand("create")} title="Create file or folder" type="button">
                <FilePlus2 size={15} />
              </button>
              <button aria-label="Chmod selected" className="iconButton" disabled={selectedPathCommandDisabled} onClick={() => setActiveCommand("chmod")} title="Chmod selected" type="button">
                <ShieldCheck size={15} />
              </button>
              <button aria-label="Chown selected" className="iconButton" disabled={selectedPathCommandDisabled} onClick={() => setActiveCommand("chown")} title="Chown selected" type="button">
                <UserRound size={15} />
              </button>
              <button aria-label="Review delete selected" className="iconButton dangerIconButton" disabled={selectedPathCommandDisabled} onClick={() => deleteSelected()} title="Review delete selected" type="button">
                <Trash2 size={15} />
              </button>
            </div>
            <label
              className="inlineCheck actionCheck"
              title="Disabled by default. Enable only when selected paths are intentionally symlinks and operations should use their targets."
            >
              <input
                checked={followSymlinks}
                onChange={(event) => setFollowSymlinks(event.target.checked)}
                type="checkbox"
              />
              <span>Follow symlinks</span>
            </label>

            {activeCommand === "upload" && (
              <section className="fileCommandPopover">
                <div className="fileCommandHeader">
                  <strong>Upload file</strong>
                  <span>{selectedEntry?.is_dir ? selectedPath : currentPath}</span>
                </div>
                <label>
                  <span>File</span>
                  <input aria-label="Single file upload" onChange={(event) => setUploadFile(event.target.files?.[0] ?? null)} type="file" />
                </label>
                <label>
                  <span>Destination</span>
                  <input aria-label="Upload destination" onChange={(event) => setUploadDestination(event.target.value)} value={uploadDestination} />
                </label>
                <div className="fileActionGrid">
                  <label>
                    <span>Mode</span>
                    <input aria-label="Upload mode" onChange={(event) => setUploadMode(event.target.value)} value={uploadMode} />
                  </label>
                  <label>
                    <span>Existing file</span>
                    <select onChange={(event) => setUploadExistingPolicy(event.target.value as FileExistingPolicy)} value={uploadExistingPolicy}>
                      <option value="skip">Skip</option>
                      <option value="replace">Replace</option>
                    </select>
                  </label>
                </div>
                <div className="fileActionGrid">
                  <label>
                    <span>Owner</span>
                    <input aria-label="Upload owner" onChange={(event) => setUploadOwner(event.target.value)} value={uploadOwner} />
                  </label>
                  <label>
                    <span>Group</span>
                    <input aria-label="Upload group" onChange={(event) => setUploadGroup(event.target.value)} value={uploadGroup} />
                  </label>
                </div>
                <label>
                  <span>Missing owner/group</span>
                  <select onChange={(event) => setUploadOwnershipPolicy(event.target.value as FileOwnershipPolicy)} value={uploadOwnershipPolicy}>
                    <option value="fail">Fail</option>
                    <option value="ignore">Ignore chown</option>
                  </select>
                </label>
                <div className="fileActionGrid">
                  <button className="secondaryAction" disabled={pending || !uploadFile || !privilegeMaterial} onClick={() => void uploadSelectedFile()} type="button">
                    <Upload size={14} />
                    <span>Review upload</span>
                  </button>
                  <button className="secondaryAction" onClick={() => setActiveCommand(null)} type="button">Cancel</button>
                </div>
              </section>
            )}

            {activeCommand === "create" && (
              <section className="fileCommandPopover">
                <div className="fileCommandHeader">
                  <strong>Create</strong>
                  <span>{selectedEntry?.is_dir ? selectedPath : currentPath}</span>
                </div>
                <label>
                  <span>Name</span>
                  <input onChange={(event) => setNewName(event.target.value)} placeholder="app.conf" value={newName} />
                </label>
                <div className="fileActionGrid">
                  <label>
                    <span>Type</span>
                    <select
                      onChange={(event) => {
                        const nextType = event.target.value as "directory" | "file";
                        setCreateType(nextType);
                        if (nextType === "directory" && createMode === DEFAULT_MODE) {
                          setCreateMode(DEFAULT_DIR_MODE);
                        }
                        if (nextType === "file" && createMode === DEFAULT_DIR_MODE) {
                          setCreateMode(DEFAULT_MODE);
                        }
                      }}
                      value={createType}
                    >
                      <option value="file">Write text</option>
                      <option value="directory">Create folder</option>
                    </select>
                  </label>
                  <label>
                    <span>Mode</span>
                    <input onChange={(event) => setCreateMode(event.target.value)} value={createMode} />
                  </label>
                </div>
                {createType === "file" && (
                  <label>
                    <span>Content</span>
                    <textarea aria-label="New file text content" onChange={(event) => setCreateContent(event.target.value)} rows={7} value={createContent} />
                  </label>
                )}
                {createType === "directory" && (
                  <label className="inlineCheck actionCheck">
                    <input checked={recursive} onChange={(event) => setRecursive(event.target.checked)} type="checkbox" />
                    <span>Create parents</span>
                  </label>
                )}
                <div className="fileActionGrid">
                  <button className="secondaryAction" disabled={pending || !privilegeMaterial} onClick={() => void submitCreate()} type="button">
                    {createType === "file" ? <FilePlus2 size={14} /> : <FolderPlus size={14} />}
                    <span>{createType === "file" ? "Review write" : "Review create"}</span>
                  </button>
                  <button className="secondaryAction" onClick={() => setActiveCommand(null)} type="button">Cancel</button>
                </div>
              </section>
            )}

            {activeCommand === "rename" && (
              <section className="fileCommandPopover">
                <div className="fileCommandHeader">
                  <strong>Move or rename</strong>
                  <span>{selectedPath}</span>
                </div>
                <label>
                  <span>Destination</span>
                  <input
                    onChange={(event) => setRenamePathValue(event.target.value)}
                    placeholder={selectedPath ? joinPath(parentPath(selectedPath), `${fileName(selectedPath)}-renamed`) : "/path"}
                    value={renamePathValue}
                  />
                </label>
                <div className="fileActionGrid">
                  <button className="secondaryAction" disabled={pending || !selectedPath || !privilegeMaterial} onClick={renameSelected} type="button">
                    <Scissors size={14} />
                    <span>Review move</span>
                  </button>
                  <button className="secondaryAction" onClick={() => setActiveCommand(null)} type="button">Cancel</button>
                </div>
              </section>
            )}

            {activeCommand === "chmod" && (
              <section className="fileCommandPopover">
                <div className="fileCommandHeader">
                  <strong>Chmod</strong>
                  <span>{selectedPath}</span>
                </div>
                <label>
                  <span>Mode</span>
                  <input onChange={(event) => setChmodMode(event.target.value)} value={chmodMode} />
                </label>
                <label className="inlineCheck actionCheck">
                  <input checked={recursive} onChange={(event) => setRecursive(event.target.checked)} type="checkbox" />
                  <span>Recursive</span>
                </label>
                <div className="fileActionGrid">
                  <button className="secondaryAction" disabled={pending || !privilegeMaterial} onClick={chmodSelected} type="button">Review chmod</button>
                  <button className="secondaryAction" onClick={() => setActiveCommand(null)} type="button">Cancel</button>
                </div>
              </section>
            )}

            {activeCommand === "chown" && (
              <section className="fileCommandPopover">
                <div className="fileCommandHeader">
                  <strong>Chown</strong>
                  <span>{selectedPath}</span>
                </div>
                <div className="fileActionGrid">
                  <label>
                    <span>Owner</span>
                    <input onChange={(event) => setChownOwner(event.target.value)} value={chownOwner} />
                  </label>
                  <label>
                    <span>Group</span>
                    <input onChange={(event) => setChownGroup(event.target.value)} value={chownGroup} />
                  </label>
                </div>
                <label className="inlineCheck actionCheck">
                  <input checked={recursive} onChange={(event) => setRecursive(event.target.checked)} type="checkbox" />
                  <span>Recursive</span>
                </label>
                <div className="fileActionGrid">
                  <button className="secondaryAction" disabled={pending || !privilegeMaterial} onClick={chownSelected} type="button">Review chown</button>
                  <button className="secondaryAction" onClick={() => setActiveCommand(null)} type="button">Cancel</button>
                </div>
              </section>
            )}
            {selectedEntry && (
              <dl className="fileMetadataList">
                <div>
                  <dt>Type</dt>
                  <dd>{selectedEntry.file_type}</dd>
                </div>
                <div>
                  <dt>Size</dt>
                  <dd>{formatBytes(selectedEntry.size_bytes)}</dd>
                </div>
                <div>
                  <dt>Mode</dt>
                  <dd>{formatMode(selectedEntry.mode)}</dd>
                </div>
                <div>
                  <dt>Modified</dt>
                  <dd>{formatTime(new Date(selectedEntry.mtime_unix * 1000).toISOString())}</dd>
                </div>
              </dl>
            )}
          </div>
        </aside>
      </div>

      <ConfirmationPrompt
        confirmLabel={pendingConfirmation?.operation.type === "file_delete" ? "Delete" : "Confirm"}
        detail={pendingConfirmation ? fileConfirmationDetail(pendingConfirmation) : ""}
        items={pendingConfirmation ? fileConfirmationItems(pendingConfirmation) : []}
        onCancel={() => setPendingConfirmation(null)}
        onConfirm={() => {
          const confirmation = pendingConfirmation;
          setPendingConfirmation(null);
          if (confirmation) {
            void executeConfirmedOperation(confirmation.operation, confirmation.refreshPath);
          }
        }}
        open={pendingConfirmation !== null}
        pending={pending}
        title={pendingConfirmation?.title ?? "Confirm file operation"}
        tone={pendingConfirmation?.operation.type === "file_delete" ? "danger" : "normal"}
      />
    </div>
  );
}

function TreeNode({
  depth,
  entriesByPath,
  expandedPaths,
  metadataByPath,
  onChmod,
  onChown,
  onCopyIntent,
  onDelete,
  onDownload,
  onOpenEntry,
  onPaste,
  onRename,
  onSelectPath,
  path,
  selectedPath,
  setExpandedPaths,
}: {
  depth: number;
  entriesByPath: Record<string, FileBrowserEntry[]>;
  expandedPaths: Record<string, boolean>;
  metadataByPath: Record<string, FileBrowserEntry>;
  onChmod: (path: string) => void;
  onChown: (path: string) => void;
  onCopyIntent: (path: string, intent: "copy" | "move") => void;
  onDelete: (path: string) => void;
  onDownload: (path: string) => void;
  onOpenEntry: (entry: FileBrowserEntry) => void;
  onPaste: (path: string) => void;
  onRename: (path: string) => void;
  onSelectPath: (path: string) => void;
  path: string;
  selectedPath: string;
  setExpandedPaths: (updater: (current: Record<string, boolean>) => Record<string, boolean>) => void;
}) {
  const entry = metadataByPath[path] ?? rootEntry();
  const children = entriesByPath[path] ?? [];
  const open = expandedPaths[path] ?? path === "/";
  return (
    <div>
      <ContextMenu.Root>
        <ContextMenu.Trigger asChild>
          <button
            aria-selected={selectedPath === path}
            className={selectedPath === path ? "fileTreeRow selected" : "fileTreeRow"}
            onClick={() => onSelectPath(path)}
            onDoubleClick={() => onOpenEntry(entry)}
            style={{ paddingLeft: `${8 + depth * 16}px` }}
            type="button"
          >
            {entry.is_dir ? (
              <span
                className="fileTreeExpander"
                onClick={(event) => {
                  event.stopPropagation();
                  setExpandedPaths((current) => ({ ...current, [path]: !open }));
                }}
              >
                {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              </span>
            ) : (
              <span className="fileTreeExpander" />
            )}
            {entry.is_dir ? <Folder size={15} /> : <File size={15} />}
            <span>{path === "/" ? "/" : fileName(path)}</span>
            <small>{entry.is_dir ? "dir" : formatBytes(entry.size_bytes)}</small>
          </button>
        </ContextMenu.Trigger>
        <ContextMenu.Content className="contextMenuContent">
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onOpenEntry(entry)}>
            {entry.is_dir ? "Open folder" : "Open editor"}
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onDownload(path)}>
            Download
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => void navigator.clipboard?.writeText(path)}>
            Copy path
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onCopyIntent(path, "copy")}>
            Copy file/folder
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onCopyIntent(path, "move")}>
            Move file/folder
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onPaste(path)}>
            Review paste here
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onRename(path)}>
            Rename
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onChmod(path)}>
            Chmod
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem" onSelect={() => onChown(path)}>
            Chown
          </ContextMenu.Item>
          <ContextMenu.Item className="contextMenuItem danger" onSelect={() => onDelete(path)}>
            Review delete
          </ContextMenu.Item>
        </ContextMenu.Content>
      </ContextMenu.Root>
      {entry.is_dir && open && (
        <div role="group">
          {children.map((child) => (
            <TreeNode
              depth={depth + 1}
              entriesByPath={entriesByPath}
              expandedPaths={expandedPaths}
              key={child.path}
              metadataByPath={metadataByPath}
              onChmod={onChmod}
              onChown={onChown}
              onCopyIntent={onCopyIntent}
              onDelete={onDelete}
              onDownload={onDownload}
              onOpenEntry={onOpenEntry}
              onPaste={onPaste}
              onRename={onRename}
              onSelectPath={onSelectPath}
              path={child.path}
              selectedPath={selectedPath}
              setExpandedPaths={setExpandedPaths}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function CodeMirrorTextEditor({ onChange, path, value }: { onChange: (value: string) => void; path: string; value: string }) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => {
    if (!containerRef.current) {
      return;
    }
    const extensions: Extension[] = [
      basicSetup,
      languageExtension(path),
      EditorView.lineWrapping,
      EditorView.updateListener.of((update) => {
        if (update.docChanged) {
          onChangeRef.current(update.state.doc.toString());
        }
      }),
    ];
    const view = new EditorView({
      doc: value,
      extensions,
      parent: containerRef.current,
    });
    viewRef.current = view;
    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [path]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || view.state.doc.toString() === value) {
      return;
    }
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: value },
    });
  }, [value]);

  return <div className="codeMirrorShell" ref={containerRef} />;
}

function languageExtension(path: string): Extension {
  const lower = path.toLowerCase();
  if (/\.(js|jsx|ts|tsx|json|mjs|cjs)$/.test(lower)) {
    return javascript({ jsx: lower.endsWith("x") });
  }
  if (/\.(md|markdown)$/.test(lower)) {
    return markdown();
  }
  if (/\.(css|scss|sass)$/.test(lower)) {
    return css();
  }
  if (/\.(html|htm|xml|svg)$/.test(lower)) {
    return html();
  }
  return [];
}

async function waitForOutputs(
  jobId: string,
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>,
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>,
  expectedType?: string,
): Promise<JobOutputRecord[]> {
  let last: JobOutputRecord[] = [];
  for (;;) {
    let terminal = false;
    try {
      const targets = await onLoadTargets(jobId);
      terminal = targets.some((target) => targetRecordTerminal(target.status));
    } catch {
      // Keep polling. Target history can race job creation for a short period.
    }
    if (terminal) {
      last = await onLoadOutputs(jobId);
      if (!expectedType || parseLatestFileStatus(last, expectedType) || last.some((output) => output.done)) {
        return last;
      }
      await delay(500);
      last = await onLoadOutputs(jobId);
      return last;
    }
    await delay(500);
  }
}

function concatenateStdout(outputs: JobOutputRecord[]): Uint8Array {
  const chunks = outputs
    .filter((output) => output.stream === "stdout" && output.data_base64)
    .sort((left, right) => left.seq - right.seq)
    .map((output) => base64ToBytes(output.data_base64));
  const total = chunks.reduce((sum, chunk) => sum + chunk.byteLength, 0);
  const merged = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return merged;
}

function arrayBufferForBytes(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function reviewedTextContent(operation: Extract<JobOperation, { type: "file_write_text" }>): string {
  return new TextDecoder().decode(base64ToBytes(operation.content_base64));
}

function operationTargetPath(operation: JobOperation): string {
  if (operation.type === "file_rename") {
    return operation.new_path;
  }
  if ("path" in operation && typeof operation.path === "string") {
    return operation.path;
  }
  return operation.type;
}

function actionErrorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function fileConfirmationDetail(confirmation: PendingConfirmation): string {
  const target = confirmation.target ? targetNameId(confirmation.target) : "selected VPS";
  const policy = fileConfirmationPolicyText(confirmation.operation);
  return `${fileBrowserOperationLabel(confirmation.operation)} on ${target}${policy ? `. ${policy}` : ""}`;
}

function fileConfirmationItems(confirmation: PendingConfirmation): Array<{ label: string; value: ReactNode }> {
  const operation = confirmation.operation;
  const items: Array<{ label: string; value: ReactNode }> = [
    { label: "Selector", value: confirmation.selectorExpression || "-" },
    {
      label: "Target VPS",
      value: confirmation.target ? <span title={confirmation.target.id}>{targetNameId(confirmation.target)}</span> : "-",
    },
    { label: "Operation", value: operation.type },
  ];
  if ("path" in operation) {
    items.push({ label: "Path", value: operation.path });
  }
  if ("new_path" in operation) {
    items.push({ label: "Destination", value: operation.new_path });
  }
  if ("mode" in operation) {
    items.push({ label: "Mode", value: formatMode(operation.mode) });
  }
  if ("size_bytes" in operation) {
    items.push({ label: "Size", value: formatBytes(operation.size_bytes) });
  }
  if ("expected_sha256_hex" in operation && operation.expected_sha256_hex) {
    items.push({ label: "Expected hash", value: shortId(operation.expected_sha256_hex) });
  }
  if ("sha256_hex" in operation && operation.sha256_hex) {
    items.push({ label: "SHA256", value: shortId(operation.sha256_hex) });
  }
  if ("recursive" in operation) {
    items.push({ label: "Recursive", value: operation.recursive ? "yes" : "no" });
  }
  if ("follow_symlinks" in operation) {
    items.push({ label: "Symlinks", value: operation.follow_symlinks ? "Follow targets" : "Do not follow" });
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
    items.push({ label: "Owner/group", value: ownerGroupConfirmationValue(operation) });
  }
  return items;
}

function fileConfirmationPolicyText(operation: JobOperation): string {
  if (operation.type === "file_download") {
    return "";
  }
  if (operation.type === "file_write_text") {
    return operation.expected_sha256_hex ? "Save rejects changed current content." : `Policy: ${operation.policy ?? "fail"}.`;
  }
  if (operation.type === "file_push" || operation.type === "file_push_chunked") {
    return `Upload policy: ${operation.existing_policy ?? "skip"} existing files; ownership ${operation.ownership_policy ?? "fail"}.`;
  }
  if (operation.type === "file_rename") {
    return `Policy: ${operation.policy ?? "fail"}; destination ${operation.overwrite ? "may be atomically replaced when compatible" : "must not already exist"}.`;
  }
  if (operation.type === "file_copy") {
    return `Policy: ${operation.policy ?? "fail"}; destination ${operation.overwrite ? "files may be overwritten; directories are merged" : "must not already exist"}.`;
  }
  if ("policy" in operation && (typeof operation.policy === "string" || typeof operation.policy === "undefined")) {
    return `Policy: ${operation.policy ?? "fail"}.`;
  }
  return "";
}

function ownerGroupConfirmationValue(operation: Extract<JobOperation, { type: "file_chown" | "file_push" | "file_push_chunked" }>): string {
  const owner = operation.owner ?? operation.uid ?? "-";
  const group = operation.group ?? operation.gid ?? "-";
  return `${owner}:${group} · ${operation.ownership_policy ?? "fail"}`;
}

function targetNameId(target: Pick<AgentView, "display_name" | "id">): string {
  const name = target.display_name?.trim();
  const id = shortId(target.id);
  if (!name || name === target.id) {
    return target.id;
  }
  return `${name}_${id}`;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function rootEntry(): FileBrowserEntry {
  return {
    name: "/",
    path: "/",
    file_type: "directory",
    is_dir: true,
    is_file: false,
    is_symlink: false,
    size_bytes: 0,
    mode: 0o755,
    uid: 0,
    gid: 0,
    mtime_unix: 0,
    symlink_target: null,
  };
}

function parseMode(value: string): number {
  const normalized = value.trim() || DEFAULT_MODE;
  return parseFileMode(normalized);
}

function formatMode(mode: number): string {
  return `0${(mode & 0o777).toString(8).padStart(3, "0")}`;
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MiB`;
  return `${(value / 1024 / 1024 / 1024).toFixed(1)} GiB`;
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

function readBrowserState(): BrowserState {
  try {
    const parsed = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}") as Partial<BrowserState> & { selectedClientId?: string };
    const targetClientId =
      typeof parsed.targetClientId === "string" && parsed.targetClientId.trim()
        ? parsed.targetClientId.trim()
        : typeof parsed.selectedClientId === "string" && parsed.selectedClientId.trim()
          ? parsed.selectedClientId.trim()
          : typeof parsed.targetExpression === "string"
            ? clientIdFromLegacyFileSelector(parsed.targetExpression)
            : "";
    return {
      path: typeof parsed.path === "string" ? parsed.path : "/",
      showHidden: Boolean(parsed.showHidden),
      targetClientId,
      targetExpression: typeof parsed.targetExpression === "string" ? parsed.targetExpression : undefined,
    };
  } catch {
    return { path: "/", showHidden: false, targetClientId: "" };
  }
}

function writeBrowserState(state: BrowserState) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

function clientIdFromLegacyFileSelector(value: string): string {
  const match = value
    .trim()
    .match(/^id:(?:"((?:\\.|[^"\\])*)"|'((?:\\.|[^'\\])*)'|([^\s()&|]+))$/i);
  if (!match) {
    return "";
  }
  return (match[1] ?? match[2] ?? match[3] ?? "").replace(/\\(["'\\])/g, "$1");
}
