import { formatPortRange, parsePortExpression } from "./portForwarding";
import type {
  PortForwardMode, PortForwardPool,
  PortForwardPoolStrategy, PortForwardProtocol,
  PortForwardUpstream,
} from "./types";

export const POOL_STRATEGIES: Record<PortForwardPoolStrategy, { label: string; hint: string }> = {
  round_robin: { label: "Round-robin", hint: "Distribute new flows in rotation, taking each endpoint's weight into account." },
  random: { label: "Random", hint: "Choose an endpoint randomly for each new flow, in proportion to its weight." },
  source_ip_hash: { label: "Source IP hash", hint: "Keep the same source IP on the same endpoint while membership is unchanged. Clients behind one NAT share this affinity." },
  least_connections: { label: "Least connections", hint: "Use the adapter's active connection or session counts, adjusted for each endpoint's weight." },
  consistent_source_ip_hash: { label: "Consistent source IP hash", hint: "Keep source-IP affinity and reduce reassignment when pool membership changes." },
};

export type UpstreamDraft = {
  id: string;
  address: string;
  hostname: string | null;
  ports: string;
  weight: string;
  role: "primary" | "backup";
  enabled: boolean;
  failureMode: "default" | "off" | "temporary";
  threshold: string;
  window: string;
  retryAfter: string;
};

export type PoolDraft = {
  strategy: PortForwardPoolStrategy;
  upstreams: UpstreamDraft[];
  retryMode: "default" | "off" | "connect_failure";
  connectTimeout: string;
  maxAttempts: string;
  retryBudget: string;
};

export function newUpstream(values: Partial<UpstreamDraft> = {}): UpstreamDraft {
  return {
    id: crypto.randomUUID(), address: "", hostname: null, ports: "", weight: "1",
    role: "primary", enabled: true, failureMode: "default",
    threshold: "1", window: "10", retryAfter: "10", ...values,
  };
}

export function newPool(): PoolDraft {
  return { strategy: "round_robin", upstreams: [newUpstream()], retryMode: "default", connectTimeout: "", maxAttempts: "", retryBudget: "" };
}

export function poolDraftFromSaved(pool: PortForwardPool): PoolDraft {
  return {
    strategy: pool.strategy,
    upstreams: pool.upstreams.map((row) => newUpstream({
      id: row.id, address: row.target_ip, hostname: row.target_hostname ?? null,
      ports: formatPortRange(row.ports), weight: String(row.weight), role: row.role,
      enabled: row.enabled, failureMode: row.failure_policy?.mode ?? "default",
      threshold: String(row.failure_policy?.threshold ?? 1),
      window: String(row.failure_policy?.window_secs ?? 10),
      retryAfter: String(row.failure_policy?.retry_after_secs ?? 10),
    })),
    retryMode: pool.retry_policy?.mode ?? "default",
    connectTimeout: String(pool.connect_timeout_secs ?? ""),
    maxAttempts: String(pool.retry_policy?.max_attempts ?? ""),
    retryBudget: String(pool.retry_policy?.retry_budget_secs ?? ""),
  };
}

// Form options follow the forwarding policy, never cached host capability or
// operator declarations about an executable's implementation.
export type PortForwardPoolOptions = {
  mode: PortForwardMode;
  strategies: PortForwardPoolStrategy[];
  address_families: Array<"ipv4" | "ipv6">;
  mixed_families: boolean;
  backup_strategies: PortForwardPoolStrategy[];
  failure_exclusion?: { linked_timeout: boolean; min_endpoints: number; description: string };
  connect_timeout?: { description: string };
  retries?: { description: string };
};

export function poolOptions(mode: PortForwardMode, protocol: PortForwardProtocol): PortForwardPoolOptions | null {
  if (mode === "redirect") return null;
  const custom = mode === "custom_adapter";
  const tcp = custom && protocol === "tcp";
  return {
    mode,
    strategies: custom ? ["round_robin", "least_connections", "source_ip_hash", "consistent_source_ip_hash", "random"] : ["round_robin", "random", "source_ip_hash"],
    address_families: ["ipv4", "ipv6"],
    mixed_families: custom,
    backup_strategies: custom ? ["round_robin", "least_connections"] : [],
    failure_exclusion: tcp ? { linked_timeout: true, min_endpoints: 2, description: "Temporary TCP connection-failure exclusion per IP:port in this rule." } : undefined,
    connect_timeout: tcp ? { description: "Deadline per connection attempt in this rule." } : undefined,
    retries: tcp ? { description: "Retry failed connection establishment within this rule." } : undefined,
  };
}

