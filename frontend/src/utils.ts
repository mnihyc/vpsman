import type {
  ActiveView,
  JsonValue,
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
    setError(error instanceof Error ? error.message : "Panel action failed");
  } finally {
    setPending(false);
  }
}

export function toggleValue(values: string[], value: string): string[] {
  return values.includes(value)
    ? values.filter((existing) => existing !== value)
    : [...values, value];
}

export function getHeroTitle(view: ActiveView): string {
  switch (view) {
    case "Dashboard":
      return "Dashboard";
    case "Fleet":
      return "Fleet overview";
    case "Config":
      return "Config";
    case "Jobs":
      return "Job history";
    case "Schedules":
      return "Schedules";
    case "Audit":
      return "Audit log";
    case "System":
      return "System";
    default:
      return `${view} management`;
  }
}

export function getHeroCopy(view: ActiveView): string {
  switch (view) {
    case "Dashboard":
      return "Operational health, resource posture, network activity, and top label clusters";
    case "Jobs":
      return "Recent command requests and authorization outcomes";
    case "Schedules":
      return "Server-side schedule registry and due-run records";
    case "Config":
      return "Rule-card patches, data-source presets, and guarded full-config edits";
    case "Audit":
      return "Operator and security events from the control plane";
    case "Tags":
      return "Tag inventory and bulk target operations";
    case "Topology":
      return "VPS network topology and extended operations";
    case "Backups":
      return "Backup, restore, and migration workflows";
    case "Access":
      return "Operator sessions, roles, and privileged unlock state";
    case "System":
      return "Control-plane dashboard, suite configuration, and operator preferences";
    default:
      return "";
  }
}

export function shortId(value: string | null | undefined): string {
  return value ? value.slice(0, 8) : "-";
}

export type VpsNameDisplayMode = "name" | "name_id_suffix";

export const DEFAULT_VPS_NAME_DISPLAY_MODE: VpsNameDisplayMode =
  "name_id_suffix";

export const DEFAULT_OPERATOR_PREFERENCES: OperatorPreferences = {
  bulk_output_compare_mode: "binary",
  dashboard_curve_exclusions: [],
  dashboard_network_top_limit: 8,
  dashboard_resource_top_limit: 8,
  gateway_endpoints: "",
  gateway_server_public_key_hex: null,
  fleet_tag_visibility_overrides: {},
  language: "en",
  show_country_flags: true,
  sidebar_subpanel_default: "active",
  timezone: null,
  vps_name_display_mode: DEFAULT_VPS_NAME_DISPLAY_MODE,
};

export function displayNameOrUnnamed(
  displayName: string | null | undefined,
): string {
  return displayName?.trim() || "Unnamed VPS";
}

export function clientIdSuffix(
  clientId: string | null | undefined,
): string | null {
  const trimmed = clientId?.trim();
  if (!trimmed) {
    return null;
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
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return safeLocaleString(date, timeZone ? { timeZone } : undefined);
}

export function formatCompactTime(
  value: string,
  timeZone = preferredTimeZone,
): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return safeLocaleString(date, {
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    month: "numeric",
    ...(timeZone ? { timeZone } : {}),
  });
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
    const binary = window.atob(value);
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

export function metadataOperator(metadata: JsonValue): string | null {
  if (!isJsonObject(metadata)) {
    return null;
  }
  const username = metadata.operator_username;
  return typeof username === "string" ? username : null;
}

export function metadataPreview(metadata: JsonValue): string {
  if (isJsonObject(metadata)) {
    const session = metadata.session_id;
    if (typeof session === "string") {
      return `session ${shortId(session)}`;
    }
  }
  const rendered = JSON.stringify(metadata);
  return rendered.length > 96 ? `${rendered.slice(0, 93)}...` : rendered;
}

export function isJsonObject(
  value: JsonValue,
): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
