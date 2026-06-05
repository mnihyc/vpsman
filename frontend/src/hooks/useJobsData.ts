import { useCallback, useState } from "react";
import { apiGet, apiGetBlob, apiPost, apiPostBinary, isApiUnauthorized } from "../api";
import { downloadVerifiedArtifact, type ArtifactDownloadMode } from "../artifactDownload";
import type {
  AgentUpdateActivationDelegationRecord,
  AgentUpdateActivationDelegationRequest,
  AgentUpdateReleaseRecord,
  AgentUpdateRollbackDelegationRecord,
  AgentUpdateRollbackDelegationRequest,
  AgentUpdateRolloutControlRequest,
  AgentUpdateRolloutPolicyRecord,
  AgentUpdateRolloutRecord,
  CancelJobRequest,
  CancelJobResponse,
  CommandTemplateRecord,
  CreateAgentUpdateRolloutPolicyRequest,
  CreateAgentUpdateReleaseRequest,
  CreateHostedAgentUpdateReleaseRequest,
  CreateJobRequest,
  CreateJobResponse,
  DispatchScheduledJobRequest,
  JobHistoryRecord,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
  ProcessSupervisorInventoryRecord,
  StreamedAgentUpdateArtifactRecord,
  UploadAgentUpdateArtifactRequest,
  UpsertCommandTemplateRequest,
} from "../types";
import type {
  FileTransferHandoffRecord,
  FileTransferSessionRecord,
  FileTransferSourceArtifactRecord,
  UploadFileTransferSourceArtifactRequest,
} from "../typesFileTransfer";
import type { TerminalReplayRecord, TerminalSessionRecord } from "../typesTerminal";

