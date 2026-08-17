import type {
  ActiveView,
  JsonValue,
  LifecycleOutcomeRecord,
  OperatorPreferences,
  WsEvent,
} from "./types";

export function parseWsEvent(value: unknown): WsEvent | null {
  if (typeof value !== "string") {
    return null;
  }
  try {
    const parsed = JSON.parse(value) as Partial<WsEvent>;
    if (typeof parsed.type !== "string") {
      return null;
    }
    return parsed as WsEvent;
  } catch {
    return null;
  }
}

export async function runPanelAction(
  setPending: (value: boolean) => void,
  setError: (value: string | null) => void,
  action: () => Promise<void>,
) {
  setPending(true);
  setError(null);
  try {
    await action();
  } catch (error) {
    setError(
      error instanceof Error
        ? error.message
        : "The panel action returned no diagnostic detail. No success is assumed; refresh current state and inspect the browser console or API logs before retrying.",
    );
  } finally {
    setPending(false);
  }
}

export async function retainMutationSuccessAfterRefresh(
  refresh: () => Promise<void>,
): Promise<void> {
  try {
    await refresh();
  } catch {
    // Refresh loaders expose their own error/evidence state. A completed
    // mutation must not be reported as failed or invite a duplicate retry.
  }
}

export function toggleValue(values: string[], value: string): string[] {
  return values.includes(value)
    ? values.filter((existing) => existing !== value)
    : [...values, value];
}

export function getPageTitle(view: ActiveView): string {
  switch (view) {
    case "Home":
      return "Home";
    case "Fleet":
      return "Fleet overview";
    case "Remote Operations":
      return "Remote";
    case "Config":
      return "Config";
    case "Jobs":
      return "Job history";
    case "Automation":
      return "Automation";
    case "Network":
      return "Network";
    case "Observability":
      return "Observability";
    case "Audit":
      return "Audit log";
    case "System":
      return "System";
    default:
      return `${view} management`;
  }
}

export function getPageDescription(view: ActiveView): string {
  switch (view) {
    case "Home":
      return "Clickable VPS health cards, fleet posture, resource and network metrics, current work, and quick operator actions";
    case "Jobs":
      return "Execution history, advanced dispatch, approvals, scheduled runs, and artifacts";
    case "Remote Operations":
      return "Browser terminal, file management, transfers, processes, and bulk file work without SSH or SCP";
    case "Automation":
      return "Schedules, runbooks, and agent update workflows";
    case "Network":
      return "Topology, tunnels, port forwarding, tests, routing, and evidence";
    case "Config":
      return "Server-desired runtime hierarchies, explicit VPS overrides, and configuration sources";
    case "Observability":
      return "Read-only fleet, network, process, alert, webhook, and dashboard analysis";
    case "Audit":
      return "Searchable audit, job, terminal, and bearer-session evidence with reviewed session revocation";
    case "Backups":
      return "Backup, restore, and migration workflows";
    case "Access":
      return "Operator authority, VPS identities, gateway sessions, and privilege vault state";
    case "System":
      return "Control-plane health, capacity, suite configuration, maintenance, and preferences";
    default:
      return "";
  }
}

export function shortId(value: string | null | undefined): string {
  return value ? value.slice(0, 8) : "-";
}

export function dispatchFailureReason(
  error: string | null | undefined,
  status: string,
  operation: string,
): string {
  const detail = error?.trim();
  if (detail) {
    return detail;
  }
  const readableStatus = status.trim().replace(/_/g, " ") || "not queued";
  return `${operation} was ${readableStatus}, but the server returned no target-specific reason. Refresh current state, inspect API logs, and retry.`;
}

export function lifecycleOutcomeFailureReason(
  outcome: LifecycleOutcomeRecord,
  committedAction: string,
): string {
  const detail = outcome.error?.trim();
  if (detail) {
    return detail;
  }
  if (outcome.operation === "gateway_session_disconnect") {
    return `${committedAction} is saved, but the gateway session disconnect did not complete. Retry from Access > Gateway sessions and inspect API/gateway logs.`;
  }
  if (outcome.operation === "job_terminal_reconciliation") {
    return `${committedAction} is saved, but related job terminal events were not fully reconciled. Durable results remain intact; refresh Jobs and inspect API logs.`;
  }
  const operation =
    outcome.operation.trim().replace(/_/g, " ") || "Post-commit operation";
  return `${committedAction} is saved, but ${operation} did not complete. Refresh the affected state and inspect server logs.`;
}

