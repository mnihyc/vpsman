import { useMemo, useState } from "react";
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
import { consolePalette } from "../../colorPalette";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  OSPF_COST_MODEL_DETAIL,
  OSPF_COST_MODEL_SUMMARY,
  readableTelemetryToken,
} from "../../topologyRuntime";
import type {
  RuntimeConfigApplyStateRecord,
  TopologyGraph,
  TopologyGraphEdge,
  TopologyGraphNode,
} from "../../types";
import {
  formatCompactTime,
  formatFullTime,
  formatTime,
  formatVpsName,
  shortId,
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

type HealthFilter = "all" | "attention" | "applied" | "planned";
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

const healthFilters: { label: string; value: HealthFilter }[] = [
  { label: "All", value: "all" },
  { label: "Attention", value: "attention" },
  { label: "Applied", value: "applied" },
  { label: "Planned", value: "planned" },
];

export function TopologyGraphPanel({
  graph,
  loading,
  onOpenVpsDetail,
  onRefresh,
  runtimeConfigApplyStates,
}: {
  graph: TopologyGraph;
  loading: boolean;
  onOpenVpsDetail?: (clientId: string) => void;
  onRefresh: () => Promise<void>;
  runtimeConfigApplyStates: RuntimeConfigApplyStateRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [query, setQuery] = useState("");
  const [healthFilter, setHealthFilter] = useState<HealthFilter>("all");
  const [selectedClientId, setSelectedClientId] = useState<string | null>(null);
  const [graphZoom, setGraphZoom] = useState(1);
  const [graphPan, setGraphPan] = useState<GraphPan>({ x: 0, y: 0 });
  const [mobileGraphOpen, setMobileGraphOpen] = useState(false);
  const filtered = useMemo(
    () => filterGraph(graph, query, healthFilter),
    [graph, healthFilter, query],
  );
  const layout = useMemo(() => positionNodes(filtered.nodes), [filtered.nodes]);
  const nodes = layout.nodes;
  const nodeById = new Map(nodes.map((node) => [node.client_id, node]));
  const selectedNode =
    nodes.find((node) => node.client_id === selectedClientId) ??
    nodes[0] ??
    null;
  const selectedEdges = selectedNode
    ? filtered.edges.filter(
        (edge) =>
          edge.left_client_id === selectedNode.client_id ||
          edge.right_client_id === selectedNode.client_id,
      )
    : [];
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
  const showEdgeLabels = filtered.edges.length <= 14 && nodes.length <= 12;
  const graphTransform = graphTransformFor(graphZoom, graphPan, layout.height);
  const latestTopologyEvidence = useMemo(
    () => latestTopologyEvidenceAt(graph),
    [graph],
  );
  const latestTopologyEvidenceStale = isStaleEvidence(latestTopologyEvidence);
  const legendItems = useMemo(
    () => buildGraphLegendItems(filtered.edges),
    [filtered.edges],
  );
  const showMinimap = filtered.edges.length > 10 || nodes.length > 8;
  const status =
    graph.edges.length === 0
      ? "No topology edges"
      : `${filtered.nodes.length} shown / ${graph.nodes.length} nodes; ${filtered.edges.length} shown / ${graph.edges.length} tunnels`;

  return (
    <section
      className={`fleetPanel topologyGraphPanel ${mobileGraphOpen ? "mobileGraphOpen" : ""}`}
    >
      <div className="sectionHeader">
        <div>
          <h2>
            Topology graph
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
          </h2>
          <span>{status}</span>
        </div>
        <button
          className="secondaryAction"
          disabled={loading}
          onClick={onRefresh}
          type="button"
        >
          <RefreshCcw size={17} />
          Refresh graph
        </button>
      </div>
      {graph.edges.length > 0 && (
        <div className="topologyGraphControls">
          <label className="searchControl compactSearch">
            <Search size={16} />
            <input
              aria-label="Filter topology graph"
              onChange={(event) => setQuery(event.target.value)}
              placeholder="Search graph"
              value={query}
            />
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
              title="Zoom out topology graph"
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
              title="Zoom in topology graph"
              type="button"
            >
              <ZoomIn size={15} />
              <span>Zoom in</span>
            </button>
            <span>{Math.round(graphZoom * 100)}%</span>
          </div>
        </div>
      )}
      {graph.edges.length === 0 ? (
        <div className="emptyState">
          <GitGraph size={28} />
          <strong>No saved tunnel plans</strong>
          <span>
            Saved plans and persisted observations will appear here as an
            applied topology graph.
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
              >
                <span>{item.label}</span>
                <strong>{item.value}</strong>
                <p>{item.detail}</p>
              </div>
            ))}
          </div>
          <details className="topologyGraphCostModel">
            <summary>Why OSPF cost changed</summary>
            <p>{OSPF_COST_MODEL_SUMMARY}</p>
            <small>{OSPF_COST_MODEL_DETAIL}</small>
          </details>
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
                      className={`topologyGraphEdge ${edge.health}`}
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
                          <text
                            className="topologyGraphMetric"
                            x={(left.x + right.x) / 2}
                            y={(left.y + right.y) / 2 + 18}
                          >
                            {ospfCostSummary(edge)}
                          </text>
                        </>
                      )}
                    </g>
                  );
                })}
                {nodes.map((node) => (
                  <g
                    aria-label={`Select ${nodeLabel(node, vpsNameDisplayMode)}`}
                    className={`topologyGraphNode ${selectedNode?.client_id === node.client_id ? "selected" : ""} ${node.degraded_tunnel_count > 0 ? "degraded" : node.status}`}
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
                    <title>{nodeHoverDetail(node, vpsNameDisplayMode)}</title>
                    <circle cx={node.x} cy={node.y} r="42" />
                    <text x={node.x} y={node.y - 8}>
                      {nodeLabel(node, vpsNameDisplayMode)}
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
                      {node.applied_tunnel_count}/{node.tunnel_count} applied
                    </text>
                  </g>
                ))}
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
                      node.degraded_tunnel_count > 0 ? "attention" : undefined
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
          {selectedNode && (
            <div className="topologyNodeInspector">
              <span className="historyPrimary">
                <strong>{nodeLabel(selectedNode, vpsNameDisplayMode)}</strong>
                <small>
                  {selectedNode.status}; {selectedEdges.length} visible tunnels
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
                  {selectedNode.applied_tunnel_count}/
                  {selectedNode.tunnel_count}
                </strong>
                <small>applied tunnels</small>
              </span>
              <span className="topologyMetric">
                <strong>{selectedNode.degraded_tunnel_count}</strong>
                <small>degraded tunnels</small>
              </span>
              <span className="topologyMetric">
                <strong>
                  {runtimeConfigApplyStateLabel(selectedRuntimeState)}
                </strong>
                <small>
                  {runtimeConfigApplyStateDetail(selectedRuntimeState)}
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
                    className={`status ${topologyEdgeHealthStatusBadgeClass(edge.health)}`}
                  >
                    {humanStatus(edge.health)}
                  </span>
                </span>
                <span className="topologyMetric" data-label="Metric">
                  <strong>{edgeMetric(edge)}</strong>
                  <small>{edgeStatusDetail(edge)}</small>
                </span>
                <span className="topologyMetric" data-label="Drift">
                  <strong>{humanStatus(edge.topology_drift_action)}</strong>
                  <small>{humanStatus(edge.topology_drift_policy)}</small>
                </span>
                <span className="topologyMetric" data-label="Neighbor">
                  <strong>{humanStatus(edge.neighbor_state)}</strong>
                  <small>{humanStatus(edge.probe_state)}</small>
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
): string {
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
): string {
  if (!state) {
    return "no server state";
  }
  if (state.pending_status === "failed") {
    return (
      state.pending_error ||
      (state.pending_job_id
        ? `job ${shortId(state.pending_job_id)}`
        : "manual review")
    );
  }
  if (state.pending_status === "queued") {
    return state.pending_job_id
      ? `job ${shortId(state.pending_job_id)}`
      : "waiting";
  }
  if (state.applied_content_hash) {
    const version = state.applied_version
      ? `v${state.applied_version}`
      : shortId(state.applied_content_hash);
    return state.applied_at
      ? `${version} ${formatTime(state.applied_at)}`
      : version;
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

function isStaleEvidence(value: string | null): boolean {
  if (!value) {
    return false;
  }
  const timestamp = new Date(value).getTime();
  return Number.isFinite(timestamp)
    ? Date.now() - timestamp > 24 * 60 * 60 * 1000
    : false;
}

function buildGraphLegendItems(edges: TopologyGraphEdge[]): GraphLegendItem[] {
  const attentionCount = edges.filter((edge) =>
    edgeMatchesHealth(edge, "attention"),
  ).length;
  const appliedCount = edges.filter((edge) =>
    edgeMatchesHealth(edge, "applied"),
  ).length;
  const plannedCount = edges.filter((edge) =>
    edgeMatchesHealth(edge, "planned"),
  ).length;
  const latestMeasuredEdge =
    edges.find(
      (edge) =>
        typeof edge.latency_avg_ms === "number" ||
        typeof edge.throughput_avg_mbps === "number",
    ) ??
    edges[0] ??
    null;
  return [
    {
      detail: `${appliedCount} applied, ${plannedCount} planned, ${attentionCount} attention`,
      label: "Layers",
      tone: attentionCount > 0 ? "attention" : "ready",
      value: `${edges.length} visible tunnel${edges.length === 1 ? "" : "s"}`,
    },
    {
      detail: latestMeasuredEdge
        ? `Network / OSPF owns review and apply; ${ospfCostReason(latestMeasuredEdge)}`
        : "Cost appears after a measured or saved tunnel is visible.",
      label: "OSPF cost",
      value: latestMeasuredEdge
        ? ospfCostSummary(latestMeasuredEdge)
        : "No cost",
    },
    {
      detail: latestMeasuredEdge
        ? `${formatLoss(latestMeasuredEdge.packet_loss_avg_ratio)} loss; ${bandwidthLabel(latestMeasuredEdge)}; ${latestMeasuredEdge.sample_count} samples`
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
}

function filterGraph(
  graph: TopologyGraph,
  query: string,
  healthFilter: HealthFilter,
) {
  const normalizedQuery = query.trim().toLowerCase();
  const graphNodeById = new Map(
    graph.nodes.map((node) => [node.client_id, node]),
  );
  const edges = graph.edges.filter((edge) => {
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
    normalizedQuery || healthFilter !== "all"
      ? visibleNodeIds.has(node.client_id)
      : true,
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
  if (nodes.length > 12) {
    const columns = Math.min(6, Math.ceil(Math.sqrt(nodes.length * 1.4)));
    const rows = Math.ceil(nodes.length / columns);
    const height = Math.max(420, rows * 92 + 120);
    const xStep = columns === 1 ? 0 : 740 / (columns - 1);
    const yStep = rows === 1 ? 0 : (height - 140) / (rows - 1);
    return {
      height,
      nodes: nodes.map((node, index) => ({
        ...node,
        x: Math.round(80 + (index % columns) * xStep),
        y: Math.round(70 + Math.floor(index / columns) * yStep),
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
  if (filter === "attention") {
    return (
      edge.health === "degraded" ||
      edge.health === "rolled_back" ||
      (edge.degraded_count ?? 0) > 0 ||
      (edge.desired_missing_count ?? 0) > 0 ||
      (edge.stale_present_count ?? 0) > 0 ||
      (edge.import_candidate_count ?? 0) > 0 ||
      Boolean(edge.convergence_blocked)
    );
  }
  if (filter === "applied") {
    return edge.health === "healthy" || edge.health === "applied";
  }
  return edge.health === "planned" || edge.status === "planned";
}

function edgeSearchText(edge: TopologyGraphEdge): string {
  return [
    edge.plan_id,
    edge.plan_name,
    edge.interface_name,
    edge.kind,
    edge.health,
    edge.status,
    edge.enabled ? "enabled" : "disabled",
    edge.topology_drift_policy ?? "",
    edge.topology_drift_action ?? "",
    edge.neighbor_state ?? "",
    edge.probe_state ?? "",
    edge.runtime_state ?? "",
    edge.adapter_state ?? "",
    edge.routing_state ?? "",
    edge.kernel_link_probe_state ?? "",
    edge.kernel_neighbor_probe_state ?? "",
    edge.kernel_route_probe_state ?? "",
    ...(edge.runtime_reasons ?? []),
    edge.left_client_id,
    edge.right_client_id,
    edge.left_status,
    edge.right_status,
    edge.convergence_blocked ? "convergence blocked" : "",
    ...edgeOfflineClientIds(edge),
    ...edgeServerDriftReasons(edge),
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
  return `${latencyLabel(edge)} / ${formatLoss(edge.packet_loss_avg_ratio)} / ${bandwidthLabel(edge)}`;
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

function formatLoss(value: number | null): string {
  return value === null
    ? "loss pending"
    : `${(value * 100).toFixed(value > 0 && value < 0.01 ? 2 : 1)}%`;
}

function ospfCostSummary(edge: TopologyGraphEdge): string {
  return edge.cost_delta === null
    ? `OSPF ${edge.recommended_ospf_cost}`
    : `OSPF ${edge.recommended_ospf_cost} (${edge.cost_delta > 0 ? "+" : ""}${edge.cost_delta})`;
}

function ospfCostReason(edge: TopologyGraphEdge): string {
  return `${latencyLabel(edge)}, ${formatLoss(edge.packet_loss_avg_ratio)} loss, ${bandwidthLabel(edge)} drive recommended cost ${edge.recommended_ospf_cost}.`;
}

function edgeStatusDetail(edge: TopologyGraphEdge): string {
  if (!edge.enabled) {
    return "disabled";
  }
  if (edge.convergence_blocked) {
    const blockedCount = edgeOfflineClientIds(edge).length;
    return blockedCount > 0
      ? `${blockedCount} endpoint${blockedCount === 1 ? "" : "s"} offline`
      : "convergence blocked";
  }
  if ((edge.import_candidate_count ?? 0) > 0) {
    return `${edge.import_candidate_count} import candidate${edge.import_candidate_count === 1 ? "" : "s"}`;
  }
  if ((edge.stale_present_count ?? 0) > 0) {
    return `${edge.stale_present_count} stale interface${edge.stale_present_count === 1 ? "" : "s"}`;
  }
  if ((edge.desired_missing_count ?? 0) > 0) {
    return `${edge.desired_missing_count} desired interface${edge.desired_missing_count === 1 ? "" : "s"} missing`;
  }
  const serverDriftReasons = edgeServerDriftReasons(edge);
  if (serverDriftReasons.length > 0) {
    return serverDriftReasons.map(humanStatus).join(", ");
  }
  const runtimeReasons = edgeRuntimeReasons(edge);
  if (runtimeReasons.length > 0) {
    return runtimeReasons.map(humanStatus).join(", ");
  }
  return `${humanStatus(edge.left_status)} / ${humanStatus(edge.right_status)}`;
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

function edgeOfflineClientIds(edge: TopologyGraphEdge): string[] {
  return Array.isArray(edge.offline_client_ids) ? edge.offline_client_ids : [];
}

function edgeServerDriftReasons(edge: TopologyGraphEdge): string[] {
  return Array.isArray(edge.server_drift_reasons)
    ? edge.server_drift_reasons
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

function edgeEndpointLabel(
  edge: TopologyGraphEdge,
  nodeById: Map<string, TopologyGraphNode>,
  mode: VpsNameDisplayMode,
): string {
  const left = nodeById.get(edge.left_client_id);
  const right = nodeById.get(edge.right_client_id);
  return `${left ? nodeLabel(left, mode) : "Unknown VPS"} -> ${right ? nodeLabel(right, mode) : "Unknown VPS"}`;
}

function nodeHoverDetail(
  node: TopologyGraphNode,
  mode: VpsNameDisplayMode,
): string {
  return [
    nodeLabel(node, mode),
    `status ${humanStatus(node.status)}`,
    `${node.applied_tunnel_count}/${node.tunnel_count} applied`,
    `${node.degraded_tunnel_count} degraded`,
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
