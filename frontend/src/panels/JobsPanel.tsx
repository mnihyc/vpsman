import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  Download,
  ExternalLink,
  Server,
  ShieldCheck,
  TerminalSquare,
  X,
} from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import {
  FLEET_DETAIL_LIMIT,
  formatLowerBoundCount,
  HISTORY_DETAIL_LIMIT,
} from "../constants";
import { ActionFeedback } from "../components/ActionFeedback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { usePanelDisplaySettings } from "../panelDisplay";
import { type PrivilegeMaterial } from "../privilege";
import type { JobDispatchPreset } from "../jobDispatchPreset";
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
  ScheduleRecord,
  UpsertCommandTemplateRequest,
  WsJobOutputEvent,
} from "../types";
import type { FileTransferSourceArtifactRecord } from "../typesFileTransfer";
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
  formatCompactTime,
  formatFullTime,
  formatTime,
  runPanelAction,
  shortHash,
  shortId,
} from "../utils";
import { parseLatestFileStatus } from "../fileBrowser";
import { retryableLazy } from "../lazyImport";
import { scrollIntoViewWithMotion } from "../motion";

const JobDispatchPanel = retryableLazy(() =>
  import("./JobDispatchPanel").then((module) => ({
    default: module.JobDispatchPanel,
  })),
);
type JobOutputComparisonGroup = JobOutputComparisonRecord["groups"][number];
type JobOutputComparisonRow = JobOutputComparisonRecord["rows"][number];
type ApprovalDecision = "approve" | "reject";
const JOB_DETAIL_SUBPAGE_PREFIX = "history:job:";

function jobDetailSubpage(jobId: string): string {
  return `${JOB_DETAIL_SUBPAGE_PREFIX}${jobId}`;
}

function jobDetailId(subpage: string): string | null {
  if (!subpage.startsWith(JOB_DETAIL_SUBPAGE_PREFIX)) {
    return null;
  }
  return subpage.slice(JOB_DETAIL_SUBPAGE_PREFIX.length).trim() || null;
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}

function displayCommandType(value: string): string {
  switch (value) {
    case "shell_argv":
      return "Argv command";
    case "scheduled_shell_argv":
      return "Scheduled shell command";
    case "shell_pty":
      return "Terminal session";
    case "terminal_input":
      return "Terminal input";
    default:
      return displayToken(value);
  }
}

function durationSortValue(job: JobHistoryRecord): number {
  const started = Date.parse(job.created_at);
  const completed = job.completed_at ? Date.parse(job.completed_at) : NaN;
  if (!Number.isFinite(started) || !Number.isFinite(completed)) {
    return -1;
  }
  return Math.max(0, completed - started);
}

