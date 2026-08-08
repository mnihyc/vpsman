import type {
  OspfCostPolicy,
  RuntimeTunnelControl,
  RuntimeTunnelFouOptions,
  RuntimeTunnelManager,
  RuntimeTunnelOpenvpnOptions,
  RuntimeTunnelOpenvpnTransport,
  RuntimeTunnelRoute,
  RuntimeTunnelTopologyIntent,
  RuntimeTunnelWireguardEndpointMode,
  RuntimeTunnelWireguardOptions,
  TunnelKind,
} from "./types";

export const DEFAULT_RUNTIME_FOU_OPTIONS: RuntimeTunnelFouOptions = {
  port: 5555,
  peer_port: 5555,
  ipproto: 4,
};
export const DEFAULT_RUNTIME_WIREGUARD_OPTIONS: RuntimeTunnelWireguardOptions = {
  endpoint_mode: "both",
  left_listen_port: 51820,
  right_listen_port: 51820,
  left_keepalive_secs: 25,
  right_keepalive_secs: 25,
};
export const DEFAULT_RUNTIME_OPENVPN_OPTIONS: RuntimeTunnelOpenvpnOptions = {
  transport: "udp",
  listener_side: "left",
  port: 1194,
};
export const MIN_TUNNEL_BANDWIDTH_MBPS = 10;
export const MAX_TUNNEL_BANDWIDTH_MBPS = 10000;
export const DEFAULT_TUNNEL_BANDWIDTH_MBPS = 100;
export const MIN_TUNNEL_MTU = 68;
export const MIN_IPV6_TUNNEL_MTU = 1280;
export const MAX_TUNNEL_MTU = 65535;
const OSPF_BANDWIDTH_REFERENCE_MBPS = 100;
const OSPF_BANDWIDTH_WEIGHT = 10;
const OSPF_LOSS_WEIGHT = 400;
const OSPF_MIN_COST = 5;
const OSPF_MAX_COST = 65535;

export type RuntimeControlFormValues = {
  leftAdapterDefinitionId?: string;
  rightAdapterDefinitionId?: string;
  ingressKbps: string;
  egressKbps: string;
  burstKb: string;
  fouPort?: string;
  fouPeerPort?: string;
  fouIpproto?: string;
  wireguardEndpointMode?: RuntimeTunnelWireguardEndpointMode;
  wireguardLeftListenPort?: string;
  wireguardRightListenPort?: string;
  wireguardLeftKeepaliveSecs?: string;
  wireguardRightKeepaliveSecs?: string;
  openvpnTransport?: RuntimeTunnelOpenvpnTransport;
  openvpnListenerSide?: "left" | "right";
  openvpnPort?: string;
};

export type RuntimeTopologyFormValues = {
  version?: string | null;
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
  const wireguard = buildWireguardOptions(values);
  const wireguardPayload = wireguard ? { wireguard } : {};
  const openvpn = buildOpenvpnOptions(values);
  const openvpnPayload = openvpn ? { openvpn } : {};
  if (manager === "external_observed") {
    return { manager, traffic_limit: {} };
  }
  if (manager === "custom_adapter") {
    return {
      manager,
      left_adapter_template_id: values.leftAdapterDefinitionId?.trim() || null,
      right_adapter_template_id: values.rightAdapterDefinitionId?.trim() || null,
      traffic_limit: trafficLimit,
      ...fouPayload,
      ...wireguardPayload,
      ...openvpnPayload,
    };
  }
  return {
    manager,
    traffic_limit: trafficLimit,
    ...fouPayload,
    ...wireguardPayload,
    ...openvpnPayload,
  };
}

