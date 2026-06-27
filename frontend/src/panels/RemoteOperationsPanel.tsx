import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";
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
  TerminalInputSubmitRequest,
  TerminalInputSubmitResponse,
  TerminalReplayRecord,
  TerminalSessionRecord,
} from "../typesTerminal";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
} from "../utils";
import type { TerminalComposerAction } from "./JobDispatchPanel";

export type DirectTerminalOpenRequest = {
  maxTimeoutSecs: number;
  session: TerminalSessionRecord;
  terminalReplayFromSeq?: string;
  terminalUser: string;
  terminalUserPolicy: "fail" | "fallback";
};

const JobDispatchPanel = lazy(() =>
  import("./JobDispatchPanel").then((module) => ({
    default: module.JobDispatchPanel,
  })),
);
const FileBrowserPanel = lazy(() =>
  import("./jobs/FileBrowserPanel").then((module) => ({
    default: module.FileBrowserPanel,
  })),
);
const FileTransferSessionsPanel = lazy(() =>
  import("./jobs/FileTransferSessionsPanel").then((module) => ({
    default: module.FileTransferSessionsPanel,
  })),
);
const MultiFileActionsPanel = lazy(() =>
  import("./jobs/MultiFileActionsPanel").then((module) => ({
    default: module.MultiFileActionsPanel,
  })),
);
const ProcessSupervisorInventoryPanel = lazy(() =>
  import("./jobs/ProcessSupervisorInventoryPanel").then((module) => ({
    default: module.ProcessSupervisorInventoryPanel,
  })),
);
const TerminalSessionsPanel = lazy(() =>
  import("./jobs/TerminalSessionsPanel").then((module) => ({
    default: module.TerminalSessionsPanel,
  })),
);

type RemoteOperationsSubpage =
  | "terminal"
  | "files"
  | "multi_files"
  | "transfers"
  | "processes";