export type VpsNameDisplayMode = "name" | "name_id_suffix";

export const DEFAULT_VPS_NAME_DISPLAY_MODE: VpsNameDisplayMode =
  "name_id_suffix";

export const DEFAULT_OPERATOR_PREFERENCES: OperatorPreferences = {
  agent_install_mode: "root",
  bulk_output_compare_mode: "binary",
  dashboard_curve_exclusions: [],
  dashboard_network_top_limit: 8,
  dashboard_resource_top_limit: 8,
  byte_unit_display_mode: "decimal",
  fleet_location_display_mode: "country_only",
  fleet_tag_visibility_overrides: {},
  gateway_endpoints: "",
  gateway_server_public_key_hex: null,
  language: "en",
  review_prompt_mode: "inline",
  show_country_flags: true,
  sidebar_subpanel_default: "active",
  timezone: null,
  vps_name_display_mode: DEFAULT_VPS_NAME_DISPLAY_MODE,
};

export function sanitizeOperatorPreferences(
  preferences: Partial<OperatorPreferences> | null | undefined,
): OperatorPreferences {
  const source = preferences ?? {};
  return {
    agent_install_mode:
      source.agent_install_mode === "root" ||
      source.agent_install_mode === "user" ||
      source.agent_install_mode === "staged"
        ? source.agent_install_mode
        : DEFAULT_OPERATOR_PREFERENCES.agent_install_mode,
    bulk_output_compare_mode:
      source.bulk_output_compare_mode ??
      DEFAULT_OPERATOR_PREFERENCES.bulk_output_compare_mode,
    dashboard_curve_exclusions: Array.isArray(source.dashboard_curve_exclusions)
      ? source.dashboard_curve_exclusions
      : DEFAULT_OPERATOR_PREFERENCES.dashboard_curve_exclusions,
    dashboard_network_top_limit:
      source.dashboard_network_top_limit ??
      DEFAULT_OPERATOR_PREFERENCES.dashboard_network_top_limit,
    dashboard_resource_top_limit:
      source.dashboard_resource_top_limit ??
      DEFAULT_OPERATOR_PREFERENCES.dashboard_resource_top_limit,
    byte_unit_display_mode:
      source.byte_unit_display_mode === "binary"
        ? "binary"
        : DEFAULT_OPERATOR_PREFERENCES.byte_unit_display_mode,
    fleet_location_display_mode:
      source.fleet_location_display_mode === "country_region"
        ? "country_region"
        : DEFAULT_OPERATOR_PREFERENCES.fleet_location_display_mode,
    fleet_tag_visibility_overrides:
      source.fleet_tag_visibility_overrides &&
      typeof source.fleet_tag_visibility_overrides === "object" &&
      !Array.isArray(source.fleet_tag_visibility_overrides)
        ? source.fleet_tag_visibility_overrides
        : DEFAULT_OPERATOR_PREFERENCES.fleet_tag_visibility_overrides,
    gateway_endpoints:
      typeof source.gateway_endpoints === "string"
        ? source.gateway_endpoints
        : DEFAULT_OPERATOR_PREFERENCES.gateway_endpoints,
    gateway_server_public_key_hex:
      typeof source.gateway_server_public_key_hex === "string" ||
      source.gateway_server_public_key_hex === null
        ? source.gateway_server_public_key_hex
        : DEFAULT_OPERATOR_PREFERENCES.gateway_server_public_key_hex,
    language: source.language ?? DEFAULT_OPERATOR_PREFERENCES.language,
    review_prompt_mode:
      source.review_prompt_mode ??
      DEFAULT_OPERATOR_PREFERENCES.review_prompt_mode,
    show_country_flags:
      source.show_country_flags ??
      DEFAULT_OPERATOR_PREFERENCES.show_country_flags,
    sidebar_subpanel_default:
      source.sidebar_subpanel_default ??
      DEFAULT_OPERATOR_PREFERENCES.sidebar_subpanel_default,
    timezone:
      typeof source.timezone === "string" || source.timezone === null
        ? source.timezone
        : DEFAULT_OPERATOR_PREFERENCES.timezone,
    vps_name_display_mode:
      source.vps_name_display_mode ??
      DEFAULT_OPERATOR_PREFERENCES.vps_name_display_mode,
  };
}

