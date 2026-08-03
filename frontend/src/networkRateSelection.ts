import type {
  TelemetryNetworkRateRecord,
  VpsRuleValueRecord,
} from "./types";

export type NetworkRateInterfaceResolution = {
  interfaces: Set<string>;
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
      interfaces: new Set(),
      mode: "exact",
      source: "network.rate.interfaces",
      valid: false,
    };
  }
  if (!rateRule) {
    return resolveTrafficSelectorReference(rules);
  }
  const mode = typeof rateValue?.mode === "string" ? rateValue.mode : null;
  if (mode === "all") {
    return {
      interfaces: new Set(),
      mode: "all",
      source: "network.rate.interfaces",
      valid: true,
    };
  }
  if (mode === "exact") {
    const interfaces = selectorInterfaces(rateValue?.selectors, "reject", false);
    return {
      interfaces: interfaces ?? new Set(),
      mode: "exact",
      source: "network.rate.interfaces",
      valid: interfaces !== null,
    };
  }
  if (
    mode !== "reference" ||
    jsonObject(rateValue?.reference)?.rule !== "traffic.selectors"
  ) {
    return {
      interfaces: new Set(),
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
      interfaces: new Set(),
      mode: "exact",
      source: "traffic.selectors",
      valid: false,
    };
  }
  const trafficValue = jsonObject(trafficRule?.value_json);
  const interfaces = trafficRule
    ? selectorInterfaces(trafficValue?.selectors, "ignore", true)
    : new Set<string>();
  return {
    interfaces: interfaces ?? new Set(),
    mode: "exact",
    source: "traffic.selectors",
    valid: interfaces !== null,
  };
}

export function selectedNetworkRates(
  rates: TelemetryNetworkRateRecord[],
  rules: VpsRuleValueRecord[],
): TelemetryNetworkRateRecord[] {
  const resolution = resolveNetworkRateInterfaces(rules);
  if (!resolution.valid) return [];
  if (resolution.mode === "all") return rates;
  return rates.filter((rate) => resolution.interfaces.has(rate.interface));
}

export function networkRateSelectionLabel(
  rules: VpsRuleValueRecord[],
): string {
  const resolution = resolveNetworkRateInterfaces(rules);
  if (!resolution.valid) return "Live-rate interface rule unavailable";
  if (resolution.mode === "all") return "All reported interfaces";
  if (resolution.interfaces.size === 0) {
    return resolution.source === "traffic.selectors"
      ? "Traffic interfaces unavailable"
      : "No live-rate interfaces selected";
  }
  const names = Array.from(resolution.interfaces).sort((left, right) =>
    left.localeCompare(right),
  );
  return resolution.source === "traffic.selectors"
    ? `${names.join(", ")} · referenced from traffic.selectors`
    : names.join(", ");
}

function selectorInterfaces(
  value: unknown,
  nonHostBehavior: "ignore" | "reject",
  allowDirectionOverlap: boolean,
): Set<string> | null {
  const interfaces = new Set<string>();
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
    interfaces.add(interfaceName);
  }
  return interfaces;
}

function jsonObject(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}
