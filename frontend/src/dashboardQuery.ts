import type {
  DashboardOverviewRecord,
  DashboardPreferences,
  DashboardScopeKind,
  DashboardWindow,
} from "./types";

export const dashboardWindowOptions: readonly DashboardWindow[] = [
  "15m",
  "1h",
  "8h",
  "1d",
  "7d",
  "30d",
  "90d",
  "180d",
  "1y",
  "all",
];

export function dashboardWindowLabel(window: DashboardWindow): string {
  if (window === "15m") return "15m";
  if (window === "all") return "All";
  return window;
}

export function dashboardWindowAccessibleLabel(
  window: DashboardWindow,
): string {
  if (window === "15m") return "Realtime · last 15 minutes";
  if (window === "1h") return "Last 1 hour";
  if (window === "8h") return "Last 8 hours";
  if (window === "1d") return "Last 1 day";
  if (window === "7d") return "Last 7 days";
  if (window === "30d") return "Last 30 days";
  if (window === "90d") return "Last 90 days";
  if (window === "180d") return "Last 180 days";
  if (window === "1y") return "Last 1 year";
  return "All retained time";
}

export function dashboardWindowLongLabel(window: DashboardWindow): string {
  if (window === "15m") return "Realtime · last 15 minutes";
  if (window === "1h") return "1 hour";
  if (window === "8h") return "8 hours";
  if (window === "1d") return "1 day";
  if (window === "7d") return "7 days";
  if (window === "30d") return "30 days";
  if (window === "90d") return "90 days";
  if (window === "180d") return "180 days";
  if (window === "1y") return "1 year";
  return "All time";
}

export function dashboardWindowDurationSeconds(
  window: DashboardWindow,
): number | null {
  if (window === "15m") return 15 * 60;
  if (window === "1h") return 60 * 60;
  if (window === "8h") return 8 * 60 * 60;
  if (window === "1d") return 24 * 60 * 60;
  if (window === "7d") return 7 * 24 * 60 * 60;
  if (window === "30d") return 30 * 24 * 60 * 60;
  if (window === "90d") return 90 * 24 * 60 * 60;
  if (window === "180d") return 180 * 24 * 60 * 60;
  if (window === "1y") return 365 * 24 * 60 * 60;
  return null;
}

export function dashboardScopeLabel(
  preferences: DashboardPreferences,
  overview: DashboardOverviewRecord | null,
): string {
  const value = preferences.scopeValue.trim();
  if (preferences.scopeKind === "all") {
    return "All VPS";
  }
  if (!value) {
    return overview?.scope.label ?? "Selected VPS";
  }
  if (preferences.scopeKind === "provider") {
    return value.startsWith("provider:") ? value : `provider:${value}`;
  }
  if (preferences.scopeKind === "country") {
    return value.startsWith("country:") ? value : `country:${value}`;
  }
  return value;
}

export function dashboardScopeValueOptions(
  kind: DashboardScopeKind,
  overview: DashboardOverviewRecord | null,
) {
  if (!overview) {
    return [];
  }
  if (kind === "provider") {
    return overview.available_filters.providers;
  }
  if (kind === "country") {
    return overview.available_filters.countries;
  }
  if (kind === "tag") {
    return overview.available_filters.tags;
  }
  return [];
}

export function isoToDateTimeLocal(value: string): string {
  if (!value.trim()) {
    return "";
  }
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return "";
  }
  const offsetMs = date.getTimezoneOffset() * 60_000;
  return new Date(date.getTime() - offsetMs).toISOString().slice(0, 16);
}

export function dateTimeLocalToIso(value: string): string {
  if (!value.trim()) {
    return "";
  }
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "" : date.toISOString();
}