export function displayNameOrUnnamed(
  displayName: string | null | undefined,
): string {
  return displayName?.trim() || "Unnamed VPS";
}

export function formatBillingRenewal(
  value: string | null | undefined,
  _periodCode?: string | null,
): string | null {
  const cycle = value?.trim();
  if (!cycle) return null;
  const day = /^(\d{1,2})$/.exec(cycle);
  if (day) return `Renews day ${Number(day[1])}`;
  const anchored = /^(\d{1,2})-(\d{1,2})$/.exec(cycle);
  if (anchored) {
    return `Renews ${anchored[1].padStart(2, "0")}-${anchored[2].padStart(2, "0")}`;
  }
  return `Renewal anchor ${cycle}`;
}

export type TrafficQuotaState = "finite" | "unlimited" | "unset";

export function trafficNonTotalSelectorDirection(traffic: {
  selector_breakdown?: Array<{ direction?: string | null }> | null;
}): "RX" | "TX" | "Max" | null {
  const directions = traffic.selector_breakdown?.map(
    ({ direction }) => direction,
  );
  if (!directions?.length || directions.some((direction) => !direction)) {
    return null;
  }
  const direction = directions[0];
  if (directions.some((candidate) => candidate !== direction)) return null;
  if (direction === "rx") return "RX";
  if (direction === "tx") return "TX";
  if (direction === "tx/rx") return "Max";
  return null;
}

export type TrafficLimitingQuota = {
  direction: "RX" | "TX" | "Total";
  percent: number;
  quota: number;
  used: number;
};

export type TrafficUnlimitedQuota = {
  direction: "RX" | "TX" | "Total";
  used: number;
};

export function trafficLimitingQuota(traffic: {
  quota_rx_bytes?: number | null;
  quota_total_bytes?: number | null;
  quota_tx_bytes?: number | null;
  rx_bytes?: number | null;
  total_bytes?: number | null;
  tx_bytes?: number | null;
}): TrafficLimitingQuota | null {
  const candidates = [
    {
      direction: "Total" as const,
      quota: traffic.quota_total_bytes,
      used: traffic.total_bytes,
    },
    {
      direction: "RX" as const,
      quota: traffic.quota_rx_bytes,
      used: traffic.rx_bytes,
    },
    {
      direction: "TX" as const,
      quota: traffic.quota_tx_bytes,
      used: traffic.tx_bytes,
    },
  ].flatMap(({ direction, quota, used }) =>
    typeof quota === "number" &&
    Number.isFinite(quota) &&
    quota > 0 &&
    typeof used === "number" &&
    Number.isFinite(used) &&
    used >= 0
      ? [{ direction, percent: (used / quota) * 100, quota, used }]
      : [],
  );
  return (
    candidates.reduce<TrafficLimitingQuota | null>(
      (limiting, candidate) =>
        limiting === null || candidate.percent > limiting.percent
          ? candidate
          : limiting,
      null,
    ) ?? null
  );
}

export function trafficQuotaState(traffic: {
  quota_rx_bytes?: number | null;
  quota_total_bytes?: number | null;
  quota_tx_bytes?: number | null;
}): TrafficQuotaState {
  const quotas = [
    traffic.quota_rx_bytes,
    traffic.quota_tx_bytes,
    traffic.quota_total_bytes,
  ];
  if (quotas.some((quota) => typeof quota === "number" && quota > 0)) {
    return "finite";
  }
  return quotas.some((quota) => quota === -1) ? "unlimited" : "unset";
}

export function trafficUnlimitedQuota(traffic: {
  quota_rx_bytes?: number | null;
  quota_total_bytes?: number | null;
  quota_tx_bytes?: number | null;
  rx_bytes?: number | null;
  total_bytes?: number | null;
  tx_bytes?: number | null;
}): TrafficUnlimitedQuota | null {
  const candidates = [
    {
      direction: "Total" as const,
      quota: traffic.quota_total_bytes,
      used: traffic.total_bytes,
    },
    {
      direction: "RX" as const,
      quota: traffic.quota_rx_bytes,
      used: traffic.rx_bytes,
    },
    {
      direction: "TX" as const,
      quota: traffic.quota_tx_bytes,
      used: traffic.tx_bytes,
    },
  ];
  for (const candidate of candidates) {
    if (
      candidate.quota === -1 &&
      typeof candidate.used === "number" &&
      Number.isFinite(candidate.used) &&
      candidate.used >= 0
    ) {
      return { direction: candidate.direction, used: candidate.used };
    }
  }
  return null;
}

