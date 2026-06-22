import {
  CalendarClock,
  ClipboardList,
  DatabaseBackup,
  GitBranch,
  KeyRound,
  LayoutDashboard,
  Server,
  ServerCog,
  SlidersHorizontal,
  Tag,
  TerminalSquare,
  type LucideIcon,
} from "lucide-react";
import type { ActiveView, FleetSummary } from "./types";

export type ConsoleSubpage = {
  id: string;
  label: string;
  description: string;
};

export const navItems: readonly { view: ActiveView; icon: LucideIcon }[] = [
  { view: "Dashboard", icon: LayoutDashboard },
  { view: "Fleet", icon: Server },
  { view: "Config", icon: SlidersHorizontal },
  { view: "Tags", icon: Tag },
  { view: "Jobs", icon: TerminalSquare },
  { view: "Schedules", icon: CalendarClock },
  { view: "Topology", icon: GitBranch },
  { view: "Backups", icon: DatabaseBackup },
  { view: "Audit", icon: ClipboardList },
  { view: "Access", icon: KeyRound },
  { view: "System", icon: ServerCog },
];

export const navSections: readonly {
  label: string;
  items: readonly { view: ActiveView; icon: LucideIcon }[];
}[] = [
  {
    label: "Operations",
    items: navItems.filter((item) => ["Dashboard", "Fleet", "Config", "Tags", "Jobs", "Schedules"].includes(item.view)),
  },
  {
    label: "Network",
    items: navItems.filter((item) => item.view === "Topology"),
  },
  {
    label: "Data & access",
    items: navItems.filter((item) => ["Backups", "Audit", "Access"].includes(item.view)),
  },
  {
    label: "System",
    items: navItems.filter((item) => item.view === "System"),
  },
];

export function viewLabel(view: ActiveView): string {
  switch (view) {
    case "Config":
      return "Agent config";
    default:
      return view;
  }
}

