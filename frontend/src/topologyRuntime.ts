import type {
  AllocateTunnelEndpointsRequest,
  OspfCostPolicy,
  RuntimeTunnelControl,
  RuntimeTunnelFouKind,
  RuntimeTunnelFouOptions,
  RuntimeTunnelManager,
  RuntimeTunnelOpenvpnOptions,
  RuntimeTunnelOpenvpnTransport,
  RuntimeTunnelRoute,
  RuntimeTunnelTopologyIntent,
  RuntimeTunnelWireguardEndpointMode,
  RuntimeTunnelWireguardOptions,
  TunnelKind,
  TunnelAdditionalAddresses,
  TunnelPlanInput,
  TunnelOspfConfig,
} from "./types";
import {
  FOU_TUNNEL_DEFAULTS,
  FOU_TUNNEL_KINDS,
  FOU_TUNNEL_KIND_DETAILS,
} from "./generated/protocolContracts";
export { FOU_TUNNEL_KINDS, FOU_TUNNEL_KIND_DETAILS } from "./generated/protocolContracts";

export type TunnelAdditionalAddressDraft = {
  left: { ipv4: string; ipv6: string };
  right: { ipv4: string; ipv6: string };
};

export function additionalAddressDraft(addresses?: TunnelAdditionalAddresses): TunnelAdditionalAddressDraft {
  return {
    left: { ipv4: addresses?.left.ipv4.join("\n") ?? "", ipv6: addresses?.left.ipv6.join("\n") ?? "" },
    right: { ipv4: addresses?.right.ipv4.join("\n") ?? "", ipv6: addresses?.right.ipv6.join("\n") ?? "" },
  };
}

export function additionalAddressLines(value: string): string[] {
  return value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
}

export function additionalAddressesFromDraft(draft: TunnelAdditionalAddressDraft): TunnelAdditionalAddresses {
  return {
    left: { ipv4: additionalAddressLines(draft.left.ipv4), ipv6: additionalAddressLines(draft.left.ipv6) },
    right: { ipv4: additionalAddressLines(draft.right.ipv4), ipv6: additionalAddressLines(draft.right.ipv6) },
  };
}

export type TunnelAllocationDraft = {
  includeIpv4: boolean;
  includeIpv6: boolean;
  ipv4Pool: string;
  ipv6Pool: string;
  ipv4Prefix: string;
  ipv6Prefix: string;
  leftIpv4: string;
  rightIpv4: string;
  leftIpv6: string;
  rightIpv6: string;
};

export function buildTunnelAllocationRequest(
  draft: TunnelAllocationDraft,
  planId?: string,
): AllocateTunnelEndpointsRequest {
  function family(enabled: boolean, label: string, prefix: string, max: number, left: string, right: string) {
    if (!enabled) return { prefix: undefined, preferred: null };
    const length = Number(prefix);
    if (!/^\d+$/.test(prefix.trim()) || !Number.isInteger(length) || length < 0 || length > max) {
      throw new Error(`${label} prefix must be a whole number from 0 to ${max}`);
    }
    const local = left.trim(), peer = right.trim();
    return { prefix: length, preferred: local || peer ? { left: local, right: peer, prefix_len: length } : null };
  }
  const ipv4 = family(draft.includeIpv4, "IPv4", draft.ipv4Prefix, 31, draft.leftIpv4, draft.rightIpv4);
  const ipv6 = family(draft.includeIpv6, "IPv6", draft.ipv6Prefix, 127, draft.leftIpv6, draft.rightIpv6);
  return {
    plan_id: planId ?? null,
    include_ipv4: draft.includeIpv4,
    include_ipv6: draft.includeIpv6,
    ipv4_pool_cidr: draft.ipv4Pool.trim() || null,
    ipv6_pool_cidr: draft.ipv6Pool.trim() || null,
    ipv4_prefix_len: ipv4.prefix,
    ipv6_prefix_len: ipv6.prefix,
    preferred_ipv4_tunnel: ipv4.preferred,
    preferred_ipv6_tunnel: ipv6.preferred,
  };
}

export function isTunnelLinkLocal(address: string): boolean {
  return /^fe[89ab][0-9a-f]:/i.test(address.trim());
}

