import {
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  ConsoleShell,
  type CommandPaletteItem,
} from "./components/ConsoleShell";
import { PrivilegeUnlockDialog } from "./components/PrivilegeUnlockDialog";
import { AdminRoleBoundary } from "./components/RoleBoundary";
import { WorkspaceErrorBoundary } from "./components/WorkspaceErrorBoundary";
import { AuthPanel } from "./panels/AuthPanel";
import { FleetAlertsPanel } from "./panels/FleetAlertsPanel";
import { FleetMonitorPanel } from "./panels/FleetMonitorPanel";
import { JobEvidencePanel } from "./panels/audit/JobEvidencePanel";
import { SessionEvidencePanel } from "./panels/audit/SessionEvidencePanel";
import { JobArtifactsPanel } from "./panels/jobs/JobArtifactsPanel";
import { PanelDisplayProvider } from "./panelDisplay";
import type { ActiveView, AgentView, FleetSummary } from "./types";
import type { PrivilegeMaterial } from "./privilege";
import {
  defaultSubpages,
  FLEET_DETAIL_LIMIT,
  isActionableFleetAlertState,
  navItems,
  normalizeSubpage,
  viewLabel,
  viewSubpages,
} from "./constants";
import {
  getPageDescription,
  getPageTitle,
  sanitizeOperatorPreferences,
  setPreferredTimeZone,
  type VpsNameDisplayMode,
} from "./utils";
import { useDashboardData } from "./hooks/useDashboardData";
import { useFleetViews } from "./hooks/useFleetViews";
import { useValueTooltips } from "./useValueTooltips";
import { agentDisplayState } from "./agentDisplayState";
import type {
  JobDispatchPreset,
  JobDispatchPresetInput,
} from "./jobDispatchPreset";
import { retryableLazy } from "./lazyImport";

type ReleaseRouteTarget = AgentView | string;

function combineErrors(
  ...errors: Array<string | null | undefined>
): string | null {
  const messages = Array.from(
    new Set(
      errors.filter(
        (error): error is string => Boolean(error && error.trim()),
      ),
    ),
  );
  return messages.length > 0 ? messages.join(" · ") : null;
}

type ConsoleRouteState = {
  subpage: string;
  view: ActiveView;
};

type WorkflowTargetIntent = {
  clientId: string;
  destination: "backup_requests" | "network_graph" | "processes" | "terminal";
  requestId: string;
};

type ReleaseRouteHelpers = {
  openAuditEvidence: (auditId?: string) => void;
  openFiles: (target: ReleaseRouteTarget, path?: string) => void;
  openJobEvidence: (jobId: string) => void;
  openNetworkEvidence: (target?: ReleaseRouteTarget) => void;
  openProcess: (target: ReleaseRouteTarget) => void;
  openTerminal: (target: ReleaseRouteTarget) => void;
  openVpsDetail: (target: ReleaseRouteTarget) => void;
};

const HomePanel = retryableLazy(() =>
  import("./panels/HomePanel").then((module) => ({
    default: module.HomePanel,
  })),
);
const FleetWorkspace = retryableLazy(() =>
  import("./panels/FleetWorkspace").then((module) => ({
    default: module.FleetWorkspace,
  })),
);
const VpsDetailPanel = retryableLazy(() =>
  import("./panels/VpsDetailPanel").then((module) => ({
    default: module.VpsDetailPanel,
  })),
);
const ConfigPanel = retryableLazy(() =>
  import("./panels/ConfigPanel").then((module) => ({
    default: module.ConfigPanel,
  })),
);
const JobsPanel = retryableLazy(() =>
  import("./panels/JobsPanel").then((module) => ({
    default: module.JobsPanel,
  })),
);
const RemoteOperationsPanel = retryableLazy(() =>
  import("./panels/RemoteOperationsPanel").then((module) => ({
    default: module.RemoteOperationsPanel,
  })),
);
const ServerJobsPanel = retryableLazy(() =>
  import("./panels/jobs/ServerJobsPanel").then((module) => ({
    default: module.ServerJobsPanel,
  })),
);
const FleetGroupsPanel = retryableLazy(() =>
  import("./panels/FleetGroupsPanel").then((module) => ({
    default: module.FleetGroupsPanel,
  })),
);
const SchedulesPanel = retryableLazy(() =>
  import("./panels/SchedulesPanel").then((module) => ({
    default: module.SchedulesPanel,
  })),
);
const SourceTemplatePanel = retryableLazy(() =>
  import("./panels/SourceTemplatesPanel").then((module) => ({
    default: module.SourceTemplatePanel,
  })),
);
const AgentUpdateReleasesPanel = retryableLazy(() =>
  import("./panels/automation/AgentUpdateReleasesPanel").then((module) => ({
    default: module.AgentUpdateReleasesPanel,
  })),
);
const OsUpdatesPanel = retryableLazy(() =>
  import("./panels/automation/OsUpdatesPanel").then((module) => ({
    default: module.OsUpdatesPanel,
  })),
);
const RolloutsPanel = retryableLazy(() =>
  import("./panels/automation/RolloutsPanel").then((module) => ({
    default: module.RolloutsPanel,
  })),
);
const RunbooksPanel = retryableLazy(() =>
  import("./panels/automation/RunbooksPanel").then((module) => ({
    default: module.RunbooksPanel,
  })),
);
const FleetMetricsPanel = retryableLazy(() =>
  import("./panels/observability/FleetMetricsPanel").then((module) => ({
    default: module.FleetMetricsPanel,
  })),
);
const NetworkMetricsPanel = retryableLazy(() =>
  import("./panels/observability/NetworkMetricsPanel").then((module) => ({
    default: module.NetworkMetricsPanel,
  })),
);
const AlertsPanel = retryableLazy(() =>
  import("./panels/observability/AlertsPanel").then((module) => ({
    default: module.AlertsPanel,
  })),
);
const WebhooksPanel = retryableLazy(() =>
  import("./panels/observability/WebhooksPanel").then((module) => ({
    default: module.WebhooksPanel,
  })),
);
const ObservabilityDashboardsPanel = retryableLazy(() =>
  import("./panels/observability/ObservabilityDashboardsPanel").then(
    (module) => ({
      default: module.ObservabilityDashboardsPanel,
    }),
  ),
);
const AccessPanel = retryableLazy(() =>
  import("./panels/AccessPanel").then((module) => ({
    default: module.AccessPanel,
  })),
);
const AuditLogPanel = retryableLazy(() =>
  import("./panels/AuditLogPanel").then((module) => ({
    default: module.AuditLogPanel,
  })),
);
const BackupsPanel = retryableLazy(() =>
  import("./panels/BackupsPanel").then((module) => ({
    default: module.BackupsPanel,
  })),
);
const TopologyPanel = retryableLazy(() =>
  import("./panels/TopologyPanel").then((module) => ({
    default: module.TopologyPanel,
  })),
);
const SystemPanel = retryableLazy(() =>
  import("./panels/SystemPanel").then((module) => ({
    default: module.SystemPanel,
  })),
);

function getScopedPageTitle(view: ActiveView, subpage: string): string {
  if (view === "System") {
    switch (subpage) {
      case "suite_config":
        return "Suite config";
      case "capacity":
        return "System capacity";
      case "maintenance":
        return "System maintenance";
      case "preferences":
        return "System preferences";
      default:
        return "System overview";
    }
  }
  if (view === "Remote Operations") {
    switch (subpage) {
      case "terminal":
        return "Terminal";
      case "files":
        return "Files";
      case "bulk_files":
        return "Bulk files";
      case "transfers":
        return "Transfers";
      case "processes":
        return "Processes";
      case "services":
        return "Services";
      case "storage":
        return "Storage";
      default:
        return "Remote";
    }
  }
  if (view === "Jobs") {
    switch (subpage) {
      case "dispatch":
        return "Command dispatch";
      case "approvals":
        return "Approvals";
      case "scheduled_runs":
        return "Scheduled runs";
      case "artifacts":
        return "Job artifacts";
      default:
        return "Job history";
    }
  }
  if (view === "Automation") {
    switch (subpage) {
      case "schedules":
        return "Schedules";
      case "rollouts":
        return "Rollouts";
      case "runbooks":
        return "Runbooks";
      case "source_templates":
        return "Source templates";
      case "os_updates":
        return "OS updates";
      case "agent_updates":
        return "Agent updates";
      default:
        return "Automation";
    }
  }
  if (view === "Network") {
    switch (subpage) {
      case "graph":
        return "Network graph";
      case "tunnel_plans":
        return "Tunnel plans";
      case "port_forwards":
        return "Port forwarding";
      case "tests":
        return "Network tests";
      case "ospf":
        return "Network OSPF";
      case "evidence":
        return "Network evidence";
      default:
        return "Network overview";
    }
  }
  if (view === "Fleet") {
    if (subpage.startsWith("instance_detail:")) {
      return "Instance detail";
    }
    switch (subpage) {
      case "monitor":
        return "Fleet monitor";
      case "groups":
        return "Fleet groups";
      case "group_assignments":
        return "Group assignments";
      case "group_bulk":
        return "Bulk groups";
      case "alerts":
        return "Fleet alerts";
      case "instance_detail":
        return "Instance detail";
      default:
        return "Fleet instances";
    }
  }
  if (view === "Backups") {
    switch (subpage) {
      case "requests":
        return "Backup requests";
      case "policies":
        return "Backup policies";
      case "artifacts":
        return "Backup artifacts";
      case "restore":
        return "Restore";
      case "migration":
        return "Migration";
      default:
        return "Backup overview";
    }
  }
  if (view === "Audit") {
    switch (subpage) {
      case "job_evidence":
        return "Job evidence";
      case "sessions":
        return "Session evidence";
      case "retention_export":
        return "Retention & export";
      default:
        return "Audit events";
    }
  }
  if (view === "Observability") {
    if (subpage.startsWith("alerts:policy:")) {
      return "Alerts";
    }
    switch (subpage) {
      case "network_metrics":
        return "Network metrics";
      case "alerts":
        return "Alerts";
      case "webhooks":
        return "Event webhooks";
      case "dashboards":
        return "Dashboards";
      default:
        return "Fleet metrics";
    }
  }
  if (view === "Access" && subpage === "operators") {
    return "Operators";
  }
  if (view === "Access") {
    switch (subpage) {
      case "vps_identities":
        return "VPS identities";
      case "gateway_sessions":
        return "Gateway sessions";
      case "privilege_vault":
        return "Privilege vault";
      default:
        return "Access overview";
    }
  }
  return getPageTitle(view);
}

function getScopedPageDescription(view: ActiveView, subpage: string): string {
  if (view === "Remote Operations") {
    switch (subpage) {
      case "terminal":
        return "Open, resume, replay, and audit browser terminal sessions";
      case "files":
        return "Browse and edit one VPS with explicit file action evidence";
      case "transfers":
        return "Transfer sessions, resumable handoffs, and integrity evidence";
      case "processes":
        return "Host and managed process inventory, logs, and lifecycle actions";
      case "services":
        return "Init provider detection, host service state, boot policy, actions, and logs";
      case "storage":
        return "Read-only block devices, mounts, capacity, and provider-reported usage";
      case "bulk_files":
        return "Apply reviewed file operations across an explicit VPS scope";
      default:
        return "Direct VPS access and host operations without leaving the console";
    }
  }
  if (view === "Automation") {
    switch (subpage) {
      case "runbooks":
        return "Reusable reviewed operations, parameters, and execution handoff";
      case "rollouts":
        return "Durable canaries, bounded batches, safety pauses, and per-VPS evidence";
      case "source_templates":
        return "Persistent source templates, rendering, tests, and job handoff";
      case "os_updates":
        return "Native package support, reviewed update candidates, and explicit application";
      case "agent_updates":
        return "Release metadata, update checks, rollout, rollback, and job evidence";
      default:
        return "Schedules, target previews, lifecycle controls, and run evidence";
    }
  }
  if (view === "System") {
    switch (subpage) {
      case "suite_config":
        return "Suite settings, validation, save review, and reload impact";
      case "capacity":
        return "Database pressure, dispatch limits, and gateway queue health";
      case "maintenance":
        return "Cleanup, object-store health, and maintenance jobs";
      case "preferences":
        return "Console display, navigation, and workflow defaults";
      default:
        return "Service health, queues, KPIs, and attention signals";
    }
  }
  if (view === "Access") {
    switch (subpage) {
      case "operators":
        return "Operator accounts, roles, MFA, scopes, and sessions";
      case "vps_identities":
        return "Agent registration, rotation, revocation, and install evidence";
      case "gateway_sessions":
        return "Gateway streams, agent connectivity, and routing readiness";
      case "privilege_vault":
        return "Local privilege unlock, vault state, and lock action";
      default:
        return "Operators, sessions, identities, gateway, and privilege";
    }
  }
  if (view === "Backups") {
    switch (subpage) {
      case "requests":
        return "Backup runs and reviewed one-time requests";
      case "policies":
        return "Policies, retention, schedules, and prune review";
      case "artifacts":
        return "Artifacts, uploads, hashes, and transfer packages";
      case "restore":
        return "Restore planning, review, verification, and rollback";
      case "migration":
        return "Replacement VPS migration, restore evidence, and cutover";
      default:
        return "Recoverability, coverage gaps, and restore readiness";
    }
  }
  if (view === "Observability") {
    if (subpage.startsWith("alerts:policy:")) {
      return "Alert policies, active context, channels, and delivery evidence";
    }
    switch (subpage) {
      case "network_metrics":
        return "Latency, loss, throughput, tunnels, endpoints, and alerts";
      case "alerts":
        return "Alert policies, active context, channels, and delivery evidence";
      case "webhooks":
        return "Webhook rules, tests, delivery evidence, and retention";
      case "dashboards":
        return "Saved read-only widgets and shared dashboards";
      default:
        return "Resource charts, grouping controls, and top VPS analysis";
    }
  }
  return getPageDescription(view);
}

