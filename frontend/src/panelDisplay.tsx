import { createContext, useContext, type ReactNode } from "react";
import { DEFAULT_VPS_NAME_DISPLAY_MODE, type VpsNameDisplayMode } from "./utils";

type PanelDisplaySettings = {
  vpsNameDisplayMode: VpsNameDisplayMode;
  setVpsNameDisplayMode: (mode: VpsNameDisplayMode) => void;
};

const fallbackSettings: PanelDisplaySettings = {
  vpsNameDisplayMode: DEFAULT_VPS_NAME_DISPLAY_MODE,
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
