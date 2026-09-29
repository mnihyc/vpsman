import type { TopologyGraphEdge } from "../../types";

export const GRAPH_WIDTH = 900;
export const GRAPH_NODE_RADIUS = 42;

export type GraphPoint = { x: number; y: number };
export type GraphLabelSize = { width: number; height: number };
type GraphRect = GraphPoint & GraphLabelSize;
type LayoutNode = GraphPoint & { client_id: string };
export type GraphRoute = {
  edge: TopologyGraphEdge;
  start: GraphPoint;
  control: GraphPoint;
  end: GraphPoint;
  path: string;
};

// One text line with padding, plus a gap between neighbouring parallel labels.
const LABEL_HEIGHT = 24;
const CLEARANCE = 8;
const LANE_SPACING = LABEL_HEIGHT + CLEARANCE;

function graphRoutePoint(route: GraphRoute, t: number): GraphPoint {
  const u = 1 - t;
  return {
    x: u * u * route.start.x + 2 * u * t * route.control.x + t * t * route.end.x,
    y: u * u * route.start.y + 2 * u * t * route.control.y + t * t * route.end.y,
  };
}

export function routeGraphEdges(
  edges: TopologyGraphEdge[],
  nodes: LayoutNode[],
  height: number,
): GraphRoute[] {
  const nodeById = new Map(nodes.map((node) => [node.client_id, node]));
  const groups = new Map<string, TopologyGraphEdge[]>();
  for (const edge of edges) {
    const pair = [edge.left_client_id, edge.right_client_id].sort();
    const key = JSON.stringify(pair);
    const group = groups.get(key) ?? [];
    group.push(edge);
    groups.set(key, group);
  }
  const routes: GraphRoute[] = [];
  for (const group of groups.values()) {
    group.sort((a, b) => a.plan_id.localeCompare(b.plan_id));
    const pair = [group[0].left_client_id, group[0].right_client_id].sort();
    const a = nodeById.get(pair[0]);
    const b = nodeById.get(pair[1]);
    if (!a || !b) continue;
    const length = Math.hypot(b.x - a.x, b.y - a.y) || 1;
    const normal = { x: -(b.y - a.y) / length, y: (b.x - a.x) / length };
    const makeRoutes = (bend: number) =>
      group.map((edge, index) => {
        const left = nodeById.get(edge.left_client_id)!;
        const right = nodeById.get(edge.right_client_id)!;
        // A quadratic curve moves half its control-point offset at the midpoint.
        const offset =
          2 * (bend + (index - (group.length - 1) / 2) * LANE_SPACING);
        const control = {
          x: (a.x + b.x) / 2 + normal.x * offset,
          y: (a.y + b.y) / 2 + normal.y * offset,
        };
        const trim = (node: GraphPoint): GraphPoint => {
          const dx = control.x - node.x;
          const dy = control.y - node.y;
          const distance = Math.hypot(dx, dy) || 1;
          // Leave room for the node stroke; never trim past a nearby endpoint.
          const inset = Math.min(GRAPH_NODE_RADIUS + 4, length / 3);
          return {
            x: node.x + (dx / distance) * inset,
            y: node.y + (dy / distance) * inset,
          };
        };
        const start = trim(left);
        const end = trim(right);
        return {
          edge,
          start,
          control,
          end,
          path: `M ${start.x} ${start.y} Q ${control.x} ${control.y} ${end.x} ${end.y}`,
        };
      });
    // Try a straight route first, then corridors one node diameter apart.
    // Two corridors either side handle collinear grid links without a simulation.
    const corridor = 2 * GRAPH_NODE_RADIUS + CLEARANCE;
    let best = makeRoutes(0);
    let bestScore = Number.POSITIVE_INFINITY;
    for (const bend of [0, corridor, -corridor, 2 * corridor, -2 * corridor]) {
      const candidate = makeRoutes(bend);
      let score = 0;
      for (const route of candidate) {
        const steps = Math.max(
          2,
          Math.ceil((length + 2 * Math.abs(bend)) / GRAPH_NODE_RADIUS),
        );
        const points = Array.from({ length: steps + 1 }, (_, i) =>
          graphRoutePoint(route, i / steps),
        );
        if (points.some((p) =>
          p.x < 4 || p.x > GRAPH_WIDTH - 4 || p.y < 4 || p.y > height - 4,
        )) {
          score = Number.POSITIVE_INFINITY;
          break;
        }
        score += nodes.filter((node) =>
          node !== a && node !== b && points.some((p) =>
            Math.hypot(p.x - node.x, p.y - node.y) < GRAPH_NODE_RADIUS + CLEARANCE,
          ),
        ).length;
      }
      if (score < bestScore) {
        best = candidate;
        bestScore = score;
      }
      if (score === 0) break;
    }
    routes.push(...best);
  }
  return routes;
}

function overlaps(a: GraphRect, b: GraphRect): boolean {
  return (
    Math.abs(a.x - b.x) < (a.width + b.width) / 2 + CLEARANCE &&
    Math.abs(a.y - b.y) < (a.height + b.height) / 2 + CLEARANCE
  );
}