export function useJobsData(
  apiToken: string,
  onUnauthorized: () => void,
  onFleetChanged: () => Promise<void>,
  onAuditChanged: () => Promise<void>,
) {
  const [jobs, setJobs] = useState<JobHistoryRecord[]>([]);
  const [agentUpdateRollouts, setAgentUpdateRollouts] = useState<AgentUpdateRolloutRecord[]>([]);
  const [agentUpdateRolloutPolicies, setAgentUpdateRolloutPolicies] = useState<AgentUpdateRolloutPolicyRecord[]>([]);
  const [agentUpdateReleases, setAgentUpdateReleases] = useState<AgentUpdateReleaseRecord[]>([]);
  const [processSupervisorInventory, setProcessSupervisorInventory] = useState<ProcessSupervisorInventoryRecord[]>([]);
  const [fileTransfers, setFileTransfers] = useState<FileTransferSessionRecord[]>([]);
  const [fileTransferSources, setFileTransferSources] = useState<FileTransferSourceArtifactRecord[]>([]);
  const [terminalSessions, setTerminalSessions] = useState<TerminalSessionRecord[]>([]);
  const [commandTemplates, setCommandTemplates] = useState<CommandTemplateRecord[]>([]);
  const [jobsError, setJobsError] = useState<string | null>(null);
  const [jobsLoading, setJobsLoading] = useState(false);

  const loadJobs = useCallback(async () => {
    setJobsLoading(true);
    setJobsError(null);
    try {
      const [
        jobsResult,
        rolloutsResult,
        rolloutPoliciesResult,
        releasesResult,
        processSupervisorInventoryResult,
        fileTransfersResult,
        fileTransferSourcesResult,
        terminalSessionsResult,
        commandTemplatesResult,
      ] = await Promise.allSettled([
        apiGet<JobHistoryRecord[]>("/api/v1/jobs?limit=1000", apiToken),
        apiGet<AgentUpdateRolloutRecord[]>("/api/v1/agent-update-rollouts?limit=200", apiToken),
        apiGet<AgentUpdateRolloutPolicyRecord[]>("/api/v1/agent-update-rollout-policies?limit=200", apiToken),
        apiGet<AgentUpdateReleaseRecord[]>("/api/v1/agent-update-releases?limit=200", apiToken),
        apiGet<ProcessSupervisorInventoryRecord[]>("/api/v1/process-supervisor/inventory?limit=200", apiToken),
        apiGet<FileTransferSessionRecord[]>("/api/v1/file-transfers?limit=200", apiToken),
        apiGet<FileTransferSourceArtifactRecord[]>("/api/v1/file-transfer-sources?limit=200", apiToken),
        apiGet<TerminalSessionRecord[]>("/api/v1/terminal-sessions?limit=200", apiToken),
        apiGet<CommandTemplateRecord[]>("/api/v1/command-templates?limit=1000", apiToken),
      ]);
      const settledResults = [
        jobsResult,
        rolloutsResult,
        rolloutPoliciesResult,
        releasesResult,
        processSupervisorInventoryResult,
        fileTransfersResult,
        fileTransferSourcesResult,
        terminalSessionsResult,
        commandTemplatesResult,
      ];
      const unauthorized = settledResults.some(
        (result) => result.status === "rejected" && isApiUnauthorized(result.reason),
      );
      if (unauthorized) {
        onUnauthorized();
        setJobs([]);
        setAgentUpdateRollouts([]);
        setAgentUpdateRolloutPolicies([]);
        setAgentUpdateReleases([]);
        setProcessSupervisorInventory([]);
        setFileTransfers([]);
        setFileTransferSources([]);
        setTerminalSessions([]);
        setCommandTemplates([]);
        setJobsError("Operator login required");
        return;
      }
      if (jobsResult.status === "fulfilled") setJobs(jobsResult.value);
      if (rolloutsResult.status === "fulfilled") setAgentUpdateRollouts(rolloutsResult.value);
      if (rolloutPoliciesResult.status === "fulfilled") {
        setAgentUpdateRolloutPolicies(rolloutPoliciesResult.value);
      }
      if (releasesResult.status === "fulfilled") setAgentUpdateReleases(releasesResult.value);
      if (processSupervisorInventoryResult.status === "fulfilled") {
        setProcessSupervisorInventory(processSupervisorInventoryResult.value);
      }
      if (fileTransfersResult.status === "fulfilled") setFileTransfers(fileTransfersResult.value);
      if (fileTransferSourcesResult.status === "fulfilled") setFileTransferSources(fileTransferSourcesResult.value);
      if (terminalSessionsResult.status === "fulfilled") setTerminalSessions(terminalSessionsResult.value);
      if (commandTemplatesResult.status === "fulfilled") setCommandTemplates(commandTemplatesResult.value);
      const firstFailure = settledResults.find((result): result is PromiseRejectedResult => result.status === "rejected");
      if (firstFailure) {
        setJobsError(firstFailure.reason instanceof Error ? firstFailure.reason.message : "Job history partially unavailable");
      }
    } finally {
      setJobsLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const loadAgentUpdateRollouts = useCallback(async () => {
    try {
      const [nextRollouts, nextRolloutPolicies, nextReleases] = await Promise.all([
        apiGet<AgentUpdateRolloutRecord[]>("/api/v1/agent-update-rollouts?limit=200", apiToken),
        apiGet<AgentUpdateRolloutPolicyRecord[]>("/api/v1/agent-update-rollout-policies?limit=200", apiToken),
        apiGet<AgentUpdateReleaseRecord[]>("/api/v1/agent-update-releases?limit=200", apiToken),
      ]);
      setAgentUpdateRollouts(nextRollouts);
      setAgentUpdateRolloutPolicies(nextRolloutPolicies);
      setAgentUpdateReleases(nextReleases);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setAgentUpdateRollouts([]);
        setAgentUpdateRolloutPolicies([]);
        setAgentUpdateReleases([]);
        return;
      }
      setJobsError(error instanceof Error ? error.message : "Agent update rollouts unavailable");
    }
  }, [apiToken, onUnauthorized]);

  const loadTerminalSessions = useCallback(async () => {
    try {
      setTerminalSessions(await apiGet<TerminalSessionRecord[]>("/api/v1/terminal-sessions?limit=200", apiToken));
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
      }
    }
  }, [apiToken, onUnauthorized]);

  const loadJobTargets = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobTargetRecord[]>(`/api/v1/jobs/${encodeURIComponent(jobId)}/targets`, apiToken);
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const loadJob = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobHistoryRecord>(`/api/v1/jobs/${encodeURIComponent(jobId)}`, apiToken);
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const loadJobOutputs = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobOutputRecord[]>(`/api/v1/jobs/${encodeURIComponent(jobId)}/outputs`, apiToken);
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const loadJobOutputComparison = useCallback(
    async (jobId: string) => {
      try {
        return await apiGet<JobOutputComparisonRecord[]>(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/output-comparison`,
          apiToken,
        );
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const upsertCommandTemplate = useCallback(
    async (request: UpsertCommandTemplateRequest) => {
      const response = await apiPost<CommandTemplateRecord>("/api/v1/command-templates", apiToken, request);
      setCommandTemplates((current) => {
        const withoutTemplate = current.filter((template) => template.id !== response.id);
        return [response, ...withoutTemplate].sort((left, right) => right.updated_at.localeCompare(left.updated_at));
      });
      void onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged],
  );

  const downloadJobOutputArtifact = useCallback(
    async (jobId: string, clientId: string, seq: number) => {
      try {
        return await apiGetBlob(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/outputs/${encodeURIComponent(clientId)}/${seq}/artifact`,
          apiToken,
        );
      } catch (error) {
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const createFileTransferHandoff = useCallback(
    async (clientId: string, sessionId: string) => {
      try {
        const response = await apiPost<FileTransferHandoffRecord>(
          `/api/v1/file-transfers/${encodeURIComponent(clientId)}/${encodeURIComponent(sessionId)}/handoff`,
          apiToken,
          { confirmed: true },
        );
        await loadJobs();
        return response;
      } catch (error) {
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
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
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
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const uploadFileTransferSource = useCallback(
    async (request: UploadFileTransferSourceArtifactRequest) => {
      try {
        const response = await apiPost<FileTransferSourceArtifactRecord>("/api/v1/file-transfer-sources", apiToken, request);
        await loadJobs();
        return response;
      } catch (error) {
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
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
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
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          throw new Error("Operator login required");
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const createJob = useCallback(
    async (request: CreateJobRequest) => {
      const response = await apiPost<CreateJobResponse>("/api/v1/jobs", apiToken, request);
      void Promise.allSettled([loadJobs(), loadAgentUpdateRollouts(), onFleetChanged(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, loadJobs, onAuditChanged, onFleetChanged],
  );

  const createAgentUpdateRelease = useCallback(
    async (request: CreateAgentUpdateReleaseRequest) => {
      const response = await apiPost<AgentUpdateReleaseRecord>("/api/v1/agent-update-releases", apiToken, request);
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const uploadAgentUpdateArtifact = useCallback(
    async (request: UploadAgentUpdateArtifactRequest) => {
      const response = await apiPost<AgentUpdateReleaseRecord>("/api/v1/agent-update-releases/upload", apiToken, request);
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const streamAgentUpdateArtifact = useCallback(
    async (file: File, artifactSignatureHex: string, artifactSigningKeyHex: string) => {
      const response = await apiPostBinary<StreamedAgentUpdateArtifactRecord>(
        "/api/v1/agent-update-artifacts/stream",
        apiToken,
        file,
        {
          "Content-Type": "application/octet-stream",
          "x-vpsman-artifact-signature-hex": artifactSignatureHex,
          "x-vpsman-artifact-signing-key-hex": artifactSigningKeyHex,
          "x-vpsman-confirmed": "true",
        },
      );
      return response;
    },
    [apiToken],
  );

  const createHostedAgentUpdateRelease = useCallback(
    async (request: CreateHostedAgentUpdateReleaseRequest) => {
      const response = await apiPost<AgentUpdateReleaseRecord>(
        "/api/v1/agent-update-releases/hosted",
        apiToken,
        request,
      );
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const updateAgentUpdateRolloutControl = useCallback(
    async (rolloutId: string, request: AgentUpdateRolloutControlRequest) => {
      const response = await apiPost<AgentUpdateRolloutRecord>(
        `/api/v1/agent-update-rollouts/${encodeURIComponent(rolloutId)}/control`,
        apiToken,
        request,
      );
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const createAgentUpdateRolloutPolicy = useCallback(
    async (request: CreateAgentUpdateRolloutPolicyRequest) => {
      const response = await apiPost<AgentUpdateRolloutPolicyRecord>(
        "/api/v1/agent-update-rollout-policies",
        apiToken,
        request,
      );
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const delegateAgentUpdateRollback = useCallback(
    async (rolloutId: string, request: AgentUpdateRollbackDelegationRequest) => {
      const response = await apiPost<AgentUpdateRollbackDelegationRecord>(
        `/api/v1/agent-update-rollouts/${encodeURIComponent(rolloutId)}/rollback-delegation`,
        apiToken,
        request,
      );
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const delegateAgentUpdateActivation = useCallback(
    async (rolloutId: string, request: AgentUpdateActivationDelegationRequest) => {
      const response = await apiPost<AgentUpdateActivationDelegationRecord>(
        `/api/v1/agent-update-rollouts/${encodeURIComponent(rolloutId)}/activation-delegation`,
        apiToken,
        request,
      );
      await loadAgentUpdateRollouts();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateRollouts, onAuditChanged],
  );

  const dispatchScheduledJob = useCallback(
    async (jobId: string, request: DispatchScheduledJobRequest) => {
      const response = await apiPost<CreateJobResponse>(
        `/api/v1/jobs/${encodeURIComponent(jobId)}/dispatch-scheduled`,
        apiToken,
        request,
      );
      void Promise.allSettled([loadJobs(), onFleetChanged(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged, onFleetChanged],
  );

  const cancelJob = useCallback(
    async (jobId: string, request: CancelJobRequest) => {
      const response = await apiPost<CancelJobResponse>(
        `/api/v1/jobs/${encodeURIComponent(jobId)}/cancel`,
        apiToken,
        request,
      );
      void Promise.allSettled([loadJobs(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged],
  );

  return {
    cancelJob,
    createAgentUpdateRelease,
    createAgentUpdateRolloutPolicy,
    delegateAgentUpdateActivation,
    delegateAgentUpdateRollback,
    updateAgentUpdateRolloutControl,
    uploadAgentUpdateArtifact,
    streamAgentUpdateArtifact,
    createHostedAgentUpdateRelease,
    createJob,
    commandTemplates,
    agentUpdateReleases,
    agentUpdateRolloutPolicies,
    agentUpdateRollouts,
    dispatchScheduledJob,
    fileTransfers,
    fileTransferSources,
    jobs,
    jobsError,
    jobsLoading,
    processSupervisorInventory,
    terminalSessions,
    loadJob,
    createFileTransferHandoff,
    uploadFileTransferSource,
    downloadJobOutputArtifact,
    downloadFileTransferHandoff,
    downloadFileTransferSource,
    saveFileTransferHandoff,
    loadJobOutputs,
    loadJobOutputComparison,
    loadJobTargets,
    loadJobs,
    loadTerminalReplay,
    loadTerminalSessions,
    loadAgentUpdateRollouts,
    upsertCommandTemplate,
  };
}
