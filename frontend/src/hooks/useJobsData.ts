import { useCallback, useState } from "react";
import { apiGet, apiGetBlob, apiPost, apiPostBinary, buildListPath, isApiUnauthorized } from "../api";
import { downloadVerifiedArtifact, type ArtifactDownloadMode } from "../artifactDownload";
import type {
  AgentUpdateReleaseRecord,
  CommandTemplateRecord,
  CreateAgentUpdateReleaseRequest,
  CreateHostedAgentUpdateReleaseRequest,
  CreateJobRequest,
  CreateJobResponse,
  JobHistoryRecord,
  JobOutputCompareMode,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
  ProcessSupervisorInventoryRecord,
  ArtifactCleanupPreviewRecord,
  ServerJobRecord,
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
  const [agentUpdateReleases, setAgentUpdateReleases] = useState<AgentUpdateReleaseRecord[]>([]);
  const [processSupervisorInventory, setProcessSupervisorInventory] = useState<ProcessSupervisorInventoryRecord[]>([]);
  const [fileTransfers, setFileTransfers] = useState<FileTransferSessionRecord[]>([]);
  const [fileTransferSources, setFileTransferSources] = useState<FileTransferSourceArtifactRecord[]>([]);
  const [terminalSessions, setTerminalSessions] = useState<TerminalSessionRecord[]>([]);
  const [serverJobs, setServerJobs] = useState<ServerJobRecord[]>([]);
  const [commandTemplates, setCommandTemplates] = useState<CommandTemplateRecord[]>([]);
  const [jobsError, setJobsError] = useState<string | null>(null);
  const [jobsLoading, setJobsLoading] = useState(false);

  const loadJobs = useCallback(async () => {
    setJobsLoading(true);
    setJobsError(null);
    try {
      const [
        jobsResult,
        releasesResult,
        processSupervisorInventoryResult,
        fileTransfersResult,
        fileTransferSourcesResult,
        terminalSessionsResult,
        serverJobsResult,
        commandTemplatesResult,
      ] = await Promise.allSettled([
        apiGet<JobHistoryRecord[]>(buildListPath("/api/v1/jobs", { limit: 1000, sort: "created_at", dir: "desc" }), apiToken),
        apiGet<AgentUpdateReleaseRecord[]>("/api/v1/agent-update-releases?limit=200", apiToken),
        apiGet<ProcessSupervisorInventoryRecord[]>("/api/v1/process-supervisor/inventory?limit=200", apiToken),
        apiGet<FileTransferSessionRecord[]>("/api/v1/file-transfers?limit=200", apiToken),
        apiGet<FileTransferSourceArtifactRecord[]>("/api/v1/file-transfer-sources?limit=200", apiToken),
        apiGet<TerminalSessionRecord[]>("/api/v1/terminal-sessions?limit=200", apiToken),
        apiGet<ServerJobRecord[]>("/api/v1/server-jobs?limit=200", apiToken),
        apiGet<CommandTemplateRecord[]>("/api/v1/command-templates?limit=1000", apiToken),
      ]);
      const settledResults = [
        jobsResult,
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
        setJobs([]);
        setAgentUpdateReleases([]);
        setProcessSupervisorInventory([]);
        setFileTransfers([]);
        setFileTransferSources([]);
        setTerminalSessions([]);
        setServerJobs([]);
        setCommandTemplates([]);
        setJobsError("Operator login required");
        return;
      }
      if (jobsResult.status === "fulfilled") setJobs(jobsResult.value);
      if (releasesResult.status === "fulfilled") setAgentUpdateReleases(releasesResult.value);
      if (processSupervisorInventoryResult.status === "fulfilled") {
        setProcessSupervisorInventory(processSupervisorInventoryResult.value);
      }
      if (fileTransfersResult.status === "fulfilled") setFileTransfers(fileTransfersResult.value);
      if (fileTransferSourcesResult.status === "fulfilled") setFileTransferSources(fileTransferSourcesResult.value);
      if (terminalSessionsResult.status === "fulfilled") setTerminalSessions(terminalSessionsResult.value);
      if (serverJobsResult.status === "fulfilled") setServerJobs(serverJobsResult.value);
      if (commandTemplatesResult.status === "fulfilled") setCommandTemplates(commandTemplatesResult.value);
      const firstFailure = settledResults.find((result): result is PromiseRejectedResult => result.status === "rejected");
      if (firstFailure) {
        setJobsError(firstFailure.reason instanceof Error ? firstFailure.reason.message : "Job history partially unavailable");
      }
    } finally {
      setJobsLoading(false);
    }
  }, [apiToken, onUnauthorized]);

  const loadAgentUpdateReleases = useCallback(async () => {
    try {
      setAgentUpdateReleases(await apiGet<AgentUpdateReleaseRecord[]>("/api/v1/agent-update-releases?limit=200", apiToken));
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setAgentUpdateReleases([]);
        return;
      }
      setJobsError(error instanceof Error ? error.message : "Agent update releases unavailable");
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

  const loadServerJobs = useCallback(async () => {
    try {
      setServerJobs(await apiGet<ServerJobRecord[]>("/api/v1/server-jobs?limit=200", apiToken));
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
    async (jobId: string, mode: JobOutputCompareMode) => {
      try {
        return await apiGet<JobOutputComparisonRecord>(
          `/api/v1/jobs/${encodeURIComponent(jobId)}/output-comparison?mode=${encodeURIComponent(mode)}`,
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
      const response = await apiPost<CreateJobResponse>("/api/v1/jobs", apiToken, {
        ...request,
        job_id: request.job_id ?? crypto.randomUUID(),
      });
      void Promise.allSettled([loadJobs(), onFleetChanged(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadJobs, onAuditChanged, onFleetChanged],
  );

  const previewArtifactCleanup = useCallback(
    async (expression: string) => {
      try {
        return await apiPost<ArtifactCleanupPreviewRecord>("/api/v1/server-jobs/artifact-cleanup/preview", apiToken, {
          expression,
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

  const createArtifactCleanupJob = useCallback(
    async (expression: string, previewHash: string) => {
      try {
        const response = await apiPost<ServerJobRecord>("/api/v1/server-jobs/artifact-cleanup", apiToken, {
          expression,
          preview_hash: previewHash,
          confirmed: true,
        });
        await loadServerJobs();
        void onAuditChanged();
        return response;
      } catch (error) {
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
          {},
        );
        await loadServerJobs();
        void onAuditChanged();
        return response;
      } catch (error) {
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
      await loadAgentUpdateReleases();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateReleases, onAuditChanged],
  );

  const uploadAgentUpdateArtifact = useCallback(
    async (request: UploadAgentUpdateArtifactRequest) => {
      const response = await apiPost<AgentUpdateReleaseRecord>("/api/v1/agent-update-releases/upload", apiToken, request);
      await loadAgentUpdateReleases();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateReleases, onAuditChanged],
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
      await loadAgentUpdateReleases();
      void onAuditChanged();
      return response;
    },
    [apiToken, loadAgentUpdateReleases, onAuditChanged],
  );




  return {
    createAgentUpdateRelease,
    uploadAgentUpdateArtifact,
    streamAgentUpdateArtifact,
    createHostedAgentUpdateRelease,
    createJob,
    commandTemplates,
    agentUpdateReleases,
    fileTransfers,
    fileTransferSources,
    jobs,
    jobsError,
    jobsLoading,
    processSupervisorInventory,
    serverJobs,
    terminalSessions,
    cancelServerJob,
    loadJob,
    createArtifactCleanupJob,
    createFileTransferHandoff,
    previewArtifactCleanup,
    uploadFileTransferSource,
    downloadJobOutputArtifact,
    downloadFileTransferHandoff,
    downloadFileTransferSource,
    saveFileTransferHandoff,
    loadJobOutputs,
    downloadFileDownloadBundle,
    loadJobOutputComparison,
    loadJobTargets,
    loadJobs,
    loadAgentUpdateReleases,
    loadServerJobs,
    loadTerminalReplay,
    loadTerminalSessions,
    upsertCommandTemplate,
  };
}
