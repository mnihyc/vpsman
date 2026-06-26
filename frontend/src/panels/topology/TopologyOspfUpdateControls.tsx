import { useMemo, useState } from "react";
import { Gauge, RotateCcw, ShieldCheck } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { usePanelDisplaySettings } from "../../panelDisplay";
import type {
  AgentView,
  NetworkOspfUpdatePlanRecord,
  TunnelPlanRecord,
  UpdateTunnelPlanOspfCostRequest,
} from "../../types";
import { clientDisplayNameFromMap, clientDisplayNameMap, formatTime, runPanelAction } from "../../utils";
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
  const costChangeSummary = selectedUpdatePlan ? formatCostChange(selectedUpdatePlan) : "No cost change";
  const measurementSummary = selectedUpdatePlan ? formatMeasurementSummary(selectedUpdatePlan) : "No evidence";
  const trafficImpactSummary = selectedUpdatePlan ? formatTrafficImpact(selectedUpdatePlan) : "No traffic impact";
  const rollbackSummary = selectedUpdatePlan ? formatRollbackPlan(selectedUpdatePlan) : "No rollback plan";
  const monitoringSummary = selectedUpdatePlan ? formatMonitoringPlan(selectedUpdatePlan) : "No monitoring plan";
  const affectedTunnelSummary = selectedUpdatePlan
    ? `${selectedUpdatePlan.interface_name} / ${clientLabel(selectedUpdatePlan.left_client_id)} / ${clientLabel(selectedUpdatePlan.right_client_id)}`
    : "No tunnel selected";

  function openReview(mode: OspfReviewMode) {
    if (!selectedUpdatePlan || !selectedTunnelPlan) {
      setActionError("Select an OSPF update plan");
      return;
    }
    if (selectedUpdatePlan.current_ospf_cost === selectedUpdatePlan.recommended_ospf_cost) {
      setActionError("OSPF cost already matches the recommendation");
      return;
    }
    const requestedCost =
      mode === "apply"
        ? selectedUpdatePlan.recommended_ospf_cost
        : selectedUpdatePlan.current_ospf_cost;
    const currentCost =
      mode === "apply"
        ? selectedUpdatePlan.current_ospf_cost
        : selectedUpdatePlan.recommended_ospf_cost;
    const actionLabel = mode === "apply" ? "Apply recommended cost" : "Rollback to prior cost";
    setActionError(null);
    setSnapshot({
      confirmLabel: mode === "apply" ? "Update cost" : "Rollback cost",
      detail:
        mode === "apply"
          ? "Update the server-managed tunnel plan cost and push runtime config to both endpoints if the plan is enabled."
          : "Restore the server-managed tunnel plan cost to the prior value and push runtime config to both endpoints if the plan is enabled.",
      items: [
        { label: "Action", value: actionLabel },
        { label: "Plan", value: selectedUpdatePlan.plan_name },
        { label: "Endpoints", value: `${clientLabel(selectedUpdatePlan.left_client_id)} / ${clientLabel(selectedUpdatePlan.right_client_id)}` },
        { label: "Interface", value: selectedUpdatePlan.interface_name },
        { label: "Cost change", value: `${currentCost} -> ${requestedCost} (${formatDelta(requestedCost - currentCost)})` },
        { label: "Why", value: selectedUpdatePlan.evidence.reason },
        { label: "Measurements", value: formatMeasurementSummary(selectedUpdatePlan) },
        { label: "Confidence", value: formatDisplayToken(selectedUpdatePlan.confidence) },
        { label: "Traffic impact", value: formatTrafficImpact(selectedUpdatePlan) },
        { label: "Rollback plan", value: formatRollbackPlan(selectedUpdatePlan) },
        { label: mode === "apply" ? "Monitor after apply" : "Monitor after rollback", value: formatMonitoringPlan(selectedUpdatePlan) },
        { label: "Approval scope", value: formatApprovalScope(selectedUpdatePlan) },
        { label: "Audit event", value: mode === "apply" ? "network.ospf_cost.apply" : "network.ospf_cost.rollback" },
        { label: "Sync", value: selectedTunnelPlan.enabled ? "Now" : "Deferred" },
      ],
      mode,
      planId: selectedUpdatePlan.plan_id,
      request: {
        confirmed: true,
        current_ospf_cost: currentCost,
        recommended_ospf_cost: requestedCost,
      },
      title: mode === "apply" ? "Confirm OSPF cost update" : "Confirm OSPF rollback",
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
          <div className="ospfCostReviewStrip" aria-label="OSPF cost review evidence">
            <div className="attention">
              <span>Cost change</span>
              <strong>{costChangeSummary}</strong>
              <p>{selectedUpdatePlan.change_summary}</p>
            </div>
            <div>
              <span>Why</span>
              <strong>{formatDisplayToken(selectedUpdatePlan.confidence)}</strong>
              <p>{selectedUpdatePlan.evidence.reason}</p>
            </div>
            <div>
              <span>Measurements</span>
              <strong>{measurementSummary}</strong>
              <p>{formatEvidenceFreshness(selectedUpdatePlan)}</p>
            </div>
            <div>
              <span>Affected tunnel</span>
              <strong>{affectedTunnelSummary}</strong>
              <p>{selectedUpdatePlan.bird2_file}</p>
            </div>
            <div className="attention">
              <span>Traffic impact</span>
              <strong>{trafficImpactSummary}</strong>
              <p>{formatBandwidthImpact(selectedUpdatePlan)}</p>
            </div>
            <div>
              <span>Rollback and monitor</span>
              <strong>{rollbackSummary}</strong>
              <p>{monitoringSummary}</p>
            </div>
          </div>
        )}
        <TargetImpactPreview
          mode="generic"
          targets={planTargets}
          title="Plan endpoint visibility"
        />
        {selectedUpdatePlan && (
          <details className="operationNote topologyEvidenceDisclosure">
            <summary>Proposed Bird2 interface snippets</summary>
            <div className="ospfSnippetGrid">
              <div>
                <strong>Left endpoint</strong>
                <pre>{selectedUpdatePlan.proposed_left_bird2_interface_snippet}</pre>
              </div>
              <div>
                <strong>Right endpoint</strong>
                <pre>{selectedUpdatePlan.proposed_right_bird2_interface_snippet}</pre>
              </div>
            </div>
          </details>
        )}
        <ConfirmationPrompt
          confirmLabel={snapshot?.confirmLabel ?? "Update cost"}
          detail={snapshot?.detail ?? ""}
          items={snapshot?.items ?? []}
          onCancel={() => setSnapshot(null)}
          onConfirm={() => snapshot && void applySnapshot(snapshot)}
          open={snapshot !== null}
          pending={pending}
          title={snapshot?.title ?? "Confirm OSPF cost update"}
          tone="normal"
        />
        <div className="dispatchActions">
          <button
            className="primaryAction"
            disabled={!canReview}
            onClick={() => openReview("apply")}
            title={
              canReview
                ? "Review OSPF cost change with evidence, impact, rollback, and monitoring context"
                : "Select an OSPF update plan with a pending cost change"
            }
            type="button"
          >
            <Gauge size={17} />
            Review cost update
          </button>
          <button
            className="secondaryAction"
            disabled={!canReview}
            onClick={() => openReview("rollback")}
            title={
              canReview
                ? "Review rollback to the prior OSPF cost with the same scope and audit evidence"
                : "Select an OSPF update plan with a pending cost change"
            }
            type="button"
          >
            <RotateCcw size={16} />
            Review rollback
          </button>
        </div>
      </div>
    </section>
  );
}

