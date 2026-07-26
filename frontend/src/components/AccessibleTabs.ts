import type { KeyboardEvent } from "react";

/**
 * Adds the WAI-ARIA tab-list keyboard contract to existing button-based tabs.
 * Selection stays owned by each panel's click handler.
 */
export function handleTabListKeyDown(event: KeyboardEvent<HTMLElement>) {
  if (event.altKey || event.ctrlKey || event.metaKey) {
    return;
  }

  const currentTab = (event.target as Element).closest<HTMLElement>(
    '[role="tab"]',
  );
  if (
    !currentTab ||
    currentTab.closest('[role="tablist"]') !== event.currentTarget
  ) {
    return;
  }

  const tabs = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>(
      '[role="tab"]:not([disabled])',
    ),
  ).filter((tab) => tab.closest('[role="tablist"]') === event.currentTarget);
  const currentIndex = tabs.indexOf(currentTab);
  if (currentIndex < 0 || tabs.length === 0) {
    return;
  }

  let nextIndex: number | null = null;
  const vertical =
    event.currentTarget.getAttribute("aria-orientation") === "vertical";
  switch (event.key) {
    case "ArrowLeft":
      if (vertical) return;
      nextIndex = (currentIndex - 1 + tabs.length) % tabs.length;
      break;
    case "ArrowRight":
      if (vertical) return;
      nextIndex = (currentIndex + 1) % tabs.length;
      break;
    case "ArrowUp":
      if (!vertical) return;
      nextIndex = (currentIndex - 1 + tabs.length) % tabs.length;
      break;
    case "ArrowDown":
      if (!vertical) return;
      nextIndex = (currentIndex + 1) % tabs.length;
      break;
    case "Home":
      nextIndex = 0;
      break;
    case "End":
      nextIndex = tabs.length - 1;
      break;
    default:
      return;
  }

  event.preventDefault();
  const nextTab = tabs[nextIndex];
  nextTab?.focus({ preventScroll: true });
  nextTab?.click();
}

export function tabId(namespace: string, value: string) {
  const suffix = value
    .trim()
    .toLocaleLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");
  return `${namespace}-tab-${suffix}`;
}
