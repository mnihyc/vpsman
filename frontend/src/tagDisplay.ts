import type { TagView } from "./types";

export type TagDisplayOrder = Map<string, number>;

export function buildTagDisplayOrder(tags: TagView[]): TagDisplayOrder {
  return new Map(
    tags.map((tag, index) => [
      tag.name,
      Number.isFinite(tag.display_order) ? tag.display_order : index,
    ]),
  );
}

export function displayFleetTags(
  tags: string[],
  tagDisplayOrder: TagDisplayOrder,
  visibilityOverrides: Record<string, boolean>,
): string[] {
  return tags
    .filter((tag) => fleetTagVisible(tag, visibilityOverrides))
    .sort((left, right) =>
      compareTagsByDisplayOrder(left, right, tagDisplayOrder),
    );
}

export function sortTagsByDisplayOrder(
  tags: string[],
  tagDisplayOrder: TagDisplayOrder,
): string[] {
  return tags
    .slice()
    .sort((left, right) =>
      compareTagsByDisplayOrder(left, right, tagDisplayOrder),
    );
}

export function compareTagsByDisplayOrder(
  left: string,
  right: string,
  tagDisplayOrder: TagDisplayOrder,
): number {
  const leftOrder = tagDisplayOrder.get(left) ?? Number.MAX_SAFE_INTEGER;
  const rightOrder = tagDisplayOrder.get(right) ?? Number.MAX_SAFE_INTEGER;
  return leftOrder - rightOrder || left.localeCompare(right);
}

export function fleetTagVisible(
  tag: string,
  visibilityOverrides: Record<string, boolean>,
): boolean {
  const override = visibilityOverrides[tag];
  return typeof override === "boolean" ? override : defaultFleetTagVisible(tag);
}

export function defaultFleetTagVisible(tag: string): boolean {
  return !isCountryTag(tag) && !isProviderTag(tag);
}

export function isCountryTag(tag: string): boolean {
  return /^country[:=_-][a-z0-9_-]{2,32}$/i.test(tag);
}

export function isProviderTag(tag: string): boolean {
  return /^provider[:=_-][a-z0-9_.-]{1,64}$/i.test(tag);
}
