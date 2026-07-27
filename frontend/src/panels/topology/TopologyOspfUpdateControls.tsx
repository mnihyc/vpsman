import { useEffect, useMemo, useState } from "react";
import {
  CircleCheck,
  ExternalLink,
  Gauge,
  RefreshCw,
  ShieldCheck,
  TriangleAlert,
  X,
} from "lucide-react";
import { ActionFeedback, type ActionFeedbackTone } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import {
  TOPOLOGY_EVIDENCE_LIMIT,
  formatLowerBoundCount,
} from "../../constants";
import { sha256Hex } from "../../fileTransfer";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  type PrivilegeMaterial,
} from "../../privilege";
import type {
  AgentView,
  NetworkOspfUpdatePlanRecord,
  TunnelPlanOspfDispatchRecord,
  TunnelPlanOspfJobsResponse,
  TunnelPlanRecord,
  UpdateTunnelPlanOspfCostRequest,
} from "../../types";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  dispatchFailureReason,
  formatCompactTime,
  shortId,
} from "../../utils";

const encoder = new TextEncoder();

export function TopologyOspfUpdateControls({
  agents,
  ospfUpdatePlans,
  onOpenJobDetails,
  onOpenSourceTemplates,
  onOpenTunnelPlans,
  onRefresh,
  onRefreshTunnelPlanOspfStatus,
  onUpdateTunnelPlanOspfCost,
  privilegeMaterial,
  setPrivilegeMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  onOpenJobDetails?: (jobId: string) => void;
  onOpenSourceTemplates: () => void;
  onOpenTunnelPlans: () => void;
  onRefresh: () => Promise<void>;
  onRefreshTunnelPlanOspfStatus: (planId: string) => Promise<TunnelPlanOspfJobsResponse>;
  onUpdateTunnelPlanOspfCost: (
    planId: string,
    request: UpdateTunnelPlanOspfCostRequest,
  ) => Promise<TunnelPlanOspfJobsResponse>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const ospfUpdatePlansTruncated =
    ospfUpdatePlans.length >= TOPOLOGY_EVIDENCE_LIMIT;
  const [expandedPlanId, setExpandedPlanId] = useState<string | null>(
    ospfUpdatePlans[0]?.plan_id ?? null,
  );
  const [pendingPlanId, setPendingPlanId] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const [snapshot, setSnapshot] = useState<ApplySnapshot | null>(null);
  const names = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const plansById = useMemo(
    () => new Map(tunnelPlans.map((plan) => [plan.id, plan])),
    [tunnelPlans],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, names);

  useEffect(() => {
    if (ospfUpdatePlans.length === 0) {
      setExpandedPlanId(null);
      return;
    }
    if (!ospfUpdatePlans.some((plan) => plan.plan_id === expandedPlanId)) {
      setExpandedPlanId(ospfUpdatePlans[0].plan_id);
    }
  }, [expandedPlanId, ospfUpdatePlans]);

  async function refreshData() {
    setRefreshing(true);
    setFeedback({ message: "Refreshing OSPF cost state", tone: "progress" });
    try {
      await onRefresh();
      setFeedback({ message: "OSPF cost state refreshed", tone: "success" });
    } catch (error) {
      setFeedback({
        message: error instanceof Error ? error.message : "OSPF state refresh failed",
        tone: "danger",
      });
    } finally {
      setRefreshing(false);
    }
  }

  async function refreshStatus(plan: NetworkOspfUpdatePlanRecord) {
    setFeedback(null);
    setPendingPlanId(plan.plan_id);
    try {
      const response = await onRefreshTunnelPlanOspfStatus(plan.plan_id);
      setFeedback(ospfDispatchFeedback(
        response.dispatch,
        `Routing adapter checks for ${plan.plan_name}`,
      ));
    } catch (error) {
      setFeedback({
        message: error instanceof Error ? error.message : "Adapter status check failed",
        tone: "danger",
      });
    } finally {
      setPendingPlanId(null);
    }
  }

  function openApplyReview(plan: NetworkOspfUpdatePlanRecord) {
    if (!canApply(plan)) {
      setFeedback({
        message: applyBlockedReason(plan),
        tone: "warning",
      });
      return;
    }
    const request: UpdateTunnelPlanOspfCostRequest = {
      confirmed: true,
      plan_revision: plan.plan_revision,
      desired_ospf_cost: plan.recommended_ospf_cost,
      left_adapter_definition_hash: plan.left_adapter_definition_hash!,
      left_current_ospf_cost: plan.left_current_ospf_cost,
      recommendation_id: plan.recommendation_id,
      right_adapter_definition_hash: plan.right_adapter_definition_hash!,
      right_current_ospf_cost: plan.right_current_ospf_cost,
    };
    setFeedback(null);
    setSnapshot({
      plan,
      request,
      targetClientIds: [plan.left_client_id, plan.right_client_id],
    });
  }

  async function applySnapshot(active: ApplySnapshot) {
    if (!privilegeMaterial) {
      setFeedback({
        message: "Privilege unlock is required before applying routing cost",
        tone: "warning",
      });
      return;
    }
    setFeedback(null);
    setPendingPlanId(active.plan.plan_id);
    try {
      const payloadHash = await sha256Hex(
        encoder.encode(ospfPrivilegePayload(active.plan.plan_id, active.request)),
      );
      const privilegeAssertion = await buildPrivilegeAssertion({
        intent: canonicalDbPrivilegeIntent({
          action: "network.ospf_cost.apply",
          confirmed: true,
          payloadHash,
          resolvedTargets: active.targetClientIds,
          target: `tunnel_plan:${active.plan.plan_id}`,
        }),
        privilegeMaterial,
      });
      const response = await onUpdateTunnelPlanOspfCost(active.plan.plan_id, {
        ...active.request,
        privilege_assertion: privilegeAssertion,
      });
      setSnapshot(null);
      setFeedback(ospfDispatchFeedback(
        response.dispatch,
        `Routing cost update for ${active.plan.plan_name}`,
      ));
    } catch (error) {
      setSnapshot(null);
      setFeedback({
        message: error instanceof Error ? error.message : "Routing cost update failed",
        tone: "danger",
      });
    } finally {
      setPendingPlanId(null);
    }
  }

  if (ospfUpdatePlans.length === 0) {
    return (
      <section className="fleetPanel topologyOspfWorkspace">
        <div className="sectionHeader">
          <div>
            <h2>OSPF cost control</h2>
            <span>No OSPF-enabled tunnel plans</span>
          </div>
          <Gauge size={20} />
        </div>
        <div className="emptyState compactEmptyState">
          <strong>OSPF is opt-in per tunnel plan</strong>
          <span>
            No routing updater is built in. Create an operator-owned adapter contract, then enable OSPF on a tunnel plan and bind one adapter to each endpoint.
          </span>
          <div className="dispatchActions">
            <button className="secondaryAction compactAction" onClick={onOpenSourceTemplates} type="button">
              <ExternalLink size={15} />
              Manage adapters
            </button>
            <button className="primaryAction compactAction" onClick={onOpenTunnelPlans} type="button">
              <Gauge size={15} />
              Open tunnel plans
            </button>
          </div>
        </div>
      </section>
    );
  }

  return (
    <section className="fleetPanel topologyOspfWorkspace">
      <div className="sectionHeader">
        <div>
          <h2>OSPF cost control</h2>
          <span>
            {formatLowerBoundCount(
              ospfUpdatePlans.length,
              ospfUpdatePlansTruncated,
            )}{" "}
            explicit routing adapter workflow
            {ospfUpdatePlans.length === 1 ? "" : "s"}
            {ospfUpdatePlansTruncated ? " loaded" : ""}
          </span>
        </div>
        <div className="headerActionStack">
          <ShieldCheck aria-hidden="true" size={20} />
          <button
            className="secondaryAction compactAction"
            disabled={refreshing}
            onClick={() => void refreshData()}
            title="Refresh saved plans, endpoint cost status, and recommendations"
            type="button"
          >
            <RefreshCw className={refreshing ? "isSpinning" : undefined} size={15} />
            Refresh
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={onOpenSourceTemplates}
            title="Create, validate, or update operator-owned routing cost adapter contracts."
            type="button"
          >
            <ExternalLink size={15} />
            Manage adapters
          </button>
        </div>
      </div>
      <ActionFeedback
        className="localActionFeedback topologyOspfActionFeedback"
        message={feedback?.message}
        tone={feedback?.tone}
      />
      <div className="topologyOspfTableWrap">
        <table aria-label="OSPF adapter update plans" className="topologyOspfTable">
          <thead>
            <tr>
              <th>Plan</th>
              <th>Control</th>
              <th>Current cost</th>
              <th>Recommendation</th>
              <th>Evidence</th>
              <th aria-label="Actions" />
            </tr>
          </thead>
          <tbody>
            {ospfUpdatePlans.map((plan) => {
              const expanded = expandedPlanId === plan.plan_id;
              const pending = pendingPlanId === plan.plan_id;
              const savedPlan = plansById.get(plan.plan_id);
              return (
                <OspfPlanRows
                  clientLabel={clientLabel}
                  expanded={expanded}
                  key={plan.plan_id}
                  onApply={() => openApplyReview(plan)}
                  onOpenJobDetails={onOpenJobDetails}
                  onRefresh={() => void refreshStatus(plan)}
                  onToggle={() =>
                    setExpandedPlanId((current) =>
                      current === plan.plan_id ? null : plan.plan_id,
                    )
                  }
                  pending={pending}
                  plan={plan}
                  savedPlan={savedPlan}
                />
              );
            })}
          </tbody>
        </table>
      </div>
      <ConfirmationPrompt
        confirmDisabled={!privilegeMaterial}
        confirmLabel="Apply routing cost"
        detail={snapshot ? applyConfirmationDetail(snapshot.plan) : "Review the frozen routing cost snapshot."}
        items={snapshot ? confirmationItems(snapshot, clientLabel) : []}
        onCancel={() => setSnapshot(null)}
        onConfirm={() => snapshot && void applySnapshot(snapshot)}
        open={snapshot !== null}
        pending={snapshot ? pendingPlanId === snapshot.plan.plan_id : false}
        title="Confirm OSPF cost update"
        tone={snapshot && isCautionRecommendation(snapshot.plan) ? "warning" : "normal"}
      >
        {snapshot && !privilegeMaterial && (
          <PrivilegeVaultBox
            labelPrefix="OSPF cost"
            lastPayloadHash={null}
            onPrivilegeMaterialChange={setPrivilegeMaterial}
            privilegeMaterial={privilegeMaterial}
            showVaultClear={false}
            usePrivilegeLabel="Unlock OSPF apply"
          />
        )}
      </ConfirmationPrompt>
    </section>
  );
}

function OspfPlanRows({
  clientLabel,
  expanded,
  onApply,
  onOpenJobDetails,
  onRefresh,
  onToggle,
  pending,
  plan,
  savedPlan,
}: {
  clientLabel: (clientId: string) => string;
  expanded: boolean;
  onApply: () => void;
  onOpenJobDetails?: (jobId: string) => void;
  onRefresh: () => void;
  onToggle: () => void;
  pending: boolean;
  plan: NetworkOspfUpdatePlanRecord;
  savedPlan: TunnelPlanRecord | undefined;
}) {
  const mutationReady = canApply(plan);
  return (
    <>
      <tr
        aria-expanded={expanded}
        className={expanded ? "isExpanded" : undefined}
        onClick={onToggle}
        tabIndex={0}
        onKeyDown={(event) => {
          if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            onToggle();
          }
        }}
      >
        <td>
          <span className="historyPrimary">
            <strong title={plan.plan_name}>{plan.plan_name}</strong>
            <small title={plan.interface_name}>{plan.interface_name}</small>
          </span>
        </td>
        <td>
          <span className="historyPrimary">
            <strong
              title={plan.control_mode === "automatic" ? "Automatic" : "Reviewed"}
            >
              {plan.control_mode === "automatic" ? "Automatic" : "Reviewed"}
            </strong>
            <small title={formatUpdateStatus(plan.status)}>
              {formatUpdateStatus(plan.status)}
            </small>
          </span>
        </td>
        <td>
          <span className="topologyEndpointPair">
            <EndpointCost
              cost={plan.left_current_ospf_cost}
              label="L"
              status={plan.left_ospf_status}
            />
            <EndpointCost
              cost={plan.right_current_ospf_cost}
              label="R"
              status={plan.right_ospf_status}
            />
          </span>
        </td>
        <td>
          <span className="historyPrimary">
            <strong title={String(plan.recommended_ospf_cost)}>
              {plan.recommended_ospf_cost}
            </strong>
            <small title={formatPlanDelta(plan)}>
              {formatPlanDelta(plan)}
            </small>
          </span>
        </td>
        <td>
          <span className="historyPrimary">
            <strong title={formatConfidence(plan.confidence)}>
              {formatConfidence(plan.confidence)}
            </strong>
            <small title={plan.evidence_summary}>
              {plan.evidence.sample_count} samples, {plan.evidence.degraded_count} degraded
            </small>
          </span>
        </td>
        <td>
          <div className="topologyRowActions" onClick={(event) => event.stopPropagation()}>
            <button
              aria-label={`Check routing adapter status for ${plan.plan_name}`}
              className="iconAction"
              disabled={pending || !savedPlan?.enabled}
              onClick={onRefresh}
              title={savedPlan?.enabled ? "Check both endpoint adapters" : "Enable the plan before checking adapters"}
              type="button"
            >
              <RefreshCw className={pending ? "isSpinning" : undefined} size={15} />
            </button>
            {plan.control_mode === "reviewed" && (
              <button
                className="primaryAction compactAction"
                disabled={pending || !mutationReady}
                onClick={onApply}
                title={mutationReady ? "Review and apply this adapter-bound cost" : applyBlockedReason(plan)}
                type="button"
              >
                <Gauge size={15} />
                Apply
              </button>
            )}
          </div>
        </td>
      </tr>
      {expanded && (
        <tr className="topologyOspfDetailRow">
          <td colSpan={6}>
            <div className="topologyOspfDetail">
              <button
                aria-label={`Close details for ${plan.plan_name}`}
                className="iconAction topologyDetailClose"
                onClick={onToggle}
                title="Close details"
                type="button"
              >
                <X size={15} />
              </button>
              <div className="topologyOspfFacts">
                <Fact label="Left endpoint" value={`${clientLabel(plan.left_client_id)} · ${plan.left_adapter_template_name ?? "Adapter unavailable"}`} />
                <Fact label="Right endpoint" value={`${clientLabel(plan.right_client_id)} · ${plan.right_adapter_template_name ?? "Adapter unavailable"}`} />
                <Fact label="Current costs" value={`${plan.left_current_ospf_cost ?? "unknown"} / ${plan.right_current_ospf_cost ?? "unknown"}`} />
                <Fact label="Recommendation" value={`${plan.recommended_ospf_cost} · ${formatPlanDelta(plan)}`} />
                <Fact label="Healthy probes" value={`${plan.evidence.healthy_probe_streak} consecutive · ${plan.evidence.required_healthy_probe_streak} required for automatic mode`} />
                <Fact label="Evidence" value={plan.evidence_summary} />
                <Fact label="Controller" value={controllerSummary(plan)} />
              </div>
              <div className="topologyAdapterHashes">
                <code title={plan.left_adapter_definition_hash ?? "No left adapter snapshot"}>
                  L {plan.left_adapter_definition_hash ? shortId(plan.left_adapter_definition_hash) : "unavailable"}
                </code>
                <code title={plan.right_adapter_definition_hash ?? "No right adapter snapshot"}>
                  R {plan.right_adapter_definition_hash ? shortId(plan.right_adapter_definition_hash) : "unavailable"}
                </code>
                <span title={plan.evidence.latest_observed_at ?? "No observation time"}>
                  {plan.evidence.latest_observed_at
                    ? formatCompactTime(plan.evidence.latest_observed_at)
                    : "No observation time"}
                </span>
              </div>
              {(savedPlan?.left_ospf_job_id || savedPlan?.right_ospf_job_id) && (
                <div className="topologyOspfJobLinks" aria-label={`Routing adapter jobs for ${plan.plan_name}`}>
                  {savedPlan.left_ospf_job_id && (
                    <button className="secondaryAction compactAction" onClick={() => onOpenJobDetails?.(savedPlan.left_ospf_job_id!)} title={`Open left adapter job ${savedPlan.left_ospf_job_id}`} type="button">
                      Left adapter job
                    </button>
                  )}
                  {savedPlan.right_ospf_job_id && (
                    <button className="secondaryAction compactAction" onClick={() => onOpenJobDetails?.(savedPlan.right_ospf_job_id!)} title={`Open right adapter job ${savedPlan.right_ospf_job_id}`} type="button">
                      Right adapter job
                    </button>
                  )}
                </div>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  );
}

function EndpointCost({
  cost,
  label,
  status,
}: {
  cost: number | null;
  label: string;
  status: string;
}) {
  const healthy = status === "verified" && cost !== null;
  return (
    <span className={`endpointCost ${healthy ? "isVerified" : "needsCheck"}`} title={`${status}; cost ${cost ?? "unknown"}`}>
      {healthy ? <CircleCheck size={13} /> : <TriangleAlert size={13} />}
      <b>{label}</b>
      <span>{cost ?? "?"}</span>
    </span>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <span>
      <small>{label}</small>
      <strong title={value}>{value}</strong>
    </span>
  );
}

function canApply(plan: NetworkOspfUpdatePlanRecord): boolean {
  return (
    plan.control_mode === "reviewed" &&
    ["review_required", "review_degraded", "review_planned_baseline"].includes(plan.status) &&
    plan.left_ospf_status === "verified" &&
    plan.right_ospf_status === "verified" &&
    Boolean(plan.left_adapter_definition_hash) &&
    Boolean(plan.right_adapter_definition_hash)
  );
}

function applyBlockedReason(plan: NetworkOspfUpdatePlanRecord): string {
  if (!plan.left_adapter_definition_hash || !plan.right_adapter_definition_hash) {
    return "Both routing adapter templates must be available";
  }
  if (plan.left_ospf_status !== "verified" || plan.right_ospf_status !== "verified") {
    return "Check both endpoint adapters before applying a cost";
  }
  if (plan.status === "below_minimum_delta") {
    return "The cost change is below this plan's minimum delta";
  }
  if (plan.status === "noop") {
    return "Both endpoints already report the recommended cost";
  }
  return `Cost apply is unavailable while status is ${formatUpdateStatus(plan.status)}`;
}

function confirmationItems(
  snapshot: ApplySnapshot,
  clientLabel: (clientId: string) => string,
) {
  const { plan, request } = snapshot;
  return [
    { label: "Plan", value: `${plan.plan_name} · ${plan.interface_name} · r${plan.plan_revision}` },
    {
      label: "Endpoints",
      value: `${clientLabel(plan.left_client_id)} / ${clientLabel(plan.right_client_id)}`,
    },
    {
      label: "Current costs",
      value: `${request.left_current_ospf_cost ?? "unknown"} / ${request.right_current_ospf_cost ?? "unknown"}`,
    },
    { label: "Desired cost", value: String(request.desired_ospf_cost) },
    { label: "Review condition", value: formatUpdateStatus(plan.status) },
    { label: "Recommendation", value: request.recommendation_id },
    {
      label: "Adapter snapshots",
      value: `${request.left_adapter_definition_hash} / ${request.right_adapter_definition_hash}`,
    },
    { label: "Evidence", value: plan.evidence_summary },
  ];
}

function isCautionRecommendation(plan: NetworkOspfUpdatePlanRecord): boolean {
  return ["review_degraded", "review_planned_baseline"].includes(plan.status);
}

function applyConfirmationDetail(plan: NetworkOspfUpdatePlanRecord): string {
  const base = "Run the bound routing adapters on both endpoints using this frozen status snapshot. The agent executes only the stored adapter argv and never edits adapter or routing-daemon files itself.";
  if (plan.status === "review_degraded") {
    return `${base} Recent evidence includes degraded samples; apply only after judging that evidence.`;
  }
  if (plan.status === "review_planned_baseline") {
    return `${base} No recent probe evidence is available, so this applies the operator-declared planned baseline.`;
  }
  return base;
}

function ospfPrivilegePayload(
  planId: string,
  request: UpdateTunnelPlanOspfCostRequest,
): string {
  return [
    "v3",
    planId,
    request.plan_revision,
    request.recommendation_id.trim(),
    request.left_current_ospf_cost ?? "none",
    request.right_current_ospf_cost ?? "none",
    request.desired_ospf_cost,
    request.left_adapter_definition_hash,
    request.right_adapter_definition_hash,
  ].join("|");
}

function controllerSummary(plan: NetworkOspfUpdatePlanRecord): string {
  if (plan.control_mode === "reviewed") {
    return "Operator review required for cost changes";
  }
  switch (plan.status) {
    case "automatic_ready":
      return "Server controller will dispatch the adapter update";
    case "automatic_waiting_evidence":
      return `Waiting for ${plan.evidence.required_healthy_probe_streak} consecutive healthy probes; ${plan.evidence.healthy_probe_streak} observed`;
    case "in_progress":
      return "Server-issued adapter jobs are in progress";
    default:
      return `Automatic controller: ${formatUpdateStatus(plan.status)}`;
  }
}

function ospfDispatchFeedback(
  dispatch: TunnelPlanOspfDispatchRecord[],
  action: string,
): Feedback {
  const failures = dispatch.filter((outcome) => outcome.status !== "queued");
  if (failures.length > 0) {
    const reason = failures
      .map(
        (outcome) =>
          `${outcome.endpoint_side} ${outcome.client_id}: ${dispatchFailureReason(outcome.error, outcome.status, "Routing adapter job")}`,
      )
      .join("; ");
    return {
      message: `${action} saved, but ${failures.length} endpoint job${failures.length === 1 ? " was" : "s were"} not queued: ${reason}. Review the endpoint state before retrying.`,
      tone: "warning",
    };
  }
  if (dispatch.length === 0) {
    return {
      message: `${action} did not queue an endpoint job. Refresh the plan state before retrying.`,
      tone: "warning",
    };
  }
  return {
    message: `${action} queued for ${dispatch.length} endpoint${dispatch.length === 1 ? "" : "s"}.`,
    tone: "progress",
  };
}

function formatUpdateStatus(status: string): string {
  const labels: Record<string, string> = {
    adapter_unavailable: "Adapter unavailable",
    automatic_ready: "Ready for controller",
    automatic_waiting_evidence: "Waiting for evidence",
    below_minimum_delta: "Below minimum delta",
    in_progress: "Jobs in progress",
    needs_adapter_status: "Check adapters",
    noop: "Current",
    review_degraded: "Degraded evidence",
    review_planned_baseline: "Planned baseline",
    review_required: "Review required",
  };
  return labels[status] ?? formatConfidence(status);
}

function formatConfidence(value: string): string {
  const normalized = value.replace(/[_-]+/g, " ").trim();
  return normalized ? normalized[0].toUpperCase() + normalized.slice(1) : "Unknown";
}

function formatMaximumDelta(delta: number): string {
  if (delta === 0) return "No endpoint change";
  return `max delta ${delta > 0 ? `+${delta}` : delta}`;
}

function formatPlanDelta(plan: NetworkOspfUpdatePlanRecord): string {
  if (plan.left_current_ospf_cost === null || plan.right_current_ospf_cost === null) {
    return "Initial apply";
  }
  return formatMaximumDelta(plan.maximum_cost_delta);
}

type ApplySnapshot = {
  plan: NetworkOspfUpdatePlanRecord;
  request: UpdateTunnelPlanOspfCostRequest;
  targetClientIds: string[];
};

type Feedback = {
  message: string;
  tone: ActionFeedbackTone;
};
