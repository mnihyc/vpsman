import type { VpsRuleValueRecord } from "./types";

export const VPS_RULE_KEYS = [
  "product.name",
  "billing.price",
  "billing.cycle",
  "network.port_speed",
  "network.rate.interfaces",
  "traffic.reset_day",
  "traffic.quota.total",
  "traffic.quota.rx",
  "traffic.quota.tx",
  "traffic.selectors",
] as const;

export type VpsRuleKey = (typeof VPS_RULE_KEYS)[number];
export type VpsRuleOrderedKind = "billing_price" | "bytes" | "day" | "speed";

export type VpsRuleFieldDefinition = {
  help: string;
  inputMode?: "decimal" | "numeric" | "text";
  key: VpsRuleKey;
  label: string;
  orderedKind?: VpsRuleOrderedKind;
  placeholder: string;
};

export const NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX =
  "[traffic.selectors]";
export const NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_PLACEHOLDER = `Default when unset: ${NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX}`;

export const VPS_RULE_FIELD_DEFINITIONS: readonly VpsRuleFieldDefinition[] = [
  {
    help: "Optional product or plan name shown beside the provider, for example LN.V2.HKGv3 or Storage-Box 4. Case and punctuation are preserved; extra whitespace is removed.",
    inputMode: "text",
    key: "product.name",
    label: "Product name",
    placeholder: "LN.V2.HKGv3",
  },
  {
    help: "Optional card price, for example 29.90 CNY/m, 48 USD/q, 60 €/hy, or 99 USD/y. Use -1 to explicitly disable billing and show -; blank leaves the rule unset.",
    inputMode: "text",
    key: "billing.price",
    label: "Billing price",
    orderedKind: "billing_price",
    placeholder: "29.90 CNY/m",
  },
  {
    help: "Optional renewal anchor, independent of traffic reset day. Use a day for /m (for example 15), or standard MM-DD for /q, /hy, and /y (for example 06-15). M-D shorthand is accepted.",
    inputMode: "text",
    key: "billing.cycle",
    label: "Billing cycle",
    placeholder: "15 or 06-15",
  },
  {
    help: "Optional display-only port speed, for example 400Mbps or 1.5 Gbps. It does not configure shaping, quotas, or the agent network.",
    inputMode: "text",
    key: "network.port_speed",
    label: "Port speed",
    orderedKind: "speed",
    placeholder: "1.5 Gbps",
  },
  {
    help: `Existing traffic-selector syntax for aggregate live rates and charts. By default, an absent or unset rule follows traffic.selectors. Enter ${NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX} to store that reference explicitly, clear a previously edited value or enter [] to select every reported interface, or override with interface selectors such as eth0,eth1. Direction suffixes such as +rx, +tx, or +tx/rx are accepted but ignored here: live speed always keeps separate RX and TX values for every selected interface. Direction suffixes remain meaningful to traffic accounting only.`,
    inputMode: "text",
    key: "network.rate.interfaces",
    label: "Live rate interfaces",
    placeholder: NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_PLACEHOLDER,
  },
  {
    help: "Day of month in UTC when traffic accounting resets. Use -1 to accumulate totals continuously from the earliest retained counter evidence.",
    inputMode: "numeric",
    key: "traffic.reset_day",
    label: "Reset day",
    orderedKind: "day",
    placeholder: "-1 or 14",
  },
  {
    help: "Total traffic quota for the current reset cycle or continuously accumulated total. Type 4TB, 750GB, raw bytes, or -1 for explicitly unlimited. Blank leaves the rule unset.",
    inputMode: "text",
    key: "traffic.quota.total",
    label: "Total quota",
    orderedKind: "bytes",
    placeholder: "4TB",
  },
  {
    help: "Optional receive-side traffic quota. Use -1 for explicitly unlimited; blank leaves the rule unset.",
    inputMode: "text",
    key: "traffic.quota.rx",
    label: "RX quota",
    orderedKind: "bytes",
    placeholder: "500GB",
  },
  {
    help: "Optional transmit-side traffic quota. Use -1 for explicitly unlimited; blank leaves the rule unset.",
    inputMode: "text",
    key: "traffic.quota.tx",
    label: "TX quota",
    orderedKind: "bytes",
    placeholder: "500GB",
  },
  {
    help: "Traffic selectors as comma-separated interface+direction tokens. Bare interfaces total RX + TX; +rx or +tx selects one direction; +tx/rx uses the larger direction. +rx+tx and +tx+rx normalize to the bare-interface total.",
    inputMode: "text",
    key: "traffic.selectors",
    label: "Interfaces / selectors",
    placeholder: "ens3, eth0+tx/rx",
  },
];