export function formatVirtualizationLabel(value: string): string {
  const normalized = value.trim().toLocaleLowerCase();
  const labels: Record<string, string> = {
    "cloud-hypervisor": "Cloud Hypervisor",
    bhyve: "bhyve",
    bochs: "Bochs",
    docker: "Docker",
    firecracker: "Firecracker",
    "hyper-v": "Hyper-V",
    kvm: "KVM",
    lxc: "LXC",
    openvz: "OpenVZ",
    parallels: "Parallels",
    podman: "Podman",
    qemu: "QEMU",
    virtualbox: "VirtualBox",
    vmware: "VMware",
    wsl: "WSL",
    xen: "Xen",
  };
  return labels[normalized] ?? value.trim();
}

export function clientIdSuffix(
  clientId: string | null | undefined,
): string | null {
  const trimmed = clientId?.trim();
  if (!trimmed) {
    return null;
  }
  if (/^v-[1-9][0-9]*$/.test(trimmed)) {
    return trimmed;
  }
  const normalized = trimmed.replace(/[^A-Za-z0-9]/g, "");
  const source = normalized || trimmed;
  return source.slice(-4) || null;
}

export function formatVpsName(
  identity: {
    id?: string | null;
    client_id?: string | null;
    display_name?: string | null;
  },
  mode: VpsNameDisplayMode = DEFAULT_VPS_NAME_DISPLAY_MODE,
): string {
  const name = displayNameOrUnnamed(identity.display_name);
  const suffix =
    mode === "name_id_suffix"
      ? clientIdSuffix(identity.id ?? identity.client_id)
      : null;
  return suffix ? `${name} (${suffix})` : name;
}

export function clientDisplayNameMap(
  clients: Array<{ id: string; display_name?: string | null }>,
  mode: VpsNameDisplayMode = DEFAULT_VPS_NAME_DISPLAY_MODE,
): Map<string, string> {
  return new Map(
    clients.map((client) => [client.id, formatVpsName(client, mode)]),
  );
}

export function clientLifecycleNameMap(
  clients: Array<{ client_id: string; display_name?: string | null }>,
  mode: VpsNameDisplayMode = DEFAULT_VPS_NAME_DISPLAY_MODE,
): Map<string, string> {
  return new Map(
    clients.map((client) => [client.client_id, formatVpsName(client, mode)]),
  );
}

export function clientDisplayNameFromMap(
  clientId: string | null | undefined,
  namesById: Map<string, string>,
): string {
  if (!clientId) {
    return "Unknown VPS";
  }
  return namesById.get(clientId) ?? "Unknown VPS";
}

export function shortHash(value: string): string {
  return value.length > 16 ? `${value.slice(0, 14)}...` : value;
}

let preferredTimeZone: string | null = null;

export function setPreferredTimeZone(timeZone: string | null): void {
  const normalized = timeZone?.trim() || null;
  preferredTimeZone =
    normalized && isBrowserTimeZoneSupported(normalized) ? normalized : null;
}

export function formatTime(
  value: string,
  timeZone = preferredTimeZone,
): string {
  const date = new Date(timestampMillis(value));
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return safeLocaleString(date, timeZone ? { timeZone } : undefined);
}

export function formatCompactTime(
  value: string,
  timeZone = preferredTimeZone,
): string {
  const date = new Date(timestampMillis(value));
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  const relative = formatRelativeTime(date, new Date());
  if (relative) {
    return relative;
  }
  return safeLocaleString(date, {
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    month: "numeric",
    ...(timeZone ? { timeZone } : {}),
  });
}

export function formatFullTime(
  value: string,
  timeZone = preferredTimeZone,
): string {
  const date = new Date(timestampMillis(value));
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return safeLocaleString(date, {
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    month: "numeric",
    second: "2-digit",
    timeZoneName: "short",
    year: "numeric",
    ...(timeZone ? { timeZone } : {}),
  });
}

