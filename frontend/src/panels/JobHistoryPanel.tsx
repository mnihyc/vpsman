import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Server, ShieldCheck, TerminalSquare } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { CrudPager } from "../components/CrudPager";
import { usePanelDisplaySettings } from "../panelDisplay";
import { type PrivilegeMaterial } from "../privilege";
import type { ArtifactDownloadMode } from "../artifactDownload";
import type {
  AgentView,
  AgentUpdateReleaseRecord,
  BulkResolveResponse,
  CommandTemplateRecord,
  CreateJobRequest,
  CreateJobResponse,
  CreateAgentUpdateReleaseRequest,
  JobHistoryRecord,
  JobOutputCompareMode,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetSelection,
  ProcessSupervisorInventoryRecord,
  ArtifactCleanupPreviewRecord,
  ServerJobRecord,
  UpsertCommandTemplateRequest,
  WsJobOutputEvent,
  WsTerminalOutputEvent,
} from "../types";
import type {
  FileTransferHandoffRecord,
  FileTransferSessionRecord,
  FileTransferSourceArtifactRecord,
  UploadFileTransferSourceArtifactRequest,
} from "../typesFileTransfer";
import type {
  TerminalReplayRecord,
  TerminalSessionRecord,
} from "../typesTerminal";
import {
  jobOutputComparisonStatusBadgeClass,
  jobStatusBadgeClass,
  jobTargetStatusBadgeClass,
} from "../jobStatusPresentation";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  decodeOutputPreview,
  formatTime,
  runPanelAction,
  shortHash,
  shortId,
} from "../utils";
import {
  JobDispatchPanel,
  type TerminalComposerAction,
} from "./JobDispatchPanel";
import { parseLatestFileStatus } from "../fileBrowser";
import { AgentUpdateReleasesPanel } from "./jobs/AgentUpdateReleasesPanel";
import { FileBrowserPanel } from "./jobs/FileBrowserPanel";
import { FileTransferSessionsPanel } from "./jobs/FileTransferSessionsPanel";
import { MultiFileActionsPanel } from "./jobs/MultiFileActionsPanel";
import { ProcessSupervisorInventoryPanel } from "./jobs/ProcessSupervisorInventoryPanel";
import { ServerJobsPanel } from "./jobs/ServerJobsPanel";
import { TerminalSessionsPanel } from "./jobs/TerminalSessionsPanel";

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error ? error.message : fallback;
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

type OutputDownloadStream = "stdout" | "stderr" | "combined";

type OutputStreamDownloadTarget = {
  clientId: string;
  combined: boolean;
  stdout: boolean;
  stderr: boolean;
};

