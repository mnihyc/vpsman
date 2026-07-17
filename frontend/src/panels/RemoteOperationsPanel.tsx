import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { ConsoleDetailPanel } from "../components/ConsoleDetailPanel";
import type { ArtifactDownloadMode } from "../artifactDownload";
import {
  JOB_COMMAND_CONFIRMATION_REQUIRED_BY_OPERATION_TYPE,
  JOB_COMMAND_TYPE_BY_OPERATION_TYPE,
} from "../generated/protocolContracts";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildPrivilegeAssertion,
  buildPrivilegeForJobOperation,
  canonicalTerminalInputPrivilegeIntent,
  textPayloadHashHex,
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
import { clientDisplayNameFromMap, clientDisplayNameMap } from "../utils";
import { JobDispatchPanel, type TerminalComposerAction } from "./JobDispatchPanel";
import { retryableLazy } from "../lazyImport";

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
  | "processes";

export function RemoteOperationsPanel({
  activeSubpage,
  agents,
  commandTemplates,
  dispatchPreset,
  fileTransfers,
  fileTransferSources,
  initialTargetIntent,
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
  onInitialTargetIntentConsumed,
  onOpenJobDetails,
  onOpenJobsDispatch,
  onOpenPrivilegeUnlock,
  onOpenSessionEvidence,
  onRefresh,
  onResolveTargets,
  onSaveFileTransferHandoff,
  onSelectSubpage,
  onSubmitTerminalInput,
  onTransferTargetConsumed,
  onUploadFileTransferSource,
  onDeleteCommandTemplate,
  onUpsertCommandTemplate,
  privilegeMaterial,
  privilegeUnlockOpen,
  processSupervisorInventory,
  setPrivilegeMaterial,
  terminalSessions,
  transferTargetIntent,
}: {
  activeSubpage: string;
  agents: AgentView[];
  commandTemplates: CommandTemplateRecord[];
  dispatchPreset?: JobDispatchPreset | null;
  fileTransfers: FileTransferSessionRecord[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
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
  onDispatchPresetApplied?: () => void;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
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
  onSubmitTerminalInput: (
    clientId: string,
    sessionId: string,
    request: TerminalInputSubmitRequest,
  ) => Promise<TerminalInputSubmitResponse>;
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
  privilegeUnlockOpen: boolean;
  processSupervisorInventory: ProcessSupervisorInventoryRecord[];
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  terminalSessions: TerminalSessionRecord[];
  transferTargetIntent?: {
    clientId: string;
    context: string;
    path: string;
  } | null;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [terminalComposerAction, setTerminalComposerAction] =
    useState<TerminalComposerAction | null>(null);
  const [processComposerPreset, setProcessComposerPreset] =
    useState<JobDispatchPreset | null>(null);
  const [transferComposerPreset, setTransferComposerPreset] =
    useState<JobDispatchPreset | null>(null);
  const [terminalAdvancedOpen, setTerminalAdvancedOpen] = useState(false);
  const terminalComposerRef = useRef<HTMLDivElement | null>(null);
  const [multiFileInitialPath, setMultiFileInitialPath] = useState("");
  const [transferFocusPath, setTransferFocusPath] = useState<string | null>(
    null,
  );
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
    options: Omit<
      TerminalComposerAction,
      "action" | "requestId" | "session"
    > = {},
  ) {
    setTerminalAdvancedOpen(true);
    setTerminalComposerAction({
      action,
      ...options,
      requestId: crypto.randomUUID(),
      session,
    });
  }

  function openProcessComposer(preset: JobDispatchPresetInput) {
    setProcessComposerPreset({ ...preset, requestId: crypto.randomUUID() });
  }

  function openTransferComposer(preset: JobDispatchPresetInput) {
    setTransferComposerPreset({ ...preset, requestId: crypto.randomUUID() });
  }

  useEffect(() => {
    if (remoteSubpage !== "terminal" || !terminalComposerAction) {
      return;
    }
    window.requestAnimationFrame(() => {
      terminalComposerRef.current?.scrollIntoView({
        block: "start",
        behavior: "smooth",
      });
    });
  }, [remoteSubpage, terminalComposerAction?.requestId]);

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

  async function closeTerminalSessionDirectly(
    session: TerminalSessionRecord,
  ): Promise<void> {
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Unlock local privilege before closing a terminal.");
    }
    const maxTimeoutSecs = 30;
    const operation: JobOperation = {
      type: "terminal_close",
      session_id: session.session_id,
      reason: "operator_closed",
    };
    const selectorExpression = `id:${session.client_id}`;
    const commandType = JOB_COMMAND_TYPE_BY_OPERATION_TYPE[operation.type];
    const { privilegeAssertion } = await buildPrivilegeForJobOperation({
      clientIds: [session.client_id],
      commandType,
      operation,
      privilegeMaterial,
      selectorExpression,
      maxTimeoutSecs,
    });
    await onCreateJob({
      job_id: crypto.randomUUID(),
      selector_expression: selectorExpression,
      target_client_ids: [session.client_id],
      destructive: true,
      confirmed: true,
      command: commandType,
      argv: [],
      operation,
      max_timeout_secs: maxTimeoutSecs,
      privileged: true,
      privilege_assertion: privilegeAssertion,
    });
    onRefresh();
  }

  async function submitTerminalInputDirectly(
    session: TerminalSessionRecord,
    text: string,
  ): Promise<TerminalInputSubmitResponse> {
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Unlock local privilege before sending terminal input.");
    }
    if (!text) {
      throw new Error("Terminal input is empty.");
    }
    const maxTimeoutSecs = 30;
    const payloadHashHex = await textPayloadHashHex(text);
    const privilegeAssertion = await buildPrivilegeAssertion({
      intent: canonicalTerminalInputPrivilegeIntent({
        clientId: session.client_id,
        sessionId: session.session_id,
        inputPayloadHash: payloadHashHex,
        maxTimeoutSecs,
        confirmed: true,
      }),
      privilegeMaterial,
    });
    const response = await onSubmitTerminalInput(
      session.client_id,
      session.session_id,
      {
        job_id: crypto.randomUUID(),
        text,
        max_timeout_secs: maxTimeoutSecs,
        confirmed: true,
        privilege_assertion: privilegeAssertion,
      },
    );
    onRefresh();
    return response;
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
          <div className="jobConsoleStack">
            <ProcessSupervisorInventoryPanel
              agents={agents}
              clientLabel={clientLabel}
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
              loading={loading}
              onCreateJob={onCreateJob}
              onLoadTargets={onLoadTargets}
              onOpenDispatchPreset={openProcessComposer}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onInitialTargetConsumed={onInitialTargetIntentConsumed}
              onRefresh={onRefresh}
              privilegeMaterial={privilegeMaterial}
            />
            {processComposerPreset ? (
              <ConsoleDetailPanel
                description="Start a process or read logs on the selected VPS without leaving the process workspace."
                onClose={() => setProcessComposerPreset(null)}
                title="Process operation"
              >
                <div className="terminalComposerAnchor">
                  <JobDispatchPanel
                    agents={agents}
                    fileTransferSources={fileTransferSources}
                    commandTemplates={commandTemplates}
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
                    onSubmitTerminalInput={onSubmitTerminalInput}
                    onUpsertCommandTemplate={onUpsertCommandTemplate}
                    privilegeMaterial={privilegeMaterial}
                    setPrivilegeMaterial={setPrivilegeMaterial}
                  />
                </div>
              </ConsoleDetailPanel>
            ) : null}
          </div>
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
              sources={fileTransferSources}
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
                    commandTemplates={commandTemplates}
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
                    onSubmitTerminalInput={onSubmitTerminalInput}
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
              lastTerminalOutputEvent={lastTerminalOutputEvent}
              loading={loading}
              onCloseTerminal={closeTerminalSessionDirectly}
              onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
              onInitialTargetConsumed={onInitialTargetIntentConsumed}
              onOpenTerminal={openTerminalSessionDirectly}
              onPrepareAction={prepareTerminalSessionAction}
              onSendInput={submitTerminalInputDirectly}
              onReplay={onLoadTerminalReplay}
              onRefresh={onRefresh}
              onOpenSessionEvidence={onOpenSessionEvidence}
              privilegeMaterial={privilegeMaterial}
              privilegeUnlockOpen={privilegeUnlockOpen}
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

function remoteOperationsPanelSubpage(
  subpage: string,
): RemoteOperationsSubpage {
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
