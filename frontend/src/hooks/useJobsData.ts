import { useCallback, useRef, useState } from "react";
import {
  apiDelete,
  apiGet,
  apiGetBlob,
  apiPost,
  apiPostPreview,
  buildListPath,
  isApiUnauthorized,
  LatestReadConsumer,
} from "../api";
import {
  downloadVerifiedArtifact,
  type ArtifactDownloadMode,
} from "../artifactDownload";
import { FLEET_DETAIL_LIMIT, HISTORY_DETAIL_LIMIT } from "../constants";
import {
  snapshotSourceAvailable,
  snapshotSourceError,
  type SnapshotSource,
} from "../homeSnapshot";
import type {
  AgentUpdateReleaseRecord,
  CommandTemplateRecord,
  CancelJobResponse,
  CreateJobApprovalRequest,
  CreateAgentUpdateReleaseRequest,
  CreateJobRequest,
  CreateJobResponse,
  DecideJobApprovalRequest,
  JobHistoryRecord,
  JobStatus,
  JobRolloutRecord,
  JobApprovalDecisionResponse,
  JobApprovalRecord,
  JobOutputListPageRecord,
  JobOutputCompareMode,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetStatusRequestItem,
  HostProcessInventoryRecord,
  HostPackageUpdatePlanRecord,
  HostServiceInventoryRecord,
  HostStorageInventoryRecord,
  ProcessSupervisorInventoryRecord,
  ArtifactCleanupPreviewRecord,
  ServerJobRecord,
  DeleteCommandTemplateRequest,
  UpsertCommandTemplateRequest,
  UpdateJobRolloutRequest,
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

const JOB_ERROR_SOURCE_ORDER = [
  "jobHistory",
  "jobApprovals",
  "jobRollouts",
  "agentUpdateReleases",
  "processSupervisorInventory",
  "fileTransfers",
  "fileTransferSources",
  "terminalSessions",
  "commandTemplates",
] as const;
type JobErrorSource = (typeof JOB_ERROR_SOURCE_ORDER)[number];

type HomeJobsHydrationFence = {
  inventory: number;
  jobHistory: number;
  jobHistoryOverlay: number;
  fileTransfers: number;
  terminal: number;
};

export function useJobsData(
  apiToken: string,
  onUnauthorized: () => void,
  onFleetChanged: () => Promise<void>,
  onAuditChanged: () => Promise<void>,
) {
  const [jobs, setJobs] = useState<JobHistoryRecord[]>([]);
  const [jobApprovals, setJobApprovals] = useState<JobApprovalRecord[]>([]);
  const [jobRollouts, setJobRollouts] = useState<JobRolloutRecord[]>([]);
  const [agentUpdateReleases, setAgentUpdateReleases] = useState<
    AgentUpdateReleaseRecord[]
  >([]);
  const [processSupervisorInventory, setProcessSupervisorInventory] = useState<
    ProcessSupervisorInventoryRecord[]
  >([]);
  const [fileTransfers, setFileTransfers] = useState<
    FileTransferSessionRecord[]
  >([]);
  const [fileTransferSources, setFileTransferSources] = useState<
    FileTransferSourceArtifactRecord[]
  >([]);
  const [terminalSessions, setTerminalSessions] = useState<
    TerminalSessionRecord[]
  >([]);
  const [serverJobs, setServerJobs] = useState<ServerJobRecord[]>([]);
  const [commandTemplates, setCommandTemplates] = useState<
    CommandTemplateRecord[]
  >([]);
  const [jobsTruncated, setJobsTruncated] = useState(false);
  const [jobRolloutsTruncated, setJobRolloutsTruncated] = useState(false);
  const [agentUpdateReleasesTruncated, setAgentUpdateReleasesTruncated] =
    useState(false);
  const [
    processSupervisorInventoryTruncated,
    setProcessSupervisorInventoryTruncated,
  ] = useState(false);
  const [fileTransfersTruncated, setFileTransfersTruncated] = useState(false);
  const [fileTransferSourcesTruncated, setFileTransferSourcesTruncated] =
    useState(false);
  const [terminalSessionsTruncated, setTerminalSessionsTruncated] =
    useState(false);
  const [commandTemplatesTruncated, setCommandTemplatesTruncated] =
    useState(false);
  const [jobsError, setJobsError] = useState<string | null>(null);
  const [serverJobsError, setServerJobsError] = useState<string | null>(null);
  const [jobsLoading, setJobsLoading] = useState(false);
  const [jobsEvidenceAvailable, setJobsEvidenceAvailable] = useState(false);
  const jobsRef = useRef<JobHistoryRecord[]>([]);
  const jobRolloutsRef = useRef<JobRolloutRecord[]>([]);
  const jobsLoadConsumer = useRef(new LatestReadConsumer());
  const jobHistoryLoadConsumer = useRef(
    new LatestReadConsumer<JobHistoryRecord[]>(),
  );
  const jobsLoadGeneration = useRef(0);
  const jobHistoryLoadGeneration = useRef(0);
  const jobApprovalsLoadGeneration = useRef(0);
  const processSupervisorInventoryLoadGeneration = useRef(0);
  const jobRowRefreshGeneration = useRef(new Map<string, number>());
  const jobRowRefreshInFlight = useRef(
    new Map<string, Promise<JobHistoryRecord | null>>(),
  );
  const jobRolloutsLoadGeneration = useRef(0);
  const agentUpdateReleasesLoadGeneration = useRef(0);
  const fileTransfersLoadGeneration = useRef(0);
  const fileTransferSourcesLoadGeneration = useRef(0);
  const terminalSessionsLoadGeneration = useRef(0);
  const serverJobsLoadGeneration = useRef(0);
  const commandTemplatesLoadGeneration = useRef(0);
  const commandTemplateMutationGeneration = useRef(0);
  const jobApprovalMutationGeneration = useRef(0);
  const jobRolloutMutationGeneration = useRef(0);
  const jobHistoryOverlay = useRef(createProjectionOverlay<JobHistoryRecord>());
  const jobApprovalsOverlay = useRef(
    createProjectionOverlay<JobApprovalRecord>(),
  );
  const jobRolloutsOverlay = useRef(
    createProjectionOverlay<JobRolloutRecord>(),
  );
  const agentUpdateReleasesOverlay = useRef(
    createProjectionOverlay<AgentUpdateReleaseRecord>(),
  );
  const fileTransferSourcesOverlay = useRef(
    createProjectionOverlay<FileTransferSourceArtifactRecord>(),
  );
  const serverJobsOverlay = useRef(createProjectionOverlay<ServerJobRecord>());
  const commandTemplatesOverlay = useRef(
    createProjectionOverlay<CommandTemplateRecord>(),
  );
  const jobSourceErrors = useRef<Partial<Record<JobErrorSource, string>>>({});
  const jobHistoryEvidenceAvailable = useRef(false);
  const fileTransfersEvidenceAvailable = useRef(false);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const publishJobsError = useCallback(() => {
    const errors = JOB_ERROR_SOURCE_ORDER.flatMap((source) => {
      const error = jobSourceErrors.current[source];
      return error ? [error] : [];
    });
    setJobsError(errors.length > 0 ? errors.join("; ") : null);
  }, []);

  const publishJobsEvidence = useCallback(() => {
    setJobsEvidenceAvailable(
      jobHistoryEvidenceAvailable.current &&
        fileTransfersEvidenceAvailable.current,
    );
  }, []);

  const loadJobHistory = useCallback((): Promise<JobHistoryRecord[]> => {
    if (currentApiToken.current !== apiToken) {
      return Promise.resolve(jobsRef.current);
    }
    return jobHistoryLoadConsumer.current.enqueue(async () => {
      const generation = jobHistoryLoadGeneration.current + 1;
      jobHistoryLoadGeneration.current = generation;
      const overlayRevision = jobHistoryOverlay.current.revision;
      try {
        const records = await apiGet<JobHistoryRecord[]>(
          buildListPath("/api/v1/jobs", {
            limit: HISTORY_DETAIL_LIMIT,
            sort: "created_at",
            dir: "desc",
          }),
          apiToken,
        );
        if (
          currentApiToken.current !== apiToken ||
          jobHistoryLoadGeneration.current !== generation
        ) {
          return jobsRef.current;
        }
        const merged = mergeProjectionRead(
          records,
          jobHistoryOverlay.current,
          overlayRevision,
          (record) => record.id,
          compareCreatedRecords,
          HISTORY_DETAIL_LIMIT,
        );
        jobsRef.current = merged.records;
        setJobs(merged.records);
        setJobsTruncated(merged.truncated);
        jobHistoryEvidenceAvailable.current = true;
        setProjectionError(jobSourceErrors.current, "jobHistory", null);
        publishJobsEvidence();
        publishJobsError();
        return merged.records;
      } catch (error) {
        if (
          currentApiToken.current === apiToken &&
          jobHistoryLoadGeneration.current === generation
        ) {
          jobHistoryEvidenceAvailable.current = false;
          setProjectionError(
            jobSourceErrors.current,
            "jobHistory",
            isApiUnauthorized(error)
              ? "Operator login required"
              : error instanceof Error
                ? `Job history: ${error.message}`
                : "Job history unavailable",
          );
          publishJobsEvidence();
          publishJobsError();
          if (isApiUnauthorized(error)) {
            onUnauthorized();
          }
        }
        throw error;
      }
    });
  }, [apiToken, onUnauthorized, publishJobsError, publishJobsEvidence]);

  const rethrowDirectRequestError = useCallback(
    (error: unknown): never => {
      if (currentApiToken.current !== apiToken) {
        throw error;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        throw new Error("Operator login required");
      }
      throw error;
    },
    [apiToken, onUnauthorized],
  );

  const beginHomeJobsHydration = useCallback((): HomeJobsHydrationFence => {
    setJobsLoading(true);
    return {
      inventory: ++jobsLoadGeneration.current,
      jobHistory: ++jobHistoryLoadGeneration.current,
      jobHistoryOverlay: jobHistoryOverlay.current.revision,
      fileTransfers: ++fileTransfersLoadGeneration.current,
      terminal: ++terminalSessionsLoadGeneration.current,
    };
  }, []);

  const hydrateHomeJobs = useCallback(
    (
      fence: HomeJobsHydrationFence,
      jobSource: SnapshotSource<JobHistoryRecord[]>,
      fileTransferSource: SnapshotSource<FileTransferSessionRecord[]>,
      terminalSessionSource: SnapshotSource<TerminalSessionRecord[]>,
    ) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (fence.jobHistory === jobHistoryLoadGeneration.current) {
        if (snapshotSourceAvailable(jobSource)) {
          const merged = mergeProjectionRead(
            jobSource.data,
            jobHistoryOverlay.current,
            fence.jobHistoryOverlay,
            (record) => record.id,
            compareCreatedRecords,
            HISTORY_DETAIL_LIMIT,
          );
          jobsRef.current = merged.records;
          setJobs(merged.records);
          setJobsTruncated(merged.truncated);
          jobHistoryEvidenceAvailable.current = true;
          setProjectionError(jobSourceErrors.current, "jobHistory", null);
        } else {
          jobHistoryEvidenceAvailable.current = false;
          setProjectionError(
            jobSourceErrors.current,
            "jobHistory",
            snapshotSourceError("Job history", jobSource),
          );
        }
        publishJobsEvidence();
      }
      if (fence.fileTransfers === fileTransfersLoadGeneration.current) {
        if (snapshotSourceAvailable(fileTransferSource)) {
          setFileTransfers(fileTransferSource.data);
          setFileTransfersTruncated(
            fileTransferSource.data.length >= FLEET_DETAIL_LIMIT,
          );
          fileTransfersEvidenceAvailable.current = true;
          setProjectionError(jobSourceErrors.current, "fileTransfers", null);
        } else {
          fileTransfersEvidenceAvailable.current = false;
          setProjectionError(
            jobSourceErrors.current,
            "fileTransfers",
            snapshotSourceError("File transfer sessions", fileTransferSource),
          );
        }
        publishJobsEvidence();
      }
      if (fence.inventory === jobsLoadGeneration.current) {
        setJobsLoading(false);
      }
      if (fence.terminal === terminalSessionsLoadGeneration.current) {
        if (snapshotSourceAvailable(terminalSessionSource)) {
          setTerminalSessions(terminalSessionSource.data);
          setTerminalSessionsTruncated(
            terminalSessionSource.data.length >= FLEET_DETAIL_LIMIT,
          );
        }
        setProjectionError(
          jobSourceErrors.current,
          "terminalSessions",
          snapshotSourceError("Terminal sessions", terminalSessionSource),
        );
      }
      publishJobsError();
    },
    [apiToken, publishJobsError, publishJobsEvidence],
  );

  const loadJobs = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const loadingGeneration = ++jobsLoadGeneration.current;
    setJobsLoading(true);
    try {
      await jobsLoadConsumer.current.enqueue(async () => {
        if (currentApiToken.current !== apiToken) {
          return;
        }
        const approvalsGeneration = ++jobApprovalsLoadGeneration.current;
        const approvalsOverlayRevision = jobApprovalsOverlay.current.revision;
        const rolloutsGeneration = ++jobRolloutsLoadGeneration.current;
        const rolloutsOverlayRevision = jobRolloutsOverlay.current.revision;
        const releasesGeneration = ++agentUpdateReleasesLoadGeneration.current;
        const releasesOverlayRevision =
          agentUpdateReleasesOverlay.current.revision;
        const processSupervisorGeneration =
          ++processSupervisorInventoryLoadGeneration.current;
        const fileTransfersGeneration = ++fileTransfersLoadGeneration.current;
        const fileTransferSourcesGeneration =
          ++fileTransferSourcesLoadGeneration.current;
        const fileTransferSourcesOverlayRevision =
          fileTransferSourcesOverlay.current.revision;
        const terminalGeneration = ++terminalSessionsLoadGeneration.current;
        const serverGeneration = ++serverJobsLoadGeneration.current;
        const serverOverlayRevision = serverJobsOverlay.current.revision;
        const commandTemplatesGeneration =
          ++commandTemplatesLoadGeneration.current;
        const commandTemplatesOverlayRevision =
          commandTemplatesOverlay.current.revision;
        for (const source of JOB_ERROR_SOURCE_ORDER) {
          setProjectionError(jobSourceErrors.current, source, null);
        }
        publishJobsError();
        setServerJobsError(null);
        const [
          jobsResult,
          jobApprovalsResult,
          jobRolloutsResult,
          releasesResult,
          processSupervisorInventoryResult,
          fileTransfersResult,
          fileTransferSourcesResult,
          terminalSessionsResult,
          serverJobsResult,
          commandTemplatesResult,
        ] = await Promise.allSettled([
          loadJobHistory(),
          apiGet<JobApprovalRecord[]>(
            buildListPath("/api/v1/job-approvals", {
              limit: FLEET_DETAIL_LIMIT,
              sort: "requested_at",
              dir: "desc",
            }),
            apiToken,
          ),
          apiGet<JobRolloutRecord[]>(
            `/api/v1/job-rollouts?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<AgentUpdateReleaseRecord[]>(
            `/api/v1/agent-update-releases?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<ProcessSupervisorInventoryRecord[]>(
            `/api/v1/process-supervisor/inventory?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<FileTransferSessionRecord[]>(
            `/api/v1/file-transfers?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<FileTransferSourceArtifactRecord[]>(
            `/api/v1/file-transfer-sources?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<TerminalSessionRecord[]>(
            `/api/v1/terminal-sessions?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<ServerJobRecord[]>(
            `/api/v1/server-jobs?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
          apiGet<CommandTemplateRecord[]>(
            `/api/v1/command-templates?limit=${FLEET_DETAIL_LIMIT}`,
            apiToken,
          ),
        ]);
        if (currentApiToken.current !== apiToken) {
          return;
        }
        const settledResults = [
          jobsResult,
          jobApprovalsResult,
          jobRolloutsResult,
          releasesResult,
          processSupervisorInventoryResult,
          fileTransfersResult,
          fileTransferSourcesResult,
          terminalSessionsResult,
          serverJobsResult,
          commandTemplatesResult,
        ];
        const unauthorized = settledResults.some(
          (result) =>
            result.status === "rejected" && isApiUnauthorized(result.reason),
        );
        if (unauthorized) {
          onUnauthorized();
          jobHistoryEvidenceAvailable.current = false;
          fileTransfersEvidenceAvailable.current = false;
          publishJobsEvidence();
          jobsRef.current = [];
          setJobs([]);
          setJobApprovals([]);
          setJobRollouts([]);
          jobRolloutsRef.current = [];
          setAgentUpdateReleases([]);
          setProcessSupervisorInventory([]);
          setFileTransfers([]);
          setFileTransferSources([]);
          setTerminalSessions([]);
          setServerJobs([]);
          setCommandTemplates([]);
          setJobsTruncated(false);
          setJobRolloutsTruncated(false);
          setAgentUpdateReleasesTruncated(false);
          setProcessSupervisorInventoryTruncated(false);
          setFileTransfersTruncated(false);
          setFileTransferSourcesTruncated(false);
          setTerminalSessionsTruncated(false);
          setCommandTemplatesTruncated(false);
          jobSourceErrors.current = {
            jobHistory: "Operator login required",
          };
          publishJobsError();
          setServerJobsError("Operator login required");
          return;
        }
        if (jobApprovalsLoadGeneration.current === approvalsGeneration) {
          if (jobApprovalsResult.status === "fulfilled") {
            const merged = mergeProjectionRead(
              jobApprovalsResult.value,
              jobApprovalsOverlay.current,
              approvalsOverlayRevision,
              (record) => record.id,
              compareRequestedRecords,
              FLEET_DETAIL_LIMIT,
            );
            setJobApprovals(merged.records);
          }
          setProjectionError(
            jobSourceErrors.current,
            "jobApprovals",
            settledSourceFailure("Job approvals", jobApprovalsResult),
          );
        }
        if (jobRolloutsLoadGeneration.current === rolloutsGeneration) {
          if (jobRolloutsResult.status === "fulfilled") {
            const merged = mergeProjectionRead(
              jobRolloutsResult.value,
              jobRolloutsOverlay.current,
              rolloutsOverlayRevision,
              (record) => record.job_id,
              compareCreatedRecords,
              FLEET_DETAIL_LIMIT,
            );
            jobRolloutsRef.current = merged.records;
            setJobRollouts(merged.records);
            setJobRolloutsTruncated(merged.truncated);
          }
          setProjectionError(
            jobSourceErrors.current,
            "jobRollouts",
            settledSourceFailure("Job rollouts", jobRolloutsResult),
          );
        }
        if (agentUpdateReleasesLoadGeneration.current === releasesGeneration) {
          if (releasesResult.status === "fulfilled") {
            const merged = mergeProjectionRead(
              releasesResult.value,
              agentUpdateReleasesOverlay.current,
              releasesOverlayRevision,
              (record) => record.id,
              compareCreatedRecords,
              FLEET_DETAIL_LIMIT,
            );
            setAgentUpdateReleases(merged.records);
            setAgentUpdateReleasesTruncated(merged.truncated);
          }
          setProjectionError(
            jobSourceErrors.current,
            "agentUpdateReleases",
            settledSourceFailure("Agent update releases", releasesResult),
          );
        }
        if (
          processSupervisorInventoryLoadGeneration.current ===
          processSupervisorGeneration
        ) {
          if (processSupervisorInventoryResult.status === "fulfilled") {
            setProcessSupervisorInventory(
              processSupervisorInventoryResult.value,
            );
            setProcessSupervisorInventoryTruncated(
              processSupervisorInventoryResult.value.length >=
                FLEET_DETAIL_LIMIT,
            );
          }
          setProjectionError(
            jobSourceErrors.current,
            "processSupervisorInventory",
            settledSourceFailure(
              "Process supervisor inventory",
              processSupervisorInventoryResult,
            ),
          );
        }
        if (fileTransfersLoadGeneration.current === fileTransfersGeneration) {
          if (fileTransfersResult.status === "fulfilled") {
            setFileTransfers(fileTransfersResult.value);
            setFileTransfersTruncated(
              fileTransfersResult.value.length >= FLEET_DETAIL_LIMIT,
            );
            fileTransfersEvidenceAvailable.current = true;
          } else {
            fileTransfersEvidenceAvailable.current = false;
          }
          setProjectionError(
            jobSourceErrors.current,
            "fileTransfers",
            settledSourceFailure("File transfer sessions", fileTransfersResult),
          );
          publishJobsEvidence();
        }
        if (
          fileTransferSourcesLoadGeneration.current ===
          fileTransferSourcesGeneration
        ) {
          if (fileTransferSourcesResult.status === "fulfilled") {
            const merged = mergeProjectionRead(
              fileTransferSourcesResult.value,
              fileTransferSourcesOverlay.current,
              fileTransferSourcesOverlayRevision,
              (record) => record.id,
              compareCreatedRecords,
              FLEET_DETAIL_LIMIT,
            );
            setFileTransferSources(merged.records);
            setFileTransferSourcesTruncated(merged.truncated);
          }
          setProjectionError(
            jobSourceErrors.current,
            "fileTransferSources",
            settledSourceFailure(
              "File transfer sources",
              fileTransferSourcesResult,
            ),
          );
        }
        if (terminalSessionsLoadGeneration.current === terminalGeneration) {
          if (terminalSessionsResult.status === "fulfilled") {
            setTerminalSessions(terminalSessionsResult.value);
            setTerminalSessionsTruncated(
              terminalSessionsResult.value.length >= FLEET_DETAIL_LIMIT,
            );
          }
          setProjectionError(
            jobSourceErrors.current,
            "terminalSessions",
            settledSourceFailure("Terminal sessions", terminalSessionsResult),
          );
        }
        if (serverJobsLoadGeneration.current === serverGeneration) {
          if (serverJobsResult.status === "fulfilled") {
            const merged = mergeProjectionRead(
              serverJobsResult.value,
              serverJobsOverlay.current,
              serverOverlayRevision,
              (record) => record.id,
              compareCreatedRecords,
              FLEET_DETAIL_LIMIT,
            );
            setServerJobs(merged.records);
            setServerJobsError(null);
          } else {
            setServerJobsError(
              serverJobsResult.reason instanceof Error
                ? `Maintenance jobs: ${serverJobsResult.reason.message}`
                : "Maintenance job inventory unavailable",
            );
          }
        }
        if (
          commandTemplatesLoadGeneration.current === commandTemplatesGeneration
        ) {
          if (commandTemplatesResult.status === "fulfilled") {
            const merged = mergeProjectionRead(
              commandTemplatesResult.value,
              commandTemplatesOverlay.current,
              commandTemplatesOverlayRevision,
              (record) => record.id,
              compareCommandTemplates,
              FLEET_DETAIL_LIMIT,
            );
            setCommandTemplates(merged.records);
            setCommandTemplatesTruncated(merged.truncated);
          }
          setProjectionError(
            jobSourceErrors.current,
            "commandTemplates",
            settledSourceFailure("Command templates", commandTemplatesResult),
          );
        }
        publishJobsError();
      });
    } finally {
      if (
        jobsLoadGeneration.current === loadingGeneration &&
        currentApiToken.current === apiToken
      ) {
        setJobsLoading(false);
      }
    }
  }, [
    apiToken,
    onUnauthorized,
    publishJobsError,
    publishJobsEvidence,
    loadJobHistory,
  ]);

  const loadAgentUpdateReleases = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = agentUpdateReleasesLoadGeneration.current + 1;
    agentUpdateReleasesLoadGeneration.current = generation;
    const overlayRevision = agentUpdateReleasesOverlay.current.revision;
    setProjectionError(jobSourceErrors.current, "agentUpdateReleases", null);
    publishJobsError();
    try {
      const records = await apiGet<AgentUpdateReleaseRecord[]>(
        `/api/v1/agent-update-releases?limit=${FLEET_DETAIL_LIMIT}`,
        apiToken,
      );
      if (
        agentUpdateReleasesLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      const merged = mergeProjectionRead(
        records,
        agentUpdateReleasesOverlay.current,
        overlayRevision,
        (record) => record.id,
        compareCreatedRecords,
        FLEET_DETAIL_LIMIT,
      );
      setAgentUpdateReleases(merged.records);
      setAgentUpdateReleasesTruncated(merged.truncated);
      setProjectionError(jobSourceErrors.current, "agentUpdateReleases", null);
      publishJobsError();
    } catch (error) {
      if (
        agentUpdateReleasesLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setAgentUpdateReleases([]);
        setAgentUpdateReleasesTruncated(false);
        setProjectionError(
          jobSourceErrors.current,
          "agentUpdateReleases",
          "Operator login required",
        );
        publishJobsError();
        return;
      }
      setProjectionError(
        jobSourceErrors.current,
        "agentUpdateReleases",
        error instanceof Error
          ? `Agent update releases: ${error.message}`
          : "Agent update releases unavailable",
      );
      publishJobsError();
    }
  }, [apiToken, onUnauthorized, publishJobsError]);

  const loadJobRollouts = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return jobRolloutsRef.current;
    }
    const generation = jobRolloutsLoadGeneration.current + 1;
    jobRolloutsLoadGeneration.current = generation;
    const overlayRevision = jobRolloutsOverlay.current.revision;
    setProjectionError(jobSourceErrors.current, "jobRollouts", null);
    publishJobsError();
    try {
      const records = await apiGet<JobRolloutRecord[]>(
        `/api/v1/job-rollouts?limit=${FLEET_DETAIL_LIMIT}`,
        apiToken,
      );
      if (
        jobRolloutsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return jobRolloutsRef.current;
      }
      const merged = mergeProjectionRead(
        records,
        jobRolloutsOverlay.current,
        overlayRevision,
        (record) => record.job_id,
        compareCreatedRecords,
        FLEET_DETAIL_LIMIT,
      );
      jobRolloutsRef.current = merged.records;
      setJobRollouts(merged.records);
      setJobRolloutsTruncated(merged.truncated);
      setProjectionError(jobSourceErrors.current, "jobRollouts", null);
      publishJobsError();
      return merged.records;
    } catch (error) {
      if (
        jobRolloutsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return jobRolloutsRef.current;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        jobRolloutsRef.current = [];
        setJobRollouts([]);
        setJobRolloutsTruncated(false);
        setProjectionError(
          jobSourceErrors.current,
          "jobRollouts",
          "Operator login required",
        );
        publishJobsError();
        throw new Error("Operator login required");
      }
      setProjectionError(
        jobSourceErrors.current,
        "jobRollouts",
        error instanceof Error
          ? `Job rollouts: ${error.message}`
          : "Job rollouts unavailable",
      );
      publishJobsError();
      throw error;
    }
  }, [apiToken, onUnauthorized, publishJobsError]);

  const loadJobRollout = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobRolloutRecord>(
          `/api/v1/job-rollouts/${encodeURIComponent(jobId)}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const updateJobRollout = useCallback(
    async (
      jobId: string,
      action: "pause" | "resume",
      request: UpdateJobRolloutRequest,
    ) => {
      const operationGeneration = jobRolloutMutationGeneration.current + 1;
      jobRolloutMutationGeneration.current = operationGeneration;
      const record = await apiPost<JobRolloutRecord>(
        `/api/v1/job-rollouts/${encodeURIComponent(jobId)}/${action}`,
        apiToken,
        request,
      );
      if (
        currentApiToken.current !== apiToken ||
        jobRolloutMutationGeneration.current !== operationGeneration
      ) {
        return record;
      }
      recordProjectionUpsert(jobRolloutsOverlay.current, record.job_id, record);
      const merged = upsertBoundedProjection(
        jobRolloutsRef.current,
        record,
        (rollout) => rollout.job_id,
        compareCreatedRecords,
        FLEET_DETAIL_LIMIT,
      );
      jobRolloutsRef.current = merged.records;
      setJobRollouts(merged.records);
      if (merged.insertedBeyondBound) {
        setJobRolloutsTruncated(true);
      }
      void onAuditChanged();
      return record;
    },
    [apiToken, onAuditChanged],
  );

  const loadTerminalSessions = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = terminalSessionsLoadGeneration.current + 1;
    terminalSessionsLoadGeneration.current = generation;
    setProjectionError(jobSourceErrors.current, "terminalSessions", null);
    publishJobsError();
    try {
      const records = await apiGet<TerminalSessionRecord[]>(
        `/api/v1/terminal-sessions?limit=${FLEET_DETAIL_LIMIT}`,
        apiToken,
      );
      if (
        terminalSessionsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setTerminalSessions(records);
      setTerminalSessionsTruncated(records.length >= FLEET_DETAIL_LIMIT);
      setProjectionError(jobSourceErrors.current, "terminalSessions", null);
      publishJobsError();
    } catch (error) {
      if (
        terminalSessionsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setTerminalSessions([]);
        setTerminalSessionsTruncated(false);
        setProjectionError(
          jobSourceErrors.current,
          "terminalSessions",
          "Operator login required",
        );
        publishJobsError();
        return;
      }
      setProjectionError(
        jobSourceErrors.current,
        "terminalSessions",
        error instanceof Error
          ? `Terminal sessions: ${error.message}`
          : "Terminal session inventory unavailable",
      );
      publishJobsError();
    }
  }, [apiToken, onUnauthorized, publishJobsError]);

  const loadFileTransfers = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = fileTransfersLoadGeneration.current + 1;
    fileTransfersLoadGeneration.current = generation;
    setProjectionError(jobSourceErrors.current, "fileTransfers", null);
    publishJobsError();
    try {
      const records = await apiGet<FileTransferSessionRecord[]>(
        `/api/v1/file-transfers?limit=${FLEET_DETAIL_LIMIT}`,
        apiToken,
      );
      if (
        fileTransfersLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setFileTransfers(records);
      setFileTransfersTruncated(records.length >= FLEET_DETAIL_LIMIT);
      fileTransfersEvidenceAvailable.current = true;
      publishJobsEvidence();
      setProjectionError(jobSourceErrors.current, "fileTransfers", null);
      publishJobsError();
    } catch (error) {
      if (
        fileTransfersLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setFileTransfers([]);
        setFileTransfersTruncated(false);
        fileTransfersEvidenceAvailable.current = false;
        publishJobsEvidence();
        setProjectionError(
          jobSourceErrors.current,
          "fileTransfers",
          "Operator login required",
        );
      } else {
        fileTransfersEvidenceAvailable.current = false;
        publishJobsEvidence();
        setProjectionError(
          jobSourceErrors.current,
          "fileTransfers",
          error instanceof Error
            ? `File transfer sessions: ${error.message}`
            : "File transfer sessions unavailable",
        );
      }
      publishJobsError();
    }
  }, [apiToken, onUnauthorized, publishJobsError, publishJobsEvidence]);

  const loadHostProcessInventory = useCallback(
    async (clientId: string, limit = 512) => {
      try {
        return await apiGet<HostProcessInventoryRecord>(
          `/api/v1/host-processes/${encodeURIComponent(clientId)}?limit=${Math.max(1, Math.min(512, Math.trunc(limit)))}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadHostServiceInventory = useCallback(
    async (clientId: string, limit = 1024) => {
      try {
        return await apiGet<HostServiceInventoryRecord>(
          `/api/v1/host-services/${encodeURIComponent(clientId)}?limit=${Math.max(1, Math.min(1024, Math.trunc(limit)))}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadHostStorageInventory = useCallback(
    async (clientId: string, limit = 2048) => {
      try {
        return await apiGet<HostStorageInventoryRecord>(
          `/api/v1/host-storage/${encodeURIComponent(clientId)}?limit=${Math.max(1, Math.min(2048, Math.trunc(limit)))}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadHostPackageUpdatePlans = useCallback(async () => {
    try {
      return await apiGet<HostPackageUpdatePlanRecord[]>(
        "/api/v1/os-updates",
        apiToken,
      );
    } catch (error) {
      return rethrowDirectRequestError(error);
    }
  }, [apiToken, rethrowDirectRequestError]);

  const loadHostPackageUpdatePlan = useCallback(
    async (clientId: string) => {
      try {
        return await apiGet<HostPackageUpdatePlanRecord>(
          `/api/v1/os-updates/${encodeURIComponent(clientId)}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadServerJobs = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = serverJobsLoadGeneration.current + 1;
    serverJobsLoadGeneration.current = generation;
    const overlayRevision = serverJobsOverlay.current.revision;
    setServerJobsError(null);
    try {
      const records = await apiGet<ServerJobRecord[]>(
        `/api/v1/server-jobs?limit=${FLEET_DETAIL_LIMIT}`,
        apiToken,
      );
      if (
        serverJobsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      const merged = mergeProjectionRead(
        records,
        serverJobsOverlay.current,
        overlayRevision,
        (record) => record.id,
        compareCreatedRecords,
        FLEET_DETAIL_LIMIT,
      );
      setServerJobs(merged.records);
      setServerJobsError(null);
    } catch (error) {
      if (
        serverJobsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setServerJobs([]);
        setServerJobsError("Operator login required");
        return;
      }
      setServerJobsError(
        error instanceof Error
          ? error.message
          : "Maintenance job inventory unavailable",
      );
    }
  }, [apiToken, onUnauthorized]);

  const loadJobTargets = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobTargetRecord[]>(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/targets`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadExactJobTargetStatuses = useCallback(
    async (items: JobTargetStatusRequestItem[]) => {
      try {
        return await apiPost<JobTargetRecord[]>(
          "/api/v1/job-targets/statuses",
          apiToken,
          { items },
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadJob = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobHistoryRecord>(
          `/api/v1/jobs/${encodeURIComponent(jobId)}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const refreshJobRecord = useCallback(
    (
      jobId: string,
      includeIfMissing: boolean,
    ): Promise<JobHistoryRecord | null> => {
      if (
        currentApiToken.current !== apiToken ||
        (!includeIfMissing && !jobsRef.current.some((job) => job.id === jobId))
      ) {
        return Promise.resolve(null);
      }
      const inFlight = jobRowRefreshInFlight.current.get(jobId);
      if (inFlight) {
        return inFlight;
      }
      const rowGeneration =
        (jobRowRefreshGeneration.current.get(jobId) ?? 0) + 1;
      jobRowRefreshGeneration.current.set(jobId, rowGeneration);
      const request = (async () => {
        try {
          const refreshed = await apiGet<JobHistoryRecord>(
            `/api/v1/jobs/${encodeURIComponent(jobId)}`,
            apiToken,
          );
          if (
            currentApiToken.current !== apiToken ||
            jobRowRefreshGeneration.current.get(jobId) !== rowGeneration
          ) {
            return null;
          }
          if (
            !includeIfMissing &&
            !jobsRef.current.some((job) => job.id === refreshed.id)
          ) {
            return refreshed;
          }
          recordProjectionUpsert(
            jobHistoryOverlay.current,
            refreshed.id,
            refreshed,
          );
          const merged = upsertBoundedProjection(
            jobsRef.current,
            refreshed,
            (job) => job.id,
            compareCreatedRecords,
            HISTORY_DETAIL_LIMIT,
          );
          jobsRef.current = merged.records;
          setJobs(merged.records);
          if (merged.insertedBeyondBound) {
            setJobsTruncated(true);
          }
          return refreshed;
        } catch (error) {
          if (currentApiToken.current === apiToken && isApiUnauthorized(error)) {
            onUnauthorized();
          }
          return null;
        }
      })();
      jobRowRefreshInFlight.current.set(jobId, request);
      void request.finally(() => {
        if (jobRowRefreshInFlight.current.get(jobId) === request) {
          jobRowRefreshInFlight.current.delete(jobId);
        }
      });
      return request;
    },
    [apiToken, onUnauthorized],
  );

  const reconcileJobStatusEvent = useCallback(
    (jobId: string, status: JobStatus): JobHistoryRecord | null => {
      const current = jobsRef.current.find((job) => job.id === jobId);
      if (!current) {
        return null;
      }
      const reconciled = { ...current, status };
      // The overlay revision fences an older aggregate/Home snapshot even
      // when the status value was already visible locally.
      recordProjectionUpsert(
        jobHistoryOverlay.current,
        reconciled.id,
        reconciled,
      );
      const merged = upsertBoundedProjection(
        jobsRef.current,
        reconciled,
        (job) => job.id,
        compareCreatedRecords,
        HISTORY_DETAIL_LIMIT,
      );
      jobsRef.current = merged.records;
      setJobs(merged.records);
      return reconciled;
    },
    [],
  );

  const refreshJobHistoryAfterEvent = useCallback(
    async (
      jobId: string,
      status: JobStatus,
    ): Promise<JobHistoryRecord | null> => {
      if (currentApiToken.current !== apiToken) {
        return null;
      }
      try {
        const records = await loadJobHistory();
        if (currentApiToken.current !== apiToken) {
          return null;
        }
        const observed = records.find((job) => job.id === jobId);
        if (!observed) {
          // A very old, long-running job can finish outside the bounded recent
          // page. Preserve that rare exact classification without restoring
          // per-event point reads for the normal burst path.
          const exact = await refreshJobRecord(jobId, true);
          if (!exact) {
            return jobsRef.current.find((job) => job.id === jobId) ?? null;
          }
          return (
            reconcileJobStatusEvent(jobId, status) ?? { ...exact, status }
          );
        }
        // Reapply the typed event after the authoritative list so an older
        // response cannot regress the terminal status. The server list still
        // supplies exact completion metadata and any previously unseen row.
        return reconcileJobStatusEvent(jobId, status);
      } catch (error) {
        if (isApiUnauthorized(error)) {
          return null;
        }
        return jobsRef.current.find((job) => job.id === jobId) ?? null;
      }
    },
    [
      apiToken,
      loadJobHistory,
      reconcileJobStatusEvent,
      refreshJobRecord,
    ],
  );

  const cancelJob = useCallback(
    async (jobId: string, reason: string) => {
      const response = await apiPost<CancelJobResponse>(
        `/api/v1/jobs/${encodeURIComponent(jobId)}/cancel`,
        apiToken,
        { confirmed: true, reason },
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      void Promise.allSettled([
        refreshJobRecord(response.job_id, true),
        onFleetChanged(),
        onAuditChanged(),
      ]);
      return response;
    },
    [apiToken, onAuditChanged, onFleetChanged, refreshJobRecord],
  );

  const loadJobOutputs = useCallback(
    async (jobId: string) => {
      try {
        const outputs: JobOutputRecord[] = [];
        let cursor: string | null = null;
        do {
          const params = new URLSearchParams({
            limit: "1000",
            include_data: "true",
          });
          if (cursor) {
            params.set("cursor", cursor);
          }
          const page = await apiGet<JobOutputListPageRecord>(
            `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs?${params.toString()}`,
            apiToken,
          );
          outputs.push(...page.items);
          cursor = page.has_more ? page.next_cursor : null;
          if (page.has_more && !cursor) {
            throw new Error("Job output page omitted next cursor");
          }
        } while (cursor);
        return outputs;
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const downloadFileDownloadBundle = useCallback(
    async (jobId: string, clientIds: string[]) => {
      try {
        const params = new URLSearchParams();
        if (clientIds.length > 0) {
          params.set("clients", clientIds.join(","));
        }
        const suffix = params.toString();
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs/download-bundle${suffix ? `?${suffix}` : ""}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const downloadJobOutputArchive = useCallback(
    async (jobId: string, clientIds: string[]) => {
      try {
        const params = new URLSearchParams();
        if (clientIds.length > 0) {
          params.set("clients", clientIds.join(","));
        }
        const suffix = params.toString();
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs/archive${suffix ? `?${suffix}` : ""}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const downloadJobTargetStatuses = useCallback(
    async (jobId: string) => {
      try {
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/targets/download`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadJobOutputComparison = useCallback(
    async (jobId: string, mode: JobOutputCompareMode) => {
      try {
        return await apiGet<JobOutputComparisonRecord>(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/output-comparison?mode=${encodeURIComponent(mode)}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const upsertCommandTemplate = useCallback(
    async (request: UpsertCommandTemplateRequest) => {
      const operationGeneration = commandTemplateMutationGeneration.current + 1;
      commandTemplateMutationGeneration.current = operationGeneration;
      const response = await apiPost<CommandTemplateRecord>(
        "/api/v1/command-templates",
        apiToken,
        request,
      );
      if (
        currentApiToken.current !== apiToken ||
        commandTemplateMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      recordProjectionUpsert(
        commandTemplatesOverlay.current,
        response.id,
        response,
      );
      setProjectionError(jobSourceErrors.current, "commandTemplates", null);
      setCommandTemplates((current) => {
        const merged = upsertBoundedProjection(
          current,
          response,
          (template) => template.id,
          compareCommandTemplates,
          FLEET_DETAIL_LIMIT,
        );
        if (merged.insertedBeyondBound) {
          setCommandTemplatesTruncated(true);
        }
        return merged.records;
      });
      publishJobsError();
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, publishJobsError],
  );

  const deleteCommandTemplate = useCallback(
    async (templateId: string, request: DeleteCommandTemplateRequest) => {
      const operationGeneration = commandTemplateMutationGeneration.current + 1;
      commandTemplateMutationGeneration.current = operationGeneration;
      const response = await apiDelete<CommandTemplateRecord>(
        `/api/v1/command-templates/${encodeURIComponent(templateId)}`,
        apiToken,
        request,
      );
      if (
        currentApiToken.current !== apiToken ||
        commandTemplateMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      recordProjectionDelete(commandTemplatesOverlay.current, response.id);
      setProjectionError(jobSourceErrors.current, "commandTemplates", null);
      setCommandTemplates((current) =>
        current.filter((template) => template.id !== response.id),
      );
      publishJobsError();
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, publishJobsError],
  );

  const downloadJobOutputChunk = useCallback(
    async (jobId: string, clientId: string, seq: number) => {
      try {
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs/${encodeURIComponent(clientId)}/${seq}/download`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const downloadJobOutputStream = useCallback(
    async (
      jobId: string,
      clientId: string,
      stream: "stdout" | "stderr" | "combined",
    ) => {
      try {
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs/${encodeURIComponent(clientId)}/download?stream=${encodeURIComponent(stream)}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const downloadFileDownloadForClient = useCallback(
    async (jobId: string, clientId: string) => {
      try {
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs/${encodeURIComponent(clientId)}/file-download`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const createFileTransferHandoff = useCallback(
    async (clientId: string, sessionId: string) => {
      try {
        const response = await apiPost<FileTransferHandoffRecord>(
          `/api/v1/file-transfers/${encodeURIComponent(clientId)}/${encodeURIComponent(sessionId)}/handoff`,
          apiToken,
          { confirmed: true },
        );
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        await loadFileTransfers();
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, loadFileTransfers, onUnauthorized],
  );

  const downloadFileTransferHandoff = useCallback(
    async (downloadPath: string) => {
      try {
        return await apiGetBlob(downloadPath, apiToken);
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const saveFileTransferHandoff = useCallback(
    async (
      downloadPath: string,
      request: {
        expectedSha256Hex?: string | null;
        expectedSizeBytes?: number | null;
        fileName: string;
        mode: ArtifactDownloadMode;
      },
    ) => {
      try {
        await downloadVerifiedArtifact({
          apiToken,
          path: downloadPath,
          ...request,
        });
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const uploadFileTransferSource = useCallback(
    async (request: UploadFileTransferSourceArtifactRequest) => {
      try {
        const response = await apiPost<FileTransferSourceArtifactRecord>(
          "/api/v1/file-transfer-sources",
          apiToken,
          request,
        );
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        recordProjectionUpsert(
          fileTransferSourcesOverlay.current,
          response.id,
          response,
        );
        setFileTransferSources((current) => {
          const merged = upsertBoundedProjection(
            current,
            response,
            (artifact) => artifact.id,
            compareCreatedRecords,
            FLEET_DETAIL_LIMIT,
          );
          if (merged.insertedBeyondBound) {
            setFileTransferSourcesTruncated(true);
          }
          return merged.records;
        });
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const downloadFileTransferSource = useCallback(
    async (downloadPath: string) => {
      try {
        return await apiGetBlob(downloadPath, apiToken);
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadTerminalReplay = useCallback(
    async (clientId: string, sessionId: string, fromSeq?: number) => {
      try {
        const query = new URLSearchParams({
          include_data: "true",
          limit: "200",
          max_bytes: String(1024 * 1024),
        });
        if (fromSeq !== undefined) {
          query.set("from_seq", String(Math.max(1, Math.trunc(fromSeq))));
        }
        return await apiGet<TerminalReplayRecord>(
          `/api/v1/terminal-sessions/${encodeURIComponent(clientId)}/${encodeURIComponent(sessionId)}/replay?${query}`,
          apiToken,
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const upsertJobApproval = useCallback((record: JobApprovalRecord) => {
    recordProjectionUpsert(jobApprovalsOverlay.current, record.id, record);
    setJobApprovals(
      (current) =>
        upsertBoundedProjection(
          current,
          record,
          (approval) => approval.id,
          compareRequestedRecords,
          FLEET_DETAIL_LIMIT,
        ).records,
    );
  }, []);

  const createJob = useCallback(
    async (request: CreateJobRequest) => {
      const response = await apiPost<CreateJobResponse>(
        "/api/v1/jobs",
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      void Promise.allSettled([
        refreshJobRecord(response.job_id, true),
        onFleetChanged(),
        onAuditChanged(),
      ]);
      return response;
    },
    [apiToken, onAuditChanged, onFleetChanged, refreshJobRecord],
  );

  const createJobApproval = useCallback(
    async (request: CreateJobApprovalRequest) => {
      const operationGeneration = jobApprovalMutationGeneration.current + 1;
      jobApprovalMutationGeneration.current = operationGeneration;
      const response = await apiPost<JobApprovalRecord>(
        "/api/v1/job-approvals",
        apiToken,
        request,
      );
      if (
        currentApiToken.current !== apiToken ||
        jobApprovalMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      upsertJobApproval(response);
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, upsertJobApproval],
  );

  const approveJobApproval = useCallback(
    async (approvalId: string, request: DecideJobApprovalRequest) => {
      const response = await apiPost<JobApprovalDecisionResponse>(
        `/api/v1/job-approvals/${encodeURIComponent(approvalId)}/approve`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      upsertJobApproval(response.approval);
      const changes = [onAuditChanged()];
      if (response.job) {
        changes.push(
          refreshJobRecord(response.job.job_id, true).then(() => undefined),
          onFleetChanged(),
        );
      }
      void Promise.allSettled(changes);
      return response;
    },
    [
      apiToken,
      onAuditChanged,
      onFleetChanged,
      refreshJobRecord,
      upsertJobApproval,
    ],
  );

  const rejectJobApproval = useCallback(
    async (approvalId: string, request: DecideJobApprovalRequest) => {
      const response = await apiPost<JobApprovalDecisionResponse>(
        `/api/v1/job-approvals/${encodeURIComponent(approvalId)}/reject`,
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      upsertJobApproval(response.approval);
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, upsertJobApproval],
  );

  const previewArtifactCleanup = useCallback(
    async (expression: string, domains: string[]) => {
      try {
        return await apiPostPreview<ArtifactCleanupPreviewRecord>(
          "/api/v1/server-jobs/artifact-cleanup/preview",
          apiToken,
          {
            expression,
            domains,
          },
        );
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const createArtifactCleanupJob = useCallback(
    async (expression: string, domains: string[], previewHash: string) => {
      try {
        const response = await apiPost<ServerJobRecord>(
          "/api/v1/server-jobs/artifact-cleanup",
          apiToken,
          {
            expression,
            domains,
            preview_hash: previewHash,
            confirmed: true,
          },
        );
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        recordProjectionUpsert(
          serverJobsOverlay.current,
          response.id,
          response,
        );
        setServerJobs(
          (current) =>
            upsertBoundedProjection(
              current,
              response,
              (job) => job.id,
              compareCreatedRecords,
              FLEET_DETAIL_LIMIT,
            ).records,
        );
        setServerJobsError(null);
        void onAuditChanged();
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onAuditChanged, onUnauthorized],
  );

  const cancelServerJob = useCallback(
    async (jobId: string) => {
      try {
        const response = await apiPost<ServerJobRecord>(
          `/api/v1/server-jobs/${encodeURIComponent(jobId)}/cancel`,
          apiToken,
          { confirmed: true },
        );
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        recordProjectionUpsert(
          serverJobsOverlay.current,
          response.id,
          response,
        );
        setServerJobs(
          (current) =>
            upsertBoundedProjection(
              current,
              response,
              (job) => job.id,
              compareCreatedRecords,
              FLEET_DETAIL_LIMIT,
            ).records,
        );
        setServerJobsError(null);
        void onAuditChanged();
        return response;
      } catch (error) {
        if (currentApiToken.current !== apiToken) {
          throw error;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onAuditChanged, onUnauthorized],
  );

  const createAgentUpdateRelease = useCallback(
    async (request: CreateAgentUpdateReleaseRequest) => {
      const response = await apiPost<AgentUpdateReleaseRecord>(
        "/api/v1/agent-update-releases",
        apiToken,
        request,
      );
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      recordProjectionUpsert(
        agentUpdateReleasesOverlay.current,
        response.id,
        response,
      );
      setAgentUpdateReleases((current) => {
        const merged = upsertBoundedProjection(
          current,
          response,
          (release) => release.id,
          compareCreatedRecords,
          FLEET_DETAIL_LIMIT,
        );
        if (merged.insertedBeyondBound) {
          setAgentUpdateReleasesTruncated(true);
        }
        return merged.records;
      });
      setProjectionError(jobSourceErrors.current, "agentUpdateReleases", null);
      publishJobsError();
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, publishJobsError],
  );

  const clearJobs = useCallback(() => {
    jobsLoadGeneration.current += 1;
    jobsLoadConsumer.current.discardPending();
    jobHistoryLoadGeneration.current += 1;
    jobHistoryLoadConsumer.current.discardPending([]);
    jobApprovalsLoadGeneration.current += 1;
    jobRolloutsLoadGeneration.current += 1;
    agentUpdateReleasesLoadGeneration.current += 1;
    processSupervisorInventoryLoadGeneration.current += 1;
    fileTransfersLoadGeneration.current += 1;
    fileTransferSourcesLoadGeneration.current += 1;
    terminalSessionsLoadGeneration.current += 1;
    serverJobsLoadGeneration.current += 1;
    commandTemplatesLoadGeneration.current += 1;
    commandTemplateMutationGeneration.current += 1;
    jobApprovalMutationGeneration.current += 1;
    jobRolloutMutationGeneration.current += 1;
    currentApiToken.current = "";
    clearProjectionOverlay(jobHistoryOverlay.current);
    clearProjectionOverlay(jobApprovalsOverlay.current);
    clearProjectionOverlay(jobRolloutsOverlay.current);
    clearProjectionOverlay(agentUpdateReleasesOverlay.current);
    clearProjectionOverlay(fileTransferSourcesOverlay.current);
    clearProjectionOverlay(serverJobsOverlay.current);
    clearProjectionOverlay(commandTemplatesOverlay.current);
    jobSourceErrors.current = {};
    jobHistoryEvidenceAvailable.current = false;
    fileTransfersEvidenceAvailable.current = false;
    jobsRef.current = [];
    jobRowRefreshGeneration.current.clear();
    jobRowRefreshInFlight.current.clear();
    jobRolloutsRef.current = [];
    setJobs([]);
    setJobApprovals([]);
    setJobRollouts([]);
    setAgentUpdateReleases([]);
    setProcessSupervisorInventory([]);
    setFileTransfers([]);
    setFileTransferSources([]);
    setTerminalSessions([]);
    setServerJobs([]);
    setCommandTemplates([]);
    setJobsTruncated(false);
    setJobRolloutsTruncated(false);
    setAgentUpdateReleasesTruncated(false);
    setProcessSupervisorInventoryTruncated(false);
    setFileTransfersTruncated(false);
    setFileTransferSourcesTruncated(false);
    setTerminalSessionsTruncated(false);
    setCommandTemplatesTruncated(false);
    setJobsError(null);
    setServerJobsError(null);
    setJobsLoading(false);
    setJobsEvidenceAvailable(false);
  }, []);

  return {
    clearJobs,
    beginHomeJobsHydration,
    createAgentUpdateRelease,
    createJob,
    createJobApproval,
    approveJobApproval,
    rejectJobApproval,
    commandTemplates,
    commandTemplatesTruncated,
    agentUpdateReleases,
    agentUpdateReleasesTruncated,
    fileTransfers,
    fileTransfersTruncated,
    fileTransferSources,
    fileTransferSourcesTruncated,
    jobApprovals,
    jobRollouts,
    jobRolloutsTruncated,
    jobs,
    hydrateHomeJobs,
    jobsTruncated,
    jobsError,
    jobsEvidenceAvailable,
    jobsLoading,
    processSupervisorInventory,
    processSupervisorInventoryTruncated,
    serverJobs,
    serverJobsError,
    terminalSessions,
    terminalSessionsTruncated,
    cancelServerJob,
    cancelJob,
    loadJob,
    reconcileJobStatusEvent,
    refreshJobHistoryAfterEvent,
    loadJobRollout,
    loadJobRollouts,
    createArtifactCleanupJob,
    createFileTransferHandoff,
    previewArtifactCleanup,
    uploadFileTransferSource,
    downloadJobOutputChunk,
    downloadJobOutputStream,
    downloadFileDownloadForClient,
    downloadJobOutputArchive,
    downloadJobTargetStatuses,
    downloadFileTransferHandoff,
    downloadFileTransferSource,
    saveFileTransferHandoff,
    loadJobOutputs,
    downloadFileDownloadBundle,
    loadJobOutputComparison,
    loadExactJobTargetStatuses,
    loadJobTargets,
    loadFileTransfers,
    loadHostProcessInventory,
    loadHostPackageUpdatePlan,
    loadHostPackageUpdatePlans,
    loadHostServiceInventory,
    loadHostStorageInventory,
    loadJobs,
    loadAgentUpdateReleases,
    loadServerJobs,
    loadTerminalReplay,
    loadTerminalSessions,
    updateJobRollout,
    deleteCommandTemplate,
    upsertCommandTemplate,
  };
}

type ProjectionOverlay<T> = {
  revision: number;
  records: Map<string, { record: T; revision: number }>;
  tombstones: Map<string, number>;
};

function createProjectionOverlay<T>(): ProjectionOverlay<T> {
  return {
    revision: 0,
    records: new Map(),
    tombstones: new Map(),
  };
}

function recordProjectionUpsert<T>(
  overlay: ProjectionOverlay<T>,
  key: string,
  record: T,
): void {
  const revision = ++overlay.revision;
  overlay.records.set(key, { record, revision });
  overlay.tombstones.delete(key);
}

function recordProjectionDelete<T>(
  overlay: ProjectionOverlay<T>,
  key: string,
): void {
  const revision = ++overlay.revision;
  overlay.records.delete(key);
  overlay.tombstones.set(key, revision);
}

function clearProjectionOverlay<T>(overlay: ProjectionOverlay<T>): void {
  overlay.revision += 1;
  overlay.records.clear();
  overlay.tombstones.clear();
}

function mergeProjectionRead<T>(
  records: T[],
  overlay: ProjectionOverlay<T>,
  readRevision: number,
  keyOf: (record: T) => string,
  compare: (left: T, right: T) => number,
  limit: number,
): { records: T[]; truncated: boolean } {
  const byKey = new Map(records.map((record) => [keyOf(record), record]));
  for (const [key, revision] of overlay.tombstones) {
    if (revision > readRevision) {
      byKey.delete(key);
    }
  }
  for (const [key, entry] of overlay.records) {
    if (entry.revision > readRevision) {
      byKey.set(key, entry.record);
    }
  }
  const merged = [...byKey.values()].sort(compare);
  for (const [key, entry] of overlay.records) {
    if (entry.revision <= readRevision) {
      overlay.records.delete(key);
    }
  }
  for (const [key, revision] of overlay.tombstones) {
    if (revision <= readRevision) {
      overlay.tombstones.delete(key);
    }
  }
  return {
    records: merged.slice(0, limit),
    truncated: records.length >= limit || merged.length > limit,
  };
}

function upsertBoundedProjection<T>(
  records: T[],
  record: T,
  keyOf: (item: T) => string,
  compare: (left: T, right: T) => number,
  limit: number,
): { records: T[]; insertedBeyondBound: boolean } {
  const key = keyOf(record);
  const existed = records.some((item) => keyOf(item) === key);
  const merged = [record, ...records.filter((item) => keyOf(item) !== key)]
    .sort(compare)
    .slice(0, limit);
  return {
    records: merged,
    insertedBeyondBound: !existed && records.length >= limit,
  };
}

function setProjectionError(
  errors: Partial<Record<JobErrorSource, string>>,
  source: JobErrorSource,
  error: string | null,
): void {
  if (error) {
    errors[source] = error;
  } else {
    delete errors[source];
  }
}

function compareCreatedRecords(
  left: { created_at: string; id?: string; job_id?: string },
  right: { created_at: string; id?: string; job_id?: string },
): number {
  return (
    right.created_at.localeCompare(left.created_at) ||
    (right.id ?? right.job_id ?? "").localeCompare(left.id ?? left.job_id ?? "")
  );
}

function compareRequestedRecords(
  left: JobApprovalRecord,
  right: JobApprovalRecord,
): number {
  return (
    right.requested_at.localeCompare(left.requested_at) ||
    right.id.localeCompare(left.id)
  );
}

function compareCommandTemplates(
  left: CommandTemplateRecord,
  right: CommandTemplateRecord,
): number {
  if (left.built_in !== right.built_in) {
    return left.built_in ? -1 : 1;
  }
  if (left.built_in) {
    return left.name.localeCompare(right.name);
  }
  return (
    right.updated_at.localeCompare(left.updated_at) ||
    left.name.localeCompare(right.name)
  );
}

function settledSourceFailure(
  label: string,
  result: PromiseSettledResult<unknown>,
): string | null {
  if (result.status === "fulfilled") {
    return null;
  }
  return result.reason instanceof Error
    ? `${label}: ${result.reason.message}`
    : `${label} unavailable`;
}
