import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { ConsoleDetailPanel } from "../components/ConsoleDetailPanel";
import type { ArtifactDownloadMode } from "../artifactDownload";
import {
  JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE,
  JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
} from "../generated/protocolContracts";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildPrivilegeForJobOperation,
  type PrivilegeMaterial,
} from "../privilege";
import type {
  JobDispatchPreset,
  JobDispatchPresetInput,
} from "../jobDispatchPreset";
import type {
  AgentView,
  BulkResolveResponse,
  CommandTemplateRecord,
  CreateJobRequest,
  CreateJobResponse,
  DeleteCommandTemplateRequest,
  JobHistoryRecord,
  JobOperation,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetSelection,
  HostProcessInventoryRecord,
  HostServiceInventoryRecord,
  HostStorageInventoryRecord,
  ProcessSupervisorInventoryRecord,
  UpsertCommandTemplateRequest,
  WsTerminalOutputEvent,
} from "../types";
import type {
  FileTransferHandoffRecord,
  FileTransferSessionRecord,
  FileTransferSourceArtifactRecord,
  UploadFileTransferSourceArtifactRequest,
} from "../typesFileTransfer";
import type {
  TerminalControlAck,
  TerminalControlSubmitRequest,
  TerminalReplayRecord,
  TerminalSessionRecord,
} from "../typesTerminal";
import { clientDisplayNameFromMap, clientDisplayNameMap } from "../utils";
import { JobDispatchPanel } from "./JobDispatchPanel";
import { retryableLazy } from "../lazyImport";
import {
  pushHistoryEntry,
  replaceHistoryEntry,
} from "../historyEntryState";

export type DirectTerminalOpenRequest = {
  maxTimeoutSecs: number;
  session: TerminalSessionRecord;
  terminalReplayFromSeq?: string;
  terminalUser: string;
  terminalUserPolicy: "fail" | "fallback";
};

const FileBrowserPanel = retryableLazy(() =>
  import("./jobs/FileBrowserPanel").then((module) => ({
    default: module.FileBrowserPanel,
  })),
);
const FileTransferSessionsPanel = retryableLazy(() =>
  import("./jobs/FileTransferSessionsPanel").then((module) => ({
    default: module.FileTransferSessionsPanel,
  })),
);
const MultiFileActionsPanel = retryableLazy(() =>
  import("./jobs/MultiFileActionsPanel").then((module) => ({
    default: module.MultiFileActionsPanel,
  })),
);
const ProcessSupervisorInventoryPanel = retryableLazy(() =>
  import("./jobs/ProcessSupervisorInventoryPanel").then((module) => ({
    default: module.ProcessSupervisorInventoryPanel,
  })),
);
const HostProcessInventoryPanel = retryableLazy(() =>
  import("./jobs/HostProcessInventoryPanel").then((module) => ({
    default: module.HostProcessInventoryPanel,
  })),
);
const HostServicesPanel = retryableLazy(() =>
  import("./jobs/HostServicesPanel").then((module) => ({
    default: module.HostServicesPanel,
  })),
);
const HostStoragePanel = retryableLazy(() =>
  import("./jobs/HostStoragePanel").then((module) => ({
    default: module.HostStoragePanel,
  })),
);
const TerminalSessionsPanel = retryableLazy(() =>
  import("./jobs/TerminalSessionsPanel").then((module) => ({
    default: module.TerminalSessionsPanel,
  })),
);

type RemoteOperationsSubpage =
  | "terminal"
  | "files"
  | "multi_files"
  | "transfers"
  | "processes"
  | "services"
  | "storage";

