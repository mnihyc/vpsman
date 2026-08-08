import type { MonitoringWindow } from "./components/MonitoringRangeTabs";

export type NetworkEvidenceSource = "automatic" | "manual" | "";
export type NetworkEvidenceKind =
  | "tunnel_reachability"
  | "network_speed_test"
  | "network_status"
  | "";
export type NetworkEvidenceHealth = "healthy" | "unhealthy" | "unknown" | "";

export type NetworkEvidenceQuery = {
  clientId?: string;
  endAt?: string;
  health?: NetworkEvidenceHealth;
  kind?: NetworkEvidenceKind;
  limit?: number;
  planIds?: string[];
  query?: string;
  source?: NetworkEvidenceSource;
  startAt?: string;
  window?: MonitoringWindow;
};

export const DEFAULT_NETWORK_EVIDENCE_WINDOW: MonitoringWindow = "1d";
export const NETWORK_EVIDENCE_OBSERVATION_LIMIT = 250_000;

export function buildNetworkEvidenceSearch(
  query: NetworkEvidenceQuery = {},
): string {
  const params = new URLSearchParams();
  const window = query.window ?? DEFAULT_NETWORK_EVIDENCE_WINDOW;
  params.set("window", window);
  if (window === "custom") {
    const startUnix = dateTimeInputUnix(query.startAt);
    const endUnix = dateTimeInputUnix(query.endAt);
    if (startUnix === null) {
      throw new Error("Select a valid custom evidence start time");
    }
    params.set("start_unix", String(startUnix));
    if (endUnix !== null) {
      params.set("end_unix", String(endUnix));
    }
  }
  if (query.planIds?.length) {
    params.set("plan_ids", query.planIds.join(","));
  }
  setTrimmed(params, "client_id", query.clientId);
  setTrimmed(params, "source", query.source);
  setTrimmed(params, "kind", query.kind);
  setTrimmed(params, "health", query.health);
  setTrimmed(params, "q", query.query);
  if (query.limit !== undefined) {
    params.set("limit", String(Math.max(1, Math.floor(query.limit))));
  }
  return params.toString();
}

export function defaultNetworkEvidenceStartAt(now = Date.now()): string {
  return dateTimeLocalValue(now - 24 * 60 * 60 * 1_000);
}

export function defaultNetworkEvidenceEndAt(now = Date.now()): string {
  return dateTimeLocalValue(now);
}

export function dateTimeLocalValue(timestamp: number): string {
  const date = new Date(timestamp);
  const offset = date.getTimezoneOffset() * 60_000;
  return new Date(timestamp - offset).toISOString().slice(0, 16);
}

export function networkEvidenceWindowLabel(window: MonitoringWindow): string {
  switch (window) {
    case "15m":
      return "last 15 minutes";
    case "1h":
      return "last hour";
    case "8h":
      return "last 8 hours";
    case "1d":
      return "last day";
    case "7d":
      return "last 7 days";
    case "30d":
      return "last 30 days";
    case "90d":
      return "last 90 days";
    case "180d":
      return "last 180 days";
    case "1y":
      return "last year";
    case "all":
      return "all retained history";
    case "custom":
      return "custom range";
  }
}

function dateTimeInputUnix(value: string | undefined): number | null {
  const timestamp = value ? new Date(value).getTime() : Number.NaN;
  return Number.isFinite(timestamp) ? Math.floor(timestamp / 1_000) : null;
}

function setTrimmed(
  params: URLSearchParams,
  key: string,
  value: string | undefined,
) {
  const normalized = value?.trim();
  if (normalized) {
    params.set(key, normalized);
  }
}
