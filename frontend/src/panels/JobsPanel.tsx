import { lazy, Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { Check, Download, ExternalLink, Server, ShieldCheck, TerminalSquare, X } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { usePanelDisplaySettings } from "../panelDisplay";
import { type PrivilegeMaterial } from "../privilege";
import type {
  JobDispatchPreset,
} from "../jobDispatchPreset";
import type {
  AgentView,
  BulkResolveResponse,
  CommandTemplateRecord,
  CreateJobApprovalRequest,
  CreateJobRequest,
  CreateJobResponse,
  DecideJobApprovalRequest,
  JobApprovalDecisionResponse,
  JobApprovalRecord,
  DeleteCommandTemplateRequest,
  JobHistoryRecord,
  JobOutputCompareMode,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetSelection,
  UpsertCommandTemplateRequest,
  WsJobOutputEvent,
} from "../types";
import type {
  FileTransferSourceArtifactRecord,
} from "../typesFileTransfer";
import type {
  TerminalInputSubmitRequest,
  TerminalInputSubmitResponse,
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
import { parseLatestFileStatus } from "../fileBrowser";

const JobDispatchPanel = lazy(() =>
  import("./JobDispatchPanel").then((module) => ({
    default: module.JobDispatchPanel,
  })),
);
type JobOutputComparisonGroup = JobOutputComparisonRecord["groups"][number];
type JobOutputComparisonRow = JobOutputComparisonRecord["rows"][number];

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}

function scheduledRunCommandLabel(commandType: string): string {
  return displayToken(commandType.replace(/^scheduled_/, ""));
}

function formatScheduleRunDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toISOString().slice(0, 10);
}

