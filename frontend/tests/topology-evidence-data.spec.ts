import { expect, test } from "@playwright/test";
import { networkEvidenceSeriesKey } from "../src/networkEvidence";
import {
  buildLatencyCurveGroups,
  latestObservationRows,
  maximumLatency,
} from "../src/panels/topology/topologyEvidenceData";
import type { NetworkObservationRecord } from "../src/types";

function observation(index: number, overrides: Partial<NetworkObservationRecord> = {}): NetworkObservationRecord {
  return {
    id: `observation-${index}`, job_id: null, client_id: "left", seq: index,
    kind: "tunnel_reachability", source: "automatic", role: null,
    plan_id: "plan-a", topology_identity_hash: "identity-a", plan_name: "Plan A",
    interface_name: "tun-a", peer_client_id: "right", target: "192.0.2.1",
    endpoint_side: "left", address_family: "ipv4", stale_after_secs: 180,
    healthy: true, transmitted: 5, received: 5, latency_min_ms: 1,
    latency_avg_ms: index, latency_max_ms: index, latency_mdev_ms: 0,
    packet_loss_ratio: 0, reason: `sample-${index}`, throughput_mbps: null,
    bytes: null, metadata: {}, observed_at: "2026-09-22T12:00:00Z",
    received_at: "2026-09-22T12:00:00Z", ...overrides,
  };
}

test("dense 250,000-observation series keeps the latest 24 stable ties without a spread-argument limit", () => {
  const observations = Array.from({ length: 250_000 }, (_, index) => observation(index));
  const groups = buildLatencyCurveGroups(observations, (id) => id);
  expect(groups).toHaveLength(1);
  expect(groups[0].points.map((point) => point.latencyAvgMs)).toEqual(
    Array.from({ length: 24 }, (_, index) => 249_976 + index),
  );
  expect(groups[0].maxLatency).toBe(249_999);
  expect(maximumLatency(observations.map((row) => ({
    latencyAvgMs: row.latency_avg_ms, lossRatio: row.packet_loss_ratio,
  })))).toBe(249_999);
});

test("unsorted interleaved series preserve old ordering, timestamp ties, gaps and latest labels", () => {
  const observations = Array.from({ length: 400 }, (_, index) => observation(index, {
    // Five timestamp values intentionally produce ties, across four series.
    observed_at: `2026-09-22T12:00:0${(index * 7) % 5}Z`,
    plan_id: `plan-${index % 2}`,
    plan_name: index > 350 ? "Latest label" : "Old label",
    client_id: `client-${index % 4}`,
    healthy: index % 7 ? true : false,
    latency_avg_ms: index % 7 ? index : null,
    packet_loss_ratio: index % 7 ? 0 : null,
  }));
  const inputOrder = observations.map((row) => row.id);
  const grouped = new Map<string, NetworkObservationRecord[]>();
  for (const row of observations) {
    const key = networkEvidenceSeriesKey(row);
    const rows = grouped.get(key) ?? [];
    rows.push(row);
    grouped.set(key, rows);
  }
  const expected = Array.from(grouped, ([key, rows]) => {
    const sorted = rows.slice().sort((left, right) => left.observed_at.localeCompare(right.observed_at)).slice(-24);
    const latest = sorted[sorted.length - 1];
    const points = sorted.map((row) => ({
      healthy: row.healthy, latencyAvgMs: row.latency_avg_ms,
      lossRatio: row.packet_loss_ratio, reason: row.reason,
    }));
    return {
      key, label: latest.plan_name,
      detail: `${latest.client_id} -> ${latest.peer_client_id}`,
      maxLatency: maximumLatency(points), points,
    };
  }).sort((left, right) => left.label!.localeCompare(right.label!) || left.detail.localeCompare(right.detail));
  expect(buildLatencyCurveGroups(observations, (id) => id)).toEqual(expected);
  expect(observations.map((row) => row.id)).toEqual(inputOrder);
});

test("all curve groups remain available beyond the first page, excluding singletons and other kinds", () => {
  const observations = Array.from({ length: 25 }, (_, group) => [
    observation(group * 2, { plan_id: `plan-${group}` }),
    observation(group * 2 + 1, { plan_id: `plan-${group}` }),
  ]).flat();
  observations.push(observation(60, { plan_id: "single" }));
  observations.push(observation(61, { kind: "network_status" }));
  const groups = buildLatencyCurveGroups(observations, (id) => id);
  expect(groups).toHaveLength(25);
  expect(groups.every((group) => group.points.length === 2)).toBe(true);
  expect(groups[24].points[1].latencyAvgMs).toBe(49);
});

test("latest status rows aggregate all input before pagination and preserve first timestamp ties", () => {
  const old = observation(0);
  const tied = observation(1);
  const newest = observation(2, { observed_at: "2026-09-22T12:01:00Z" });
  const otherSide = observation(3, { endpoint_side: "right" });
  const otherKind = observation(4, { kind: "network_status" });
  const otherIdentity = observation(5, { topology_identity_hash: "other" });
  expect(latestObservationRows([old, tied])).toEqual([old]);
  const observations = [old, tied, ...Array.from({ length: 30 }, (_, index) => observation(100 + index, { plan_id: `extra-${index}` })), otherSide, otherKind, otherIdentity, newest];
  const rows = latestObservationRows(observations);
  expect(rows).toHaveLength(34);
  expect(rows[0]).toBe(newest);
  expect(rows).toEqual(expect.arrayContaining([otherSide, otherKind, otherIdentity]));
  expect(rows).not.toContain(old);
  expect(rows).not.toContain(tied);
});

test("empty and all-gap histories retain a safe unit scale", () => {
  expect(maximumLatency([])).toBe(1);
  const observations = [observation(0, { latency_avg_ms: null }), observation(1, { latency_avg_ms: null })];
  expect(buildLatencyCurveGroups(observations, (id) => id)[0].maxLatency).toBe(1);
  expect(buildLatencyCurveGroups([], (id) => id)).toEqual([]);
});
