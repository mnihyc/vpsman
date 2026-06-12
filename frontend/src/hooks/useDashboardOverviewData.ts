import { useCallback, useRef, useState } from "react";
import { apiGet, isApiUnauthorized } from "../api";
import type {
  DashboardGroupBy,
  DashboardNetworkViewMode,
  DashboardOverviewRecord,
  DashboardPointDensity,
  DashboardPreferences,
  DashboardRefreshIntervalSecs,
  DashboardResourceMetric,
  DashboardScopeKind,
  DashboardTrafficSort,
  DashboardWindow,
} from "../types";

const DASHBOARD_PREFERENCES_STORAGE_KEY = "vpsman.dashboardPreferences";

const defaultDashboardPreferences: DashboardPreferences = {
  endAt: "",
  groupBy: "labels",
  networkView: "speed",
  pointDensity: "balanced",
  refreshIntervalSecs: 30,
  resourceMetric: "cpu_load",
  scopeKind: "all",
  scopeValue: "",
  startAt: "",
  trafficSort: "total",
  window: "24h",
};

export function useDashboardOverviewData(
  apiToken: string,
  onUnauthorized: () => void,
) {
  const [dashboardOverview, setDashboardOverview] = useState<DashboardOverviewRecord | null>(null);
  const [dashboardPreferences, setDashboardPreferencesState] = useState(readDashboardPreferences);
  const [dashboardOverviewLoading, setDashboardOverviewLoading] = useState(false);
  const [dashboardOverviewError, setDashboardOverviewError] = useState<string | null>(null);
  const dashboardPreferencesRef = useRef(dashboardPreferences);
  const dashboardOverviewRef = useRef<DashboardOverviewRecord | null>(null);
  const desiredRequestKey = useRef(dashboardPreferencesToParams(dashboardPreferences).toString());
  const loadSequence = useRef(0);

  const loadDashboardOverview = useCallback(
    async (nextPreferences?: DashboardPreferences) => {
      const requestPreferences = nextPreferences ?? dashboardPreferencesRef.current;
      const sequence = loadSequence.current + 1;
      loadSequence.current = sequence;
      setDashboardOverviewLoading(true);
      try {
        const params = dashboardPreferencesToParams(requestPreferences);
        const requestKey = params.toString();
        desiredRequestKey.current = requestKey;
        const overview = await apiGet<DashboardOverviewRecord>(`/api/v1/dashboard/overview?${requestKey}`, apiToken);
        if (requestKey !== desiredRequestKey.current) {
          return;
        }
        dashboardOverviewRef.current = overview;
        setDashboardOverview(overview);
        setDashboardOverviewError(null);
      } catch (error) {
        if (sequence !== loadSequence.current) {
          return;
        }
        if (isApiUnauthorized(error)) {
          onUnauthorized();
          setDashboardOverview(null);
          setDashboardOverviewError("Operator login required");
          return;
        }
        setDashboardOverviewError(error instanceof Error ? error.message : "Dashboard overview unavailable");
      } finally {
        if (sequence === loadSequence.current) {
          setDashboardOverviewLoading(false);
        }
      }
    },
    [apiToken, onUnauthorized],
  );

  const setDashboardOverviewWindow = useCallback(
    (nextWindow: DashboardWindow) => {
      const nextPreferences = {
        ...dashboardPreferences,
        endAt: "",
        startAt: "",
        window: nextWindow,
      };
      writeDashboardPreferences(nextPreferences);
      dashboardPreferencesRef.current = nextPreferences;
      setDashboardPreferencesState(nextPreferences);
      void loadDashboardOverview(nextPreferences);
    },
    [dashboardPreferences, loadDashboardOverview],
  );

  const updateDashboardPreferences = useCallback(
    (patch: Partial<DashboardPreferences>) => {
      const nextPreferences = normalizeDashboardPreferences({
        ...dashboardPreferences,
        ...patch,
      });
      writeDashboardPreferences(nextPreferences);
      dashboardPreferencesRef.current = nextPreferences;
      setDashboardPreferencesState(nextPreferences);
      if (
        dashboardPreferencesToParams(nextPreferences).toString() !==
        dashboardPreferencesToParams(dashboardPreferences).toString()
      ) {
        void loadDashboardOverview(nextPreferences);
      }
    },
    [dashboardPreferences, loadDashboardOverview],
  );

  const clearDashboardOverview = useCallback(() => {
    dashboardOverviewRef.current = null;
    setDashboardOverview(null);
    setDashboardOverviewError(null);
  }, []);

  return {
    clearDashboardOverview,
    dashboardOverview,
    dashboardOverviewError,
    dashboardOverviewLoading,
    dashboardOverviewWindow: dashboardPreferences.window,
    dashboardPreferences,
    loadDashboardOverview,
    setDashboardOverviewWindow,
    updateDashboardPreferences,
  };
}

