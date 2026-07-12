import type {
  DashboardOverviewRecord,
  DashboardPreferences,
  DashboardScopeKind,
} from "./types";

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