export function RemoteOperationsPanel({
  activeSubpage,
  agents,
  commandTemplates,
  commandTemplatesTruncated,
  dispatchPreset,
  fleetEvidenceAvailable,
  fileTransfers,
  fileTransfersTruncated,
  fileTransferSources,
  fileTransferSourcesTruncated,
  initialTargetIntent,
  lastTerminalOutputEvent,
  loading,
  onCreateFileTransferHandoff,
  onCreateJob,
  onDownloadFileBundle,
  onDownloadFileTransferSource,
  onDownloadOutputChunk,
  onDownloadOutputStream,
  onDispatchPresetApplied,
  onLoadJob,
  onLoadHostProcessInventory,
  onLoadHostServiceInventory,
  onLoadHostStorageInventory,
  onLoadOutputs,
  onLoadTargets,
  onLoadTerminalReplay,
  onInitialTargetIntentConsumed,
  onOpenJobDetails,
  onOpenJobsDispatch,
  onOpenPrivilegeUnlock,
  onOpenSessionEvidence,
  onRefresh,
  onResolveTargets,
  onSaveFileTransferHandoff,
  onSelectSubpage,
  onControlTerminalSession,
  onTransferTargetConsumed,
  onUploadFileTransferSource,
  onDeleteCommandTemplate,
  onUpsertCommandTemplate,
  privilegeMaterial,
  processSupervisorInventory,
  processSupervisorInventoryTruncated,
  setPrivilegeMaterial,
  terminalSessions,
  terminalSessionsTruncated,
  transferTargetIntent,
}: {
  activeSubpage: string;
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  commandTemplatesTruncated: boolean;
  dispatchPreset?: JobDispatchPreset | null;
  fleetEvidenceAvailable: boolean;
  fileTransfers: FileTransferSessionRecord[];
  fileTransfersTruncated: boolean;
  fileTransferSources: FileTransferSourceArtifactRecord[];
  fileTransferSourcesTruncated: boolean;
  initialTargetIntent?: {
    clientId: string;
    destination: "processes" | "terminal";
    requestId: string;
  } | null;
  lastTerminalOutputEvent: WsTerminalOutputEvent | null;
  loading: boolean;
  onCreateFileTransferHandoff: (
    clientId: string,
    sessionId: string,
  ) => Promise<FileTransferHandoffRecord>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadFileBundle: (jobId: string, clientIds: string[]) => Promise<Blob>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onDownloadOutputChunk: (
    jobId: string,
    clientId: string,
    seq: number,
  ) => Promise<Blob>;
  onDownloadOutputStream: (
    jobId: string,
    clientId: string,
    stream: "stdout" | "stderr" | "combined",
  ) => Promise<Blob>;
  onDispatchPresetApplied?: () => void;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadHostProcessInventory: (
    clientId: string,
    limit?: number,
  ) => Promise<HostProcessInventoryRecord>;
  onLoadHostServiceInventory: (
    clientId: string,
    limit?: number,
  ) => Promise<HostServiceInventoryRecord>;
  onLoadHostStorageInventory: (
    clientId: string,
    limit?: number,
  ) => Promise<HostStorageInventoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onLoadTerminalReplay: (
    clientId: string,
    sessionId: string,
    fromSeq?: number,
  ) => Promise<TerminalReplayRecord>;
  onInitialTargetIntentConsumed?: (requestId: string) => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobsDispatch?: () => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenSessionEvidence?: () => void;
  onRefresh: () => void;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
  onSaveFileTransferHandoff: (
    downloadPath: string,
    request: {
      expectedSha256Hex?: string | null;
      expectedSizeBytes?: number | null;
      fileName: string;
      mode: ArtifactDownloadMode;
    },
  ) => Promise<void>;
  onSelectSubpage?: (subpage: string) => void;
  onControlTerminalSession: (
    clientId: string,
    sessionId: string,
    request: TerminalControlSubmitRequest,
  ) => Promise<TerminalControlAck>;
  onTransferTargetConsumed?: () => void;
  onUploadFileTransferSource: (
    request: UploadFileTransferSourceArtifactRequest,
  ) => Promise<FileTransferSourceArtifactRecord>;
  onDeleteCommandTemplate: (
    templateId: string,
    request: DeleteCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  onUpsertCommandTemplate: (
    request: UpsertCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  processSupervisorInventory: ProcessSupervisorInventoryRecord[];
  processSupervisorInventoryTruncated: boolean;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => Promise<void>;
  terminalSessions: TerminalSessionRecord[];
  terminalSessionsTruncated: boolean;
  transferTargetIntent?: {
    clientId: string;
    context: string;
    path: string;
  } | null;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [processComposerPreset, setProcessComposerPreset] =
    useState<JobDispatchPreset | null>(null);
  const [transferComposerPreset, setTransferComposerPreset] =
    useState<JobDispatchPreset | null>(null);
  const [multiFileInitialPath, setMultiFileInitialPath] = useState("");
  const [transferFocusPath, setTransferFocusPath] = useState<string | null>(
    null,
  );
  const [processRoute, setProcessRoute] = useState(readProcessWorkspaceRoute);
  const routedProcessIntentRef = useRef<string | null>(null);
  const remoteSubpage = remoteOperationsPanelSubpage(activeSubpage);
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);

  useEffect(() => {
    const applyRoute = () => setProcessRoute(readProcessWorkspaceRoute());
    window.addEventListener("popstate", applyRoute);
    window.addEventListener("hashchange", applyRoute);
    return () => {
      window.removeEventListener("popstate", applyRoute);
      window.removeEventListener("hashchange", applyRoute);
    };
  }, []);

  useEffect(() => {
    if (
      remoteSubpage !== "processes" ||
      initialTargetIntent?.destination !== "processes" ||
      routedProcessIntentRef.current === initialTargetIntent.requestId
    ) {
      return;
    }
    routedProcessIntentRef.current = initialTargetIntent.requestId;
    setProcessWorkspaceRoute("host", initialTargetIntent.clientId, "replace");
    setProcessRoute({
      clientId: initialTargetIntent.clientId,
      mode: "host",
    });
    onInitialTargetIntentConsumed?.(initialTargetIntent.requestId);
  }, [
    initialTargetIntent,
    onInitialTargetIntentConsumed,
    remoteSubpage,
  ]);

  function updateProcessRoute(
    mode: ProcessWorkspaceMode,
    clientId = processRoute.clientId,
    historyMode: "push" | "replace" = "push",
  ) {
    setProcessWorkspaceRoute(mode, clientId, historyMode);
    setProcessRoute({ clientId, mode });
  }

  function openProcessComposer(preset: JobDispatchPresetInput) {
    setProcessComposerPreset({ ...preset, requestId: crypto.randomUUID() });
  }

  function openTransferComposer(preset: JobDispatchPresetInput) {
    setTransferComposerPreset({ ...preset, requestId: crypto.randomUUID() });
  }

  async function openTerminalSessionDirectly(
    request: DirectTerminalOpenRequest,
  ) {
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Privilege unlock required before opening a terminal.");
    }
    const session = request.session;
    const replayFromSeq = request.terminalReplayFromSeq?.trim();
    const operation: JobOperation = {
      type: "terminal_open",
      session_id: session.session_id,
      argv: session.argv,
      cwd: session.cwd ?? null,
      ...(request.terminalUser.trim()
        ? { user: request.terminalUser.trim() }
        : {}),
      user_policy: request.terminalUserPolicy,
      cols: session.cols ?? 120,
      rows: session.rows ?? 40,
      ...(replayFromSeq
        ? { replay_from_seq: Math.max(0, Math.trunc(Number(replayFromSeq))) }
        : {}),
      idle_timeout_secs: session.idle_timeout_secs ?? request.maxTimeoutSecs,
      flow_window_bytes: session.flow_window_bytes ?? 65536,
    };
    const selectorExpression = `id:${session.client_id}`;
    const commandType = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const { privilegeAssertion } = await buildPrivilegeForJobOperation({
      clientIds: [session.client_id],
      commandType,
      operation,
      privilegeMaterial,
      selectorExpression,
      maxTimeoutSecs: request.maxTimeoutSecs,
    });
    await onCreateJob({
      job_id: crypto.randomUUID(),
      selector_expression: selectorExpression,
      target_client_ids: [session.client_id],
      destructive: Boolean(
        JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE[operation.type],
      ),
      confirmed: true,
      command: commandType,
      argv: operation.argv,
      operation,
      max_timeout_secs: request.maxTimeoutSecs,
      privileged: true,
      privilege_assertion: privilegeAssertion,
    });
    onRefresh();
  }

  return (
    <section className="workspace singleColumn">
      <Suspense
        fallback={
          <div
            className="emptyState compactEmpty"
            role="status"
            aria-live="polite"
          >
            Loading {displayToken(remoteSubpage)} workspace
          </div>
        }
      >
        {remoteSubpage === "files" && (
          <FileBrowserPanel
            agents={agents}
            fileTransfers={fileTransfers}
            fleetEvidenceAvailable={fleetEvidenceAvailable}
            loading={loading}
            onCreateJob={onCreateJob}
            onLoadOutputs={onLoadOutputs}
            onLoadTargets={onLoadTargets}
            onOpenMultiFiles={(path) => {
              setMultiFileInitialPath(path);
              onSelectSubpage?.("bulk_files");
            }}
            onOpenTransfers={(path) => {
              setTransferFocusPath(path);
              onSelectSubpage?.("transfers");
            }}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            privilegeMaterial={privilegeMaterial}
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        )}
        {remoteSubpage === "multi_files" && (
          <MultiFileActionsPanel
            agents={agents}
            initialPath={multiFileInitialPath}
            loading={loading}
            onCreateJob={onCreateJob}
            onDownloadFileBundle={onDownloadFileBundle}
            onLoadOutputs={onLoadOutputs}
            onLoadTargets={onLoadTargets}
            onOpenJobDetails={onOpenJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onResolveTargets={onResolveTargets}
            privilegeMaterial={privilegeMaterial}
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        )}
        {remoteSubpage === "processes" && (
          <div className="jobConsoleStack">
            <div className="processWorkspaceModeBar">
              <div>
                <strong>Process scope</strong>
                <span>
                  {processRoute.mode === "host"
                    ? "Linux host inventory"
                    : "vpsman-managed lifecycle"}
                </span>
              </div>
              <div
                aria-label="Process scope"
                className="segmented"
                role="group"
              >
                <button
                  aria-pressed={processRoute.mode === "host"}
                  className={processRoute.mode === "host" ? "selected" : ""}
                  onClick={() => updateProcessRoute("host")}
                  title="Read processes reported by the selected Linux host"
                  type="button"
                >
                  Host
                </button>
                <button
                  aria-pressed={processRoute.mode === "managed"}
                  className={processRoute.mode === "managed" ? "selected" : ""}
                  onClick={() => updateProcessRoute("managed")}
                  title="Operate only processes started and supervised by vpsman"
                  type="button"
                >
                  Managed
                </button>
              </div>
            </div>
            {processRoute.mode === "host" ? (
              <HostProcessInventoryPanel
                agents={agents}
                clientLabel={clientLabel}
                onCreateJob={onCreateJob}
                onLoadInventory={onLoadHostProcessInventory}
                onLoadTargets={onLoadTargets}
                onSelectedClientIdChange={(clientId) =>
                  updateProcessRoute("host", clientId)
                }
                selectedClientId={processRoute.clientId}
              />
            ) : (
              <ProcessSupervisorInventoryPanel
                agents={agents}
                clientLabel={clientLabel}
                fleetEvidenceAvailable={fleetEvidenceAvailable}
                initialTargetClientId={
                  initialTargetIntent?.destination === "processes"
                    ? initialTargetIntent.clientId
                    : null
                }
                initialTargetRequestId={
                  initialTargetIntent?.destination === "processes"
                    ? initialTargetIntent.requestId
                    : null
                }
                inventory={processSupervisorInventory}
                inventoryTruncated={processSupervisorInventoryTruncated}
                loading={loading}
                onCreateJob={onCreateJob}
                onLoadTargets={onLoadTargets}
                onOpenDispatchPreset={openProcessComposer}
                onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
                onInitialTargetConsumed={onInitialTargetIntentConsumed}
                onRefresh={onRefresh}
                onSelectedClientIdChange={(clientId, historyMode) =>
                  updateProcessRoute("managed", clientId, historyMode)
                }
                privilegeMaterial={privilegeMaterial}
                selectedClientId={processRoute.clientId}
              />
            )}
            {processRoute.mode === "managed" && processComposerPreset ? (
              <ConsoleDetailPanel
                description="Start a process or read logs on the selected VPS without leaving the process workspace."
                onClose={() => setProcessComposerPreset(null)}
                title="Process operation"
              >
                <div className="terminalComposerAnchor">
                  <JobDispatchPanel
                    agents={agents}
                    fileTransferSources={fileTransferSources}
                    fileTransferSourcesTruncated={fileTransferSourcesTruncated}
                    commandTemplates={commandTemplates}
                    commandTemplatesTruncated={commandTemplatesTruncated}
                    dispatchPreset={processComposerPreset}
                    fixedMode="process_supervisor"
                    onCreateJob={onCreateJob}
                    onDeleteCommandTemplate={onDeleteCommandTemplate}
                    onDownloadFileTransferSource={onDownloadFileTransferSource}
                    onDownloadOutputChunk={onDownloadOutputChunk}
                    onLoadJob={onLoadJob}
                    onLoadOutputs={onLoadOutputs}
                    onLoadTargets={onLoadTargets}
                    onOpenJobDetails={onOpenJobDetails}
                    onOpenJobsDispatch={onOpenJobsDispatch}
                    onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
                    onResolveTargets={onResolveTargets}
                    onUpsertCommandTemplate={onUpsertCommandTemplate}
                    privilegeMaterial={privilegeMaterial}
                    setPrivilegeMaterial={setPrivilegeMaterial}
                  />
                </div>
              </ConsoleDetailPanel>
            ) : null}
          </div>
        )}
        {remoteSubpage === "services" && (
          <HostServicesPanel
            agents={agents}
            clientLabel={clientLabel}
            onCreateJob={onCreateJob}
            onDownloadOutputStream={onDownloadOutputStream}
            onLoadInventory={onLoadHostServiceInventory}
            onLoadTargets={onLoadTargets}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            privilegeMaterial={privilegeMaterial}
          />
        )}
        {remoteSubpage === "storage" && (
          <HostStoragePanel
            agents={agents}
            clientLabel={clientLabel}
            onCreateJob={onCreateJob}
            onLoadInventory={onLoadHostStorageInventory}
            onLoadTargets={onLoadTargets}
          />
        )}
        {remoteSubpage === "transfers" && (
          <div className="jobConsoleStack">
            <FileTransferSessionsPanel
              agents={agents}
              clientLabel={clientLabel}
              focusPath={transferFocusPath}
              initialUploadContext={transferTargetIntent?.context}
              initialUploadPath={transferTargetIntent?.path}
              initialUploadTargetClientId={transferTargetIntent?.clientId}
              transfers={fileTransfers}
              transfersTruncated={fileTransfersTruncated}
              sources={fileTransferSources}
              sourcesTruncated={fileTransferSourcesTruncated}
              loading={loading}
              onCreateHandoff={onCreateFileTransferHandoff}
              onDownloadSource={onDownloadFileTransferSource}
              onOpenDispatchPreset={openTransferComposer}
              onOpenJobDetails={onOpenJobDetails}
              onRefresh={onRefresh}
              onSaveHandoff={onSaveFileTransferHandoff}
              onInitialUploadTargetConsumed={onTransferTargetConsumed}
              onUploadSource={onUploadFileTransferSource}
            />
            {transferComposerPreset ? (
              <ConsoleDetailPanel
                description="Review and run the prefilled transfer without leaving transfer history."
                onClose={() => setTransferComposerPreset(null)}
                title="File transfer"
              >
                <div className="terminalComposerAnchor">
                  <JobDispatchPanel
                    agents={agents}
                    fileTransferSources={fileTransferSources}
                    fileTransferSourcesTruncated={fileTransferSourcesTruncated}
                    commandTemplates={commandTemplates}
                    commandTemplatesTruncated={commandTemplatesTruncated}
                    dispatchPreset={transferComposerPreset}
                    fixedMode={transferComposerPreset.mode}
                    onCreateJob={onCreateJob}
                    onDeleteCommandTemplate={onDeleteCommandTemplate}
                    onDownloadFileTransferSource={onDownloadFileTransferSource}
                    onDownloadOutputChunk={onDownloadOutputChunk}
                    onLoadJob={onLoadJob}
                    onLoadOutputs={onLoadOutputs}
                    onLoadTargets={onLoadTargets}
                    onOpenJobDetails={onOpenJobDetails}
                    onOpenJobsDispatch={onOpenJobsDispatch}
                    onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
                    onResolveTargets={onResolveTargets}
                    onUpsertCommandTemplate={onUpsertCommandTemplate}
                    privilegeMaterial={privilegeMaterial}
                    setPrivilegeMaterial={setPrivilegeMaterial}
                  />
                </div>
              </ConsoleDetailPanel>
            ) : null}
          </div>
        )}
        {remoteSubpage === "terminal" && (
          <div className="jobConsoleStack">
            <TerminalSessionsPanel
              agents={agents}
              clientLabel={clientLabel}
              initialTargetClientId={
                initialTargetIntent?.destination === "terminal"
                  ? initialTargetIntent.clientId
                  : null
              }
              initialTargetRequestId={
                initialTargetIntent?.destination === "terminal"
                  ? initialTargetIntent.requestId
                  : null
              }
              sessions={terminalSessions}
              sessionsTruncated={terminalSessionsTruncated}
              lastTerminalOutputEvent={lastTerminalOutputEvent}
              loading={loading}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onInitialTargetConsumed={onInitialTargetIntentConsumed}
              onOpenTerminal={openTerminalSessionDirectly}
              onControl={onControlTerminalSession}
              onReplay={onLoadTerminalReplay}
              onRefresh={onRefresh}
              onOpenSessionEvidence={onOpenSessionEvidence}
              privilegeMaterial={privilegeMaterial}
            />
          </div>
        )}
      </Suspense>
    </section>
  );
}

function remoteOperationsPanelSubpage(
  subpage: string,
): RemoteOperationsSubpage {
  if (
    subpage === "files" ||
    subpage === "multi_files" ||
    subpage === "transfers" ||
    subpage === "processes" ||
    subpage === "services" ||
    subpage === "storage" ||
    subpage === "terminal"
  ) {
    return subpage;
  }
  return "terminal";
}

type ProcessWorkspaceMode = "host" | "managed";

function readProcessWorkspaceRoute(): {
  clientId: string | null;
  mode: ProcessWorkspaceMode;
} {
  if (typeof window === "undefined") {
    return { clientId: null, mode: "host" };
  }
  const params = new URLSearchParams(window.location.search);
  const mode = params.get("process_mode") === "managed" ? "managed" : "host";
  const clientId = params.get("process_client")?.trim() || null;
  return { clientId, mode };
}

function setProcessWorkspaceRoute(
  mode: ProcessWorkspaceMode,
  clientId: string | null,
  historyMode: "push" | "replace",
) {
  if (typeof window === "undefined") {
    return;
  }
  const url = new URL(window.location.href);
  if (mode === "managed") {
    url.searchParams.set("process_mode", mode);
  } else {
    url.searchParams.delete("process_mode");
  }
  if (clientId) {
    url.searchParams.set("process_client", clientId);
  } else {
    url.searchParams.delete("process_client");
  }
  const next = `${url.pathname}${url.search}${url.hash}`;
  if (`${window.location.pathname}${window.location.search}${window.location.hash}` === next) {
    return;
  }
  if (historyMode === "replace") {
    replaceHistoryEntry(next);
  } else {
    pushHistoryEntry(next);
  }
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}