function dashboardPreferencesToParams(preferences: DashboardPreferences): URLSearchParams {
  const scoped = preferences.scopeKind !== "all" && preferences.scopeValue.trim().length > 0;
  const params = new URLSearchParams({
    group_by: preferences.groupBy,
    resource_metric: preferences.resourceMetric,
    scope_kind: scoped ? preferences.scopeKind : "all",
    window: preferences.window,
  });
  params.set("chart_points", String(dashboardChartPoints(preferences.pointDensity)));
  if (scoped) {
    params.set("scope_value", preferences.scopeValue.trim());
  }
  if (preferences.startAt.trim()) {
    params.set("start_at", preferences.startAt.trim());
  }
  if (preferences.endAt.trim()) {
    params.set("end_at", preferences.endAt.trim());
  }
  return params;
}

function dashboardChartPoints(pointDensity: DashboardPointDensity): number {
  const width =
    typeof window === "undefined"
      ? 960
      : Math.max(360, Math.min(1440, Math.floor(window.innerWidth - 420)));
  const pixelsPerPoint =
    pointDensity === "compact"
      ? 5
      : pointDensity === "dense"
        ? 1.5
        : 3;
  return Math.max(60, Math.min(1440, Math.round(width / pixelsPerPoint)));
}

function readDashboardPreferences(): DashboardPreferences {
  if (typeof window === "undefined") {
    return defaultDashboardPreferences;
  }
  try {
    const raw = window.localStorage.getItem(DASHBOARD_PREFERENCES_STORAGE_KEY);
    if (!raw) {
      return defaultDashboardPreferences;
    }
    return normalizeDashboardPreferences(JSON.parse(raw) as Partial<DashboardPreferences>);
  } catch {
    return defaultDashboardPreferences;
  }
}

function writeDashboardPreferences(preferences: DashboardPreferences) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.setItem(DASHBOARD_PREFERENCES_STORAGE_KEY, JSON.stringify(preferences));
  } catch {
    // Best-effort local dashboard preference only.
  }
}

function normalizeDashboardPreferences(value: Partial<DashboardPreferences>): DashboardPreferences {
  const scopeKind = isDashboardScopeKind(value.scopeKind) ? value.scopeKind : defaultDashboardPreferences.scopeKind;
  return {
    endAt: typeof value.endAt === "string" ? value.endAt : "",
    groupBy: isDashboardGroupBy(value.groupBy) ? value.groupBy : defaultDashboardPreferences.groupBy,
    networkView: isDashboardNetworkViewMode(value.networkView)
      ? value.networkView
      : defaultDashboardPreferences.networkView,
    pointDensity: isDashboardPointDensity(value.pointDensity)
      ? value.pointDensity
      : defaultDashboardPreferences.pointDensity,
    refreshIntervalSecs: normalizeDashboardRefreshInterval(value.refreshIntervalSecs),
    resourceMetric: isDashboardResourceMetric(value.resourceMetric)
      ? value.resourceMetric
      : defaultDashboardPreferences.resourceMetric,
    scopeKind,
    scopeValue: scopeKind === "all" ? "" : typeof value.scopeValue === "string" ? value.scopeValue : "",
    startAt: typeof value.startAt === "string" ? value.startAt : "",
    trafficSort: isDashboardTrafficSort(value.trafficSort)
      ? value.trafficSort
      : defaultDashboardPreferences.trafficSort,
    window: isDashboardWindow(value.window) ? value.window : defaultDashboardPreferences.window,
  };
}

function isDashboardWindow(value: unknown): value is DashboardWindow {
  return typeof value === "string" && ["15m", "1h", "6h", "24h", "7d", "14d", "30d", "all"].includes(value);
}

function isDashboardGroupBy(value: unknown): value is DashboardGroupBy {
  return (
    typeof value === "string" &&
    ["labels", "tags", "countries", "providers", "clients", "status", "date"].includes(value)
  );
}

function isDashboardScopeKind(value: unknown): value is DashboardScopeKind {
  return typeof value === "string" && ["all", "tag", "country", "provider", "client"].includes(value);
}

function isDashboardResourceMetric(value: unknown): value is DashboardResourceMetric {
  return typeof value === "string" && ["cpu_load", "memory_used", "disk_free"].includes(value);
}

function isDashboardNetworkViewMode(value: unknown): value is DashboardNetworkViewMode {
  return typeof value === "string" && ["speed", "traffic"].includes(value);
}

function isDashboardPointDensity(value: unknown): value is DashboardPointDensity {
  return typeof value === "string" && ["compact", "balanced", "dense"].includes(value);
}

function normalizeDashboardRefreshInterval(value: unknown): DashboardRefreshIntervalSecs {
  const numeric = typeof value === "number" ? value : typeof value === "string" ? Number(value) : NaN;
  return numeric === 5 || numeric === 30 || numeric === 60
    ? (numeric as DashboardRefreshIntervalSecs)
    : defaultDashboardPreferences.refreshIntervalSecs;
}

function isDashboardTrafficSort(value: unknown): value is DashboardTrafficSort {
  return typeof value === "string" && ["total", "rx", "tx"].includes(value);
}