export function tunnelLinkLocalSummary(
  input: Pick<TunnelPlanInput, "ipv6_tunnel" | "additional_addresses" | "manage_link_local">,
  side: "left" | "right",
): string {
  const addresses = [input.ipv6_tunnel?.[side], ...(input.additional_addresses?.[side].ipv6 ?? [])]
    .filter((address): address is string => Boolean(address));
  const manual = addresses.filter(isTunnelLinkLocal);
  if (input.manage_link_local === false) {
    return `Off · native behavior${manual.length ? `; explicit ${manual.join(", ")}` : ""}`;
  }
  return `On · automatic link-local${manual.length ? `; explicit ${manual.join(", ")}` : ""}`;
}

export function additionalAddressChanges(current: string[], previous: string[] = []): string {
  const added = current.filter((address) => !previous.includes(address));
  const removed = previous.filter((address) => !current.includes(address));
  return [
    added.length ? `Add ${added.join(", ")}` : null,
    removed.length ? `Remove ${removed.join(", ")}` : null,
  ].filter(Boolean).join("; ") || (current.join(", ") || "None");
}

export const DEFAULT_RUNTIME_FOU_OPTIONS: RuntimeTunnelFouOptions = FOU_TUNNEL_DEFAULTS;
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
export const MAX_TUNNEL_PLAN_NAME_BYTES = 128;
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
  fouTunnelKind?: RuntimeTunnelFouKind;
  wireguardEndpointMode?: RuntimeTunnelWireguardEndpointMode;
  wireguardLeftListenPort?: string;
  wireguardRightListenPort?: string;
  wireguardLeftKeepaliveSecs?: string;
  wireguardRightKeepaliveSecs?: string;
  openvpnTransport?: RuntimeTunnelOpenvpnTransport;
  openvpnListenerSide?: "left" | "right";
  openvpnPort?: string;
};

export function validateTunnelPlanName(name: string): string | null {
  if (!name.trim()) return "Plan name is required";
  if (new TextEncoder().encode(name).byteLength > MAX_TUNNEL_PLAN_NAME_BYTES) {
    return "Plan name must be 128 UTF-8 bytes or fewer";
  }
  return null;
}

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
  "The base cost uses latency, loss, bandwidth and preference with the configured weights, rounding and bounds. Each endpoint then applies (base + Add) × Multiply, rounds down to a multiple of Floor step, and keeps the minimum/maximum bounds. L is Left → Right; R is Right → Left. Manual speed-test evidence can downgrade effective bandwidth; bandwidth tests never run automatically.";

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

export function defaultAgentTunnelMtu(
  kind: TunnelKind,
  fouKind: RuntimeTunnelFouKind = DEFAULT_RUNTIME_FOU_OPTIONS.tunnel_kind,
): number | null {
  switch (kind) {
    case "gre":
      return 1476;
    case "gre6":
      return 1448; // IPv6 + GRE + the default encapsulation-limit option.
    case "ipip":
    case "sit":
      return 1480;
    case "fou":
      return FOU_TUNNEL_KIND_DETAILS[fouKind].default_mtu;
    case "wireguard":
      return 1420;
    case "openvpn":
      return 1500;
    case "tun_tap":
    case "custom":
      return null;
  }
}

export function fouRuntimeFacts(
  options: RuntimeTunnelFouOptions = DEFAULT_RUNTIME_FOU_OPTIONS,
): { label: string; value: string }[] {
  return [
    {
      label: "FOU tunnel",
      value: `${options.tunnel_kind.toUpperCase()} over UDP · IP protocol ${FOU_TUNNEL_KIND_DETAILS[options.tunnel_kind].ip_protocol}`,
    },
    { label: "FOU receive port", value: `${options.port} UDP (both VPSs)` },
    { label: "FOU peer port", value: `${options.peer_port} UDP` },
  ];
}

export function validateFouAddressFamilies(
  kind: RuntimeTunnelFouKind,
  ipv4: boolean,
  ipv6: boolean,
): string | null {
  const details = FOU_TUNNEL_KIND_DETAILS[kind];
  if (!details) return "FOU tunnel type must be GRE, GRE6, IPIP, or SIT";
  if (ipv4 && !details.ipv4) return "FOU SIT carries IPv6 only; remove IPv4 addresses or select GRE";
  if (ipv6 && !details.ipv6) return "FOU IPIP carries IPv4 only; remove IPv6 addresses or select GRE";
  return null;
}

