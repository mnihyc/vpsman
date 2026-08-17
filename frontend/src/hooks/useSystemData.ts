import { useCallback, useRef, useState } from "react";
import { apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import {
  snapshotSourceAvailable,
  snapshotSourceError,
  type SnapshotSource,
} from "../homeSnapshot";
import type {
  SuiteConfigResponse,
  SuiteConfigUpdateResponse,
  SuiteConfigValidateResponse,
  SystemDashboardRecord,
  DashboardWindow,
} from "../types";

export type SystemDashboardWindow = DashboardWindow;
export type SystemDashboardPointDensity = "compact" | "balanced" | "dense";

export function useSystemData(apiToken: string, onUnauthorized: () => void) {
  const [systemDashboard, setSystemDashboard] = useState<SystemDashboardRecord | null>(null);
  const [systemDashboardLoading, setSystemDashboardLoading] = useState(false);
  const [systemDashboardError, setSystemDashboardError] = useState<string | null>(null);
  const [systemDashboardWindow, setSystemDashboardWindow] = useState<SystemDashboardWindow>("1d");
  const [systemDashboardPointDensity, setSystemDashboardPointDensity] = useState<SystemDashboardPointDensity>("balanced");
  const [suiteConfig, setSuiteConfig] = useState<SuiteConfigResponse | null>(null);
  const [suiteConfigLoading, setSuiteConfigLoading] = useState(false);
  const [suiteConfigError, setSuiteConfigError] = useState<string | null>(null);
  const systemDashboardLoadGeneration = useRef(0);
  const suiteConfigLoadGeneration = useRef(0);
  const currentApiToken = useRef(apiToken);
  currentApiToken.current = apiToken;

  const loadSystemDashboard = useCallback(
    async (
      nextWindow = systemDashboardWindow,
      nextDensity = systemDashboardPointDensity,
    ) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      const generation = systemDashboardLoadGeneration.current + 1;
      systemDashboardLoadGeneration.current = generation;
      setSystemDashboardLoading(true);
      try {
        const params = new URLSearchParams({
          chart_points: String(systemChartPoints(nextDensity)),
          window: nextWindow,
        });
        const record = await apiGet<SystemDashboardRecord>(`/api/v1/system/dashboard?${params.toString()}`, apiToken);
        if (
          systemDashboardLoadGeneration.current !== generation ||
          currentApiToken.current !== apiToken
        ) {
          return;
        }
        setSystemDashboard(record);
        setSystemDashboardError(null);
      } catch (error) {
        if (
          systemDashboardLoadGeneration.current !== generation ||
          currentApiToken.current !== apiToken
        ) {
          return;
        }
        handleSystemError(error, onUnauthorized, setSystemDashboardError, "System overview unavailable");
      } finally {
        if (
          systemDashboardLoadGeneration.current === generation &&
          currentApiToken.current === apiToken
        ) {
          setSystemDashboardLoading(false);
        }
      }
    },
    [apiToken, onUnauthorized, systemDashboardPointDensity, systemDashboardWindow],
  );

  const loadSuiteConfig = useCallback(async () => {
    if (currentApiToken.current !== apiToken) {
      return;
    }
    const generation = suiteConfigLoadGeneration.current + 1;
    suiteConfigLoadGeneration.current = generation;
    setSuiteConfigLoading(true);
    try {
      const record = await apiGet<SuiteConfigResponse>("/api/v1/admin/suite-config", apiToken);
      if (
        suiteConfigLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      setSuiteConfig(record);
      setSuiteConfigError(null);
    } catch (error) {
      if (
        suiteConfigLoadGeneration.current !== generation ||
        currentApiToken.current !== apiToken
      ) {
        return;
      }
      handleSystemError(error, onUnauthorized, setSuiteConfigError, "Suite config unavailable");
    } finally {
      if (
        suiteConfigLoadGeneration.current === generation &&
        currentApiToken.current === apiToken
      ) {
        setSuiteConfigLoading(false);
      }
    }
  }, [apiToken, onUnauthorized]);

  const beginHomeSystemDashboardHydration = useCallback(
    () => {
      setSystemDashboardLoading(true);
      return ++systemDashboardLoadGeneration.current;
    },
    [],
  );

  const hydrateHomeSystemDashboard = useCallback(
    (generation: number, source: SnapshotSource<SystemDashboardRecord>) => {
      if (currentApiToken.current !== apiToken) {
        return;
      }
      if (systemDashboardLoadGeneration.current !== generation) {
        return;
      }
      if (snapshotSourceAvailable(source)) {
        setSystemDashboard(source.data);
      }
      setSystemDashboardError(snapshotSourceError("System overview", source));
      setSystemDashboardLoading(false);
    },
    [apiToken],
  );

  const validateSuiteConfig = useCallback(
    async (toml: string) =>
      apiPost<SuiteConfigValidateResponse>("/api/v1/admin/suite-config/validate", apiToken, { toml }),
    [apiToken],
  );

  const updateSuiteConfig = useCallback(
    async (toml: string, privilegeAssertion: unknown) => {
      const response = await apiPut<SuiteConfigUpdateResponse>("/api/v1/admin/suite-config", apiToken, {
        confirmed: true,
        privilege_assertion: privilegeAssertion,
        toml,
      });
      if (currentApiToken.current !== apiToken) {
        return response;
      }
      await loadSuiteConfig();
      return response;
    },
    [apiToken, loadSuiteConfig],
  );

  const setSystemDashboardWindowAndReload = useCallback(
    (window: SystemDashboardWindow) => {
      setSystemDashboardWindow(window);
      void loadSystemDashboard(window, systemDashboardPointDensity);
    },
    [loadSystemDashboard, systemDashboardPointDensity],
  );

  const setSystemDashboardPointDensityAndReload = useCallback(
    (density: SystemDashboardPointDensity) => {
      setSystemDashboardPointDensity(density);
      void loadSystemDashboard(systemDashboardWindow, density);
    },
    [loadSystemDashboard, systemDashboardWindow],
  );

  const clearSystem = useCallback(() => {
    systemDashboardLoadGeneration.current += 1;
    suiteConfigLoadGeneration.current += 1;
    currentApiToken.current = "";
    setSystemDashboard(null);
    setSystemDashboardLoading(false);
    setSystemDashboardError(null);
    setSuiteConfig(null);
    setSuiteConfigLoading(false);
    setSuiteConfigError(null);
  }, []);

  return {
    clearSystem,
    beginHomeSystemDashboardHydration,
    loadSuiteConfig,
    loadSystemDashboard,
    hydrateHomeSystemDashboard,
    setSystemDashboardPointDensity: setSystemDashboardPointDensityAndReload,
    setSystemDashboardWindow: setSystemDashboardWindowAndReload,
    suiteConfig,
    suiteConfigError,
    suiteConfigLoading,
    systemDashboard,
    systemDashboardError,
    systemDashboardLoading,
    systemDashboardPointDensity,
    systemDashboardWindow,
    updateSuiteConfig,
    validateSuiteConfig,
  };
}

function systemChartPoints(density: SystemDashboardPointDensity): number {
  switch (density) {
    case "compact":
      return 120;
    case "dense":
      return 720;
    default:
      return 240;
  }
}

function handleSystemError(
  error: unknown,
  onUnauthorized: () => void,
  setError: (message: string | null) => void,
  fallback: string,
) {
  if (isApiUnauthorized(error)) {
    onUnauthorized();
    setError("Operator login required");
    return;
  }
  setError(error instanceof Error ? error.message : fallback);
}