const RULE_DEFINITION_BY_KEY = new Map(
  VPS_RULE_FIELD_DEFINITIONS.map((definition) => [definition.key, definition]),
);
const MAX_I64 = 9_223_372_036_854_775_807n;
const MAX_VPS_RULE_VALUE_BYTES = 4_096;
const MAX_PRODUCT_NAME_BYTES = 160;
const MAX_TRAFFIC_INTERFACE_BYTES = 128;
const MAX_TRAFFIC_SELECTOR_ITEMS = 16;

const QUOTA_MULTIPLIERS: Readonly<Record<string, bigint>> = {
  "": 1n,
  b: 1n,
  gb: 1_000_000_000n,
  gib: 1_073_741_824n,
  kb: 1_000n,
  kib: 1_024n,
  mb: 1_000_000n,
  mib: 1_048_576n,
  tb: 1_000_000_000_000n,
  tib: 1_099_511_627_776n,
};

const SPEED_MULTIPLIERS: Readonly<Record<string, bigint>> = {
  bps: 1n,
  gbps: 1_000_000_000n,
  kbps: 1_000n,
  mbps: 1_000_000n,
  tbps: 1_000_000_000_000n,
};

export function isVpsRuleKey(value: string): value is VpsRuleKey {
  return VPS_RULE_KEYS.includes(value as VpsRuleKey);
}

export function vpsRuleDefinition(
  key: string,
): VpsRuleFieldDefinition | undefined {
  return RULE_DEFINITION_BY_KEY.get(key as VpsRuleKey);
}

export function indexVpsRulesByClient(
  rows: readonly VpsRuleValueRecord[],
): ReadonlyMap<string, readonly VpsRuleValueRecord[]> {
  const byClient = new Map<string, VpsRuleValueRecord[]>();
  for (const row of rows) {
    const existing = byClient.get(row.client_id);
    if (existing) {
      existing.push(row);
    } else {
      byClient.set(row.client_id, [row]);
    }
  }
  return byClient;
}

export function productNameFromVpsRules(
  rows: readonly VpsRuleValueRecord[],
  clientId: string,
): string | null {
  return (
    rows.find((row) => row.client_id === clientId && row.key === "product.name")
      ?.value_raw ?? null
  );
}

export function providerProductLabel(
  providerInput: string | null | undefined,
  productInput: string | null | undefined,
  fallback = "",
): string {
  const provider = providerInput?.trim() ?? "";
  const product = productInput?.trim() ?? "";
  if (provider) return product ? `${provider} · ${product}` : provider;
  if (product) return `provider unset · ${product}`;
  return fallback;
}

/**
 * Canonicalizes the operator-facing text shape shared by rule editing and
 * selector equality. Validation remains authoritative on the control plane.
 */
export function normalizeVpsRuleValue(
  key: string,
  input: string | null,
): string {
  const trimmed = input?.trim() ?? "";
  return tryNormalizeVpsRuleValue(key, input) ?? trimmed;
}

