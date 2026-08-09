import { useEffect, useMemo, useRef, useState } from "react";
import {
  GitGraph,
  Maximize2,
  RefreshCcw,
  Search,
  Server,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { topologyEdgeHealthStatusBadgeClass } from "../../jobStatusPresentation";
import { ActionFeedback } from "../../components/ActionFeedback";
import { NetworkEvidenceRangeControls } from "../../components/NetworkEvidenceRangeControls";
import type { MonitoringWindow } from "../../components/MonitoringRangeTabs";
import {
  DEFAULT_NETWORK_EVIDENCE_WINDOW,
  defaultNetworkEvidenceEndAt,
  defaultNetworkEvidenceStartAt,
  networkEvidenceWindowLabel,
  type NetworkEvidenceQuery,
} from "../../networkEvidence";
import { consolePalette } from "../../colorPalette";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { agentDisplayState } from "../../agentDisplayState";
import {
  OSPF_COST_MODEL_DETAIL,
  OSPF_COST_MODEL_SUMMARY,
  readableTelemetryToken,
} from "../../topologyRuntime";
import type {
  AgentView,
  RuntimeConfigApplyStateRecord,
  TopologyEdgeHealthStatus,
  TopologyGraph,
  TopologyGraphEdge,
  TopologyGraphNode,
} from "../../types";
import {
  dispatchFailureReason,
  formatCompactTime,
  formatFullTime,
  formatTime,
  formatVpsName,
  shortId,
  timestampMillis,
  type VpsNameDisplayMode,
} from "../../utils";

type PositionedNode = TopologyGraphNode & {
  x: number;
  y: number;
};

type GraphLayout = {
  height: number;
  nodes: PositionedNode[];
};

type HealthFilter = "all" | "attention" | "healthy" | "unknown" | "disabled";
type GraphPan = {
  x: number;
  y: number;
};
type GraphLegendItem = {
  detail: string;
  label: string;
  tone?: "attention" | "ready";
  value: string;
};
type NodeTunnelStats = {
  attention: number;
  disabled: number;
  enabled: number;
  healthy: number;
  total: number;
};

const EMPTY_NODE_TUNNEL_STATS: NodeTunnelStats = {
  attention: 0,
  disabled: 0,
  enabled: 0,
  healthy: 0,
  total: 0,
};

const healthFilters: { label: string; value: HealthFilter }[] = [
  { label: "All", value: "all" },
  { label: "Attention", value: "attention" },
  { label: "Healthy", value: "healthy" },
  { label: "Unknown", value: "unknown" },
  { label: "Disabled", value: "disabled" },
];

export function TopologyGraphPanel({
  agents,
  error,
  graph,
  initialSelectedClientId,
  initialSelectionRequestId,
  loading,
  onInitialSelectionConsumed,
  onOpenVpsDetail,
  onRefresh,
  runtimeConfigEvidenceState,
  runtimeConfigApplyStates,
}: {
  agents: AgentView[];
  error: string | null;
  graph: TopologyGraph;
  initialSelectedClientId?: string | null;
  initialSelectionRequestId?: string | null;
  loading: boolean;
  onInitialSelectionConsumed?: (requestId: string) => void;
  onOpenVpsDetail?: (clientId: string) => void;
  onRefresh: (query?: NetworkEvidenceQuery) => Promise<void>;
  runtimeConfigEvidenceState: "available" | "loading" | "unavailable";
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [query, setQuery] = useState("");
  const [healthFilter, setHealthFilter] = useState<HealthFilter>("all");
  const [planFilter, setPlanFilter] = useState("");
  const [evidenceWindow, setEvidenceWindow] = useState<MonitoringWindow>(
    DEFAULT_NETWORK_EVIDENCE_WINDOW,
  );
  const [customStartAt, setCustomStartAt] = useState(
    defaultNetworkEvidenceStartAt,
  );
  const [customEndAt, setCustomEndAt] = useState(defaultNetworkEvidenceEndAt);
  const [selectedClientId, setSelectedClientId] = useState<string | null>(null);
  const appliedInitialSelectionRequestRef = useRef<string | null>(null);
  const [graphZoom, setGraphZoom] = useState(1);
  const [graphPan, setGraphPan] = useState<GraphPan>({ x: 0, y: 0 });
  const [mobileGraphOpen, setMobileGraphOpen] = useState(false);
  const agentById = useMemo(
    () => new Map(agents.map((agent) => [agent.id, agent])),
    [agents],
  );
  const filtered = useMemo(
    () => filterGraph(graph, query, healthFilter, planFilter),
    [graph, healthFilter, planFilter, query],
  );
  const layout = useMemo(() => positionNodes(filtered.nodes), [filtered.nodes]);
  const nodes = layout.nodes;
  const nodeById = new Map(nodes.map((node) => [node.client_id, node]));
  const nodeStatsById = buildNodeTunnelStats(filtered.edges);
  const selectedNode =
    selectedClientId === null
      ? (nodes[0] ?? null)
      : (nodes.find((node) => node.client_id === selectedClientId) ?? null);
  const selectedEdges = selectedNode
    ? filtered.edges.filter(
        (edge) =>
          edge.left_client_id === selectedNode.client_id ||
          edge.right_client_id === selectedNode.client_id,
      )
    : [];
  const selectedNodeStats = selectedNode
    ? (nodeStatsById.get(selectedNode.client_id) ?? EMPTY_NODE_TUNNEL_STATS)
    : EMPTY_NODE_TUNNEL_STATS;
  const runtimeStateByClientId = useMemo(
    () =>
      new Map(
        runtimeConfigApplyStates.map((state) => [state.client_id, state]),
      ),
    [runtimeConfigApplyStates],
  );
  const selectedRuntimeState = selectedNode
    ? (runtimeStateByClientId.get(selectedNode.client_id) ?? null)
    : null;
  const selectedDisplayState = selectedNode
    ? nodeDisplayState(selectedNode, agentById)
    : null;
  const showEdgeLabels = filtered.edges.length <= 14 && nodes.length <= 12;
  const graphTransform = graphTransformFor(graphZoom, graphPan, layout.height);
  const latestTopologyEvidence = useMemo(
    () => latestTopologyEvidenceAt(graph),
    [graph],
  );
  const latestTopologyEvidenceStale = isStaleEvidence(latestTopologyEvidence);

  useEffect(() => {
    if (
      !initialSelectionRequestId ||
      appliedInitialSelectionRequestRef.current === initialSelectionRequestId ||
      (loading && graph.nodes.length === 0)
    ) {
      return;
    }
    appliedInitialSelectionRequestRef.current = initialSelectionRequestId;
    setSelectedClientId(initialSelectedClientId ?? null);
    onInitialSelectionConsumed?.(initialSelectionRequestId);
  }, [
    graph.nodes,
    initialSelectedClientId,
    initialSelectionRequestId,
    loading,
    onInitialSelectionConsumed,
  ]);
  const legendItems = useMemo(
    () => buildGraphLegendItems(filtered.edges),
    [filtered.edges],
  );
  const hasVisibleOspfCost = filtered.edges.some(
    (edge) => edge.enabled && edge.recommended_ospf_cost !== null,
  );
  const showMinimap = filtered.edges.length > 10 || nodes.length > 8;
  const planOptions = useMemo(
    () =>
      graph.edges
        .map((edge) => ({ id: edge.plan_id, name: edge.plan_name }))
        .sort((left, right) => left.name.localeCompare(right.name)),
    [graph.edges],
  );
  const status =
    graph.edges.length === 0
      ? "No topology edges"
      : `${filtered.nodes.length} of ${graph.nodes.length} plan endpoints shown; ${filtered.edges.length} of ${graph.edges.length} ${graph.edges.length === 1 ? "tunnel" : "tunnels"} shown in ${networkEvidenceWindowLabel(evidenceWindow)}`;

  function evidenceQuery(
    windowOverride: MonitoringWindow = evidenceWindow,
  ): NetworkEvidenceQuery {
    return {
      endAt: customEndAt,
      startAt: customStartAt,
      window: windowOverride,
    };
  }

  function selectEvidenceWindow(next: MonitoringWindow) {
    setEvidenceWindow(next);
    if (next !== "custom") {
      void onRefresh(evidenceQuery(next));
    }
  }

  return (
    <section
      className={`fleetPanel topologyGraphPanel ${mobileGraphOpen ? "mobileGraphOpen" : ""}`}
    >
      <div className="sectionHeader">
        <div>
          <div className="topologyGraphTitleRow">
            <h2>Topology graph</h2>
            {latestTopologyEvidence ? (
              <span
                className={`topologyFreshnessBadge ${latestTopologyEvidenceStale ? "stale" : "current"}`}
                title={formatFullTime(latestTopologyEvidence)}
              >
                Last topology evidence:{" "}
                {formatCompactTime(latestTopologyEvidence)} -{" "}
                {latestTopologyEvidenceStale ? "stale" : "current"}
              </span>
            ) : null}
          </div>
          <span>{status}</span>
        </div>
        <button
          className="secondaryAction"
          disabled={loading || runtimeConfigEvidenceState === "loading"}
          onClick={() => void onRefresh(evidenceQuery())}
          title={
            loading || runtimeConfigEvidenceState === "loading"
              ? "Topology or runtime configuration evidence is already loading"
              : "Refresh topology edges and retained endpoint evidence for the selected range"
          }
          type="button"
        >
          <RefreshCcw size={17} />
          Refresh graph
        </button>
      </div>
      <ActionFeedback
        className="localActionFeedback"
        message={error}
        tone="danger"
      />
      <NetworkEvidenceRangeControls
        ariaLabel="Topology graph evidence range"
        endAt={customEndAt}
        onEndAtChange={setCustomEndAt}
        onStartAtChange={setCustomStartAt}
        onWindowChange={selectEvidenceWindow}
        startAt={customStartAt}
        window={evidenceWindow}
      />
      {evidenceWindow === "custom" ? (
        <button
          className="secondaryAction compactAction"
          disabled={loading}
          onClick={() => void onRefresh(evidenceQuery("custom"))}
          title={
            loading
              ? "Topology evidence is already loading"
              : "Apply the custom topology evidence range"
          }
          type="button"
        >
          Apply custom range
        </button>
      ) : null}
      {graph.edges.length > 0 && (
        <div className="topologyGraphControls">
          <label className="searchControl compactSearch">
            <Search size={16} />
            <input
              aria-label="Filter topology graph"
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search plans or endpoints"
              value={query}
            />
          </label>
          <label className="topologyFilterSelect">
            <span>Plan</span>
            <select
              aria-label="Topology tunnel plan filter"
              onChange={(event) => setPlanFilter(event.target.value)}
              value={planFilter}
            >
              <option value="">All eligible plans</option>
              {planOptions.map((plan) => (
                <option key={plan.id} value={plan.id}>
                  {plan.name}
                </option>
              ))}
            </select>
          </label>
          <label className="topologyFilterSelect">
            <span>View</span>
            <select
              aria-label="Topology health filter"
              onChange={(event) =>
                setHealthFilter(event.target.value as HealthFilter)
              }
              value={healthFilter}
            >
              {healthFilters.map((filter) => (
                <option key={filter.value} value={filter.value}>
                  {filter.label}
                </option>
              ))}
            </select>
          </label>
          <div
            aria-label="Topology graph viewport"
            className="topologyGraphViewportControls"
          >
            <button
              aria-label="Zoom out topology graph"
              disabled={graphZoom <= 0.8}
              onClick={() =>
                setGraphZoom((current) =>
                  Math.max(0.8, roundZoom(current - 0.2)),
                )
              }
              title={
                graphZoom <= 0.8
                  ? "Topology graph is already at the minimum 80% zoom"
                  : "Zoom out topology graph"
              }
              type="button"
            >
              <ZoomOut size={15} />
              <span>Zoom out</span>
            </button>
            <button
              aria-label="Reset topology graph view"
              onClick={() => {
                setGraphZoom(1);
                setGraphPan({ x: 0, y: 0 });
              }}
              title="Reset graph view"
              type="button"
            >
              <Maximize2 size={15} />
              <span>Reset</span>
            </button>
            <button
              aria-label="Zoom in topology graph"
              disabled={graphZoom >= 1.6}
              onClick={() =>
                setGraphZoom((current) =>
                  Math.min(1.6, roundZoom(current + 0.2)),
                )
              }
              title={
                graphZoom >= 1.6
                  ? "Topology graph is already at the maximum 160% zoom"
                  : "Zoom in topology graph"
              }
              type="button"
            >
              <ZoomIn size={15} />
              <span>Zoom in</span>
            </button>
            <span title={`Topology graph zoom ${Math.round(graphZoom * 100)}%`}>
              {Math.round(graphZoom * 100)}%
            </span>
          </div>
        </div>
      )}
      {graph.edges.length === 0 ? (
        <div className="emptyState">
          <GitGraph size={28} />
          <strong>No saved tunnel plans</strong>
          <span>
            Saved plans and their explicit endpoint observations will appear
            here.
          </span>
        </div>
      ) : filtered.edges.length === 0 && filtered.nodes.length === 0 ? (
        <div className="emptyState">
          <GitGraph size={28} />
          <strong>No matching topology edges</strong>
          <span>
            {graph.edges.length} saved tunnels remain outside the current
            filter.
          </span>
        </div>
      ) : (
        <>
          <div
            className="topologyGraphLegend"
            aria-label="Topology graph legend"
          >
            {legendItems.map((item) => (
              <div
                className={item.tone ? item.tone : undefined}
                key={item.label}
                title={`${item.label}: ${item.value}. ${item.detail}`}
              >
                <span title={item.label}>{item.label}</span>
                <strong title={`${item.label}: ${item.value}`}>
                  {item.value}
                </strong>
                <p title={item.detail}>{item.detail}</p>
              </div>
            ))}
          </div>
          {hasVisibleOspfCost ? (
            <details className="topologyGraphCostModel">
              <summary>Why OSPF cost changed</summary>
              <p>{OSPF_COST_MODEL_SUMMARY}</p>
              <small>{OSPF_COST_MODEL_DETAIL}</small>
            </details>
          ) : null}
          <div className="topologyMobileGraphToggle">
            <span>
              Mobile defaults to the tunnel list so endpoint state and actions
              stay readable.
            </span>
            <button
              className="secondaryAction compactAction"
              onClick={() => setMobileGraphOpen((current) => !current)}
              title={
                mobileGraphOpen
                  ? "Return to the mobile tunnel list first."
                  : "Open the topology graph canvas on mobile."
              }
              type="button"
            >
              {mobileGraphOpen ? "Show list first" : "Open graph view"}
            </button>
          </div>
          <div className="topologyGraphViewport">
            <svg
              aria-label="Topology graph"
              className="topologyGraphCanvas"
              preserveAspectRatio="xMidYMid meet"
              role="img"
              viewBox={`0 0 900 ${layout.height}`}
            >
              <defs>
                <marker
                  id="topologyArrow"
                  markerHeight="8"
                  markerWidth="8"
                  orient="auto"
                  refX="7"
                  refY="4"
                >
                  <path
                    d="M0,0 L8,4 L0,8 z"
                    fill={consolePalette.neutral.muted}
                  />
                </marker>
              </defs>
              <g transform={graphTransform}>
                {filtered.edges.map((edge) => {
                  const left = nodeById.get(edge.left_client_id);
                  const right = nodeById.get(edge.right_client_id);
                  if (!left || !right) {
                    return null;
                  }
                  return (
                    <g
                      className={`topologyGraphEdge ${effectiveEdgeHealth(edge)}`}
                      key={edge.plan_id}
                    >
                      <title>
                        {edgeHoverDetail(edge, nodeById, vpsNameDisplayMode)}
                      </title>
                      <line
                        markerEnd="url(#topologyArrow)"
                        x1={left.x}
                        x2={right.x}
                        y1={left.y}
                        y2={right.y}
                      />
                      {showEdgeLabels && (
                        <>
                          <text
                            x={(left.x + right.x) / 2}
                            y={(left.y + right.y) / 2 - 16}
                          >
                            {edge.plan_name}
                          </text>
                          <text
                            className="topologyGraphMetric"
                            x={(left.x + right.x) / 2}
                            y={(left.y + right.y) / 2 + 2}
                          >
                            {edgeInlineMetric(edge)}
                          </text>
                          {edge.recommended_ospf_cost !== null ? (
                            <text
                              className="topologyGraphMetric"
                              x={(left.x + right.x) / 2}
                              y={(left.y + right.y) / 2 + 18}
                            >
                              {ospfCostSummary(edge)}
                            </text>
                          ) : null}
                        </>
                      )}
                    </g>
                  );
                })}
                {nodes.map((node) => {
                  const stats =
                    nodeStatsById.get(node.client_id) ??
                    EMPTY_NODE_TUNNEL_STATS;
                  return (
                    <g
                      aria-label={`Select ${nodeLabel(node, vpsNameDisplayMode)}`}
                      className={`topologyGraphNode ${selectedNode?.client_id === node.client_id ? "selected" : ""} ${stats.attention > 0 ? "degraded" : nodeStatusClass(node, agentById)}`}
                      key={node.client_id}
                      onClick={() => setSelectedClientId(node.client_id)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          setSelectedClientId(node.client_id);
                        }
                      }}
                      role="button"
                      tabIndex={0}
                    >
                      <title>
                        {nodeHoverDetail(
                          node,
                          vpsNameDisplayMode,
                          agentById,
                          stats,
                        )}
                      </title>
                      <circle cx={node.x} cy={node.y} r="42" />
                      <text x={node.x} y={node.y - 8}>
                        {graphNodeLabel(node, vpsNameDisplayMode)}
                      </text>
                      <text
                        className="topologyGraphMetric"
                        x={node.x}
                        y={node.y + 10}
                      >
                        {regionLabel(node)}
                      </text>
                      <text
                        className="topologyGraphMetric"
                        x={node.x}
                        y={node.y + 26}
                      >
                        {stats.healthy}/{stats.enabled} active
                      </text>
                    </g>
                  );
                })}
              </g>
            </svg>
            {showMinimap ? (
              <svg
                aria-label="Topology minimap"
                className="topologyGraphMinimap"
                viewBox={`0 0 900 ${layout.height}`}
              >
                {filtered.edges.map((edge) => {
                  const left = nodeById.get(edge.left_client_id);
                  const right = nodeById.get(edge.right_client_id);
                  return left && right ? (
                    <line
                      key={edge.plan_id}
                      x1={left.x}
                      x2={right.x}
                      y1={left.y}
                      y2={right.y}
                    />
                  ) : null;
                })}
                {nodes.map((node) => (
                  <circle
                    className={
                      (nodeStatsById.get(node.client_id)?.attention ?? 0) > 0
                        ? "attention"
                        : undefined
                    }
                    cx={node.x}
                    cy={node.y}
                    key={node.client_id}
                    r="12"
                  />
                ))}
              </svg>
            ) : null}
          </div>
          {selectedClientId && !selectedNode ? (
            <div
              className="emptyState compactEmpty topologySelectionEmpty"
              role="status"
            >
              <Server size={20} />
              <strong>VPS not in the managed graph</strong>
              <span>
                {graphSelectionTargetLabel(
                  selectedClientId,
                  agentById,
                  vpsNameDisplayMode,
                )}{" "}
                has no visible managed tunnel declaration in the current graph
                or filter. No different VPS was selected.
              </span>
            </div>
          ) : null}
          {selectedNode && (
            <div className="topologyNodeInspector">
              <span className="historyPrimary">
                <strong>{nodeLabel(selectedNode, vpsNameDisplayMode)}</strong>
                <small>
                  {selectedDisplayState
                    ? `${selectedDisplayState.label}; ${selectedDisplayState.detail}`
                    : `Topology identity ${humanStatus(selectedNode.status)}`}
                  ; {selectedEdges.length} visible tunnel
                  {selectedEdges.length === 1 ? "" : "s"}
                </small>
              </span>
              <span className="topologyTagList">
                {selectedNode.tags.slice(0, 6).map((tag) => (
                  <span key={tag}>{tag}</span>
                ))}
                {selectedNode.tags.length === 0 && <span>untagged</span>}
              </span>
              <span className="topologyMetric">
                <strong>
                  {selectedNodeStats.healthy}/{selectedNodeStats.enabled}
                </strong>
                <small>healthy enabled tunnels</small>
              </span>
              <span className="topologyMetric">
                <strong>{selectedNodeStats.attention}</strong>
                <small>
                  need attention; {selectedNodeStats.disabled} disabled
                </small>
              </span>
              <span className="topologyMetric">
                <strong>
                  {runtimeConfigApplyStateLabel(
                    selectedRuntimeState,
                    runtimeConfigEvidenceState,
                  )}
                </strong>
                <small>
                  {runtimeConfigApplyStateDetail(
                    selectedRuntimeState,
                    runtimeConfigEvidenceState,
                  )}
                </small>
              </span>
              {onOpenVpsDetail ? (
                <button
                  className="secondaryAction compactAction"
                  onClick={() => onOpenVpsDetail(selectedNode.client_id)}
                  title={`Open VPS detail for ${nodeLabel(selectedNode, vpsNameDisplayMode)}.`}
                  type="button"
                >
                  <Server size={14} />
                  <span>Open VPS</span>
                </button>
              ) : null}
            </div>
          )}
          <div className="topologyGraphSummary">
            {filtered.edges.map((edge) => (
              <div className="topologyGraphEdgeRow" key={edge.plan_id}>
                <span
                  className="historyPrimary topologySummaryCell"
                  data-label="Tunnel"
                >
                  <strong>{edge.plan_name}</strong>
                  <small>
                    {edge.enabled
                      ? edgeEndpointLabel(edge, nodeById, vpsNameDisplayMode)
                      : `disabled; ${edgeEndpointLabel(edge, nodeById, vpsNameDisplayMode)}`}
                  </small>
                </span>
                <span className="topologySummaryCell" data-label="Health">
                  <span
                    className={`status ${topologyEdgeHealthStatusBadgeClass(effectiveEdgeHealth(edge))}`}
                  >
                    {humanStatus(effectiveEdgeHealth(edge))}
                  </span>
                </span>
                <span className="topologyMetric" data-label="Metric">
                  <strong>{edgeMetric(edge)}</strong>
                  <small
                    data-tooltip-sensitive={
                      edgeAvailabilityReasons(edge).length > 0 ||
                      edgeRuntimeReasons(edge).length > 0
                        ? "true"
                        : undefined
                    }
                    data-value-tooltip-skip={
                      edgeAvailabilityReasons(edge).length > 0 ||
                      edgeRuntimeReasons(edge).length > 0
                        ? "true"
                        : undefined
                    }
                    title={
                      edgeAvailabilityReasons(edge).length > 0 ||
                      edgeRuntimeReasons(edge).length > 0
                        ? "Tunnel health detail is displayed here; exact runtime diagnostic content is excluded from tooltips."
                        : undefined
                    }
                  >
                    {edgeStatusDetail(edge)}
                  </small>
                </span>
                <span className="topologyMetric" data-label="Endpoints">
                  <strong>
                    {humanStatus(edge.left_runtime_state)} /{" "}
                    {humanStatus(edge.right_runtime_state)}
                  </strong>
                  <small
                    data-tooltip-sensitive={
                      edge.left_runtime_reason || edge.right_runtime_reason
                        ? "true"
                        : undefined
                    }
                    data-value-tooltip-skip={
                      edge.left_runtime_reason || edge.right_runtime_reason
                        ? "true"
                        : undefined
                    }
                    title={
                      edge.left_runtime_reason || edge.right_runtime_reason
                        ? "Endpoint runtime detail is displayed here; exact provider diagnostic content is excluded from tooltips."
                        : undefined
                    }
                  >
                    {endpointRuntimeDetail(edge)}
                  </small>
                </span>
                <span className="topologyMetric" data-label="Neighbor">
                  <strong>{humanStatus(edge.neighbor_state)}</strong>
                  <small title={endpointReachabilityTitle(edge)}>
                    {endpointReachabilitySummary(edge)}
                  </small>
                </span>
                <span className="topologyMetric" data-label="Runtime">
                  <strong>{humanStatus(edge.runtime_state)}</strong>
                  <small>{runtimeCoverageDetail(edge)}</small>
                </span>
                {hasLatencyCurve(edge) ? (
                  <LatencySparkline edge={edge} />
                ) : null}
                <span className="topologyMetric" data-label="Cost">
                  <strong>{ospfCostSummary(edge)}</strong>
                  <small>
                    {edge.latest_observed_at
                      ? formatTime(edge.latest_observed_at)
                      : "no observations"}
                  </small>
                </span>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function runtimeConfigApplyStateLabel(
  state: RuntimeConfigApplyStateRecord | null,
  evidenceState: "available" | "loading" | "unavailable",
): string {
  if (evidenceState === "loading") {
    return "checking";
  }
  if (evidenceState === "unavailable") {
    return "unknown";
  }
  if (state?.pending_status === "failed") {
    return "sync failed";
  }
  if (state?.pending_status === "queued") {
    return "sync pending";
  }
  if (state?.applied_content_hash) {
    return "sync applied";
  }
  return "not applied";
}

function runtimeConfigApplyStateDetail(
  state: RuntimeConfigApplyStateRecord | null,
  evidenceState: "available" | "loading" | "unavailable",
): string {
  if (evidenceState === "loading") {
    return "loading runtime configuration evidence";
  }
  if (evidenceState === "unavailable") {
    return "runtime configuration evidence unavailable";
  }
  if (!state) {
    return "no server state";
  }
  if (state.pending_status === "failed") {
    const job = state.pending_job_id
      ? ` Job ${shortId(state.pending_job_id)}.`
      : "";
    return (
      dispatchFailureReason(
        state.pending_error,
        state.pending_status,
        "Runtime config apply",
      ) + job
    );
  }
  if (state.pending_status === "queued") {
    return state.pending_job_id
      ? `job ${shortId(state.pending_job_id)}`
      : "waiting";
  }
  if (state.applied_content_hash) {
    return state.applied_at
      ? `applied ${formatCompactTime(state.applied_at)}`
      : `hash ${shortId(state.applied_content_hash)}`;
  }
  return "no successful sync";
}

function roundZoom(value: number): number {
  return Math.round(value * 10) / 10;
}

function graphTransformFor(
  zoom: number,
  pan: GraphPan,
  height: number,
): string {
  const originX = 450;
  const originY = height / 2;
  const translateX = Math.round(originX - originX * zoom + pan.x);
  const translateY = Math.round(originY - originY * zoom + pan.y);
  return `translate(${translateX} ${translateY}) scale(${zoom})`;
}

function latestTopologyEvidenceAt(graph: TopologyGraph): string | null {
  return latestIso([
    graph.generated_at,
    ...graph.nodes.map((node) => node.latest_observed_at),
    ...graph.edges.map((edge) => edge.latest_observed_at),
  ]);
}

function latestIso(values: Array<string | null | undefined>): string | null {
  return values.reduce<string | null>((latest, value) => {
    if (!value) {
      return latest;
    }
    const timestamp = timestampMillis(value);
    if (!Number.isFinite(timestamp)) {
      return latest;
    }
    if (!latest || timestamp > timestampMillis(latest)) {
      return value;
    }
    return latest;
  }, null);
}

function isStaleEvidence(value: string | null): boolean {
  if (!value) {
    return false;
  }
  const timestamp = timestampMillis(value);
  return Number.isFinite(timestamp)
    ? Date.now() - timestamp > 24 * 60 * 60 * 1000
    : false;
}

function buildGraphLegendItems(edges: TopologyGraphEdge[]): GraphLegendItem[] {
  const enabledEdges = edges.filter((edge) => edge.enabled);
  const attentionCount = edges.filter((edge) =>
    edgeMatchesHealth(edge, "attention"),
  ).length;
  const healthyCount = edges.filter((edge) =>
    edgeMatchesHealth(edge, "healthy"),
  ).length;
  const unknownCount = edges.filter((edge) =>
    edgeMatchesHealth(edge, "unknown"),
  ).length;
  const disabledCount = edges.filter((edge) => !edge.enabled).length;
  const latestMeasuredEdge =
    enabledEdges.find(
      (edge) =>
        typeof edge.latency_avg_ms === "number" ||
        typeof edge.throughput_avg_mbps === "number",
    ) ??
    enabledEdges[0] ??
    null;
  const latestOspfEdge =
    enabledEdges.find((edge) => edge.recommended_ospf_cost !== null) ?? null;
  const items: GraphLegendItem[] = [
    {
      detail: `${healthyCount} healthy, ${unknownCount} unknown, ${attentionCount} attention, ${disabledCount} disabled`,
      label: "Layers",
      tone:
        attentionCount > 0
          ? "attention"
          : healthyCount > 0 && unknownCount === 0
            ? "ready"
            : undefined,
      value: `${edges.length} visible tunnel${edges.length === 1 ? "" : "s"}`,
    },
    {
      detail: latestMeasuredEdge
        ? `${lossLabel(latestMeasuredEdge.packet_loss_avg_ratio)}; ${bandwidthLabel(latestMeasuredEdge)}; ${latestMeasuredEdge.sample_count} sample${latestMeasuredEdge.sample_count === 1 ? "" : "s"}`
        : "Measurement evidence is not available yet.",
      label: "Measurements",
      tone:
        latestMeasuredEdge && (latestMeasuredEdge.degraded_count ?? 0) > 0
          ? "attention"
          : undefined,
      value: latestMeasuredEdge
        ? latencyLabel(latestMeasuredEdge)
        : "No samples",
    },
  ];
  if (latestOspfEdge) {
    items.splice(1, 0, {
      detail: `Optional OSPF uses explicit endpoint adapters; ${ospfCostReason(latestOspfEdge)}`,
      label: "OSPF cost",
      value: ospfCostSummary(latestOspfEdge),
    });
  }
  return items;
}

function filterGraph(
  graph: TopologyGraph,
  query: string,
  healthFilter: HealthFilter,
  planFilter: string,
) {
  const normalizedQuery = query.trim().toLowerCase();
  const graphNodeById = new Map(
    graph.nodes.map((node) => [node.client_id, node]),
  );
  const edges = graph.edges.filter((edge) => {
    if (planFilter && edge.plan_id !== planFilter) {
      return false;
    }
    if (!edgeMatchesHealth(edge, healthFilter)) {
      return false;
    }
    if (!normalizedQuery) {
      return true;
    }
    return (
      textMatches(edgeSearchText(edge), normalizedQuery) ||
      textMatches(
        nodeSearchText(graphNodeById.get(edge.left_client_id)),
        normalizedQuery,
      ) ||
      textMatches(
        nodeSearchText(graphNodeById.get(edge.right_client_id)),
        normalizedQuery,
      )
    );
  });
  const visibleNodeIds = new Set<string>();
  for (const edge of edges) {
    visibleNodeIds.add(edge.left_client_id);
    visibleNodeIds.add(edge.right_client_id);
  }
  const queryMatchedNodeIds = new Set(
    graph.nodes
      .filter(
        (node) =>
          normalizedQuery && textMatches(nodeSearchText(node), normalizedQuery),
      )
      .map((node) => node.client_id),
  );
  for (const clientId of queryMatchedNodeIds) {
    visibleNodeIds.add(clientId);
  }
  const nodes = graph.nodes.filter((node) =>
    visibleNodeIds.has(node.client_id),
  );
  return { edges, nodes };
}

function positionNodes(nodes: TopologyGraphNode[]): GraphLayout {
  if (nodes.length === 0) {
    return { height: 360, nodes: [] };
  }
  if (nodes.length === 1) {
    return { height: 360, nodes: [{ ...nodes[0], x: 450, y: 180 }] };
  }
  if (nodes.length > 8) {
    const columns = Math.min(4, Math.ceil(Math.sqrt(nodes.length * 1.2)));
    const rows = Math.ceil(nodes.length / columns);
    const height = Math.max(420, rows * 116 + 100);
    const xStep = columns === 1 ? 0 : 740 / (columns - 1);
    const yStep = rows === 1 ? 0 : (height - 128) / (rows - 1);
    return {
      height,
      nodes: nodes.map((node, index) => ({
        ...node,
        x: Math.round(80 + (index % columns) * xStep),
        y: Math.round(64 + Math.floor(index / columns) * yStep),
      })),
    };
  }
  const centerX = 450;
  const centerY = 180;
  const radiusX = 330;
  const radiusY = 120;
  return {
    height: 360,
    nodes: nodes.map((node, index) => {
      const angle = (Math.PI * 2 * index) / nodes.length - Math.PI / 2;
      return {
        ...node,
        x: Math.round(centerX + Math.cos(angle) * radiusX),
        y: Math.round(centerY + Math.sin(angle) * radiusY),
      };
    }),
  };
}

function edgeMatchesHealth(
  edge: TopologyGraphEdge,
  filter: HealthFilter,
): boolean {
  if (filter === "all") {
    return true;
  }
  if (filter === "disabled") {
    return !edge.enabled;
  }
  if (!edge.enabled) {
    return false;
  }
  if (filter === "attention") {
    return (
      edge.health === "degraded" ||
      (edge.degraded_count ?? 0) > 0 ||
      (edge.desired_missing_count ?? 0) > 0 ||
      (edge.stale_present_count ?? 0) > 0 ||
      edge.unavailable_client_ids.length > 0
    );
  }
  if (filter === "healthy") {
    return edge.health === "healthy";
  }
  return edge.health === "unknown";
}

function effectiveEdgeHealth(
  edge: TopologyGraphEdge,
): TopologyEdgeHealthStatus {
  return edge.enabled ? edge.health : "disabled";
}

function buildNodeTunnelStats(
  edges: TopologyGraphEdge[],
): Map<string, NodeTunnelStats> {
  const statsByClientId = new Map<string, NodeTunnelStats>();
  for (const edge of edges) {
    for (const clientId of [edge.left_client_id, edge.right_client_id]) {
      const stats = statsByClientId.get(clientId) ?? {
        ...EMPTY_NODE_TUNNEL_STATS,
      };
      stats.total += 1;
      if (!edge.enabled) {
        stats.disabled += 1;
      } else {
        stats.enabled += 1;
        if (edgeMatchesHealth(edge, "healthy")) {
          stats.healthy += 1;
        }
        if (edgeMatchesHealth(edge, "attention")) {
          stats.attention += 1;
        }
      }
      statsByClientId.set(clientId, stats);
    }
  }
  return statsByClientId;
}

function edgeSearchText(edge: TopologyGraphEdge): string {
  return [
    edge.plan_id,
    edge.plan_name,
    edge.interface_name,
    edge.kind,
    edge.health,
    edge.enabled ? "enabled" : "disabled",
    edge.neighbor_state ?? "",
    edge.reachability_state ?? "",
    edge.left_reachability_state,
    edge.right_reachability_state,
    edge.left_reachability_reason ?? "",
    edge.right_reachability_reason ?? "",
    edge.left_reachability_source ?? "",
    edge.right_reachability_source ?? "",
    edge.left_reachability_observed_at ?? "",
    edge.right_reachability_observed_at ?? "",
    edge.runtime_state ?? "",
    edge.adapter_state ?? "",
    edge.routing_state ?? "",
    edge.kernel_link_probe_state ?? "",
    edge.kernel_neighbor_probe_state ?? "",
    edge.kernel_route_probe_state ?? "",
    ...(edge.runtime_reasons ?? []),
    edge.left_client_id,
    edge.right_client_id,
    edge.left_runtime_state,
    edge.right_runtime_state,
    edge.left_runtime_reason ?? "",
    edge.right_runtime_reason ?? "",
    ...edgeUnavailableClientIds(edge),
    ...edgeAvailabilityReasons(edge),
    edge.left_tunnel_address,
    edge.right_tunnel_address,
  ].join(" ");
}

function nodeSearchText(node?: TopologyGraphNode): string {
  if (!node) {
    return "";
  }
  return [node.client_id, node.display_name, node.status, ...node.tags].join(
    " ",
  );
}

function textMatches(value: string, query: string): boolean {
  return value.toLowerCase().includes(query);
}

function edgeMetric(edge: TopologyGraphEdge): string {
  if (typeof edge.latency_avg_ms === "number") {
    return `${edge.latency_avg_ms.toFixed(1)} ms`;
  }
  if (typeof edge.throughput_avg_mbps === "number") {
    return `${edge.throughput_avg_mbps.toFixed(1)} Mbps`;
  }
  return formatBandwidthMbps(edge.bandwidth_mbps);
}

function edgeInlineMetric(edge: TopologyGraphEdge): string {
  if (!edge.enabled) {
    return "disabled plan";
  }
  return `${latencyLabel(edge)} / ${lossLabel(edge.packet_loss_avg_ratio)} / ${bandwidthLabel(edge)}`;
}

function latencyLabel(edge: TopologyGraphEdge): string {
  return typeof edge.latency_avg_ms === "number"
    ? `${edge.latency_avg_ms.toFixed(1)} ms`
    : "latency pending";
}

function bandwidthLabel(edge: TopologyGraphEdge): string {
  return typeof edge.throughput_avg_mbps === "number"
    ? `${edge.throughput_avg_mbps.toFixed(1)} Mbps avg`
    : `${formatBandwidthMbps(edge.bandwidth_mbps)} target`;
}

function formatBandwidthMbps(value: number): string {
  return `${Math.round(value)} Mbps`;
}

function formatLoss(value: number): string {
  return `${(value * 100).toFixed(value > 0 && value < 0.01 ? 2 : 1)}%`;
}

function lossLabel(value: number | null): string {
  return value === null ? "loss pending" : `${formatLoss(value)} loss`;
}

function ospfCostSummary(edge: TopologyGraphEdge): string {
  if (edge.recommended_ospf_cost === null) {
    return "OSPF off";
  }
  return edge.cost_delta === null
    ? `OSPF ${edge.recommended_ospf_cost}`
    : `OSPF ${edge.recommended_ospf_cost} (${edge.cost_delta > 0 ? "+" : ""}${edge.cost_delta})`;
}

function ospfCostReason(edge: TopologyGraphEdge): string {
  if (edge.recommended_ospf_cost === null) {
    return "This tunnel plan has no routing cost adapter workflow.";
  }
  return `${latencyLabel(edge)}, ${lossLabel(edge.packet_loss_avg_ratio)}, ${bandwidthLabel(edge)} drive recommended cost ${edge.recommended_ospf_cost}.`;
}

function edgeStatusDetail(edge: TopologyGraphEdge): string {
  if (!edge.enabled) {
    return "disabled";
  }
  const unavailableCount = edgeUnavailableClientIds(edge).length;
  if (unavailableCount > 0) {
    return `${unavailableCount} endpoint${unavailableCount === 1 ? "" : "s"} unavailable`;
  }
  if ((edge.stale_present_count ?? 0) > 0) {
    return `${edge.stale_present_count} stale interface${edge.stale_present_count === 1 ? "" : "s"}`;
  }
  if ((edge.desired_missing_count ?? 0) > 0) {
    return `${edge.desired_missing_count} desired interface${edge.desired_missing_count === 1 ? "" : "s"} missing`;
  }
  const availabilityReasons = edgeAvailabilityReasons(edge);
  if (availabilityReasons.length > 0) {
    return availabilityReasons.map(humanStatus).join(", ");
  }
  const runtimeReasons = edgeRuntimeReasons(edge);
  if (runtimeReasons.length > 0) {
    return runtimeReasons.map(humanStatus).join(", ");
  }
  return `${humanStatus(edge.left_runtime_state)} / ${humanStatus(edge.right_runtime_state)}`;
}

function runtimeCoverageDetail(edge: TopologyGraphEdge): string {
  const kernel = edge.kernel_namespace_covered ? "kernel" : "sysfs";
  const probes = [
    `link ${humanStatus(edge.kernel_link_probe_state)}`,
    `neigh ${humanStatus(edge.kernel_neighbor_probe_state)}`,
    `route ${humanStatus(edge.kernel_route_probe_state)}`,
  ].join(", ");
  if (edge.routing_state && edge.routing_state !== "unknown") {
    return `${kernel}; routing ${humanStatus(edge.routing_state)}`;
  }
  if (edge.adapter_state && edge.adapter_state !== "unknown") {
    return `${kernel}; adapter ${humanStatus(edge.adapter_state)}`;
  }
  return `${kernel}; ${probes}`;
}

function LatencySparkline({ edge }: { edge: TopologyGraphEdge }) {
  const series = Array.isArray(edge.latency_series_ms)
    ? edge.latency_series_ms.filter(Number.isFinite)
    : [];
  const max = Math.max(1, ...series);
  return (
    <span className="topologySummaryCell latencyCurveCell" data-label="Curve">
      <span
        className="latencyMiniCurve"
        aria-label={`${edge.plan_name} latency curve`}
      >
        {series.map((value, index) => (
          <span
            key={`${edge.plan_id}-${index}`}
            style={{
              height: `${Math.max(6, Math.round((value / max) * 28))}px`,
            }}
            title={`${value.toFixed(value < 10 ? 2 : 1)} ms`}
          />
        ))}
      </span>
    </span>
  );
}

function hasLatencyCurve(edge: TopologyGraphEdge): boolean {
  return (
    Array.isArray(edge.latency_series_ms) &&
    edge.latency_series_ms.some(Number.isFinite)
  );
}

function edgeUnavailableClientIds(edge: TopologyGraphEdge): string[] {
  return Array.isArray(edge.unavailable_client_ids)
    ? edge.unavailable_client_ids
    : [];
}

function edgeAvailabilityReasons(edge: TopologyGraphEdge): string[] {
  return Array.isArray(edge.availability_reasons)
    ? edge.availability_reasons
    : [];
}

function edgeRuntimeReasons(edge: TopologyGraphEdge): string[] {
  return Array.isArray(edge.runtime_reasons) ? edge.runtime_reasons : [];
}

function humanStatus(value: string | null | undefined): string {
  return value ? readableTelemetryToken(value) : "Unknown";
}

function nodeLabel(
  node: Pick<TopologyGraphNode, "client_id" | "display_name">,
  mode: VpsNameDisplayMode,
): string {
  return formatVpsName(node, mode);
}

function graphSelectionTargetLabel(
  clientId: string,
  agentById: Map<string, AgentView>,
  mode: VpsNameDisplayMode,
): string {
  const agent = agentById.get(clientId);
  return agent ? formatVpsName(agent, mode) : clientId;
}

function graphNodeLabel(
  node: Pick<TopologyGraphNode, "client_id" | "display_name">,
  mode: VpsNameDisplayMode,
): string {
  const label = nodeLabel(node, mode);
  if (label.length <= 24) {
    return label;
  }
  return `${label.slice(0, 16)}...${label.slice(-5)}`;
}

function edgeEndpointLabel(
  edge: TopologyGraphEdge,
  nodeById: Map<string, TopologyGraphNode>,
  mode: VpsNameDisplayMode,
): string {
  const left = nodeById.get(edge.left_client_id);
  const right = nodeById.get(edge.right_client_id);
  return `${left ? nodeLabel(left, mode) : "Unknown VPS"} -> ${right ? nodeLabel(right, mode) : "Unknown VPS"}`;
}

function nodeDisplayState(
  node: TopologyGraphNode,
  agentById: Map<string, AgentView>,
) {
  const agent = agentById.get(node.client_id);
  return agent ? agentDisplayState(agent) : null;
}

function nodeStatusClass(
  node: TopologyGraphNode,
  agentById: Map<string, AgentView>,
): string {
  const displayState = nodeDisplayState(node, agentById);
  if (!displayState) {
    return node.status || "unknown";
  }
  if (displayState.label === "Online") {
    return "online";
  }
  if (displayState.label === "Offline") {
    return "unknown";
  }
  return displayState.tone === "ok" ? "online" : "stale";
}

function nodeHoverDetail(
  node: TopologyGraphNode,
  mode: VpsNameDisplayMode,
  agentById: Map<string, AgentView>,
  stats: NodeTunnelStats,
): string {
  const displayState = nodeDisplayState(node, agentById);
  const statusDetail = displayState
    ? `${displayState.label}; ${displayState.detail}; topology ${humanStatus(node.status)}`
    : humanStatus(node.status);
  return [
    nodeLabel(node, mode),
    `status ${statusDetail}`,
    `${stats.healthy}/${stats.enabled} enabled tunnels healthy`,
    `${stats.attention} need attention`,
    `${stats.disabled} disabled`,
    `region ${regionLabel(node)}`,
    node.latest_observed_at
      ? `observed ${formatTime(node.latest_observed_at)}`
      : "no observation timestamp",
  ].join("; ");
}

function edgeHoverDetail(
  edge: TopologyGraphEdge,
  nodeById: Map<string, TopologyGraphNode>,
  mode: VpsNameDisplayMode,
): string {
  return [
    edge.plan_name,
    edgeEndpointLabel(edge, nodeById, mode),
    humanStatus(edge.health),
    edgeInlineMetric(edge),
    ospfCostSummary(edge),
    ospfCostReason(edge),
  ].join("; ");
}

function endpointRuntimeDetail(edge: TopologyGraphEdge): string {
  const reasons = [edge.left_runtime_reason, edge.right_runtime_reason]
    .filter((value): value is string => Boolean(value))
    .map(humanStatus);
  if (reasons.length > 0) {
    return reasons.join("; ");
  }
  const observed = latestIso([edge.left_observed_at, edge.right_observed_at]);
  return observed
    ? `latest ${formatCompactTime(observed)}`
    : "no endpoint evidence";
}

function endpointReachabilitySummary(edge: TopologyGraphEdge): string {
  const left = humanStatus(edge.left_reachability_state);
  const right = humanStatus(edge.right_reachability_state);
  return left === right
    ? `Both ${left.toLowerCase()}`
    : `L ${left} / R ${right}`;
}

function endpointReachabilityTitle(edge: TopologyGraphEdge): string {
  return [
    endpointReachabilityDetail(
      "Left",
      edge.left_reachability_state,
      edge.left_reachability_source,
      edge.left_reachability_observed_at,
    ),
    endpointReachabilityDetail(
      "Right",
      edge.right_reachability_state,
      edge.right_reachability_source,
      edge.right_reachability_observed_at,
    ),
  ].join("; ");
}

function endpointReachabilityDetail(
  label: string,
  state: string,
  source: string | null,
  observedAt: string | null,
): string {
  return [
    `${label}: ${humanStatus(state)}`,
    source ? `${humanStatus(source)} source` : null,
    observedAt ? `observed ${formatFullTime(observedAt)}` : "not observed",
    "exact probe diagnostic content excluded from tooltips",
  ]
    .filter(Boolean)
    .join(", ");
}

function regionLabel(node: Pick<TopologyGraphNode, "tags">): string {
  const country = node.tags.find((tag) => tag.startsWith("country:"));
  if (country) {
    return country.replace("country:", "");
  }
  const region = node.tags.find((tag) => tag.startsWith("region:"));
  if (region) {
    return region.replace("region:", "");
  }
  return "unregioned";
}
