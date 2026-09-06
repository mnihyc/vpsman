import { dashboardChartColors } from "./colorPalette";

export type PingTargetDisplayMetadata = {
  display_color?: string | null;
  display_order?: number | null;
};

export function defaultPingTargetColor(targetId: string): string {
  let hash = 0;
  for (let index = 0; index < targetId.length; index += 1) {
    hash = (hash * 31 + targetId.charCodeAt(index)) >>> 0;
  }
  return dashboardChartColors[hash % dashboardChartColors.length];
}

export function pingTargetColor(
  targetId: string,
  metadata?: PingTargetDisplayMetadata,
): string {
  return metadata?.display_color ?? defaultPingTargetColor(targetId);
}

// Resolve only the shared explicit order. Callers retain their existing
// name/primary ordering when neither target has a saved position.
export function comparePingTargetDisplayOrder(
  left?: PingTargetDisplayMetadata,
  right?: PingTargetDisplayMetadata,
): number {
  const leftOrder = left?.display_order ?? Number.POSITIVE_INFINITY;
  const rightOrder = right?.display_order ?? Number.POSITIVE_INFINITY;
  if (leftOrder === rightOrder) return 0;
  return leftOrder < rightOrder ? -1 : 1;
}
