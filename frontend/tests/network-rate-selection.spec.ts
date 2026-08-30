import { expect, test } from "@playwright/test";
import {
  networkInterfaceIsEligible,
  networkRateSelectionLabel,
  resolveNetworkInterfaceEligibility,
  resolveNetworkRateInterfaces,
  selectedNetworkRates,
} from "../src/networkRateSelection";
import type {
  TelemetryNetworkRateRecord,
  VpsRuleValueRecord,
} from "../src/types";

const rates: TelemetryNetworkRateRecord[] = [
  networkRate("eth0", 8_000, 16_000),
  networkRate("eth1", 32_000, 64_000),
  networkRate("lo", 128_000, 256_000),
];

test("live-rate selection follows an explicit traffic-selector reference", () => {
  const rules = [
    rule(
      "network.rate.interfaces",
      {
        mode: "reference",
        reference: { rule: "traffic.selectors" },
      },
      "[traffic.selectors]",
    ),
    rule("traffic.selectors", {
      mode: "exact",
      selectors: [
        selector("eth0", "rx"),
        selector("eth1", "tx"),
        selector("wg0", "total", "tunnel"),
      ],
    }),
  ];
  const before = structuredClone(rates);

  const resolution = resolveNetworkRateInterfaces(rules);
  const selected = selectedNetworkRates(rates, rules);

  expect(resolution.interfaces.has("eth0")).toBe(true);
  expect(resolution.interfaces.has("eth1")).toBe(true);
  expect(resolution.interfaces.has("wg0")).toBe(false);
  expect(selected.map((rate) => rate.interface)).toEqual(["eth0", "eth1"]);
  expect(selected[0].tx_bps_avg).toBe(16_000);
  expect(selected[1].rx_bps_avg).toBe(32_000);
  expect(rates).toEqual(before);
  expect(networkRateSelectionLabel(rules)).toContain(
    "referenced from traffic.selectors",
  );
});

test("a missing live-rate rule selects none rather than inheriting traffic selectors", () => {
  const rules = [
    rule("traffic.selectors", {
      mode: "exact",
      selectors: [selector("eth0", "rx")],
    }),
  ];
  expect(resolveNetworkRateInterfaces(rules)).toMatchObject({
    mode: "exact",
    source: "network.rate.interfaces",
    valid: true,
  });
  expect(selectedNetworkRates(rates, rules)).toEqual([]);
  expect(networkRateSelectionLabel(rules)).toBe(
    "No live-rate interfaces selected",
  );

  expect(selectedNetworkRates(rates, [])).toEqual([]);
  expect(networkRateSelectionLabel([])).toBe("No live-rate interfaces selected");
});

test("live-rate reference object, rather than display syntax, controls inheritance", () => {
  const rules = [
    rule(
      "network.rate.interfaces",
      {
        mode: "reference",
        reference: { rule: "traffic.selectors" },
      },
      "eth9+tx",
    ),
    rule("traffic.selectors", {
      mode: "exact",
      selectors: [selector("eth0", "rx")],
    }),
  ];

  const selected = selectedNetworkRates(rates, rules);
  expect(selected.map((rate) => rate.interface)).toEqual(["eth0"]);
  expect(selected[0].rx_bps_avg).toBe(8_000);
  expect(selected[0].tx_bps_avg).toBe(16_000);

  rules[0].value_json = {
    mode: "reference",
    reference: { rule: "billing.price" },
  };
  expect(selectedNetworkRates(rates, rules)).toEqual([]);
  expect(networkRateSelectionLabel(rules)).toBe(
    "Live-rate interface rule unavailable",
  );

  rules[0] = rule("network.rate.interfaces", { mode: "all" }, "*");
  rules[0].state = "invalid";
  expect(selectedNetworkRates(rates, rules)).toEqual([]);
  expect(networkRateSelectionLabel(rules)).toBe(
    "Live-rate interface rule unavailable",
  );
});

test("live-rate selection filters interfaces but keeps RX and TX", () => {
  const all = [
    rule("network.rate.interfaces", { mode: "all" }, "*"),
  ];
  expect(selectedNetworkRates(rates, all)).toEqual(rates.slice(0, 2));
  expect(networkRateSelectionLabel(all)).toBe("All eligible interfaces");

  const exact = [
    rule(
      "network.rate.interfaces",
      {
        mode: "exact",
        selectors: [selector("eth0", "total"), selector("eth1", "tx")],
      },
      "eth0,eth1+tx",
    ),
  ];
  const selected = selectedNetworkRates(rates, exact);
  expect(selected.map((rate) => rate.interface)).toEqual(["eth0", "eth1"]);
  expect(selected[0].rx_bps_avg).toBe(8_000);
  expect(selected[0].tx_bps_avg).toBe(16_000);
  expect(selected[1].rx_bps_avg).toBe(32_000);
  expect(selected[1].tx_bps_avg).toBe(64_000);
  expect(networkRateSelectionLabel(exact)).toBe("eth0, eth1");

  exact[0].value_json = {
    mode: "exact",
    selectors: [{ ...selector("eth0", "rx"), canonical: "eth0+tx" }],
  };
  expect(selectedNetworkRates(rates, exact)).toEqual([]);
});