function ConsolePanelFallback({ view }: { view: ActiveView }) {
  return (
    <div className="emptyState compactEmpty" role="status" aria-live="polite">
      Loading {view.toLowerCase()} workspace
    </div>
  );
}

const VIEW_ROUTE_SLUGS: Record<ActiveView, string> = Object.fromEntries(
  navItems.map((item) => [item.view, routeToken(viewLabel(item.view))]),
) as Record<ActiveView, string>;

const ROUTE_VIEWS_BY_SLUG = Object.fromEntries(
  navItems.map((item) => [VIEW_ROUTE_SLUGS[item.view], item.view]),
) as Record<string, ActiveView>;

function routeToken(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[\s_]+/g, "-");
}

function consoleRouteHash(view: ActiveView, subpage: string): string {
  return `#/${VIEW_ROUTE_SLUGS[view]}/${subpageRouteSegment(view, subpage)}`;
}

function parseConsoleRouteHash(hash: string): ConsoleRouteState | null {
  const trimmed = hash.trim();
  if (!trimmed.startsWith("#/")) {
    return null;
  }
  const segments = trimmed.slice(2).split("/").filter(Boolean);
  if (segments.length === 0) {
    return null;
  }
  const view = ROUTE_VIEWS_BY_SLUG[decodeRouteSegment(segments[0])];
  if (!view) {
    return null;
  }
  const subpage = routeSegmentSubpage(
    view,
    segments[1] ?? "",
    segments[2] ?? "",
  );
  return {
    subpage: normalizeSubpage(view, subpage),
    view,
  };
}

function readConsoleRouteFromLocation(): ConsoleRouteState | null {
  if (typeof window === "undefined") {
    return null;
  }
  return parseConsoleRouteHash(window.location.hash);
}

function writeConsoleRoute(
  view: ActiveView,
  subpage: string,
  mode: "push" | "replace" = "push",
) {
  if (typeof window === "undefined") {
    return;
  }
  const hash = consoleRouteHash(view, subpage);
  if (window.location.hash === hash) {
    return;
  }
  const searchParams = new URLSearchParams(window.location.search);
  if (!(view === "Remote Operations" && subpage === "processes")) {
    searchParams.delete("process_mode");
    searchParams.delete("process_client");
  }
  if (!(view === "Remote Operations" && subpage === "services")) {
    searchParams.delete("service_client");
  }
  if (!(view === "Remote Operations" && subpage === "storage")) {
    searchParams.delete("storage_client");
    searchParams.delete("storage_system");
    searchParams.delete("storage_view");
  }
  if (!(view === "Automation" && subpage === "os_updates")) {
    searchParams.delete("os_update_client");
  }
  if (!(view === "Automation" && subpage === "rollouts")) {
    searchParams.delete("rollout_job");
  }
  if (!(view === "Observability" && subpage === "network_metrics")) {
    searchParams.delete("network_metric");
  }
  if (!(view === "Observability" && subpage === "dashboards")) {
    [
      "dashboard",
      "window",
      "scope_kind",
      "scope_value",
      "group_by",
      "resource_metric",
      "network_view",
      "traffic_sort",
      "start_at",
      "end_at",
    ].forEach((key) => searchParams.delete(key));
  }
  const search = searchParams.toString();
  const url = `${window.location.pathname}${search ? `?${search}` : ""}${hash}`;
  if (mode === "replace") {
    window.history.replaceState(null, "", url);
    return;
  }
  window.history.pushState(null, "", url);
}

function subpageRouteSegment(view: ActiveView, subpage: string): string {
  if (view === "Fleet" && subpage.startsWith("instance_detail:")) {
    const clientId = subpage.slice("instance_detail:".length).trim();
    return `instance-detail/${encodeURIComponent(clientId)}`;
  }
  const subpages = viewSubpages[view] ?? [];
  const known = subpages.find((entry) => entry.id === subpage);
  if (known) {
    return known.route ?? routeToken(known.id);
  }
  return encodeURIComponent(subpage);
}

function routeSegmentSubpage(
  view: ActiveView,
  segment: string,
  resourceSegment = "",
): string {
  const decoded = decodeRouteSegment(segment);
  if (view === "Fleet" && decoded === "instance-detail" && resourceSegment) {
    return `instance_detail:${decodeRouteSegment(resourceSegment)}`;
  }
  const subpages = viewSubpages[view] ?? [];
  const known = subpages.find(
    (entry) => (entry.route ?? routeToken(entry.id)) === decoded,
  );
  if (known) {
    return known.id;
  }
  return decoded;
}

function decodeRouteSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

function fleetDetailClientId(subpage: string): string | null {
  if (!subpage.startsWith("instance_detail:")) {
    return null;
  }
  return subpage.slice("instance_detail:".length).trim() || null;
}

function shortCommandId(id: string) {
  return id.length > 12 ? id.slice(0, 8) : id;
}

function releaseTargetId(target: ReleaseRouteTarget): string {
  return typeof target === "string" ? target : target.id;
}

