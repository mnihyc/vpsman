import type {
  TelemetryNetworkRateRecord,
  VpsRuleValueRecord,
} from "./types";

export type NetworkInterfaceEligibility = {
  mode: "all" | "default_physical" | "patterns";
  patterns: readonly string[];
  valid: boolean;
};

export type NetworkRateInterfaceResolution = {
  eligibility: NetworkInterfaceEligibility;
  interfaces: Set<string>;
  mode: "all" | "exact";
  source: "network.rate.interfaces" | "traffic.selectors";
  valid: boolean;
};

export function resolveNetworkInterfaceEligibility(
  rules: readonly VpsRuleValueRecord[],
): NetworkInterfaceEligibility {
  const rule = rules.find((candidate) => candidate.key === "network.interfaces");
  if (!rule) {
    return { mode: "default_physical", patterns: ["e*", "w*"], valid: true };
  }
  if (rule.state !== "ok") {
    return { mode: "patterns", patterns: [], valid: false };
  }
  const value = jsonObject(rule.value_json);
  if (value?.mode === "all") {
    return { mode: "all", patterns: [], valid: true };
  }
  if (value?.mode !== "patterns" || !validInterfacePatterns(value.patterns)) {
    return { mode: "patterns", patterns: [], valid: false };
  }
  return {
    mode: "patterns",
    patterns: value.patterns as string[],
    valid: true,
  };
}

export function networkInterfaceIsEligible(
  eligibility: NetworkInterfaceEligibility,
  source: "host" | "tunnel",
  interfaceName: string,
): boolean {
  if (!eligibility.valid) return false;
  if (eligibility.mode === "all") return true;
  if (eligibility.mode === "default_physical") {
    return (
      source === "host" &&
      (interfaceName.startsWith("e") || interfaceName.startsWith("w"))
    );
  }
  return eligibility.patterns.some((pattern) =>
    pattern.endsWith("*")
      ? interfaceName.startsWith(pattern.slice(0, -1))
      : interfaceName === pattern,
  );
}

export function resolveNetworkRateInterfaces(
  rules: VpsRuleValueRecord[],
): NetworkRateInterfaceResolution {
  const eligibility = resolveNetworkInterfaceEligibility(rules);
  if (!eligibility.valid) {
    return invalidResolution(eligibility, "network.rate.interfaces");
  }
  const rateRule = rules.find(
    (rule) => rule.key === "network.rate.interfaces",
  );
  if (!rateRule) {
    return exactResolution(eligibility, "network.rate.interfaces", new Set());
  }
  if (rateRule.state !== "ok") {
    return invalidResolution(eligibility, "network.rate.interfaces");
  }
  const rateValue = jsonObject(rateRule.value_json);
  const mode = typeof rateValue?.mode === "string" ? rateValue.mode : null;
  if (mode === "all") {
    return allResolution(eligibility, "network.rate.interfaces");
  }
  if (mode === "exact") {
    const interfaces = selectorInterfaces(rateValue?.selectors, "reject", false);
    if (interfaces === null) {
      return invalidResolution(eligibility, "network.rate.interfaces");
    }
    return exactResolution(
      eligibility,
      "network.rate.interfaces",
      eligibleHostInterfaces(interfaces, eligibility),
    );
  }
  if (
    mode !== "reference" ||
    jsonObject(rateValue?.reference)?.rule !== "traffic.selectors"
  ) {
    return invalidResolution(eligibility, "network.rate.interfaces");
  }
  return resolveTrafficSelectorReference(rules, eligibility);
}

function resolveTrafficSelectorReference(
  rules: VpsRuleValueRecord[],
  eligibility: NetworkInterfaceEligibility,
): NetworkRateInterfaceResolution {
  const trafficRule = rules.find((rule) => rule.key === "traffic.selectors");
  if (!trafficRule) {
    return exactResolution(eligibility, "traffic.selectors", new Set());
  }
  if (trafficRule.state !== "ok") {
    return invalidResolution(eligibility, "traffic.selectors");
  }
  const trafficValue = jsonObject(trafficRule.value_json);
  if (trafficValue?.mode === "all") {
    return allResolution(eligibility, "traffic.selectors");
  }
  if (trafficValue?.mode !== "exact") {
    return invalidResolution(eligibility, "traffic.selectors");
  }
  const interfaces = selectorInterfaces(trafficValue.selectors, "ignore", true);
  if (interfaces === null) {
    return invalidResolution(eligibility, "traffic.selectors");
  }
  return exactResolution(
    eligibility,
    "traffic.selectors",
    eligibleHostInterfaces(interfaces, eligibility),
  );
}

export function selectedNetworkRates(
  rates: TelemetryNetworkRateRecord[],
  rules: VpsRuleValueRecord[],
): TelemetryNetworkRateRecord[] {
  const resolution = resolveNetworkRateInterfaces(rules);
  if (!resolution.valid) return [];
  return rates.filter(
    (rate) =>
      networkInterfaceIsEligible(resolution.eligibility, "host", rate.interface) &&
      (resolution.mode === "all" || resolution.interfaces.has(rate.interface)),
  );
}

export function networkRateSelectionLabel(
  rules: VpsRuleValueRecord[],
): string {
  const resolution = resolveNetworkRateInterfaces(rules);
  if (!resolution.valid) return "Live-rate interface rule unavailable";
  if (resolution.mode === "all") return "All eligible interfaces";
  if (resolution.interfaces.size === 0) return "No live-rate interfaces selected";
  const names = Array.from(resolution.interfaces).sort((left, right) =>
    left.localeCompare(right),
  );
  return resolution.source === "traffic.selectors"
    ? `${names.join(", ")} · referenced from traffic.selectors`
    : names.join(", ");
}

function allResolution(
  eligibility: NetworkInterfaceEligibility,
  source: NetworkRateInterfaceResolution["source"],
): NetworkRateInterfaceResolution {
  return { eligibility, interfaces: new Set(), mode: "all", source, valid: true };
}

function exactResolution(
  eligibility: NetworkInterfaceEligibility,
  source: NetworkRateInterfaceResolution["source"],
  interfaces: Set<string>,
): NetworkRateInterfaceResolution {
  return { eligibility, interfaces, mode: "exact", source, valid: true };
}

function invalidResolution(
  eligibility: NetworkInterfaceEligibility,
  source: NetworkRateInterfaceResolution["source"],
): NetworkRateInterfaceResolution {
  return { eligibility, interfaces: new Set(), mode: "exact", source, valid: false };
}

function eligibleHostInterfaces(
  interfaces: Set<string>,
  eligibility: NetworkInterfaceEligibility,
): Set<string> {
  return new Set(
    [...interfaces].filter((interfaceName) =>
      networkInterfaceIsEligible(eligibility, "host", interfaceName),
    ),
  );
}

function validInterfacePatterns(value: unknown): value is string[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > 16) return false;
  const seen = new Set<string>();
  return value.every((pattern) => {
    if (
      typeof pattern !== "string" ||
      pattern.length === 0 ||
      new TextEncoder().encode(pattern).length > 128 ||
      /[,+:\s\u0000-\u001f\u007f-\u009f]/u.test(pattern) ||
      (pattern.includes("*") &&
        (!pattern.endsWith("*") || pattern.indexOf("*") !== pattern.length - 1)) ||
      seen.has(pattern)
    ) {
      return false;
    }
    seen.add(pattern);
    return true;
  });
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
      /[,*+:\s\u0000-\u001f\u007f-\u009f]/u.test(interfaceName) ||
      (direction !== "rx" && direction !== "tx" && direction !== "total" && direction !== "tx/rx") ||
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
