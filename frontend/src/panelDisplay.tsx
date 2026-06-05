import { createContext, useContext, type ReactNode } from "react";
import type { OperatorPreferences } from "./types";
import { DEFAULT_OPERATOR_PREFERENCES, type VpsNameDisplayMode } from "./utils";

type PanelDisplaySettings = {
  preferences: OperatorPreferences;
  preferencesError: string | null;
  preferencesSaving: boolean;
  vpsNameDisplayMode: VpsNameDisplayMode;
  updatePreferences: (preferences: OperatorPreferences) => Promise<void>;
  setVpsNameDisplayMode: (mode: VpsNameDisplayMode) => void;
};

const fallbackSettings: PanelDisplaySettings = {
  preferences: DEFAULT_OPERATOR_PREFERENCES,
  preferencesError: null,
  preferencesSaving: false,
  vpsNameDisplayMode: DEFAULT_OPERATOR_PREFERENCES.vps_name_display_mode,
  updatePreferences: async () => undefined,
  setVpsNameDisplayMode: () => undefined,
};

const PanelDisplayContext = createContext<PanelDisplaySettings>(fallbackSettings);

export function PanelDisplayProvider({
  children,
  value,
}: {
  children: ReactNode;
  value: PanelDisplaySettings;
}) {
  return <PanelDisplayContext.Provider value={value}>{children}</PanelDisplayContext.Provider>;
}

export function usePanelDisplaySettings(): PanelDisplaySettings {
  return useContext(PanelDisplayContext);
}
