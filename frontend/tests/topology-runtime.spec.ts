import { expect, test } from "@playwright/test";
import {
  additionalAddressChanges,
  additionalAddressDraft,
  additionalAddressLines,
  additionalAddressReservations,
  additionalAddressesFromDraft,
  buildRuntimeControl,
  calculateOspfCostPreview,
  clampTunnelBandwidthMbps,
  defaultAgentTunnelMtu,
  isDerivedAgentTunnelMtu,
  runtimeManagerLabel,
  validateTunnelPlanName,
  tunnelLinkLocalSummary,
  isTunnelLinkLocal,
} from "../src/topologyRuntime";
import type { TunnelPlanInput } from "../src/types";

test("additional tunnel addresses round trip independently without changing editable lines", () => {
  const addresses = {
    left: { ipv4: ["192.0.2.1/32"], ipv6: [] },
    right: { ipv4: [], ipv6: ["fd00::2/128", "fe80::2/64"] },
  };
  const draft = additionalAddressDraft(addresses);
  expect(additionalAddressesFromDraft(draft)).toEqual(addresses);
  draft.left.ipv6 = "\n  fd00::1/128  \r\n\nfe80::1/64\n";
  const before = structuredClone(draft);
  expect(additionalAddressesFromDraft(draft).left.ipv6).toEqual(["fd00::1/128", "fe80::1/64"]);
  expect(additionalAddressesFromDraft(draft).right).toEqual(addresses.right);
  expect(draft).toEqual(before);
  draft.right.ipv6 = "";
  expect(additionalAddressesFromDraft(draft).right.ipv6).toEqual([]);
  expect(additionalAddressLines("\n \r\n")).toEqual([]);
});

test("allocation reserves unsaved extra host addresses without changing CIDR drafts", () => {
  const draft = additionalAddressDraft();
  draft.left.ipv4 = " 10.255.0.2/32\n10.255.0.3/24\n";
  draft.left.ipv6 = "fe80::1/64";
  draft.right.ipv6 = "fd00::2/128\nfe80::2/64";
  const before = structuredClone(draft);
  expect(additionalAddressReservations(draft)).toEqual([
    "10.255.0.2", "10.255.0.3", "fe80::1", "fd00::2", "fe80::2",
  ]);
  expect(draft).toEqual(before);
});

test("link-local policy is per endpoint and manual addresses survive unmanaged mode", () => {
  const input: Pick<TunnelPlanInput, "ipv6_tunnel" | "additional_addresses" | "manage_link_local"> = {
    additional_addresses: {
      left: { ipv4: [], ipv6: ["fe80::1/64"] },
      right: { ipv4: ["192.0.2.2/32"], ipv6: [] },
    },
  };
  expect(tunnelLinkLocalSummary(input, "left")).toBe("On · automatic link-local; explicit fe80::1/64");
  expect(tunnelLinkLocalSummary(input, "right")).toBe("On · inactive (no configured IPv6)");
  input.additional_addresses!.right.ipv6 = ["fd00::2/128"];
  expect(tunnelLinkLocalSummary(input, "right")).toBe("On · automatic link-local");
  input.manage_link_local = false;
  expect(tunnelLinkLocalSummary(input, "left")).toBe("Off · native behavior; explicit fe80::1/64");
  expect(tunnelLinkLocalSummary(input, "right")).toBe("Off · native behavior");
  input.manage_link_local = true;
  input.additional_addresses!.left.ipv6 = [];
  input.ipv6_tunnel = { left: "fe80::10", right: "fe80::20", prefix_len: 64 };
  expect(tunnelLinkLocalSummary(input, "left")).toBe("On · automatic link-local; explicit fe80::10");
});

test("link-local classification preserves link scope across the entire fe80::/10 range", () => {
  for (const address of ["fe80::1", "FE9A::1/64", "feaf::1", "febf::1/128"]) {
    expect(isTunnelLinkLocal(address)).toBe(true);
  }
  for (const address of ["fec0::1", "fd00::1", "192.0.2.1"]) {
    expect(isTunnelLinkLocal(address)).toBe(false);
  }
});

