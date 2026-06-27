import {
  Activity,
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
  { view: "Home", icon: LayoutDashboard },
  { view: "Fleet", icon: Server },
  { view: "Remote Operations", icon: TerminalSquare },
  { view: "Jobs", icon: TerminalSquare },
  { view: "Automation", icon: CalendarClock },
  { view: "Network", icon: GitBranch },
  { view: "Backups", icon: DatabaseBackup },
  { view: "Config", icon: SlidersHorizontal },
  { view: "Observability", icon: Activity },
  { view: "Audit", icon: ClipboardList },
  { view: "Access", icon: KeyRound },
  { view: "System", icon: ServerCog },
];

export const navSections: readonly {
  label: string;
  items: readonly { view: ActiveView; icon: LucideIcon }[];
}[] = [
  {
    label: "Operate",
    items: navItems.filter((item) =>
      ["Home", "Fleet", "Remote Operations", "Jobs", "Automation"].includes(
        item.view,
      ),
    ),
  },
  {
    label: "Infrastructure",
    items: navItems.filter((item) =>
      ["Network", "Backups", "Config", "Observability"].includes(item.view),
    ),
  },
  {
    label: "Governance",
    items: navItems.filter((item) =>
      ["Audit", "Access", "System"].includes(item.view),
    ),
  },
];

export function viewLabel(view: ActiveView): string {
  return view;
}

