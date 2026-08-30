import { useCallback, useRef, useState } from "react";
import { apiGet, isApiUnauthorized, LatestReadConsumer } from "../api";
import { DEFAULT_MONITORING_REFRESH_INTERVAL_SECS } from "../constants";
import { dashboardWindowOptions } from "../dashboardQuery";
import type { SnapshotSource } from "../homeSnapshot";
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
  refreshIntervalSecs: DEFAULT_MONITORING_REFRESH_INTERVAL_SECS,
  resourceMetric: "cpu_load",
  scopeKind: "all",
  scopeValue: "",
  startAt: "",
  trafficSort: "total",
  window: "1d",
};

export function useDashboardOverviewData(
  apiToken: string,
  onUnauthorized: () => void,
) {
  const [dashboardOverview, setDashboardOverview] =
    useState<DashboardOverviewRecord | null>(null);
  const [dashboardPreferences, setDashboardPreferencesState] = useState(
    readDashboardPreferences,
  );
  const [dashboardOverviewLoading, setDashboardOverviewLoading] =
    useState(false);
  const [dashboardOverviewError, setDashboardOverviewError] = useState<
    string | null
  >(null);
  const dashboardPreferencesRef = useRef(dashboardPreferences);
  const dashboardOverviewRef = useRef<DashboardOverviewRecord | null>(null);
  const desiredRequestKey = useRef(
    dashboardPreferencesToParams(dashboardPreferences).toString(),
  );
  const loadSequence = useRef(0);
  const loadConsumer = useRef(new LatestReadConsumer());
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const loadDashboardOverview = useCallback(
    (nextPreferences?: DashboardPreferences): Promise<void> => {
      if (currentApiToken.current !== apiToken) {
        return Promise.resolve();
      }
      const requestPreferences =
        nextPreferences ?? dashboardPreferencesRef.current;
      const sequence = loadSequence.current + 1;
      loadSequence.current = sequence;
      const requestKey =
        dashboardPreferencesToParams(requestPreferences).toString();
      desiredRequestKey.current = requestKey;
      // Polling, WebSocket invalidations, and preference changes are producers
      // for one browser-local read consumer. While a read is active, retain
      // exactly the latest desired request; completion drains it immediately.
      // Generation fences below still own publication, so a superseded result
      // can neither overwrite the latest view nor suppress its trailing read.
      setDashboardOverviewLoading(true);
      return loadConsumer.current.enqueue(async () => {
        try {
          const overview = await apiGet<DashboardOverviewRecord>(
            `/api/v1/dashboard/overview?${requestKey}`,
            apiToken,
          );
          if (
            sequence !== loadSequence.current ||
            requestKey !== desiredRequestKey.current ||
            currentApiToken.current !== apiToken
          ) {
            return;
          }
          dashboardOverviewRef.current = overview;
          setDashboardOverview(overview);
          setDashboardOverviewError(null);
        } catch (error) {
          if (
            sequence !== loadSequence.current ||
            currentApiToken.current !== apiToken
          ) {
            return;
          }
          if (isApiUnauthorized(error)) {
            onUnauthorized();
            setDashboardOverview(null);
            setDashboardOverviewError("Operator login required");
            return;
          }
          setDashboardOverviewError(
            error instanceof Error
              ? error.message
              : "Dashboard overview unavailable",
          );
        } finally {
          if (sequence === loadSequence.current) {
            setDashboardOverviewLoading(false);
          }
        }
      });
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

  const beginHomeDashboardOverviewHydration = useCallback(() => {
    setDashboardOverviewLoading(true);
    return ++loadSequence.current;
  }, []);

  const hydrateHomeDashboardOverview = useCallback(
    (sequence: number, source: SnapshotSource<DashboardOverviewRecord>) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (loadSequence.current !== sequence) {
        return;
      }
      if (source.data !== null && source.error === null) {
        dashboardOverviewRef.current = source.data;
        setDashboardOverview(source.data);
        setDashboardOverviewError(null);
      } else {
        setDashboardOverviewError(
          `Dashboard overview: ${source.error ?? "snapshot source unavailable"}`,
        );
      }
      setDashboardOverviewLoading(false);
    },
    [apiToken],
  );

  const updateDashboardPreferences = useCallback(
    (patch: Partial<DashboardPreferences>) => {
      const currentPreferences = dashboardPreferencesRef.current;
      const nextPreferences = normalizeDashboardPreferences({
        ...currentPreferences,
        ...patch,
      });
      writeDashboardPreferences(nextPreferences);
      dashboardPreferencesRef.current = nextPreferences;
      setDashboardPreferencesState(nextPreferences);
      if (
        dashboardPreferencesToParams(nextPreferences).toString() !==
        dashboardPreferencesToParams(currentPreferences).toString()
      ) {
        void loadDashboardOverview(nextPreferences);
      }
    },
    [loadDashboardOverview],
  );

  const clearDashboardOverview = useCallback(() => {
    loadSequence.current += 1;
    loadConsumer.current.discardPending();
    currentApiToken.current = "";
    dashboardOverviewRef.current = null;
    setDashboardOverview(null);
    setDashboardOverviewError(null);
    setDashboardOverviewLoading(false);
  }, []);

  return {
    clearDashboardOverview,
    beginHomeDashboardOverviewHydration,
    dashboardOverview,
    dashboardOverviewError,
    dashboardOverviewLoading,
    dashboardOverviewWindow: dashboardPreferences.window,
    dashboardPreferences,
    hydrateHomeDashboardOverview,
    loadDashboardOverview,
    setDashboardOverviewWindow,
    updateDashboardPreferences,
  };
}

export function dashboardPreferencesToParams(
  preferences: DashboardPreferences,
): URLSearchParams {
  const scoped =
    preferences.scopeKind !== "all" && preferences.scopeValue.trim().length > 0;
  const params = new URLSearchParams({
    group_by: preferences.groupBy,
    resource_metric: preferences.resourceMetric,
    scope_kind: scoped ? preferences.scopeKind : "all",
    window: preferences.window,
  });
  params.set(
    "chart_points",
    String(dashboardChartPoints(preferences.pointDensity)),
  );
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
  // Stable density profiles keep identical dashboards on the same bounded
  // server read path across viewport sizes and avoid cache-key fragmentation.
  // The three choices remain distinct presentation densities.
  return pointDensity === "compact"
    ? 120
    : pointDensity === "dense"
      ? 480
      : 240;
}

function readDashboardPreferences(): DashboardPreferences {
  if (typeof window === "undefined") {
    return defaultDashboardPreferences;
  }
  let stored = defaultDashboardPreferences;
  try {
    const raw = window.localStorage.getItem(DASHBOARD_PREFERENCES_STORAGE_KEY);
    if (raw) {
      stored = normalizeDashboardPreferences(
        JSON.parse(raw) as Partial<DashboardPreferences>,
      );
    }
  } catch {
    stored = defaultDashboardPreferences;
  }
  return dashboardPreferencesFromLocation(stored);
}

function dashboardPreferencesFromLocation(
  stored: DashboardPreferences,
): DashboardPreferences {
  const params = new URLSearchParams(window.location.search);
  const next = { ...stored };
  const sharedWindow = params.get("window");
  const scopeKind = params.get("scope_kind");
  const groupBy = params.get("group_by");
  const resourceMetric = params.get("resource_metric");
  const networkView = params.get("network_view");
  const trafficSort = params.get("traffic_sort");
  if (isDashboardWindow(sharedWindow)) next.window = sharedWindow;
  if (isDashboardScopeKind(scopeKind)) next.scopeKind = scopeKind;
  if (isDashboardGroupBy(groupBy)) next.groupBy = groupBy;
  if (isDashboardResourceMetric(resourceMetric)) {
    next.resourceMetric = resourceMetric;
  }
  if (isDashboardNetworkViewMode(networkView)) {
    next.networkView = networkView;
  }
  if (isDashboardTrafficSort(trafficSort)) next.trafficSort = trafficSort;
  if (params.has("scope_value")) {
    next.scopeValue = params.get("scope_value") ?? "";
  }
  if (params.has("start_at")) next.startAt = params.get("start_at") ?? "";
  if (params.has("end_at")) next.endAt = params.get("end_at") ?? "";
  return normalizeDashboardPreferences(next);
}

function writeDashboardPreferences(preferences: DashboardPreferences) {
  if (typeof window === "undefined") {
    return;
  }
  try {
    window.localStorage.setItem(
      DASHBOARD_PREFERENCES_STORAGE_KEY,
      JSON.stringify(preferences),
    );
  } catch {
    // Best-effort local dashboard preference only.
  }
}

function normalizeDashboardPreferences(
  value: Partial<DashboardPreferences>,
): DashboardPreferences {
  const scopeKind = isDashboardScopeKind(value.scopeKind)
    ? value.scopeKind
    : defaultDashboardPreferences.scopeKind;
  return {
    endAt: typeof value.endAt === "string" ? value.endAt : "",
    groupBy: isDashboardGroupBy(value.groupBy)
      ? value.groupBy
      : defaultDashboardPreferences.groupBy,
    networkView: isDashboardNetworkViewMode(value.networkView)
      ? value.networkView
      : defaultDashboardPreferences.networkView,
    pointDensity: isDashboardPointDensity(value.pointDensity)
      ? value.pointDensity
      : defaultDashboardPreferences.pointDensity,
    refreshIntervalSecs: normalizeDashboardRefreshInterval(
      value.refreshIntervalSecs,
    ),
    resourceMetric: isDashboardResourceMetric(value.resourceMetric)
      ? value.resourceMetric
      : defaultDashboardPreferences.resourceMetric,
    scopeKind,
    scopeValue:
      scopeKind === "all"
        ? ""
        : typeof value.scopeValue === "string"
          ? value.scopeValue
          : "",
    startAt: typeof value.startAt === "string" ? value.startAt : "",
    trafficSort: isDashboardTrafficSort(value.trafficSort)
      ? value.trafficSort
      : defaultDashboardPreferences.trafficSort,
    window: isDashboardWindow(value.window)
      ? value.window
      : defaultDashboardPreferences.window,
  };
}

function isDashboardWindow(value: unknown): value is DashboardWindow {
  return (
    typeof value === "string" &&
    dashboardWindowOptions.includes(value as DashboardWindow)
  );
}

function isDashboardGroupBy(value: unknown): value is DashboardGroupBy {
  return (
    typeof value === "string" &&
    [
      "labels",
      "tags",
      "countries",
      "providers",
      "clients",
      "status",
      "date",
    ].includes(value)
  );
}

function isDashboardScopeKind(value: unknown): value is DashboardScopeKind {
  return (
    typeof value === "string" &&
    ["all", "tag", "country", "provider", "client"].includes(value)
  );
}

function isDashboardResourceMetric(
  value: unknown,
): value is DashboardResourceMetric {
  return (
    typeof value === "string" &&
    ["cpu_load", "memory_used", "disk_free"].includes(value)
  );
}

function isDashboardNetworkViewMode(
  value: unknown,
): value is DashboardNetworkViewMode {
  return typeof value === "string" && ["speed", "traffic"].includes(value);
}

function isDashboardPointDensity(
  value: unknown,
): value is DashboardPointDensity {
  return (
    typeof value === "string" &&
    ["compact", "balanced", "dense"].includes(value)
  );
}

function normalizeDashboardRefreshInterval(
  value: unknown,
): DashboardRefreshIntervalSecs {
  const numeric =
    typeof value === "number"
      ? value
      : typeof value === "string"
        ? Number(value)
        : NaN;
  return numeric === 5 || numeric === 15 || numeric === 30 || numeric === 60
    ? (numeric as DashboardRefreshIntervalSecs)
    : defaultDashboardPreferences.refreshIntervalSecs;
}

function isDashboardTrafficSort(value: unknown): value is DashboardTrafficSort {
  return typeof value === "string" && ["total", "rx", "tx"].includes(value);
}