export function tryNormalizeVpsRuleValue(
  key: string,
  input: string | null,
): string | null {
  const raw = input ?? "";
  const trimmed = raw.trim();
  if (!isVpsRuleKey(key)) return null;
  if (utf8ByteLength(raw) > MAX_VPS_RULE_VALUE_BYTES) {
    return null;
  }
  if (key === "product.name") {
    const canonical = raw
      .replace(/\p{White_Space}+/gu, " ")
      .replace(/^ +| +$/g, "");
    return canonical &&
      !/[\p{Cc}\p{Cs}]/u.test(canonical) &&
      utf8ByteLength(canonical) <= MAX_PRODUCT_NAME_BYTES
      ? canonical
      : null;
  }
  if (!trimmed) return key === "network.rate.interfaces" ? "[]" : null;
  if (trimmed === "-1") {
    return key === "billing.price" ||
      key === "traffic.reset_day" ||
      key.startsWith("traffic.quota.")
      ? trimmed
      : null;
  }
  if (key === "billing.price") {
    const compact = trimmed.replace(/\s+/g, "");
    if (compact === "-1") return "-1";
    const match = trimmed.match(
      /^(\d+)(?:\.(\d{0,2}))?\s*([^/\s]+)\s*\/\s*(m|q|h|hy|y)$/i,
    );
    if (!match || match[1].length > 9) return null;
    const whole = match[1].replace(/^0+(?=\d)/, "");
    const fraction = (match[2] ?? "").padEnd(2, "0");
    const periodInput = match[4].toLowerCase();
    const period = periodInput === "h" ? "hy" : periodInput;
    const currency = normalizedBillingCurrencyDisplay(match[3]);
    return currency ? `${whole}.${fraction} ${currency}/${period}` : null;
  }
  if (key === "billing.cycle") {
    const match = trimmed.match(/^(\+?\d+)(?:\s*-\s*(\+?\d+))?$/);
    if (!match) return null;
    const first = Number(match[1]);
    const day = match[2] ? Number(match[2]) : first;
    const month = match[2] ? first : null;
    if (day < 1 || day > 31 || (month !== null && (month < 1 || month > 12))) {
      return null;
    }
    if (month !== null) {
      const maximumDay =
        month === 2 ? 29 : [4, 6, 9, 11].includes(month) ? 30 : 31;
      if (day > maximumDay) return null;
    }
    return month === null
      ? String(day)
      : `${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
  }
  if (key === "network.port_speed") {
    return parsePortSpeed(trimmed)?.canonical ?? null;
  }
  if (key.startsWith("traffic.quota.")) {
    return parseTrafficQuota(trimmed)?.canonical ?? null;
  }
  if (key === "traffic.reset_day") {
    if (!/^[+-]?\d+$/.test(trimmed)) return null;
    const day = Number(trimmed);
    return day === -1 || (day >= 1 && day <= 31) ? String(day) : null;
  }
  if (key === "traffic.selectors" || key === "network.rate.interfaces") {
    if (
      key === "network.rate.interfaces" &&
      (trimmed === "[]" ||
        trimmed === NETWORK_RATE_TRAFFIC_SELECTOR_REFERENCE_SYNTAX)
    ) {
      return trimmed;
    }
    return normalizeTrafficSelectorList(
      trimmed,
      key === "network.rate.interfaces",
    );
  }
  return null;
}

function normalizedBillingCurrencyDisplay(input: string): string | null {
  if (input === "￥") return "¥";
  if (input === "$" || input === "¥" || input === "€" || input === "£") {
    return input;
  }
  return /^[A-Za-z]{3}$/.test(input) ? input.toUpperCase() : null;
}

type ParsedTrafficSelector = {
  canonical: string;
  directionMask: number;
  interfaceName: string;
  source: "host" | "tunnel";
};

type ParsedScaledRuleValue = {
  canonical: string;
  value: bigint;
};

export function vpsRuleOrderedIntegerValue(
  key: string,
  input: string,
): bigint | null {
  if (key.startsWith("traffic.quota.")) {
    return parseTrafficQuota(input)?.value ?? null;
  }
  if (key === "network.port_speed") {
    return parsePortSpeed(input)?.value ?? null;
  }
  if (key === "traffic.reset_day") {
    const canonical = tryNormalizeVpsRuleValue(key, input);
    return canonical && canonical !== "-1" ? BigInt(canonical) : null;
  }
  return null;
}

function parseTrafficQuota(input: string): ParsedScaledRuleValue | null {
  const match = input.trim().match(/^(\d+)(?:\.(\d*))?\s*([A-Za-z]*)$/);
  if (!match) return null;
  const whole = match[1];
  const fraction = match[2] ?? "";
  if (![...whole, ...fraction].some((digit) => digit !== "0")) {
    return null;
  }
  const unitInput = match[3].toLowerCase();
  const multiplier = QUOTA_MULTIPLIERS[unitInput];
  if (multiplier === undefined) return null;
  const value = scaledDecimalInteger(whole, fraction, multiplier, true);
  if (value === null || value > MAX_I64) return null;
  const normalizedUnit =
    unitInput === ""
      ? ""
      : unitInput === "b"
        ? "B"
        : unitInput.endsWith("ib")
          ? `${unitInput[0].toUpperCase()}iB`
          : `${unitInput[0].toUpperCase()}B`;
  return {
    canonical: `${normalizeDecimalParts(whole, fraction)}${normalizedUnit}`,
    value,
  };
}

function parsePortSpeed(input: string): ParsedScaledRuleValue | null {
  const match = input
    .trim()
    .match(/^(\d+)(?:\.(\d{0,3}))?\s*(bps|kbps|mbps|gbps|tbps)$/i);
  if (!match) return null;
  const whole = match[1];
  const fraction = match[2] ?? "";
  const unitInput = match[3].toLowerCase();
  const value = scaledDecimalInteger(
    whole,
    fraction,
    SPEED_MULTIPLIERS[unitInput],
    false,
  );
  if (value === null || value <= 0n || value > MAX_I64) return null;
  const unit = unitInput === "bps" ? "bps" : `${unitInput[0].toUpperCase()}bps`;
  return {
    canonical: `${normalizeDecimalParts(whole, fraction)} ${unit}`,
    value,
  };
}

function scaledDecimalInteger(
  whole: string,
  fraction: string,
  multiplier: bigint | undefined,
  roundHalfUp: boolean,
): bigint | null {
  if (multiplier === undefined) return null;
  try {
    const scale = 10n ** BigInt(fraction.length);
    const digits = `${whole}${fraction}`;
    const scaled = BigInt(digits) * multiplier;
    const quotient = scaled / scale;
    const remainder = scaled % scale;
    return roundHalfUp && remainder * 2n >= scale ? quotient + 1n : quotient;
  } catch {
    return null;
  }
}

function normalizeDecimalParts(
  wholeInput: string,
  fractionInput: string,
): string {
  const whole = wholeInput.replace(/^0+(?=\d)/, "");
  const fraction = fractionInput.replace(/0+$/, "");
  return fraction ? `${whole}.${fraction}` : whole;
}

function normalizeTrafficSelectorList(
  input: string,
  hostOnly: boolean,
): string | null {
  const selectors = input.split(",").map(parseTrafficSelectorText);
  if (
    selectors.length > MAX_TRAFFIC_SELECTOR_ITEMS ||
    selectors.some((selector) => !selector)
  ) {
    return null;
  }
  const canonical = new Set<string>();
  const selectedDirections = new Map<string, number>();
  for (const selector of selectors as ParsedTrafficSelector[]) {
    if (hostOnly && selector.source !== "host") return null;
    if (canonical.has(selector.canonical)) return null;
    canonical.add(selector.canonical);
    const identity = `${selector.source}\0${selector.interfaceName}`;
    const selected = selectedDirections.get(identity) ?? 0;
    if ((selected & selector.directionMask) !== 0) return null;
    selectedDirections.set(identity, selected | selector.directionMask);
  }
  return (selectors as ParsedTrafficSelector[])
    .map((selector) => selector.canonical)
    .join(",");
}

function parseTrafficSelectorText(input: string): ParsedTrafficSelector | null {
  const compact = input.trim();
  const sourceSeparator = compact.indexOf(":");
  const hasSource = sourceSeparator >= 0;
  const source = (
    hasSource ? compact.slice(0, sourceSeparator).trim() : "host"
  ).toLowerCase();
  const rest = (
    hasSource ? compact.slice(sourceSeparator + 1) : compact
  ).trim();
  const directionSeparator = rest.indexOf("+");
  const interfaceName = (
    directionSeparator >= 0 ? rest.slice(0, directionSeparator) : rest
  ).trim();
  const directionInput =
    directionSeparator >= 0 ? rest.slice(directionSeparator + 1) : undefined;
  const directionToken = (directionInput ?? "total").trim().toLowerCase();
  const direction =
    directionToken === "rx+tx" || directionToken === "tx+rx"
      ? "total"
      : directionToken === "rx/tx" || directionToken === "tx/rx"
        ? "tx/rx"
        : directionToken;
  if (
    !interfaceName ||
    utf8ByteLength(interfaceName) > MAX_TRAFFIC_INTERFACE_BYTES ||
    /[,+:\s\p{Cc}]/u.test(interfaceName) ||
    !new Set(["host", "tunnel"]).has(source) ||
    !new Set(["rx", "tx", "total", "tx/rx"]).has(direction)
  ) {
    return null;
  }
  const sourcePrefix = source === "host" ? "" : `${source}:`;
  const directionSuffix = direction === "total" ? "" : `+${direction}`;
  return {
    canonical: `${sourcePrefix}${interfaceName}${directionSuffix}`,
    directionMask: direction === "rx" ? 1 : direction === "tx" ? 2 : 3,
    interfaceName,
    source: source as "host" | "tunnel",
  };
}

function utf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}
