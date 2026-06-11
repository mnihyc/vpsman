import { useMemo, useState } from "react";
import { Gauge, ShieldCheck } from "lucide-react";
import {
  acceptedDispatchTargetCount,
  formatTargetAvailabilitySummary,
  targetPreflightUnavailable,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../../bulkJobProgress";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "../../privilege";
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
  const [selectedPlanId, setSelectedPlanId] = useState(() => ospfUpdatePlans[0]?.plan_id ?? "");
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [timeoutSecs, setTimeoutSecs] = useState(60);
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
        ? `OSPF update job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} queued`
      : privilegeMaterial
        ? "Ready"
        : "Locked");
  const canSubmit =
    !pending &&
    !promptOpen &&
    !!selectedUpdatePlan &&
    !!selectedTunnelPlan &&
    !!targetClientId &&
    !!privilegeMaterial &&
    selectedUpdatePlan.current_ospf_cost !== selectedUpdatePlan.recommended_ospf_cost;
  const confirmationItems = [
    { label: "Operation", value: "OSPF cost update" },
    { label: "Selector", value: targetClientId ? selectorExpressionForClientIds([targetClientId]) : "-" },
    {
      label: "Targets",
      value: formatTargetAvailabilitySummary(mutationTargets),
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
    { label: "Privilege unlock", value: privilegeMaterial ? "Unlocked locally" : "Locked" },
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
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      const builtOperation = await buildNetworkOspfCostUpdateOperation(
        selectedTunnelPlan.plan,
        side,
        selectedUpdatePlan.current_ospf_cost,
        selectedUpdatePlan.recommended_ospf_cost,
      );
      const endpointTarget = builtOperation.endpoint.localClientId;
      const selectorExpression = selectorExpressionForClientIds([endpointTarget]);
      const boundedTimeoutSecs = clampInteger(timeoutSecs, 1, 3600);
      const builtPrivilege = await buildPrivilegeForJobOperation({
        clientIds: [endpointTarget],
        commandType: "network_ospf_cost_update",
        forceUnprivileged,
        operation: builtOperation.operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: boundedTimeoutSecs,
      });
      const job = await onCreateJob({
        argv: [],
        selector_expression: selectorExpression,
        target_client_ids: [endpointTarget],
        command: "network_ospf_cost_update",
        confirmed: true,
        destructive: true,
        operation: builtOperation.operation,
        force_unprivileged: forceUnprivileged,
        privileged: true,
        privilege_assertion: builtPrivilege.privilegeAssertion,
        timeout_secs: boundedTimeoutSecs,
      });
      setLastPayloadHash(builtPrivilege.payloadHashHex);
      setLastJob(job);
      await trackOspfProgress(job, resolveAgentsById(agents, [endpointTarget]));
    });
  }

  async function trackOspfProgress(job: CreateJobResponse, targets: AgentView[]) {
    const accepted = acceptedDispatchTargetCount(job.target_count, targets);
    setLastJobProgress(null);
    setJobProgress({
      accepted,
      completed: 0,
      doing: accepted,
      expected: targets.length,
      failed: 0,
      jobId: job.job_id,
      retrieved: 0,
      unavailable: targets.filter(targetPreflightUnavailable).length,
    });
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        acceptedTargets: job.target_count,
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
        onPrivilegeMaterialChange={setPrivilegeMaterial}
        privilegeMaterial={privilegeMaterial}
        unlockRedirectLabel="Unlock OSPF"
        unlockLabel="Unlock OSPF"
        usePrivilegeLabel="Unlock OSPF privilege"
      />
    </section>
  );
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}
