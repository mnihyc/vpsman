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
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatCompactTime,
  runPanelAction,
  shortId,
} from "../../utils";
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
  onUpdateTunnelPlanOspfCost: (
    planId: string,
    request: UpdateTunnelPlanOspfCostRequest,
  ) => Promise<void>;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [selectedPlanId, setSelectedPlanId] = useState(
    () => ospfUpdatePlans[0]?.plan_id ?? "",
  );
  const [snapshot, setSnapshot] = useState<OspfUpdateSnapshot | null>(null);
  const [appliedRollback, setAppliedRollback] =
    useState<OspfAppliedRollback | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  const selectedUpdatePlan =
    ospfUpdatePlans.find((plan) => plan.plan_id === selectedPlanId) ??
    ospfUpdatePlans[0] ??
    null;
  const selectedTunnelPlan = useMemo(
    () =>
      tunnelPlans.find((plan) => plan.id === selectedUpdatePlan?.plan_id) ??
      null,
    [selectedUpdatePlan?.plan_id, tunnelPlans],
  );
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);
  const selectedTunnelOspfCost =
    selectedTunnelPlan?.plan.recommended_ospf_cost ??
    selectedTunnelPlan?.recommended_ospf_cost ??
    null;
  const hasPendingCostChange =
    !!selectedUpdatePlan &&
    (selectedTunnelOspfCost ?? selectedUpdatePlan.current_ospf_cost) !==
      selectedUpdatePlan.recommended_ospf_cost;
  const planTargets = resolveAgentsById(
    agents,
    selectedUpdatePlan
      ? [selectedUpdatePlan.left_client_id, selectedUpdatePlan.right_client_id]
      : [],
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
    hasPendingCostChange;
  const activeRollback =
    appliedRollback &&
    selectedUpdatePlan?.plan_id === appliedRollback.planId &&
    selectedTunnelOspfCost === appliedRollback.appliedCost
      ? appliedRollback
      : null;
  const canReviewRollback =
    !pending &&
    !snapshot &&
    !!selectedUpdatePlan &&
    !!selectedTunnelPlan &&
    !!activeRollback;
  const costChangeSummary = selectedUpdatePlan
    ? formatCostChange(selectedUpdatePlan)
    : "No cost change";
  const measurementSummary = selectedUpdatePlan
    ? formatMeasurementSummary(selectedUpdatePlan)
    : "No evidence";
  const trafficImpactSummary = selectedUpdatePlan
    ? formatTrafficImpact(selectedUpdatePlan)
    : "No traffic impact";
  const rollbackSummary = activeRollback
    ? `Restore cost ${activeRollback.rollbackCost} on ${activeRollback.interfaceName}`
    : "Rollback available after a successful Apply in this panel";
  const monitoringSummary = selectedUpdatePlan
    ? formatMonitoringPlan(selectedUpdatePlan)
    : "No monitoring plan";
  const baselineMismatch = selectedUpdatePlan
    ? formatBaselineMismatch(selectedUpdatePlan)
    : null;
  const affectedTunnelSummary = selectedUpdatePlan
    ? `${selectedUpdatePlan.interface_name} / ${clientLabel(selectedUpdatePlan.left_client_id)} / ${clientLabel(selectedUpdatePlan.right_client_id)}`
    : "No tunnel selected";

  function openReview(mode: OspfReviewMode) {
    if (!selectedUpdatePlan || !selectedTunnelPlan) {
      setActionError("Select an OSPF update plan");
      return;
    }
    if (!hasPendingCostChange) {
      if (mode === "rollback" && activeRollback) {
        openRollbackReview(selectedUpdatePlan, activeRollback);
        return;
      }
      setActionError("OSPF cost already matches the recommendation");
      return;
    }
    if (mode === "rollback") {
      if (!activeRollback) {
        setActionError(
          "Rollback becomes available after a successful OSPF apply creates a rollback value",
        );
        return;
      }
      openRollbackReview(selectedUpdatePlan, activeRollback);
      return;
    }
    const requestedCost = selectedUpdatePlan.recommended_ospf_cost;
    const currentCost = selectedUpdatePlan.current_ospf_cost;
    setActionError(null);
    setSnapshot({
      appliedCost: requestedCost,
      confirmLabel: "Update cost",
      detail:
        "Update the server-managed tunnel plan cost and push runtime config to both endpoints if the plan is enabled.",
      items: [
        { label: "Action", value: "Apply recommended cost" },
        {
          label: "Recommendation ID",
          value: selectedUpdatePlan.recommendation_id,
        },
        { label: "Plan", value: selectedUpdatePlan.plan_name },
        {
          label: "Endpoints",
          value: `${clientLabel(selectedUpdatePlan.left_client_id)} / ${clientLabel(selectedUpdatePlan.right_client_id)}`,
        },
        { label: "Interface", value: selectedUpdatePlan.interface_name },
        {
          label: "Cost change",
          value: `${currentCost} -> ${requestedCost} (${formatDelta(requestedCost - currentCost)})`,
        },
        { label: "Why", value: selectedUpdatePlan.evidence.reason },
        {
          label: "Evidence summary",
          value: selectedUpdatePlan.evidence_summary,
        },
        {
          label: "Evidence time",
          value: formatEvidenceFreshness(selectedUpdatePlan),
        },
        { label: "Status", value: formatOspfPlanStatus(selectedUpdatePlan) },
        {
          label: "Traffic impact",
          value: formatTrafficImpact(selectedUpdatePlan),
        },
        ...(baselineMismatch
          ? [{ label: "Baseline warning", value: baselineMismatch }]
          : []),
        {
          label: "Rollback plan",
          value: formatRollbackPlan(selectedUpdatePlan),
        },
        {
          label: "Monitor after apply",
          value: formatMonitoringPlan(selectedUpdatePlan),
        },
        {
          label: "Approval scope",
          value: formatApprovalScope(selectedUpdatePlan),
        },
        { label: "Audit event", value: "network.ospf_cost.apply" },
        {
          label: "Sync",
          value: selectedTunnelPlan.enabled ? "Now" : "Deferred",
        },
      ],
      mode: "apply",
      planId: selectedUpdatePlan.plan_id,
      planName: selectedUpdatePlan.plan_name,
      request: {
        confirmed: true,
        mutation_intent: "apply",
        recommendation_id: selectedUpdatePlan.recommendation_id,
        current_ospf_cost: currentCost,
        recommended_ospf_cost: requestedCost,
      },
      recommendationId: selectedUpdatePlan.recommendation_id,
      rollbackCost: currentCost,
      rollbackInterfaceName: selectedUpdatePlan.interface_name,
      title: "Confirm OSPF cost update",
    });
  }

  async function applySnapshot(next: OspfUpdateSnapshot) {
    setSnapshot(null);
    let completed = false;
    await runPanelAction(setPending, setActionError, async () => {
      await onUpdateTunnelPlanOspfCost(next.planId, next.request);
      completed = true;
    });
    if (!completed) {
      return;
    }
    if (next.mode === "apply") {
      setAppliedRollback({
        appliedCost: next.appliedCost,
        interfaceName: next.rollbackInterfaceName,
        planId: next.planId,
        planName: next.planName,
        recommendationId: next.recommendationId,
        rollbackCost: next.rollbackCost,
      });
    } else {
      setAppliedRollback(null);
    }
  }

  function openRollbackReview(
    plan: NetworkOspfUpdatePlanRecord,
    rollback: OspfAppliedRollback,
  ) {
    setActionError(null);
    setSnapshot({
      appliedCost: rollback.rollbackCost,
      confirmLabel: "Rollback cost",
      detail:
        "Restore the previously reviewed OSPF cost from the applied recommendation and push runtime config to both endpoints if the plan is enabled.",
      items: [
        { label: "Action", value: "Rollback applied recommendation" },
        { label: "Recommendation ID", value: rollback.recommendationId },
        { label: "Plan", value: rollback.planName },
        {
          label: "Endpoints",
          value: `${clientLabel(plan.left_client_id)} / ${clientLabel(plan.right_client_id)}`,
        },
        { label: "Interface", value: rollback.interfaceName },
        {
          label: "Cost change",
          value: `${rollback.appliedCost} -> ${rollback.rollbackCost} (${formatDelta(rollback.rollbackCost - rollback.appliedCost)})`,
        },
        { label: "Evidence summary", value: plan.evidence_summary },
        { label: "Monitor after rollback", value: formatMonitoringPlan(plan) },
        { label: "Approval scope", value: formatApprovalScope(plan) },
        { label: "Audit event", value: "network.ospf_cost.rollback" },
        {
          label: "Sync",
          value: selectedTunnelPlan?.enabled ? "Now" : "Deferred",
        },
      ],
      mode: "rollback",
      planId: rollback.planId,
      planName: rollback.planName,
      request: {
        confirmed: true,
        mutation_intent: "rollback",
        recommendation_id: rollback.recommendationId,
        current_ospf_cost: rollback.appliedCost,
        recommended_ospf_cost: rollback.rollbackCost,
      },
      recommendationId: rollback.recommendationId,
      rollbackCost: rollback.appliedCost,
      rollbackInterfaceName: rollback.interfaceName,
      title: "Confirm OSPF rollback",
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
          <>
            <div
              className="ospfRecommendationObject"
              aria-label="Immutable OSPF recommendation"
            >
              <span>
                <small>Recommendation ID</small>
                <strong title={selectedUpdatePlan.recommendation_id}>
                  {shortId(selectedUpdatePlan.recommendation_id)}
                </strong>
              </span>
              <span>
                <small>Current cost</small>
                <strong>{selectedUpdatePlan.current_ospf_cost}</strong>
              </span>
              <span>
                <small>Proposed cost</small>
                <strong>{selectedUpdatePlan.recommended_ospf_cost}</strong>
              </span>
              <span>
                <small>Evidence time</small>
                <strong>{formatEvidenceTime(selectedUpdatePlan)}</strong>
              </span>
              <span>
                <small>Status</small>
                <strong>{formatOspfPlanStatus(selectedUpdatePlan)}</strong>
              </span>
            </div>
            <div
              className="ospfCostReviewStrip"
              aria-label="OSPF cost review evidence"
            >
              <div className="attention">
                <span>Cost change</span>
                <strong>{costChangeSummary}</strong>
                <p>{selectedUpdatePlan.change_summary}</p>
              </div>
              <div>
                <span>Why</span>
                <strong>
                  {formatDisplayToken(selectedUpdatePlan.confidence)}
                </strong>
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
                <p>
                  {baselineMismatch ??
                    formatBandwidthImpact(selectedUpdatePlan)}
                </p>
              </div>
              <div>
                <span>Rollback and monitor</span>
                <strong>{rollbackSummary}</strong>
                <p>{monitoringSummary}</p>
              </div>
            </div>
          </>
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
                <pre>
                  {selectedUpdatePlan.proposed_left_bird2_interface_snippet}
                </pre>
              </div>
              <div>
                <strong>Right endpoint</strong>
                <pre>
                  {selectedUpdatePlan.proposed_right_bird2_interface_snippet}
                </pre>
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
                ? "Confirm the reviewed OSPF recommendation before applying it"
                : "Select an OSPF update plan with a pending cost change"
            }
            type="button"
          >
            <Gauge size={17} />
            Apply cost
          </button>
          <button
            className="secondaryAction"
            disabled={!canReviewRollback}
            onClick={() => openReview("rollback")}
            title={
              canReviewRollback
                ? "Rollback the OSPF recommendation applied in this panel"
                : "Rollback becomes available after a successful Apply creates a rollback value"
            }
            type="button"
          >
            <RotateCcw size={16} />
            Rollback cost
          </button>
        </div>
      </div>
    </section>
  );
}

type OspfReviewMode = "apply" | "rollback";

type OspfUpdateSnapshot = {
  appliedCost: number;
  confirmLabel: string;
  detail: string;
  items: Array<{ label: string; value: string }>;
  mode: OspfReviewMode;
  planId: string;
  planName: string;
  recommendationId: string;
  request: UpdateTunnelPlanOspfCostRequest;
  rollbackCost: number;
  rollbackInterfaceName: string;
  title: string;
};

type OspfAppliedRollback = {
  appliedCost: number;
  interfaceName: string;
  planId: string;
  planName: string;
  recommendationId: string;
  rollbackCost: number;
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
  return `Configured ${formatBandwidthMbps(plan.evidence.configured_bandwidth_mbps)}; effective ${formatBandwidthMbps(plan.evidence.effective_bandwidth_mbps)}. Equal-prefix traffic should prefer lower-cost alternatives when available.`;
}

function formatBaselineMismatch(
  plan: NetworkOspfUpdatePlanRecord,
): string | null {
  const configured = plan.evidence.configured_bandwidth_mbps;
  const effective = plan.evidence.effective_bandwidth_mbps;
  const measured = plan.evidence.throughput_avg_mbps ?? effective;
  if (!configured || !measured || measured >= configured * 0.8) {
    return null;
  }
  const percent = Math.max(1, Math.round((measured / configured) * 100));
  return `${formatMetric(measured)} Mbps is ${percent}% of expected ${configured} Mbps; keep this tunnel less preferred until speed evidence improves.`;
}

function formatRollbackPlan(plan: NetworkOspfUpdatePlanRecord): string {
  return `Restore cost ${plan.current_ospf_cost} on ${plan.interface_name}`;
}

function formatMonitoringPlan(plan: NetworkOspfUpdatePlanRecord): string {
  return `After apply, rerun probe/speed tests and verify ${plan.interface_name} in Evidence.`;
}

function formatEvidenceFreshness(plan: NetworkOspfUpdatePlanRecord): string {
  const latest = formatEvidenceTime(plan);
  const stale = isStaleEvidence(plan.evidence.latest_observed_at)
    ? "Stale evidence: "
    : "";
  return `${stale}${plan.evidence.sample_count} samples, ${plan.evidence.degraded_count} degraded, latest ${latest}`;
}

function formatEvidenceTime(plan: NetworkOspfUpdatePlanRecord): string {
  return plan.evidence.latest_observed_at
    ? formatCompactTime(plan.evidence.latest_observed_at)
    : "No observation time";
}

function isStaleEvidence(value: string | null): boolean {
  if (!value) {
    return true;
  }
  const observed = new Date(value).getTime();
  if (Number.isNaN(observed)) {
    return true;
  }
  return Date.now() - observed > 24 * 60 * 60 * 1000;
}

function formatApprovalScope(plan: NetworkOspfUpdatePlanRecord): string {
  const approval = plan.requires_approval
    ? "approval required"
    : "no approval required";
  const privilege = plan.privilege_required
    ? "privilege required"
    : "no privilege required";
  return `${approval}; ${privilege}; ${formatDisplayToken(plan.mutation_mode)}; ${plan.approval_scope.join(", ")}`;
}

function formatOspfPlanStatus(plan: NetworkOspfUpdatePlanRecord): string {
  if (plan.status === "review_required") {
    return "Review required";
  }
  if (plan.status === "review_degraded") {
    return "Review degraded evidence";
  }
  if (plan.status === "needs_observation") {
    return "Needs fresh observation";
  }
  if (plan.status === "noop") {
    return "No cost change";
  }
  return formatDisplayToken(plan.status);
}

function formatBandwidthMbps(value: number): string {
  return `${Math.round(value)} Mbps`;
}

function formatNullableMetric(value: number | null, unit: string): string {
  return value === null
    ? `${unit} unavailable`
    : `${formatMetric(value)} ${unit}`;
}

function formatLossRatio(value: number | null): string {
  return value === null
    ? "loss unavailable"
    : `${formatMetric(value * 100)}% loss`;
}

function formatMetric(value: number): string {
  return Number.isInteger(value)
    ? String(value)
    : value.toFixed(value < 10 ? 2 : 1);
}

function formatDisplayToken(value: string): string {
  return value.replace(/_/g, " ");
}
