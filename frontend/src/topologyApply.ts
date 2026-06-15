import { sha256Hex } from "./fileTransfer";
import type {
  JobOperation,
  RuntimeTunnelFouOptions,
  RuntimeTunnelManager,
  TunnelConfigBackend,
  TunnelEndpointSide,
  TunnelKind,
  TunnelPlan,
} from "./types";
import { DEFAULT_RUNTIME_FOU_OPTIONS } from "./topologyRuntime";
import { networkBackendFilePresets } from "./presets/networkBackendPresets";

const encoder = new TextEncoder();

export type TunnelEndpointConfig = {
  localClientId: string;
  peerClientId: string;
  ifupdownSnippet: string;
  bird2InterfaceSnippet: string;
  localUnderlay: string;
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

export function renderTunnelEndpointConfig(plan: TunnelPlan, side: TunnelEndpointSide): TunnelEndpointConfig {
  const left = side === "left";
  const localClientId = left ? plan.left_client_id : plan.right_client_id;
  const peerClientId = left ? plan.right_client_id : plan.left_client_id;
  const ipv4Address = plan.ipv4_tunnel ? endpointAddressPair(plan.ipv4_tunnel, side) : null;
  const ipv6Address = plan.ipv6_tunnel ? endpointAddressPair(plan.ipv6_tunnel, side) : null;
  return {
    localClientId,
    peerClientId,
    localUnderlay: left ? plan.left_underlay : plan.right_underlay,
    remoteUnderlay: left ? plan.right_underlay : plan.left_underlay,
    localAddress: left ? plan.left_tunnel_address : plan.right_tunnel_address,
    remoteAddress: left ? plan.right_tunnel_address : plan.left_tunnel_address,
    prefixLen: plan.tunnel_prefix_len,
    ipv4Address,
    ipv6Address,
    ifupdownSnippet: renderRuntimeSnippet(
      {
        kind: plan.kind,
        localUnderlay: left ? plan.left_underlay : plan.right_underlay,
        name: plan.name,
        remoteUnderlay: left ? plan.right_underlay : plan.left_underlay,
        interfaceName: plan.interface_name,
        ipv4: ipv4Address,
        ipv6: ipv6Address,
        fou: runtimeFouOptions(plan.kind, plan.runtime_control?.fou),
      },
      plan.runtime_control?.manager ?? "agent_iproute2_managed",
    ),
    bird2InterfaceSnippet: renderBird2InterfaceSnippet(
      plan.kind,
      plan.name,
      plan.interface_name,
      localClientId,
      peerClientId,
      plan.recommended_ospf_cost,
    ),
  };
}

function endpointAddressPair(
  pair: { left: string; right: string; prefix_len: number },
  side: TunnelEndpointSide,
): EndpointAddressPair {
  return side === "left"
    ? { local: pair.left, remote: pair.right, prefixLen: pair.prefix_len }
    : { local: pair.right, remote: pair.left, prefixLen: pair.prefix_len };
}

export async function buildNetworkApplyOperation(
  plan: TunnelPlan,
  side: TunnelEndpointSide,
  backend: TunnelConfigBackend = "ifupdown",
): Promise<{ endpoint: TunnelEndpointConfig; operation: JobOperation }> {
  const endpoint = renderTunnelEndpointConfig(plan, side);
  const backendConfig = renderBackendConfig(plan, endpoint, backend);
  return {
    endpoint,
    operation: {
      type: "network_apply",
      plan,
      side,
      config_backend: backend,
      config_sha256_hex: await sha256Text(backendSignaturePayload(backendConfig, backend)),
      ifupdown_sha256_hex: await sha256Text(endpoint.ifupdownSnippet),
      bird2_sha256_hex: await sha256Text(endpoint.bird2InterfaceSnippet),
    },
  };
}

export async function buildNetworkOspfCostUpdateOperation(
  plan: TunnelPlan,
  side: TunnelEndpointSide,
  currentOspfCost: number,
  recommendedOspfCost: number,
): Promise<{ endpoint: TunnelEndpointConfig; operation: JobOperation }> {
  const proposedPlan = { ...plan, recommended_ospf_cost: recommendedOspfCost };
  const endpoint = renderTunnelEndpointConfig(proposedPlan, side);
  return {
    endpoint,
    operation: {
      type: "network_ospf_cost_update",
      plan: proposedPlan,
      side,
      current_ospf_cost: currentOspfCost,
      recommended_ospf_cost: recommendedOspfCost,
      bird2_sha256_hex: await sha256Text(endpoint.bird2InterfaceSnippet),
    },
  };
}

export function buildNetworkRollbackOperation(
  plan: TunnelPlan,
  side: TunnelEndpointSide,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  const endpoint = renderTunnelEndpointConfig(plan, side);
  return {
    endpoint,
    operation: {
      type: "network_rollback",
      plan,
      side,
    },
  };
}

export function buildNetworkStatusOperation(
  plan: TunnelPlan,
  side: TunnelEndpointSide,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  const endpoint = renderTunnelEndpointConfig(plan, side);
  return {
    endpoint,
    operation: {
      type: "network_status",
      plan,
      side,
    },
  };
}

export function buildNetworkProbeOperation(
  plan: TunnelPlan,
  side: TunnelEndpointSide,
  count: number,
  intervalMs: number,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  const endpoint = renderTunnelEndpointConfig(plan, side);
  return {
    endpoint,
    operation: {
      type: "network_probe",
      plan,
      side,
      count,
      interval_ms: intervalMs,
    },
  };
}

export function buildNetworkSpeedTestOperation(
  plan: TunnelPlan,
  serverSide: TunnelEndpointSide,
  durationSecs: number,
  maxBytes: number,
  rateLimitKbps: number,
  port: number,
  connectTimeoutMs: number,
): { endpoint: TunnelEndpointConfig; operation: JobOperation } {
  const endpoint = renderTunnelEndpointConfig(plan, serverSide);
  return {
    endpoint,
    operation: {
      type: "network_speed_test",
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

function sha256Text(value: string): Promise<string> {
  return sha256Hex(encoder.encode(value));
}

type BackendFile = {
  managedPath: string;
  blockKind: string;
  contents: string;
};

function renderBackendConfig(plan: TunnelPlan, endpoint: TunnelEndpointConfig, backend: TunnelConfigBackend): BackendFile[] {
  if ((plan.runtime_control?.manager ?? "agent_iproute2_managed") !== "agent_iproute2_managed") {
    return [];
  }
  if (backend === "ifupdown") {
    return networkBackendFilePresets(backend).map((preset) => ({
      ...preset,
      contents: endpoint.ifupdownSnippet,
    }));
  }
  if (backend === "netplan") {
    if (plan.kind === "fou" || !isLinuxTunnelKind(plan.kind)) {
      throw new Error("Netplan backend does not support this tunnel rendering");
    }
    return networkBackendFilePresets(backend).map((preset) => ({
      ...preset,
      contents: renderNetplanSnippet(plan, endpoint),
    }));
  }
  const [netdevPreset, networkPreset] = networkBackendFilePresets(backend);
  return [
    {
      ...netdevPreset,
      contents: renderSystemdNetdevSnippet(plan, endpoint),
    },
    {
      ...networkPreset,
      contents: renderSystemdNetworkSnippet(plan, endpoint),
    },
  ];
}

function backendSignaturePayload(files: BackendFile[], backend: TunnelConfigBackend): string {
  return files
    .map(
      (file) =>
        `vpsman-network-backend-file-v1\nbackend=${backend}\npath=${file.managedPath}\nkind=${file.blockKind}\ncontents-sha256-context\n${file.contents}\n`,
    )
    .join("");
}

function renderIfupdownSnippet(input: {
  name: string;
  interfaceName: string;
  kind: TunnelKind;
  localUnderlay: string;
  remoteUnderlay: string;
  ipv4: EndpointAddressPair | null;
  ipv6: EndpointAddressPair | null;
  fou: RuntimeTunnelFouOptions;
}): string {
  if (!isLinuxTunnelKind(input.kind)) {
    throw new Error("iproute2-managed rendering requires GRE, IPIP, SIT, or FOU");
  }
  const lines = [`# vpsman tunnel ${input.name}: generated plan only`];
  if (input.ipv4) {
    lines.push(...renderIfupdownIpv4Stanza(input, input.ipv4, true));
  }
  if (input.ipv6) {
    lines.push(...renderIfupdownIpv6Stanza(input, input.ipv6, !input.ipv4));
  }
  return lines.join("\n");
}

function renderIfupdownIpv4Stanza(
  input: Parameters<typeof renderIfupdownSnippet>[0],
  address: EndpointAddressPair,
  includeLifecycle: boolean,
): string[] {
  const lines = [
    `auto ${input.interfaceName}`,
    `iface ${input.interfaceName} inet static`,
    `    address ${address.local}`,
    `    netmask ${ipv4Netmask(address.prefixLen)}`,
    `    pointopoint ${address.remote}`,
  ];
  if (includeLifecycle) {
    appendTunnelLifecycle(lines, input);
  }
  return lines;
}

function renderIfupdownIpv6Stanza(
  input: Parameters<typeof renderIfupdownSnippet>[0],
  address: EndpointAddressPair,
  includeLifecycle: boolean,
): string[] {
  const lines = [
    `auto ${input.interfaceName}`,
    `iface ${input.interfaceName} inet6 static`,
    `    address ${address.local}`,
    `    netmask ${address.prefixLen}`,
    `    pointopoint ${address.remote}`,
  ];
  if (includeLifecycle) {
    appendTunnelLifecycle(lines, input);
  }
  return lines;
}

function appendTunnelLifecycle(lines: string[], input: Parameters<typeof renderIfupdownSnippet>[0]) {
  if (input.kind === "fou") {
    lines.push(`    pre-up ip fou add port ${input.fou.port} ipproto ${input.fou.ipproto} || true`);
  }
  lines.push(
    `    pre-up ip tunnel add $IFACE mode ${linuxTunnelMode(input.kind)} remote ${input.remoteUnderlay} local ${input.localUnderlay} ttl 255${input.kind === "fou" ? ` encap fou encap-sport auto encap-dport ${input.fou.peer_port}` : ""}`,
  );
  lines.push("    up ip link set $IFACE up");
  lines.push("    post-down ip tunnel del $IFACE || true");
  if (input.kind === "fou") {
    lines.push(`    post-down ip fou del port ${input.fou.port} || true`);
  }
}

function renderRuntimeSnippet(
  input: {
    name: string;
    interfaceName: string;
    kind: TunnelKind;
    localUnderlay: string;
    remoteUnderlay: string;
    ipv4: EndpointAddressPair | null;
    ipv6: EndpointAddressPair | null;
    fou: RuntimeTunnelFouOptions;
  },
  manager: RuntimeTunnelManager,
): string {
  if (manager === "agent_iproute2_managed") {
    return renderIfupdownSnippet(input);
  }
  if (manager === "external_observed") {
    return [
      `# vpsman tunnel ${input.name}: external observed runtime tunnel`,
      `# interface ${input.interfaceName} is owned by an external program and is not created by vpsman`,
      "# vpsman will observe status, probe/speed evidence, and manage the Bird2 block",
    ].join("\n");
  }
  return [
    `# vpsman tunnel ${input.name}: external managed adapter runtime tunnel`,
    `# interface ${input.interfaceName} is created, restarted, shaped, or stopped by adapter commands`,
    "# vpsman will run bounded adapter argv, observe evidence, and manage the Bird2 block",
  ].join("\n");
}

function renderNetplanSnippet(plan: TunnelPlan, endpoint: TunnelEndpointConfig): string {
  return [
    `# vpsman tunnel ${plan.name}: generated endpoint ${endpoint.localClientId}`,
    "network:",
    "  version: 2",
    "  renderer: networkd",
    "  tunnels:",
    `    ${plan.interface_name}:`,
    `      mode: ${plan.kind}`,
    `      local: ${endpoint.localUnderlay}`,
    `      remote: ${endpoint.remoteUnderlay}`,
    "      ttl: 255",
    "      addresses:",
    ...endpointAddresses(endpoint).map((address) => `        - ${address.local}/${address.prefixLen}`),
    "",
  ].join("\n");
}

function renderSystemdNetdevSnippet(plan: TunnelPlan, endpoint: TunnelEndpointConfig): string {
  if (!isLinuxTunnelKind(plan.kind)) {
    throw new Error("systemd-networkd backend does not support this tunnel rendering");
  }
  const lines = [
    `# vpsman tunnel ${plan.name}: generated endpoint ${endpoint.localClientId}`,
    "[NetDev]",
    `Name=${plan.interface_name}`,
    `Kind=${plan.kind === "fou" ? "fou" : plan.kind}`,
    "",
    "[Tunnel]",
    `Local=${endpoint.localUnderlay}`,
    `Remote=${endpoint.remoteUnderlay}`,
    "TTL=255",
  ];
  if (plan.kind === "fou") {
    const fou = runtimeFouOptions(plan.kind, plan.runtime_control?.fou);
    lines.push(
      "",
      "[FooOverUDP]",
      "Encapsulation=FooOverUDP",
      `Port=${fou.port}`,
      `PeerPort=${fou.peer_port}`,
      `Protocol=${fou.ipproto}`,
    );
  }
  lines.push("");
  return lines.join("\n");
}

function renderSystemdNetworkSnippet(plan: TunnelPlan, endpoint: TunnelEndpointConfig): string {
  return [
    `# vpsman tunnel ${plan.name}: generated endpoint ${endpoint.localClientId}`,
    "[Match]",
    `Name=${plan.interface_name}`,
    "",
    "[Network]",
    ...endpointAddresses(endpoint).flatMap((address) => [
      `Address=${address.local}/${address.prefixLen}`,
      `Peer=${address.remote}`,
    ]),
    "",
  ].join("\n");
}

function endpointAddresses(endpoint: TunnelEndpointConfig): EndpointAddressPair[] {
  const addresses = [endpoint.ipv4Address, endpoint.ipv6Address].filter(Boolean) as EndpointAddressPair[];
  if (addresses.length > 0) {
    return addresses;
  }
  return [{ local: endpoint.localAddress, remote: endpoint.remoteAddress, prefixLen: endpoint.prefixLen }];
}

function renderBird2InterfaceSnippet(
  kind: TunnelKind,
  name: string,
  interfaceName: string,
  localClientId: string,
  peerClientId: string,
  ospfCost: number,
): string {
  return [
    `# vpsman ${bird2Label(kind)} tunnel ${name}: ${localClientId} -> ${peerClientId}`,
    `interface "${interfaceName}" {`,
    "  type ptp;",
    `  cost ${ospfCost};`,
    "};",
  ].join("\n");
}

function linuxTunnelMode(kind: TunnelKind): string {
  if (kind === "sit") {
    return "sit";
  }
  if (kind === "gre") {
    return "gre";
  }
  return "ipip";
}

function ipv4Netmask(prefixLen: number): string {
  const clamped = Math.max(0, Math.min(32, Math.trunc(prefixLen)));
  const mask = clamped === 0 ? 0 : (0xffffffff << (32 - clamped)) >>> 0;
  return [24, 16, 8, 0].map((shift) => (mask >>> shift) & 255).join(".");
}

function bird2Label(kind: TunnelKind): string {
  if (kind === "openvpn") {
    return "OpenVPN";
  }
  if (kind === "wireguard") {
    return "WireGuard";
  }
  if (kind === "tun_tap") {
    return "TUN/TAP";
  }
  if (kind === "custom") {
    return "custom";
  }
  return kind.toUpperCase();
}

function isLinuxTunnelKind(kind: TunnelKind): kind is "gre" | "ipip" | "sit" | "fou" {
  return kind === "gre" || kind === "ipip" || kind === "sit" || kind === "fou";
}

function runtimeFouOptions(kind: TunnelKind, options: RuntimeTunnelFouOptions | undefined): RuntimeTunnelFouOptions {
  if (kind !== "fou") {
    return DEFAULT_RUNTIME_FOU_OPTIONS;
  }
  return { ...DEFAULT_RUNTIME_FOU_OPTIONS, ...(options ?? {}) };
}
