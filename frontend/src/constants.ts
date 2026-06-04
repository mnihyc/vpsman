import {
  CalendarClock,
  ClipboardList,
  DatabaseBackup,
  GitBranch,
  KeyRound,
  Layers3,
  Server,
  TerminalSquare,
  type LucideIcon,
} from "lucide-react";
import type { ActiveView, FleetSummary } from "./types";

export const navItems: readonly { view: ActiveView; icon: LucideIcon }[] = [
  { view: "Fleet", icon: Server },
  { view: "Pools", icon: Layers3 },
  { view: "Jobs", icon: TerminalSquare },
  { view: "Schedules", icon: CalendarClock },
  { view: "Topology", icon: GitBranch },
  { view: "Backups", icon: DatabaseBackup },
  { view: "Audit", icon: ClipboardList },
  { view: "Access", icon: KeyRound },
];

export const navSections: readonly {
  label: string;
  items: readonly { view: ActiveView; icon: LucideIcon }[];
}[] = [
  {
    label: "Operations",
    items: navItems.filter((item) => ["Fleet", "Pools", "Jobs", "Schedules"].includes(item.view)),
  },
  {
    label: "Network",
    items: navItems.filter((item) => item.view === "Topology"),
  },
  {
    label: "Data & access",
    items: navItems.filter((item) => ["Backups", "Audit", "Access"].includes(item.view)),
  },
];

export const emptySummary: FleetSummary = {
  total: 0,
  connected: 0,
  warnings: 0,
  running_jobs: 0,
};

export const ACCESS_TOKEN_STORAGE_KEY = "vpsman.accessToken";
export const REFRESH_TOKEN_STORAGE_KEY = "vpsman.refreshToken";
