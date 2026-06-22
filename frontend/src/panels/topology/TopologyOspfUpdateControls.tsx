import { useMemo, useState } from "react";
import { Gauge, ShieldCheck } from "lucide-react";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../../bulkJobProgress";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import { useReviewGenerationGuard, waitForReviewRender } from "../../hooks/useReviewGenerationGuard";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeAssertion, type PrivilegeMaterial } from "../../privilege";
import { selectorExpressionForClientIds } from "../../searchExpression";
import { buildNetworkOspfCostUpdateOperation } from "../../topologyApply";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  JobOperation,
  JobTargetRecord,
  NetworkOspfUpdatePlanRecord,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import { clientDisplayNameFromMap, clientDisplayNameMap, runPanelAction, shortId } from "../../utils";
import {
  clampJobTimeoutSecs,
  clampInteger,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

export function TopologyOspfUpdateControls({
  agents,
  onCreateJob,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  ospfUpdatePlans,
  privilegeMaterial,
  setPrivilegeMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const [selectedPlanId, setSelectedPlanId] = useState(() => ospfUpdatePlans[0]?.plan_id ?? "");
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [timeoutSecs, setTimeoutSecs] = useState(60);
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [ospfSnapshot, setOspfSnapshot] = useState<OspfUpdateSnapshot | null>(null);
  const [jobProgress, setJobProgress] = useState<BulkJobProgress | null>(null);
  const [lastJobProgress, setLastJobProgress] = useState<BulkJobProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);

  const selectedUpdatePlan =
    ospfUpdatePlans.find((plan) => plan.plan_id === selectedPlanId) ?? ospfUpdatePlans[0] ?? null;
  const selectedTunnelPlan = useMemo(
    () => tunnelPlans.find((plan) => plan.id === selectedUpdatePlan?.plan_id) ?? null,
    [selectedUpdatePlan?.plan_id, tunnelPlans],
  );
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
  const targetClientId =
    side === "left" ? selectedUpdatePlan?.left_client_id : selectedUpdatePlan?.right_client_id;
  const mutationTargets = resolveAgentsById(agents, targetClientId ? [targetClientId] : []);
  const visibleJobProgress = jobProgress ?? lastJobProgress;
  const status =
    actionError ??
    (reviewPending
      ? "Preparing OSPF review"
      : visibleJobProgress
      ? `OSPF result for job ${shortId(visibleJobProgress.jobId)}`
      : lastJob
        ? `OSPF update job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} targets`
      : privilegeMaterial
        ? "Ready"
        : "Locked");
  const canSubmit =
    !pending &&
    !ospfSnapshot &&
    !!selectedUpdatePlan &&
    !!selectedTunnelPlan &&
    !!targetClientId &&
    !!privilegeMaterial &&
    selectedUpdatePlan.current_ospf_cost !== selectedUpdatePlan.recommended_ospf_cost;

  function clearOspfReview() {
    invalidateReviewGeneration();
    setOspfSnapshot(null);
  }

  async function openOspfPrompt() {
    setActionError(null);
    const reviewGeneration = captureReviewGeneration();
    setReviewPending(true);
    try {
      await waitForReviewRender();
      await runPanelAction(setPending, setActionError, async () => {
      if (!selectedUpdatePlan || !selectedTunnelPlan || !targetClientId) {
        throw new Error("Select an OSPF update plan");
      }
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      if (selectedUpdatePlan.current_ospf_cost === selectedUpdatePlan.recommended_ospf_cost) {
        throw new Error("OSPF cost is already at the recommended value");
      }
      const builtOperation = await buildNetworkOspfCostUpdateOperation(
        selectedTunnelPlan.plan,
        side,
        selectedUpdatePlan.current_ospf_cost,
        selectedUpdatePlan.recommended_ospf_cost,
      );
      const endpointTarget = builtOperation.endpoint.localClientId;
      const selectorExpression = selectorExpressionForClientIds([endpointTarget]);
      const targets = resolveAgentsById(agents, [endpointTarget]);
      const boundedTimeoutSecs = clampJobTimeoutSecs(timeoutSecs);
      const builtPrivilege = await buildPrivilegeForJobOperation({
        clientIds: [endpointTarget],
        commandType: "network_ospf_cost_update",
        forceUnprivileged,
        operation: builtOperation.operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: boundedTimeoutSecs,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setOspfSnapshot({
        detail: `OSPF cost update on ${vpsCountLabel(targets.length)}.`,
        forceUnprivileged,
        jobId: crypto.randomUUID(),
        items: [
          { label: "Operation", value: "OSPF cost update" },
          { label: "Selector", value: selectorExpression },
          { label: "Targets", value: formatTargetAvailabilitySummary(targets) },
          { label: "Plan", value: selectedUpdatePlan.plan_name },
          { label: "Endpoint", value: side },
          {
            label: "Cost",
            value: `${selectedUpdatePlan.current_ospf_cost} to ${selectedUpdatePlan.recommended_ospf_cost}`,
          },
          { label: "Timeout", value: `${boundedTimeoutSecs}s` },
          { label: "Privilege unlock", value: "Unlocked locally" },
          { label: "Privilege", value: forceUnprivileged ? "Forced best effort" : "Root required" },
        ],
        operation: builtOperation.operation,
        payloadHashHex: builtPrivilege.payloadHashHex,
        privilegeAssertion: builtPrivilege.privilegeAssertion,
        selectorExpression,
        targetClientId: endpointTarget,
        targets,
        timeoutSecs: boundedTimeoutSecs,
      });
    });
    } finally {
      setReviewPending(false);
    }
  }

  function clearExecutionResults() {
    setJobProgress(null);
    setLastJobProgress(null);
    setLastJob(null);
  }

  async function submitOspfCostUpdate(snapshot: OspfUpdateSnapshot) {
    setOspfSnapshot(null);
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      const job = await onCreateJob({
        argv: [],
        selector_expression: snapshot.selectorExpression,
        target_client_ids: [snapshot.targetClientId],
        command: "network_ospf_cost_update",
        confirmed: true,
        destructive: true,
        operation: snapshot.operation,
        force_unprivileged: snapshot.forceUnprivileged,
        job_id: snapshot.jobId,
        privileged: true,
        privilege_assertion: snapshot.privilegeAssertion,
        timeout_secs: snapshot.timeoutSecs,
      });
      setLastPayloadHash(snapshot.payloadHashHex);
      setLastJob(job);
      await trackOspfProgress(job, snapshot.targets, snapshot.timeoutSecs);
    });
  }

  async function trackOspfProgress(job: CreateJobResponse, targets: AgentView[], timeoutSecsForSnapshot: number) {
    const targetCount = createJobTargetCount(job);
    setLastJobProgress(null);
    setJobProgress(buildBulkJobProgress({
      jobId: job.job_id,
      targetCount,
      targetRecords: [],
      targets,
      timeoutSecs: timeoutSecsForSnapshot,
    }));
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        onProgress: setJobProgress,
        targetCount,
        targets,
        timeoutSecs: timeoutSecsForSnapshot,
      });
      setLastJobProgress(result.progress);
    } finally {
      setJobProgress(null);
    }
  }

  if (ospfUpdatePlans.length === 0) {
    return null;
  }

  return (
    <section className="fleetPanel commandComposer">
      <div className="sectionHeader">
        <div>
          <h2>OSPF cost apply</h2>
          <span>{status}</span>
        </div>
        <ShieldCheck size={20} />
      </div>
      <div className="dispatchForm">
        <div className="dispatchControls">
          <label>
            <span>Update plan</span>
            <select
              aria-label="OSPF update plan"
              onChange={(event) => {
                clearOspfReview();
                setSelectedPlanId(event.target.value);
              }}
              value={selectedUpdatePlan?.plan_id ?? ""}
            >
              {ospfUpdatePlans.map((plan) => (
                <option key={plan.plan_id} value={plan.plan_id}>
                  {plan.plan_name}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Endpoint side</span>
            <select
              aria-label="OSPF update endpoint side"
              onChange={(event) => {
                clearOspfReview();
                setSide(event.target.value as TunnelEndpointSide);
              }}
              value={side}
            >
              <option value="left">Left endpoint</option>
              <option value="right">Right endpoint</option>
            </select>
          </label>
        </div>
        <div className="dispatchControls">
          <label>
            <span>Timeout seconds</span>
            <input
              aria-label="OSPF update timeout seconds"
              max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
              min={1}
              onChange={(event) => {
                clearOspfReview();
                setTimeoutSecs(Number(event.target.value));
              }}
              type="number"
              value={timeoutSecs}
            />
          </label>
        </div>
        {selectedUpdatePlan && (
          <div className="operationNote">
            <strong>
              {selectedUpdatePlan.current_ospf_cost} to {selectedUpdatePlan.recommended_ospf_cost}
            </strong>
            <span>
              {selectedUpdatePlan.interface_name} / {targetClientId ? clientLabel(targetClientId) : "Unknown VPS"}
            </span>
          </div>
        )}
        <TargetImpactPreview
          forceUnprivileged={forceUnprivileged}
          mode="root_network_mutation"
          targets={mutationTargets}
          title="OSPF mutation impact"
        />
        <label className="checkLine">
          <input
            aria-label="Force unprivileged OSPF best effort"
            checked={forceUnprivileged}
            onChange={(event) => {
              clearOspfReview();
              setForceUnprivileged(event.target.checked);
            }}
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
        <ConfirmationPrompt
          confirmLabel="Apply cost"
          detail={ospfSnapshot?.detail ?? ""}
          expiresAtUnix={ospfSnapshot?.privilegeAssertion.expires_unix}
          items={ospfSnapshot?.items ?? []}
          onCancel={() => setOspfSnapshot(null)}
          onConfirm={() => ospfSnapshot && void submitOspfCostUpdate(ospfSnapshot)}
          open={ospfSnapshot !== null}
          pending={pending}
          title="Confirm OSPF cost update"
          tone="danger"
        />
        {visibleJobProgress && (
          <ExecutionResultPanel
            loading={jobProgress !== null}
            onClearResults={clearExecutionResults}
            onOpenJobDetails={onOpenJobDetails}
            progress={visibleJobProgress}
          />
        )}
        <div className="dispatchActions">
          <button className="primaryAction" disabled={!canSubmit} onClick={openOspfPrompt} type="button">
            <Gauge size={17} />
            Review cost apply
          </button>
        </div>
      </div>
      <PrivilegeVaultBox
        clearVaultLabel="Clear OSPF vault"
        labelPrefix="OSPF"
        lastPayloadHash={lastPayloadHash}
        lockPrivilegeLabel="Lock OSPF privilege"
        onOpenUnlock={onOpenPrivilegeUnlock}
        onPrivilegeMaterialChange={(material) => {
          clearOspfReview();
          setPrivilegeMaterial(material);
        }}
        privilegeMaterial={privilegeMaterial}
        unlockRedirectLabel="Unlock OSPF"
        unlockLabel="Unlock OSPF"
        usePrivilegeLabel="Unlock OSPF privilege"
      />
    </section>
  );
}

type OspfUpdateSnapshot = {
  detail: string;
  forceUnprivileged: boolean;
  jobId: string;
  items: Array<{ label: string; value: string }>;
  operation: JobOperation;
  payloadHashHex: string;
  privilegeAssertion: PrivilegeAssertion;
  selectorExpression: string;
  targetClientId: string;
  targets: AgentView[];
  timeoutSecs: number;
};

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}