function formatScheduleRunClock(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return `${date.toISOString().slice(11, 16)} UTC`;
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

export function JobsPanel({
  activeSubpage,
  agents,
  error,
  fileTransferSources,
  jobApprovals,
  jobs,
  commandTemplates,
  dispatchPreset,
  lastJobOutputEvent,
  loading,
  onApproveJobApproval,
  onCreateJob,
  onCreateJobApproval,
  onDownloadFileBundle,
  onDownloadOutputArchive,
  onDownloadTargetStatusArchive,
  onDownloadOutputChunk,
  onDownloadOutputStream,
  onDownloadFileForClient,
  onDownloadFileTransferSource,
  onDispatchPresetApplied,
  onLoadJob,
  onLoadOutputs,
  onLoadOutputComparison,
  onLoadTargets,
  onSubmitTerminalInput,
  onOpenPrivilegeUnlock,
  onOpenSchedules,
  onOpenVpsDetail,
  onOpenRemoteOperations,
  onRefresh,
  onRejectJobApproval,
  onResolveTargets,
  onSelectSubpage,
  onSelectedJobDetailsOpened,
  onDeleteCommandTemplate,
  onUpsertCommandTemplate,
  pendingSelectedJobId,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  error: string | null;
  fileTransferSources: FileTransferSourceArtifactRecord[];
  jobApprovals: JobApprovalRecord[];
  jobs: JobHistoryRecord[];
  commandTemplates: CommandTemplateRecord[];
  dispatchPreset?: JobDispatchPreset | null;
  lastJobOutputEvent: WsJobOutputEvent | null;
  loading: boolean;
  onApproveJobApproval: (
    approvalId: string,
    request: DecideJobApprovalRequest,
  ) => Promise<JobApprovalDecisionResponse>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateJobApproval: (
    request: CreateJobApprovalRequest,
  ) => Promise<JobApprovalRecord>;
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
  onDispatchPresetApplied?: () => void;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadOutputComparison: (
    jobId: string,
    mode: JobOutputCompareMode,
  ) => Promise<JobOutputComparisonRecord>;
  onSubmitTerminalInput: (
    clientId: string,
    sessionId: string,
    request: TerminalInputSubmitRequest,
  ) => Promise<TerminalInputSubmitResponse>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenPrivilegeUnlock: () => void;
  onOpenSchedules?: () => void;
  onOpenVpsDetail?: (clientId: string) => void;
  onOpenRemoteOperations?: (subpage: string) => void;
  onRefresh: () => void;
  onRejectJobApproval: (
    approvalId: string,
    request: DecideJobApprovalRequest,
  ) => Promise<JobApprovalDecisionResponse>;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
  onSelectedJobDetailsOpened?: (jobId: string) => void;
  onSelectSubpage?: (subpage: string) => void;
  onDeleteCommandTemplate: (
    templateId: string,
    request: DeleteCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  onUpsertCommandTemplate: (
    request: UpsertCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  pendingSelectedJobId?: string | null;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
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
  const [approvalActionPending, setApprovalActionPending] = useState(false);
  const [approvalActionError, setApprovalActionError] = useState<string | null>(
    null,
  );
  const jobSubpage = [
    "history",
    "dispatch",
    "approvals",
    "scheduled_runs",
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
  const pendingApprovalCount = jobApprovals.filter(
    (approval) => approval.status === "pending",
  ).length;
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

  function decideApproval(approval: JobApprovalRecord, decision: "approve" | "reject") {
    void runPanelAction(setApprovalActionPending, setApprovalActionError, async () => {
      const response =
        decision === "approve"
          ? await onApproveJobApproval(approval.id, {
              confirmed: true,
              reason: "Approved from Jobs / Approvals",
            })
          : await onRejectJobApproval(approval.id, {
              confirmed: true,
              reason: "Rejected from Jobs / Approvals",
            });
      if (response.job) {
        openSubmittedJobDetails(response.job.job_id);
      }
    });
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

  const approvalColumns = useMemo<ConsoleDataGridColumn<JobApprovalRecord>[]>(
    () => [
      {
        id: "command",
        header: "Command",
        size: 230,
        minSize: 180,
        sortValue: (approval) => approval.command_type,
        searchValue: (approval) =>
          `${approval.command_type} ${approval.job_id} ${approval.id}`,
        cell: (approval) => (
          <span className="historyPrimary">
            <strong title={approval.command_type}>
              {displayToken(approval.command_type)}
            </strong>
            <small>Job {shortId(approval.job_id)}</small>
          </span>
        ),
      },
      {
        id: "status",
        header: "Status",
        size: 125,
        minSize: 110,
        sortValue: (approval) => approval.status,
        searchValue: (approval) => approval.status,
        cell: (approval) => (
          <span className={`status ${approvalStatusBadgeClass(approval.status)}`}>
            {approval.status}
          </span>
        ),
      },
      {
        id: "scope",
        header: "Scope",
        size: 250,
        minSize: 190,
        sortValue: (approval) => approval.target_count,
        searchValue: (approval) =>
          `${approval.selector_expression} ${approval.target_client_ids.join(" ")}`,
        cell: (approval) => (
          <span className="historyPrimary">
            <strong>
              {approval.target_count} target{approval.target_count === 1 ? "" : "s"}
            </strong>
            <small title={approval.selector_expression}>
              {approval.selector_expression || "fixed target set"}
            </small>
          </span>
        ),
      },
      {
        id: "risk",
        header: "Risk",
        size: 160,
        minSize: 130,
        sortValue: (approval) => approval.risk,
        searchValue: (approval) =>
          `${approval.risk} ${approval.destructive ? "destructive" : ""} ${
            approval.force_unprivileged ? "force unprivileged" : ""
          }`,
        cell: (approval) => (
          <span className="jobStatusCell">
            <span
              className={`status ${
                approval.destructive ? "warn" : approval.privileged ? "info" : "neutral"
              }`}
            >
              {approval.risk}
            </span>
            {approval.force_unprivileged ? (
              <small>forced unprivileged</small>
            ) : null}
          </span>
        ),
      },
      {
        id: "requester",
        header: "Requester",
        size: 175,
        minSize: 145,
        sortValue: (approval) => approval.requester_username,
        searchValue: (approval) =>
          `${approval.requester_username} ${approval.requester_role}`,
        cell: (approval) => (
          <span className="historyPrimary">
            <strong>{approval.requester_username}</strong>
            <small>{approval.requester_role}</small>
          </span>
        ),
      },
      {
        id: "requested",
        header: "Requested",
        size: 195,
        minSize: 165,
        sortValue: (approval) => approval.requested_at,
        searchValue: (approval) => approval.requested_at,
        cell: (approval) => formatTime(approval.requested_at),
      },
      {
        id: "actions",
        header: "Actions",
        size: 96,
        minSize: 90,
        enableHiding: false,
        cell: (approval) => (
          <span className="inlineActions">
            <button
              aria-label="Approve job approval"
              className="secondaryAction compactAction"
              disabled={approval.status !== "pending" || approvalActionPending}
              onClick={(event) => {
                event.stopPropagation();
                decideApproval(approval, "approve");
              }}
              title={
                approval.status === "pending"
                  ? "Approve and dispatch the frozen job request"
                  : "Only pending approvals can be approved"
              }
              type="button"
            >
              <Check size={14} />
            </button>
            <button
              aria-label="Reject job approval"
              className="secondaryAction compactAction dangerAction"
              disabled={approval.status !== "pending" || approvalActionPending}
              onClick={(event) => {
                event.stopPropagation();
                decideApproval(approval, "reject");
              }}
              title={
                approval.status === "pending"
                  ? "Reject this reviewed job request"
                  : "Only pending approvals can be rejected"
              }
              type="button"
            >
              <X size={14} />
            </button>
          </span>
        ),
      },
    ],
    [approvalActionPending],
  );

  const targetColumns = useMemo<ConsoleDataGridColumn<JobTargetRecord>[]>(
    () => [
      {
        cell: (target) => (
          <span className="historyPrimary">
            <strong>{clientLabel(target.client_id)}</strong>
            <small>{shortId(target.job_id)}</small>
          </span>
        ),
        header: "Client",
        id: "client",
        searchValue: (target) => `${clientLabel(target.client_id)} ${target.client_id} ${target.job_id}`,
        sortValue: (target) => clientLabel(target.client_id),
      },
      {
        cell: (target) => (
          <span className={`status ${jobTargetStatusBadgeClass(target.status)}`}>
            {target.status}
          </span>
        ),
        header: "Status",
        id: "status",
        searchValue: (target) => target.status,
        sortValue: (target) => target.status,
      },
      {
        cell: (target) => <span title={target.message ?? undefined}>{target.message ?? "-"}</span>,
        header: "Reason",
        id: "reason",
        searchValue: (target) => target.message ?? "",
        sortValue: (target) => target.message ?? "",
      },
      {
        cell: (target) => target.exit_code ?? "-",
        header: "Exit",
        id: "exit",
        searchValue: (target) => target.exit_code ?? "",
        sortValue: (target) => target.exit_code ?? Number.MAX_SAFE_INTEGER,
      },
      {
        cell: (target) => (target.completed_at ? formatTime(target.completed_at) : "-"),
        header: "Completed",
        id: "completed",
        searchValue: (target) => target.completed_at ?? "",
        sortValue: (target) => target.completed_at ?? "",
      },
      {
        cell: (target) => (
          <span className="inlineActions">
            {onOpenVpsDetail ? (
              <button
                className="secondaryAction compactAction"
                onClick={(event) => {
                  event.stopPropagation();
                  onOpenVpsDetail(target.client_id);
                }}
                type="button"
              >
                <Server size={14} />
                <span>Open VPS detail</span>
              </button>
            ) : null}
            {fileDownloadStatusByClient.has(target.client_id) ? (
              <button
                className="secondaryAction compactAction"
                disabled={fileDownloadPendingClientId === target.client_id}
                onClick={(event) => {
                  event.stopPropagation();
                  void downloadFileForClient(target.client_id);
                }}
                type="button"
              >
                <Download size={14} />
                <span>
                  {fileDownloadPendingClientId === target.client_id
                    ? "Downloading"
                    : "Download file"}
                </span>
              </button>
            ) : onOpenVpsDetail ? null : (
              "-"
            )}
          </span>
        ),
        enableHiding: false,
        header: "Actions",
        id: "actions",
      },
    ],
    [agentNameById, fileDownloadPendingClientId, fileDownloadStatusByClient, onOpenVpsDetail],
  );
  const comparisonGroupColumns = useMemo<ConsoleDataGridColumn<JobOutputComparisonGroup>[]>(
    () => [
      {
        cell: (group) => (
          <span className="historyPrimary">
            <strong className={`status ${jobOutputComparisonStatusBadgeClass(group.status)}`}>
              {group.status}
            </strong>
            <small>exit {group.exit_code ?? "-"}</small>
          </span>
        ),
        header: "Outcome",
        id: "outcome",
        searchValue: (group) => `${group.status} ${group.exit_code ?? ""}`,
        sortValue: (group) => group.status,
      },
      {
        cell: (group) => (
          <span className="historyPrimary">
            <strong>{group.target_count} targets</strong>
            <small>{clientLabel(group.representative_client_id)}</small>
          </span>
        ),
        header: "Targets",
        id: "targets",
        searchValue: (group) => group.client_ids.map(clientLabel).join(" "),
        sortValue: (group) => group.target_count,
      },
      {
        cell: (group) => (
          <span className="historyPrimary">
            <strong>{outputCompareBasisLabel(group.output_compare_basis)}</strong>
            <small>
              {group.stream_count} chunks / {formatBytes(group.byte_count)}
            </small>
          </span>
        ),
        header: "Output",
        id: "output",
        searchValue: (group) => `${group.output_compare_basis} ${group.stream_count} ${group.byte_count} ${group.preview}`,
        sortValue: (group) => group.byte_count,
      },
      {
        cell: (group) => (
          <span className="monoValue" title={group.preview}>
            {shortHash(group.output_digest_hex)}
          </span>
        ),
        header: "Digest",
        id: "digest",
        searchValue: (group) => `${group.output_digest_hex} ${group.preview}`,
        sortValue: (group) => group.output_digest_hex,
      },
    ],
    [agentNameById],
  );
  const comparisonTargetColumns = useMemo<ConsoleDataGridColumn<JobOutputComparisonRow>[]>(
    () => [
      {
        cell: (row) => (
          <span className="historyPrimary">
            <strong>{clientLabel(row.client_id)}</strong>
            <small>
              {row.stream_count} chunks / {formatBytes(row.byte_count)}
            </small>
          </span>
        ),
        header: "Client",
        id: "client",
        searchValue: (row) => `${clientLabel(row.client_id)} ${row.client_id}`,
        sortValue: (row) => clientLabel(row.client_id),
      },
      {
        cell: (row) => (
          <span className={`status ${jobOutputComparisonStatusBadgeClass(row.status)}`}>
            {row.status} / {row.exit_code ?? "-"}
          </span>
        ),
        header: "Status",
        id: "status",
        searchValue: (row) => `${row.status} ${row.exit_code ?? ""}`,
        sortValue: (row) => row.status,
      },
      {
        cell: (row) => (
          <span className={row.matches_largest_group ? "status ok" : "status warn"}>
            {row.matches_largest_group ? "largest" : row.group_id}
          </span>
        ),
        header: "Group",
        id: "group",
        searchValue: (row) => row.group_id,
        sortValue: (row) => row.group_id,
      },
      {
        cell: (row) => (
          <span className="monoValue" title={row.preview}>
            {shortHash(row.output_digest_hex)}
          </span>
        ),
        header: "Digest",
        id: "digest",
        searchValue: (row) => `${row.output_digest_hex} ${row.preview}`,
        sortValue: (row) => row.output_digest_hex,
      },
    ],
    [agentNameById],
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

  return (
    <section className="workspace singleColumn">
      <Suspense
        fallback={
          <div className="emptyState compactEmpty" role="status" aria-live="polite">
            Loading {displayToken(jobSubpage)} workspace
          </div>
        }
      >
      {jobSubpage === "dispatch" && (
        <JobDispatchPanel
          agents={agents}
          fileTransferSources={fileTransferSources}
          commandTemplates={commandTemplates}
          dispatchPreset={dispatchPreset}
          onDispatchPresetApplied={onDispatchPresetApplied}
          onCreateJob={onCreateJob}
          onDownloadFileTransferSource={onDownloadFileTransferSource}
          onDownloadOutputChunk={onDownloadOutputChunk}
          onOpenRemoteTerminal={() => onOpenRemoteOperations?.("terminal")}
          onLoadJob={onLoadJob}
          onLoadOutputs={onLoadOutputs}
          onLoadTargets={onLoadTargets}
          onSubmitTerminalInput={onSubmitTerminalInput}
          onOpenJobDetails={openSubmittedJobDetails}
          onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
          onResolveTargets={onResolveTargets}
          onDeleteCommandTemplate={onDeleteCommandTemplate}
          onUpsertCommandTemplate={onUpsertCommandTemplate}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
        />
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
            <div
              className="jobHistoryWorkflowLinks"
              aria-label="Related Remote Operations pages"
            >
              <span className="jobHistoryWorkflowIntro">
                <strong>Related workflow owners</strong>
                <small>
                  Use Jobs for execution evidence. Open operational workflows in
                  Remote Operations.
                </small>
              </span>
              <span className="jobHistoryWorkflowActions">
                {[
                  { label: "Terminal", subpage: "terminal" },
                  { label: "Files", subpage: "files" },
                  { label: "Transfers", subpage: "transfers" },
                  { label: "Processes", subpage: "processes" },
                  { label: "Bulk files", subpage: "bulk_files" },
                ].map((link) => (
                  <button
                    className="secondaryAction compactAction"
                    disabled={!onOpenRemoteOperations}
                    key={link.subpage}
                    onClick={() => onOpenRemoteOperations?.(link.subpage)}
                    type="button"
                  >
                    <ExternalLink size={14} />
                    <span>{link.label}</span>
                  </button>
                ))}
              </span>
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
              <ConsoleDataGrid
                columns={targetColumns}
                defaultPageSize={10}
                expandOnRowClick
                getRowId={(target) => `${target.job_id}:${target.client_id}`}
                itemLabel="targets"
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
                renderExpandedRow={(target) => (
                  <div className="consoleInlineDetailGrid">
                    <span>Client</span>
                    <strong>{clientLabel(target.client_id)}</strong>
                    <span>Client ID</span>
                    <strong>{target.client_id}</strong>
                    <span>Job ID</span>
                    <strong>{target.job_id}</strong>
                    <span>Status</span>
                    <strong>{target.status}</strong>
                    <span>Reason</span>
                    <strong>{target.message ?? "None"}</strong>
                    <span>Completed</span>
                    <strong>{target.completed_at ? formatTime(target.completed_at) : "Not completed"}</strong>
                  </div>
                )}
                rows={targets}
                searchPlaceholder="Search targets"
                selectable={false}
                storageKey="vpsman.jobs.history.targets"
                title="Target result records"
              />
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
                    <ConsoleDataGrid
                      columns={comparisonGroupColumns}
                      defaultPageSize={6}
                      expandOnRowClick
                      getRowId={(group) => group.group_id}
                      itemLabel="groups"
                      onOpenRow={(group) => setSelectedComparisonGroupId(group.group_id)}
                      renderExpandedRow={(group) => (
                        <div className="consoleInlineDetailGrid">
                          <span>Group</span>
                          <strong>{group.group_id}</strong>
                          <span>Status</span>
                          <strong>{group.status}</strong>
                          <span>Targets</span>
                          <strong>{group.client_ids.map(clientLabel).join(", ")}</strong>
                          <span>Digest</span>
                          <strong>{group.output_digest_hex}</strong>
                          <span>Preview</span>
                          <strong>{group.preview || "No preview"}</strong>
                        </div>
                      )}
                      rows={outputComparison.groups}
                      searchPlaceholder="Search grouped outcomes"
                      selectable={false}
                      storageKey="vpsman.jobs.history.comparisonGroups"
                      title="Grouped outcomes"
                    />
                  )}
                  {outputComparison && displayedComparisonRows.length > 0 && (
                    <ConsoleDataGrid
                      columns={comparisonTargetColumns}
                      defaultPageSize={8}
                      expandOnRowClick
                      getRowId={(row) => row.client_id}
                      itemLabel="targets"
                      title={
                        selectedComparisonGroupId
                          ? `Targets in ${selectedComparisonGroupId}`
                          : "Target result details"
                      }
                      renderExpandedRow={(row) => (
                        <div className="consoleInlineDetailGrid">
                          <span>Client</span>
                          <strong>{clientLabel(row.client_id)}</strong>
                          <span>Group</span>
                          <strong>{row.group_id}</strong>
                          <span>Digest</span>
                          <strong>{row.output_digest_hex}</strong>
                          <span>Output</span>
                          <strong>{row.stream_count} chunks / {formatBytes(row.byte_count)}</strong>
                          <span>Preview</span>
                          <strong>{row.preview || "No preview"}</strong>
                        </div>
                      )}
                      rows={displayedComparisonRows}
                      searchPlaceholder="Search target results"
                      selectable={false}
                      storageKey="vpsman.jobs.history.comparisonTargets"
                    />
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
          <div className="fleetPanel">
            <div className="sectionHeader compact">
              <div>
                <h2>Approvals</h2>
                <span>
                  {pendingApprovalCount} pending · {jobApprovals.length} reviewed requests
                </span>
              </div>
              <div className="inlineActions">
                <button
                  className="secondaryAction compactAction"
                  disabled={loading || approvalActionPending}
                  onClick={onRefresh}
                  type="button"
                >
                  Refresh
                </button>
              </div>
            </div>
            {approvalActionError && (
              <div className="panelError" role="alert">
                {approvalActionError}
              </div>
            )}
            <ConsoleDataGrid
              columns={approvalColumns}
              defaultColumnVisibility={{ requested: true }}
              defaultPageSize={10}
              empty={
                <div className="emptyState">
                  <ShieldCheck size={22} />
                  <strong>No reviewed work is waiting</strong>
                  <span>
                    Approval requests that have passed privilege review appear here for final dispatch or rejection.
                  </span>
                </div>
              }
              expandOnRowClick
              getRowId={(approval) => approval.id}
              itemLabel="approvals"
              renderExpandedRow={(approval) => (
                <div className="consoleInlineDetailGrid">
                  <span>Approval</span>
                  <strong>{approval.id}</strong>
                  <span>Job</span>
                  <strong>{approval.job_id}</strong>
                  <span>Targets</span>
                  <strong>{approval.target_client_ids.map(clientLabel).join(", ")}</strong>
                  <span>Payload</span>
                  <strong>{approval.payload_hash}</strong>
                  <span>Fingerprint</span>
                  <strong>{approval.request_fingerprint}</strong>
                  <span>Timeout</span>
                  <strong>{approval.max_timeout_secs}s</strong>
                  <span>Request reason</span>
                  <strong>{approval.request_reason ?? "No request reason"}</strong>
                  <span>Decision</span>
                  <strong>
                    {approval.decision_username
                      ? `${approval.decision_username} · ${approval.decision_reason ?? "No decision note"}`
                      : "Pending"}
                  </strong>
                </div>
              )}
              rowActions={[
                {
                  label: "Approve",
                  icon: <Check size={14} />,
                  disabled: (rows) =>
                    rows.length !== 1 ||
                    rows[0].status !== "pending" ||
                    approvalActionPending,
                  onSelect: (rows) => decideApproval(rows[0], "approve"),
                },
                {
                  label: "Reject",
                  icon: <X size={14} />,
                  tone: "danger",
                  disabled: (rows) =>
                    rows.length !== 1 ||
                    rows[0].status !== "pending" ||
                    approvalActionPending,
                  onSelect: (rows) => decideApproval(rows[0], "reject"),
                },
              ]}
              rows={jobApprovals}
              searchPlaceholder="Search approvals"
              singleExpandedRow
              storageKey="vpsman.jobs.approvals"
              title="Job approval queue"
            />
          </div>
        </div>
      )}
      {jobSubpage === "scheduled_runs" && (
        <div className="jobConsoleStack">
          <div className="fleetPanel scheduleRunsPanel">
            <div className="sectionHeader compact">
              <div>
                <h2>Scheduled runs</h2>
                <span>{`${scheduleRunJobs.length} worker-created due runs`}</span>
              </div>
              <div className="inlineActions">
                <button className="secondaryAction compactAction" onClick={onOpenSchedules} type="button">
                  Open schedule registry
                </button>
                <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
                  Refresh
                </button>
              </div>
            </div>
            {scheduleRunJobs.length > 0 ? (
              <div className="table historyTable">
                <div className="historyRow scheduledRunGrid heading">
                  <span>Schedule job</span>
                  <span>Lifecycle</span>
                  <span>Result</span>
                  <span>Worker evidence</span>
                  <span>Actions</span>
                </div>
                {scheduleRunJobs.map((job) => (
                  <div className="historyRow scheduledRunGrid" key={job.id}>
                    <span className="historyPrimary">
                      <strong>{scheduledRunCommandLabel(job.command_type)}</strong>
                      <small>Job {shortId(job.id)} · payload {shortHash(job.payload_hash)}</small>
                      <small>Schedule link not exposed</small>
                    </span>
                    <span className="scheduleRunsCell">
                      <strong title={formatTime(job.created_at)}>Dispatched {formatScheduleRunClock(job.created_at)}</strong>
                      <small>{formatScheduleRunDate(job.created_at)} · due not exposed</small>
                      <small title={job.completed_at ? formatTime(job.completed_at) : undefined}>
                        {job.completed_at ? `Completed ${formatScheduleRunClock(job.completed_at)}` : `Timeout ${job.max_timeout_secs}s`}
                      </small>
                    </span>
                    <span className="scheduleRunsCell">
                      <span className={`status ${jobStatusBadgeClass(job.status)}`}>{job.status}</span>
                      <small>{job.target_count} target{job.target_count === 1 ? "" : "s"}</small>
                      <small>Skipped count not exposed</small>
                    </span>
                    <span className="scheduleRunsCell">
                      <strong>{job.actor_id ? "Actor-triggered" : "Worker automation"}</strong>
                      <small>{job.privileged ? "Privilege assertion required" : "Default job authority"}</small>
                      <small>Retry/worker health not exposed</small>
                    </span>
                    <span className="inlineActions">
                      <button
                        className="secondaryAction compactAction"
                        onClick={() => void openTargets(job.id)}
                        type="button"
                      >
                        Open targets
                      </button>
                      <button
                        className="secondaryAction compactAction"
                        disabled
                        title="Retry requires schedule-run retry support with original due time, schedule id, and privilege revalidation."
                        type="button"
                      >
                        Retry
                      </button>
                    </span>
                  </div>
                ))}
              </div>
            ) : (
              <div className="emptyState">
                <ShieldCheck size={22} />
                <strong>No schedule runs yet</strong>
                <span>Due schedule jobs are created and dispatched by worker automation. Create or inspect schedules in the registry.</span>
                <div className="emptyStateActions">
                  <button className="primaryAction compactAction" onClick={onOpenSchedules} type="button">
                    Open schedule registry
                  </button>
                  <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
                    Check worker
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
      </Suspense>
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

function approvalStatusBadgeClass(status: JobApprovalRecord["status"]): string {
  switch (status) {
    case "approved":
      return "ok";
    case "rejected":
      return "neutral";
    default:
      return "warn";
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