test("address review reports exact additions and removals without treating reorder as replacement", () => {
  expect(additionalAddressChanges(["fd00::2/128"], ["fd00::1/128"]))
    .toBe("Add fd00::2/128; Remove fd00::1/128");
  expect(additionalAddressChanges(["fd00::2/128", "fd00::1/128"], ["fd00::1/128", "fd00::2/128"]))
    .toBe("fd00::2/128, fd00::1/128");
  expect(additionalAddressChanges([], ["fd00::1/128"]))
    .toBe("Remove fd00::1/128");
  expect(additionalAddressChanges([])).toBe("None");
});
import { networkSpeedServerSide } from "../src/topologyNetworkJobs";
import {
  networkEvidenceMatchesPlan,
  networkEvidencePlanKey,
  networkEvidenceSeriesKey,
} from "../src/networkEvidence";

test("tunnel evidence ownership survives rename without accepting another plan's reused name", () => {
  const plan = { id: "plan-a", name: "renamed" };
  const sameOwner = { plan_id: "plan-a", plan_name: "old" };
  const otherOwner = { plan_id: "plan-b", plan_name: "renamed" };
  const unbound = { plan_id: null, plan_name: "renamed" };
  expect(networkEvidenceMatchesPlan(sameOwner, plan)).toBe(true);
  expect(networkEvidenceMatchesPlan(otherOwner, plan)).toBe(false);
  expect(networkEvidenceMatchesPlan(unbound, plan)).toBe(false);
});

test("tunnel bandwidth baseline uses its UUID without falling through to a reused display name", () => {
  const original = networkEvidencePlanKey({ planId: "plan-a", planName: "old" });
  const renamed = networkEvidencePlanKey({ planId: "plan-a", planName: "new" });
  const reusedName = networkEvidencePlanKey({
    planId: "plan-b",
    planName: "old",
  });
  const unbound = networkEvidencePlanKey({ planName: "old" });
  const baselines = new Map([[original, 100]]);
  expect(baselines.get(renamed)).toBe(100);
  expect(baselines.get(reusedName)).toBeUndefined();
  expect(baselines.get(unbound)).toBeUndefined();
  expect(networkEvidencePlanKey({})).toBeNull();
});

test("tunnel evidence curves share renamed samples but separate topology and stream changes", () => {
  const observation = {
    kind: "tunnel_reachability",
    plan_id: "plan-a",
    plan_name: "old",
    topology_identity_hash: "topology-a",
    interface_name: "tun0",
    client_id: "left",
    peer_client_id: "right",
    target: "10.0.0.1",
  };
  const key = networkEvidenceSeriesKey(observation);
  expect(networkEvidenceSeriesKey({ ...observation, plan_name: "new" })).toBe(
    key,
  );
  for (const change of [
    { plan_id: "plan-b" },
    { topology_identity_hash: "topology-b" },
    { interface_name: "tun1" },
    { peer_client_id: "other" },
    { target: "fd00::1" },
  ]) {
    expect(networkEvidenceSeriesKey({ ...observation, ...change })).not.toBe(
      key,
    );
  }
});

const runtimeControlValues = {
  burstKb: "",
  egressKbps: "",
  ingressKbps: "",
};

test("tunnel plan names enforce the 128-byte UTF-8 boundary", () => {
  expect(validateTunnelPlanName("p".repeat(128))).toBeNull();
  expect(validateTunnelPlanName("é".repeat(64))).toBeNull();
  expect(validateTunnelPlanName("é".repeat(65))).toBe(
    "Plan name must be 128 UTF-8 bytes or fewer",
  );
  expect(validateTunnelPlanName(` ${"p".repeat(128)} `)).toBe(
    "Plan name must be 128 UTF-8 bytes or fewer",
  );
});

function previewCost(
  bandwidthMbps: number,
  latencyMs = 20,
  packetLossRatio = 0,
  preference = 1,
) {
  return calculateOspfCostPreview({
    bandwidthMbps,
    latencyMs,
    packetLossRatio,
    preference,
  });
}