export function formatUptimeStartTime(
  observedAt: string | null | undefined,
  uptimeSecs: number | null | undefined,
  timeZone = preferredTimeZone,
): string | null {
  if (
    !observedAt ||
    uptimeSecs === null ||
    uptimeSecs === undefined ||
    !Number.isFinite(uptimeSecs) ||
    uptimeSecs < 0
  ) {
    return null;
  }
  const startedAtMs =
    timestampMillis(observedAt) - Math.floor(uptimeSecs) * 1_000;
  if (
    !Number.isFinite(startedAtMs) ||
    Number.isNaN(new Date(startedAtMs).getTime())
  ) {
    return null;
  }
  return formatFullTime(String(startedAtMs), timeZone);
}

export function timestampMillis(value: string): number {
  const trimmed = value.trim();
  if (/^-?\d+(?:\.\d+)?$/.test(trimmed)) {
    const numeric = Number(trimmed);
    if (Number.isFinite(numeric)) {
      return Math.abs(numeric) < 100_000_000_000 ? numeric * 1000 : numeric;
    }
  }
  return new Date(value).getTime();
}

function formatRelativeTime(date: Date, now: Date): string | null {
  const deltaMs = date.getTime() - now.getTime();
  const absSeconds = Math.round(Math.abs(deltaMs) / 1000);
  if (absSeconds < 45) {
    return "just now";
  }

  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["year", 365 * 24 * 60 * 60],
    ["month", 30 * 24 * 60 * 60],
    ["week", 7 * 24 * 60 * 60],
    ["day", 24 * 60 * 60],
    ["hour", 60 * 60],
    ["minute", 60],
  ];
  const [unit, secondsPerUnit] = units.find(
    ([, secondsPerUnit]) => absSeconds >= secondsPerUnit,
  ) ?? ["minute", 60];
  const count = Math.max(1, Math.round(absSeconds / secondsPerUnit));
  const signedCount = deltaMs < 0 ? -count : count;
  try {
    return new Intl.RelativeTimeFormat(undefined, {
      numeric: "always",
      style: "narrow",
    }).format(signedCount, unit);
  } catch {
    const suffix = signedCount < 0 ? "ago" : "from now";
    return `${count}${relativeUnitSuffix(unit)} ${suffix}`;
  }
}

function relativeUnitSuffix(unit: Intl.RelativeTimeFormatUnit): string {
  switch (unit) {
    case "year":
      return "y";
    case "month":
      return "mo";
    case "week":
      return "w";
    case "day":
      return "d";
    case "hour":
      return "h";
    default:
      return "m";
  }
}

function isBrowserTimeZoneSupported(timeZone: string): boolean {
  try {
    new Intl.DateTimeFormat(undefined, { timeZone }).format(new Date());
    return true;
  } catch {
    return false;
  }
}

function safeLocaleString(
  date: Date,
  options?: Intl.DateTimeFormatOptions,
): string {
  try {
    return date.toLocaleString(undefined, options);
  } catch {
    if (options?.timeZone) {
      const { timeZone: _ignored, ...fallbackOptions } = options;
      try {
        return date.toLocaleString(
          undefined,
          Object.keys(fallbackOptions).length > 0 ? fallbackOptions : undefined,
        );
      } catch {
        return date.toISOString();
      }
    }
    return date.toISOString();
  }
}

export function decodeOutputPreview(value: string): string {
  if (!value) {
    return "";
  }
  try {
    const binary = globalThis.atob(value);
    const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
  } catch {
    return "[binary output]";
  }
}

export function statusClass(status: string): string {
  const lower = status.toLowerCase();
  if (
    lower.includes("rejected") ||
    lower.includes("failed") ||
    lower.includes("error") ||
    lower.includes("degraded") ||
    lower.includes("drift") ||
    lower.includes("timeout") ||
    lower.includes("offline") ||
    lower.includes("revoked") ||
    lower.includes("deleted") ||
    lower.includes("unsupported") ||
    lower.includes("ineffective") ||
    lower.includes("missing") ||
    lower.includes("no_store") ||
    lower.includes("no_artifacts") ||
    lower.includes("no_samples")
  ) {
    return "warn";
  }
  if (
    lower === "ok" ||
    lower.startsWith("selected") ||
    lower === "active" ||
    lower.includes("identity active") ||
    lower.includes("running") ||
    lower.includes("complete") ||
    lower.includes("accepted") ||
    lower.includes("healthy") ||
    lower.includes("applied")
  ) {
    return "ok";
  }
  return "neutral";
}

export function isJsonObject(
  value: JsonValue,
): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
