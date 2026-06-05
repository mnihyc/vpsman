import { useMemo, useState } from "react";
import { Gauge, ShieldCheck } from "lucide-react";
import { ProofVaultBox } from "../../components/ProofVaultBox";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildEnvelopesForOperation, type ProofMaterial } from "../../proof";
import { buildNetworkOspfCostUpdateOperation } from "../../topologyApply";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
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
  ospfUpdatePlans,
  tunnelPlans,
}: {
  agents: AgentView[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectedPlanId, setSelectedPlanId] = useState(() => ospfUpdatePlans[0]?.plan_id ?? "");
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [timeoutSecs, setTimeoutSecs] = useState(60);
  const [proofTtlSecs, setProofTtlSecs] = useState(300);
  const [confirmed, setConfirmed] = useState(false);
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [proofMaterial, setProofMaterial] = useState<ProofMaterial | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
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
  const status =
    actionError ??
    (lastJob
      ? `OSPF update job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.accepted_targets} accepted`
      : proofMaterial
        ? "Proof unlocked"
        : "Proof locked");
  const canSubmit =
    !pending &&
    !!selectedUpdatePlan &&
    !!selectedTunnelPlan &&
    !!targetClientId &&
    !!proofMaterial &&
    confirmed &&
    selectedUpdatePlan.current_ospf_cost !== selectedUpdatePlan.recommended_ospf_cost;

  async function submitOspfCostUpdate() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedUpdatePlan || !selectedTunnelPlan || !targetClientId) {
        throw new Error("Select an OSPF update plan");
      }
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      if (!confirmed) {
        throw new Error("OSPF cost update requires confirmation");
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
        clients: [endpointTarget],
        command: "network_ospf_cost_update",
        confirmed: true,
        destructive: true,
        envelope: null,
        envelopes: builtProof.envelopes,
        operation: builtOperation.operation,
        force_unprivileged: forceUnprivileged,
        privileged: true,
        tags: [],
        timeout_secs: clampInteger(timeoutSecs, 1, 3600),
      });
      setLastPayloadHash(builtProof.payloadHashHex);
      setLastJob(job);
    });
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
            aria-label="Confirm OSPF cost update"
            checked={confirmed}
            onChange={(event) => setConfirmed(event.target.checked)}
            type="checkbox"
          />
          <span>Confirm OSPF cost update</span>
        </label>
        <label className="checkLine">
          <input
            aria-label="Force unprivileged OSPF best effort"
            checked={forceUnprivileged}
            onChange={(event) => setForceUnprivileged(event.target.checked)}
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
        <div className="dispatchActions">
          <button className="primaryAction" disabled={!canSubmit} onClick={submitOspfCostUpdate} type="button">
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
        onProofMaterialChange={setProofMaterial}
        proofMaterial={proofMaterial}
        unlockLabel="Unlock OSPF"
        useProofLabel="Use OSPF proof"
      />
    </section>
  );
}
