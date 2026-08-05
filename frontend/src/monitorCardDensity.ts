import { useEffect } from "react";
import { useHistoryEntryState } from "./historyEntryState";

export type MonitorCardDensity = "compact" | "comfortable";

export const OPERATOR_MONITOR_DENSITY_STORAGE_KEY =
  "vpsman.monitoring.operator-card-density";
export const PUBLIC_MONITOR_DENSITY_STORAGE_KEY =
  "vpsman.monitoring.public-card-density";

export function usePersistentMonitorCardDensity(
  historySlot: string,
  storageKey: string,
): [MonitorCardDensity, (density: MonitorCardDensity) => void] {
  const [density, setDensity] = useHistoryEntryState<MonitorCardDensity>(
    `${historySlot}.density`,
    () => readMonitorCardDensity(storageKey),
  );

  useEffect(() => {
    try {
      window.localStorage.setItem(storageKey, density);
    } catch (error) {
      console.warn("Monitor card density could not be persisted", error);
    }
  }, [density, storageKey]);

  return [density, setDensity];
}

function readMonitorCardDensity(storageKey: string): MonitorCardDensity {
  try {
    const stored = window.localStorage.getItem(storageKey);
    return stored === "compact" || stored === "comfortable"
      ? stored
      : "compact";
  } catch (error) {
    console.warn("Monitor card density could not be read", error);
    return "compact";
  }
}
