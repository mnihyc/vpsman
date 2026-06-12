import { useCallback, useState } from "react";
import { apiGet, apiPost, apiPut, isApiUnauthorized } from "../api";
import type {
  SuiteConfigResponse,
  SuiteConfigUpdateResponse,
  SuiteConfigValidateResponse,
  SystemDashboardRecord,
} from "../types";

export type SystemDashboardWindow = "15m" | "1h" | "6h" | "24h" | "7d" | "30d";
export type SystemDashboardPointDensity = "compact" | "balanced" | "dense";

export function useSystemData(apiToken: string, onUnauthorized: () => void) {
  const [systemDashboard, setSystemDashboard] = useState<SystemDashboardRecord | null>(null);
  const [systemDashboardLoading, setSystemDashboardLoading] = useState(false);
  const [systemDashboardError, setSystemDashboardError] = useState<string | null>(null);
  const [systemDashboardWindow, setSystemDashboardWindow] = useState<SystemDashboardWindow>("24h");
  const [systemDashboardPointDensity, setSystemDashboardPointDensity] = useState<SystemDashboardPointDensity>("balanced");
  const [suiteConfig, setSuiteConfig] = useState<SuiteConfigResponse | null>(null);
  const [suiteConfigLoading, setSuiteConfigLoading] = useState(false);
  const [suiteConfigError, setSuiteConfigError] = useState<string | null>(null);

  const loadSystemDashboard = useCallback(
    async (
      nextWindow = systemDashboardWindow,
      nextDensity = systemDashboardPointDensity,
    ) => {
      setSystemDashboardLoading(true);
      try {
        const params = new URLSearchParams({
          chart_points: String(systemChartPoints(nextDensity)),
          window: nextWindow,
        });
        const record = await apiGet<SystemDashboardRecord>(`/api/v1/system/dashboard?${params.toString()}`, apiToken);
        setSystemDashboard(record);
        setSystemDashboardError(null);
      } catch (error) {
        handleSystemError(error, onUnauthorized, setSystemDashboardError, "System dashboard unavailable");
      } finally {
        setSystemDashboardLoading(false);
      }
    },
    [apiToken, onUnauthorized, systemDashboardPointDensity, systemDashboardWindow],
  );

  const loadSuiteConfig = useCallback(async () => {
    setSuiteConfigLoading(true);
    try {
      const record = await apiGet<SuiteConfigResponse>("/api/v1/admin/suite-config", apiToken);
      setSuiteConfig(record);
      setSuiteConfigError(null);
    } catch (error) {
      handleSystemError(error, onUnauthorized, setSuiteConfigError, "Suite config unavailable");
    } finally {
      setSuiteConfigLoading(false);
    }
  }, [apiToken, onUnauthorized]);

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

  return {
    loadSuiteConfig,
    loadSystemDashboard,
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