export function RemoteOperationsPanel({
  activeSubpage,
  agents,
  commandTemplates,
  dispatchPreset,
  fileTransfers,
  fileTransferSources,
  lastTerminalOutputEvent,
  loading,
  onCreateFileTransferHandoff,
  onCreateJob,
  onDownloadFileBundle,
  onDownloadFileTransferSource,
  onDownloadOutputChunk,
  onDispatchPresetApplied,
  onLoadJob,
  onLoadOutputs,
  onLoadTargets,
  onLoadTerminalReplay,
  onOpenDispatchPreset,
  onOpenJobDetails,
  onOpenJobsDispatch,
  onOpenPrivilegeUnlock,
  onOpenSessionEvidence,
  onRefresh,
  onResolveTargets,
  onSaveFileTransferHandoff,
  onSelectSubpage,
  onSubmitTerminalInput,
  onUploadFileTransferSource,
  onDeleteCommandTemplate,
  onUpsertCommandTemplate,
  privilegeMaterial,
  processSupervisorInventory,
  setPrivilegeMaterial,
  terminalSessions,
}: {
  activeSubpage: string;
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  dispatchPreset?: JobDispatchPreset | null;
  fileTransfers: FileTransferSessionRecord[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
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
  onDispatchPresetApplied?: () => void;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onLoadTerminalReplay: (
    clientId: string,
    sessionId: string,
    fromSeq?: number,
  ) => Promise<TerminalReplayRecord>;
  onOpenDispatchPreset: (preset: JobDispatchPresetInput) => void;
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
  onSubmitTerminalInput: (
    clientId: string,
    sessionId: string,
    request: TerminalInputSubmitRequest,
  ) => Promise<TerminalInputSubmitResponse>;
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
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  terminalSessions: TerminalSessionRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [terminalComposerAction, setTerminalComposerAction] =
    useState<TerminalComposerAction | null>(null);
  const [terminalAdvancedOpen, setTerminalAdvancedOpen] = useState(false);
  const terminalComposerRef = useRef<HTMLDivElement | null>(null);
  const [multiFileInitialPath, setMultiFileInitialPath] = useState("");
  const [transferFocusPath, setTransferFocusPath] = useState<string | null>(null);
  const remoteSubpage = remoteOperationsPanelSubpage(activeSubpage);
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);

  function prepareTerminalSessionAction(
    session: TerminalSessionRecord,
    action: TerminalComposerAction["action"],
    options: Omit<TerminalComposerAction, "action" | "requestId" | "session"> = {},
  ) {
    setTerminalAdvancedOpen(true);
    setTerminalComposerAction({
      action,
      ...options,
      requestId: crypto.randomUUID(),
      session,
    });
  }

  useEffect(() => {
    if (remoteSubpage !== "terminal" || !terminalComposerAction) {
      return;
    }
    window.requestAnimationFrame(() => {
      terminalComposerRef.current?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
  }, [remoteSubpage, terminalComposerAction?.requestId]);

  async function openTerminalSessionDirectly(request: DirectTerminalOpenRequest) {
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
          <div className="emptyState compactEmpty" role="status" aria-live="polite">
            Loading {displayToken(remoteSubpage)} workspace
          </div>
        }
      >
        {remoteSubpage === "files" && (
          <FileBrowserPanel
            agents={agents}
            fileTransfers={fileTransfers}
            loading={loading}
            onCreateJob={onCreateJob}
            onLoadOutputs={onLoadOutputs}
            onLoadTargets={onLoadTargets}
            onOpenMultiFiles={(path) => {
              setMultiFileInitialPath(path);
              onSelectSubpage?.("multi_files");
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
          <ProcessSupervisorInventoryPanel
            clientLabel={clientLabel}
            inventory={processSupervisorInventory}
            loading={loading}
            onCreateJob={onCreateJob}
            onOpenDispatchPreset={onOpenDispatchPreset}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onRefresh={onRefresh}
            privilegeMaterial={privilegeMaterial}
          />
        )}
        {remoteSubpage === "transfers" && (
          <FileTransferSessionsPanel
            agents={agents}
            clientLabel={clientLabel}
            focusPath={transferFocusPath}
            transfers={fileTransfers}
            sources={fileTransferSources}
            loading={loading}
            onCreateHandoff={onCreateFileTransferHandoff}
            onDownloadSource={onDownloadFileTransferSource}
            onOpenDispatchPreset={onOpenDispatchPreset}
            onOpenJobDetails={onOpenJobDetails}
            onRefresh={onRefresh}
            onSaveHandoff={onSaveFileTransferHandoff}
            onUploadSource={onUploadFileTransferSource}
          />
        )}
        {remoteSubpage === "terminal" && (
          <div className="jobConsoleStack">
            <TerminalSessionsPanel
              agents={agents}
              clientLabel={clientLabel}
              sessions={terminalSessions}
              lastTerminalOutputEvent={lastTerminalOutputEvent}
              loading={loading}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onOpenTerminal={openTerminalSessionDirectly}
              onPrepareAction={prepareTerminalSessionAction}
              onReplay={onLoadTerminalReplay}
              onRefresh={onRefresh}
              onOpenSessionEvidence={onOpenSessionEvidence}
              privilegeMaterial={privilegeMaterial}
            />
            <details
              className="terminalAdvancedComposer"
              onToggle={(event) =>
                setTerminalAdvancedOpen(event.currentTarget.open)
              }
              open={terminalAdvancedOpen}
            >
              <summary>Advanced session controls</summary>
              <div className="terminalComposerAnchor" ref={terminalComposerRef}>
                <JobDispatchPanel
                  agents={agents}
                  fileTransferSources={fileTransferSources}
                  commandTemplates={commandTemplates}
                  dispatchPreset={dispatchPreset}
                  fixedMode="terminal_session"
                  surface="terminal"
                  terminalComposerAction={terminalComposerAction}
                  onDispatchPresetApplied={onDispatchPresetApplied}
                  onCreateJob={onCreateJob}
                  onDownloadFileTransferSource={onDownloadFileTransferSource}
                  onDownloadOutputChunk={onDownloadOutputChunk}
                  onLoadJob={onLoadJob}
                  onLoadOutputs={onLoadOutputs}
                  onLoadTargets={onLoadTargets}
                  onSubmitTerminalInput={onSubmitTerminalInput}
                  onOpenJobDetails={onOpenJobDetails}
                  onOpenJobsDispatch={onOpenJobsDispatch}
                  onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
                  onResolveTargets={onResolveTargets}
                  onDeleteCommandTemplate={onDeleteCommandTemplate}
                  onUpsertCommandTemplate={onUpsertCommandTemplate}
                  privilegeMaterial={privilegeMaterial}
                  setPrivilegeMaterial={setPrivilegeMaterial}
                />
              </div>
            </details>
          </div>
        )}
      </Suspense>
    </section>
  );
}

function remoteOperationsPanelSubpage(subpage: string): RemoteOperationsSubpage {
  if (
    subpage === "files" ||
    subpage === "multi_files" ||
    subpage === "transfers" ||
    subpage === "processes" ||
    subpage === "terminal"
  ) {
    return subpage;
  }
  return "terminal";
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}
