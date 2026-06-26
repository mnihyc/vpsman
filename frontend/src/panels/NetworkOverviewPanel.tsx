import { Activity, FileText, GitBranch, Map, Route } from "lucide-react";
import type {
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  TelemetryTunnelRecord,
  TunnelPlanRecord,
} from "../types";

type NetworkOverviewPanelProps = {
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  onSelectSubpage: (subpage: "graph" | "tunnel_plans" | "tests" | "ospf" | "evidence") => void;
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
  onSelectSubpage,
  ospfRecommendations,
  ospfUpdatePlans,
  telemetryTunnels,
  tunnelPlans,
}: NetworkOverviewPanelProps) {
  const enabledPlans = tunnelPlans.filter((plan) => plan.enabled).length;
  const promotionCandidates = telemetryTunnels.filter((tunnel) => tunnel.promotion_required).length;
  const degradedTrends = networkTrends.filter((trend) => trend.degraded_count > 0).length;
  const unhealthyObservations = networkObservations.filter((observation) => observation.healthy === false).length;
  const ospfDeltaCount = ospfRecommendations.filter((recommendation) => recommendation.cost_delta !== 0).length;
  const pendingOspfChanges = Math.max(ospfDeltaCount, ospfUpdatePlans.length);
  const latestEvidence = networkObservations[0]?.observed_at ?? networkTrends[0]?.latest_observed_at ?? null;
  const workflows: NetworkWorkflow[] = [
    {
      detail: "Topology map, overlays, and selected endpoint inspection.",
      icon: <Map size={17} />,
      label: "Graph",
      metric: `${tunnelPlans.length} plans`,
      subpage: "graph",
    },
    {
      detail: "Plan registry, endpoint allocation, lifecycle, export, and promotion.",
      icon: <GitBranch size={17} />,
      label: "Tunnel Plans",
      metric: `${enabledPlans}/${tunnelPlans.length} enabled`,
      subpage: "tunnel_plans",
    },
    {
      detail: "Status, probe, and speed-test diagnostics with retained evidence.",
      icon: <Activity size={17} />,
      label: "Tests",
      metric: `${networkTrends.length} trends`,
      subpage: "tests",
    },
    {
      detail: "Reviewed cost updates, rollback planning, and BIRD2 diff evidence.",
      icon: <Route size={17} />,
      label: "OSPF",
      metric: `${pendingOspfChanges} pending`,
      subpage: "ospf",
    },
    {
      detail: "Observations, trend history, job output, and retained network proof.",
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
            <span>Tunnel posture, drift, diagnostics, OSPF review, and retained evidence.</span>
          </div>
          <GitBranch size={20} />
        </div>
        <div className="metricGrid" aria-label="Network posture summary">
          <NetworkMetric label="Saved plans" value={`${enabledPlans}/${tunnelPlans.length}`} detail="enabled / total" />
          <NetworkMetric label="Promotion candidates" value={promotionCandidates} detail="observed tunnels needing plan review" />
          <NetworkMetric label="Degraded signals" value={degradedTrends + unhealthyObservations} detail="trend groups plus unhealthy observations" />
          <NetworkMetric label="OSPF review" value={pendingOspfChanges} detail="recommended or staged cost changes" />
        </div>
        <div className="networkWorkflowGrid" aria-label="Network overview workflow links">
          {workflows.map((workflow) => (
            <button
              className="networkWorkflowButton"
              key={workflow.subpage}
              onClick={() => onSelectSubpage(workflow.subpage)}
              type="button"
            >
              <span className="networkWorkflowIcon">{workflow.icon}</span>
              <span className="networkWorkflowText">
                <strong>{workflow.label}</strong>
                <small>{workflow.detail}</small>
              </span>
              <span className="networkWorkflowMetric">{workflow.metric}</span>
            </button>
          ))}
        </div>
        <div className="networkOverviewEvidence" aria-label="Network overview evidence summary">
          <div>
            <strong>Latest evidence</strong>
            <span>{latestEvidence ? latestEvidence : "No retained network evidence"}</span>
          </div>
          <div>
            <strong>Observed tunnels</strong>
            <span>{telemetryTunnels.length} telemetry records, {promotionCandidates} requiring promotion review</span>
          </div>
        </div>
      </div>
    </section>
  );
}

function NetworkMetric({ detail, label, value }: { detail: string; label: string; value: number | string }) {
  return (
    <div className="metricCard">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}
