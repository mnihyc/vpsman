import type {
  PromoteTelemetryTunnelRequest,
  RuntimeTunnelCommand,
  RuntimeTunnelControl,
  RuntimeTunnelFouOptions,
  RuntimeTunnelManager,
  RuntimeTunnelRoute,
  RuntimeTunnelTopologyIntent,
} from "./types";

export const DEFAULT_RUNTIME_FOU_OPTIONS: RuntimeTunnelFouOptions = {
  port: 5555,
  peer_port: 5555,
  ipproto: 4,
};

export type RuntimeControlFormValues = {
  startup: string;
  stop: string;
  cleanup: string;
  restart: string;
  status: string;
  traffic: string;
  ingressKbps: string;
  egressKbps: string;
  burstKb: string;
  fouPort?: string;
  fouPeerPort?: string;
  fouIpproto?: string;
};

export type RuntimeTopologyFormValues = {
  version: string;
  desiredText: string;
  staleText: string;
  routesText: string;
  staleRoutesText: string;
};

export function buildRuntimeControl(
  manager: RuntimeTunnelManager,
  values: RuntimeControlFormValues,
): RuntimeTunnelControl {
  const trafficLimit = {
    ingress_kbps: numericValue(values.ingressKbps),
    egress_kbps: numericValue(values.egressKbps),
    burst_kb: numericValue(values.burstKb),
  };
  const fou = buildFouOptions(values);
  const fouPayload = fou ? { fou } : {};
  if (manager === "external_observed") {
    return { manager, traffic_limit: {}, ...fouPayload };
  }
  if (manager === "external_managed_adapter") {
    return {
      manager,
      startup: commandFromText(values.startup),
      stop: commandFromText(values.stop),
      cleanup: commandFromText(values.cleanup),
      restart: commandFromText(values.restart),
      status: commandFromText(values.status),
      traffic_limit_apply: commandFromText(values.traffic),
      traffic_limit: trafficLimit,
      ...fouPayload,
    };
  }
  return { manager, traffic_limit: trafficLimit, ...fouPayload };
}

export function buildRuntimeTopology(values: RuntimeTopologyFormValues): RuntimeTunnelTopologyIntent {
  return {
    version: values.version.trim() || undefined,
    desired_interfaces: splitList(values.desiredText),
    stale_interfaces: splitList(values.staleText),
    routes: parseRouteLines(values.routesText),
    stale_routes: parseRouteLines(values.staleRoutesText),
  };
}

export function isDefaultRuntimeTopology(topology: RuntimeTunnelTopologyIntent): boolean {
  return (
    !topology.version &&
    (topology.desired_interfaces?.length ?? 0) === 0 &&
    (topology.stale_interfaces?.length ?? 0) === 0 &&
    (topology.routes?.length ?? 0) === 0 &&
    (topology.stale_routes?.length ?? 0) === 0
  );
}

export function normalizeTelemetryPromotionRequest(
  request: PromoteTelemetryTunnelRequest,
): PromoteTelemetryTunnelRequest {
  return {
    ...request,
    client_id: request.client_id.trim(),
    interface: request.interface.trim(),
    peer_client_id: request.peer_client_id.trim(),
    local_underlay: request.local_underlay.trim(),
    peer_underlay: request.peer_underlay.trim(),
    address_pool_cidr: request.address_pool_cidr.trim(),
    name: request.name?.trim() || undefined,
    topology_version: request.topology_version?.trim() || undefined,
    bandwidth: request.bandwidth ?? "100m",
    latency_ms: request.latency_ms ?? 20,
    packet_loss_ratio: request.packet_loss_ratio ?? 0,
    preference: request.preference ?? 1,
  };
}

export function runtimeManagerLabel(manager: RuntimeTunnelManager | undefined): string {
  if (manager === "external_observed") {
    return "External observed";
  }
  if (manager === "external_managed_adapter") {
    return "External adapter";
  }
  return "Agent iproute2";
}

function commandFromText(value: string): RuntimeTunnelCommand | undefined {
  const argv = value
    .split(/[\n,]/)
    .map((part) => part.trim())
    .filter(Boolean);
  return argv.length > 0 ? { argv } : undefined;
}

function splitList(value: string): string[] {
  return value
    .split(/[\n,]/)
    .map((part) => part.trim())
    .filter(Boolean);
}

function parseRouteLines(value: string): RuntimeTunnelRoute[] {
  return value
    .split(/\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map(parseRouteLine);
}

function parseRouteLine(value: string): RuntimeTunnelRoute {
  const [destination_cidr, ...options] = value
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean);
  if (!destination_cidr) {
    throw new Error("Route destination CIDR is required");
  }
  const route: RuntimeTunnelRoute = { destination_cidr };
  for (const option of options) {
    const [key, optionValue] = option.split("=", 2);
    if (!key || !optionValue) {
      throw new Error(`Invalid route option ${option}`);
    }
    if (key === "via") {
      route.via = optionValue;
    } else if (key === "dev" || key === "interface" || key === "interface_name") {
      route.interface_name = optionValue;
    } else if (key === "metric") {
      route.metric = Number(optionValue);
    } else {
      throw new Error(`Unknown route option ${key}`);
    }
  }
  return route;
}

function numericValue(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    throw new Error(`Invalid numeric value ${value}`);
  }
  return Math.trunc(parsed);
}

function buildFouOptions(values: RuntimeControlFormValues): RuntimeTunnelFouOptions | undefined {
  const fou: RuntimeTunnelFouOptions = {
    port: numericValueOrDefault(values.fouPort, DEFAULT_RUNTIME_FOU_OPTIONS.port),
    peer_port: numericValueOrDefault(values.fouPeerPort, DEFAULT_RUNTIME_FOU_OPTIONS.peer_port),
    ipproto: numericValueOrDefault(values.fouIpproto, DEFAULT_RUNTIME_FOU_OPTIONS.ipproto),
  };
  if (
    fou.port === DEFAULT_RUNTIME_FOU_OPTIONS.port &&
    fou.peer_port === DEFAULT_RUNTIME_FOU_OPTIONS.peer_port &&
    fou.ipproto === DEFAULT_RUNTIME_FOU_OPTIONS.ipproto
  ) {
    return undefined;
  }
  return fou;
}

function numericValueOrDefault(value: string | undefined, fallback: number): number {
  if (value === undefined || value.trim() === "") {
    return fallback;
  }
  return numericValue(value) ?? fallback;
}
