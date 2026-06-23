import { useMemo, useState } from "react";
import { Gauge, ShieldCheck } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { usePanelDisplaySettings } from "../../panelDisplay";
import type {
  AgentView,
  NetworkOspfUpdatePlanRecord,
  TunnelPlanRecord,
  UpdateTunnelPlanOspfCostRequest,
} from "../../types";
import { clientDisplayNameFromMap, clientDisplayNameMap, runPanelAction } from "../../utils";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

export function TopologyOspfUpdateControls({
  agents,
  ospfUpdatePlans,
  tunnelPlans,
  onUpdateTunnelPlanOspfCost,
}: {
  agents: AgentView[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  tunnelPlans: TunnelPlanRecord[];
  onUpdateTunnelPlanOspfCost: (planId: string, request: UpdateTunnelPlanOspfCostRequest) => Promise<void>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectedPlanId, setSelectedPlanId] = useState(() => ospfUpdatePlans[0]?.plan_id ?? "");
  const [snapshot, setSnapshot] = useState<OspfUpdateSnapshot | null>(null);
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
  const planTargets = resolveAgentsById(
    agents,
    selectedUpdatePlan ? [selectedUpdatePlan.left_client_id, selectedUpdatePlan.right_client_id] : [],
  );
  const status =
    actionError ??
    (pending
      ? "Updating OSPF cost"
      : selectedUpdatePlan
        ? `${selectedUpdatePlan.current_ospf_cost} to ${selectedUpdatePlan.recommended_ospf_cost}`
        : "No OSPF cost changes");
  const canReview =
    !pending &&
    !snapshot &&
    !!selectedUpdatePlan &&
    !!selectedTunnelPlan &&
    selectedUpdatePlan.current_ospf_cost !== selectedUpdatePlan.recommended_ospf_cost;

  function openReview() {
    if (!selectedUpdatePlan || !selectedTunnelPlan) {
      setActionError("Select an OSPF update plan");
      return;
    }
    if (selectedUpdatePlan.current_ospf_cost === selectedUpdatePlan.recommended_ospf_cost) {
      setActionError("OSPF cost already matches the recommendation");
      return;
    }
    setActionError(null);
    setSnapshot({
      detail: `Update the server-managed tunnel plan cost and push runtime config to both endpoints if the plan is enabled.`,
      items: [
        { label: "Plan", value: selectedUpdatePlan.plan_name },
        { label: "Endpoints", value: `${clientLabel(selectedUpdatePlan.left_client_id)} / ${clientLabel(selectedUpdatePlan.right_client_id)}` },
        { label: "Interface", value: selectedUpdatePlan.interface_name },
        { label: "Cost", value: `${selectedUpdatePlan.current_ospf_cost} to ${selectedUpdatePlan.recommended_ospf_cost}` },
        { label: "Sync", value: selectedTunnelPlan.enabled ? "Now" : "Deferred" },
      ],
      planId: selectedUpdatePlan.plan_id,
      request: {
        confirmed: true,
        current_ospf_cost: selectedUpdatePlan.current_ospf_cost,
        recommended_ospf_cost: selectedUpdatePlan.recommended_ospf_cost,
      },
    });
  }

  async function applySnapshot(next: OspfUpdateSnapshot) {
    setSnapshot(null);
    await runPanelAction(setPending, setActionError, async () => {
      await onUpdateTunnelPlanOspfCost(next.planId, next.request);
    });
  }

  if (ospfUpdatePlans.length === 0) {
    return null;
  }

  return (
    <section className="fleetPanel commandComposer">
      <div className="sectionHeader">
        <div>
          <h2>OSPF cost</h2>
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
                setSnapshot(null);
                setActionError(null);
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
        </div>
        {selectedUpdatePlan && (
          <div className="operationNote">
            <strong>
              {selectedUpdatePlan.current_ospf_cost} to {selectedUpdatePlan.recommended_ospf_cost}
            </strong>
            <span>
              {selectedUpdatePlan.interface_name} / {clientLabel(selectedUpdatePlan.left_client_id)} / {clientLabel(selectedUpdatePlan.right_client_id)}
            </span>
          </div>
        )}
        <TargetImpactPreview
          mode="generic"
          targets={planTargets}
          title="Plan endpoint visibility"
        />
        <ConfirmationPrompt
          confirmLabel="Update cost"
          detail={snapshot?.detail ?? ""}
          items={snapshot?.items ?? []}
          onCancel={() => setSnapshot(null)}
          onConfirm={() => snapshot && void applySnapshot(snapshot)}
          open={snapshot !== null}
          pending={pending}
          title="Confirm OSPF cost update"
          tone="normal"
        />
        <div className="dispatchActions">
          <button className="primaryAction" disabled={!canReview} onClick={openReview} type="button">
            <Gauge size={17} />
            Review cost update
          </button>
        </div>
      </div>
    </section>
  );
}

type OspfUpdateSnapshot = {
  detail: string;
  items: Array<{ label: string; value: string }>;
  planId: string;
  request: UpdateTunnelPlanOspfCostRequest;
};
