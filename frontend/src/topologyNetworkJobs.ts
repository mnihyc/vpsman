import type {
  JobOperation,
  TunnelEndpointSide,
  TunnelPlan,
} from "./types";

export type TunnelEndpointConfig = {
  localClientId: string;
  peerClientId: string;
  localUnderlay: string | null;
  remoteUnderlay: string;
  localAddress: string;
  remoteAddress: string;
  prefixLen: number;
  ipv4Address: EndpointAddressPair | null;
  ipv6Address: EndpointAddressPair | null;
};

type EndpointAddressPair = {
  local: string;
  remote: string;
  prefixLen: number;
};

export type NetworkSpeedDirection = "left_to_right" | "right_to_left";

export function renderTunnelEndpointConfig(
  plan: TunnelPlan,
  side: TunnelEndpointSide,
): TunnelEndpointConfig {
  const left = side === "left";
  return {
    localAddress: left ? plan.left_tunnel_address : plan.right_tunnel_address,
    localClientId: left ? plan.left_client_id : plan.right_client_id,
    localUnderlay: left ? (plan.left_local_underlay ?? null) : (plan.right_local_underlay ?? null),
    peerClientId: left ? plan.right_client_id : plan.left_client_id,
    prefixLen: plan.tunnel_prefix_len,
    remoteAddress: left ? plan.right_tunnel_address : plan.left_tunnel_address,
    remoteUnderlay: left ? plan.left_remote_underlay : plan.right_remote_underlay,
    ipv4Address: plan.ipv4_tunnel
      ? endpointAddressPair(plan.ipv4_tunnel, side)
      : null,
    ipv6Address: plan.ipv6_tunnel
      ? endpointAddressPair(plan.ipv6_tunnel, side)
      : null,
  };
}

export function buildNetworkStatusOperation(
  planId: string,
  plan: TunnelPlan,
  side: TunnelEndpointSide,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  return {
    endpoint: renderTunnelEndpointConfig(plan, side),
    operation: { type: "network_status", plan_id: planId, plan, side },
  };
}

export function buildNetworkProbeOperation(
  planId: string,
  plan: TunnelPlan,
  side: TunnelEndpointSide,
  count: number,
  intervalMs: number,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  return {
    endpoint: renderTunnelEndpointConfig(plan, side),
    operation: {
      type: "network_probe",
      plan_id: planId,
      plan,
      side,
      count,
      interval_ms: intervalMs,
    },
  };
}

export function buildNetworkSpeedTestOperation(
  planId: string,
  plan: TunnelPlan,
  direction: NetworkSpeedDirection,
  durationSecs: number,
  maxBytes: number,
  rateLimitKbps: number,
  port: number,
  connectTimeoutMs: number,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  const serverSide = networkSpeedServerSide(direction);
  return {
    endpoint: renderTunnelEndpointConfig(plan, serverSide),
    operation: {
      type: "network_speed_test",
      plan_id: planId,
      plan,
      server_side: serverSide,
      duration_secs: durationSecs,
      max_bytes: maxBytes,
      rate_limit_kbps: rateLimitKbps,
      port,
      connect_timeout_ms: connectTimeoutMs,
    },
  };
}

export function networkSpeedServerSide(
  direction: NetworkSpeedDirection,
): TunnelEndpointSide {
  return direction === "left_to_right" ? "right" : "left";
}

function endpointAddressPair(
  pair: { left: string; right: string; prefix_len: number },
  side: TunnelEndpointSide,
): EndpointAddressPair {
  return side === "left"
    ? { local: pair.left, remote: pair.right, prefixLen: pair.prefix_len }
    : { local: pair.right, remote: pair.left, prefixLen: pair.prefix_len };
}