test("OSPF preview keeps arbitrary bandwidth Mbps smooth and bounded", () => {
  expect(previewCost(10)).toBe(52);
  expect(previewCost(20)).toBe(42);
  expect(previewCost(50)).toBe(34);
  expect(previewCost(100)).toBe(30);
  expect(previewCost(250)).toBe(26);
  expect(previewCost(500)).toBe(24);
  expect(previewCost(1000)).toBe(23);
  expect(previewCost(5000)).toBe(21);
  expect(previewCost(10000)).toBe(21);

  let previous = previewCost(10);
  for (let bandwidthMbps = 11; bandwidthMbps <= 10000; bandwidthMbps += 1) {
    const current = previewCost(bandwidthMbps);
    expect(
      current,
      `cost increased from ${previous} to ${current} at ${bandwidthMbps} Mbps`,
    ).toBeLessThanOrEqual(previous);
    expect(
      previous - current,
      `cost changed too abruptly from ${previous} to ${current} at ${bandwidthMbps} Mbps`,
    ).toBeLessThanOrEqual(2);
    previous = current;
  }

  const lowBandwidthGain = previewCost(10) - previewCost(100);
  const midBandwidthGain = previewCost(100) - previewCost(1000);
  const highBandwidthGain = previewCost(1000) - previewCost(10000);
  expect(lowBandwidthGain).toBeGreaterThan(midBandwidthGain);
  expect(midBandwidthGain).toBeGreaterThan(highBandwidthGain);
  expect(previewCost(10000, 70)).toBeGreaterThan(previewCost(10, 20));
});

test("OSPF preview handles non-preset operator bandwidth values", () => {
  expect(previewCost(123)).toBe(29);
  expect(previewCost(1234)).toBe(23);
  expect(previewCost(9876)).toBe(21);

  expect(previewCost(10) - previewCost(100)).toBeGreaterThan(
    previewCost(100) - previewCost(1000),
  );
  expect(previewCost(100) - previewCost(1000)).toBeGreaterThan(
    previewCost(1000) - previewCost(10000),
  );
});

test("OSPF preview has no legacy bandwidth tier cliffs", () => {
  for (const legacyTier of [100, 1000, 5000, 10000]) {
    const lower = Math.max(10, legacyTier - 1);
    const upper = Math.min(10000, legacyTier + 1);
    const costs = Array.from(
      { length: upper - lower + 1 },
      (_value, index) => previewCost(lower + index),
    );
    const minCost = Math.min(...costs);
    const maxCost = Math.max(...costs);
    expect(
      maxCost - minCost,
      `legacy tier ${legacyTier} has a preview cliff across ${lower}..=${upper}: ${costs.join(", ")}`,
    ).toBeLessThanOrEqual(1);
  }
});

test("OSPF preview balances arbitrary bandwidth against loss latency and preference", () => {
  const baselineLowBandwidth = previewCost(10, 20, 0, 1);

  expect(previewCost(10000, 70, 0, 1)).toBeGreaterThan(baselineLowBandwidth);
  expect(previewCost(10000, 20, 0.1, 1)).toBeGreaterThan(baselineLowBandwidth);
  expect(previewCost(100, 20, 0, 1.2)).toBeLessThan(
    previewCost(10000, 20, 0, 0.8),
  );
});

test("OSPF preview keeps full bandwidth advantage bounded", () => {
  const slowHealthy = previewCost(10, 20, 0, 1);
  const fastHealthy = previewCost(10000, 20, 0, 1);
  const fullBandwidthAdvantage = slowHealthy - fastHealthy;

  expect(fullBandwidthAdvantage).toBe(31);
  expect(
    previewCost(10000, 20 + fullBandwidthAdvantage + 1, 0, 1),
  ).toBeGreaterThan(slowHealthy);
  expect(
    previewCost(10000, 20, (fullBandwidthAdvantage + 1) / 400, 1),
  ).toBeGreaterThan(slowHealthy);
});

test("OSPF preview keeps bandwidth secondary to path health", () => {
  for (const latencyMs of [5, 20, 80, 180]) {
    expect(previewCost(10000, latencyMs + 32, 0, 1)).toBeGreaterThan(
      previewCost(10, latencyMs, 0, 1),
    );
    expect(previewCost(10000, latencyMs, 0.08, 1)).toBeGreaterThan(
      previewCost(10, latencyMs, 0, 1),
    );
  }

  expect(previewCost(10000, 20, 0, 0.8)).toBeGreaterThan(
    previewCost(10000, 20, 0, 1.2),
  );
});

