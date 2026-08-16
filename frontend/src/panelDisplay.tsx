import { createContext, useCallback, useContext, type ReactNode } from "react";
import type { OperatorPreferences } from "./types";
import { DEFAULT_OPERATOR_PREFERENCES, type VpsNameDisplayMode } from "./utils";
import {
  formatByteCount,
  formatByteRateFromBitsPerSecond,
  type ByteUnitDisplayMode,
} from "./telemetryMetrics";

type PanelDisplaySettings = {
  preferences: OperatorPreferences;
  preferencesError: string | null;
  preferencesSaving: boolean;
  vpsNameDisplayMode: VpsNameDisplayMode;
  updatePreferences: (preferences: OperatorPreferences) => Promise<void>;
  setVpsNameDisplayMode: (mode: VpsNameDisplayMode) => void;
};

export type ByteCountFormatter = (value: number | null | undefined) => string;
export type ByteRateFormatter = (value: number | null | undefined) => string;

const fallbackSettings: PanelDisplaySettings = {
  preferences: DEFAULT_OPERATOR_PREFERENCES,
  preferencesError: null,
  preferencesSaving: false,
  vpsNameDisplayMode: DEFAULT_OPERATOR_PREFERENCES.vps_name_display_mode,
  updatePreferences: async () => undefined,
  setVpsNameDisplayMode: () => undefined,
};

const PanelDisplayContext =
  createContext<PanelDisplaySettings>(fallbackSettings);

export function PanelDisplayProvider({
  children,
  value,
}: {
  children: ReactNode;
  value: PanelDisplaySettings;
}) {
  return (
    <PanelDisplayContext.Provider value={value}>
      {children}
    </PanelDisplayContext.Provider>
  );
}

export function usePanelDisplaySettings(): PanelDisplaySettings {
  return useContext(PanelDisplayContext);
}

export function useByteUnitDisplayMode(): ByteUnitDisplayMode {
  return usePanelDisplaySettings().preferences.byte_unit_display_mode;
}

export function useByteCountFormatter(): ByteCountFormatter {
  const mode = useByteUnitDisplayMode();
  return useCallback(
    (value: number | null | undefined) => formatByteCount(value, mode),
    [mode],
  );
}

export function useByteRateFormatter(): ByteRateFormatter {
  const mode = useByteUnitDisplayMode();
  return useCallback(
    (value: number | null | undefined) =>
      formatByteRateFromBitsPerSecond(value, mode),
    [mode],
  );
}
