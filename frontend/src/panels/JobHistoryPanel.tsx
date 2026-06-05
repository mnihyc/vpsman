import { useCallback, useEffect, useMemo, useState } from "react";
import { Download, Server, ShieldCheck, TerminalSquare } from "lucide-react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../components/ConsoleDataGrid";
import { CrudPager } from "../components/CrudPager";
import { ProofVaultBox } from "../components/ProofVaultBox";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildEnvelopesForOperation,
  buildEnvelopesForPayloadHash,
  type ProofMaterial,
} from "../proof";
import type { ArtifactDownloadMode } from "../artifactDownload";
import type {
  AgentView,
  AgentUpdateActivationDelegationRecord,
  AgentUpdateActivationDelegationRequest,
  AgentUpdateReleaseRecord,
  AgentUpdateRollbackDelegationRecord,
  AgentUpdateRollbackDelegationRequest,
  AgentUpdateRolloutControlRequest,
  AgentUpdateRolloutPolicyRecord,
  AgentUpdateRolloutRecord,
  BulkResolveResponse,
  CancelJobRequest,
  CancelJobResponse,
  CommandTemplateRecord,
  CreateJobRequest,
  CreateJobResponse,
  CreateAgentUpdateRolloutPolicyRequest,
  CreateAgentUpdateReleaseRequest,
  DispatchScheduledJobRequest,
  JobHistoryRecord,
  JobOutputComparisonRecord,
  JobOutputRecord,
  JobTargetRecord,
  JobTargetSelection,
  ProcessSupervisorInventoryRecord,
  TagView,
  UploadAgentUpdateArtifactRequest,
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
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  decodeOutputPreview,
  formatTime,
  runPanelAction,
  shortHash,
  shortId,
  statusClass,
} from "../utils";
import {
  JobDispatchPanel,
  type TerminalComposerAction,
} from "./JobDispatchPanel";
import { AgentUpdateReleasesPanel } from "./jobs/AgentUpdateReleasesPanel";
import { AgentUpdateRolloutsPanel } from "./jobs/AgentUpdateRolloutsPanel";
import { FileTransferSessionsPanel } from "./jobs/FileTransferSessionsPanel";
import { ProcessSupervisorInventoryPanel } from "./jobs/ProcessSupervisorInventoryPanel";
import { TerminalSessionsPanel } from "./jobs/TerminalSessionsPanel";

function selectRolloutClients(
  rollout: AgentUpdateRolloutRecord,
  eligibleStatuses: string[],
  batchSize?: number,
): string[] {
  const statusByClient = new Map(
    rollout.targets.map((target) => [target.client_id, target.status]),
  );
  const recommended = rollout.automation_targets.filter((clientId) =>
    eligibleStatuses.includes(statusByClient.get(clientId) ?? ""),
  );
  const candidates =
    recommended.length > 0
      ? recommended
      : rollout.targets
          .filter((target) => eligibleStatuses.includes(target.status))
          .map((target) => target.client_id);
  const unique = Array.from(new Set(candidates)).sort();
  if (batchSize !== undefined) {
    unique.splice(Math.max(1, Math.trunc(batchSize)));
  }
  return unique;
}

function selectRolloutDelegationClients(
  rollout: AgentUpdateRolloutRecord,
): string[] {
  const rolloutClients = new Set(
    rollout.targets.map((target) => target.client_id),
  );
  const recommended = rollout.automation_targets.filter((clientId) =>
    rolloutClients.has(clientId),
  );
  const candidates =
    recommended.length > 0 ? recommended : Array.from(rolloutClients);
  return Array.from(new Set(candidates)).sort();
}

function selectRolloutActivationDelegationClients(
  rollout: AgentUpdateRolloutRecord,
): string[] {
  return Array.from(
    new Set(rollout.targets.map((target) => target.client_id)),
  ).sort();
}

function displayToken(value: string): string {
  return value.replace(/_/g, " ");
}