test("OSPF preview clamps bandwidth and applies operator preference predictably", () => {
  expect(clampTunnelBandwidthMbps(1)).toBe(10);
  expect(clampTunnelBandwidthMbps(10)).toBe(10);
  expect(clampTunnelBandwidthMbps(1234.4)).toBe(1234);
  expect(clampTunnelBandwidthMbps(1234.5)).toBe(1235);
  expect(clampTunnelBandwidthMbps(20000)).toBe(10000);

  expect(previewCost(1)).toBe(previewCost(10));
  expect(previewCost(20000)).toBe(previewCost(10000));
  expect(previewCost(100, 20, 0, 2)).toBe(15);
  expect(previewCost(100, 20, 0, 0.01)).toBe(300);
  expect(previewCost(100, 20, 0.01, 1)).toBe(34);
});

test("OSPF preview sanitizes temporary numeric form states", () => {
  expect(previewCost(Number.NaN)).toBe(previewCost(100));
  expect(previewCost(Number.POSITIVE_INFINITY)).toBe(previewCost(100));
  expect(
    calculateOspfCostPreview({
      bandwidthMbps: 100,
      latencyMs: Number.NaN,
      packetLossRatio: Number.POSITIVE_INFINITY,
      preference: Number.NaN,
    }),
  ).toBe(10);
  expect(previewCost(100, -20, -1, -1)).toBe(100);
});

test("tunnel runtime ownership uses one operator-facing vocabulary", () => {
  expect(runtimeManagerLabel("agent_builtin")).toBe("Agent builtin");
  expect(runtimeManagerLabel("external_observed")).toBe("External observed");
  expect(runtimeManagerLabel("custom_adapter")).toBe("Custom adapter");
});

test("Agent builtin tunnel MTU defaults account for encapsulation", () => {
  expect(defaultAgentTunnelMtu("gre")).toBe(1476);
  expect(defaultAgentTunnelMtu("ipip")).toBe(1480);
  expect(defaultAgentTunnelMtu("sit")).toBe(1480);
  expect(defaultAgentTunnelMtu("fou")).toBe(1472);
  expect(defaultAgentTunnelMtu("wireguard")).toBe(1420);
  expect(defaultAgentTunnelMtu("openvpn")).toBe(1500);
  expect(defaultAgentTunnelMtu("tun_tap")).toBeNull();
  expect(defaultAgentTunnelMtu("custom")).toBeNull();
});

test("only absent or kind-default Agent builtin MTUs remain derived", () => {
  expect(isDerivedAgentTunnelMtu("gre", null)).toBe(true);
  expect(isDerivedAgentTunnelMtu("gre", 1476)).toBe(true);
  expect(isDerivedAgentTunnelMtu("gre", 1400)).toBe(false);
  expect(isDerivedAgentTunnelMtu("wireguard", 1476)).toBe(false);
});

test("network speed directions map to the receiving endpoint", () => {
  expect(networkSpeedServerSide("left_to_right")).toBe("right");
  expect(networkSpeedServerSide("right_to_left")).toBe("left");
});

test("WireGuard runtime accepts zero keepalive without relaxing port validation", () => {
  const control = buildRuntimeControl("agent_builtin", {
    ...runtimeControlValues,
    wireguardEndpointMode: "both",
    wireguardLeftKeepaliveSecs: "0",
    wireguardLeftListenPort: "51820",
    wireguardRightKeepaliveSecs: "25",
    wireguardRightListenPort: "51821",
  });

  expect(control.wireguard).toEqual({
    endpoint_mode: "both",
    left_keepalive_secs: 0,
    left_listen_port: 51820,
    right_keepalive_secs: 25,
    right_listen_port: 51821,
  });
  expect(() =>
    buildRuntimeControl("agent_builtin", {
      ...runtimeControlValues,
      wireguardEndpointMode: "both",
      wireguardLeftListenPort: "0",
    }),
  ).toThrow("Invalid numeric value 0");
  expect(() =>
    buildRuntimeControl("agent_builtin", {
      ...runtimeControlValues,
      wireguardEndpointMode: "both",
      wireguardLeftKeepaliveSecs: "0.5",
    }),
  ).toThrow("Invalid non-negative integer value 0.5");
});
