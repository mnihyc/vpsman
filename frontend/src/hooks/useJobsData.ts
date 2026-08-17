import { useCallback, useRef, useState } from "react";
import { apiDelete, apiGet, apiGetBlob, apiPost, apiPostPreview, buildListPath, isApiUnauthorized } from "../api";
import { downloadVerifiedArtifact, type ArtifactDownloadMode } from "../artifactDownload";
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
  JobRolloutRecord,
  JobApprovalDecisionResponse,
  JobApprovalRecord,
  JobOutputListPageRecord,
  JobOutputCompareMode,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
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

const JOB_SOURCE_LABELS = [
  "job history",
  "job approvals",
  "process supervisor inventory",
  "file transfer sessions",
  "file transfer sources",
  "command templates",
] as const;

type HomeJobsHydrationFence = {
  inventory: number;
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
  const [agentUpdateReleases, setAgentUpdateReleases] = useState<AgentUpdateReleaseRecord[]>([]);
  const [processSupervisorInventory, setProcessSupervisorInventory] = useState<ProcessSupervisorInventoryRecord[]>([]);
  const [fileTransfers, setFileTransfers] = useState<FileTransferSessionRecord[]>([]);
  const [fileTransferSources, setFileTransferSources] = useState<FileTransferSourceArtifactRecord[]>([]);
  const [terminalSessions, setTerminalSessions] = useState<TerminalSessionRecord[]>([]);
  const [serverJobs, setServerJobs] = useState<ServerJobRecord[]>([]);
  const [commandTemplates, setCommandTemplates] = useState<CommandTemplateRecord[]>([]);
  const [jobsTruncated, setJobsTruncated] = useState(false);
  const [jobRolloutsTruncated, setJobRolloutsTruncated] = useState(false);
  const [agentUpdateReleasesTruncated, setAgentUpdateReleasesTruncated] = useState(false);
  const [processSupervisorInventoryTruncated, setProcessSupervisorInventoryTruncated] = useState(false);
  const [fileTransfersTruncated, setFileTransfersTruncated] = useState(false);
  const [fileTransferSourcesTruncated, setFileTransferSourcesTruncated] = useState(false);
  const [terminalSessionsTruncated, setTerminalSessionsTruncated] = useState(false);
  const [commandTemplatesTruncated, setCommandTemplatesTruncated] = useState(false);
  const [jobsError, setJobsError] = useState<string | null>(null);
  const [serverJobsError, setServerJobsError] = useState<string | null>(null);
  const [jobsLoading, setJobsLoading] = useState(false);
  const [jobsEvidenceAvailable, setJobsEvidenceAvailable] = useState(false);
  const jobsRef = useRef<JobHistoryRecord[]>([]);
  const jobRolloutsRef = useRef<JobRolloutRecord[]>([]);
  const jobsLoadGeneration = useRef(0);
  const jobRowRefreshGeneration = useRef(new Map<string, number>());
  const jobRolloutsLoadGeneration = useRef(0);
  const agentUpdateReleasesLoadGeneration = useRef(0);
  const terminalSessionsLoadGeneration = useRef(0);
  const serverJobsLoadGeneration = useRef(0);
  const commandTemplatesLoadGeneration = useRef(0);
  const commandTemplateMutationGeneration = useRef(0);
  const jobApprovalMutationGeneration = useRef(0);
  const jobRolloutMutationGeneration = useRef(0);
  const jobsInventoryError = useRef<string | null>(null);
  const jobRolloutsError = useRef<string | null>(null);
  const agentUpdateReleasesError = useRef<string | null>(null);
  const terminalSessionsError = useRef<string | null>(null);
  const commandTemplatesError = useRef<string | null>(null);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const publishJobsError = useCallback(() => {
    const errors = [
      jobsInventoryError.current,
      jobRolloutsError.current,
      agentUpdateReleasesError.current,
      terminalSessionsError.current,
      commandTemplatesError.current,
    ].filter((message): message is string => Boolean(message));
    setJobsError(errors.length > 0 ? errors.join("; ") : null);
  }, []);

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

  const beginHomeJobsHydration = useCallback(
    (): HomeJobsHydrationFence => {
      setJobsLoading(true);
      return {
        inventory: ++jobsLoadGeneration.current,
        terminal: ++terminalSessionsLoadGeneration.current,
      };
    },
    [],
  );

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
      if (fence.inventory === jobsLoadGeneration.current) {
        if (snapshotSourceAvailable(jobSource)) {
          jobsRef.current = jobSource.data;
          setJobs(jobSource.data);
          setJobsTruncated(jobSource.data.length >= HISTORY_DETAIL_LIMIT);
        }
        if (snapshotSourceAvailable(fileTransferSource)) {
          setFileTransfers(fileTransferSource.data);
          setFileTransfersTruncated(
            fileTransferSource.data.length >= FLEET_DETAIL_LIMIT,
          );
        }
        setJobsEvidenceAvailable(
          snapshotSourceAvailable(jobSource) &&
            snapshotSourceAvailable(fileTransferSource),
        );
        const inventoryFailures = [
          snapshotSourceError("Job history", jobSource),
          snapshotSourceError("File transfer sessions", fileTransferSource),
        ].filter((message): message is string => message !== null);
        jobsInventoryError.current =
          inventoryFailures.length > 0
            ? `Some job sources are unavailable: ${inventoryFailures.join("; ")}`
            : null;
        setJobsLoading(false);
      }
      if (fence.terminal === terminalSessionsLoadGeneration.current) {
        if (snapshotSourceAvailable(terminalSessionSource)) {
          setTerminalSessions(terminalSessionSource.data);
          setTerminalSessionsTruncated(
            terminalSessionSource.data.length >= FLEET_DETAIL_LIMIT,
          );
        }
        terminalSessionsError.current = snapshotSourceError(
          "Terminal sessions",
          terminalSessionSource,
        );
      }
      publishJobsError();
    },
    [apiToken, publishJobsError],
  );

  const loadJobs = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = jobsLoadGeneration.current + 1;
    jobsLoadGeneration.current = generation;
    const rolloutsGeneration = jobRolloutsLoadGeneration.current + 1;
    jobRolloutsLoadGeneration.current = rolloutsGeneration;
    const releasesGeneration =
      agentUpdateReleasesLoadGeneration.current + 1;
    agentUpdateReleasesLoadGeneration.current = releasesGeneration;
    const terminalGeneration = terminalSessionsLoadGeneration.current + 1;
    terminalSessionsLoadGeneration.current = terminalGeneration;
    const serverGeneration = serverJobsLoadGeneration.current + 1;
    serverJobsLoadGeneration.current = serverGeneration;
    const commandTemplatesGeneration =
      commandTemplatesLoadGeneration.current + 1;
    commandTemplatesLoadGeneration.current = commandTemplatesGeneration;
    jobsInventoryError.current = null;
    jobRolloutsError.current = null;
    agentUpdateReleasesError.current = null;
    terminalSessionsError.current = null;
    commandTemplatesError.current = null;
    setJobsLoading(true);
    setJobsError(null);
    setServerJobsError(null);
    try {
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
        apiGet<JobHistoryRecord[]>(buildListPath("/api/v1/jobs", { limit: HISTORY_DETAIL_LIMIT, sort: "created_at", dir: "desc" }), apiToken),
        apiGet<JobApprovalRecord[]>(buildListPath("/api/v1/job-approvals", { limit: FLEET_DETAIL_LIMIT, sort: "requested_at", dir: "desc" }), apiToken),
        apiGet<JobRolloutRecord[]>(`/api/v1/job-rollouts?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<AgentUpdateReleaseRecord[]>(`/api/v1/agent-update-releases?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<ProcessSupervisorInventoryRecord[]>(`/api/v1/process-supervisor/inventory?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<FileTransferSessionRecord[]>(`/api/v1/file-transfers?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<FileTransferSourceArtifactRecord[]>(`/api/v1/file-transfer-sources?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<TerminalSessionRecord[]>(`/api/v1/terminal-sessions?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<ServerJobRecord[]>(`/api/v1/server-jobs?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
        apiGet<CommandTemplateRecord[]>(`/api/v1/command-templates?limit=${FLEET_DETAIL_LIMIT}`, apiToken),
      ]);
      if (
        jobsLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
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
        (result) => result.status === "rejected" && isApiUnauthorized(result.reason),
      );
      if (unauthorized) {
        onUnauthorized();
        setJobsEvidenceAvailable(false);
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
        jobsInventoryError.current = "Operator login required";
        jobRolloutsError.current = null;
        agentUpdateReleasesError.current = null;
        terminalSessionsError.current = null;
        commandTemplatesError.current = null;
        setJobsError("Operator login required");
        setServerJobsError("Operator login required");
        return;
      }
      if (jobsResult.status === "fulfilled") {
        jobsRef.current = jobsResult.value;
        setJobs(jobsResult.value);
        setJobsTruncated(jobsResult.value.length >= HISTORY_DETAIL_LIMIT);
      }
      setJobsEvidenceAvailable(
        jobsResult.status === "fulfilled" &&
          fileTransfersResult.status === "fulfilled",
      );
      if (jobApprovalsResult.status === "fulfilled") setJobApprovals(jobApprovalsResult.value);
      if (jobRolloutsLoadGeneration.current === rolloutsGeneration) {
        if (jobRolloutsResult.status === "fulfilled") {
          jobRolloutsRef.current = jobRolloutsResult.value;
          setJobRollouts(jobRolloutsResult.value);
          setJobRolloutsTruncated(
            jobRolloutsResult.value.length >= FLEET_DETAIL_LIMIT,
          );
        }
        jobRolloutsError.current = settledSourceFailure(
          "Job rollouts",
          jobRolloutsResult,
        );
      }
      if (agentUpdateReleasesLoadGeneration.current === releasesGeneration) {
        if (releasesResult.status === "fulfilled") {
          setAgentUpdateReleases(releasesResult.value);
          setAgentUpdateReleasesTruncated(
            releasesResult.value.length >= FLEET_DETAIL_LIMIT,
          );
        }
        agentUpdateReleasesError.current = settledSourceFailure(
          "Agent update releases",
          releasesResult,
        );
      }
      if (processSupervisorInventoryResult.status === "fulfilled") {
        setProcessSupervisorInventory(processSupervisorInventoryResult.value);
        setProcessSupervisorInventoryTruncated(
          processSupervisorInventoryResult.value.length >= FLEET_DETAIL_LIMIT,
        );
      }
      if (fileTransfersResult.status === "fulfilled") {
        setFileTransfers(fileTransfersResult.value);
        setFileTransfersTruncated(fileTransfersResult.value.length >= FLEET_DETAIL_LIMIT);
      }
      if (fileTransferSourcesResult.status === "fulfilled") {
        setFileTransferSources(fileTransferSourcesResult.value);
        setFileTransferSourcesTruncated(
          fileTransferSourcesResult.value.length >= FLEET_DETAIL_LIMIT,
        );
      }
      if (terminalSessionsLoadGeneration.current === terminalGeneration) {
        if (terminalSessionsResult.status === "fulfilled") {
          setTerminalSessions(terminalSessionsResult.value);
          setTerminalSessionsTruncated(
            terminalSessionsResult.value.length >= FLEET_DETAIL_LIMIT,
          );
        }
        terminalSessionsError.current = settledSourceFailure(
          "Terminal sessions",
          terminalSessionsResult,
        );
      }
      if (serverJobsLoadGeneration.current === serverGeneration) {
        if (serverJobsResult.status === "fulfilled") {
          setServerJobs(serverJobsResult.value);
        } else {
          setServerJobs([]);
          setServerJobsError(
            serverJobsResult.reason instanceof Error
              ? `Maintenance jobs: ${serverJobsResult.reason.message}`
              : "Maintenance job inventory unavailable",
          );
        }
      }
      if (
        commandTemplatesLoadGeneration.current ===
        commandTemplatesGeneration
      ) {
        if (commandTemplatesResult.status === "fulfilled") {
          setCommandTemplates(
            sortCommandTemplates(commandTemplatesResult.value),
          );
          setCommandTemplatesTruncated(
            commandTemplatesResult.value.length >= FLEET_DETAIL_LIMIT,
          );
        }
        commandTemplatesError.current = settledSourceFailure(
          "Command templates",
          commandTemplatesResult,
        );
      }
      jobsInventoryError.current = unavailableSourceSummary(
        "Some job sources are unavailable",
        [
          jobsResult,
          jobApprovalsResult,
          processSupervisorInventoryResult,
          fileTransfersResult,
          fileTransferSourcesResult,
        ],
        JOB_SOURCE_LABELS.slice(0, 5),
      );
      publishJobsError();
    } finally {
      if (
        jobsLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setJobsLoading(false);
      }
    }
  }, [apiToken, onUnauthorized, publishJobsError]);

  const loadAgentUpdateReleases = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = agentUpdateReleasesLoadGeneration.current + 1;
    agentUpdateReleasesLoadGeneration.current = generation;
    agentUpdateReleasesError.current = null;
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
      setAgentUpdateReleases(records);
      setAgentUpdateReleasesTruncated(records.length >= FLEET_DETAIL_LIMIT);
      agentUpdateReleasesError.current = null;
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
        agentUpdateReleasesError.current = "Operator login required";
        publishJobsError();
        return;
      }
      agentUpdateReleasesError.current =
        error instanceof Error
          ? `Agent update releases: ${error.message}`
          : "Agent update releases unavailable";
      publishJobsError();
    }
  }, [apiToken, onUnauthorized, publishJobsError]);

  const loadJobRollouts = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return jobRolloutsRef.current;
    }
    const generation = jobRolloutsLoadGeneration.current + 1;
    jobRolloutsLoadGeneration.current = generation;
    jobRolloutsError.current = null;
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
      jobRolloutsRef.current = records;
      setJobRollouts(records);
      setJobRolloutsTruncated(records.length >= FLEET_DETAIL_LIMIT);
      jobRolloutsError.current = null;
      publishJobsError();
      return records;
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
        jobRolloutsError.current = "Operator login required";
        publishJobsError();
        throw new Error("Operator login required");
      }
      jobRolloutsError.current =
        error instanceof Error
          ? `Job rollouts: ${error.message}`
          : "Job rollouts unavailable";
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
      jobRolloutsLoadGeneration.current += 1;
      const nextRollouts = [
        record,
        ...jobRolloutsRef.current.filter(
          (rollout) => rollout.job_id !== record.job_id,
        ),
      ];
      jobRolloutsRef.current = nextRollouts;
      setJobRollouts(nextRollouts);
      void onAuditChanged();
      return record;
    },
    [apiToken, onAuditChanged],
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
      void Promise.allSettled([loadJobs(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged],
  );

  const loadTerminalSessions = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = terminalSessionsLoadGeneration.current + 1;
    terminalSessionsLoadGeneration.current = generation;
    terminalSessionsError.current = null;
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
      terminalSessionsError.current = null;
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
        terminalSessionsError.current = "Operator login required";
        publishJobsError();
        return;
      }
      terminalSessionsError.current =
        error instanceof Error
          ? `Terminal sessions: ${error.message}`
          : "Terminal session inventory unavailable";
      publishJobsError();
    }
  }, [apiToken, onUnauthorized, publishJobsError]);

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
      setServerJobs(records);
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
      setServerJobs([]);
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
        return await apiGet<JobTargetRecord[]>(`/api/v1/jobs/${encodeURIComponent(jobId)}/targets`, apiToken);
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const loadJob = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobHistoryRecord>(`/api/v1/jobs/${encodeURIComponent(jobId)}`, apiToken);
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const refreshLoadedJob = useCallback(
    async (jobId: string) => {
      if (
        currentApiToken.current !== apiToken ||
        !jobsRef.current.some((job) => job.id === jobId)
      ) {
        return;
      }
      const listGeneration = jobsLoadGeneration.current;
      const rowGeneration =
        (jobRowRefreshGeneration.current.get(jobId) ?? 0) + 1;
      jobRowRefreshGeneration.current.set(jobId, rowGeneration);
      try {
        const refreshed = await apiGet<JobHistoryRecord>(
          `/api/v1/jobs/${encodeURIComponent(jobId)}`,
          apiToken,
        );
        if (
          currentApiToken.current !== apiToken ||
          jobsLoadGeneration.current !== listGeneration ||
          jobRowRefreshGeneration.current.get(jobId) !== rowGeneration
        ) {
          return;
        }
        setJobs((current) => {
          const index = current.findIndex((job) => job.id === jobId);
          if (index < 0) {
            return current;
          }
          const next = [...current];
          next[index] = refreshed;
          jobsRef.current = next;
          return next;
        });
      } catch (error) {
        if (
          currentApiToken.current === apiToken &&
          isApiUnauthorized(error)
        ) {
          onUnauthorized();
        }
      }
    },
    [apiToken, onUnauthorized],
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
      const operationGeneration =
        commandTemplateMutationGeneration.current + 1;
      commandTemplateMutationGeneration.current = operationGeneration;
      const response = await apiPost<CommandTemplateRecord>("/api/v1/command-templates", apiToken, request);
      if (
        currentApiToken.current !== apiToken ||
        commandTemplateMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      commandTemplatesLoadGeneration.current += 1;
      commandTemplatesError.current = null;
      setCommandTemplates((current) => {
        const withoutTemplate = current.filter((template) => template.id !== response.id);
        return sortCommandTemplates([response, ...withoutTemplate]);
      });
      publishJobsError();
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, publishJobsError],
  );

  const deleteCommandTemplate = useCallback(
    async (templateId: string, request: DeleteCommandTemplateRequest) => {
      const operationGeneration =
        commandTemplateMutationGeneration.current + 1;
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
      commandTemplatesLoadGeneration.current += 1;
      commandTemplatesError.current = null;
      setCommandTemplates((current) => current.filter((template) => template.id !== response.id));
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
    async (jobId: string, clientId: string, stream: "stdout" | "stderr" | "combined") => {
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
        await loadJobs();
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
    [apiToken, loadJobs, onUnauthorized],
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
        const response = await apiPost<FileTransferSourceArtifactRecord>("/api/v1/file-transfer-sources", apiToken, request);
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        await loadJobs();
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
    [apiToken, loadJobs, onUnauthorized],
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

  const createJob = useCallback(
    async (request: CreateJobRequest) => {
      const response = await apiPost<CreateJobResponse>("/api/v1/jobs", apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      void Promise.allSettled([loadJobs(), onFleetChanged(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged, onFleetChanged],
  );

  const createJobApproval = useCallback(
    async (request: CreateJobApprovalRequest) => {
      const operationGeneration = jobApprovalMutationGeneration.current + 1;
      jobApprovalMutationGeneration.current = operationGeneration;
      const response = await apiPost<JobApprovalRecord>("/api/v1/job-approvals", apiToken, request);
      if (
        currentApiToken.current !== apiToken ||
        jobApprovalMutationGeneration.current !== operationGeneration
      ) {
        return response;
      }
      setJobApprovals((current) => [
        response,
        ...current.filter((approval) => approval.id !== response.id),
      ]);
      void Promise.allSettled([loadJobs(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged],
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
      void Promise.allSettled([loadJobs(), onFleetChanged(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged, onFleetChanged],
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
      void Promise.allSettled([loadJobs(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged],
  );

  const previewArtifactCleanup = useCallback(
    async (expression: string, domains: string[]) => {
      try {
        return await apiPostPreview<ArtifactCleanupPreviewRecord>("/api/v1/server-jobs/artifact-cleanup/preview", apiToken, {
          expression,
          domains,
        });
      } catch (error) {
        return rethrowDirectRequestError(error);
      }
    },
    [apiToken, rethrowDirectRequestError],
  );

  const createArtifactCleanupJob = useCallback(
    async (expression: string, domains: string[], previewHash: string) => {
      try {
        const response = await apiPost<ServerJobRecord>("/api/v1/server-jobs/artifact-cleanup", apiToken, {
          expression,
          domains,
          preview_hash: previewHash,
          confirmed: true,
        });
        if (currentApiToken.current !== apiToken) {
          return response;
        }
        await loadServerJobs();
        if (currentApiToken.current !== apiToken) {
          return response;
        }
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
    [apiToken, loadServerJobs, onAuditChanged, onUnauthorized],
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
        await loadServerJobs();
        if (currentApiToken.current !== apiToken) {
          return response;
        }
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
    [apiToken, loadServerJobs, onAuditChanged, onUnauthorized],
  );

  const createAgentUpdateRelease = useCallback(
    async (request: CreateAgentUpdateReleaseRequest) => {
      const response = await apiPost<AgentUpdateReleaseRecord>("/api/v1/agent-update-releases", apiToken, request);
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await loadAgentUpdateReleases();
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateReleases, onAuditChanged],
  );

  const clearJobs = useCallback(() => {
    jobsLoadGeneration.current += 1;
    jobRolloutsLoadGeneration.current += 1;
    agentUpdateReleasesLoadGeneration.current += 1;
    terminalSessionsLoadGeneration.current += 1;
    serverJobsLoadGeneration.current += 1;
    commandTemplatesLoadGeneration.current += 1;
    commandTemplateMutationGeneration.current += 1;
    jobApprovalMutationGeneration.current += 1;
    jobRolloutMutationGeneration.current += 1;
    currentApiToken.current = "";
    jobsInventoryError.current = null;
    jobRolloutsError.current = null;
    agentUpdateReleasesError.current = null;
    terminalSessionsError.current = null;
    commandTemplatesError.current = null;
    jobsRef.current = [];
    jobRowRefreshGeneration.current.clear();
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
    refreshLoadedJob,
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
    loadJobTargets,
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

function sortCommandTemplates(templates: CommandTemplateRecord[]): CommandTemplateRecord[] {
  return [...templates].sort((left, right) => {
    if (left.built_in !== right.built_in) {
      return left.built_in ? -1 : 1;
    }
    if (left.built_in) {
      return left.name.localeCompare(right.name);
    }
    return right.updated_at.localeCompare(left.updated_at) || left.name.localeCompare(right.name);
  });
}

function unavailableSourceSummary(
  prefix: string,
  results: readonly PromiseSettledResult<unknown>[],
  labels: readonly string[],
): string | null {
  const failedLabels = results.flatMap((result, index) =>
    result.status === "rejected" ? [labels[index]] : [],
  );
  return failedLabels.length > 0
    ? `${prefix}: ${failedLabels.join(", ")}`
    : null;
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