export function JobHistoryPanel({
  activeSubpage,
  agents,
  agentUpdateReleases,
  agentUpdateRolloutPolicies,
  agentUpdateRollouts,
  error,
  fileTransfers,
  fileTransferSources,
  jobs,
  commandTemplates,
  lastJobOutputEvent,
  lastTerminalOutputEvent,
  loading,
  onCancelJob,
  onCreateAgentUpdateRelease,
  onCreateAgentUpdateRolloutPolicy,
  onCreateFileTransferHandoff,
  onUploadAgentUpdateArtifact,
  onCreateJob,
  onDispatchScheduledJob,
  onDelegateAgentUpdateActivation,
  onDelegateAgentUpdateRollback,
  onDownloadOutputArtifact,
  onDownloadFileTransferSource,
  onLoadJob,
  onLoadOutputs,
  onLoadOutputComparison,
  onLoadTerminalReplay,
  onLoadTargets,
  onRefresh,
  onResolveTargets,
  onSaveFileTransferHandoff,
  onUpdateAgentUpdateRolloutControl,
  onUploadFileTransferSource,
  onUpsertCommandTemplate,
  processSupervisorInventory,
  terminalSessions,
  tags,
}: {
  activeSubpage: string;
  agents: AgentView[];
  agentUpdateReleases: AgentUpdateReleaseRecord[];
  agentUpdateRolloutPolicies: AgentUpdateRolloutPolicyRecord[];
  agentUpdateRollouts: AgentUpdateRolloutRecord[];
  error: string | null;
  fileTransfers: FileTransferSessionRecord[];
  fileTransferSources: FileTransferSourceArtifactRecord[];
  jobs: JobHistoryRecord[];
  commandTemplates: CommandTemplateRecord[];
  lastJobOutputEvent: WsJobOutputEvent | null;
  lastTerminalOutputEvent: WsTerminalOutputEvent | null;
  loading: boolean;
  onCancelJob: (
    jobId: string,
    request: CancelJobRequest,
  ) => Promise<CancelJobResponse>;
  onCreateAgentUpdateRelease: (
    request: CreateAgentUpdateReleaseRequest,
  ) => Promise<AgentUpdateReleaseRecord>;
  onCreateAgentUpdateRolloutPolicy: (
    request: CreateAgentUpdateRolloutPolicyRequest,
  ) => Promise<AgentUpdateRolloutPolicyRecord>;
  onCreateFileTransferHandoff: (
    clientId: string,
    sessionId: string,
  ) => Promise<FileTransferHandoffRecord>;
  onUploadAgentUpdateArtifact: (
    request: UploadAgentUpdateArtifactRequest,
  ) => Promise<AgentUpdateReleaseRecord>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onDispatchScheduledJob: (
    jobId: string,
    request: DispatchScheduledJobRequest,
  ) => Promise<CreateJobResponse>;
  onDelegateAgentUpdateActivation: (
    rolloutId: string,
    request: AgentUpdateActivationDelegationRequest,
  ) => Promise<AgentUpdateActivationDelegationRecord>;
  onDelegateAgentUpdateRollback: (
    rolloutId: string,
    request: AgentUpdateRollbackDelegationRequest,
  ) => Promise<AgentUpdateRollbackDelegationRecord>;
  onDownloadOutputArtifact: (
    jobId: string,
    clientId: string,
    seq: number,
  ) => Promise<Blob>;
  onDownloadFileTransferSource: (downloadPath: string) => Promise<Blob>;
  onLoadJob: (jobId: string) => Promise<JobHistoryRecord>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadOutputComparison: (
    jobId: string,
  ) => Promise<JobOutputComparisonRecord[]>;
  onLoadTerminalReplay: (
    clientId: string,
    sessionId: string,
    fromSeq?: number,
  ) => Promise<TerminalReplayRecord>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onRefresh: () => void;
  onResolveTargets: (
    selection: JobTargetSelection,
  ) => Promise<BulkResolveResponse>;
  onUpdateAgentUpdateRolloutControl: (
    rolloutId: string,
    request: AgentUpdateRolloutControlRequest,
  ) => Promise<AgentUpdateRolloutRecord>;
  onSaveFileTransferHandoff: (
    downloadPath: string,
    request: {
      expectedSha256Hex?: string | null;
      expectedSizeBytes?: number | null;
      fileName: string;
      mode: ArtifactDownloadMode;
    },
  ) => Promise<void>;
  onUploadFileTransferSource: (
    request: UploadFileTransferSourceArtifactRequest,
  ) => Promise<FileTransferSourceArtifactRecord>;
  onUpsertCommandTemplate: (
    request: UpsertCommandTemplateRequest,
  ) => Promise<CommandTemplateRecord>;
  processSupervisorInventory: ProcessSupervisorInventoryRecord[];
  terminalSessions: TerminalSessionRecord[];
  tags: TagView[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectedJobId, setSelectedJobId] = useState<string | null>(null);
  const [targets, setTargets] = useState<JobTargetRecord[]>([]);
  const [outputs, setOutputs] = useState<JobOutputRecord[]>([]);
  const [outputComparison, setOutputComparison] = useState<
    JobOutputComparisonRecord[]
  >([]);
  const [targetError, setTargetError] = useState<string | null>(null);
  const [outputError, setOutputError] = useState<string | null>(null);
  const [comparisonError, setComparisonError] = useState<string | null>(null);
  const [targetsLoading, setTargetsLoading] = useState(false);
  const [outputsLoading, setOutputsLoading] = useState(false);
  const [comparisonLoading, setComparisonLoading] = useState(false);
  const [proofMaterial, setProofMaterial] = useState<ProofMaterial | null>(
    null,
  );
  const [proofTtlSecs, setProofTtlSecs] = useState(300);
  const [approvalError, setApprovalError] = useState<string | null>(null);
  const [approvalPending, setApprovalPending] = useState(false);
  const [approvalPendingJobId, setApprovalPendingJobId] = useState<
    string | null
  >(null);
  const [forceScheduledUnprivileged, setForceScheduledUnprivileged] =
    useState(false);
  const [lastApprovalHash, setLastApprovalHash] = useState<string | null>(null);
  const [rolloutBatchSize, setRolloutBatchSize] = useState(1);
  const [rolloutRestartAgent, setRolloutRestartAgent] = useState(false);
  const [rolloutForceUnprivileged, setRolloutForceUnprivileged] =
    useState(false);
  const [rolloutActionError, setRolloutActionError] = useState<string | null>(
    null,
  );
  const [rolloutActionPending, setRolloutActionPending] = useState(false);
  const [rolloutActionId, setRolloutActionId] = useState<string | null>(null);
  const [terminalComposerAction, setTerminalComposerAction] =
    useState<TerminalComposerAction | null>(null);
  const jobSubpage = [
    "history",
    "dispatch",
    "updates",
    "transfers",
    "terminal",
    "processes",
    "approvals",
  ].includes(activeSubpage)
    ? activeSubpage
    : "history";
  const [cancelError, setCancelError] = useState<string | null>(null);
  const [cancelPending, setCancelPending] = useState(false);
  const [cancelPendingJobId, setCancelPendingJobId] = useState<string | null>(
    null,
  );
  const [artifactError, setArtifactError] = useState<string | null>(null);
  const [artifactPendingKey, setArtifactPendingKey] = useState<string | null>(
    null,
  );
  const scheduledApprovalCount = jobs.filter(isScheduledApprovalJob).length;
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);
  const openTargets = useCallback(
    async (jobId: string) => {
      setSelectedJobId(jobId);
      setTargetsLoading(true);
      setOutputsLoading(true);
      setTargetError(null);
      setOutputError(null);
      try {
        const [nextTargets, nextOutputs] = await Promise.all([
          onLoadTargets(jobId),
          onLoadOutputs(jobId),
        ]);
        setTargets(nextTargets);
        setOutputs(nextOutputs);
      } catch (loadError) {
        setTargets([]);
        setOutputs([]);
        setOutputComparison([]);
        setTargetError(
          loadError instanceof Error
            ? loadError.message
            : "Job target history unavailable",
        );
        setOutputError(
          loadError instanceof Error
            ? loadError.message
            : "Job output unavailable",
        );
      } finally {
        setTargetsLoading(false);
        setOutputsLoading(false);
      }
    },
    [onLoadOutputs, onLoadTargets],
  );

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
              className={`status ${statusClass(job.status)}`}
              title={job.status}
            >
              {displayToken(job.status)}
            </span>
            {isScheduledApprovalJob(job) && (
              <button
                aria-label="Approve scheduled job"
                className="secondaryAction compactAction"
                disabled={approvalPending || !proofMaterial}
                onClick={(event) => {
                  event.stopPropagation();
                  void approveScheduledJob(job);
                }}
                type="button"
              >
                <ShieldCheck size={14} />
                <span>
                  {approvalPendingJobId === job.id ? "Approving" : "Approve"}
                </span>
              </button>
            )}
            {isCancelableJob(job) && (
              <button
                aria-label={
                  isActiveCancelableJob(job)
                    ? "Cancel active job"
                    : "Cancel pending job"
                }
                className="secondaryAction compactAction dangerAction"
                disabled={cancelPending}
                onClick={(event) => {
                  event.stopPropagation();
                  void cancelPendingJob(job);
                }}
                type="button"
              >
                <span>
                  {cancelPendingJobId === job.id ? "Canceling" : "Cancel"}
                </span>
              </button>
            )}
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
        sortValue: (job) => (job.privileged ? "proof" : "none"),
        searchValue: (job) =>
          job.privileged ? "proof privileged" : "none unprivileged",
        cell: (job) => (
          <span className={job.privileged ? "status info" : "status neutral"}>
            {job.privileged ? "proof" : "none"}
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
    [
      approvalPending,
      approvalPendingJobId,
      cancelPending,
      cancelPendingJobId,
      openTargets,
      proofMaterial,
    ],
  );

  async function compareSelectedJobOutputs(jobId: string) {
    setComparisonLoading(true);
    setComparisonError(null);
    try {
      setOutputComparison(await onLoadOutputComparison(jobId));
    } catch (loadError) {
      setOutputComparison([]);
      setComparisonError(
        loadError instanceof Error
          ? loadError.message
          : "Output comparison unavailable",
      );
    } finally {
      setComparisonLoading(false);
    }
  }

  useEffect(() => {
    if (lastJobOutputEvent && selectedJobId === lastJobOutputEvent.job_id) {
      void openTargets(lastJobOutputEvent.job_id);
    }
  }, [lastJobOutputEvent, openTargets, selectedJobId]);

  async function approveScheduledJob(job: JobHistoryRecord) {
    setApprovalPendingJobId(job.id);
    await runPanelAction(setApprovalPending, setApprovalError, async () => {
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      const [latestJob, latestTargets] = await Promise.all([
        onLoadJob(job.id),
        onLoadTargets(job.id),
      ]);
      if (!isScheduledApprovalJob(latestJob)) {
        throw new Error("Scheduled job is not waiting for approval");
      }
      const approvalTargets = latestTargets
        .filter((target) => target.status === "approval_required")
        .map((target) => target.client_id);
      if (approvalTargets.length === 0) {
        throw new Error("Scheduled job has no waiting targets");
      }
      const built = await buildEnvelopesForPayloadHash({
        clientIds: approvalTargets,
        payloadHashHex: latestJob.payload_hash,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      setLastApprovalHash(built.payloadHashHex);
      await onDispatchScheduledJob(job.id, {
        confirmed: true,
        timeout_secs: 30,
        force_unprivileged: forceScheduledUnprivileged,
        envelope: null,
        envelopes: built.envelopes,
      });
      await openTargets(job.id);
    });
    setApprovalPendingJobId(null);
  }

  async function cancelPendingJob(job: JobHistoryRecord) {
    setCancelPendingJobId(job.id);
    await runPanelAction(setCancelPending, setCancelError, async () => {
      await onCancelJob(job.id, {
        confirmed: true,
        reason: `Canceled from panel while status was ${job.status}`,
      });
      if (selectedJobId === job.id) {
        await openTargets(job.id);
      }
    });
    setCancelPendingJobId(null);
  }

  async function downloadOutputArtifact(output: JobOutputRecord) {
    if (!selectedJobId) {
      return;
    }
    const pendingKey = `${output.client_id}:${output.seq}`;
    setArtifactPendingKey(pendingKey);
    await runPanelAction(
      () => undefined,
      setArtifactError,
      async () => {
        const blob = await onDownloadOutputArtifact(
          selectedJobId,
          output.client_id,
          output.seq,
        );
        const url = URL.createObjectURL(blob);
        const link = document.createElement("a");
        link.href = url;
        link.download = `job-output-${shortId(selectedJobId)}-${output.client_id}-${output.seq}.bin`;
        document.body.appendChild(link);
        link.click();
        link.remove();
        URL.revokeObjectURL(url);
      },
    );
    setArtifactPendingKey(null);
  }

  async function activateRolloutBatch(rollout: AgentUpdateRolloutRecord) {
    setRolloutActionId(rollout.id);
    await runPanelAction(
      setRolloutActionPending,
      setRolloutActionError,
      async () => {
        if (!proofMaterial) {
          throw new Error("Proof is locked");
        }
        const batchSize = Math.max(
          1,
          Math.trunc(rolloutBatchSize || rollout.canary_count || 1),
        );
        const clientIds = selectRolloutClients(
          rollout,
          ["completed"],
          batchSize,
        );
        if (clientIds.length === 0) {
          throw new Error("No staged targets are eligible for activation");
        }
        const operation = rolloutRestartAgent
          ? ({
              type: "agent_update_activate",
              staged_sha256_hex: rollout.artifact_sha256_hex,
              restart_agent: true,
            } as const)
          : ({
              type: "agent_update_activate",
              staged_sha256_hex: rollout.artifact_sha256_hex,
            } as const);
        const built = await buildEnvelopesForOperation({
          clientIds,
          operation,
          proofTtlSecs,
          superPassword: proofMaterial.superPassword,
          superSaltHex: proofMaterial.superSaltHex,
        });
        setLastApprovalHash(built.payloadHashHex);
        await onCreateJob({
          clients: clientIds,
          tags: [],
          destructive: false,
          confirmed: true,
          command: "agent_update_activate",
          argv: [],
          operation,
          timeout_secs: 60,
          canary_count: null,
          force_unprivileged: rolloutForceUnprivileged,
          privileged: true,
          idempotency_key: `panel:rollout-activate:${rollout.id}:${built.payloadHashHex.slice(0, 16)}`,
          reconnect_policy: {
            duplicate_delivery: "ignore_completed",
            resume_outputs: true,
            cancel_on_disconnect: false,
          },
          envelope: null,
          envelopes: built.envelopes,
        });
      },
    );
    setRolloutActionId(null);
  }

  async function rollbackRolloutTargets(rollout: AgentUpdateRolloutRecord) {
    setRolloutActionId(rollout.id);
    await runPanelAction(
      setRolloutActionPending,
      setRolloutActionError,
      async () => {
        if (!proofMaterial) {
          throw new Error("Proof is locked");
        }
        const clientIds = selectRolloutClients(rollout, [
          "activation_pending_restart",
          "activation_failed",
          "heartbeat_timeout",
          "heartbeat_verified",
        ]);
        if (clientIds.length === 0) {
          throw new Error(
            "No activation-pending, activation-failed, heartbeat-timeout, or heartbeat-verified targets are eligible for rollback",
          );
        }
        const operation = { type: "agent_update_rollback" } as const;
        const built = await buildEnvelopesForOperation({
          clientIds,
          operation,
          proofTtlSecs,
          superPassword: proofMaterial.superPassword,
          superSaltHex: proofMaterial.superSaltHex,
        });
        setLastApprovalHash(built.payloadHashHex);
        await onCreateJob({
          clients: clientIds,
          tags: [],
          destructive: false,
          confirmed: true,
          command: "agent_update_rollback",
          argv: [],
          operation,
          timeout_secs: 60,
          canary_count: null,
          force_unprivileged: rolloutForceUnprivileged,
          privileged: true,
          idempotency_key: `panel:rollout-rollback:${rollout.id}:${built.payloadHashHex.slice(0, 16)}`,
          reconnect_policy: {
            duplicate_delivery: "ignore_completed",
            resume_outputs: true,
            cancel_on_disconnect: false,
          },
          envelope: null,
          envelopes: built.envelopes,
        });
      },
    );
    setRolloutActionId(null);
  }

  async function delegateRolloutActivation(rollout: AgentUpdateRolloutRecord) {
    setRolloutActionId(rollout.id);
    await runPanelAction(
      setRolloutActionPending,
      setRolloutActionError,
      async () => {
        if (!proofMaterial) {
          throw new Error("Proof is locked");
        }
        const clientIds = selectRolloutActivationDelegationClients(rollout);
        if (clientIds.length === 0) {
          throw new Error(
            "No rollout targets are available for activation delegation",
          );
        }
        const operation = rolloutRestartAgent
          ? ({
              type: "agent_update_activate",
              staged_sha256_hex: rollout.artifact_sha256_hex,
              restart_agent: true,
            } as const)
          : ({
              type: "agent_update_activate",
              staged_sha256_hex: rollout.artifact_sha256_hex,
            } as const);
        const built = await buildEnvelopesForOperation({
          clientIds,
          maxProofTtlSecs: 86_400,
          operation,
          proofTtlSecs,
          superPassword: proofMaterial.superPassword,
          superSaltHex: proofMaterial.superSaltHex,
        });
        setLastApprovalHash(built.payloadHashHex);
        await onDelegateAgentUpdateActivation(rollout.id, {
          confirmed: true,
          restart_agent: rolloutRestartAgent,
          force_unprivileged: rolloutForceUnprivileged,
          envelopes: built.envelopes,
        });
      },
    );
    setRolloutActionId(null);
  }

  async function delegateRolloutRollback(rollout: AgentUpdateRolloutRecord) {
    setRolloutActionId(rollout.id);
    await runPanelAction(
      setRolloutActionPending,
      setRolloutActionError,
      async () => {
        if (!proofMaterial) {
          throw new Error("Proof is locked");
        }
        const clientIds = selectRolloutDelegationClients(rollout);
        if (clientIds.length === 0) {
          throw new Error(
            "No rollout targets are available for rollback delegation",
          );
        }
        const operation = { type: "agent_update_rollback" } as const;
        const built = await buildEnvelopesForOperation({
          clientIds,
          maxProofTtlSecs: 86_400,
          operation,
          proofTtlSecs,
          superPassword: proofMaterial.superPassword,
          superSaltHex: proofMaterial.superSaltHex,
        });
        setLastApprovalHash(built.payloadHashHex);
        await onDelegateAgentUpdateRollback(rollout.id, {
          confirmed: true,
          force_unprivileged: rolloutForceUnprivileged,
          envelopes: built.envelopes,
        });
      },
    );
    setRolloutActionId(null);
  }

  async function controlRollout(
    rollout: AgentUpdateRolloutRecord,
    request: AgentUpdateRolloutControlRequest,
  ) {
    setRolloutActionId(rollout.id);
    await runPanelAction(
      setRolloutActionPending,
      setRolloutActionError,
      async () => {
        await onUpdateAgentUpdateRolloutControl(rollout.id, request);
      },
    );
    setRolloutActionId(null);
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
    <section
      className={
        jobSubpage === "dispatch"
          ? "workspace singleColumn jobWorkspace"
          : "workspace singleColumn"
      }
    >
      {jobSubpage === "dispatch" && (
        <JobDispatchPanel
          agents={agents}
          fileTransferSources={fileTransferSources}
          commandTemplates={commandTemplates}
          terminalComposerAction={terminalComposerAction}
          onCreateJob={onCreateJob}
          onDownloadFileTransferSource={onDownloadFileTransferSource}
          onDownloadOutputArtifact={onDownloadOutputArtifact}
          onLoadJob={onLoadJob}
          onLoadOutputs={onLoadOutputs}
          onResolveTargets={onResolveTargets}
          onUpsertCommandTemplate={onUpsertCommandTemplate}
          tags={tags}
        />
      )}
      {jobSubpage === "updates" && (
        <div className="jobConsoleStack">
          <AgentUpdateReleasesPanel
            loading={loading}
            onCreateAgentUpdateRelease={onCreateAgentUpdateRelease}
            onRefresh={onRefresh}
            onUploadAgentUpdateArtifact={onUploadAgentUpdateArtifact}
            releases={agentUpdateReleases}
          />
          <AgentUpdateRolloutsPanel
            actionError={rolloutActionError}
            actionId={rolloutActionId}
            actionPending={rolloutActionPending}
            batchSize={rolloutBatchSize}
            loading={loading}
            onActivateBatch={(rollout) => void activateRolloutBatch(rollout)}
            onBatchSizeChange={setRolloutBatchSize}
            onControlRollout={(rollout, request) =>
              void controlRollout(rollout, request)
            }
            onCreatePolicy={onCreateAgentUpdateRolloutPolicy}
            onDelegateActivation={(rollout) =>
              void delegateRolloutActivation(rollout)
            }
            onDelegateRollback={(rollout) =>
              void delegateRolloutRollback(rollout)
            }
            onForceUnprivilegedChange={setRolloutForceUnprivileged}
            onProofTtlSecsChange={setProofTtlSecs}
            onRefresh={onRefresh}
            onRestartAgentChange={setRolloutRestartAgent}
            onRollbackTargets={(rollout) =>
              void rollbackRolloutTargets(rollout)
            }
            proofMaterial={proofMaterial}
            proofTtlSecs={proofTtlSecs}
            restartAgent={rolloutRestartAgent}
            forceUnprivileged={rolloutForceUnprivileged}
            policies={agentUpdateRolloutPolicies}
            rollouts={agentUpdateRollouts}
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
            onDownloadOutputArtifact={onDownloadOutputArtifact}
            onLoadJob={onLoadJob}
            onLoadOutputs={onLoadOutputs}
            onResolveTargets={onResolveTargets}
            onUpsertCommandTemplate={onUpsertCommandTemplate}
            tags={tags}
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
                    cancelError ??
                    (loading
                      ? "Refreshing command records"
                      : "Latest proof-gated requests")}
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
                {
                  label: "Approve selected",
                  disabled: (rows) =>
                    !proofMaterial || !rows.some(isScheduledApprovalJob),
                  onSelect: (rows) =>
                    rows
                      .filter(isScheduledApprovalJob)
                      .forEach((job) => void approveScheduledJob(job)),
                },
                {
                  label: "Cancel selected",
                  tone: "danger",
                  disabled: (rows) => !rows.some(isCancelableJob),
                  onSelect: (rows) =>
                    rows
                      .filter(isCancelableJob)
                      .forEach((job) => void cancelPendingJob(job)),
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
                      <span>Exit</span>
                      <span>Completed</span>
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
                          className={`status ${statusClass(target.status)}`}
                        >
                          {target.status}
                        </span>
                        <span>{target.exit_code ?? "-"}</span>
                        <span>
                          {target.completed_at
                            ? formatTime(target.completed_at)
                            : "-"}
                        </span>
                      </div>
                    ))}
                  </div>
                )}
              </CrudPager>
              <div className="outputDetail">
                <div className="sectionHeader compact">
                  <h2>Output</h2>
                  <span>
                    {outputError ??
                      artifactError ??
                      (outputsLoading
                        ? "Loading output records"
                        : `${outputs.length} chunks`)}
                  </span>
                </div>
                <div className="approvalControls compact">
                  <button
                    className="secondaryAction"
                    disabled={comparisonLoading}
                    onClick={() =>
                      void compareSelectedJobOutputs(selectedJobId)
                    }
                    type="button"
                  >
                    Compare outputs
                  </button>
                  <span>
                    {comparisonError ??
                      (comparisonLoading
                        ? "Comparing outputs"
                        : `${outputComparison.length} comparison rows`)}
                  </span>
                </div>
                {outputComparison.length > 0 && (
                  <CrudPager
                    fields={[
                      {
                        label: "Client",
                        value: (row) => clientLabel(row.client_id),
                      },
                      { label: "Hash", value: (row) => row.output_sha256_hex },
                      {
                        label: "Majority",
                        value: (row) =>
                          row.matches_majority ? "match" : "diff",
                      },
                      { label: "Exit", value: (row) => row.exit_code },
                    ]}
                    itemLabel="comparisons"
                    items={outputComparison}
                    pageSize={8}
                    title="Output comparison"
                  >
                    {(comparisonRows) => (
                      <div className="table historyTable">
                        <div className="historyRow heading targetHistoryGrid">
                          <span>Client</span>
                          <span>Majority</span>
                          <span>Bytes</span>
                          <span>Hash</span>
                        </div>
                        {comparisonRows.map((row) => (
                          <div
                            className="historyRow targetHistoryGrid"
                            key={row.client_id}
                          >
                            <span className="historyPrimary">
                              <strong>{clientLabel(row.client_id)}</strong>
                              <small>
                                {row.stream_count} chunks, exit{" "}
                                {row.exit_code ?? "-"}
                              </small>
                            </span>
                            <span
                              className={`status ${row.matches_majority ? "ok" : "warn"}`}
                            >
                              {row.matches_majority ? "match" : "diff"}
                            </span>
                            <span>{formatBytes(row.byte_count)}</span>
                            <span className="monoValue" title={row.preview}>
                              {shortHash(row.output_sha256_hex)}
                            </span>
                          </div>
                        ))}
                      </div>
                    )}
                  </CrudPager>
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
                          {output.storage === "object_store" &&
                          output.artifact_size_bytes
                            ? ` · ${formatBytes(output.artifact_size_bytes)}`
                            : ""}
                        </small>
                      </div>
                      {output.storage === "object_store" ? (
                        <div className="outputArtifact">
                          <div className="outputActions">
                            <button
                              aria-label="Download retained job output artifact"
                              className="secondaryAction compactAction"
                              disabled={
                                artifactPendingKey ===
                                `${output.client_id}:${output.seq}`
                              }
                              onClick={() =>
                                void downloadOutputArtifact(output)
                              }
                              type="button"
                            >
                              <Download size={14} />
                              <span>
                                {artifactPendingKey ===
                                `${output.client_id}:${output.seq}`
                                  ? "Downloading"
                                  : "Download"}
                              </span>
                            </button>
                          </div>
                          <pre>
                            {`artifact ${output.artifact_object_key ?? "retained externally"}\nsha256 ${output.artifact_sha256_hex ?? "-"}`}
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
          {scheduledApprovalCount > 0 || agentUpdateRollouts.length > 0 ? (
            <div className="scheduledApprovalPanel">
              <div className="sectionHeader compact">
                <h2>Privileged approvals</h2>
                <span>
                  {approvalError ??
                    rolloutActionError ??
                    cancelError ??
                    `${scheduledApprovalCount} scheduled waiting`}
                </span>
              </div>
              <div className="approvalControls">
                <label>
                  <span>Proof TTL</span>
                  <input
                    aria-label="Scheduled approval proof TTL seconds"
                    max={3600}
                    min={15}
                    onChange={(event) =>
                      setProofTtlSecs(Number(event.target.value))
                    }
                    type="number"
                    value={proofTtlSecs}
                  />
                </label>
                <label className="checkLine">
                  <input
                    aria-label="Force unprivileged scheduled best effort"
                    checked={forceScheduledUnprivileged}
                    onChange={(event) =>
                      setForceScheduledUnprivileged(event.target.checked)
                    }
                    type="checkbox"
                  />
                  <span>Force unprivileged best effort</span>
                </label>
              </div>
              <ProofVaultBox
                clearVaultLabel="Clear approval vault"
                labelPrefix="Approval"
                lastPayloadHash={lastApprovalHash}
                lockProofLabel="Lock approval proof"
                onProofMaterialChange={setProofMaterial}
                proofMaterial={proofMaterial}
                unlockLabel="Unlock approval proof"
                useProofLabel="Use approval proof"
              />
            </div>
          ) : (
            <div className="fleetPanel">
              <div className="emptyState">
                <ShieldCheck size={22} />
                <strong>No privileged approvals</strong>
                <span>
                  No scheduled jobs or rollout actions are waiting for proof.
                </span>
              </div>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function isScheduledApprovalJob(job: JobHistoryRecord): boolean {
  return (
    job.status === "approval_required" &&
    job.command_type.startsWith("scheduled_")
  );
}

function isActiveCancelableJob(job: JobHistoryRecord): boolean {
  return job.status === "dispatching" || job.status === "cancel_requested";
}

async function copyText(value: string) {
  if (!value.trim()) {
    return;
  }
  await navigator.clipboard?.writeText(value);
}

function isCancelableJob(job: JobHistoryRecord): boolean {
  return isScheduledApprovalJob(job) || isActiveCancelableJob(job);
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