export const viewSubpages: Record<ActiveView, readonly ConsoleSubpage[]> = {
  Dashboard: [
    {
      id: "overview",
      label: "Overview",
      description: "Operational health, resource usage, network curves, and label clusters",
    },
  ],
  Fleet: [
    { id: "instances", label: "Instances", description: "VPS inventory, health, and selected-instance details" },
    { id: "alerts", label: "Alerts", description: "Active fleet alerts and triage state" },
    { id: "policies", label: "Alert policies", description: "Scoped fleet alert thresholds" },
    { id: "notifications", label: "Notifications", description: "Alert channels, expression webhooks, and delivery queues" },
  ],
  Tags: [
    { id: "registry", label: "Registry", description: "Provider, country, and custom tag counts" },
    { id: "assignments", label: "Assignments", description: "VPS-centric tag assignment and removal" },
    { id: "bulk", label: "Bulk", description: "Selector-based tag add, remove, and delete" },
  ],
  Config: [
    { id: "overview", label: "Overview", description: "Agent config workflow map, source selections, and recent operations" },
    { id: "bulk", label: "Bulk patch", description: "Temporary incremental patches and reusable patch generators" },
    { id: "single", label: "VPS config", description: "One-VPS config read and guarded full override" },
    { id: "templates", label: "Templates", description: "Persistent source templates, assignments, apply, and status" },
  ],
  Jobs: [
    { id: "history", label: "History", description: "Command requests, targets, outputs, and history" },
    { id: "dispatch", label: "Dispatch", description: "Compose privileged commands and terminal actions" },
    { id: "files", label: "Files", description: "Browse, edit, upload, download, and manage one VPS filesystem" },
    { id: "multi_files", label: "Multi files", description: "Bulk file actions by selector expression and policy" },
    { id: "updates", label: "Update registry", description: "Registered external agent release metadata" },
    { id: "transfers", label: "Transfer history", description: "Source artifacts, handoffs, and resumable transfer sessions" },
    { id: "terminal", label: "Terminal sessions", description: "Retained terminal sessions and replay" },
    { id: "processes", label: "Processes", description: "Process supervisor inventory" },
    { id: "server_jobs", label: "Server jobs", description: "Server-side artifact cleanup and control-plane work" },
    { id: "approvals", label: "Schedule runs", description: "Due schedule jobs and queued actions" },
  ],
  Schedules: [
    { id: "registry", label: "Schedule registry", description: "Server-side schedules and due-run records" },
  ],
  Topology: [
    { id: "graph", label: "Graph", description: "Observed topology graph and tunnel plan summary" },
    { id: "plans", label: "Tunnel plans", description: "Saved tunnel plans and plan authoring" },
    { id: "apply", label: "Apply / rollback", description: "Privilege-unlocked tunnel apply, rollback, status, probes, and speed tests" },
    { id: "promotion", label: "Promotion", description: "External observe and custom adapter workflows" },
    { id: "evidence", label: "Evidence", description: "Network trends, observations, and retained plan output" },
    { id: "ospf", label: "OSPF", description: "OSPF update recommendations and cost apply" },
  ],
  Backups: [
    { id: "requests", label: "Requests", description: "Backup request history and metadata" },
    { id: "policies", label: "Policies", description: "Policy create and retention pruning" },
    { id: "artifacts", label: "Artifacts", description: "Upload retained backup artifacts and create handoffs" },
    { id: "restore", label: "Restore", description: "Plan restore, run restore, and rollback" },
    { id: "migration", label: "Migration", description: "Migration assistant for replacement VPS workflows" },
  ],
  Audit: [
    { id: "events", label: "Events", description: "Operator and security audit events" },
    { id: "retention", label: "Retention", description: "History export and retention pruning" },
  ],
  Access: [
    { id: "overview", label: "Overview", description: "Session, vault, key, and live stream posture" },
    { id: "clients", label: "VPS keys", description: "Gateway identities and client key lifecycle" },
    { id: "gateway", label: "Gateway", description: "Gateway sessions and control-plane stream state" },
    { id: "privilege", label: "Privilege unlock", description: "Local privilege unlock and vault controls" },
  ],
  System: [
    {
      id: "dashboard",
      label: "Dashboard",
      description: "Control-plane capacity, queues, deadlines, gateway events, and service health",
    },
    {
      id: "users",
      label: "Users",
      description: "Operator accounts, roles, scopes, lifecycle, password reset, and TOTP reset",
    },
    {
      id: "sessions",
      label: "Sessions",
      description: "Operator sessions and login history",
    },
    {
      id: "config",
      label: "Suite config",
      description: "Suite TOML validation, redacted diff review, and privileged config save",
    },
    {
      id: "operator",
      label: "Preferences",
      description: "Display, timezone, language, and navigation defaults",
    },
  ],
};

export const defaultSubpages: Record<ActiveView, string> = Object.fromEntries(
  Object.entries(viewSubpages).map(([view, subpages]) => [view, subpages[0]?.id ?? "main"]),
) as Record<ActiveView, string>;

export function normalizeSubpage(view: ActiveView, subpage: string | null | undefined): string {
  const valid = viewSubpages[view].some((entry) => entry.id === subpage);
  return valid && subpage ? subpage : defaultSubpages[view];
}

export function subpageLabel(view: ActiveView, subpage: string): string {
  return viewSubpages[view].find((entry) => entry.id === subpage)?.label ?? subpage;
}

export function subpageDescription(view: ActiveView, subpage: string): string {
  return viewSubpages[view].find((entry) => entry.id === subpage)?.description ?? "";
}

export const emptySummary: FleetSummary = {
  never: 0,
  offline: 0,
  online: 0,
  running_jobs: 0,
  stale: 0,
  total: 0,
  warnings: 0,
};

export const ACCESS_TOKEN_STORAGE_KEY = "vpsman.accessToken";
export const REFRESH_TOKEN_STORAGE_KEY = "vpsman.refreshToken";