function formatJobDuration(job: JobHistoryRecord): string {
  if (!job.completed_at) {
    return job.status === "queued" ? "Not started" : "Not completed";
  }
  const durationMs = durationSortValue(job);
  if (durationMs < 0) {
    return "Unknown";
  }
  const totalSeconds = Math.round(durationMs / 1000);
  if (totalSeconds < 60) {
    return `${totalSeconds}s`;
  }
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (minutes < 60) {
    return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`;
  }
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  if (hours < 24) {
    return remainingMinutes > 0
      ? `${hours}h ${remainingMinutes}m`
      : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const remainingHours = hours % 24;
  return remainingHours > 0 ? `${days}d ${remainingHours}h` : `${days}d`;
}

function jobStartedByLabel(job: JobHistoryRecord): string {
  return job.actor_id
    ? `Operator ${shortId(job.actor_id)}`
    : "Worker automation";
}

function scheduledRunCommandLabel(commandType: string): string {
  return displayCommandType(commandType);
}

function scheduledRunScheduleLabel(
  job: JobHistoryRecord,
  schedule?: ScheduleRecord,
): string {
  if (schedule) {
    return schedule.name;
  }
  return job.source_schedule_id
    ? `Schedule ${shortId(job.source_schedule_id)}`
    : "Scheduled run";
}

function scheduledRunCadenceLabel(schedule: ScheduleRecord): string {
  return schedule.cadence_error
    ? "Invalid cadence"
    : describeCronExpression(schedule.cron_expr);
}

function scheduledRunNextRunLabel(schedule: ScheduleRecord): string {
  if (schedule.cadence_error) {
    return "Unavailable — invalid cadence";
  }
  return schedule.next_run_at
    ? formatTime(schedule.next_run_at)
    : "Not reported";
}

function describeCronExpression(expr: string): string {
  const fields = expr.trim().split(/\s+/);
  if (fields.length !== 5) {
    return "Invalid schedule";
  }
  const [minute, hour, dom, month, dow] = fields;
  if (
    minute.startsWith("*/") &&
    hour === "*" &&
    dom === "*" &&
    month === "*" &&
    dow === "*"
  ) {
    const interval = Number(minute.slice(2));
    return Number.isInteger(interval) && interval > 0
      ? `Every ${interval} minutes`
      : "Custom cron schedule";
  }
  if (hour === "*" && dom === "*" && month === "*" && dow === "*") {
    return `Hourly at minute ${minute}`;
  }
  if (dom === "*" && month === "*" && dow === "*") {
    return `Daily at ${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")} UTC`;
  }
  if (dom === "*" && month === "*" && dow !== "*") {
    return `Weekly at ${String(hour).padStart(2, "0")}:${String(minute).padStart(2, "0")} UTC`;
  }
  if (month === "*" && dow === "*") {
    return `Monthly on day ${dom}`;
  }
  return "Custom cron schedule";
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
  fileTransferSourcesTruncated,
  jobApprovals,
  jobs,
  schedules,
  commandTemplates,
  commandTemplatesTruncated,
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
  onOpenRollout,
  onOpenSchedules,
  onOpenVpsDetail,
  onOpenRemoteOperations,
  onRefresh,
  onRejectJobApproval,
  onResolveTargets,
  onSelectSubpage,
  onDeleteCommandTemplate,
  onUpsertCommandTemplate,
  privilegeMaterial,
  setPrivilegeMaterial,
}: {
  activeSubpage: string;
  agents: AgentView[];
  error: string | null;
  fileTransferSources: FileTransferSourceArtifactRecord[];
  fileTransferSourcesTruncated: boolean;
  jobApprovals: JobApprovalRecord[];
  jobs: JobHistoryRecord[];
  schedules: ScheduleRecord[];
  commandTemplates: CommandTemplateRecord[];
  commandTemplatesTruncated: boolean;
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
  onDownloadOutputArchive: (
    jobId: string,
    clientIds: string[],
  ) => Promise<Blob>;
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
  onOpenRollout?: (jobId: string) => void;
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
  onSelectSubpage?: (subpage: string) => void;
  onDeleteCommandTemplate: (
    templateId: string,
    request: DeleteCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  onUpsertCommandTemplate: (
    request: UpsertCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => Promise<void>;
}) {
  const { preferences, vpsNameDisplayMode } = usePanelDisplaySettings();
  const targetDetailRef = useRef<HTMLDivElement | null>(null);
  const targetLoadGenerationRef = useRef(0);
  const comparisonLoadGenerationRef = useRef(0);
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
  const [approvalReview, setApprovalReview] =
    useState<JobApprovalRecord | null>(null);
  const [approvalDecision, setApprovalDecision] =
    useState<ApprovalDecision>("approve");
  const [approvalDecisionReason, setApprovalDecisionReason] = useState("");
  const jobSubpage = [
    "history",
    "dispatch",
    "approvals",
    "scheduled_runs",
  ].includes(activeSubpage)
    ? activeSubpage
    : "history";
  const routeSelectedJobId = jobDetailId(activeSubpage);
  const jobHistoryFeedbackMessage =
    error ?? (loading ? "Refreshing command records" : null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [streamPendingKey, setStreamPendingKey] = useState<string | null>(null);
  const [fileDownloadPendingClientId, setFileDownloadPendingClientId] =
    useState<string | null>(null);
  const [archivePendingKey, setArchivePendingKey] = useState<
    "files" | "outputs" | "status" | null
  >(null);
  const scheduleRunJobs = jobs.filter((job) =>
    job.command_type.startsWith("scheduled_"),
  );
  const historicalJobsBanner = jobHistoryFreshnessBanner(jobs);
  const scheduleById = useMemo(
    () => new Map(schedules.map((schedule) => [schedule.id, schedule])),
    [schedules],
  );
  const pendingApprovalCount = jobApprovals.filter(
    (approval) => approval.status === "pending",
  ).length;
  const jobHistoryTruncated = jobs.length >= HISTORY_DETAIL_LIMIT;
  const approvalsTruncated = jobApprovals.length >= FLEET_DETAIL_LIMIT;
  const approvalActionFeedbackMessage =
    approvalActionError ??
    (approvalActionPending ? "Recording approval decision" : null);
  const approvalActionFeedbackTone = approvalActionError
    ? "danger"
    : "progress";
  const targetDetailFeedbackMessage = targetError;
  const outputDetailFeedbackMessage = outputError ?? downloadError;
  const comparisonFeedbackMessage = comparisonError;
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
    const statusByClient = new Map<
      string,
      ReturnType<typeof parseLatestFileStatus>
    >();
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
  const outputStreamDownloadTargets = useMemo<
    OutputStreamDownloadTarget[]
  >(() => {
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
      const generation = ++targetLoadGenerationRef.current;
      const comparisonGeneration = ++comparisonLoadGenerationRef.current;
      setSelectedJobId(jobId);
      setTargets([]);
      setOutputs([]);
      setOutputComparison(null);
      setTargetsLoading(true);
      setOutputsLoading(true);
      setComparisonLoading(true);
      setTargetError(null);
      setOutputError(null);
      setComparisonError(null);
      setDownloadError(null);
      setSelectedComparisonGroupId(null);
      const [targetResult, outputResult, comparisonResult] =
        await Promise.allSettled([
          onLoadTargets(jobId),
          onLoadOutputs(jobId),
          onLoadOutputComparison(jobId, comparisonMode),
        ]);
      if (generation !== targetLoadGenerationRef.current) {
        return;
      }
      if (targetResult.status === "fulfilled") {
        setTargets(targetResult.value);
      } else {
        setTargets([]);
        setTargetError(
          errorMessage(targetResult.reason, "Job target history unavailable"),
        );
      }
      if (outputResult.status === "fulfilled") {
        setOutputs(outputResult.value);
      } else {
        setOutputs([]);
        setOutputError(
          errorMessage(outputResult.reason, "Job output unavailable"),
        );
      }
      if (comparisonGeneration === comparisonLoadGenerationRef.current) {
        if (comparisonResult.status === "fulfilled") {
          setOutputComparison(comparisonResult.value);
        } else {
          setOutputComparison(null);
          setComparisonError(
            errorMessage(
              comparisonResult.reason,
              "Execution summary unavailable",
            ),
          );
        }
        setComparisonLoading(false);
      }
      setTargetsLoading(false);
      setOutputsLoading(false);
    },
    [comparisonMode, onLoadOutputComparison, onLoadOutputs, onLoadTargets],
  );

  const clearTargetDetails = useCallback(() => {
    targetLoadGenerationRef.current += 1;
    comparisonLoadGenerationRef.current += 1;
    setSelectedJobId(null);
    setTargets([]);
    setOutputs([]);
    setOutputComparison(null);
    setSelectedComparisonGroupId(null);
    setTargetError(null);
    setOutputError(null);
    setComparisonError(null);
    setTargetsLoading(false);
    setOutputsLoading(false);
    setComparisonLoading(false);
  }, []);

  const closeTargetDetails = useCallback(() => {
    clearTargetDetails();
    onSelectSubpage?.("history");
  }, [clearTargetDetails, onSelectSubpage]);

  const openSubmittedJobDetails = useCallback(
    (jobId: string) => {
      if (onSelectSubpage) {
        onSelectSubpage(jobDetailSubpage(jobId));
        return;
      }
      void openTargets(jobId);
    },
    [onSelectSubpage, openTargets],
  );

  function openApprovalReview(approval: JobApprovalRecord) {
    setApprovalReview(approval);
    setApprovalDecision("approve");
    setApprovalDecisionReason("");
    setApprovalActionError(null);
  }

  function closeApprovalReview() {
    if (approvalActionPending) {
      return;
    }
    setApprovalReview(null);
    setApprovalDecision("approve");
    setApprovalDecisionReason("");
  }

  function decideApproval(
    approval: JobApprovalRecord,
    decision: ApprovalDecision,
    reason: string | null,
  ) {
    void runPanelAction(
      setApprovalActionPending,
      setApprovalActionError,
      async () => {
        const response =
          decision === "approve"
            ? await onApproveJobApproval(approval.id, {
                confirmed: true,
                reason,
              })
            : await onRejectJobApproval(approval.id, {
                confirmed: true,
                reason,
              });
        setApprovalReview(null);
        setApprovalDecision("approve");
        setApprovalDecisionReason("");
        if (response.job) {
          openSubmittedJobDetails(response.job.job_id);
        }
      },
    );
  }

  function submitApprovalDecision() {
    if (!approvalReview) {
      return;
    }
    const reason = approvalDecisionReason.trim();
    if (approvalDecision === "reject" && !reason) {
      setApprovalActionError("Rejection reason is required.");
      return;
    }
    decideApproval(approvalReview, approvalDecision, reason || null);
  }

  useEffect(() => {
    if (routeSelectedJobId) {
      if (routeSelectedJobId !== selectedJobId) {
        void openTargets(routeSelectedJobId);
      }
      return;
    }
    if (selectedJobId) {
      clearTargetDetails();
    }
  }, [
    clearTargetDetails,
    openTargets,
    routeSelectedJobId,
    selectedJobId,
  ]);

  useEffect(() => {
    if (!selectedJobId) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      const detail = targetDetailRef.current;
      if (!detail) {
        return;
      }
      scrollIntoViewWithMotion(detail, { block: "start" });
      detail.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [selectedJobId]);

  const jobColumns = useMemo<ConsoleDataGridColumn<JobHistoryRecord>[]>(
    () => [
      {
        id: "operation",
        header: "Operation",
        size: 250,
        minSize: 190,
        sortValue: (job) => job.command_type,
        searchValue: (job) => `${job.command_type} ${job.id}`,
        cell: (job) => (
          <span className="historyPrimary">
            <strong title={job.command_type}>
              {displayCommandType(job.command_type)}
            </strong>
            <small title={job.id}>{shortId(job.id)}</small>
          </span>
        ),
      },
      {
        id: "targets",
        header: "Targets",
        size: 130,
        minSize: 112,
        sortValue: (job) => job.target_count,
        searchValue: (job) => job.target_count,
        cell: (job) => (
          <button
            className="linkButton"
            onClick={(event) => {
              event.stopPropagation();
              openSubmittedJobDetails(job.id);
            }}
            type="button"
          >
            {job.target_count} target{job.target_count === 1 ? "" : "s"}
          </button>
        ),
      },
      {
        id: "result",
        header: "Result",
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
        id: "duration",
        header: "Duration",
        size: 120,
        minSize: 110,
        sortValue: (job) => durationSortValue(job),
        searchValue: (job) => formatJobDuration(job),
        cell: (job) => formatJobDuration(job),
      },
      {
        id: "startedBy",
        header: "Started by",
        size: 170,
        minSize: 145,
        sortValue: (job) => job.actor_id ?? "worker",
        searchValue: (job) =>
          `${jobStartedByLabel(job)} ${job.actor_id ?? ""} ${
            job.privileged ? "privileged" : "unprivileged"
          }`,
        cell: (job) => (
          <span className="historyPrimary">
            <strong title={job.actor_id ?? undefined}>
              {jobStartedByLabel(job)}
            </strong>
            <small>{job.privileged ? "privileged" : "unprivileged"}</small>
          </span>
        ),
      },
      {
        id: "age",
        header: "Age",
        size: 140,
        minSize: 120,
        sortValue: (job) => job.created_at,
        searchValue: (job) => job.created_at,
        cell: (job) => (
          <time
            dateTime={job.created_at}
            title={formatFullTime(job.created_at)}
          >
            {formatCompactTime(job.created_at)}
          </time>
        ),
      },
    ],
    [openSubmittedJobDetails],
  );

  const scheduledRunColumns = useMemo<
    ConsoleDataGridColumn<JobHistoryRecord>[]
  >(
    () => [
      {
        id: "schedule",
        header: "Schedule",
        size: 220,
        minSize: 180,
        sortValue: (job) => job.source_schedule_id ?? job.id,
        searchValue: (job) =>
          `${job.source_schedule_id ?? ""} ${job.id} ${job.command_type} ${
            job.source_schedule_id
              ? (scheduleById.get(job.source_schedule_id)?.name ?? "")
              : ""
          }`,
        cell: (job) => {
          const schedule = job.source_schedule_id
            ? scheduleById.get(job.source_schedule_id)
            : undefined;
          return (
            <span className="historyPrimary">
              <strong title={job.source_schedule_id ?? undefined}>
                {scheduledRunScheduleLabel(job, schedule)}
              </strong>
              <small title={schedule?.cadence_error ?? undefined}>
                {schedule
                  ? scheduledRunCadenceLabel(schedule)
                  : job.source_schedule_id
                    ? "Saved schedule source"
                    : "Worker-created run"}
              </small>
            </span>
          );
        },
      },
      {
        id: "operation",
        header: "Operation",
        size: 220,
        minSize: 180,
        sortValue: (job) => job.command_type,
        searchValue: (job) => job.command_type,
        cell: (job) => (
          <span className="historyPrimary">
            <strong>{scheduledRunCommandLabel(job.command_type)}</strong>
            <small>{job.privileged ? "privileged" : "unprivileged"}</small>
          </span>
        ),
      },
      {
        id: "targets",
        header: "Targets",
        size: 130,
        minSize: 112,
        sortValue: (job) => job.target_count,
        searchValue: (job) => job.target_count,
        cell: (job) => (
          <button
            className="linkButton"
            onClick={(event) => {
              event.stopPropagation();
              openSubmittedJobDetails(job.id);
            }}
            type="button"
          >
            {job.target_count} target{job.target_count === 1 ? "" : "s"}
          </button>
        ),
      },
      {
        id: "started",
        header: "Started",
        size: 150,
        minSize: 130,
        sortValue: (job) => job.created_at,
        searchValue: (job) => job.created_at,
        cell: (job) => (
          <time
            dateTime={job.created_at}
            title={formatFullTime(job.created_at)}
          >
            {formatCompactTime(job.created_at)}
          </time>
        ),
      },
      {
        id: "result",
        header: "Result",
        size: 150,
        minSize: 130,
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
        id: "duration",
        header: "Duration",
        size: 120,
        minSize: 110,
        sortValue: (job) => durationSortValue(job),
        searchValue: (job) => formatJobDuration(job),
        cell: (job) => formatJobDuration(job),
      },
    ],
    [openSubmittedJobDetails, scheduleById],
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
              {displayCommandType(approval.command_type)}
            </strong>
            <small title={approval.job_id}>
              Job {shortId(approval.job_id)}
            </small>
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
          <span
            className={`status ${approvalStatusBadgeClass(approval.status)}`}
          >
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
              {approval.target_count} target
              {approval.target_count === 1 ? "" : "s"}
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
                approval.destructive
                  ? "warn"
                  : approval.privileged
                    ? "info"
                    : "neutral"
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
    ],
    [],
  );

  const targetColumns = useMemo<ConsoleDataGridColumn<JobTargetRecord>[]>(
    () => [
      {
        cell: (target) => (
          <span className="historyPrimary">
            <strong>{clientLabel(target.client_id)}</strong>
            <small title={target.job_id}>{shortId(target.job_id)}</small>
          </span>
        ),
        header: "Client",
        id: "client",
        searchValue: (target) =>
          `${clientLabel(target.client_id)} ${target.client_id} ${target.job_id}`,
        sortValue: (target) => clientLabel(target.client_id),
      },
      {
        cell: (target) => (
          <span
            className={`status ${jobTargetStatusBadgeClass(target.status)}`}
          >
            {target.status}
          </span>
        ),
        header: "Status",
        id: "status",
        searchValue: (target) => target.status,
        sortValue: (target) => target.status,
      },
      {
        cell: (target) => {
          const reason = jobTargetReason(target);
          const rawReason = target.message?.trim();
          return (
            <span
              title={
                rawReason && rawReason !== reason
                  ? `${reason} Raw agent reason: ${rawReason}`
                  : reason
              }
            >
              {reason}
            </span>
          );
        },
        header: "Reason",
        id: "reason",
        searchValue: (target) =>
          `${jobTargetReason(target)} ${target.message ?? ""}`,
        sortValue: jobTargetReason,
      },
      {
        cell: (target) => target.exit_code ?? "-",
        header: "Exit",
        id: "exit",
        searchValue: (target) => target.exit_code ?? "",
        sortValue: (target) => target.exit_code ?? Number.MAX_SAFE_INTEGER,
      },
      {
        cell: (target) =>
          target.completed_at ? formatTime(target.completed_at) : "-",
        header: "Completed",
        id: "completed",
        searchValue: (target) => target.completed_at ?? "",
        sortValue: (target) => target.completed_at ?? "",
      },
    ],
    [agentNameById],
  );
  const comparisonGroupColumns = useMemo<
    ConsoleDataGridColumn<JobOutputComparisonGroup>[]
  >(
    () => [
      {
        cell: (group) => (
          <span className="historyPrimary">
            <strong
              className={`status ${jobOutputComparisonStatusBadgeClass(group.status)}`}
            >
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
            <strong>
              {group.target_count} target{group.target_count === 1 ? "" : "s"}
            </strong>
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
            <strong>
              {outputCompareBasisLabel(group.output_compare_basis)}
            </strong>
            <small>
              {group.stream_count} chunks / {formatBytes(group.byte_count)}
            </small>
          </span>
        ),
        header: "Output",
        id: "output",
        searchValue: (group) =>
          `${group.output_compare_basis} ${group.stream_count} ${group.byte_count} ${group.preview}`,
        sortValue: (group) => group.byte_count,
      },
      {
        cell: (group) => (
          <span className="monoValue" title={group.output_digest_hex}>
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
  const comparisonTargetColumns = useMemo<
    ConsoleDataGridColumn<JobOutputComparisonRow>[]
  >(
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
          <span
            className={`status ${jobOutputComparisonStatusBadgeClass(row.status)}`}
          >
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
          <span
            className={row.matches_largest_group ? "status ok" : "status warn"}
          >
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
          <span className="monoValue" title={row.output_digest_hex}>
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
    const generation = ++comparisonLoadGenerationRef.current;
    setComparisonLoading(true);
    setComparisonError(null);
    try {
      const comparison = await onLoadOutputComparison(jobId, mode);
      if (generation !== comparisonLoadGenerationRef.current) {
        return;
      }
      setOutputComparison(comparison);
    } catch (loadError) {
      if (generation !== comparisonLoadGenerationRef.current) {
        return;
      }
      setOutputComparison(null);
      setComparisonError(
        loadError instanceof Error
          ? loadError.message
          : "Output comparison unavailable",
      );
    } finally {
      if (generation === comparisonLoadGenerationRef.current) {
        setComparisonLoading(false);
      }
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
        saveBlob(
          blob,
          `job-output-${shortId(selectedJobId)}-${safeDownloadName(clientId)}-${stream}.bin`,
        );
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

  async function downloadSelectedJobArchive(
    kind: "files" | "outputs" | "status",
  ) {
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

  const approvalReviewTargets =
    approvalReview?.target_client_ids.map(clientLabel).join(", ") ||
    (approvalReview
      ? `${approvalReview.target_count} target${approvalReview.target_count === 1 ? "" : "s"}`
      : "-");
  const approvalRejectReasonMissing =
    approvalDecision === "reject" && !approvalDecisionReason.trim();
  const approvalDecisionNoteLabel =
    approvalDecision === "reject" ? "Rejection reason" : "Approval note";
  const approvalReviewRisk = approvalReview
    ? approvalReview.destructive &&
      approvalReview.risk.trim().toLowerCase() !== "destructive"
      ? `${approvalReview.risk} · destructive`
      : approvalReview.risk
    : "-";

  return (
    <section className="workspace singleColumn">
      <Suspense
        fallback={
          <div
            className="emptyState compactEmpty"
            role="status"
            aria-live="polite"
          >
            Loading {displayToken(jobSubpage)} workspace
          </div>
        }
      >
        {jobSubpage === "dispatch" && (
          <JobDispatchPanel
            agents={agents}
            fileTransferSources={fileTransferSources}
            fileTransferSourcesTruncated={fileTransferSourcesTruncated}
            commandTemplates={commandTemplates}
            commandTemplatesTruncated={commandTemplatesTruncated}
            dispatchPreset={dispatchPreset}
            onDispatchPresetApplied={onDispatchPresetApplied}
            onCreateJob={onCreateJob}
            onCreateJobApproval={onCreateJobApproval}
            onDownloadFileTransferSource={onDownloadFileTransferSource}
            onDownloadOutputChunk={onDownloadOutputChunk}
            onOpenRemoteTerminal={() => onOpenRemoteOperations?.("terminal")}
            onOpenRollout={onOpenRollout}
            onLoadJob={onLoadJob}
            onLoadOutputs={onLoadOutputs}
            onLoadTargets={onLoadTargets}
            onSubmitTerminalInput={onSubmitTerminalInput}
            onOpenJobDetails={openSubmittedJobDetails}
            onOpenPrivilegeUnlock={onOpenPrivilegeUnlock}
            onApprovalRequested={() => onSelectSubpage?.("approvals")}
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
                  <span>Latest execution records</span>
                </div>
                <div className="headerActionStack">
                  <button
                    className="secondaryAction"
                    disabled={loading}
                    onClick={onRefresh}
                    type="button"
                  >
                    Refresh
                  </button>
                  <ActionFeedback
                    message={jobHistoryFeedbackMessage}
                    tone={error ? "danger" : "progress"}
                  />
                </div>
              </div>
              <div
                className="jobHistoryWorkflowLinks"
                aria-label="Related Remote pages"
              >
                <span className="jobHistoryWorkflowIntro">
                  <strong>Related workflow owners</strong>
                  <small>
                    Use Jobs for execution evidence. Open operational workflows
                    in Remote.
                  </small>
                </span>
                <span className="jobHistoryWorkflowActions">
                  {[
                    { label: "Terminal", subpage: "terminal" },
                    { label: "Files", subpage: "files" },
                    { label: "Transfers", subpage: "transfers" },
                    { label: "Processes", subpage: "processes" },
                    { label: "Services", subpage: "services" },
                    { label: "Storage", subpage: "storage" },
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
              <ActionFeedback
                className="localActionFeedback jobHistoryFreshnessFeedback"
                message={historicalJobsBanner}
                tone="warning"
              />
              <ConsoleDataGrid
                actions={[
                  {
                    label: "Open target detail",
                    disabled: (rows) => rows.length !== 1,
                    onSelect: (rows) =>
                      openSubmittedJobDetails(rows[0].id),
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
                    <span>No job records match the current search.</span>
                  </div>
                }
                getRowId={(job) => job.id}
                itemLabel="jobs"
                onOpenRow={(job) => openSubmittedJobDetails(job.id)}
                openRowLabel="Open targets"
                openRowTitle={(job) => `Load target results for job ${job.id}.`}
                showMobileOpenRowAction={false}
                renderExpandedRow={(job) => (
                  <div className="consoleInlineDetailGrid">
                    <span>Job ID</span>
                    <strong>{job.id}</strong>
                    <span>Operation type</span>
                    <strong>{job.command_type}</strong>
                    <span>Payload hash</span>
                    <strong>{job.payload_hash}</strong>
                    <span>Started</span>
                    <strong>{formatFullTime(job.created_at)}</strong>
                    <span>Completed</span>
                    <strong>
                      {job.completed_at
                        ? formatFullTime(job.completed_at)
                        : "Not completed"}
                    </strong>
                    <span>Duration</span>
                    <strong>{formatJobDuration(job)}</strong>
                    <span>Started by</span>
                    <strong title={job.actor_id ?? undefined}>
                      {jobStartedByLabel(job)}
                    </strong>
                    <span>Actor ID</span>
                    <strong>{job.actor_id ?? "Worker automation"}</strong>
                    <span>Privilege</span>
                    <strong>
                      {job.privileged ? "Privileged" : "Unprivileged"}
                    </strong>
                    <span>Timeout</span>
                    <strong>{job.max_timeout_secs}s</strong>
                    <span>Result evidence</span>
                    <strong>
                      Job-level summary only. Open target results for per-VPS
                      exit code and error evidence.
                    </strong>
                  </div>
                )}
                rows={jobs}
                rowsTruncated={jobHistoryTruncated}
                storageKey="vpsman.grid.jobs.history"
                title="Job records"
              />
            </div>
            {selectedJobId && (
              <div
                aria-label="Job target details"
                className="targetDetail"
                ref={targetDetailRef}
                role="region"
                tabIndex={-1}
              >
                <div className="sectionHeader compact targetDetailHeader">
                  <div>
                    <h2>Target results</h2>
                    <span title={selectedJobId}>
                      {targetsLoading
                        ? "Loading target records"
                        : shortId(selectedJobId)}
                    </span>
                  </div>
                  <button
                    aria-label="Close job target details"
                    className="iconButton"
                    onClick={closeTargetDetails}
                    title="Close job target details"
                    type="button"
                  >
                    <X size={16} />
                  </button>
                </div>
                <ActionFeedback
                  className="localActionFeedback jobDetailActionFeedback"
                  message={targetDetailFeedbackMessage}
                  tone="danger"
                />
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
                      <span>This job has no resolved per-client records.</span>
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
                      <strong>{jobTargetReason(target)}</strong>
                      {target.message?.trim() &&
                      target.message.trim() !== jobTargetReason(target) ? (
                        <>
                          <span>Raw agent reason</span>
                          <strong>{target.message.trim()}</strong>
                        </>
                      ) : null}
                      <span>Completed</span>
                      <strong>
                        {target.completed_at
                          ? formatTime(target.completed_at)
                          : "Not completed"}
                      </strong>
                    </div>
                  )}
                  rowActions={[
                    {
                      description: ([target]) =>
                        target
                          ? `Open VPS detail for ${clientLabel(target.client_id)}.`
                          : "Open VPS detail.",
                      disabled: () => !onOpenVpsDetail,
                      hidden: () => !onOpenVpsDetail,
                      icon: <Server size={14} />,
                      label: "Open VPS",
                      onSelect: ([target]) => {
                        if (target) {
                          onOpenVpsDetail?.(target.client_id);
                        }
                      },
                    },
                    {
                      description: () =>
                        "Download the file retained for this target.",
                      disabled: ([target]) =>
                        !target ||
                        fileDownloadPendingClientId === target.client_id,
                      hidden: ([target]) =>
                        !target ||
                        !fileDownloadStatusByClient.has(target.client_id),
                      icon: <Download size={14} />,
                      label: "Download file",
                      onSelect: ([target]) => {
                        if (target) {
                          void downloadFileForClient(target.client_id);
                        }
                      },
                    },
                  ]}
                  rows={targets}
                  searchPlaceholder="Search targets"
                  storageKey="vpsman.jobs.history.targets"
                  title="Target result records"
                />
                <div className="outputDetail">
                  <div className="sectionHeader compact">
                    <div>
                      <h2>Output</h2>
                      <span>
                        {outputsLoading
                          ? "Loading output records"
                          : `${outputs.length} chunks`}
                      </span>
                    </div>
                    <div className="outputActions">
                      {fileDownloadStatus && (
                        <button
                          className="secondaryAction compactAction"
                          disabled={
                            outputsLoading || archivePendingKey !== null
                          }
                          onClick={() =>
                            void downloadSelectedJobArchive("files")
                          }
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
                          disabled={
                            outputsLoading || archivePendingKey !== null
                          }
                          onClick={() =>
                            void downloadSelectedJobArchive("outputs")
                          }
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
                          disabled={
                            targetsLoading || archivePendingKey !== null
                          }
                          onClick={() =>
                            void downloadSelectedJobArchive("status")
                          }
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
                  <ActionFeedback
                    className="localActionFeedback jobDetailActionFeedback"
                    message={outputDetailFeedbackMessage}
                    tone="danger"
                  />
                  <div className="executionSummary">
                    <div className="sectionHeader compact">
                      <h2>Execution summary</h2>
                      <span>
                        {comparisonLoading
                          ? "Comparing target results"
                          : outputComparison
                            ? `${outputComparison.group_count} group${outputComparison.group_count === 1 ? "" : "s"} across ${outputComparison.compared_targets} target${outputComparison.compared_targets === 1 ? "" : "s"}`
                            : "No summary loaded"}
                      </span>
                    </div>
                    <ActionFeedback
                      className="localActionFeedback jobDetailActionFeedback"
                      message={comparisonFeedbackMessage}
                      tone="danger"
                    />
                    <div className="comparisonToolbar">
                      <div
                        className="targetModeControls"
                        role="group"
                        aria-label="Output comparison mode"
                      >
                        <span>Compare</span>
                        <button
                          className={
                            comparisonMode === "binary" ? "selected" : ""
                          }
                          onClick={() => changeComparisonMode("binary")}
                          type="button"
                        >
                          Binary
                        </button>
                        <button
                          className={
                            comparisonMode === "text" ? "selected" : ""
                          }
                          onClick={() => changeComparisonMode("text")}
                          type="button"
                        >
                          Text
                        </button>
                      </div>
                      <button
                        className="secondaryAction compactAction"
                        disabled={comparisonLoading}
                        onClick={() =>
                          void compareSelectedJobOutputs(selectedJobId)
                        }
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
                          <strong>
                            {formatComparisonTime(outputComparison.compared_at)}
                          </strong>
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
                        onOpenRow={(group) =>
                          setSelectedComparisonGroupId(group.group_id)
                        }
                        openRowLabel="Select group"
                        openRowTitle={(group) =>
                          `Select comparison group ${group.group_id}.`
                        }
                        renderExpandedRow={(group) => (
                          <div className="consoleInlineDetailGrid">
                            <span>Group</span>
                            <strong>{group.group_id}</strong>
                            <span>Status</span>
                            <strong>{group.status}</strong>
                            <span>Targets</span>
                            <strong>
                              {group.client_ids.map(clientLabel).join(", ")}
                            </strong>
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
                            <strong>
                              {row.stream_count} chunks /{" "}
                              {formatBytes(row.byte_count)}
                            </strong>
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
                        <div
                          className="outputDownloadRow"
                          key={target.clientId}
                        >
                          <span className="historyPrimary">
                            <strong>{clientLabel(target.clientId)}</strong>
                            <small>retained stdout/stderr payload</small>
                          </span>
                          <span className="inlineActions">
                            {target.stdout && (
                              <button
                                className="secondaryAction compactAction"
                                disabled={
                                  streamPendingKey ===
                                  `${target.clientId}:stdout`
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
                                  streamPendingKey ===
                                  `${target.clientId}:stderr`
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
                                  streamPendingKey ===
                                  `${target.clientId}:combined`
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
                          This job has no retained stdout, stderr, or status
                          output.
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
                    {approvalsTruncated
                      ? `${formatLowerBoundCount(
                          pendingApprovalCount,
                          true,
                        )} pending in ${formatLowerBoundCount(
                          jobApprovals.length,
                          true,
                        )} loaded requests`
                      : `${pendingApprovalCount} pending · ${jobApprovals.length} total request${jobApprovals.length === 1 ? "" : "s"}`}
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
              <ActionFeedback
                className="localActionFeedback approvalActionFeedback"
                message={approvalActionFeedbackMessage}
                tone={approvalActionFeedbackTone}
              />
              <ConfirmationPrompt
                cancelLabel="Close"
                confirmDisabled={approvalRejectReasonMissing}
                confirmLabel={
                  approvalDecision === "approve"
                    ? "Approve and dispatch"
                    : "Reject request"
                }
                detail="Review the frozen job request before recording a decision. Approval can include an optional note; rejection requires the operator reason."
                error={
                  approvalRejectReasonMissing
                    ? "Rejection reason is required."
                    : undefined
                }
                items={[
                  {
                    label: "Operation",
                    value: approvalReview
                      ? displayCommandType(approvalReview.command_type)
                      : "-",
                  },
                  {
                    label: "Targets",
                    title: approvalReviewTargets,
                    value: approvalReviewTargets,
                  },
                  {
                    label: "Requester",
                    value: approvalReview
                      ? `${approvalReview.requester_username} (${approvalReview.requester_role})`
                      : "-",
                  },
                  {
                    label: "Risk",
                    value: approvalReviewRisk,
                  },
                  {
                    label: "Requested",
                    value: approvalReview
                      ? formatTime(approvalReview.requested_at)
                      : "-",
                  },
                  {
                    label: "Selector",
                    title:
                      approvalReview?.selector_expression || "fixed target set",
                    value:
                      approvalReview?.selector_expression || "fixed target set",
                  },
                  {
                    label: "Payload",
                    title: approvalReview?.payload_hash,
                    value: approvalReview
                      ? shortHash(approvalReview.payload_hash)
                      : "-",
                  },
                  {
                    label: "Request reason",
                    title: approvalReview?.request_reason ?? undefined,
                    value:
                      approvalReview?.request_reason ?? "No request reason",
                  },
                ]}
                onCancel={closeApprovalReview}
                onConfirm={submitApprovalDecision}
                open={approvalReview !== null}
                pending={approvalActionPending}
                title="Review job approval"
                tone={
                  approvalDecision === "approve" && approvalReview?.destructive
                    ? "danger"
                    : "normal"
                }
              >
                <div className="approvalDecisionFields">
                  <div
                    aria-label="Approval decision"
                    className="approvalDecisionToggle"
                    role="group"
                  >
                    <button
                      aria-pressed={approvalDecision === "approve"}
                      className={
                        approvalDecision === "approve"
                          ? "secondaryAction compactAction active"
                          : "secondaryAction compactAction"
                      }
                      disabled={approvalActionPending}
                      onClick={() => {
                        setApprovalDecision("approve");
                        setApprovalActionError(null);
                      }}
                      type="button"
                    >
                      Approve
                    </button>
                    <button
                      aria-pressed={approvalDecision === "reject"}
                      className={
                        approvalDecision === "reject"
                          ? "secondaryAction compactAction dangerAction active"
                          : "secondaryAction compactAction dangerAction"
                      }
                      disabled={approvalActionPending}
                      onClick={() => {
                        setApprovalDecision("reject");
                        setApprovalActionError(null);
                      }}
                      type="button"
                    >
                      Reject
                    </button>
                  </div>
                  <label className="confirmationTypedInput approvalDecisionNote">
                    <span>
                      {approvalDecisionNoteLabel}
                      {approvalDecision === "approve"
                        ? " (optional)"
                        : " (required)"}
                    </span>
                    <textarea
                      aria-label={approvalDecisionNoteLabel}
                      disabled={approvalActionPending}
                      maxLength={1024}
                      onChange={(event) => {
                        setApprovalDecisionReason(event.target.value);
                        if (approvalDecision === "reject") {
                          setApprovalActionError(null);
                        }
                      }}
                      placeholder={
                        approvalDecision === "approve"
                          ? "Optional decision note"
                          : "Why this reviewed request is being rejected"
                      }
                      rows={3}
                      value={approvalDecisionReason}
                    />
                  </label>
                </div>
              </ConfirmationPrompt>
              <ConsoleDataGrid
                columns={approvalColumns}
                defaultColumnVisibility={{ requested: true }}
                defaultPageSize={10}
                empty={
                  <div className="emptyState">
                    <ShieldCheck size={22} />
                    <strong>No reviewed work is waiting</strong>
                    <span>
                      Approval requests that have passed privilege review appear
                      here for final dispatch or rejection.
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
                    <strong>
                      {approval.target_client_ids.map(clientLabel).join(", ")}
                    </strong>
                    <span>Payload</span>
                    <strong>{approval.payload_hash}</strong>
                    <span>Fingerprint</span>
                    <strong>{approval.request_fingerprint}</strong>
                    <span>Timeout</span>
                    <strong>{approval.max_timeout_secs}s</strong>
                    <span>Request reason</span>
                    <strong>
                      {approval.request_reason ?? "No request reason"}
                    </strong>
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
                    label: "Review",
                    icon: <ShieldCheck size={14} />,
                    disabled: (rows) =>
                      rows.length !== 1 ||
                      rows[0].status !== "pending" ||
                      approvalActionPending,
                    onSelect: (rows) => openApprovalReview(rows[0]),
                  },
                ]}
                rows={jobApprovals}
                rowsTruncated={approvalsTruncated}
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
                  <span>
                    {`${formatLowerBoundCount(
                      scheduleRunJobs.length,
                      jobHistoryTruncated,
                    )}${jobHistoryTruncated ? " loaded" : ""} schedule-created ${
                      scheduleRunJobs.length === 1 ? "run" : "runs"
                    }`}
                  </span>
                </div>
                <div className="inlineActions">
                  <button
                    className="secondaryAction compactAction"
                    onClick={onOpenSchedules}
                    type="button"
                  >
                    Open schedule registry
                  </button>
                  <button
                    className="secondaryAction compactAction"
                    disabled={loading}
                    onClick={onRefresh}
                    type="button"
                  >
                    Refresh
                  </button>
                </div>
              </div>
              {scheduleRunJobs.length > 0 ? (
                <ConsoleDataGrid
                  columns={scheduledRunColumns}
                  defaultPageSize={10}
                  empty="No scheduled runs match the current search."
                  expandOnRowClick={false}
                  getRowId={(job) => job.id}
                  itemLabel="runs"
                  onOpenRow={(job) => openSubmittedJobDetails(job.id)}
                  openRowLabel="Open targets"
                  openRowTitle={(job) =>
                    `Load target results for scheduled run ${job.id}.`
                  }
                  renderExpandedRow={(job) => {
                    const schedule = job.source_schedule_id
                      ? scheduleById.get(job.source_schedule_id)
                      : undefined;
                    return (
                      <div className="consoleInlineDetailGrid">
                        <span>Schedule</span>
                        <strong title={job.source_schedule_id ?? undefined}>
                          {scheduledRunScheduleLabel(job, schedule)}
                        </strong>
                        <span>Schedule ID</span>
                        <strong>
                          {job.source_schedule_id ?? "Data unavailable"}
                        </strong>
                        <span>Cadence</span>
                        <strong>
                          {schedule
                            ? `${scheduledRunCadenceLabel(schedule)} · ${schedule.timezone}`
                            : "Open schedule registry"}
                        </strong>
                        <span>Current next run</span>
                        <strong>
                          {schedule
                            ? scheduledRunNextRunLabel(schedule)
                            : "Data unavailable"}
                        </strong>
                        <span>Job</span>
                        <strong>{job.id}</strong>
                        <span>Operation</span>
                        <strong>
                          {scheduledRunCommandLabel(job.command_type)}
                        </strong>
                        <span>Targets</span>
                        <strong>
                          {job.target_count} target
                          {job.target_count === 1 ? "" : "s"}
                        </strong>
                        <span>Due time</span>
                        <strong>Not reported by job history</strong>
                        <span>Started</span>
                        <strong>{formatTime(job.created_at)}</strong>
                        <span>Completed</span>
                        <strong>
                          {job.completed_at
                            ? formatTime(job.completed_at)
                            : "Not completed"}
                        </strong>
                        <span>Duration</span>
                        <strong>{formatJobDuration(job)}</strong>
                        <span>Payload</span>
                        <strong>{job.payload_hash}</strong>
                        <span>Authority</span>
                        <strong title={job.actor_id ?? undefined}>
                          {job.actor_id
                            ? `Operator ${shortId(job.actor_id)}`
                            : "Worker automation"}{" "}
                          · {job.privileged ? "privileged" : "unprivileged"}
                        </strong>
                      </div>
                    );
                  }}
                  rows={scheduleRunJobs}
                  rowsTruncated={jobHistoryTruncated}
                  searchPlaceholder="Search scheduled runs"
                  selectable={false}
                  showMobileOpenRowAction={false}
                  singleExpandedRow
                  storageKey="vpsman.jobs.scheduledRuns"
                  title="Schedule run records"
                />
              ) : (
                <div className="emptyState">
                  <ShieldCheck size={22} />
                  <strong>No schedule runs yet</strong>
                  <span>
                    Due schedule jobs are created and dispatched by worker
                    automation. Create or inspect schedules in the registry.
                  </span>
                  <div className="emptyStateActions">
                    <button
                      className="primaryAction compactAction"
                      onClick={onOpenSchedules}
                      type="button"
                    >
                      Open schedule registry
                    </button>
                    <button
                      className="secondaryAction compactAction"
                      disabled={loading}
                      onClick={onRefresh}
                      type="button"
                    >
                      Refresh runs
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

function jobTargetReason(target: JobTargetRecord): string {
  const raw = target.message?.trim();
  if (raw && raw !== target.status) {
    if (raw === "command_timeout") {
      return "Agent stopped the command after its configured execution timeout.";
    }
    return raw;
  }
  switch (target.status) {
    case "completed":
      return "-";
    case "running":
    case "dispatching":
      return "Waiting for a terminal result.";
    case "agent_timeout":
      return "Agent stopped the command after its configured execution timeout.";
    case "control_timeout":
      return "The control plane did not receive a terminal result before the job deadline.";
    case "agent_lost":
      return "The agent disconnected before reporting a terminal result.";
    case "canceled":
      return "The command was canceled before completion.";
    case "rejected":
      return "The agent rejected the command without an additional reason.";
    case "failed":
      return "The command failed without an additional reason.";
    default:
      return raw || "No failure reason reported.";
  }
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function jobHistoryFreshnessBanner(jobs: JobHistoryRecord[]): string | null {
  const timestamps = jobs
    .map((job) => Date.parse(job.created_at))
    .filter(Number.isFinite);
  if (timestamps.length === 0) {
    return null;
  }
  const newestMs = Math.max(...timestamps);
  if (Date.now() - newestMs < 7 * 24 * 60 * 60 * 1000) {
    return null;
  }
  return `Showing historical jobs from ${formatCompactTime(new Date(newestMs).toISOString())}; refresh or open live workflow pages for current operation state.`;
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

function safeDownloadName(
  value: string | null | undefined,
  fallback = "download.bin",
): string {
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