test("live-rate selection accepts accounting aggregation suffixes without combining directions", () => {
  const exact = [
    rule(
      "network.rate.interfaces",
      {
        mode: "exact",
        selectors: [selector("eth0", "tx/rx"), selector("eth1", "total")],
      },
      "eth0+tx/rx,eth1",
    ),
  ];

  const selected = selectedNetworkRates(rates, exact);
  expect(selected.map((rate) => rate.interface)).toEqual(["eth0", "eth1"]);
  expect(selected[0]).toMatchObject({ rx_bps_avg: 8_000, tx_bps_avg: 16_000 });
  expect(selected[1]).toMatchObject({ rx_bps_avg: 32_000, tx_bps_avg: 64_000 });
});

test("network interface eligibility is the outer boundary for all and exact rate selection", () => {
  const defaultEligibility = resolveNetworkInterfaceEligibility([]);
  expect(networkInterfaceIsEligible(defaultEligibility, "host", "eth0")).toBe(true);
  expect(networkInterfaceIsEligible(defaultEligibility, "host", "wlan0")).toBe(true);
  expect(networkInterfaceIsEligible(defaultEligibility, "host", "lo")).toBe(false);
  expect(networkInterfaceIsEligible(defaultEligibility, "tunnel", "eth-tunnel")).toBe(false);

  const exactConstrained = [
    rule("network.interfaces", {
      mode: "patterns",
      patterns: ["eth0", "lo"],
    }, "eth0,lo"),
    rule("network.rate.interfaces", {
      mode: "exact",
      selectors: [selector("eth0", "total"), selector("eth1", "total")],
    }, "eth0,eth1"),
  ];
  expect(selectedNetworkRates(rates, exactConstrained).map((rate) => rate.interface)).toEqual([
    "eth0",
  ]);

  const allConstrained = [
    rule("network.interfaces", {
      mode: "patterns",
      patterns: ["eth*"],
    }, "eth*"),
    rule("network.rate.interfaces", { mode: "all" }, "*"),
  ];
  expect(selectedNetworkRates(rates, allConstrained).map((rate) => rate.interface)).toEqual([
    "eth0",
    "eth1",
  ]);

  allConstrained[0] = rule("network.interfaces", { mode: "all" }, "*");
  expect(selectedNetworkRates(rates, allConstrained)).toEqual(rates);
});

test("traffic-selector all remains constrained by network interface eligibility", () => {
  const rules = [
    rule("network.interfaces", {
      mode: "patterns",
      patterns: ["eth0"],
    }, "eth0"),
    rule("network.rate.interfaces", {
      mode: "reference",
      reference: { rule: "traffic.selectors" },
    }, "[traffic.selectors]"),
    rule("traffic.selectors", { mode: "all" }, "*"),
  ];
  expect(selectedNetworkRates(rates, rules).map((rate) => rate.interface)).toEqual(["eth0"]);
});

function rule(
  key: string,
  valueJson: VpsRuleValueRecord["value_json"],
  valueRaw = "eth0+rx,eth1+tx,tunnel:wg0",
): VpsRuleValueRecord {
  return {
    client_id: "v-1",
    key,
    parsed_display: valueRaw,
    source_id: null,
    source_kind: "operator",
    state: "ok",
    updated_at: "2026-08-03T00:00:00Z",
    updated_by: null,
    validation_errors: [],
    value_json: valueJson,
    value_raw: valueRaw,
  };
}

function selector(
  interfaceName: string,
  direction: "rx" | "tx" | "total" | "tx/rx",
  source: "host" | "tunnel" = "host",
) {
  return {
    canonical: `${source === "host" ? "" : `${source}:`}${interfaceName}${direction === "total" ? "" : `+${direction}`}`,
    direction,
    interface: interfaceName,
    source,
  };
}

function networkRate(
  interfaceName: string,
  rxBps: number,
  txBps: number,
): TelemetryNetworkRateRecord {
  return {
    bucket_secs: 60,
    bucket_start: "2026-08-03T00:00:00Z",
    client_id: "v-1",
    interface: interfaceName,
    rx_bps_avg: rxBps,
    rx_bytes_avg: 1_000,
    rx_bytes_delta: 100,
    sample_count: 1,
    tx_bps_avg: txBps,
    tx_bytes_avg: 2_000,
    tx_bytes_delta: 200,
    updated_at: "2026-08-03T00:00:00Z",
  };
}