export function JobHistoryPanel({
  activeSubpage,
  agents,
  agentUpdateReleases,
  error,
  fileTransfers,
  fileTransferSources,
  jobs,
  commandTemplates,
  lastJobOutputEvent,
  lastTerminalOutputEvent,
  loading,
  onCancelServerJob,
  onCreateAgentUpdateRelease,
  onCreateArtifactCleanupJob,
  onCreateFileTransferHandoff,
  onCreateJob,
  onDownloadFileBundle,
  onDownloadOutputArchive,
  onDownloadTargetStatusArchive,
  onDownloadOutputChunk,
  onDownloadOutputStream,
  onDownloadFileForClient,
  onDownloadFileTransferSource,
  onLoadJob,
  onLoadOutputs,
  onLoadOutputComparison,
  onLoadTerminalReplay,
  onLoadTargets,
  onOpenPrivilegeUnlock,
  onPreviewArtifactCleanup,
  onRefresh,
  onResolveTargets,
  onSaveFileTransferHandoff,
  onSelectSubpage,
  onSelectedJobDetailsOpened,
  onUploadFileTransferSource,
  onUpsertCommandTemplate,
  pendingSelectedJobId,
  privilegeMaterial,
  processSupervisorInventory,
  serverJobs,
  setPrivilegeMaterial,
  terminalSessions,
}: {
  activeSubpage: string;
  agents: AgentView[];
  agentUpdateReleases: AgentUpdateReleaseRecord[];
  error: string | null;
  fileTransfers: FileTransferSessionRecord[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
  jobs: JobHistoryRecord[];
  commandTemplates: CommandTemplateRecord[];
  lastJobOutputEvent: WsJobOutputEvent | null;
  lastTerminalOutputEvent: WsTerminalOutputEvent | null;
  loading: boolean;
  onCancelServerJob: (jobId: string) => Promise<ServerJobRecord>;
  onCreateAgentUpdateRelease: (
    request: CreateAgentUpdateReleaseRequest,
  ) => Promise<AgentUpdateReleaseRecord>;
  onCreateArtifactCleanupJob: (
    expression: string,
    previewHash: string,
  ) => Promise<ServerJobRecord>;
  onCreateFileTransferHandoff: (
    clientId: string,
    sessionId: string,
  ) => Promise<FileTransferHandoffRecord>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDownloadOutputChunk: (
    jobId: string,
    clientId: string,
    seq: number,
  ) => Promise<Blob>;
  onDownloadOutputStream: (
    jobId: string,
    clientId: string,
    stream: OutputDownloadStream,
  ) => Promise<Blob>;
  onDownloadFileForClient: (jobId: string, clientId: string) => Promise<Blob>;
  onDownloadOutputArchive: (jobId: string, clientIds: string[]) => Promise<Blob>;
  onDownloadTargetStatusArchive: (jobId: string) => Promise<Blob>;
  onDownloadFileBundle: (jobId: string, clientIds: string[]) => Promise<Blob>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadOutputComparison: (
    jobId: string,
    mode: JobOutputCompareMode,
  ) => Promise<JobOutputComparisonRecord>;
  onLoadTerminalReplay: (
    clientId: string,
    sessionId: string,
    fromSeq?: number,
  ) => Promise<TerminalReplayRecord>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenPrivilegeUnlock: () => void;
  onPreviewArtifactCleanup: (
    expression: string,
  ) => Promise<ArtifactCleanupPreviewRecord>;
  onRefresh: () => void;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
  onSelectedJobDetailsOpened?: (jobId: string) => void;
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
  onUploadFileTransferSource: (
    request: UploadFileTransferSourceArtifactRequest,
  ) => Promise<FileTransferSourceArtifactRecord>;
  onUpsertCommandTemplate: (
    request: UpsertCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  pendingSelectedJobId?: string | null;
  privilegeMaterial: PrivilegeMaterial | null;
  processSupervisorInventory: ProcessSupervisorInventoryRecord[];
  serverJobs: ServerJobRecord[];
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  terminalSessions: TerminalSessionRecord[];
}) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [targets, setTargets] = useState<JobTargetRecord[]>([]);
  const [outputs, setOutputs] = useState<JobOutputRecord[]>([]);
  const [outputComparison, setOutputComparison] =
    useState<JobOutputComparisonRecord | null>(null);
  const [comparisonMode, setComparisonMode] = useState<JobOutputCompareMode>(
    preferences.bulk_output_compare_mode,
  );
  const [selectedComparisonGroupId, setSelectedComparisonGroupId] = useState<
    string | null
  >(null);
  const [targetError, setTargetError] = useState<string | null>(null);
  const [outputError, setOutputError] = useState<string | null>(null);
  const [comparisonError, setComparisonError] = useState<string | null>(null);
  const [targetsLoading, setTargetsLoading] = useState(false);
  const [outputsLoading, setOutputsLoading] = useState(false);
  const [comparisonLoading, setComparisonLoading] = useState(false);
  const [terminalComposerAction, setTerminalComposerAction] =
    useState<TerminalComposerAction | null>(null);
  const [multiFileInitialPath, setMultiFileInitialPath] = useState("/");
  const jobSubpage = [
    "history",
    "dispatch",
    "files",
    "multi_files",
    "updates",
    "transfers",
    "terminal",
    "processes",
    "server_jobs",
    "approvals",
  ].includes(activeSubpage)
    ? activeSubpage
    : "history";
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [streamPendingKey, setStreamPendingKey] = useState<string | null>(null);
  const [fileDownloadPendingClientId, setFileDownloadPendingClientId] =
    useState<string | null>(null);
  const [archivePendingKey, setArchivePendingKey] = useState<
    "files" | "outputs" | "status" | null
  >(null);
  const scheduleRunJobs = jobs.filter((job) => job.command_type.startsWith("scheduled_"));
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);
  const fileDownloadStatusByClient = useMemo(() => {
    const byClient = new Map<string, JobOutputRecord[]>();
    for (const output of outputs) {
      const clientOutputs = byClient.get(output.client_id);
      if (clientOutputs) {
        clientOutputs.push(output);
      } else {
        byClient.set(output.client_id, [output]);
      }
    }
    const statusByClient = new Map<string, ReturnType<typeof parseLatestFileStatus>>();
    for (const [clientId, clientOutputs] of byClient) {
      const status = parseLatestFileStatus(clientOutputs, "file_download");
      if (
        status &&
        status.type === "file_download" &&
        (status.status ?? "completed") === "completed" &&
        hasCompleteRetainedOutputStream(clientOutputs, "stdout")
      ) {
        statusByClient.set(clientId, status);
      }
    }
    return statusByClient;
  }, [outputs]);
  const fileDownloadStatus = fileDownloadStatusByClient.size > 0;
  const outputStreamDownloadTargets = useMemo<OutputStreamDownloadTarget[]>(() => {
    const outputsByClient = new Map<string, JobOutputRecord[]>();
    for (const output of outputs) {
      const clientOutputs = outputsByClient.get(output.client_id);
      if (clientOutputs) {
        clientOutputs.push(output);
      } else {
        outputsByClient.set(output.client_id, [output]);
      }
    }
    const targets: OutputStreamDownloadTarget[] = [];
    for (const [clientId, clientOutputs] of outputsByClient) {
      const stdout = hasCompleteRetainedOutputStream(clientOutputs, "stdout");
      const stderr = hasCompleteRetainedOutputStream(clientOutputs, "stderr");
      const hasDeletedPayload = clientOutputs.some(
        (output) =>
          matchesOutputPayloadStream(output.stream) &&
          output.storage === "artifact_deleted",
      );
      if (stdout || stderr) {
        targets.push({
          clientId,
          combined: !hasDeletedPayload,
          stdout,
          stderr,
        });
      }
    }
    return targets.sort((left, right) =>
      clientLabel(left.clientId).localeCompare(clientLabel(right.clientId)),
    );
  }, [outputs, agentNameById]);
  const displayedComparisonRows = useMemo(() => {
    if (!outputComparison) {
      return [];
    }
    if (!selectedComparisonGroupId) {
      return outputComparison.rows;
    }
    return outputComparison.rows.filter(
      (row) => row.group_id === selectedComparisonGroupId,
    );
  }, [outputComparison, selectedComparisonGroupId]);

  useEffect(() => {
    setComparisonMode(preferences.bulk_output_compare_mode);
  }, [preferences.bulk_output_compare_mode]);

  const openTargets = useCallback(
    async (jobId: string) => {
      setSelectedJobId(jobId);
      setTargetsLoading(true);
      setOutputsLoading(true);
      setComparisonLoading(true);
      setTargetError(null);
      setOutputError(null);
      setComparisonError(null);
      setSelectedComparisonGroupId(null);
      const [targetResult, outputResult, comparisonResult] =
        await Promise.allSettled([
          onLoadTargets(jobId),
          onLoadOutputs(jobId),
          onLoadOutputComparison(jobId, comparisonMode),
        ]);
      if (targetResult.status === "fulfilled") {
        setTargets(targetResult.value);
      } else {
        setTargets([]);
        setTargetError(errorMessage(targetResult.reason, "Job target history unavailable"));
      }
      if (outputResult.status === "fulfilled") {
        setOutputs(outputResult.value);
      } else {
        setOutputs([]);
        setOutputError(errorMessage(outputResult.reason, "Job output unavailable"));
      }
      if (comparisonResult.status === "fulfilled") {
        setOutputComparison(comparisonResult.value);
      } else {
        setOutputComparison(null);
        setComparisonError(errorMessage(comparisonResult.reason, "Execution summary unavailable"));
      }
      setTargetsLoading(false);
      setOutputsLoading(false);
      setComparisonLoading(false);
    },
    [comparisonMode, onLoadOutputComparison, onLoadOutputs, onLoadTargets],
  );

  function openSubmittedJobDetails(jobId: string) {
    onSelectSubpage?.("history");
    void openTargets(jobId);
  }

  useEffect(() => {
    if (!pendingSelectedJobId) {
      return;
    }
    onSelectSubpage?.("history");
    void openTargets(pendingSelectedJobId).finally(() => onSelectedJobDetailsOpened?.(pendingSelectedJobId));
  }, [onSelectSubpage, onSelectedJobDetailsOpened, openTargets, pendingSelectedJobId]);

  const jobColumns = useMemo<ConsoleDataGridColumn<JobHistoryRecord>[]>(
    () => [
      {
        id: "command",
        header: "Command",
        size: 250,
        minSize: 190,
        sortValue: (job) => job.command_type,
        searchValue: (job) => `${job.command_type} ${job.id}`,
        cell: (job) => (
          <span className="historyPrimary">
            <strong title={job.command_type}>
              {displayToken(job.command_type)}
            </strong>
            <small>{shortId(job.id)}</small>
          </span>
        ),
      },
      {
        id: "status",
        header: "Status",
        size: 180,
        minSize: 160,
        sortValue: (job) => job.status,
        searchValue: (job) => job.status,
        cell: (job) => (
          <span className="jobStatusCell">
            <span
              className={`status ${jobStatusBadgeClass(job.status)}`}
              title={job.status}
            >
              {displayToken(job.status)}
            </span>
          </span>
        ),
      },
      {
        id: "targets",
        header: "Targets",
        size: 80,
        minSize: 70,
        align: "end",
        sortValue: (job) => job.target_count,
        searchValue: (job) => job.target_count,
        cell: (job) => (
          <button
            className="linkButton"
            onClick={(event) => {
              event.stopPropagation();
              void openTargets(job.id);
            }}
            type="button"
          >
            {job.target_count}
          </button>
        ),
      },
      {
        id: "privilege",
        header: "Privilege",
        size: 105,
        minSize: 90,
        sortValue: (job) => (job.privileged ? "privileged" : "none"),
        searchValue: (job) =>
          job.privileged ? "privileged" : "none unprivileged",
        cell: (job) => (
          <span className={job.privileged ? "status info" : "status neutral"}>
            {job.privileged ? "privileged" : "none"}
          </span>
        ),
      },
      {
        id: "payload",
        header: "Payload",
        size: 120,
        minSize: 110,
        sortValue: (job) => job.payload_hash,
        searchValue: (job) => job.payload_hash,
        cell: (job) => (
          <span className="monoValue">{shortHash(job.payload_hash)}</span>
        ),
      },
      {
        id: "created",
        header: "Created",
        size: 210,
        minSize: 170,
        sortValue: (job) => job.created_at,
        searchValue: (job) => job.created_at,
        cell: (job) => formatTime(job.created_at),
      },
    ],
    [openTargets],
  );

  async function compareSelectedJobOutputs(
    jobId: string,
    mode: JobOutputCompareMode = comparisonMode,
  ) {
    setComparisonLoading(true);
    setComparisonError(null);
    try {
      setOutputComparison(await onLoadOutputComparison(jobId, mode));
    } catch (loadError) {
      setOutputComparison(null);
      setComparisonError(
        loadError instanceof Error
          ? loadError.message
          : "Output comparison unavailable",
      );
    } finally {
      setComparisonLoading(false);
    }
  }

  function changeComparisonMode(mode: JobOutputCompareMode) {
    setComparisonMode(mode);
    setSelectedComparisonGroupId(null);
    if (selectedJobId) {
      void compareSelectedJobOutputs(selectedJobId, mode);
    }
  }

  useEffect(() => {
    if (lastJobOutputEvent && selectedJobId === lastJobOutputEvent.job_id) {
      void openTargets(lastJobOutputEvent.job_id);
    }
  }, [lastJobOutputEvent, openTargets, selectedJobId]);

  async function downloadOutputStreamForClient(
    clientId: string,
    stream: OutputDownloadStream,
  ) {
    if (!selectedJobId) {
      return;
    }
    const pendingKey = `${clientId}:${stream}`;
    setStreamPendingKey(pendingKey);
    await runPanelAction(
      () => undefined,
      setDownloadError,
      async () => {
        const blob = await onDownloadOutputStream(
          selectedJobId,
          clientId,
          stream,
        );
        saveBlob(blob, `job-output-${shortId(selectedJobId)}-${safeDownloadName(clientId)}-${stream}.bin`);
      },
    );
    setStreamPendingKey(null);
  }

  async function downloadFileForClient(clientId: string) {
    if (!selectedJobId) {
      return;
    }
    const status = fileDownloadStatusByClient.get(clientId);
    const filename = safeDownloadName(
      status?.filename,
      `file-download-${shortId(selectedJobId)}-${clientId}.bin`,
    );
    setFileDownloadPendingClientId(clientId);
    await runPanelAction(
      () => undefined,
      setDownloadError,
      async () => {
        const blob = await onDownloadFileForClient(selectedJobId, clientId);
        saveBlob(blob, filename);
      },
    );
    setFileDownloadPendingClientId(null);
  }

  async function downloadSelectedJobArchive(kind: "files" | "outputs" | "status") {
    if (!selectedJobId) {
      return;
    }
    setArchivePendingKey(kind);
    await runPanelAction(
      () => undefined,
      setDownloadError,
      async () => {
        const blob =
          kind === "files"
            ? await onDownloadFileBundle(selectedJobId, [])
            : kind === "outputs"
              ? await onDownloadOutputArchive(selectedJobId, [])
              : await onDownloadTargetStatusArchive(selectedJobId);
        saveBlob(
          blob,
          kind === "files"
            ? `file-download-${shortId(selectedJobId)}.tar`
            : kind === "outputs"
              ? `job-outputs-${shortId(selectedJobId)}.tar`
              : `job-status-${shortId(selectedJobId)}.tar`,
        );
      },
    );
    setArchivePendingKey(null);
  }

  function prepareTerminalSessionAction(
    session: TerminalSessionRecord,
    action: TerminalComposerAction["action"],
  ) {
    setTerminalComposerAction({
      action,
      requestId: crypto.randomUUID(),
      session,
    });
  }

  return (
    <section className="workspace singleColumn">
      {jobSubpage === "dispatch" && (
        <JobDispatchPanel
          agents={agents}
          fileTransferSources={fileTransferSources}
          commandTemplates={commandTemplates}
          terminalComposerAction={terminalComposerAction}
          onCreateJob={onCreateJob}
          onDownloadFileTransferSource={onDownloadFileTransferSource}
          onDownloadOutputChunk={onDownloadOutputChunk}
          onLoadJob={onLoadJob}
          onLoadOutputs={onLoadOutputs}
          onLoadTargets={onLoadTargets}
          onOpenJobDetails={openSubmittedJobDetails}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          onResolveTargets={onResolveTargets}
          onUpsertCommandTemplate={onUpsertCommandTemplate}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
        />
      )}
      {jobSubpage === "files" && (
        <FileBrowserPanel
          agents={agents}
          loading={loading}
          onCreateJob={onCreateJob}
          onLoadOutputs={onLoadOutputs}
          onLoadTargets={onLoadTargets}
          onOpenMultiFiles={(path) => {
            setMultiFileInitialPath(path);
            onSelectSubpage?.("multi_files");
          }}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
        />
      )}
      {jobSubpage === "multi_files" && (
        <MultiFileActionsPanel
          agents={agents}
          initialPath={multiFileInitialPath}
          loading={loading}
          onCreateJob={onCreateJob}
          onDownloadFileBundle={onDownloadFileBundle}
          onLoadOutputs={onLoadOutputs}
          onLoadTargets={onLoadTargets}
          onOpenJobDetails={openSubmittedJobDetails}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          onResolveTargets={onResolveTargets}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
        />
      )}
      {jobSubpage === "updates" && (
        <div className="jobConsoleStack">
          <AgentUpdateReleasesPanel
            loading={loading}
            onCreateAgentUpdateRelease={onCreateAgentUpdateRelease}
            onRefresh={onRefresh}
            releases={agentUpdateReleases}
          />
        </div>
      )}
      {jobSubpage === "processes" && (
        <ProcessSupervisorInventoryPanel
          clientLabel={clientLabel}
          inventory={processSupervisorInventory}
          loading={loading}
          onRefresh={onRefresh}
        />
      )}
      {jobSubpage === "server_jobs" && (
        <ServerJobsPanel
          jobs={serverJobs}
          loading={loading}
          onCancelJob={onCancelServerJob}
          onCreateCleanupJob={onCreateArtifactCleanupJob}
          onPreviewCleanup={onPreviewArtifactCleanup}
          onRefresh={onRefresh}
        />
      )}
      {jobSubpage === "transfers" && (
        <FileTransferSessionsPanel
          clientLabel={clientLabel}
          transfers={fileTransfers}
          sources={fileTransferSources}
          loading={loading}
          onCreateHandoff={onCreateFileTransferHandoff}
          onDownloadSource={onDownloadFileTransferSource}
          onRefresh={onRefresh}
          onSaveHandoff={onSaveFileTransferHandoff}
          onUploadSource={onUploadFileTransferSource}
        />
      )}
      {jobSubpage === "terminal" && (
        <div className="jobConsoleStack">
          <TerminalSessionsPanel
            clientLabel={clientLabel}
            sessions={terminalSessions}
            lastTerminalOutputEvent={lastTerminalOutputEvent}
            loading={loading}
            onPrepareAction={prepareTerminalSessionAction}
            onReplay={onLoadTerminalReplay}
            onRefresh={onRefresh}
          />
          <JobDispatchPanel
            agents={agents}
            fileTransferSources={fileTransferSources}
            commandTemplates={commandTemplates}
            terminalComposerAction={terminalComposerAction}
            onCreateJob={onCreateJob}
            onDownloadFileTransferSource={onDownloadFileTransferSource}
            onDownloadOutputChunk={onDownloadOutputChunk}
            onLoadJob={onLoadJob}
            onLoadOutputs={onLoadOutputs}
            onLoadTargets={onLoadTargets}
            onOpenJobDetails={openSubmittedJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onResolveTargets={onResolveTargets}
            onUpsertCommandTemplate={onUpsertCommandTemplate}
            privilegeMaterial={privilegeMaterial}
            setPrivilegeMaterial={setPrivilegeMaterial}
          />
        </div>
      )}
      {jobSubpage === "history" && (
        <div className="jobConsoleStack">
          <div className="fleetPanel">
            <div className="sectionHeader">
              <div>
                <h2>Job history</h2>
                <span>
                  {error ??
                    (loading
                      ? "Refreshing command records"
                      : "Latest privileged requests")}
                </span>
              </div>
              <button
                className="secondaryAction"
                disabled={loading}
                onClick={onRefresh}
                type="button"
              >
                Refresh
              </button>
            </div>
            <ConsoleDataGrid
              actions={[
                {
                  label: "Open target detail",
                  disabled: (rows) => rows.length !== 1,
                  onSelect: (rows) => void openTargets(rows[0].id),
                },
                {
                  label: "Copy job IDs",
                  onSelect: (rows) =>
                    void copyText(rows.map((job) => job.id).join("\n")),
                },
              ]}
              columns={jobColumns}
              defaultPageSize={12}
              empty={
                <div className="emptyState">
                  <TerminalSquare size={22} />
                  <strong>No job records</strong>
                  <span>
                    {error ?? "No job records match the current search."}
                  </span>
                </div>
              }
              getRowId={(job) => job.id}
              itemLabel="jobs"
              onOpenRow={(job) => void openTargets(job.id)}
              renderExpandedRow={(job) => (
                <div className="gridDetailLine">
                  <strong>{displayToken(job.command_type)}</strong>
                  <span>{job.target_count} targets</span>
                  <span>{displayToken(job.status)}</span>
                  <span>{shortHash(job.payload_hash)}</span>
                  <span>{formatTime(job.created_at)}</span>
                </div>
              )}
              rows={jobs}
              storageKey="vpsman.grid.jobs.history"
              title="Job records"
            />
          </div>
          {selectedJobId && (
            <div className="targetDetail">
              <div className="sectionHeader compact">
                <h2>Target results</h2>
                <span>
                  {targetError ??
                    (targetsLoading
                      ? "Loading target records"
                      : shortId(selectedJobId))}
                </span>
              </div>
              <CrudPager
                fields={[
                  {
                    label: "Client",
                    value: (target) => clientLabel(target.client_id),
                  },
                  { label: "Status", value: (target) => target.status },
                  { label: "Reason", value: (target) => target.message ?? "" },
                  { label: "Exit", value: (target) => target.exit_code },
                  {
                    label: "Completed",
                    value: (target) => target.completed_at,
                  },
                  { label: "Job", value: (target) => target.job_id },
                ]}
                itemLabel="targets"
                items={targets}
                pageSize={10}
                title="Target result records"
                empty={
                  <div className="emptyState">
                    <Server size={22} />
                    <strong>No target records</strong>
                    <span>
                      {targetError ??
                        "This job has no resolved per-client records."}
                    </span>
                  </div>
                }
              >
                {(targetRows) => (
                  <div className="table historyTable">
                    <div className="historyRow heading targetHistoryGrid">
                      <span>Client</span>
                      <span>Status</span>
                      <span>Reason</span>
                      <span>Exit</span>
                      <span>Completed</span>
                      <span>Actions</span>
                    </div>
                    {targetRows.map((target) => (
                      <div
                        className="historyRow targetHistoryGrid"
                        key={`${target.job_id}:${target.client_id}`}
                      >
                        <span className="historyPrimary">
                          <strong>{clientLabel(target.client_id)}</strong>
                          <small>{shortId(target.job_id)}</small>
                        </span>
                        <span
                          className={`status ${jobTargetStatusBadgeClass(target.status)}`}
                        >
                          {target.status}
                        </span>
                        <span title={target.message ?? undefined}>{target.message ?? "-"}</span>
                        <span>{target.exit_code ?? "-"}</span>
                        <span>
                          {target.completed_at
                            ? formatTime(target.completed_at)
                            : "-"}
                        </span>
                        <span className="inlineActions">
                          {fileDownloadStatusByClient.has(target.client_id) ? (
                            <button
                              className="secondaryAction compactAction"
                              disabled={
                                fileDownloadPendingClientId === target.client_id
                              }
                              onClick={() =>
                                void downloadFileForClient(target.client_id)
                              }
                              type="button"
                            >
                              <Download size={14} />
                              <span>
                                {fileDownloadPendingClientId ===
                                target.client_id
                                  ? "Downloading"
                                  : "Download file"}
                              </span>
                            </button>
                          ) : (
                            "-"
                          )}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </CrudPager>
              <div className="outputDetail">
                <div className="sectionHeader compact">
                  <div>
                    <h2>Output</h2>
                    <span>
                      {outputError ??
                        downloadError ??
                        (outputsLoading
                          ? "Loading output records"
                          : `${outputs.length} chunks`)}
                    </span>
                  </div>
                  <div className="outputActions">
                    {fileDownloadStatus && (
                      <button
                        className="secondaryAction compactAction"
                        disabled={outputsLoading || archivePendingKey !== null}
                        onClick={() => void downloadSelectedJobArchive("files")}
                        type="button"
                      >
                        <Download size={14} />
                        <span>
                          {archivePendingKey === "files"
                            ? "Downloading"
                            : "Download files"}
                        </span>
                      </button>
                    )}
                    {outputs.length > 0 && (
                      <button
                        className="secondaryAction compactAction"
                        disabled={outputsLoading || archivePendingKey !== null}
                        onClick={() => void downloadSelectedJobArchive("outputs")}
                        type="button"
                      >
                        <Download size={14} />
                        <span>
                          {archivePendingKey === "outputs"
                            ? "Downloading"
                            : "Download outputs"}
                        </span>
                      </button>
                    )}
                    {targets.length > 0 && (
                      <button
                        className="secondaryAction compactAction"
                        disabled={targetsLoading || archivePendingKey !== null}
                        onClick={() => void downloadSelectedJobArchive("status")}
                        type="button"
                      >
                        <Download size={14} />
                        <span>
                          {archivePendingKey === "status"
                            ? "Downloading"
                            : "Download status"}
                        </span>
                      </button>
                    )}
                  </div>
                </div>
                <div className="executionSummary">
                  <div className="sectionHeader compact">
                    <h2>Execution summary</h2>
                    <span>
                      {comparisonError ??
                        (comparisonLoading
                          ? "Comparing target results"
                          : outputComparison
                            ? `${outputComparison.group_count} groups across ${outputComparison.compared_targets} targets`
                            : "No summary loaded")}
                    </span>
                  </div>
                  <div className="comparisonToolbar">
                    <div
                      className="targetModeControls"
                      role="group"
                      aria-label="Output comparison mode"
                    >
                      <span>Compare</span>
                      <button
                        className={comparisonMode === "binary" ? "selected" : ""}
                        onClick={() => changeComparisonMode("binary")}
                        type="button"
                      >
                        Binary
                      </button>
                      <button
                        className={comparisonMode === "text" ? "selected" : ""}
                        onClick={() => changeComparisonMode("text")}
                        type="button"
                      >
                        Text
                      </button>
                    </div>
                    <button
                      className="secondaryAction compactAction"
                      disabled={comparisonLoading}
                      onClick={() => void compareSelectedJobOutputs(selectedJobId)}
                      type="button"
                    >
                      Refresh summary
                    </button>
                    {selectedComparisonGroupId && (
                      <button
                        className="secondaryAction compactAction"
                        onClick={() => setSelectedComparisonGroupId(null)}
                        type="button"
                      >
                        Show all targets
                      </button>
                    )}
                  </div>
                  {outputComparison && (
                    <div className="executionSummaryStats">
                      <span>
                        <strong>{outputComparison.group_count}</strong>
                        groups
                      </span>
                      <span>
                        <strong>{outputComparison.total_targets}</strong>
                        targets
                      </span>
                      <span>
                        <strong>{outputComparison.mode}</strong>
                        compare mode
                      </span>
                      <span>
                        <strong>{formatComparisonTime(outputComparison.compared_at)}</strong>
                        compared
                      </span>
                    </div>
                  )}
                  {outputComparison && outputComparison.groups.length > 0 && (
                    <CrudPager
                      fields={[
                        {
                          label: "Status",
                          value: (group) => `${group.status} ${group.exit_code ?? "-"}`,
                        },
                        {
                          label: "Targets",
                          value: (group) => group.client_ids.map(clientLabel).join(" "),
                        },
                        {
                          label: "Digest",
                          value: (group) => group.output_digest_hex,
                        },
                        { label: "Preview", value: (group) => group.preview },
                      ]}
                      itemLabel="groups"
                      items={outputComparison.groups}
                      pageSize={6}
                      title="Grouped outcomes"
                    >
                      {(groups) => (
                        <div className="table historyTable">
                          <div className="historyRow heading comparisonGroupGrid">
                            <span>Outcome</span>
                            <span>Targets</span>
                            <span>Output</span>
                            <span>Digest</span>
                          </div>
                          {groups.map((group) => (
                            <div
                              className={`historyRow comparisonGroupGrid clickableRow ${
                                selectedComparisonGroupId === group.group_id ? "selected" : ""
                              }`}
                              key={group.group_id}
                              onClick={() => setSelectedComparisonGroupId(group.group_id)}
                              onKeyDown={(event) => {
                                if (event.key === "Enter" || event.key === " ") {
                                  event.preventDefault();
                                  setSelectedComparisonGroupId(group.group_id);
                                }
                              }}
                              role="button"
                              tabIndex={0}
                            >
                              <span className="historyPrimary">
                                <strong className={`status ${jobOutputComparisonStatusBadgeClass(group.status)}`}>
                                  {group.status}
                                </strong>
                                <small>exit {group.exit_code ?? "-"}</small>
                              </span>
                              <span className="historyPrimary">
                                <strong>{group.target_count} targets</strong>
                                <small>{clientLabel(group.representative_client_id)}</small>
                              </span>
                              <span className="historyPrimary">
                                <strong>{outputCompareBasisLabel(group.output_compare_basis)}</strong>
                                <small>
                                  {group.stream_count} chunks / {formatBytes(group.byte_count)}
                                </small>
                              </span>
                              <span className="monoValue" title={group.preview}>
                                {shortHash(group.output_digest_hex)}
                              </span>
                            </div>
                          ))}
                        </div>
                      )}
                    </CrudPager>
                  )}
                  {outputComparison && displayedComparisonRows.length > 0 && (
                    <CrudPager
                      fields={[
                        {
                          label: "Client",
                          value: (row) => clientLabel(row.client_id),
                        },
                        {
                          label: "Status",
                          value: (row) => `${row.status} ${row.exit_code ?? "-"}`,
                        },
                        {
                          label: "Group",
                          value: (row) => row.group_id,
                        },
                        { label: "Digest", value: (row) => row.output_digest_hex },
                      ]}
                      itemLabel="targets"
                      items={displayedComparisonRows}
                      pageSize={8}
                      title={
                        selectedComparisonGroupId
                          ? `Targets in ${selectedComparisonGroupId}`
                          : "Target result details"
                      }
                    >
                      {(comparisonRows) => (
                        <div className="table historyTable">
                          <div className="historyRow heading comparisonTargetGrid">
                            <span>Client</span>
                            <span>Status</span>
                            <span>Group</span>
                            <span>Digest</span>
                          </div>
                          {comparisonRows.map((row) => (
                            <div
                              className="historyRow comparisonTargetGrid"
                              key={row.client_id}
                            >
                              <span className="historyPrimary">
                                <strong>{clientLabel(row.client_id)}</strong>
                                <small>
                                  {row.stream_count} chunks / {formatBytes(row.byte_count)}
                                </small>
                              </span>
                              <span className={`status ${jobOutputComparisonStatusBadgeClass(row.status)}`}>
                                {row.status} / {row.exit_code ?? "-"}
                              </span>
                              <span className={row.matches_largest_group ? "status ok" : "status warn"}>
                                {row.matches_largest_group ? "largest" : row.group_id}
                              </span>
                              <span className="monoValue" title={row.preview}>
                                {shortHash(row.output_digest_hex)}
                              </span>
                            </div>
                          ))}
                        </div>
                      )}
                    </CrudPager>
                  )}
                </div>
                {outputStreamDownloadTargets.length > 0 && (
                  <div className="outputDownloadRows">
                    {outputStreamDownloadTargets.map((target) => (
                      <div className="outputDownloadRow" key={target.clientId}>
                        <span className="historyPrimary">
                          <strong>{clientLabel(target.clientId)}</strong>
                          <small>retained stdout/stderr payload</small>
                        </span>
                        <span className="inlineActions">
                          {target.stdout && (
                            <button
                              className="secondaryAction compactAction"
                              disabled={
                                streamPendingKey === `${target.clientId}:stdout`
                              }
                              onClick={() =>
                                void downloadOutputStreamForClient(
                                  target.clientId,
                                  "stdout",
                                )
                              }
                              type="button"
                            >
                              <Download size={14} />
                              <span>
                                {streamPendingKey ===
                                `${target.clientId}:stdout`
                                  ? "Downloading"
                                  : "Download stdout"}
                              </span>
                            </button>
                          )}
                          {target.stderr && (
                            <button
                              className="secondaryAction compactAction"
                              disabled={
                                streamPendingKey === `${target.clientId}:stderr`
                              }
                              onClick={() =>
                                void downloadOutputStreamForClient(
                                  target.clientId,
                                  "stderr",
                                )
                              }
                              type="button"
                            >
                              <Download size={14} />
                              <span>
                                {streamPendingKey ===
                                `${target.clientId}:stderr`
                                  ? "Downloading"
                                  : "Download stderr"}
                              </span>
                            </button>
                          )}
                          {target.combined && (
                            <button
                              className="secondaryAction compactAction"
                              disabled={
                                streamPendingKey === `${target.clientId}:combined`
                              }
                              onClick={() =>
                                void downloadOutputStreamForClient(
                                  target.clientId,
                                  "combined",
                                )
                              }
                              type="button"
                            >
                              <Download size={14} />
                              <span>
                                {streamPendingKey ===
                                `${target.clientId}:combined`
                                  ? "Downloading"
                                  : "Download combined"}
                              </span>
                            </button>
                          )}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
                <div className="outputList">
                  {outputs.map((output) => (
                    <article
                      className="outputChunk"
                      key={`${output.client_id}:${output.seq}`}
                    >
                      <div className="outputMeta">
                        <span
                          className={`status ${output.stream === "stderr" ? "warn" : "info"}`}
                        >
                          {output.stream}
                        </span>
                        <strong>{clientLabel(output.client_id)}</strong>
                        <small>
                          #{output.seq}{" "}
                          {output.exit_code === null
                            ? ""
                            : `exit ${output.exit_code}`}
                          {(output.storage === "object_store" ||
                            output.storage === "artifact_deleted") &&
                          output.artifact_size_bytes != null
                            ? ` · ${formatBytes(output.artifact_size_bytes)}`
                            : ""}
                        </small>
                      </div>
                      {output.storage === "object_store" ? (
                        <div className="outputArtifact">
                          <pre>
                            {`artifact ${output.artifact_object_key ?? "retained externally"}\nsha256 ${output.artifact_sha256_hex ?? "-"}`}
                          </pre>
                        </div>
                      ) : output.storage === "artifact_deleted" ? (
                        <div className="outputArtifact deletedArtifact">
                          <pre>
                            {`artifact deleted\nsha256 ${output.artifact_sha256_hex ?? "-"}\nfull size ${
                              output.artifact_size_bytes != null
                                ? formatBytes(output.artifact_size_bytes)
                                : "-"
                            }\n\npreview only\n${decodeOutputPreview(output.data_base64)}`}
                          </pre>
                        </div>
                      ) : (
                        <pre>{decodeOutputPreview(output.data_base64)}</pre>
                      )}
                    </article>
                  ))}
                  {outputs.length === 0 && (
                    <div className="emptyState">
                      <TerminalSquare size={22} />
                      <strong>No output chunks</strong>
                      <span>
                        {outputError ??
                          "This job has no retained stdout, stderr, or status output."}
                      </span>
                    </div>
                  )}
                </div>
              </div>
            </div>
          )}
        </div>
      )}
      {jobSubpage === "approvals" && (
        <div className="jobConsoleStack">
          <div className="fleetPanel scheduleRunsPanel">
            <div className="sectionHeader compact">
              <div>
                <h2>Schedule runs</h2>
                <span>{`${scheduleRunJobs.length} worker-created due runs`}</span>
              </div>
              <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
                Refresh
              </button>
            </div>
            {scheduleRunJobs.length > 0 ? (
              <div className="table historyTable">
                <div className="historyRow scheduledRunGrid heading">
                  <span>Run</span>
                  <span>Status</span>
                  <span>Targets</span>
                  <span>Created</span>
                  <span>Actions</span>
                </div>
                {scheduleRunJobs.map((job) => (
                  <div className="historyRow scheduledRunGrid" key={job.id}>
                    <span className="historyPrimary">
                      <strong>{job.command_type.replace(/^scheduled_/, "")}</strong>
                      <small>{shortId(job.id)} · {shortHash(job.payload_hash)}</small>
                    </span>
                    <span className={`status ${jobStatusBadgeClass(job.status)}`}>{job.status}</span>
                    <span>{job.target_count}</span>
                    <span>{formatTime(job.created_at)}</span>
                    <span className="inlineActions">
                      <button className="secondaryAction compactAction" onClick={() => void openTargets(job.id)} type="button">
                        Open
                      </button>
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <div className="emptyState">
                <ShieldCheck size={22} />
                <strong>No schedule runs yet</strong>
                <span>Due schedule jobs are created and dispatched by worker automation.</span>
              </div>
            )}
          </div>
        </div>
      )}
    </section>
  );
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function matchesOutputPayloadStream(stream: string): boolean {
  return stream === "stdout" || stream === "stderr";
}

function hasCompleteRetainedOutputStream(
  outputs: JobOutputRecord[],
  stream: "stdout" | "stderr",
): boolean {
  const streamOutputs = outputs.filter((output) => output.stream === stream);
  return (
    streamOutputs.length > 0 &&
    streamOutputs.every((output) => output.storage !== "artifact_deleted")
  );
}

function safeDownloadName(value: string | null | undefined, fallback = "download.bin"): string {
  const cleaned = (value ?? "")
    .trim()
    .replace(/[\\/\u0000-\u001f\u007f]+/g, "_")
    .slice(0, 180);
  return cleaned || fallback;
}

function outputCompareBasisLabel(value: string): string {
  switch (value) {
    case "text":
      return "Text normalized";
    case "binary_metadata":
      return "Artifact metadata";
    default:
      return "Binary exact";
  }
}

function formatComparisonTime(value: string): string {
  if (/^\d+$/.test(value)) {
    return formatTime(new Date(Number(value) * 1000).toISOString());
  }
  return formatTime(value);
}

function formatBytes(value: number): string {
  if (value < 1024) {
    return `${value} B`;
  }
  if (value < 1024 * 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
}