function buildWireguardOptions(
  values: RuntimeControlFormValues,
): RuntimeTunnelWireguardOptions | null {
  if (!values.wireguardEndpointMode) return null;
  return {
    endpoint_mode: values.wireguardEndpointMode,
    left_listen_port:
      numericValue(values.wireguardLeftListenPort ?? "") ??
      DEFAULT_RUNTIME_WIREGUARD_OPTIONS.left_listen_port,
    right_listen_port:
      numericValue(values.wireguardRightListenPort ?? "") ??
      DEFAULT_RUNTIME_WIREGUARD_OPTIONS.right_listen_port,
    left_keepalive_secs:
      nonNegativeIntegerValue(values.wireguardLeftKeepaliveSecs ?? "") ??
      DEFAULT_RUNTIME_WIREGUARD_OPTIONS.left_keepalive_secs,
    right_keepalive_secs:
      nonNegativeIntegerValue(values.wireguardRightKeepaliveSecs ?? "") ??
      DEFAULT_RUNTIME_WIREGUARD_OPTIONS.right_keepalive_secs,
  };
}

function buildOpenvpnOptions(
  values: RuntimeControlFormValues,
): RuntimeTunnelOpenvpnOptions | null {
  if (!values.openvpnTransport || !values.openvpnListenerSide) return null;
  return {
    transport: values.openvpnTransport,
    listener_side: values.openvpnListenerSide,
    port:
      numericValue(values.openvpnPort ?? "") ??
      DEFAULT_RUNTIME_OPENVPN_OPTIONS.port,
  };
}

export function buildRuntimeTopology(
  values: RuntimeTopologyFormValues,
): RuntimeTunnelTopologyIntent {
  return {
    version: values.version?.trim() || undefined,
    desired_interfaces: splitList(values.desiredText),
    stale_interfaces: splitList(values.staleText),
    routes: parseRouteLines(values.routesText),
    stale_routes: parseRouteLines(values.staleRoutesText),
  };
}

export function isDefaultRuntimeTopology(
  topology: RuntimeTunnelTopologyIntent,
): boolean {
  return (
    !topology.version &&
    (topology.desired_interfaces?.length ?? 0) === 0 &&
    (topology.stale_interfaces?.length ?? 0) === 0 &&
    (topology.routes?.length ?? 0) === 0 &&
    (topology.stale_routes?.length ?? 0) === 0
  );
}

export const OSPF_COST_MODEL_DETAIL =
  "cost = clamp(round((latency_ms + loss_ratio * 400 + 10 * sqrt(100 / clamp(bandwidth_mbps, 10, 10000))) / max(preference, 0.1)), 5, 65535). The sqrt bandwidth term gives diminishing returns across arbitrary Mbps values, so low bandwidth is visible but high bandwidth cannot hide bad latency or loss. Manual speed-test evidence can downgrade effective bandwidth; bandwidth tests never run automatically.";

export const OSPF_COST_MODEL_SUMMARY =
  "Latency/loss plus a bounded sqrt bandwidth penalty; evidence is explicit and automatic changes are controlled by the server per plan.";

export function normalizeTunnelBandwidthMbps(value: unknown): number {
  const numeric = Number(value);
  if (!Number.isFinite(numeric)) {
    return DEFAULT_TUNNEL_BANDWIDTH_MBPS;
  }
  return Math.round(numeric);
}

export function clampTunnelBandwidthMbps(value: unknown): number {
  return Math.min(
    MAX_TUNNEL_BANDWIDTH_MBPS,
    Math.max(MIN_TUNNEL_BANDWIDTH_MBPS, normalizeTunnelBandwidthMbps(value)),
  );
}

export function defaultAgentTunnelMtu(kind: TunnelKind): number | null {
  switch (kind) {
    case "gre":
      return 1476;
    case "ipip":
    case "sit":
      return 1480;
    case "fou":
      return 1472;
    case "wireguard":
      return 1420;
    case "openvpn":
      return 1500;
    case "tun_tap":
    case "custom":
      return null;
  }
}

export function isDerivedAgentTunnelMtu(
  kind: TunnelKind,
  mtu: number | null | undefined,
): boolean {
  if (mtu == null) return true;
  const defaultMtu = defaultAgentTunnelMtu(kind);
  return defaultMtu !== null && mtu === defaultMtu;
}

