import type {
  TelemetryNetworkRateRecord,
  VpsRuleValueRecord,
} from "./types";

export type NetworkRateInterfaceResolution = {
  directions: Map<string, number>;
  mode: "all" | "exact";
  source: "network.rate.interfaces" | "traffic.selectors";
  valid: boolean;
};

export function resolveNetworkRateInterfaces(
  rules: VpsRuleValueRecord[],
): NetworkRateInterfaceResolution {
  const rateRule = rules.find(
    (rule) => rule.key === "network.rate.interfaces",
  );
  const rateValue = jsonObject(rateRule?.value_json);
  if (rateRule && rateRule.state !== "ok") {
    return {
      directions: new Map(),
      mode: "exact",
      source: "network.rate.interfaces",
      valid: false,
    };
  }
  if (!rateRule) {
    return {
      directions: new Map(),
      mode: "all",
      source: "network.rate.interfaces",
      valid: true,
    };
  }
  const mode = typeof rateValue?.mode === "string" ? rateValue.mode : null;
  if (mode === "all") {
    return {
      directions: new Map(),
      mode: "all",
      source: "network.rate.interfaces",
      valid: true,
    };
  }
  if (mode === "exact") {
    const directions = selectorDirections(rateValue?.selectors, "reject", false);
    return {
      directions: directions ?? new Map(),
      mode: "exact",
      source: "network.rate.interfaces",
      valid: directions !== null,
    };
  }
  if (
    mode !== "reference" ||
    jsonObject(rateValue?.reference)?.rule !== "traffic.selectors"
  ) {
    return {
      directions: new Map(),
      mode: "exact",
      source: "network.rate.interfaces",
      valid: false,
    };
  }

  return resolveTrafficSelectorReference(rules);
}

function resolveTrafficSelectorReference(
  rules: VpsRuleValueRecord[],
): NetworkRateInterfaceResolution {
  const trafficRule = rules.find((rule) => rule.key === "traffic.selectors");
  if (trafficRule && trafficRule.state !== "ok") {
    return {
      directions: new Map(),
      mode: "exact",
      source: "traffic.selectors",
      valid: false,
    };
  }
  const trafficValue = jsonObject(trafficRule?.value_json);
  const directions = trafficRule
    ? selectorDirections(trafficValue?.selectors, "ignore", true)
    : new Map<string, number>();
  return {
    directions: directions ?? new Map(),
    mode: "exact",
    source: "traffic.selectors",
    valid: directions !== null,
  };
}

export function selectedNetworkRates(
  rates: TelemetryNetworkRateRecord[],
  rules: VpsRuleValueRecord[],
): TelemetryNetworkRateRecord[] {
  const resolution = resolveNetworkRateInterfaces(rules);
  if (!resolution.valid) return [];
  if (resolution.mode === "all") return rates;
  return rates.flatMap((rate) => {
    const directions = resolution.directions.get(rate.interface) ?? 0;
    if (directions === 0) return [];
    return [
      {
        ...rate,
        rx_bytes_avg: directions & 0b01 ? rate.rx_bytes_avg : 0,
        rx_bytes_delta: directions & 0b01 ? rate.rx_bytes_delta : 0,
        rx_bps_avg: directions & 0b01 ? rate.rx_bps_avg : 0,
        tx_bytes_avg: directions & 0b10 ? rate.tx_bytes_avg : 0,
        tx_bytes_delta: directions & 0b10 ? rate.tx_bytes_delta : 0,
        tx_bps_avg: directions & 0b10 ? rate.tx_bps_avg : 0,
      },
    ];
  });
}

export function networkRateSelectionLabel(
  rules: VpsRuleValueRecord[],
): string {
  const resolution = resolveNetworkRateInterfaces(rules);
  if (!resolution.valid) return "Live-rate interface rule unavailable";
  if (resolution.mode === "all") return "All reported interfaces";
  if (resolution.directions.size === 0) {
    return resolution.source === "traffic.selectors"
      ? "Traffic interfaces unavailable"
      : "No live-rate interfaces selected";
  }
  const names = Array.from(resolution.directions, ([name, directions]) =>
    directions === 0b01 ? `${name}+rx` : directions === 0b10 ? `${name}+tx` : name,
  ).sort((left, right) => left.localeCompare(right));
  return resolution.source === "traffic.selectors"
    ? `${names.join(", ")} · referenced from traffic.selectors`
    : names.join(", ");
}

function selectorDirections(
  value: unknown,
  nonHostBehavior: "ignore" | "reject",
  allowDirectionOverlap: boolean,
): Map<string, number> | null {
  const directions = new Map<string, number>();
  const claimedDirections = new Map<string, number>();
  const seen = new Set<string>();
  if (!Array.isArray(value) || value.length > 16) return null;
  for (const item of value) {
    const selector = jsonObject(item);
    const source = selector?.source;
    const interfaceName = selector?.interface;
    const direction = selector?.direction;
    const canonical = selector?.canonical;
    if (
      (source !== "host" && source !== "tunnel") ||
      typeof interfaceName !== "string" ||
      interfaceName.length === 0 ||
      new TextEncoder().encode(interfaceName).length > 128 ||
      /[,+:\s\u0000-\u001f\u007f-\u009f]/u.test(interfaceName) ||
      (direction !== "rx" && direction !== "tx" && direction !== "total") ||
      typeof canonical !== "string"
    ) {
      return null;
    }
    const normalized = `${source === "host" ? "" : `${source}:`}${interfaceName}${direction === "total" ? "" : `+${direction}`}`;
    const mask = direction === "rx" ? 0b01 : direction === "tx" ? 0b10 : 0b11;
    const selectorKey = `${source}\u0000${interfaceName}`;
    const claimed = claimedDirections.get(selectorKey) ?? 0;
    if (
      canonical !== normalized ||
      seen.has(canonical) ||
      (!allowDirectionOverlap && (claimed & mask) !== 0)
    ) {
      return null;
    }
    seen.add(canonical);
    claimedDirections.set(selectorKey, claimed | mask);
    if (source !== "host") {
      if (nonHostBehavior === "reject") return null;
      continue;
    }
    directions.set(
      interfaceName,
      (directions.get(interfaceName) ?? 0) | mask,
    );
  }
  return directions;
}

function jsonObject(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
