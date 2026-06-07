import { useMemo, useState } from "react";
import { Gauge, ShieldCheck } from "lucide-react";
import { waitForBulkJobTargets, type BulkJobProgress } from "../../bulkJobProgress";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../../components/ExecutionResultPanel";
import { ProofVaultBox } from "../../components/ProofVaultBox";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildEnvelopesForOperation, type ProofMaterial } from "../../proof";
import { selectorExpressionForClientIds } from "../../searchExpression";
import { buildNetworkOspfCostUpdateOperation } from "../../topologyApply";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  JobTargetRecord,
  NetworkOspfUpdatePlanRecord,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import { clientDisplayNameFromMap, clientDisplayNameMap, runPanelAction, shortId } from "../../utils";
import { clampInteger } from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

export function TopologyOspfUpdateControls({
  agents,
  onCreateJob,
  onLoadTargets,
  onOpenJobDetails,
  onOpenProofUnlock,
  ospfUpdatePlans,
  proofMaterial,
  setProofMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenProofUnlock: () => void;
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  proofMaterial: ProofMaterial | null;
  setProofMaterial: (material: ProofMaterial | null) => void;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectedPlanId, setSelectedPlanId] = useState(() => ospfUpdatePlans[0]?.plan_id ?? "");
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [timeoutSecs, setTimeoutSecs] = useState(60);
  const [proofTtlSecs, setProofTtlSecs] = useState(300);
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [promptOpen, setPromptOpen] = useState(false);
  const [jobProgress, setJobProgress] = useState<BulkJobProgress | null>(null);
  const [lastJobProgress, setLastJobProgress] = useState<BulkJobProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

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
    (visibleJobProgress
      ? `OSPF result for job ${shortId(visibleJobProgress.jobId)}`
      : lastJob
        ? `OSPF update job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.accepted_targets} pushed`
      : proofMaterial
        ? "Ready"
        : "Locked");
  const canSubmit =
    !pending &&
    !promptOpen &&
    !!selectedUpdatePlan &&
    !!selectedTunnelPlan &&
    !!targetClientId &&
    !!proofMaterial &&
    selectedUpdatePlan.current_ospf_cost !== selectedUpdatePlan.recommended_ospf_cost;
  const connectedTargets = mutationTargets.filter((target) => target.status === "connected").length;
  const unavailableTargets = Math.max(0, mutationTargets.length - connectedTargets);
  const confirmationItems = [
    { label: "Operation", value: "OSPF cost update" },
    { label: "Selector", value: targetClientId ? selectorExpressionForClientIds([targetClientId]) : "-" },
    {
      label: "Targets",
      value:
        unavailableTargets > 0
          ? `${mutationTargets.length} resolved (${connectedTargets} connected, ${unavailableTargets} unavailable)`
          : `${mutationTargets.length} resolved`,
    },
    { label: "Plan", value: selectedUpdatePlan?.plan_name ?? "-" },
    { label: "Endpoint", value: side },
    {
      label: "Cost",
      value: selectedUpdatePlan
        ? `${selectedUpdatePlan.current_ospf_cost} to ${selectedUpdatePlan.recommended_ospf_cost}`
        : "-",
    },
    { label: "Timeout", value: `${clampInteger(timeoutSecs, 1, 3600)}s` },
    { label: "Proof TTL", value: `${clampInteger(proofTtlSecs, 15, 3600)}s` },
    { label: "Privilege", value: forceUnprivileged ? "Forced best effort" : "Root required" },
  ];

  function openOspfPrompt() {
    setActionError(null);
    if (!canSubmit) {
      return;
    }
    setPromptOpen(true);
  }

  function clearExecutionResults() {
    setJobProgress(null);
    setLastJobProgress(null);
    setLastJob(null);
  }

  async function submitOspfCostUpdate() {
    setPromptOpen(false);
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedUpdatePlan || !selectedTunnelPlan || !targetClientId) {
        throw new Error("Select an OSPF update plan");
      }
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      const builtOperation = await buildNetworkOspfCostUpdateOperation(
        selectedTunnelPlan.plan,
        side,
        selectedUpdatePlan.current_ospf_cost,
        selectedUpdatePlan.recommended_ospf_cost,
      );
      const endpointTarget = builtOperation.endpoint.localClientId;
      const builtProof = await buildEnvelopesForOperation({
        clientIds: [endpointTarget],
        operation: builtOperation.operation,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      const job = await onCreateJob({
        argv: [],
        selector_expression: selectorExpressionForClientIds([endpointTarget]),
        command: "network_ospf_cost_update",
        confirmed: true,
        destructive: true,
        envelope: null,
        envelopes: builtProof.envelopes,
        operation: builtOperation.operation,
        force_unprivileged: forceUnprivileged,
        privileged: true,
        timeout_secs: clampInteger(timeoutSecs, 1, 3600),
      });
      setLastPayloadHash(builtProof.payloadHashHex);
      setLastJob(job);
      await trackOspfProgress(job, resolveAgentsById(agents, [endpointTarget]));
    });
  }

  async function trackOspfProgress(job: CreateJobResponse, targets: AgentView[]) {
    setLastJobProgress(null);
    setJobProgress({
      accepted: Math.min(job.accepted_targets, targets.length),
      completed: 0,
      doing: Math.min(job.accepted_targets, targets.length),
      expected: targets.length,
      failed: 0,
      jobId: job.job_id,
      retrieved: 0,
      unavailable: targets.filter((target) => target.status !== "connected").length,
    });
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        acceptedTargets: job.accepted_targets,
        onProgress: setJobProgress,
        targets,
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
              onChange={(event) => setSelectedPlanId(event.target.value)}
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
              onChange={(event) => setSide(event.target.value as TunnelEndpointSide)}
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
              max={3600}
              min={1}
              onChange={(event) => setTimeoutSecs(Number(event.target.value))}
              type="number"
              value={timeoutSecs}
            />
          </label>
          <label>
            <span>Proof TTL seconds</span>
            <input
              aria-label="OSPF update proof TTL seconds"
              max={3600}
              min={15}
              onChange={(event) => setProofTtlSecs(Number(event.target.value))}
              type="number"
              value={proofTtlSecs}
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
            onChange={(event) => setForceUnprivileged(event.target.checked)}
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
        <ConfirmationPrompt
          confirmLabel="Apply cost"
          detail={`OSPF cost update on ${vpsCountLabel(mutationTargets.length)}.`}
          items={confirmationItems}
          onCancel={() => setPromptOpen(false)}
          onConfirm={() => void submitOspfCostUpdate()}
          open={promptOpen}
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
            Apply cost
          </button>
        </div>
      </div>
      <ProofVaultBox
        clearVaultLabel="Clear OSPF vault"
        labelPrefix="OSPF"
        lastPayloadHash={lastPayloadHash}
        lockProofLabel="Lock OSPF proof"
        onOpenUnlock={onOpenProofUnlock}
        onProofMaterialChange={setProofMaterial}
        proofMaterial={proofMaterial}
        unlockRedirectLabel="Unlock OSPF"
        unlockLabel="Unlock OSPF"
        useProofLabel="Use OSPF proof"
      />
    </section>
  );
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}
