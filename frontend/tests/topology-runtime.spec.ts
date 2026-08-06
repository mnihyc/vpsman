import { expect, test } from "@playwright/test";
import {
  buildRuntimeControl,
  calculateOspfCostPreview,
  clampTunnelBandwidthMbps,
  defaultAgentTunnelMtu,
  isDerivedAgentTunnelMtu,
  runtimeManagerLabel,
} from "../src/topologyRuntime";
import { networkSpeedServerSide } from "../src/topologyNetworkJobs";

const runtimeControlValues = {
  burstKb: "",
  egressKbps: "",
  ingressKbps: "",
};

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
