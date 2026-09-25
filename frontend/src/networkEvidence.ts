import type { MonitoringWindow } from "./components/MonitoringRangeTabs";
import type { NetworkObservationRecord } from "./types";

// This fixed schema is the observations endpoint's opt-in fields-once format.
export const NETWORK_OBSERVATION_FIELDS = [
  "id", "job_id", "client_id", "seq", "kind", "source", "role", "plan_id",
  "topology_identity_hash", "plan_name", "interface_name", "peer_client_id",
  "target", "endpoint_side", "address_family", "stale_after_secs", "healthy",
  "transmitted", "received", "latency_min_ms", "latency_avg_ms", "latency_max_ms",
  "latency_mdev_ms", "packet_loss_ratio", "reason", "throughput_mbps", "bytes",
  "metadata", "observed_at", "received_at",
] as const satisfies readonly (keyof NetworkObservationRecord)[];

type NetworkObservationValues<Fields extends readonly (keyof NetworkObservationRecord)[]> = {
  [Index in keyof Fields]: NetworkObservationRecord[Fields[Index]];
};

export type CompactNetworkObservations = {
  fields: typeof NETWORK_OBSERVATION_FIELDS;
  rows: NetworkObservationValues<typeof NETWORK_OBSERVATION_FIELDS>[];
};

export function decodeNetworkObservations(
  response: CompactNetworkObservations,
): NetworkObservationRecord[] {
  if (
    !Array.isArray(response.fields) ||
    response.fields.length !== NETWORK_OBSERVATION_FIELDS.length ||
    NETWORK_OBSERVATION_FIELDS.some((field, index) => response.fields[index] !== field) ||
    !Array.isArray(response.rows)
  ) {
    throw new Error("Unsupported compact network observation schema");
  }
  return response.rows.map((row) => {
    if (!Array.isArray(row) || row.length !== NETWORK_OBSERVATION_FIELDS.length) {
      throw new Error("Invalid compact network observation row");
    }
    // Rebuild the existing record shape once at the API boundary. Explicit
    // fields avoid temporary entry arrays and preserve metadata without edits.
    return {
      id: row[0],
      job_id: row[1],
      client_id: row[2],
      seq: row[3],
      kind: row[4],
      source: row[5],
      role: row[6],
      plan_id: row[7],
      topology_identity_hash: row[8],
      plan_name: row[9],
      interface_name: row[10],
      peer_client_id: row[11],
      target: row[12],
      endpoint_side: row[13],
      address_family: row[14],
      stale_after_secs: row[15],
      healthy: row[16],
      transmitted: row[17],
      received: row[18],
      latency_min_ms: row[19],
      latency_avg_ms: row[20],
      latency_max_ms: row[21],
      latency_mdev_ms: row[22],
      packet_loss_ratio: row[23],
      reason: row[24],
      throughput_mbps: row[25],
      bytes: row[26],
      metadata: row[27],
      observed_at: row[28],
      received_at: row[29],
    };
  });
}

export type NetworkEvidencePlanIdentity = {
  planId?: string | null;
  topologyIdentityHash?: string | null;
  planName?: string | null;
  interfaceName?: string | null;
};

export function networkEvidencePlanKey(
  identity: NetworkEvidencePlanIdentity,
): string | null {
  if (identity.planId) return JSON.stringify(["plan", identity.planId]);
  if (identity.topologyIdentityHash) {
    return JSON.stringify(["topology", identity.topologyIdentityHash]);
  }
  return identity.planName
    ? JSON.stringify(["name", identity.planName, identity.interfaceName ?? null])
    : null;
}

export function networkEvidenceMatchesPlan(
  evidence: Pick<NetworkObservationRecord, "plan_id">,
  plan: { id: string },
): boolean {
  return evidence.plan_id === plan.id;
}

export function networkEvidenceSeriesKey(
  evidence: Pick<
    NetworkObservationRecord,
    | "kind"
    | "plan_id"
    | "plan_name"
    | "topology_identity_hash"
    | "interface_name"
    | "client_id"
    | "peer_client_id"
  > & { target?: string | null },
): string {
  return JSON.stringify([
    evidence.kind,
    networkEvidencePlanKey({
      planId: evidence.plan_id,
      topologyIdentityHash: evidence.topology_identity_hash,
      planName: evidence.plan_name,
      interfaceName: evidence.interface_name,
    }),
    evidence.topology_identity_hash,
    evidence.interface_name,
    evidence.client_id,
    evidence.peer_client_id,
    evidence.target ?? null,
  ]);
}

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