type OspfReviewMode = "apply" | "rollback";

type OspfUpdateSnapshot = {
  confirmLabel: string;
  detail: string;
  items: Array<{ label: string; value: string }>;
  mode: OspfReviewMode;
  planId: string;
  request: UpdateTunnelPlanOspfCostRequest;
  title: string;
};

function formatCostChange(plan: NetworkOspfUpdatePlanRecord): string {
  return `${plan.current_ospf_cost} -> ${plan.recommended_ospf_cost} (${formatDelta(plan.cost_delta)})`;
}

function formatDelta(delta: number): string {
  return delta > 0 ? `+${delta}` : String(delta);
}

function formatMeasurementSummary(plan: NetworkOspfUpdatePlanRecord): string {
  const evidence = plan.evidence;
  return [
    formatNullableMetric(evidence.latency_avg_ms, "ms avg"),
    formatLossRatio(evidence.packet_loss_avg_ratio),
    formatNullableMetric(evidence.throughput_avg_mbps, "Mbps avg"),
    formatNullableMetric(evidence.throughput_max_mbps, "Mbps max"),
  ].join("; ");
}

function formatTrafficImpact(plan: NetworkOspfUpdatePlanRecord): string {
  if (plan.cost_delta > 0) {
    return `Less preferred by ${plan.cost_delta}`;
  }
  if (plan.cost_delta < 0) {
    return `More preferred by ${Math.abs(plan.cost_delta)}`;
  }
  return "No preference change";
}

function formatBandwidthImpact(plan: NetworkOspfUpdatePlanRecord): string {
  return `Configured ${formatBandwidthTier(plan.evidence.configured_bandwidth)}; effective ${formatBandwidthTier(plan.evidence.effective_bandwidth)}. Equal-prefix traffic should prefer lower-cost alternatives when available.`;
}

function formatRollbackPlan(plan: NetworkOspfUpdatePlanRecord): string {
  return `Restore cost ${plan.current_ospf_cost} on ${plan.interface_name}`;
}

function formatMonitoringPlan(plan: NetworkOspfUpdatePlanRecord): string {
  return `After apply, rerun probe/speed tests and verify ${plan.interface_name} in Evidence.`;
}

function formatEvidenceFreshness(plan: NetworkOspfUpdatePlanRecord): string {
  const latest = plan.evidence.latest_observed_at
    ? formatTime(plan.evidence.latest_observed_at)
    : "No observation time";
  return `${plan.evidence.sample_count} samples, ${plan.evidence.degraded_count} degraded, latest ${latest}`;
}

function formatApprovalScope(plan: NetworkOspfUpdatePlanRecord): string {
  const approval = plan.requires_approval ? "approval required" : "no approval required";
  const privilege = plan.privilege_required ? "privilege required" : "no privilege required";
  return `${approval}; ${privilege}; ${formatDisplayToken(plan.mutation_mode)}; ${plan.approval_scope.join(", ")}`;
}

function formatBandwidthTier(value: string): string {
  if (value === "1000m") {
    return "1000 Mbps";
  }
  if (value === "100m") {
    return "100 Mbps";
  }
  if (value === "10m") {
    return "10 Mbps";
  }
  return value;
}

function formatNullableMetric(value: number | null, unit: string): string {
  return value === null ? `${unit} unavailable` : `${formatMetric(value)} ${unit}`;
}

function formatLossRatio(value: number | null): string {
  return value === null ? "loss unavailable" : `${formatMetric(value * 100)}% loss`;
}

function formatMetric(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(value < 10 ? 2 : 1);
}

function formatDisplayToken(value: string): string {
  return value.replace(/_/g, " ");
}