export function placeGraphLabels(
  routes: GraphRoute[],
  nodes: LayoutNode[],
  textSizes: Record<string, GraphLabelSize>,
  height: number,
): Map<string, GraphRect & { anchor: GraphPoint }> {
  const nodeBounds: GraphRect[] = nodes.map((node) => ({
    ...node,
    width: Math.max(
      GRAPH_NODE_RADIUS * 2,
      textSizes[`node:${node.client_id}`]?.width ?? 160,
    ),
    height: GRAPH_NODE_RADIUS * 2,
  }));
  type Label = GraphRect & { anchor: GraphPoint };
  const labels = new Map<string, Label>();
  // Reported routing costs take priority over non-OSPF plan names. Within each
  // group, reserve space for labels with less link length per measured text width
  // first: an asymmetric L/R cost needs more room than a single shared cost.
  const labelRoom = (route: GraphRoute) =>
    Math.hypot(route.end.x - route.start.x, route.end.y - route.start.y) /
    ((textSizes[`edge:${route.edge.plan_id}`]?.width ?? 160) + 12);
  const ordered = [...routes].sort((a, b) =>
    Number(b.edge.ospf_enabled) - Number(a.edge.ospf_enabled) ||
    labelRoom(a) - labelRoom(b) ||
    a.edge.plan_id.localeCompare(b.edge.plan_id),
  );
  const routeById = new Map(routes.map((route) => [route.edge.plan_id, route]));
  function* candidates(route: GraphRoute): Generator<Label> {
    const size = textSizes[`edge:${route.edge.plan_id}`];
    const width = (size?.width ?? 160) + 12;
    const labelHeight = Math.max(LABEL_HEIGHT, (size?.height ?? 14) + 8);
    const length = Math.hypot(
      route.end.x - route.start.x,
      route.end.y - route.start.y,
    ) || 1;
    const normal = {
      x: -(route.end.y - route.start.y) / length,
      y: (route.end.x - route.start.x) / length,
    };
    // A wider name-and-cost label may need to clear both its own half-width and
    // a node's radius. Search that distance in the existing label-sized lanes.
    const offsetLanes = Math.ceil(
      (GRAPH_NODE_RADIUS + width / 2 + CLEARANCE) / LANE_SPACING,
    );
    const offsets = [0];
    for (let lane = 1; lane <= offsetLanes; lane++) {
      offsets.push(lane * LANE_SPACING, -lane * LANE_SPACING);
    }
    // Fill narrow gaps between preferred lanes at the same precision as fine
    // node movement; the measured label width bounds this search.
    for (let offset = 1; offset < offsetLanes * LANE_SPACING; offset++) {
      if (offset % LANE_SPACING !== 0) offsets.push(offset, -offset);
    }
    const pathSteps = Math.ceil(length / CLEARANCE);
    const fractions = [0.5, 0.35, 0.65, 0.2, 0.8, 0, 1];
    // Use the existing clearance for the additional along-path spacing.
    fractions.push(...Array.from({ length: pathSteps - 1 }, (_, i) =>
      (i + 1) / pathSteps,
    ).sort((a, b) => Math.abs(a - 0.5) - Math.abs(b - 0.5)));
    for (const offset of offsets) {
      for (const t of fractions) {
        const anchor = graphRoutePoint(route, t);
        const rect = {
          x: anchor.x + normal.x * offset,
          y: anchor.y + normal.y * offset,
          width,
          height: labelHeight,
        };
        if (
          rect.x - width / 2 < CLEARANCE ||
          rect.x + width / 2 > GRAPH_WIDTH - CLEARANCE ||
          rect.y - labelHeight / 2 < CLEARANCE ||
          rect.y + labelHeight / 2 > height - CLEARANCE ||
          nodeBounds.some((other) => overlaps(rect, other))
        ) continue;
        yield { ...rect, anchor };
      }
    }
  }
  for (const route of ordered) {
    const placed = [...labels];
    for (const candidate of candidates(route)) {
      if (placed.some(([, other]) => overlaps(candidate, other))) continue;
      labels.set(route.edge.plan_id, candidate);
      break;
    }
    if (labels.has(route.edge.plan_id)) continue;
    // An earlier midpoint label can consume space that would fit two labels
    // nearer the path ends. Try moving that one blocker before hiding this label;
    // one relocation avoids a recursive search through the entire graph.
    const alternativesById = new Map<string, Label[]>();
    for (const candidate of candidates(route)) {
      const blockers = placed.filter(([, other]) => overlaps(candidate, other));
      if (blockers.length !== 1) continue;
      const [blockedId] = blockers[0];
      let alternatives = alternativesById.get(blockedId);
      if (!alternatives) {
        alternatives = [...candidates(routeById.get(blockedId)!)].filter(
          (alternative) => !placed.some(
            ([id, other]) => id !== blockedId && overlaps(alternative, other),
          ),
        );
        alternativesById.set(blockedId, alternatives);
      }
      const alternative = alternatives.find((other) => !overlaps(other, candidate));
      if (alternative) {
        labels.set(blockedId, alternative);
        labels.set(route.edge.plan_id, candidate);
      }
      if (labels.has(route.edge.plan_id)) break;
    }
    // If no space remains, the path's focus/hover title and summary still expose
    // the complete plan. Density never switches every label off at once.
  }
  return labels;
}