export function calculateOspfCostPreview({
  bandwidthMbps,
  latencyMs,
  packetLossRatio,
  policy,
  preference,
}: {
  bandwidthMbps: number;
  latencyMs: number;
  packetLossRatio: number;
  policy?: OspfCostPolicy;
  preference: number;
}): number {
  const bandwidth = clampTunnelBandwidthMbps(bandwidthMbps);
  const latency = Math.max(0, Number.isFinite(latencyMs) ? latencyMs : 0);
  const loss = Math.min(
    1,
    Math.max(0, Number.isFinite(packetLossRatio) ? packetLossRatio : 0),
  );
  const preferenceBias = Math.max(
    0.1,
    Number.isFinite(preference) ? preference : 1,
  );
  const effectivePolicy = policy ?? {
    bandwidth_weight: OSPF_BANDWIDTH_WEIGHT,
    latency_weight: 1,
    loss_weight: OSPF_LOSS_WEIGHT,
    max_cost: OSPF_MAX_COST,
    min_cost: OSPF_MIN_COST,
    preference_bias: 1,
  };
  const bandwidthPenalty =
    effectivePolicy.bandwidth_weight *
    Math.sqrt(OSPF_BANDWIDTH_REFERENCE_MBPS / bandwidth);
  const raw =
    latency * effectivePolicy.latency_weight +
    loss * effectivePolicy.loss_weight +
    bandwidthPenalty;
  return Math.min(
    effectivePolicy.max_cost,
    Math.max(
      effectivePolicy.min_cost,
      Math.round((raw * effectivePolicy.preference_bias) / preferenceBias),
    ),
  );
}

export function runtimeManagerLabel(
  manager: RuntimeTunnelManager | string | null | undefined,
): string {
  if (manager === "external_observed") {
    return "External observed";
  }
  if (manager === "custom_adapter") {
    return "Custom adapter";
  }
  if (manager === "agent_builtin" || !manager) {
    return "Agent builtin";
  }
  return readableTelemetryToken(manager);
}

export function latencyStatusLabel(status: string | null | undefined): string {
  switch (status) {
    case "healthy":
      return "Healthy";
    case "down":
      return "Probe failed";
    case "missed":
      return "Probe missed";
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

export function ospfStatusLabel(
  status: string | null | undefined,
  enabled?: boolean | null,
): string {
  switch (status) {
    case "verified":
      return "Verified";
    case "unverified":
      return "Check required";
    case "stale":
      return "Stale";
    case "partial":
      return "Partial";
    case "failed":
      return "Failed";
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

export function telemetryReasonLabel(
  reason: string | null | undefined,
): string {
  if (!reason) {
    return "";
  }
  const [key, suffix] = reason.split(":", 2);
  const label = telemetryReasonLabelByKey(key);
  return suffix ? `${label} (${suffix})` : label;
}

export function telemetrySourceLabel(
  source: string | null | undefined,
): string {
  switch (source) {
    case "approved_runtime_status_telemetry":
      return "Agent telemetry";
    case "sysfs_proc_net_dev":
      return "Kernel counters";
    case "interface_counters":
      return "Interface counters";
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
    } else if (
      key === "dev" ||
      key === "interface" ||
      key === "interface_name"
    ) {
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

function nonNegativeIntegerValue(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = Number(trimmed);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`Invalid non-negative integer value ${value}`);
  }
  return parsed;
}

function buildFouOptions(
  values: RuntimeControlFormValues,
): RuntimeTunnelFouOptions | undefined {
  const fou: RuntimeTunnelFouOptions = {
    port: numericValueOrDefault(
      values.fouPort,
      DEFAULT_RUNTIME_FOU_OPTIONS.port,
    ),
    peer_port: numericValueOrDefault(
      values.fouPeerPort,
      DEFAULT_RUNTIME_FOU_OPTIONS.peer_port,
    ),
    ipproto: numericValueOrDefault(
      values.fouIpproto,
      DEFAULT_RUNTIME_FOU_OPTIONS.ipproto,
    ),
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

function numericValueOrDefault(
  value: string | undefined,
  fallback: number,
): number {
  if (value === undefined || value.trim() === "") {
    return fallback;
  }
  return numericValue(value) ?? fallback;
}
