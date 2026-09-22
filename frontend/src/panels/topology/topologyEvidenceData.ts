import { networkEvidenceSeriesKey } from "../../networkEvidence";
import type { NetworkObservationRecord } from "../../types";
import { timestampMillis } from "../../utils";

// Preserve the existing 24-sample curve, without retaining or sorting an
// unbounded history for each series. The fixed insertion window makes the
// observation pass linear in input size, including a single dense series.
export const LATENCY_CURVE_SAMPLE_COUNT = 24;

export type LatencyCurvePoint = {
  healthy?: boolean | null;
  latencyAvgMs: number | null;
  lossRatio: number | null;
  reason?: string | null;
};

export type LatencyCurveGroup = {
  key: string;
  label: string;
  detail: string;
  maxLatency: number;
  points: LatencyCurvePoint[];
};

export function maximumLatency(points: readonly LatencyCurvePoint[]): number {
  return points.reduce(
    (maximum, point) =>
      typeof point.latencyAvgMs === "number"
        ? Math.max(maximum, point.latencyAvgMs)
        : maximum,
    1,
  );
}

export function buildLatencyCurveGroups(
  observations: NetworkObservationRecord[],
  clientLabel: (clientId: string) => string,
): LatencyCurveGroup[] {
  const grouped = new Map<string, NetworkObservationRecord[]>();
  for (const observation of observations) {
    if (observation.kind !== "tunnel_reachability") continue;
    const key = networkEvidenceSeriesKey(observation);
    let rows = grouped.get(key);
    if (!rows) {
      rows = [];
      grouped.set(key, rows);
    }
    // Insert after equal timestamps: stable sort + slice(-24) previously kept
    // the last encountered rows on a tie, and chose the last as the label.
    let index = rows.length;
    while (
      index > 0 &&
      rows[index - 1].observed_at.localeCompare(observation.observed_at) > 0
    ) {
      index -= 1;
    }
    if (index === 0 && rows.length === LATENCY_CURVE_SAMPLE_COUNT) continue;
    rows.splice(index, 0, observation);
    if (rows.length > LATENCY_CURVE_SAMPLE_COUNT) rows.shift();
  }
  const groups: LatencyCurveGroup[] = [];
  for (const [key, rows] of grouped) {
    if (rows.length < 2) continue;
    const latest = rows[rows.length - 1];
    const points = rows.map((row) => ({
      healthy: row.healthy,
      latencyAvgMs: row.latency_avg_ms,
      lossRatio: row.packet_loss_ratio ?? null,
      reason: row.reason,
    }));
    groups.push({
      key,
      label: latest.plan_name ?? latest.interface_name ?? "network probe",
      detail: `${latest.client_id ? clientLabel(latest.client_id) : "Unknown VPS"} -> ${latest.peer_client_id ? clientLabel(latest.peer_client_id) : "peer"}`,
      maxLatency: maximumLatency(points),
      points,
    });
  }
  return groups.sort(
    (left, right) =>
      left.label.localeCompare(right.label) ||
      left.detail.localeCompare(right.detail),
  );
}

export function latestObservationRows(
  observations: NetworkObservationRecord[],
): NetworkObservationRecord[] {
  const latest = new Map<string, NetworkObservationRecord>();
  for (const observation of observations) {
    const key = [
      observation.plan_id ?? "unplanned",
      observation.topology_identity_hash ?? "identity",
      observation.kind,
      observation.endpoint_side ?? observation.client_id,
    ].join(":");
    const current = latest.get(key);
    if (
      !current ||
      timestampMillis(observation.observed_at) >
        timestampMillis(current.observed_at)
    ) {
      latest.set(key, observation);
    }
  }
  return Array.from(latest.values()).sort(
    (left, right) =>
      timestampMillis(right.observed_at) - timestampMillis(left.observed_at) ||
      (left.plan_name ?? "").localeCompare(right.plan_name ?? "") ||
      left.client_id.localeCompare(right.client_id),
  );
}
