import type { ActiveView, JsonValue, WsEvent } from "./types";

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
  return values.includes(value) ? values.filter((existing) => existing !== value) : [...values, value];
}

export function getHeroTitle(view: ActiveView): string {
  switch (view) {
    case "Fleet":
      return "Fleet overview";
    case "Jobs":
      return "Job history";
    case "Schedules":
      return "Schedules";
    case "Audit":
      return "Audit log";
    default:
      return `${view} management`;
  }
}

export function getHeroCopy(view: ActiveView): string {
  switch (view) {
    case "Jobs":
      return "Recent command requests and authorization outcomes";
    case "Schedules":
      return "Server-side schedule registry and due-run records";
    case "Audit":
      return "Operator and security events from the control plane";
    case "Pools":
      return "Resource-pool hierarchy and tag bulk operations";
    case "Topology":
      return "BGP, tunnel, and OSPF topology operations";
    case "Backups":
      return "Backup, restore, and migration workflows";
    case "Access":
      return "Operator sessions, roles, and privileged unlock state";
    default:
      return "";
  }
}

export function shortId(value: string | null | undefined): string {
  return value ? value.slice(0, 8) : "-";
}

export function shortHash(value: string): string {
  return value.length > 16 ? `${value.slice(0, 14)}...` : value;
}

export function formatTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return date.toLocaleString();
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

export function isJsonObject(value: JsonValue): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
