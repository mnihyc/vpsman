import type { PortForwardMapping, PortRange } from "./types";

const MAX_MAPPING_ITEMS = 256;

export function parsePortExpression(expression: string, label = "Port"): PortRange[] {
  const normalized = expression.trim();
  if (!normalized) throw new Error("Enter at least one port or range");
  const ranges = normalized.split(",").map((raw) => {
    const item = raw.trim();
    if (!item) throw new Error("Remove the empty port item");
    const fields = item.split("-");
    if (fields.length > 2) throw new Error(`Invalid port item: ${item}`);
    const start = parsePort(fields[0] ?? "", item);
    const end = fields.length === 2 ? parsePort(fields[1] ?? "", item) : start;
    if (start > end) throw new Error(`Range start exceeds its end: ${item}`);
    return { start, end };
  });
  if (ranges.length > MAX_MAPPING_ITEMS) {
    throw new Error(`Use no more than ${MAX_MAPPING_ITEMS} mapping items`);
  }
  const sorted = [...ranges].sort((left, right) => left.start - right.start || left.end - right.end);
  for (let index = 1; index < sorted.length; index += 1) {
    if (sorted[index]!.start <= sorted[index - 1]!.end) {
      throw new Error(`${label} ranges overlap`);
    }
  }
  return ranges;
}

export function pairPortExpressions(
  incomingExpression: string,
  targetExpression: string,
): PortForwardMapping[] {
  const incoming = parsePortExpression(incomingExpression, "Incoming port");
  const targets = parsePortExpression(targetExpression, "Target port");
  const mappings =
    targets.length === 1 && targets[0]!.start === targets[0]!.end
      ? incoming.map((range) => ({ incoming: range, target: targets[0]! }))
      : incoming.map((range, index) => {
          if (targets.length !== incoming.length) {
            throw new Error(
              "Target must be one port or contain one item for every incoming item",
            );
          }
          return { incoming: range, target: targets[index]! };
        });
  for (const mapping of mappings) {
    const incomingSize = mapping.incoming.end - mapping.incoming.start + 1;
    const targetSize = mapping.target.end - mapping.target.start + 1;
    if (targetSize !== 1 && targetSize !== incomingSize) {
      throw new Error(
        "Each target range must be one port or match its incoming range size",
      );
    }
  }
  return mappings;
}

export function formatPortRange(range: PortRange): string {
  return range.start === range.end
    ? String(range.start)
    : `${range.start}-${range.end}`;
}

export function formatPortMappings(mappings: PortForwardMapping[]): string {
  return mappings
    .map(
      (mapping) =>
        `${formatPortRange(mapping.incoming)} -> ${formatPortRange(mapping.target)}`,
    )
    .join(", ");
}

export function mappingsToExpressions(mappings: PortForwardMapping[]): {
  incoming: string;
  target: string;
} {
  return {
    incoming: mappings.map((mapping) => formatPortRange(mapping.incoming)).join(","),
    target: mappings.map((mapping) => formatPortRange(mapping.target)).join(","),
  };
}

function parsePort(value: string, item: string): number {
  if (!/^\d+$/.test(value.trim())) throw new Error(`Invalid port item: ${item}`);
  const port = Number(value);
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    throw new Error(`Port must be between 1 and 65535: ${item}`);
  }
  return port;
}
