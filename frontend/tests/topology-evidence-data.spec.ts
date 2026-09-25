import { expect, test } from "@playwright/test";
import {
  decodeNetworkObservations,
  NETWORK_OBSERVATION_FIELDS,
  networkEvidenceSeriesKey,
  type CompactNetworkObservations,
} from "../src/networkEvidence";
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

function compactObservations(records: NetworkObservationRecord[]): CompactNetworkObservations {
  // Simulate the JSON boundary, preserving the same values as an object reply.
  return JSON.parse(JSON.stringify({
    fields: NETWORK_OBSERVATION_FIELDS,
    rows: records.map((record) => NETWORK_OBSERVATION_FIELDS.map((field) => record[field])),
  }));
}

test("compact observations preserve every manual field, null, empty key and arbitrary metadata in order", () => {
  const manual = observation(7, {
    job_id: "22222222-2222-4222-8222-222222222222", source: "manual",
    seq: 0, kind: "network_speed_test", role: "client", interface_name: "",
    target: "[2001:db8::1]:5201", endpoint_side: "right", address_family: "ipv6",
    healthy: false, received: 0, latency_min_ms: 0, latency_avg_ms: 1.25,
    latency_max_ms: 3.5, latency_mdev_ms: 0.75, packet_loss_ratio: 1,
    reason: "line\nquote\"", throughput_mbps: 42.125, bytes: Number.MAX_SAFE_INTEGER,
    plan_name: "Plan \"quoted\" / 隧道",
    metadata: JSON.parse('{"":null,"nested":[true,false,0,-1.25,{"id":"not-a-column","__proto__":{"safe":true}}]}'),
    observed_at: "2026-09-26T01:02:03.123456Z",
    received_at: "2026-09-26T01:02:04Z",
  });
  const nullable = observation(8, {
    job_id: null, client_id: "", seq: null, role: null, plan_id: null,
    topology_identity_hash: null, plan_name: null, interface_name: null,
    peer_client_id: null, target: null, endpoint_side: null, address_family: null,
    stale_after_secs: null, healthy: null, transmitted: null, received: null,
    latency_min_ms: null, latency_avg_ms: null, latency_max_ms: null,
    latency_mdev_ms: null, packet_loss_ratio: null, reason: null,
    throughput_mbps: null, bytes: null, metadata: null,
    observed_at: "2026-09-26T01:02:00Z", received_at: "",
  });
  const records = [manual, nullable, ...[[], {}, "", false, 12.5].map((metadata) => ({ ...nullable, metadata }))];
  const response = compactObservations(records);
  const restored = decodeNetworkObservations(response);
  expect(restored).toEqual(records);
  expect(restored.map((record) => Object.keys(record).sort())).toEqual(
    records.map((record) => Object.keys(record).sort()),
  );
  expect(restored[0].metadata).toBe(response.rows[0][27]);
  expect(NETWORK_OBSERVATION_FIELDS).toHaveLength(30);
  expect(new Set(NETWORK_OBSERVATION_FIELDS).size).toBe(30);
  expect(JSON.stringify(response).length).toBeLessThan(JSON.stringify(records).length);
  expect(decodeNetworkObservations(compactObservations([]))).toEqual([]);
});

test("compact observations reject missing, duplicated or reordered fields and truncated rows", () => {
  const response = compactObservations([observation(1)]);
  const malformed = [
    [],
    { ...response, fields: response.fields.slice(1) },
    { ...response, fields: response.fields.map((field, index) => index === 1 ? "id" : field) },
    { ...response, fields: response.fields.slice().reverse() },
    { ...response, rows: [response.rows[0].slice(1)] },
    { ...response, rows: [[...response.rows[0], "unexpected"]] },
  ];
  for (const payload of malformed) {
    expect(() => decodeNetworkObservations(JSON.parse(JSON.stringify(payload)))).toThrow(/compact network observation/);
  }
});

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