export function allowsBackup(draft: PoolDraft, options: PortForwardPoolOptions) {
  return options.backup_strategies?.includes(draft.strategy) ?? false;
}

export function poolFailureOptions(draft: PoolDraft, options: PortForwardPoolOptions) {
  const failure = options.failure_exclusion;
  if (!failure) return undefined;
  const endpoints = draft.upstreams.reduce((count, row) => {
    if (!literalIpFamily(row.address)) return count;
    try {
      return count + parsePortExpression(row.ports).reduce((total, range) => total + range.end - range.start + 1, 0);
    } catch { return count; }
  }, 0);
  return endpoints >= (failure.min_endpoints ?? 1) ? failure : undefined;
}

export function literalIpFamily(value: string): "ipv4" | "ipv6" | null {
  const input = value.trim();
  const ipv4 = input.split(".");
  if (ipv4.length === 4 && ipv4.every((part) => /^\d{1,3}$/.test(part) && Number(part) <= 255)) return "ipv4";
  if (input.includes(":") && /^[0-9a-f:.]+$/i.test(input)) {
    try { new URL(`http://[${input}]/`); return "ipv6"; } catch { return null; }
  }
  return null;
}

export function normalizedIp(value: string): string {
  const family = literalIpFamily(value);
  if (family === "ipv4") return value.trim().split(".").map(Number).join(".");
  if (family === "ipv6") return new URL(`http://[${value.trim()}]/`).hostname.slice(1, -1);
  return value.trim();
}

function positiveInteger(value: string, label: string): number {
  if (!/^\d+$/.test(value) || !Number.isSafeInteger(Number(value)) || Number(value) < 1 || Number(value) > 0xffffffff) {
    throw new Error(`${label} must be a positive whole number (at most 4294967295)`);
  }
  return Number(value);
}

export function upstreamError(row: UpstreamDraft, options: PortForwardPoolOptions): string | null {
  try {
    const family = literalIpFamily(row.address);
    if (!family) throw new Error(row.address.trim() ? "Resolve the hostname and select addresses" : "Enter an IP or hostname");
    if (!options.address_families.includes(family)) throw new Error("Address family is not supported by this forwarder");
    const ports = parsePortExpression(row.ports, "Target port");
    if (ports.length !== 1) throw new Error("Use one port or contiguous range per row");
    positiveInteger(row.weight, "Weight");
    if (options.failure_exclusion && row.failureMode === "temporary") {
      positiveInteger(row.threshold, "Failure threshold");
      positiveInteger(row.window, "Failure window");
      if (!options.failure_exclusion.linked_timeout) positiveInteger(row.retryAfter, "Retry after");
    }
    return null;
  } catch (error) { return (error as Error).message; }
}