export function App() {
  useValueTooltips();
  const initialRouteRef = useRef<ConsoleRouteState | null>(
    readConsoleRouteFromLocation(),
  );
  const [activeView, setActiveView] = useState<ActiveView>(
    initialRouteRef.current?.view ?? "Home",
  );
  const [activeSubpages, setActiveSubpages] = useState<
    Record<ActiveView, string>
  >(() => ({
    ...defaultSubpages,
    ...(initialRouteRef.current
      ? {
          [initialRouteRef.current.view]: initialRouteRef.current.subpage,
        }
      : {}),
  }));
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [workflowTargetIntent, setWorkflowTargetIntent] =
    useState<WorkflowTargetIntent | null>(null);
  const [pendingJobDetailId, setPendingJobDetailId] = useState<string | null>(
    null,
  );
  const [jobDispatchPreset, setJobDispatchPreset] =
    useState<JobDispatchPreset | null>(null);
  const [networkPlanWorkflowIntent, setNetworkPlanWorkflowIntent] = useState<
    "create" | null
  >(null);
  const [transferTargetIntent, setTransferTargetIntent] = useState<{
    clientId: string;
    context: string;
    path: string;
  } | null>(null);
  const [accessIdentityWorkflowIntent, setAccessIdentityWorkflowIntent] =
    useState<"register" | null>(null);
  const [sourceTemplateWorkflowIntent, setSourceTemplateWorkflowIntent] =
    useState<"runtime_tunnel_adapter" | "routing_cost_adapter" | null>(null);
  const [privilegeGrant, setPrivilegeGrant] = useState<{
    material: PrivilegeMaterial;
    operatorId: string;
  } | null>(null);
  const [privilegeUnlockOpen, setPrivilegeUnlockOpen] = useState(false);
  const closePrivilegeUnlock = useCallback(
    () => setPrivilegeUnlockOpen(false),
    [],
  );
  const dashboard = useDashboardData(activeView);
  const privilegeMaterial =
    privilegeGrant &&
    dashboard.apiToken &&
    dashboard.operator?.id === privilegeGrant.operatorId
      ? privilegeGrant.material
      : null;
  const setPrivilegeMaterial = useCallback(
    (material: PrivilegeMaterial | null) => {
      if (!material || !dashboard.apiToken || !dashboard.operator?.id) {
        setPrivilegeGrant(null);
        return;
      }
      setPrivilegeGrant({
        material,
        operatorId: dashboard.operator.id,
      });
    },
    [dashboard.apiToken, dashboard.operator?.id],
  );
  useEffect(() => {
    if (!dashboard.apiToken) {
      setPrivilegeGrant(null);
      setPrivilegeUnlockOpen(false);
    }
  }, [dashboard.apiToken]);
  useEffect(() => {
    if (
      privilegeGrant &&
      privilegeGrant.operatorId !== dashboard.operator?.id
    ) {
      setPrivilegeGrant(null);
      setPrivilegeUnlockOpen(false);
    }
  }, [dashboard.operator?.id, privilegeGrant]);
  const fleetViews = useFleetViews(dashboard.agents);
  const operatorPreferences = useMemo(
    () => sanitizeOperatorPreferences(dashboard.operator?.preferences),
    [dashboard.operator?.preferences],
  );
  const visibleAgents = fleetViews.filteredAgents;
  const activeSubpage = normalizeSubpage(
    activeView,
    activeSubpages[activeView],
  );
  const selectedAgent = useMemo(
    () =>
      visibleAgents.find((agent) => agent.id === selectedAgentId) ??
      visibleAgents[0] ??
      null,
    [selectedAgentId, visibleAgents],
  );
  const selectedAgentForDetail = useMemo(() => {
    const clientId =
      activeView === "Fleet" ? fleetDetailClientId(activeSubpage) : null;
    return clientId
      ? dashboard.agents.find((agent) => agent.id === clientId) ?? null
      : null;
  }, [activeSubpage, activeView, dashboard.agents]);
  const visibleSummary = useMemo(
    () =>
      displaySummaryForAgents(visibleAgents, dashboard.summary.running_jobs),
    [dashboard.summary.running_jobs, visibleAgents],
  );
  const pageTitle = getScopedPageTitle(activeView, activeSubpage);
  const hasFleetScope =
    fleetViews.fleetQuery.trim().length > 0 ||
    fleetViews.activeSavedViewId !== null;
  const runtimeConfigEvidenceState = dashboard.runtimeConfigApplyLoading
    ? "loading"
    : dashboard.runtimeConfigApplyEvidenceAvailable
      ? "available"
      : "unavailable";
  const configInventoryEvidenceState = dashboard.tagsLoading
    ? "loading"
    : dashboard.tagInventoryEvidenceAvailable &&
        dashboard.tagsError === null
      ? "available"
      : "unavailable";
  const homeEvidenceLoading =
    dashboard.dashboardOverviewLoading ||
    dashboard.jobsLoading ||
    dashboard.backupsLoading ||
    dashboard.auditLoading ||
    dashboard.schedulesLoading ||
    dashboard.systemDashboardLoading;
  const scopedFleetAlertsEvidenceAvailable =
    dashboard.fleetAlertsEvidenceAvailable &&
    (!hasFleetScope || dashboard.fleetCoreEvidenceAvailable);
  const homeJobsEvidenceAvailable =
    dashboard.fleetCoreEvidenceAvailable &&
    dashboard.jobsEvidenceAvailable &&
    !dashboard.jobsLoading;
  const homeBackupsEvidenceAvailable =
    dashboard.fleetCoreEvidenceAvailable &&
    dashboard.backupsEvidenceAvailable &&
    !dashboard.backupsLoading;
  const recordPageBounds = {
    audits: dashboard.auditsTruncated,
    backupArtifacts: dashboard.backupArtifactsTruncated,
    backups: dashboard.backupsTruncated,
    fileTransfers: dashboard.fileTransfersTruncated,
    fleetAlerts: dashboard.fleetAlerts.length >= FLEET_DETAIL_LIMIT,
    jobs: dashboard.jobsTruncated,
    schedules: dashboard.schedulesTruncated,
  };
  const shellSummary =
    hasFleetScope || activeView === "Home" || activeView === "Fleet"
      ? visibleSummary
      : dashboard.summary;
  const summaryScopeLabel = hasFleetScope ? "Current scope" : "Entire fleet";
  const shellAlertCounts = useMemo(() => {
    const scopedClientIds = new Set(visibleAgents.map((agent) => agent.id));
    const activeAlerts = dashboard.fleetAlerts.filter(
      (alert) =>
        isActionableFleetAlertState(alert.operator_state) &&
        (!hasFleetScope ||
          alert.client_id === null ||
          scopedClientIds.has(alert.client_id)),
    );
    const critical = activeAlerts.filter(
      (alert) => alert.severity === "critical",
    ).length;
    const warning = activeAlerts.filter(
      (alert) => alert.severity === "warning",
    ).length;
    const info = activeAlerts.length - critical - warning;
    return {
      critical,
      info,
      total: activeAlerts.length,
      truncated: dashboard.fleetAlerts.length >= FLEET_DETAIL_LIMIT,
      warning,
    };
  }, [dashboard.fleetAlerts, hasFleetScope, visibleAgents]);
  const homeScopedRecords = useMemo(() => {
    if (!hasFleetScope) {
      return {
        backupArtifacts: dashboard.backupArtifacts,
        backups: dashboard.backups,
        fileTransfers: dashboard.fileTransfers,
        fleetAlerts: dashboard.fleetAlerts,
        schedules: dashboard.schedules,
      };
    }
    const scopedClientIds = new Set(visibleAgents.map((agent) => agent.id));
    return {
      backupArtifacts: dashboard.backupArtifacts.filter((artifact) =>
        scopedClientIds.has(artifact.client_id),
      ),
      backups: dashboard.backups.filter((backup) =>
        scopedClientIds.has(backup.client_id),
      ),
      fileTransfers: dashboard.fileTransfers.filter((transfer) =>
        scopedClientIds.has(transfer.client_id),
      ),
      fleetAlerts: dashboard.fleetAlerts.filter(
        (alert) =>
          alert.client_id === null || scopedClientIds.has(alert.client_id),
      ),
      schedules: dashboard.schedules.filter((schedule) =>
        schedule.target_client_ids.some((clientId) =>
          scopedClientIds.has(clientId),
        ),
      ),
    };
  }, [
    dashboard.backupArtifacts,
    dashboard.backups,
    dashboard.fileTransfers,
    dashboard.fleetAlerts,
    dashboard.schedules,
    hasFleetScope,
    visibleAgents,
  ]);
  const onlineRatio = useMemo(() => {
    if (!dashboard.fleetCoreEvidenceAvailable) {
      return "Unknown";
    }
    if (shellSummary.total === 0) {
      return "0%";
    }
    return `${Math.round((shellSummary.online / shellSummary.total) * 100)}%`;
  }, [
    dashboard.fleetCoreEvidenceAvailable,
    shellSummary.online,
    shellSummary.total,
  ]);
  const pageDescription =
    activeView === "Fleet" && !dashboard.fleetCoreEvidenceAvailable
      ? "Fleet inventory evidence unavailable; retry before assuming the fleet is empty"
      : activeView === "Fleet" && hasFleetScope
      ? `${visibleSummary.online} visible live / ${visibleSummary.never + visibleSummary.unknown} no contact / ${visibleSummary.total} visible / ${dashboard.summary.total} total`
      : activeView === "Fleet"
        ? `${visibleSummary.online} live / ${visibleSummary.never + visibleSummary.unknown} no contact / ${visibleSummary.total} total`
        : getScopedPageDescription(activeView, activeSubpage);

  useEffect(() => {
    setPreferredTimeZone(operatorPreferences.timezone);
  }, [operatorPreferences.timezone]);

  useEffect(() => {
    const initialLocationRoute = readConsoleRouteFromLocation();
    if (!initialLocationRoute) {
      writeConsoleRoute(activeView, activeSubpage, "replace");
    } else if (
      window.location.hash !==
      consoleRouteHash(initialLocationRoute.view, initialLocationRoute.subpage)
    ) {
      writeConsoleRoute(
        initialLocationRoute.view,
        initialLocationRoute.subpage,
        "replace",
      );
    }
    const applyLocationRoute = () => {
      const route = readConsoleRouteFromLocation();
      if (!route) {
        setActiveView("Home");
        setActiveSubpages((current) => ({
          ...current,
          Home: "overview",
        }));
        writeConsoleRoute("Home", "overview", "replace");
        return;
      }
      setActiveView(route.view);
      setActiveSubpages((current) => ({
        ...current,
        [route.view]: route.subpage,
      }));
      if (
        window.location.hash !== consoleRouteHash(route.view, route.subpage)
      ) {
        writeConsoleRoute(route.view, route.subpage, "replace");
      }
    };
    window.addEventListener("hashchange", applyLocationRoute);
    window.addEventListener("popstate", applyLocationRoute);
    return () => {
      window.removeEventListener("hashchange", applyLocationRoute);
      window.removeEventListener("popstate", applyLocationRoute);
    };
  }, []);

  function updateVpsNameDisplayMode(mode: VpsNameDisplayMode) {
    void dashboard.updateOperatorPreferences({
      ...operatorPreferences,
      vps_name_display_mode: mode,
    });
  }

  function selectView(
    view: ActiveView,
    subpage?: string,
    preserveWorkflowTargetIntent = false,
  ) {
    const nextSubpage = normalizeSubpage(view, subpage ?? activeSubpages[view]);
    if (!preserveWorkflowTargetIntent) {
      setWorkflowTargetIntent(null);
    }
    setActiveView(view);
    setActiveSubpages((current) => ({
      ...current,
      [view]: nextSubpage,
    }));
    writeConsoleRoute(view, nextSubpage);
  }

  function selectSubpage(subpage: string) {
    const nextSubpage = normalizeSubpage(activeView, subpage);
    setWorkflowTargetIntent(null);
    setActiveSubpages((current) => ({
      ...current,
      [activeView]: nextSubpage,
    }));
    writeConsoleRoute(activeView, nextSubpage);
  }

  function openRolloutDetails(jobId: string) {
    selectView("Automation", "rollouts");
    const url = new URL(window.location.href);
    url.searchParams.set("rollout_job", jobId);
    window.history.replaceState(
      null,
      "",
      `${url.pathname}${url.search}${url.hash}`,
    );
  }

  function selectReleaseDestination(
    view: ActiveView,
    subpage?: string,
    targetClientId?: string,
  ) {
    const destination = releaseDestination(view, subpage);
    if (targetClientId) {
      if (destination.view === "Fleet" && destination.subpage === "instance_detail") {
        openVpsDetail(targetClientId);
        return;
      }
      if (
        destination.view === "Remote Operations" &&
        destination.subpage === "terminal"
      ) {
        openRemoteTerminal(targetClientId);
        return;
      }
      if (
        destination.view === "Remote Operations" &&
        destination.subpage === "files"
      ) {
        openRemoteFiles(targetClientId);
        return;
      }
      if (
        destination.view === "Remote Operations" &&
        destination.subpage === "processes"
      ) {
        openRemoteProcesses(targetClientId);
        return;
      }
      if (destination.view === "Backups" && destination.subpage === "requests") {
        openBackupWorkflowById(targetClientId);
        return;
      }
      if (destination.view === "Network" && destination.subpage === "graph") {
        openNetworkWorkflowById(targetClientId);
        return;
      }
      if (destination.view === "Config" && destination.subpage === "per_vps") {
        openConfigWorkflowById(targetClientId);
        return;
      }
    }
    selectView(destination.view, destination.subpage);
  }

  function navigateDashboardTarget(target: {
    query: string | null;
    subpage: string;
    view: ActiveView;
  }) {
    const destination = releaseDestination(target.view, target.subpage);
    if (target.view === "Fleet" && target.query) {
      fleetViews.setFleetQuery(target.query);
    }
    selectView(destination.view, destination.subpage);
  }

  function openJobEvidence(jobId: string) {
    setPendingJobDetailId(jobId);
    selectView("Jobs", "history");
  }

  function openJobDetails(jobId: string) {
    openJobEvidence(jobId);
  }

  function openJobHistory() {
    setPendingJobDetailId(null);
    selectView("Jobs", "history");
  }

  function openJobDispatchPreset(preset: JobDispatchPresetInput) {
    setJobDispatchPreset({
      ...preset,
      requestId: crypto.randomUUID(),
    });
    selectView("Jobs", "dispatch");
  }

  function openPrivilegeUnlock() {
    setPrivilegeUnlockOpen(true);
  }

  function lockPrivilege() {
    setPrivilegeMaterial(null);
    setPrivilegeUnlockOpen(false);
  }

  function clearOperatorSession() {
    lockPrivilege();
    dashboard.clearSession();
  }

  function openVpsDetail(target: ReleaseRouteTarget) {
    const clientId = releaseTargetId(target);
    setSelectedAgentId(clientId);
    setWorkflowTargetIntent(null);
    selectView("Fleet", `instance_detail:${clientId}`);
  }

  function openRemoteTerminal(target: ReleaseRouteTarget) {
    const clientId = releaseTargetId(target);
    setSelectedAgentId(clientId);
    setWorkflowTargetIntent({
      clientId,
      destination: "terminal",
      requestId: crypto.randomUUID(),
    });
    selectView("Remote Operations", "terminal", true);
  }

  function openRemoteFiles(target: ReleaseRouteTarget, path = "/") {
    const targetClientId = releaseTargetId(target);
    setSelectedAgentId(targetClientId);
    window.localStorage.setItem(
      "vpsman.fileBrowser.state",
      JSON.stringify({ path, showHidden: false, targetClientId }),
    );
    selectView("Remote Operations", "files");
  }

  function openRemoteProcesses(target: ReleaseRouteTarget) {
    const clientId = releaseTargetId(target);
    setSelectedAgentId(clientId);
    setWorkflowTargetIntent({
      clientId,
      destination: "processes",
      requestId: crypto.randomUUID(),
    });
    selectView("Remote Operations", "processes", true);
  }

  function openBackupWorkflow(agent: AgentView) {
    openBackupWorkflowById(agent.id);
  }

  function openBackupWorkflowById(clientId: string) {
    setSelectedAgentId(clientId);
    setWorkflowTargetIntent({
      clientId,
      destination: "backup_requests",
      requestId: crypto.randomUUID(),
    });
    selectView("Backups", "requests", true);
  }

  function openNetworkWorkflow(agent: AgentView) {
    openNetworkWorkflowById(agent.id);
  }

  function openNetworkWorkflowById(clientId: string) {
    setSelectedAgentId(clientId);
    setWorkflowTargetIntent({
      clientId,
      destination: "network_graph",
      requestId: crypto.randomUUID(),
    });
    selectView("Network", "graph", true);
  }

  const openCreateTunnelPlan = useCallback(() => {
    setNetworkPlanWorkflowIntent("create");
    selectView("Network", "tunnel_plans");
  }, []);

  function openConfigWorkflow(agent: AgentView) {
    openConfigWorkflowById(agent.id);
  }

  function openConfigWorkflowById(clientId: string) {
    setSelectedAgentId(clientId);
    window.localStorage.setItem("vpsman.config.single.clientId", clientId);
    selectView("Config", "per_vps");
  }

  function openNetworkEvidence(_target?: ReleaseRouteTarget) {
    selectView("Network", "evidence");
  }

  function consumeWorkflowTargetIntent(requestId: string) {
    setWorkflowTargetIntent((current) =>
      current?.requestId === requestId ? null : current,
    );
  }

  function openAuditEvidence(_auditId?: string) {
    selectView("Audit", "events");
  }

  function openHomeDispatch(agent: AgentView) {
    setSelectedAgentId(agent.id);
    openJobDispatchPreset({
      mode: "shell",
      selectorExpression: `id:${agent.id}`,
    });
  }

  const releaseRoutes: ReleaseRouteHelpers = {
    openAuditEvidence,
    openFiles: openRemoteFiles,
    openJobEvidence,
    openNetworkEvidence,
    openProcess: openRemoteProcesses,
    openTerminal: openRemoteTerminal,
    openVpsDetail,
  };

  const commandItems = useMemo<CommandPaletteItem[]>(() => {
    const agentNameById = new Map(
      dashboard.agents.map((agent) => [
        agent.id,
        agent.display_name || agent.id,
      ]),
    );
    const pageItems = (
      Object.entries(viewSubpages) as Array<
        [ActiveView, (typeof viewSubpages)[ActiveView]]
      >
    ).flatMap(([view, subpages]) =>
      subpages.map((subpage) => ({
        id: `page:${view}:${subpage.id}`,
        group: "Page" as const,
        label: `${viewLabel(view)} / ${subpage.label}`,
        detail: subpage.description,
        keywords: `${viewLabel(view)} ${view} ${subpage.id} ${subpage.label}`,
        onSelect: () => selectView(view, subpage.id),
      })),
    );
    const vpsItems = dashboard.agents.map((agent) => ({
      id: `vps:${agent.id}`,
      group: "VPS" as const,
      label: agent.display_name || agent.id,
      detail: `${agent.status} · ${agent.id}${agent.tags.length ? ` · ${agent.tags.join(", ")}` : ""}`,
      keywords: `server agent instance ${agent.id} ${agent.tags.join(" ")} ${agent.last_ip ?? ""} ${agent.registration_ip ?? ""}`,
      onSelect: () => {
        fleetViews.setFleetQuery(`id:${agent.id}`);
        releaseRoutes.openVpsDetail(agent);
      },
    }));
    const jobItems = dashboard.jobs.map((job) => ({
      id: `job:${job.id}`,
      group: "Job" as const,
      label: `Job ${shortCommandId(job.id)}`,
      detail: `${job.command_type} · ${job.status} · ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      keywords: `${job.id} ${job.command_type} ${job.status} ${job.privileged ? "privileged" : "standard"}`,
      onSelect: () => releaseRoutes.openJobEvidence(job.id),
    }));
    const terminalItems = dashboard.terminalSessions.map((session) => ({
      id: `terminal:${session.client_id}:${session.session_id}`,
      group: "Terminal" as const,
      label: `Terminal ${shortCommandId(session.session_id)}`,
      detail: `${agentNameById.get(session.client_id) ?? session.client_id} · ${session.state} · ${session.argv.join(" ")}`,
      keywords: `${session.client_id} ${session.session_id} ${session.state} ${session.last_status} ${session.last_command_type} ${session.argv.join(" ")}`,
      onSelect: () => releaseRoutes.openTerminal(session.client_id),
    }));
    const transferItems = dashboard.fileTransfers.map((transfer) => ({
      id: `transfer:${transfer.client_id}:${transfer.session_id}`,
      group: "Transfer" as const,
      label: `Transfer ${shortCommandId(transfer.session_id)}`,
      detail: `${agentNameById.get(transfer.client_id) ?? transfer.client_id} · ${transfer.direction} · ${transfer.status} · ${transfer.path}`,
      keywords: `${transfer.client_id} ${transfer.session_id} ${transfer.direction} ${transfer.status} ${transfer.path} ${transfer.last_command_type}`,
      onSelect: () => {
        setSelectedAgentId(transfer.client_id);
        selectView("Remote Operations", "transfers");
      },
    }));
    const backupRequestItems = dashboard.backups.map((backup) => ({
      id: `backup:${backup.id}`,
      group: "Backup" as const,
      label: `Backup ${shortCommandId(backup.id)}`,
      detail: `${agentNameById.get(backup.client_id) ?? backup.client_id} · ${backup.status} · ${backup.paths.join(", ")}`,
      keywords: `${backup.id} ${backup.client_id} ${backup.status} ${backup.paths.join(" ")} ${backup.note ?? ""}`,
      onSelect: () => {
        setSelectedAgentId(backup.client_id);
        selectView("Backups", "requests");
      },
    }));
    const backupArtifactItems = dashboard.backupArtifacts.map((artifact) => ({
      id: `backup-artifact:${artifact.id}`,
      group: "Backup" as const,
      label: `Backup artifact ${shortCommandId(artifact.id)}`,
      detail: `${agentNameById.get(artifact.client_id) ?? artifact.client_id} · ${artifact.status} · ${artifact.object_key}`,
      keywords: `${artifact.id} ${artifact.client_id} ${artifact.status} ${artifact.object_key} ${artifact.sha256_hex}`,
      onSelect: () => {
        setSelectedAgentId(artifact.client_id);
        selectView("Backups", "artifacts");
      },
    }));
    const auditItems = dashboard.audits.map((audit) => ({
      id: `audit:${audit.id}`,
      group: "Audit" as const,
      label: `Audit event ${shortCommandId(audit.id)}`,
      detail: `${audit.action} · ${audit.target}`,
      keywords: `${audit.id} ${audit.action} ${audit.target} ${audit.actor_id ?? ""} ${audit.command_hash ?? ""}`,
      onSelect: () => releaseRoutes.openAuditEvidence(audit.id),
    }));
    const scheduleItems = dashboard.schedules.map((schedule) => ({
      id: `schedule:${schedule.id}`,
      group: "Schedule" as const,
      label: `Schedule ${schedule.name}`,
      detail: `${
        schedule.cadence_error
          ? "invalid cadence"
          : schedule.enabled
            ? "enabled"
            : "disabled"
      } · ${schedule.cron_expr} · ${schedule.selector_expression}`,
      keywords: `${schedule.id} ${schedule.name} ${schedule.command_type} ${
        schedule.cadence_error ?? ""
      } ${schedule.selector_expression} ${schedule.target_client_ids.join(" ")}`,
      onSelect: () => selectView("Automation", "schedules"),
    }));
    const savedViewItems = fleetViews.savedViews.map((view) => ({
      id: `saved-view:${view.id}`,
      group: "Saved view" as const,
      label: `Saved view ${view.name}`,
      detail: view.query,
      keywords: `${view.id} ${view.name} ${view.query}`,
      onSelect: () => {
        fleetViews.applySavedFleetView(view.id);
        selectView("Fleet", "instances");
      },
    }));
    return [
      ...pageItems,
      ...vpsItems,
      ...jobItems,
      ...terminalItems,
      ...transferItems,
      ...backupRequestItems,
      ...backupArtifactItems,
      ...auditItems,
      ...scheduleItems,
      ...savedViewItems,
    ];
  }, [
    dashboard.agents,
    dashboard.audits,
    dashboard.backupArtifacts,
    dashboard.backups,
    dashboard.fileTransfers,
    dashboard.jobs,
    dashboard.schedules,
    dashboard.terminalSessions,
    fleetViews,
  ]);

  function renderHomePanel() {
    return (
      <HomePanel
        agents={visibleAgents}
        allAgents={dashboard.agents}
        auditLogs={dashboard.audits}
        backupArtifacts={homeScopedRecords.backupArtifacts}
        backups={homeScopedRecords.backups}
        backupsEvidenceAvailable={homeBackupsEvidenceAvailable}
        dashboardError={combineErrors(
          dashboard.dashboardOverviewError,
          dashboard.apiError,
          dashboard.jobsError,
          dashboard.backupsError,
          dashboard.auditError,
          dashboard.schedulesError,
          dashboard.systemDashboardError,
        )}
        dashboardLoading={homeEvidenceLoading}
        dashboardPreferences={dashboard.dashboardPreferences}
        dashboardWindow={dashboard.dashboardOverviewWindow}
        fileTransfers={homeScopedRecords.fileTransfers}
        fleetAlertsEvidenceAvailable={scopedFleetAlertsEvidenceAvailable}
        fleetAlerts={homeScopedRecords.fleetAlerts}
        fleetCoreEvidenceAvailable={dashboard.fleetCoreEvidenceAvailable}
        homeEvidenceComplete={
          !homeEvidenceLoading &&
          dashboard.fleetCoreEvidenceAvailable &&
          scopedFleetAlertsEvidenceAvailable &&
          dashboard.dashboardOverview !== null &&
          dashboard.jobsEvidenceAvailable &&
          dashboard.backupsEvidenceAvailable &&
          dashboard.auditEvidenceAvailable &&
          dashboard.schedulesEvidenceAvailable &&
          dashboard.systemDashboard !== null &&
          combineErrors(
            dashboard.dashboardOverviewError,
            dashboard.jobsError,
            dashboard.backupsError,
            dashboard.auditError,
            dashboard.schedulesError,
            dashboard.systemDashboardError,
          ) === null
        }
        jobs={dashboard.jobs}
        jobsEvidenceAvailable={homeJobsEvidenceAvailable}
        recordBounds={recordPageBounds}
        schedules={homeScopedRecords.schedules}
        scopeFiltered={hasFleetScope}
        summary={visibleSummary}
        systemDashboard={dashboard.systemDashboard}
        telemetryNetworkRates={dashboard.telemetryNetworkRates}
        telemetryRollups={dashboard.telemetryRollups}
        telemetryTunnels={dashboard.telemetryTunnels}
        onDashboardNavigate={navigateDashboardTarget}
        onDashboardPreferencesChange={dashboard.updateDashboardPreferences}
        onDashboardRefresh={() => void dashboard.loadDashboardOverview()}
        onDashboardWindowChange={dashboard.setDashboardOverviewWindow}
        onOpenAudit={releaseRoutes.openAuditEvidence}
        onOpenBackup={openBackupWorkflow}
        onOpenBackups={() => selectView("Backups", "requests")}
        onOpenDispatch={openHomeDispatch}
        onOpenFiles={releaseRoutes.openFiles}
        onOpenFleetAlerts={() => selectView("Fleet", "alerts")}
        onOpenJobDetails={releaseRoutes.openJobEvidence}
        onOpenJobs={() => selectView("Jobs", "history")}
        onOpenNetwork={openNetworkWorkflow}
        onOpenNetworkEvidence={releaseRoutes.openNetworkEvidence}
        onOpenProcesses={releaseRoutes.openProcess}
        onOpenSchedule={() => selectView("Automation", "schedules")}
        onOpenSystemCapacity={() => selectView("System", "capacity")}
        onOpenTerminal={releaseRoutes.openTerminal}
        onOpenTransfers={() => selectView("Remote Operations", "transfers")}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onRegisterVps={() => {
          setAccessIdentityWorkflowIntent("register");
          selectView("Access", "vps_identities");
        }}
      />
    );
  }

  function renderFleetWorkspace(panelSubpage: string) {
    return (
      <FleetWorkspace
        activeSubpage={panelSubpage}
        agents={visibleAgents}
        apiError={dashboard.apiError}
        fleetCoreEvidenceAvailable={dashboard.fleetCoreEvidenceAvailable}
        sourceTemplateAssignments={dashboard.sourceTemplateAssignments}
        sourceStatus={dashboard.sourceStatus}
        fleetAlerts={dashboard.fleetAlerts}
        fleetAlertStates={dashboard.fleetAlertStates}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        policyAlerts={dashboard.policyAlerts}
        trafficAccounting={dashboard.trafficAccounting}
        vpsRuleValues={dashboard.vpsRuleValues}
        fleetAlertNotificationChannels={
          dashboard.fleetAlertNotificationChannels
        }
        fleetAlertNotifications={dashboard.fleetAlertNotifications}
        webhookRules={dashboard.webhookRules}
        webhookRuleDeliveries={dashboard.webhookRuleDeliveries}
        lastLiveEvent={dashboard.lastLiveEvent}
        onCreateJob={dashboard.createJob}
        onBulkMutateTags={dashboard.bulkMutateTags}
        onDeleteAgent={dashboard.deleteAgent}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onLoadJobTargets={dashboard.loadJobTargets}
        onNavigatePanel={selectReleaseDestination}
        onOpenJobDispatchPreset={openJobDispatchPreset}
        onOpenJobDetails={openJobDetails}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onRenderTemplateRuntimeConfig={dashboard.renderTemplateRuntimeConfig}
        onSelectAgent={setSelectedAgentId}
        onUpdateAgentAlias={dashboard.updateAgentAlias}
        privilegeMaterial={privilegeMaterial}
        scopeActive={hasFleetScope}
        onDeleteFleetAlertNotificationChannel={
          dashboard.deleteFleetAlertNotificationChannel
        }
        onDeleteFleetAlertPolicy={dashboard.deleteFleetAlertPolicy}
        onDeleteWebhookRule={dashboard.deleteWebhookRule}
        onDispatchFleetAlertNotifications={
          dashboard.dispatchFleetAlertNotifications
        }
        onDispatchWebhookRules={dashboard.dispatchWebhookRules}
        onDryRunWebhookRule={dashboard.dryRunWebhookRule}
        onDryRunFleetAlertPolicy={dashboard.dryRunFleetAlertPolicy}
        onProcessFleetAlertNotifications={
          dashboard.processFleetAlertNotifications
        }
        onProcessWebhookRuleDeliveries={dashboard.processWebhookRuleDeliveries}
        onRotateWebhookDeliveryHistory={dashboard.rotateWebhookDeliveryHistory}
        onUpdateFleetAlertState={dashboard.updateFleetAlertState}
        onUpsertFleetAlertNotificationChannel={
          dashboard.upsertFleetAlertNotificationChannel
        }
        onUpsertFleetAlertPolicy={dashboard.upsertFleetAlertPolicy}
        onUpsertWebhookRule={dashboard.upsertWebhookRule}
        selectedAgent={selectedAgent}
        summary={visibleSummary}
        tags={dashboard.tags}
        targetAgents={dashboard.agents}
        telemetryNetworkRates={dashboard.telemetryNetworkRates}
        telemetryRollups={dashboard.telemetryRollups}
        telemetryTunnels={dashboard.telemetryTunnels}
        wsState={dashboard.wsState}
      />
    );
  }

  function renderTagsPanel(panelSubpage: string) {
    return (
      <FleetGroupsPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        error={dashboard.tagsError}
        loading={dashboard.tagsLoading}
        onAssignTag={dashboard.assignTag}
        onCreateTag={dashboard.createTag}
        onBulkMutateTags={dashboard.bulkMutateTags}
        onDeleteTag={dashboard.deleteTag}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSchedules={() => selectView("Automation", "schedules")}
        onRefresh={dashboard.loadTagInventory}
        onResolveBulk={dashboard.resolveBulkPreview}
        onUpdateTagOrder={dashboard.updateTagOrder}
        privilegeMaterial={privilegeMaterial}
        schedules={dashboard.schedules}
        tags={dashboard.tags}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
      />
    );
  }

  function renderVpsDetailPanel() {
    return (
      <VpsDetailPanel
        agent={selectedAgentForDetail}
        agents={dashboard.agents}
        apiError={combineErrors(
          dashboard.apiError,
          dashboard.jobsError,
          dashboard.backupsError,
          dashboard.auditError,
          dashboard.tagsError,
          dashboard.runtimeConfigApplyError,
          dashboard.topologyError,
        )}
        audits={dashboard.audits}
        backupArtifacts={dashboard.backupArtifacts}
        backups={dashboard.backups}
        fileTransfers={dashboard.fileTransfers}
        fleetAlerts={dashboard.fleetAlerts}
        fleetAlertsTruncated={recordPageBounds.fleetAlerts}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        jobs={dashboard.jobs}
        recordBounds={recordPageBounds}
        loading={
          dashboard.jobsLoading ||
          dashboard.backupsLoading ||
          dashboard.topologyLoading ||
          dashboard.auditLoading ||
          dashboard.tagsLoading ||
          dashboard.runtimeConfigApplyLoading
        }
        networkObservations={dashboard.networkObservations}
        networkTrends={dashboard.networkTrends}
        onOpenAudit={() => selectView("Audit", "events")}
        onOpenAlertPolicies={(policyId) =>
          selectView(
            "Observability",
            policyId ? `alerts:policy:${policyId}` : "alerts",
          )
        }
        onOpenBackup={openBackupWorkflow}
        onOpenConfig={openConfigWorkflow}
        onOpenDispatch={openHomeDispatch}
        onOpenFiles={releaseRoutes.openFiles}
        onOpenFleetAlerts={() => selectView("Fleet", "alerts")}
        onOpenFleetMetrics={(agent) => {
          dashboard.updateDashboardPreferences({
            endAt: "",
            scopeKind: "client",
            scopeValue: agent.id,
            startAt: "",
          });
          selectView("Observability", "fleet_metrics");
        }}
        onOpenInstances={() => selectView("Fleet", "instances")}
        onOpenJob={releaseRoutes.openJobEvidence}
        onOpenJobs={() => selectView("Jobs", "history")}
        onOpenNetwork={openNetworkWorkflow}
        onOpenNetworkEvidence={releaseRoutes.openNetworkEvidence}
        onOpenProcesses={releaseRoutes.openProcess}
        onOpenTerminal={releaseRoutes.openTerminal}
        policyAlerts={dashboard.policyAlerts}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        runtimeConfigEvidenceState={runtimeConfigEvidenceState}
        sourceStatus={dashboard.sourceStatus}
        sourceTemplateAssignments={dashboard.sourceTemplateAssignments}
        summary={visibleSummary}
        telemetryNetworkRates={dashboard.telemetryNetworkRates}
        telemetryRollups={dashboard.telemetryRollups}
        telemetryTunnels={dashboard.telemetryTunnels}
        vpsRuleValues={dashboard.vpsRuleValues}
      />
    );
  }

  function renderConfigPanel(panelSubpage: string) {
    return (
      <ConfigPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        trafficAccounting={dashboard.trafficAccounting}
        vpsRuleValues={dashboard.vpsRuleValues}
        sourceTemplateAssignments={dashboard.sourceTemplateAssignments}
        sourceTemplates={dashboard.sourceTemplates}
        sourceStatus={dashboard.sourceStatus}
        fleetConfigEvidenceAvailable={
          dashboard.fleetCoreEvidenceAvailable &&
          dashboard.configPolicyEvidenceAvailable
        }
        inventoryEvidenceState={configInventoryEvidenceState}
        error={combineErrors(
          dashboard.apiError,
          dashboard.tagsError,
          dashboard.runtimeConfigApplyError,
        )}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        runtimeConfigEvidenceState={runtimeConfigEvidenceState}
        runtimeConfigPatchGenerators={dashboard.runtimeConfigPatchGenerators}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        jobs={dashboard.jobs}
        loading={
          dashboard.tagsLoading || dashboard.runtimeConfigApplyLoading
        }
        onSubmitRuntimeConfigPatch={dashboard.submitRuntimeConfigPatch}
        onCreateJob={dashboard.createJob}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onLoadJobTargets={dashboard.loadJobTargets}
        onDeleteRuntimeConfigPatchGenerator={
          dashboard.deleteRuntimeConfigPatchGenerator
        }
        onOpenJobDetails={openJobDetails}
        onOpenJobHistory={openJobHistory}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSourceTemplates={() =>
          selectView("Automation", "source_templates")
        }
        onOpenAlerts={() => selectView("Observability", "alerts")}
        onRefresh={dashboard.loadTagInventory}
        onBulkUnsetVpsRules={dashboard.bulkUnsetVpsRules}
        onBulkUpsertVpsRules={dashboard.bulkUpsertVpsRules}
        onDryRunVpsRules={dashboard.dryRunVpsRules}
        onRenderRuntimeConfigPatchGenerator={
          dashboard.renderRuntimeConfigPatchGenerator
        }
        onResolveBulk={dashboard.resolveBulkPreview}
        onSelectSubpage={(subpage) =>
          selectReleaseDestination("Config", subpage)
        }
        onUpsertRuntimeConfigPatchGenerator={
          dashboard.upsertRuntimeConfigPatchGenerator
        }
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
      />
    );
  }

  function renderAlertsPanel() {
    const policyFocusId = activeSubpage.startsWith("alerts:policy:")
      ? activeSubpage.replace("alerts:policy:", "")
      : null;
    return (
      <AlertsPanel
        agents={dashboard.agents}
        apiError={dashboard.apiError}
        fleetAlertNotificationChannels={
          dashboard.fleetAlertNotificationChannels
        }
        fleetAlertNotifications={dashboard.fleetAlertNotifications}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        fleetAlerts={dashboard.fleetAlerts}
        onDeleteFleetAlertNotificationChannel={
          dashboard.deleteFleetAlertNotificationChannel
        }
        onDeleteFleetAlertPolicy={dashboard.deleteFleetAlertPolicy}
        onDispatchFleetAlertNotifications={
          dashboard.dispatchFleetAlertNotifications
        }
        onDryRunFleetAlertPolicy={dashboard.dryRunFleetAlertPolicy}
        onOpenFleetAlerts={() => selectView("Fleet", "alerts")}
        onProcessFleetAlertNotifications={
          dashboard.processFleetAlertNotifications
        }
        onUpsertFleetAlertNotificationChannel={
          dashboard.upsertFleetAlertNotificationChannel
        }
        onUpsertFleetAlertPolicy={dashboard.upsertFleetAlertPolicy}
        policyFocusId={policyFocusId}
        policyAlerts={dashboard.policyAlerts}
      />
    );
  }

  function renderWebhooksPanel() {
    return (
      <WebhooksPanel
        agents={dashboard.agents}
        apiError={dashboard.apiError}
        onDeleteWebhookRule={dashboard.deleteWebhookRule}
        onDispatchWebhookRules={dashboard.dispatchWebhookRules}
        onDryRunWebhookRule={dashboard.dryRunWebhookRule}
        onProcessWebhookRuleDeliveries={dashboard.processWebhookRuleDeliveries}
        onRotateWebhookDeliveryHistory={dashboard.rotateWebhookDeliveryHistory}
        onUpsertWebhookRule={dashboard.upsertWebhookRule}
        webhookRuleDeliveries={dashboard.webhookRuleDeliveries}
        webhookRules={dashboard.webhookRules}
      />
    );
  }

  function renderObservabilityDashboardsPanel() {
    return (
      <ObservabilityDashboardsPanel
        error={dashboard.dashboardOverviewError}
        loading={dashboard.dashboardOverviewLoading}
        onOpenFleetMetrics={() => selectView("Observability", "fleet_metrics")}
        onOpenNetworkMetrics={() =>
          selectView("Observability", "network_metrics")
        }
        onRefresh={() => void dashboard.loadDashboardOverview()}
        overview={dashboard.dashboardOverview}
        preferences={dashboard.dashboardPreferences}
        window={dashboard.dashboardOverviewWindow}
      />
    );
  }

  function renderSourceTemplatesPanel() {
    return (
      <section className="workspace singleColumn">
        <SourceTemplatePanel
          activeSubpage="templates"
          agents={dashboard.agents}
          assignments={dashboard.sourceTemplateAssignments}
          sourceStatus={dashboard.sourceStatus}
          onAssignTemplate={dashboard.assignSourceTemplate}
          onCloneTemplate={dashboard.cloneSourceTemplate}
          onCreateTemplate={dashboard.createSourceTemplate}
          onDiffTemplate={dashboard.diffSourceTemplate}
          initialCreateDomain={sourceTemplateWorkflowIntent}
          onInitialCreateDomainConsumed={() =>
            setSourceTemplateWorkflowIntent(null)
          }
          onOpenTunnelPlans={() => selectView("Network", "tunnel_plans")}
          onRenderTemplateRuntimeConfig={dashboard.renderTemplateRuntimeConfig}
          onResolveBulk={dashboard.resolveBulkPreview}
          onTestTemplate={dashboard.testSourceTemplate}
          onUpdateTemplate={dashboard.updateSourceTemplate}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
          templates={dashboard.sourceTemplates}
        />
      </section>
    );
  }

  function renderAgentUpdatesPanel() {
    const canInspectSuitePolicy = dashboard.operator?.role === "admin";
    const suitePolicyRoleError =
      dashboard.operator && !canInspectSuitePolicy
        ? "Admin role required to inspect Suite Config; the server still enforces its configured update policy."
        : null;
    return (
      <section className="workspace singleColumn">
        <AgentUpdateReleasesPanel
          agents={dashboard.agents}
          jobs={dashboard.jobs}
          loading={dashboard.jobsLoading}
          onCreateAgentUpdateRelease={dashboard.createAgentUpdateRelease}
          onOpenDispatchPreset={openJobDispatchPreset}
          onOpenJobDetails={openJobDetails}
          onOpenJobHistory={() => selectView("Jobs", "history")}
          onRefresh={dashboard.loadJobs}
          releases={dashboard.agentUpdateReleases}
          releasesTruncated={dashboard.agentUpdateReleasesTruncated}
          suiteConfig={canInspectSuitePolicy ? dashboard.suiteConfig : null}
          suiteConfigError={
            suitePolicyRoleError ??
            (canInspectSuitePolicy ? dashboard.suiteConfigError : null)
          }
          suiteConfigLoading={
            dashboard.operator
              ? canInspectSuitePolicy && dashboard.suiteConfigLoading
              : dashboard.accessLoading
          }
        />
      </section>
    );
  }

  function renderOsUpdatesPanel() {
    return (
      <section className="workspace singleColumn">
        <OsUpdatesPanel
          agents={dashboard.agents}
          onCreateJob={dashboard.createJob}
          onDownloadOutputStream={dashboard.downloadJobOutputStream}
          onLoadPlan={dashboard.loadHostPackageUpdatePlan}
          onLoadPlans={dashboard.loadHostPackageUpdatePlans}
          onLoadTargets={dashboard.loadJobTargets}
          onOpenJobDetails={openJobDetails}
          onOpenPrivilegeUnlock={openPrivilegeUnlock}
          privilegeMaterial={privilegeMaterial}
        />
      </section>
    );
  }

  function renderRolloutsPanel() {
    return (
      <section className="workspace singleColumn">
        <RolloutsPanel
          agents={dashboard.agents}
          jobs={dashboard.jobs}
          onCancelJob={dashboard.cancelJob}
          onLoadRollouts={dashboard.loadJobRollouts}
          onOpenJobDetails={openJobDetails}
          onUpdateRollout={dashboard.updateJobRollout}
          rollouts={dashboard.jobRollouts}
          rolloutsTruncated={dashboard.jobRolloutsTruncated}
        />
      </section>
    );
  }

  function renderRunbooksPanel() {
    return (
      <RunbooksPanel
        agents={dashboard.agents}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        jobs={dashboard.jobs}
        loading={dashboard.jobsLoading}
        onOpenDispatchPreset={openJobDispatchPreset}
        onOpenJobsDispatch={() => selectView("Jobs", "dispatch")}
        onOpenRemoteTerminal={() => selectView("Remote Operations", "terminal")}
        onOpenSchedules={() => selectView("Automation", "schedules")}
        onRefresh={dashboard.loadJobs}
      />
    );
  }

  function renderFleetMetricsPanel() {
    return (
      <FleetMetricsPanel
        agents={visibleAgents}
        error={dashboard.dashboardOverviewError}
        loading={dashboard.dashboardOverviewLoading}
        onPreferencesChange={dashboard.updateDashboardPreferences}
        onRefresh={() => void dashboard.loadDashboardOverview()}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onWindowChange={dashboard.setDashboardOverviewWindow}
        overview={dashboard.dashboardOverview}
        preferences={dashboard.dashboardPreferences}
        window={dashboard.dashboardOverviewWindow}
      />
    );
  }

  function renderNetworkMetricsPanel() {
    return (
      <NetworkMetricsPanel
        networkObservations={dashboard.networkObservations}
        networkTrends={dashboard.networkTrends}
        onOpenEvidence={() => selectView("Network", "evidence")}
        onOpenOspf={() => selectView("Network", "ospf")}
        onOpenTests={() => selectView("Network", "tests")}
        ospfRecommendations={dashboard.ospfRecommendations}
        telemetryTunnels={dashboard.telemetryTunnels}
        tunnelPlans={dashboard.tunnelPlans}
      />
    );
  }

  function renderJobPanel(panelSubpage: string) {
    return (
      <JobsPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        error={dashboard.jobsError}
        jobApprovals={dashboard.jobApprovals}
        jobs={dashboard.jobs}
        schedules={dashboard.schedules}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        dispatchPreset={jobDispatchPreset}
        fileTransferSources={dashboard.fileTransferSources}
        fileTransferSourcesTruncated={dashboard.fileTransferSourcesTruncated}
        lastJobOutputEvent={dashboard.lastJobOutputEvent}
        loading={dashboard.jobsLoading}
        onApproveJobApproval={dashboard.approveJobApproval}
        onCreateJob={dashboard.createJob}
        onCreateJobApproval={dashboard.createJobApproval}
        onDownloadFileBundle={dashboard.downloadFileDownloadBundle}
        onDownloadOutputChunk={dashboard.downloadJobOutputChunk}
        onDownloadOutputStream={dashboard.downloadJobOutputStream}
        onDownloadFileForClient={dashboard.downloadFileDownloadForClient}
        onDownloadOutputArchive={dashboard.downloadJobOutputArchive}
        onDownloadTargetStatusArchive={dashboard.downloadJobTargetStatuses}
        onDownloadFileTransferSource={dashboard.downloadFileTransferSource}
        onDispatchPresetApplied={() => setJobDispatchPreset(null)}
        onLoadJob={dashboard.loadJob}
        onLoadOutputs={dashboard.loadJobOutputs}
        onLoadOutputComparison={dashboard.loadJobOutputComparison}
        onLoadTargets={dashboard.loadJobTargets}
        onSubmitTerminalInput={dashboard.submitTerminalInput}
        onOpenSchedules={() => selectView("Automation", "schedules")}
        onOpenRollout={openRolloutDetails}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onOpenRemoteOperations={(subpage) =>
          selectView("Remote Operations", subpage)
        }
        onSelectedJobDetailsOpened={() => setPendingJobDetailId(null)}
        onRefresh={dashboard.loadJobs}
        onResolveTargets={dashboard.resolveJobTargets}
        onRejectJobApproval={dashboard.rejectJobApproval}
        onSelectSubpage={(subpage) => selectReleaseDestination("Jobs", subpage)}
        onDeleteCommandTemplate={dashboard.deleteCommandTemplate}
        onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
        pendingSelectedJobId={pendingJobDetailId}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
      />
    );
  }

  function renderRemoteOperationsPanel(panelSubpage: string) {
    return (
      <RemoteOperationsPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        dispatchPreset={jobDispatchPreset}
        fileTransfers={dashboard.fileTransfers}
        fileTransfersTruncated={dashboard.fileTransfersTruncated}
        fileTransferSources={dashboard.fileTransferSources}
        fileTransferSourcesTruncated={dashboard.fileTransferSourcesTruncated}
        lastTerminalOutputEvent={dashboard.lastTerminalOutputEvent}
        loading={dashboard.jobsLoading}
        initialTargetIntent={
          workflowTargetIntent?.destination === "terminal" ||
          workflowTargetIntent?.destination === "processes"
            ? {
                clientId: workflowTargetIntent.clientId,
                destination: workflowTargetIntent.destination,
                requestId: workflowTargetIntent.requestId,
              }
            : null
        }
        onCreateFileTransferHandoff={dashboard.createFileTransferHandoff}
        onCreateJob={dashboard.createJob}
        onDownloadFileBundle={dashboard.downloadFileDownloadBundle}
        onDownloadFileTransferSource={dashboard.downloadFileTransferSource}
        onDownloadOutputChunk={dashboard.downloadJobOutputChunk}
        onDownloadOutputStream={dashboard.downloadJobOutputStream}
        onDispatchPresetApplied={() => setJobDispatchPreset(null)}
        onLoadJob={dashboard.loadJob}
        onLoadHostProcessInventory={dashboard.loadHostProcessInventory}
        onLoadHostServiceInventory={dashboard.loadHostServiceInventory}
        onLoadHostStorageInventory={dashboard.loadHostStorageInventory}
        onLoadOutputs={dashboard.loadJobOutputs}
        onLoadTargets={dashboard.loadJobTargets}
        onLoadTerminalReplay={dashboard.loadTerminalReplay}
        onInitialTargetIntentConsumed={consumeWorkflowTargetIntent}
        onOpenJobDetails={openJobDetails}
        onOpenJobsDispatch={() => selectView("Jobs", "dispatch")}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSessionEvidence={() => selectView("Audit", "sessions")}
        onRefresh={dashboard.loadJobs}
        onResolveTargets={dashboard.resolveJobTargets}
        onSaveFileTransferHandoff={dashboard.saveFileTransferHandoff}
        onSelectSubpage={(subpage) =>
          selectReleaseDestination("Remote Operations", subpage)
        }
        onSubmitTerminalInput={dashboard.submitTerminalInput}
        onTransferTargetConsumed={() => setTransferTargetIntent(null)}
        onUploadFileTransferSource={dashboard.uploadFileTransferSource}
        onDeleteCommandTemplate={dashboard.deleteCommandTemplate}
        onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
        privilegeMaterial={privilegeMaterial}
        privilegeUnlockOpen={privilegeUnlockOpen}
        processSupervisorInventory={dashboard.processSupervisorInventory}
        processSupervisorInventoryTruncated={
          dashboard.processSupervisorInventoryTruncated
        }
        setPrivilegeMaterial={setPrivilegeMaterial}
        terminalSessions={dashboard.terminalSessions}
        terminalSessionsTruncated={dashboard.terminalSessionsTruncated}
        transferTargetIntent={transferTargetIntent}
      />
    );
  }

  function renderSystemMaintenancePanel() {
    if (dashboard.operator?.role !== "admin") {
      return (
        <section className="workspace singleColumn">
          <AdminRoleBoundary
            currentRole={dashboard.operator?.role}
            detail="Control-plane cleanup and maintenance jobs can remove retained data and are intentionally visible only to admins."
            title="System maintenance"
          />
        </section>
      );
    }
    return (
      <section className="workspace singleColumn">
        <ServerJobsPanel
          error={dashboard.serverJobsError}
          jobs={dashboard.serverJobs}
          loading={dashboard.jobsLoading}
          onCancelJob={dashboard.cancelServerJob}
          onCreateCleanupJob={dashboard.createArtifactCleanupJob}
          onPreviewCleanup={dashboard.previewArtifactCleanup}
          onRefresh={dashboard.loadJobs}
        />
      </section>
    );
  }

  function renderSchedulesPanel() {
    return (
        <SchedulesPanel
        activeSubpage="registry"
        agents={dashboard.agents}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        error={dashboard.schedulesError}
        loading={dashboard.schedulesLoading}
        onApplyScheduleNow={dashboard.applyScheduleNow}
        onCreateSchedule={dashboard.createSchedule}
        onDeferSchedule={dashboard.deferSchedule}
        onDeleteSchedule={dashboard.deleteSchedule}
        onDisableSchedule={dashboard.disableSchedule}
        onEnableSchedule={dashboard.enableSchedule}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenScheduledRuns={() => selectView("Jobs", "scheduled_runs")}
        onRefresh={dashboard.loadSchedules}
        onResolveTargets={dashboard.resolveJobTargets}
        onUpdateSchedule={dashboard.updateSchedule}
        onUpdateScheduleTargets={dashboard.updateScheduleTargets}
        privilegeMaterial={privilegeMaterial}
        schedules={dashboard.schedules}
        schedulesTruncated={dashboard.schedulesTruncated}
      />
    );
  }

  function renderNetworkPanel(panelSubpage: string) {
    return (
      <TopologyPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        error={combineErrors(
          dashboard.topologyError,
          dashboard.tagsError,
          dashboard.runtimeConfigApplyError,
        )}
        jobs={dashboard.jobs}
        loading={dashboard.topologyLoading}
        initialPlanWorkflow={networkPlanWorkflowIntent}
        initialTargetIntent={
          workflowTargetIntent?.destination === "network_graph"
            ? workflowTargetIntent
            : null
        }
        networkObservations={dashboard.networkObservations}
        networkTrends={dashboard.networkTrends}
        onInitialPlanWorkflowConsumed={() => setNetworkPlanWorkflowIntent(null)}
        onInitialTargetIntentConsumed={consumeWorkflowTargetIntent}
        ospfRecommendations={dashboard.ospfRecommendations}
        ospfUpdatePlans={dashboard.ospfUpdatePlans}
        operator={dashboard.operator}
        portForwardError={dashboard.portForwardError}
        portForwardLoading={dashboard.portForwardLoading}
        portForwardRules={dashboard.portForwardRules}
        runtimeConfigEvidenceState={runtimeConfigEvidenceState}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        onAllocateTunnelEndpoints={dashboard.allocateTunnelEndpoints}
        onCreateJob={dashboard.createJob}
        onCreateTunnelPlan={dashboard.createTunnelPlan}
        onDeleteTunnelPlan={dashboard.deleteTunnelPlan}
        onExportTunnelPlan={dashboard.exportTunnelPlan}
        onLoadNetworkObservations={dashboard.loadNetworkObservations}
        onLoadNetworkTrends={dashboard.loadNetworkTrends}
        onLoadOspfRecommendations={dashboard.loadOspfRecommendations}
        onLoadOspfUpdatePlans={dashboard.loadOspfUpdatePlans}
        onLoadRuntimeConfigApplyStates={
          dashboard.loadRuntimeConfigApplyStates
        }
        onLoadSourceTemplates={dashboard.loadSourceTemplates}
        onLoadTopologyGraph={dashboard.loadTopologyGraph}
        onLoadOutputs={dashboard.loadJobOutputs}
        onLoadTargets={dashboard.loadJobTargets}
        onOpenCreateTunnelPlan={openCreateTunnelPlan}
        onOpenJobDetails={openJobDetails}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSourceTemplates={(domain) => {
          setSourceTemplateWorkflowIntent(domain);
          selectView("Automation", "source_templates");
        }}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onBulkMutatePortForwardRules={dashboard.bulkMutatePortForwardRules}
        onCreatePortForwardRule={dashboard.createPortForwardRule}
        onLoadPortForwardRules={dashboard.loadPortForwardRules}
        onMutatePortForwardRule={dashboard.mutatePortForwardRule}
        onResolvePortForwardHostname={dashboard.resolvePortForwardHostname}
        onUpdatePortForwardRule={dashboard.updatePortForwardRule}
        onSelectSubpage={(subpage) =>
          selectReleaseDestination("Network", subpage)
        }
        onRefresh={dashboard.loadTunnelPlans}
        onRefreshTunnelPlanOspfStatus={dashboard.refreshTunnelPlanOspfStatus}
        onSetTunnelPlanEnabled={dashboard.setTunnelPlanEnabled}
        onUpdateTunnelConnectionAssessment={
          dashboard.updateTunnelConnectionAssessment
        }
        onUpdateTunnelPlanOspfCost={dashboard.updateTunnelPlanOspfCost}
        onUpdateTunnelPlan={dashboard.updateTunnelPlan}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        sourceTemplates={dashboard.sourceTemplates}
        topologyGraph={dashboard.topologyGraph}
        telemetryTunnels={dashboard.telemetryTunnels}
        tunnelPlanCorruptions={dashboard.tunnelPlanCorruptions}
        tunnelPlans={dashboard.tunnelPlans}
      />
    );
  }

  function renderAuditPanel(panelSubpage: string) {
    return (
      <AuditLogPanel
        activeSubpage={panelSubpage}
        audits={dashboard.audits}
        auditsTruncated={dashboard.auditsTruncated}
        error={dashboard.auditError}
        historyExport={dashboard.historyExport}
        historyPruneResult={dashboard.historyPruneResult}
        historyRetentionPolicies={dashboard.historyRetentionPolicies}
        loading={dashboard.auditLoading}
        onExportHistory={dashboard.loadHistoryExport}
        onPruneHistoryRetention={dashboard.pruneHistoryRetention}
        onRefresh={dashboard.loadAudits}
        onUpsertHistoryRetentionPolicy={dashboard.upsertHistoryRetentionPolicy}
      />
    );
  }

  function renderBackupsPanel(panelSubpage: string) {
    return (
      <BackupsPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        artifacts={dashboard.backupArtifacts}
        backupPolicies={dashboard.backupPolicies}
        backupPoliciesTruncated={dashboard.backupPoliciesTruncated}
        backups={dashboard.backups}
        fileTransfers={dashboard.fileTransfers}
        jobs={dashboard.jobs}
        migrationLinks={dashboard.migrationLinks}
        restorePlans={dashboard.restorePlans}
        error={dashboard.backupsError}
        initialTargetIntent={
          workflowTargetIntent?.destination === "backup_requests"
            ? workflowTargetIntent
            : null
        }
        loading={dashboard.backupsLoading}
        onCreateBackupPolicy={dashboard.createBackupPolicy}
        onUpdateBackupPolicy={dashboard.updateBackupPolicy}
        onCreateJob={dashboard.createJob}
        onCreateMigrationLink={dashboard.createMigrationLink}
        onCreateMigrationRun={dashboard.createMigrationRun}
        onCreateRestorePlan={dashboard.createRestorePlan}
        onDownloadBackupArtifact={dashboard.downloadBackupArtifact}
        onHandoffBackupArtifact={dashboard.handoffBackupArtifact}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onInitialTargetIntentConsumed={consumeWorkflowTargetIntent}
        onPruneBackupPolicies={dashboard.pruneBackupPolicies}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenJobArtifacts={() => selectView("Jobs", "artifacts")}
        onOpenJobDetails={openJobDetails}
        onOpenTransfers={(clientId, path, context) => {
          setTransferTargetIntent({ clientId, context, path });
          selectView("Remote Operations", "transfers");
        }}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onRefresh={dashboard.loadBackups}
        onResolveTargets={dashboard.resolveJobTargets}
        onSelectSubpage={(subpage) => selectView("Backups", subpage)}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        onUploadBackupArtifact={dashboard.uploadBackupArtifact}
        onUploadBackupArtifactChunked={dashboard.uploadBackupArtifactChunked}
      />
    );
  }

  function renderAccessPanel(panelSubpage: string) {
    return (
      <AccessPanel
        activeSubpage={panelSubpage}
        apiToken={dashboard.apiToken}
        error={dashboard.accessError}
        gatewaySessions={dashboard.gatewaySessions}
        initialIdentityWorkflow={accessIdentityWorkflowIntent}
        lastLiveEvent={dashboard.lastLiveEvent}
        loading={dashboard.accessLoading}
        onClearSession={clearOperatorSession}
        onClearOperatorTotp={dashboard.clearOperatorTotp}
        onConfirmTotp={dashboard.confirmTotp}
        onCreateOperator={dashboard.createOperator}
        onUpsertAgentIdentity={dashboard.upsertAgentIdentity}
        onDisableTotp={dashboard.disableTotp}
        onInitialIdentityWorkflowConsumed={() =>
          setAccessIdentityWorkflowIntent(null)
        }
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSystemConfig={() => selectView("System", "suite_config")}
        onOpenSystemSessions={() => selectView("Audit", "sessions")}
        onOpenTerminalSessions={() =>
          selectView("Remote Operations", "terminal")
        }
        onRefresh={dashboard.loadCurrentOperator}
        onResetOperatorPassword={dashboard.resetOperatorPassword}
        onRevokeClientKey={dashboard.revokeClientKey}
        onRevokeOperatorSession={dashboard.revokeOperatorSession}
        onSelectSubpage={(subpage) => selectView("Access", subpage)}
        onSetOperatorStatus={dashboard.setOperatorStatus}
        onSetupTotp={dashboard.setupTotp}
        onUpdateOperator={dashboard.updateOperator}
        onUpdateOperatorPreferences={dashboard.updateOperatorPreferences}
        operator={dashboard.operator}
        operatorAuthEvents={dashboard.operatorAuthEvents}
        operatorSessions={dashboard.operatorSessions}
        operators={dashboard.operators}
        privilegeMaterial={privilegeMaterial}
        clientKeyRevocations={dashboard.clientKeyRevocations}
        keyLifecycleReport={dashboard.keyLifecycleReport}
        setPrivilegeMaterial={setPrivilegeMaterial}
        terminalSessions={dashboard.terminalSessions}
        wsState={dashboard.wsState}
      />
    );
  }

  function renderSystemPanel(panelSubpage: string) {
    return (
      <SystemPanel
        activeSubpage={panelSubpage}
        accessError={dashboard.accessError}
        accessLoading={dashboard.accessLoading}
        dashboard={dashboard.systemDashboard}
        dashboardError={dashboard.systemDashboardError}
        dashboardLoading={dashboard.systemDashboardLoading}
        dashboardPointDensity={dashboard.systemDashboardPointDensity}
        dashboardWindow={dashboard.systemDashboardWindow}
        onDashboardPointDensityChange={dashboard.setSystemDashboardPointDensity}
        onDashboardRefresh={() => void dashboard.loadSystemDashboard()}
        onDashboardWindowChange={dashboard.setSystemDashboardWindow}
        onClearOperatorTotp={dashboard.clearOperatorTotp}
        onCreateOperator={dashboard.createOperator}
        onLoadSuiteConfig={() => void dashboard.loadSuiteConfig()}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onResetOperatorPassword={dashboard.resetOperatorPassword}
        onRevokeOperatorSession={dashboard.revokeOperatorSession}
        onSelectView={selectView}
        onSetOperatorStatus={dashboard.setOperatorStatus}
        onUpdateOperator={dashboard.updateOperator}
        onUpdateSuiteConfig={dashboard.updateSuiteConfig}
        onValidateSuiteConfig={dashboard.validateSuiteConfig}
        operator={dashboard.operator}
        operatorAuthEvents={dashboard.operatorAuthEvents}
        operatorAuthEventsTruncated={dashboard.operatorAuthEventsTruncated}
        operatorSessions={dashboard.operatorSessions}
        operatorSessionsTruncated={dashboard.operatorSessionsTruncated}
        operators={dashboard.operators}
        privilegeMaterial={privilegeMaterial}
        suiteConfig={dashboard.suiteConfig}
        suiteConfigError={dashboard.suiteConfigError}
        suiteConfigLoading={dashboard.suiteConfigLoading}
        tags={dashboard.tags}
      />
    );
  }

  function renderActivePanel() {
    if (activeView === "Home") {
      return renderHomePanel();
    }
    if (activeView === "Fleet") {
      if (activeSubpage.startsWith("instance_detail")) {
        return renderVpsDetailPanel();
      }
      if (activeSubpage === "monitor") {
        return (
          <FleetMonitorPanel
            agents={visibleAgents}
            apiError={dashboard.apiError}
            backups={dashboard.backups}
            failedJobCount={
              dashboard.jobs.filter((job) => isFailedJobStatus(job.status))
                .length
            }
            fileTransfers={dashboard.fileTransfers}
            fleetAlerts={dashboard.fleetAlerts}
            jobs={dashboard.jobs}
            recordBounds={recordPageBounds}
            runningJobCount={Math.max(
              dashboard.jobs.filter((job) => isActiveJobStatus(job.status))
                .length,
              dashboard.summary.running_jobs,
            )}
            telemetryNetworkRates={dashboard.telemetryNetworkRates}
            telemetryRollups={dashboard.telemetryRollups}
            telemetryTunnels={dashboard.telemetryTunnels}
            title="VPS cards"
            onOpenBackup={openBackupWorkflow}
            onOpenFiles={releaseRoutes.openFiles}
            onOpenNetwork={openNetworkWorkflow}
            onOpenProcesses={releaseRoutes.openProcess}
            onOpenTerminal={releaseRoutes.openTerminal}
            onOpenVpsDetail={releaseRoutes.openVpsDetail}
          />
        );
      }
      if (activeSubpage.startsWith("group")) {
        return renderTagsPanel(tagPanelSubpage(activeSubpage));
      }
      if (activeSubpage === "alerts") {
        return (
          <FleetAlertsPanel
            agents={visibleAgents}
            apiError={dashboard.apiError}
            alerts={dashboard.fleetAlerts}
            onOpenAlertPolicies={() => selectView("Observability", "alerts")}
            onOpenVpsDetail={releaseRoutes.openVpsDetail}
            onUpdate={dashboard.updateFleetAlertState}
            stateCount={dashboard.fleetAlertStates.length}
          />
        );
      }
      return renderFleetWorkspace("instances");
    }
    if (activeView === "Remote Operations") {
      return renderRemoteOperationsPanel(
        remoteOperationsSubpage(activeSubpage),
      );
    }
    if (activeView === "Jobs") {
      if (activeSubpage === "artifacts") {
        return (
          <JobArtifactsPanel
            agentUpdateReleases={dashboard.agentUpdateReleases}
            agentUpdateReleasesTruncated={
              dashboard.agentUpdateReleasesTruncated
            }
            backupArtifacts={dashboard.backupArtifacts}
            backupArtifactsTruncated={dashboard.backupArtifactsTruncated}
            fileTransferSources={dashboard.fileTransferSources}
            fileTransferSourcesTruncated={
              dashboard.fileTransferSourcesTruncated
            }
            error={combineErrors(
              dashboard.jobsError,
              dashboard.backupsError,
            )}
            loading={dashboard.jobsLoading || dashboard.backupsLoading}
            onOpenAgentUpdates={() => selectView("Automation", "agent_updates")}
            onOpenBackupsArtifacts={() => selectView("Backups", "artifacts")}
            onOpenTransfers={() => selectView("Remote Operations", "transfers")}
          />
        );
      }
      return renderJobPanel(jobSubpage(activeSubpage));
    }
    if (activeView === "Automation") {
      if (activeSubpage === "rollouts") return renderRolloutsPanel();
      if (activeSubpage === "schedules") return renderSchedulesPanel();
      if (activeSubpage === "runbooks") return renderRunbooksPanel();
      if (activeSubpage === "source_templates")
        return renderSourceTemplatesPanel();
      if (activeSubpage === "os_updates") return renderOsUpdatesPanel();
      if (activeSubpage === "agent_updates") return renderAgentUpdatesPanel();
      return renderRunbooksPanel();
    }
    if (activeView === "Network") {
      return renderNetworkPanel(networkSubpage(activeSubpage));
    }
    if (activeView === "Backups") {
      return renderBackupsPanel(activeSubpage);
    }
    if (activeView === "Config") {
      return renderConfigPanel(configSubpage(activeSubpage));
    }
    if (activeView === "Observability") {
      if (activeSubpage === "fleet_metrics") return renderFleetMetricsPanel();
      if (activeSubpage === "network_metrics")
        return renderNetworkMetricsPanel();
      if (
        activeSubpage === "alerts" ||
        activeSubpage.startsWith("alerts:policy:")
      )
        return renderAlertsPanel();
      if (activeSubpage === "webhooks") return renderWebhooksPanel();
      if (activeSubpage === "dashboards")
        return renderObservabilityDashboardsPanel();
      return renderFleetMetricsPanel();
    }
    if (activeView === "Audit") {
      if (activeSubpage === "events") return renderAuditPanel("events");
      if (activeSubpage === "job_evidence") {
        return (
          <JobEvidencePanel
            agents={dashboard.agents}
            audits={dashboard.audits}
            auditsTruncated={dashboard.auditsTruncated}
            error={dashboard.jobsError ?? dashboard.auditError}
            jobs={dashboard.jobs}
            jobsTruncated={dashboard.jobsTruncated}
            loading={dashboard.jobsLoading || dashboard.auditLoading}
            onLoadJobOutputs={dashboard.loadJobOutputs}
            onLoadJobTargets={dashboard.loadJobTargets}
            onOpenJobDetails={openJobDetails}
            onRefresh={() => {
              void dashboard.loadJobs();
              void dashboard.loadAudits();
            }}
          />
        );
      }
      if (activeSubpage === "retention_export")
        return renderAuditPanel("retention");
      if (activeSubpage === "sessions") {
        return (
          <SessionEvidencePanel
            agents={dashboard.agents}
            audits={dashboard.audits}
            auditsTruncated={dashboard.auditsTruncated}
            jobs={dashboard.jobs}
            jobsTruncated={dashboard.jobsTruncated}
            loading={
              dashboard.jobsLoading ||
              dashboard.auditLoading ||
              dashboard.accessLoading
            }
            onRefresh={() => {
              void dashboard.loadAudits();
              void dashboard.loadJobs();
              void dashboard.loadTerminalSessions();
              void dashboard.loadCurrentOperator();
            }}
            operator={dashboard.operator}
            operatorAuthEvents={dashboard.operatorAuthEvents}
            operatorAuthEventsTruncated={
              dashboard.operatorAuthEventsTruncated
            }
            operatorSessions={dashboard.operatorSessions}
            operatorSessionsTruncated={dashboard.operatorSessionsTruncated}
            terminalSessions={dashboard.terminalSessions}
            terminalSessionsTruncated={dashboard.terminalSessionsTruncated}
          />
        );
      }
      return renderAuditPanel("events");
    }
    if (activeView === "Access") {
      if (activeSubpage === "operators") return renderSystemPanel("users");
      return renderAccessPanel(accessSubpage(activeSubpage));
    }
    if (activeView === "System") {
      if (activeSubpage === "maintenance")
        return renderSystemMaintenancePanel();
      return renderSystemPanel(systemSubpage(activeSubpage));
    }
    return null;
  }

  const authBlocked = dashboard.authRequired && !dashboard.apiToken;

  if (authBlocked) {
    return (
      <main className="authOnlyShell" aria-labelledby="operator-access-title">
        <AuthPanel
          apiError={dashboard.apiError}
          onAuth={dashboard.handleAuth}
          sessionNotice={dashboard.logoutWarning}
        />
      </main>
    );
  }

  return (
    <PanelDisplayProvider
      value={{
        preferences: operatorPreferences,
        preferencesError: dashboard.preferencesError,
        preferencesSaving: dashboard.preferencesSaving,
        setVpsNameDisplayMode: updateVpsNameDisplayMode,
        updatePreferences: dashboard.updateOperatorPreferences,
        vpsNameDisplayMode: operatorPreferences.vps_name_display_mode,
      }}
    >
      <>
        <ConsoleShell
          activeSavedFleetViewId={fleetViews.activeSavedViewId}
          activeSubpage={activeSubpage}
          activeView={activeView}
          agents={dashboard.agents}
          alertCounts={shellAlertCounts}
          apiToken={dashboard.apiToken}
          authRefreshError={dashboard.authRefreshError}
          commandItems={commandItems}
          onlineRatio={onlineRatio}
          draftSavedFleetViewName={fleetViews.draftSavedViewName}
          filteredAgentCount={visibleAgents.length}
          fleetAlertsEvidenceAvailable={
            scopedFleetAlertsEvidenceAvailable
          }
          fleetCoreEvidenceAvailable={dashboard.fleetCoreEvidenceAvailable}
          fleetQuery={fleetViews.fleetQuery}
          hideFleetStatusSummary={
            activeView === "Fleet" && activeSubpage.startsWith("instance_detail")
          }
          pageDescription={pageDescription}
          pageTitle={pageTitle}
          onApplySavedFleetView={fleetViews.applySavedFleetView}
          onClearFleetView={fleetViews.clearFleetView}
          onClearSession={clearOperatorSession}
          onDeleteSavedFleetView={fleetViews.deleteSavedFleetView}
          onFleetQueryChange={fleetViews.setFleetQuery}
          onLockPrivilege={lockPrivilege}
          onOpenAccessControls={openPrivilegeUnlock}
          onRetryAuthRefresh={() => void dashboard.retryAuthRefresh()}
          onSaveFleetView={fleetViews.saveFleetView}
          onSelectView={selectView}
          onSavedFleetViewNameChange={fleetViews.setDraftSavedViewName}
          operatorPreferencesReady={dashboard.operator !== null}
          privilegeUnlocked={privilegeMaterial !== null}
          savedFleetViews={fleetViews.savedViews}
          summary={shellSummary}
          summaryScopeLabel={summaryScopeLabel}
          wsState={dashboard.wsState}
        >
          <WorkspaceErrorBoundary
            resetKey={`${activeView}:${activeSubpage}`}
            subpageLabel={pageTitle}
            viewLabel={activeView}
          >
            <Suspense fallback={<ConsolePanelFallback view={activeView} />}>
              {renderActivePanel()}
            </Suspense>
          </WorkspaceErrorBoundary>
        </ConsoleShell>
        <PrivilegeUnlockDialog
          onClose={closePrivilegeUnlock}
          onPrivilegeMaterialChange={setPrivilegeMaterial}
          open={privilegeUnlockOpen}
        />
      </>
    </PanelDisplayProvider>
  );
}

function releaseDestination(
  view: ActiveView,
  subpage = "",
): { view: ActiveView; subpage: string } {
  if (view === "Config")
    return { view: "Config", subpage: configReleaseSubpage(subpage) };
  if (view === "Jobs") return jobReleaseDestination(subpage);
  if (view === "Fleet")
    return { view: "Fleet", subpage: normalizeFleetReleaseSubpage(subpage) };
  if (view === "Access")
    return { view: "Access", subpage: accessReleaseSubpage(subpage) };
  if (view === "System") return systemReleaseDestination(subpage);
  if (view === "Audit")
    return { view: "Audit", subpage: auditReleaseSubpage(subpage) };
  if (view === "Network")
    return { view: "Network", subpage: networkReleaseSubpage(subpage) };
  if (view === "Remote Operations")
    return { view, subpage: remoteOperationsReleaseSubpage(subpage) };
  if (view === "Automation")
    return { view, subpage: automationReleaseSubpage(subpage) };
  if (view === "Observability")
    return { view, subpage: observabilityReleaseSubpage(subpage) };
  if (view === "Home") return { view, subpage: subpage || "overview" };
  if (view === "Backups") return { view, subpage: subpage || "overview" };
  return { view: "Home", subpage: "overview" };
}

function normalizeFleetReleaseSubpage(subpage: string) {
  if (subpage.startsWith("instance_detail:")) {
    return subpage;
  }
  if (
    [
      "instances",
      "monitor",
      "groups",
      "group_assignments",
      "group_bulk",
      "alerts",
      "instance_detail",
    ].includes(subpage)
  ) {
    return subpage;
  }
  return "instances";
}

function configReleaseSubpage(subpage: string) {
  if (
    ["overview", "per_vps", "bulk_patch", "templates", "rules"].includes(
      subpage,
    )
  ) {
    return subpage;
  }
  return "overview";
}

function jobReleaseDestination(subpage: string): {
  view: ActiveView;
  subpage: string;
} {
  if (
    [
      "history",
      "dispatch",
      "approvals",
      "scheduled_runs",
      "artifacts",
    ].includes(subpage)
  ) {
    return { view: "Jobs", subpage };
  }
  return { view: "Jobs", subpage: "history" };
}

function networkReleaseSubpage(subpage: string) {
  if (
    [
      "overview",
      "graph",
      "tunnel_plans",
      "port_forwards",
      "tests",
      "ospf",
      "evidence",
    ].includes(subpage)
  ) {
    return subpage;
  }
  return "overview";
}

function remoteOperationsReleaseSubpage(subpage: string) {
  if (
    [
      "terminal",
      "files",
      "transfers",
      "processes",
      "services",
      "storage",
      "bulk_files",
    ].includes(subpage)
  ) {
    return subpage;
  }
  return "terminal";
}

function automationReleaseSubpage(subpage: string) {
  if (
    ["rollouts", "schedules", "runbooks", "source_templates", "os_updates", "agent_updates"].includes(subpage)
  ) {
    return subpage;
  }
  return "schedules";
}

function observabilityReleaseSubpage(subpage: string) {
  if (subpage.startsWith("alerts:policy:")) {
    return subpage;
  }
  if (
    [
      "fleet_metrics",
      "network_metrics",
      "alerts",
      "webhooks",
      "dashboards",
    ].includes(subpage)
  ) {
    return subpage;
  }
  return "fleet_metrics";
}

function accessReleaseSubpage(subpage: string) {
  if (
    [
      "operators",
      "vps_identities",
      "gateway_sessions",
      "privilege_vault",
    ].includes(subpage)
  ) {
    return subpage;
  }
  return "overview";
}

function systemReleaseDestination(subpage: string): {
  view: ActiveView;
  subpage: string;
} {
  if (
    ["capacity", "suite_config", "maintenance", "preferences"].includes(subpage)
  ) {
    return { view: "System", subpage };
  }
  return { view: "System", subpage: "overview" };
}

function auditReleaseSubpage(subpage: string) {
  if (["job_evidence", "sessions", "retention_export"].includes(subpage)) {
    return subpage;
  }
  return "events";
}

function isActiveJobStatus(status: string) {
  return ["queued", "dispatching", "running"].includes(status);
}

function displaySummaryForAgents(
  agents: AgentView[],
  runningJobs: number,
): FleetSummary {
  const states = agents.map((agent) => agentDisplayState(agent));
  const online = states.filter((state) => state.label === "Online").length;
  const offline = states.filter((state) => state.label === "Offline").length;
  const never = states.filter(
    (state) => state.label === "Never connected",
  ).length;
  const stale = states.filter((state) => state.label === "Stale").length;
  const unknown = agents.length - online - offline - never - stale;
  return {
    never,
    offline,
    online,
    running_jobs: runningJobs,
    stale,
    total: agents.length,
    unknown,
    warnings: states.filter(
      (state) => state.tone === "warning" || state.tone === "critical",
    ).length,
  };
}

function isFailedJobStatus(status: string) {
  return [
    "failed",
    "rejected",
    "agent_lost",
    "agent_timeout",
    "control_timeout",
    "deadline_expired",
  ].includes(status);
}

function tagPanelSubpage(subpage: string) {
  if (subpage === "group_assignments") return "assignments";
  if (subpage === "group_bulk") return "bulk";
  return "registry";
}

function configSubpage(subpage: string) {
  if (subpage === "per_vps") return "single";
  if (subpage === "bulk_patch") return "bulk";
  if (subpage.startsWith("rules:")) return subpage;
  if (subpage === "rules" || subpage === "templates") return subpage;
  return "overview";
}

function remoteOperationsSubpage(subpage: string) {
  if (subpage === "bulk_files") return "multi_files";
  if (
    ["terminal", "files", "transfers", "processes", "services", "storage"].includes(
      subpage,
    )
  )
    return subpage;
  return "terminal";
}

function jobSubpage(subpage: string) {
  if (["approvals", "dispatch", "scheduled_runs"].includes(subpage))
    return subpage;
  return "history";
}

function networkSubpage(subpage: string) {
  if (
    [
      "overview",
      "graph",
      "tunnel_plans",
      "port_forwards",
      "tests",
      "ospf",
      "evidence",
    ].includes(subpage)
  ) {
    return subpage;
  }
  return "overview";
}

function accessSubpage(subpage: string) {
  if (subpage === "vps_identities") return "clients";
  if (subpage === "gateway_sessions") return "gateway";
  if (subpage === "privilege_vault") return "privilege";
  return "overview";
}

function systemSubpage(subpage: string) {
  if (subpage === "capacity") return "capacity";
  if (subpage === "suite_config") return "config";
  if (subpage === "preferences") return "operator";
  return "dashboard";
}