export function isDerivedAgentTunnelMtu(
  kind: TunnelKind,
  mtu: number | null | undefined,
  fouKind: RuntimeTunnelFouKind = DEFAULT_RUNTIME_FOU_OPTIONS.tunnel_kind,
): boolean {
  if (mtu == null) return true;
  const defaultMtu = defaultAgentTunnelMtu(kind, fouKind);
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

type OspfDirectionalPolicy = Pick<
  TunnelOspfConfig,
  "left_cost_offset" | "right_cost_offset" | "left_cost_multiplier" | "right_cost_multiplier" | "cost_floor" | "policy"
>;

export function calculateDirectionalOspfCosts(
  baseCost: number,
  config: OspfDirectionalPolicy,
): { left: number | null; right: number | null } {
  return {
    left: adjustOspfCost(baseCost, config.left_cost_offset, config.left_cost_multiplier, config),
    right: adjustOspfCost(baseCost, config.right_cost_offset, config.right_cost_multiplier, config),
  };
}

// Match the server's exact arithmetic on the shortest decimal representation of
// each finite input. Binary floating-point multiplication would floor 100 × 1.15
// to 110 instead of 115 when the step is 5.
function decimalRatio(value: number): [bigint, bigint] {
  const [coefficient, exponentText] = value.toString().split("e");
  const [whole, fraction = ""] = coefficient.split(".");
  const exponent = Number(exponentText ?? 0) - fraction.length;
  const digits = BigInt(whole + fraction);
  return exponent >= 0
    ? [digits * 10n ** BigInt(exponent), 1n]
    : [digits, 10n ** BigInt(-exponent)];
}

function adjustOspfCost(
  base: number,
  offset: number,
  multiplier: number,
  { cost_floor: step, policy }: OspfDirectionalPolicy,
): number | null {
  if (
    !Number.isInteger(base) || !Number.isFinite(offset) ||
    !Number.isFinite(multiplier) || multiplier <= 0 ||
    !Number.isInteger(step) || step < 1 || step > 65535 ||
    !Number.isInteger(policy.min_cost) || !Number.isInteger(policy.max_cost) ||
    policy.min_cost < 1 || policy.min_cost > policy.max_cost || policy.max_cost > 65535
  ) return null;
  const [offsetNumerator, offsetDenominator] = decimalRatio(offset);
  const [multiplierNumerator, multiplierDenominator] = decimalRatio(multiplier);
  const numerator = (BigInt(base) * offsetDenominator + offsetNumerator) * multiplierNumerator;
  const denominator = offsetDenominator * multiplierDenominator;
  // Negative results and tiny positive values clamp to the minimum. Division
  // below therefore only sees positive operands, where BigInt truncation floors.
  if (numerator <= BigInt(policy.min_cost) * denominator) return policy.min_cost;
  const floored = numerator / (denominator * BigInt(step)) * BigInt(step);
  return Number(floored > BigInt(policy.max_cost)
    ? BigInt(policy.max_cost)
    : floored < BigInt(policy.min_cost) ? BigInt(policy.min_cost) : floored);
}

export function formatOspfCostPair(left: number | null | undefined, right: number | null | undefined): string {
  return `L ${left ?? "?"} / R ${right ?? "?"}`;
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
  if (values.fouTunnelKind && !FOU_TUNNEL_KINDS.includes(values.fouTunnelKind)) {
    throw new Error("FOU tunnel type must be GRE, GRE6, IPIP, or SIT");
  }
  const fou: RuntimeTunnelFouOptions = {
    port: numericValueOrDefault(
      values.fouPort,
      DEFAULT_RUNTIME_FOU_OPTIONS.port,
    ),
    peer_port: numericValueOrDefault(
      values.fouPeerPort,
      DEFAULT_RUNTIME_FOU_OPTIONS.peer_port,
    ),
    tunnel_kind: values.fouTunnelKind ?? DEFAULT_RUNTIME_FOU_OPTIONS.tunnel_kind,
  };
  if (
    fou.port === DEFAULT_RUNTIME_FOU_OPTIONS.port &&
    fou.peer_port === DEFAULT_RUNTIME_FOU_OPTIONS.peer_port &&
    fou.tunnel_kind === DEFAULT_RUNTIME_FOU_OPTIONS.tunnel_kind
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