export const viewSubpages: Record<ActiveView, readonly ConsoleSubpage[]> = {
  Home: [
    {
      id: "overview",
      label: "Overview",
      description:
        "Fleet posture, quick actions, attention queue, and recent activity",
    },
  ],
  Fleet: [
    {
      id: "instances",
      label: "Instances",
      description: "Canonical VPS inventory and row actions",
    },
    {
      id: "monitor",
      label: "Monitor",
      description: "Komari-style VPS health cards for quick scanning",
    },
    {
      id: "groups",
      label: "Groups",
      description: "Tag registry and saved resource grouping",
    },
    {
      id: "group_assignments",
      label: "Assignments",
      description: "VPS-centric group/tag assignment",
    },
    {
      id: "group_bulk",
      label: "Bulk groups",
      description: "Selector-based group/tag mutations with review",
    },
    {
      id: "alerts",
      label: "Alerts",
      description: "Active fleet-resource alert queue and triage",
    },
    {
      id: "instance_detail",
      label: "Instance detail",
      description: "Canonical one-VPS details and workflow links",
    },
  ],
  "Remote Operations": [
    {
      id: "terminal",
      label: "Terminal",
      description: "Open, resume, replay, and audit browser terminal sessions",
    },
    {
      id: "files",
      label: "Files",
      description: "One-VPS file browser, editor, and guarded file actions",
    },
    {
      id: "transfers",
      label: "Transfers",
      description: "Transfer sessions, handoffs, and retry evidence",
    },
    {
      id: "processes",
      label: "Processes",
      description: "Process inventory, logs, restart, and stop workflows",
    },
    {
      id: "bulk_files",
      label: "Bulk files",
      description: "Multi-VPS file operations with preflight and results",
    },
  ],
  Jobs: [
    {
      id: "history",
      label: "History",
      description: "Execution evidence, targets, outputs, and comparisons",
    },
    {
      id: "dispatch",
      label: "Dispatch",
      description: "Advanced generic command composer and reviewed dispatch",
    },
    {
      id: "approvals",
      label: "Approvals",
      description: "Pending reviewed work waiting for approval",
    },
    {
      id: "scheduled_runs",
      label: "Scheduled runs",
      description: "Automation execution history and worker evidence",
    },
    {
      id: "artifacts",
      label: "Artifacts",
      description: "Retained execution artifacts linked to workflows",
    },
  ],
  Automation: [
    {
      id: "schedules",
      label: "Schedules",
      description:
        "Schedule registry, editor, target preview, and lifecycle actions",
    },
    {
      id: "runbooks",
      label: "Runbooks",
      description: "Reusable reviewed operations and parameters",
    },
    {
      id: "source_templates",
      label: "Source templates",
      description:
        "Persistent source template registry and render/test workflow",
    },
    {
      id: "agent_updates",
      label: "Agent updates",
      description: "Agent release metadata, rollout, and rollback planning",
    },
  ],
  Config: [
    {
      id: "overview",
      label: "Overview",
      description:
        "Config health, drift, template coverage, recent changes, and workflow entry points",
    },
    {
      id: "per_vps",
      label: "Per-VPS",
      description: "One-VPS runtime config read and guarded override",
    },
    {
      id: "bulk_patch",
      label: "Bulk patch",
      description:
        "Temporary incremental patches and reusable patch generators",
    },
    {
      id: "templates",
      label: "Template coverage",
      description:
        "Runtime template coverage summary with links to source templates",
    },
    {
      id: "rules",
      label: "Rules",
      description:
        "Per-VPS traffic rule values for accounting and alert policies",
    },
  ],
  Network: [
    {
      id: "overview",
      label: "Overview",
      description:
        "Network posture, drift, tunnel health, and recommended next actions",
    },
    {
      id: "graph",
      label: "Graph",
      description: "Observed network graph and tunnel plan summary",
    },
    {
      id: "tunnel_plans",
      label: "Tunnel plans",
      description:
        "Saved tunnel plans, reviewed plan authoring, and observed-to-managed promotion",
    },
    {
      id: "tests",
      label: "Tests",
      description: "Tunnel status, probes, and speed tests",
    },
    {
      id: "ospf",
      label: "OSPF",
      description:
        "OSPF update recommendations, cost updates, and rollback planning",
    },
    {
      id: "evidence",
      label: "Evidence",
      description: "Network trends, observations, and retained plan output",
    },
  ],
  Backups: [
    {
      id: "overview",
      label: "Overview",
      description:
        "Backup posture, recent failures, coverage, and recovery readiness",
    },
    {
      id: "requests",
      label: "Requests",
      description: "Backup request history and metadata",
    },
    {
      id: "policies",
      label: "Policies",
      description: "Policy create and retention pruning",
    },
    {
      id: "artifacts",
      label: "Artifacts",
      description: "Upload retained backup artifacts and create transfer packages",
    },
    {
      id: "restore",
      label: "Restore",
      description: "Choose artifact, confirm restore, and rollback",
    },
    {
      id: "migration",
      label: "Migration",
      description: "Map source artifacts to replacement VPS workflows",
    },
  ],
  Observability: [
    {
      id: "fleet_metrics",
      label: "Fleet metrics",
      description: "CPU, memory, disk, and network trends by fleet group",
    },
    {
      id: "network_metrics",
      label: "Network metrics",
      description: "Latency, loss, speed, tunnel, and endpoint trends",
    },
    {
      id: "alerts",
      label: "Alerts",
      description:
        "Alert policies, issued policy alerts, and notification channels",
    },
    {
      id: "webhooks",
      label: "Event webhooks",
      description:
        "Event webhook rules, tests, deliveries, and maintenance separate from alert destinations",
    },
    {
      id: "dashboards",
      label: "Dashboards",
      description: "Saved read-only observability dashboards and widgets",
    },
  ],
  Audit: [
    {
      id: "events",
      label: "Events",
      description: "Operator and security audit events",
    },
    {
      id: "job_evidence",
      label: "Job evidence",
      description: "Who ran what, with privilege, target, and output context",
    },
    {
      id: "sessions",
      label: "Sessions",
      description:
        "Operator and terminal session evidence without live terminal controls",
    },
    {
      id: "retention_export",
      label: "Retention & export",
      description: "History export, retention policy, and prune preview",
    },
  ],
  Access: [
    {
      id: "overview",
      label: "Overview",
      description:
        "Operator, session, identity, gateway, and privilege posture",
    },
    {
      id: "operators",
      label: "Operators",
      description: "User table, roles, scopes, MFA, and session revocation",
    },
    {
      id: "vps_identities",
      label: "VPS identities",
      description:
        "Agent key lifecycle, registration, rotation, and revocation",
    },
    {
      id: "gateway_sessions",
      label: "Gateway sessions",
      description: "Gateway stream state and control-plane routing",
    },
    {
      id: "privilege_vault",
      label: "Privilege vault",
      description: "Local privilege unlock, vault state, and lock action",
    },
  ],
  System: [
    {
      id: "overview",
      label: "Overview",
      description:
        "Control-plane capacity, queues, deadlines, gateway events, and service health",
    },
    {
      id: "capacity",
      label: "Capacity",
      description:
        "Queue depth, dispatch capacity, artifact storage, and worker lag",
    },
    {
      id: "suite_config",
      label: "Suite config",
      description:
        "Suite TOML validation, redacted diff review, and privileged config save",
    },
    {
      id: "maintenance",
      label: "Maintenance",
      description:
        "Artifact cleanup dry-run, object-store health, prune history, and maintenance jobs",
    },
    {
      id: "preferences",
      label: "Preferences",
      description: "Display, timezone, language, and navigation defaults",
    },
  ],
};

export const defaultSubpages: Record<ActiveView, string> = Object.fromEntries(
  Object.entries(viewSubpages).map(([view, subpages]) => [
    view,
    subpages[0]?.id ?? "main",
  ]),
) as Record<ActiveView, string>;

export function normalizeSubpage(
  view: ActiveView,
  subpage: string | null | undefined,
): string {
  const requested = subpage ?? "";
  const base = requested.split(":")[0];
  const valid = viewSubpages[view].some(
    (entry) => entry.id === requested || entry.id === base,
  );
  return valid && requested ? requested : defaultSubpages[view];
}

export function subpageLabel(view: ActiveView, subpage: string): string {
  const base = subpage.split(":")[0];
  return (
    viewSubpages[view].find(
      (entry) => entry.id === subpage || entry.id === base,
    )?.label ?? subpage
  );
}

export function subpageDescription(view: ActiveView, subpage: string): string {
  const base = subpage.split(":")[0];
  return (
    viewSubpages[view].find(
      (entry) => entry.id === subpage || entry.id === base,
    )?.description ?? ""
  );
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
