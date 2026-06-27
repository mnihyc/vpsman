import {
  Activity,
  ArrowRight,
  FileText,
  GitBranch,
  Map,
  Plus,
  Route,
} from "lucide-react";
import type {
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  TelemetryTunnelRecord,
  TunnelPlanRecord,
} from "../types";
import { formatCompactTime, formatFullTime } from "../utils";

type NetworkOverviewPanelProps = {
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  onCreateTunnelPlan: () => void;
  onSelectSubpage: (
    subpage: "graph" | "tunnel_plans" | "tests" | "ospf" | "evidence",
  ) => void;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
};

type NetworkWorkflow = {
  detail: string;
  icon: JSX.Element;
  label: string;
  metric: string;
  subpage: "graph" | "tunnel_plans" | "tests" | "ospf" | "evidence";
};

export function NetworkOverviewPanel({
  networkObservations,
  networkTrends,
  onCreateTunnelPlan,
  onSelectSubpage,
  ospfRecommendations,
  ospfUpdatePlans,
  telemetryTunnels,
  tunnelPlans,
}: NetworkOverviewPanelProps) {
  const enabledPlans = tunnelPlans.filter((plan) => plan.enabled).length;
  const promotionCandidates = telemetryTunnels.filter(
    (tunnel) => tunnel.promotion_required,
  ).length;
  const degradedTrends = networkTrends.filter(
    (trend) => trend.degraded_count > 0,
  ).length;
  const unhealthyObservations = networkObservations.filter(
    (observation) => observation.healthy === false,
  ).length;
  const ospfDeltaCount = ospfRecommendations.filter(
    (recommendation) => recommendation.cost_delta !== 0,
  ).length;
  const pendingOspfChanges = Math.max(ospfDeltaCount, ospfUpdatePlans.length);
  const latestEvidence = latestIso([
    ...networkObservations.map((observation) => observation.observed_at),
    ...networkTrends.map((trend) => trend.latest_observed_at),
    ...ospfRecommendations.map(
      (recommendation) => recommendation.latest_observed_at,
    ),
    ...ospfUpdatePlans.map((plan) => plan.evidence.latest_observed_at),
    ...telemetryTunnels.map((tunnel) => tunnel.observed_at),
  ]);
  const latestEvidenceTime = latestEvidence
    ? new Date(latestEvidence).getTime()
    : Number.NaN;
  const latestEvidenceStale = Number.isFinite(latestEvidenceTime)
    ? Date.now() - latestEvidenceTime > 24 * 60 * 60 * 1000
    : false;
  const workflows: NetworkWorkflow[] = [
    {
      detail: "Topology map, overlays, and selected endpoint inspection.",
      icon: <Map size={17} />,
      label: "Graph",
      metric: `${tunnelPlans.length} plans`,
      subpage: "graph",
    },
    {
      detail:
        "Plan registry, endpoint allocation, lifecycle, export, and promotion.",
      icon: <GitBranch size={17} />,
      label: "Tunnel plans",
      metric: `${enabledPlans}/${tunnelPlans.length} enabled`,
      subpage: "tunnel_plans",
    },
    {
      detail:
        "Status, probe, and speed-test diagnostics with retained evidence.",
      icon: <Activity size={17} />,
      label: "Tests",
      metric: `${networkTrends.length} trends`,
      subpage: "tests",
    },
    {
      detail: "Review OSPF cost changes from measurements.",
      icon: <Route size={17} />,
      label: "OSPF",
      metric: `${pendingOspfChanges} to review`,
      subpage: "ospf",
    },
    {
      detail:
        "Observations, trend history, job output, and retained network proof.",
      icon: <FileText size={17} />,
      label: "Evidence",
      metric: `${networkObservations.length} observations`,
      subpage: "evidence",
    },
  ];

  return (
    <section className="workspace singleColumn networkOverviewWorkspace">
      <div className="fleetPanel networkOverviewPanel">
        <div className="sectionHeader">
          <div>
            <h2>Network overview</h2>
            <span>
              Tunnel posture, drift, diagnostics, OSPF review, and retained
              evidence.
            </span>
          </div>
          <button
            className="primaryAction compactAction"
            onClick={onCreateTunnelPlan}
            type="button"
          >
            <Plus size={16} />
            <span>Create tunnel</span>
          </button>
        </div>
        <div className="metricGrid" aria-label="Network posture summary">
          <NetworkMetric
            label="Saved plans"
            value={`${enabledPlans}/${tunnelPlans.length}`}
            detail="enabled / total"
          />
          <NetworkMetric
            label="Observed tunnels to save"
            value={promotionCandidates}
            detail="runtime reports not saved as plans"
          />
          <NetworkMetric
            label="Degraded signals"
            value={degradedTrends + unhealthyObservations}
            detail="trend groups plus unhealthy observations"
          />
          <NetworkMetric
            label="OSPF review"
            value={pendingOspfChanges}
            detail="cost changes waiting for review"
          />
        </div>
        <div
          className="networkWorkflowGrid"
          aria-label="Network overview workflow links"
        >
          {workflows.map((workflow) => (
            <button
              className="networkWorkflowButton"
              key={workflow.subpage}
              onClick={() => onSelectSubpage(workflow.subpage)}
              title={workflow.detail}
              type="button"
            >
              <span className="networkWorkflowIcon">{workflow.icon}</span>
              <span className="networkWorkflowText">
                <strong>{workflow.label}</strong>
                <small>{workflow.detail}</small>
              </span>
              <span className="networkWorkflowAction">
                <span>{workflow.metric}</span>
                <ArrowRight size={15} />
              </span>
            </button>
          ))}
        </div>
        <div
          className="networkOverviewEvidence"
          aria-label="Network overview evidence summary"
        >
          <div>
            <strong>Latest evidence</strong>
            <span
              title={
                latestEvidence ? formatFullTime(latestEvidence) : undefined
              }
            >
              {latestEvidence
                ? `${formatCompactTime(latestEvidence)}${latestEvidenceStale ? " · stale" : " · current"}`
                : "No retained network evidence"}
            </span>
          </div>
          <div>
            <strong>Observed tunnels</strong>
            <span>
              {telemetryTunnels.length} telemetry records, {promotionCandidates}{" "}
              observed tunnels available to save
            </span>
          </div>
        </div>
      </div>
    </section>
  );
}

function NetworkMetric({
  detail,
  label,
  value,
}: {
  detail: string;
  label: string;
  value: number | string;
}) {
  return (
    <div className="metricCard" title={detail}>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function latestIso(values: Array<string | null | undefined>): string | null {
  return values.reduce<string | null>((latest, value) => {
    if (!value) {
      return latest;
    }
    const timestamp = new Date(value).getTime();
    if (!Number.isFinite(timestamp)) {
      return latest;
    }
    if (!latest || timestamp > new Date(latest).getTime()) {
      return value;
    }
    return latest;
  }, null);
}
