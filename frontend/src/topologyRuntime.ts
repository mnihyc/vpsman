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
    ipv4_tunnel: normalizeTunnelAddressPair(request.ipv4_tunnel ?? null),
    ipv6_address_pool_cidr: request.ipv6_address_pool_cidr?.trim() || null,
    ipv6_tunnel: normalizeTunnelAddressPair(request.ipv6_tunnel ?? null),
    latency_primary_family: request.latency_primary_family ?? "ipv4",
    name: request.name?.trim() || undefined,
    topology_version: request.topology_version?.trim() || undefined,
    bandwidth: request.bandwidth ?? "100m",
    latency_ms: request.latency_ms ?? 20,
    packet_loss_ratio: request.packet_loss_ratio ?? 0,
    preference: request.preference ?? 1,
  };
}

function normalizeTunnelAddressPair(pair: PromoteTelemetryTunnelRequest["ipv4_tunnel"]) {
  if (!pair) {
    return null;
  }
  const left = pair.left.trim();
  const right = pair.right.trim();
  if (!left || !right) {
    return null;
  }
  return { left, right, prefix_len: pair.prefix_len };
}

export const OSPF_COST_MODEL_DETAIL =
  "cost = clamp(round((latency_ms + loss_ratio * 400 + 150 / bandwidth_mbps) / max(preference, 0.1)), 5, 65535). Tiers: 10m=10 Mbps, 100m=100 Mbps, 1000m=1000 Mbps. Manual speed-test evidence can downgrade effective bandwidth at 800 Mbps and 80 Mbps thresholds; bandwidth tests never run automatically.";

export const OSPF_COST_MODEL_SUMMARY =
  "Latency/loss plus bandwidth tier. Manual speed tests only.";

export function runtimeManagerLabel(manager: RuntimeTunnelManager | string | null | undefined): string {
  if (manager === "external_observed") {
    return "External observed";
  }
  if (manager === "external_managed_adapter") {
    return "External adapter";
  }
  if (manager === "agent_iproute2_managed" || !manager) {
    return "Agent iproute2";
  }
  return readableTelemetryToken(manager);
}

export function latencyStatusLabel(status: string | null | undefined): string {
  switch (status) {
    case "healthy":
      return "Healthy";
    case "down":
      return "Down";
    case "missed":
      return "Missing";
    case "unconfigured":
      return "Not configured";
    case "disabled":
      return "Off";
    case "pending":
      return "Pending";
    case "no_latency":
    case null:
    case undefined:
      return "No samples";
    default:
      return readableTelemetryToken(status);
  }
}

export function ospfStatusLabel(status: string | null | undefined, enabled?: boolean | null): string {
  switch (status) {
    case "updated":
      return "Updated";
    case "stable":
      return "Stable";
    case "failed":
      return "Failed";
    case "report_only":
      return "Report only";
    case "stabilizing":
      return "Stabilizing";
    case "monitoring_only":
      return "Monitoring only";
    case "disabled":
      return "Off";
    case "pending":
      return "Pending";
    case null:
    case undefined:
      return enabled ? "Pending" : "Off";
    default:
      return readableTelemetryToken(status);
  }
}

export function telemetryReasonLabel(reason: string | null | undefined): string {
  if (!reason) {
    return "";
  }
  const [key, suffix] = reason.split(":", 2);
  const label = telemetryReasonLabelByKey(key);
  return suffix ? `${label} (${suffix})` : label;
}

export function telemetrySourceLabel(source: string | null | undefined): string {
  switch (source) {
    case "approved_runtime_status_telemetry":
      return "Agent telemetry";
    case "sysfs_proc_net_dev":
      return "Kernel counters";
    case "interface_counters":
      return "Interface counters";
    case "vnstat":
      return "vnStat";
    case null:
    case undefined:
      return "Source unknown";
    default:
      return readableTelemetryToken(source);
  }
}

export function mutationPolicyLabel(policy: string | null | undefined): string {
  switch (policy) {
    case "managed_desired":
      return "Managed desired";
    case "observe_only_saved_plan":
      return "Observed only";
    case "unmanaged_observed":
      return "Observed";
    case null:
    case undefined:
      return "Policy unknown";
    default:
      return readableTelemetryToken(policy);
  }
}

export function planCorrelationLabel(correlation: string | null | undefined): string {
  switch (correlation) {
    case "matched_saved_plan":
      return "Saved plan";
    case "unmatched":
      return "Unmatched";
    case null:
    case undefined:
      return "Plan unknown";
    default:
      return readableTelemetryToken(correlation);
  }
}

export function trafficStatusLabel(status: string | null | undefined): string {
  if (!status || status === "ok") {
    return "OK";
  }
  return readableTelemetryToken(status);
}

export function readableTelemetryToken(value: string): string {
  const normalized = value.replace(/[_-]+/g, " ").trim();
  if (!normalized) {
    return "Unknown";
  }
  if (normalized.length <= 3) {
    return normalized.toUpperCase();
  }
  return normalized[0].toUpperCase() + normalized.slice(1);
}

function telemetryReasonLabelByKey(key: string): string {
  switch (key) {
    case "probe_ok":
      return "Probe OK";
    case "external_cost_program_succeeded":
      return "Updater applied";
    case "external_cost_program_unconfigured":
      return "No external updater";
    case "external_cost_program_failed":
      return "Updater failed";
    case "latency_probe_unhealthy_ospf_handles_dead_adjacency":
      return "Adjacency down; OSPF handles failover";
    case "latency_probe_missing_healthy_sample":
      return "Waiting for healthy probes";
    case "latency_probe_disabled":
      return "Latency monitor off";
    case "adapter_status_failed":
      return "Adapter status failed";
    case "adapter_status_ok":
      return "Adapter healthy";
    case "traffic_accounting_unavailable":
      return "Traffic counters unavailable";
    default:
      return readableTelemetryToken(key);
  }
}

export function endpointSideLabel(side: string | null | undefined): string {
  switch (side) {
    case "left":
      return "Left side";
    case "right":
      return "Right side";
    case null:
    case undefined:
      return "Endpoint";
    default:
      return readableTelemetryToken(side);
  }
}

export function addressFamilyLabel(family: string | null | undefined): string {
  switch (family) {
    case "ipv4":
      return "IPv4";
    case "ipv6":
      return "IPv6";
    case null:
    case undefined:
      return "IP family";
    default:
      return readableTelemetryToken(family);
  }
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