export function buildPool(incoming: string, draft: PoolDraft, options: PortForwardPoolOptions): PortForwardPool {
  const ranges = parsePortExpression(incoming, "Incoming port");
  if (!options.strategies.includes(draft.strategy)) throw new Error("Choose a strategy supported by this forwarder");
  if (!draft.upstreams.length) throw new Error("Add at least one upstream");
  if (draft.upstreams.length > 256) throw new Error("Use no more than 256 upstream rows");
  const backup = allowsBackup(draft, options);
  const failure = poolFailureOptions(draft, options);
  if (!backup && draft.upstreams.some((row) => row.role === "backup")) {
    throw new Error("This strategy cannot use backups. Change backup rows to primary with the previous forwarder or strategy, or remove those rows.");
  }
  const upstreams: PortForwardUpstream[] = draft.upstreams.map((row, index) => {
    const error = upstreamError(row, { ...options, failure_exclusion: failure });
    if (error) throw new Error(`Upstream ${index + 1}: ${error}`);
    return {
      id: row.id, target_ip: normalizedIp(row.address), target_hostname: row.hostname,
      ports: parsePortExpression(row.ports)[0]!, weight: Number(row.weight),
      enabled: row.enabled, role: row.role,
      ...(failure && row.failureMode !== "default" ? {
        failure_policy: row.failureMode === "off" ? { mode: "off" as const } : {
          mode: "temporary" as const, threshold: Number(row.threshold), window_secs: Number(row.window),
          retry_after_secs: Number(failure.linked_timeout ? row.window : row.retryAfter),
        },
      } : {}),
    };
  });
  if (!upstreams.some((row) => row.enabled)) throw new Error("Enable at least one upstream, or remove this pool");
  if (!upstreams.some((row) => row.role === "primary")) throw new Error("Keep at least one primary upstream");
  const families = new Set(upstreams.map((row) => literalIpFamily(row.target_ip)));
  if (!options.mixed_families && families.size > 1) throw new Error("Use one address family in this pool; create a separate rule for the other family");
  for (let index = 0; index < upstreams.length; index++) {
    const current = upstreams[index]!;
    const duplicate = upstreams.slice(0, index).findIndex((other) => other.target_ip === current.target_ip
      && other.ports.start <= current.ports.end && current.ports.start <= other.ports.end);
    if (duplicate >= 0) throw new Error(`Upstreams ${duplicate + 1} and ${index + 1} overlap at ${current.target_ip}; adjust ports or weight instead`);
  }
  const totalWeight = upstreams.filter((row) => row.enabled).reduce((sum, row) => sum + (row.ports.end - row.ports.start + 1) * row.weight, 0);
  if (options.mode === "dnat" && totalWeight > 0xffffffff) throw new Error("Expanded endpoint weights exceed 4294967295; reduce weights proportionally");
  const pool: PortForwardPool = { incoming: ranges, strategy: draft.strategy, upstreams };
  if (options.connect_timeout && draft.connectTimeout) {
    pool.connect_timeout_secs = positiveInteger(draft.connectTimeout, "Connect timeout");
  }
  if (options.retries && draft.retryMode !== "default") {
    pool.retry_policy = { mode: draft.retryMode };
    if (draft.retryMode === "connect_failure") {
      for (const [key, value, label] of [
        ["max_attempts", draft.maxAttempts, "Maximum attempts"],
        ["retry_budget_secs", draft.retryBudget, "Retry budget"],
      ] as const) {
        if (value) pool.retry_policy[key] = positiveInteger(value, label);
      }
    }
  }
  return pool;
}

export function poolEndpointCount(pool: PortForwardPool) {
  return pool.upstreams.filter((row) => row.enabled).reduce((sum, row) => sum + row.ports.end - row.ports.start + 1, 0);
}

export function failureLabel(policy: PortForwardUpstream["failure_policy"]): string {
  if (!policy) return "Default";
  if (policy.mode === "off") return "Automatic exclusion off";
  return `${policy.threshold} failures / ${policy.window_secs}s → exclude ${policy.retry_after_secs}s, then retry`;
}

export function upstreamSummary(row: PortForwardUpstream) {
  return `${row.role === "backup" ? "Backup" : "Primary"}${row.enabled ? "" : " · disabled"} · weight ${row.weight} per port${row.failure_policy ? ` · ${failureLabel(row.failure_policy)}` : ""}${row.target_hostname ? ` · ${row.target_hostname}` : ""}`;
}

export function poolSummary(pool: PortForwardPool) {
  return `${pool.incoming.map(formatPortRange).join(",")} → ${poolEndpointCount(pool)} endpoints · ${POOL_STRATEGIES[pool.strategy].label}`;
}

export function poolTooltip(pool: PortForwardPool) {
  return [poolSummary(pool), `Every incoming port uses this pool.${pool.upstreams.some((row) => row.role === "backup") ? " Backups are selected only when no primary is eligible." : ""}`, ...pool.upstreams.map((row) => `${upstreamAddress(row)} · ${upstreamSummary(row)}`)].join("\n");
}

export function upstreamAddress(row: PortForwardUpstream) {
  return `${row.target_ip.includes(":") ? `[${row.target_ip}]` : row.target_ip}:${formatPortRange(row.ports)}`;
}

export function retrySummary(pool: PortForwardPool) {
  const retry = pool.retry_policy;
  if (!retry) return "Default";
  if (retry.mode === "off") return "Off";
  return `On connect failure · attempts ${retry.max_attempts ?? "default"} · budget ${retry.retry_budget_secs ? `${retry.retry_budget_secs}s` : "default"}`;
}
