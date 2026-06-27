import {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";
import {
  ConsoleShell,
  type CommandPaletteItem,
} from "./components/ConsoleShell";
import { AuthPanel } from "./panels/AuthPanel";
import { FleetAlertsPanel } from "./panels/FleetAlertsPanel";
import { FleetMonitorPanel } from "./panels/FleetMonitorPanel";
import { JobEvidencePanel } from "./panels/audit/JobEvidencePanel";
import { SessionEvidencePanel } from "./panels/audit/SessionEvidencePanel";
import { JobArtifactsPanel } from "./panels/jobs/JobArtifactsPanel";
import { NetworkOverviewPanel } from "./panels/NetworkOverviewPanel";
import { PanelDisplayProvider } from "./panelDisplay";
import type { ActiveView, AgentView, FleetSummary } from "./types";
import type { PrivilegeMaterial } from "./privilege";
import { defaultSubpages, normalizeSubpage, viewSubpages } from "./constants";
import {
  DEFAULT_OPERATOR_PREFERENCES,
  getPageDescription,
  getPageTitle,
  setPreferredTimeZone,
  type VpsNameDisplayMode,
} from "./utils";
import { useDashboardData } from "./hooks/useDashboardData";
import { useFleetViews } from "./hooks/useFleetViews";
import { agentDisplayState } from "./agentDisplayState";
import type {
  JobDispatchPreset,
  JobDispatchPresetInput,
} from "./jobDispatchPreset";

type ReleaseRouteTarget = AgentView | string;

type ReleaseRouteHelpers = {
  openAuditEvidence: (auditId?: string) => void;
  openFiles: (target: ReleaseRouteTarget, path?: string) => void;
  openJobEvidence: (jobId: string) => void;
  openNetworkEvidence: (target?: ReleaseRouteTarget) => void;
  openProcess: (target: ReleaseRouteTarget) => void;
  openTerminal: (target: ReleaseRouteTarget) => void;
  openVpsDetail: (target: ReleaseRouteTarget) => void;
};

const HomePanel = lazy(() =>
  import("./panels/HomePanel").then((module) => ({
    default: module.HomePanel,
  })),
);
const FleetWorkspace = lazy(() =>
  import("./panels/FleetWorkspace").then((module) => ({
    default: module.FleetWorkspace,
  })),
);
const VpsDetailPanel = lazy(() =>
  import("./panels/VpsDetailPanel").then((module) => ({
    default: module.VpsDetailPanel,
  })),
);
const ConfigPanel = lazy(() =>
  import("./panels/ConfigPanel").then((module) => ({
    default: module.ConfigPanel,
  })),
);
const JobsPanel = lazy(() =>
  import("./panels/JobsPanel").then((module) => ({
    default: module.JobsPanel,
  })),
);
const RemoteOperationsPanel = lazy(() =>
  import("./panels/RemoteOperationsPanel").then((module) => ({
    default: module.RemoteOperationsPanel,
  })),
);
const ServerJobsPanel = lazy(() =>
  import("./panels/jobs/ServerJobsPanel").then((module) => ({
    default: module.ServerJobsPanel,
  })),
);
const FleetGroupsPanel = lazy(() =>
  import("./panels/FleetGroupsPanel").then((module) => ({
    default: module.FleetGroupsPanel,
  })),
);
const SchedulesPanel = lazy(() =>
  import("./panels/SchedulesPanel").then((module) => ({
    default: module.SchedulesPanel,
  })),
);
const SourceTemplatePanel = lazy(() =>
  import("./panels/SourceTemplatesPanel").then((module) => ({
    default: module.SourceTemplatePanel,
  })),
);
const AgentUpdateReleasesPanel = lazy(() =>
  import("./panels/automation/AgentUpdateReleasesPanel").then((module) => ({
    default: module.AgentUpdateReleasesPanel,
  })),
);
const RunbooksPanel = lazy(() =>
  import("./panels/automation/RunbooksPanel").then((module) => ({
    default: module.RunbooksPanel,
  })),
);
const FleetMetricsPanel = lazy(() =>
  import("./panels/observability/FleetMetricsPanel").then((module) => ({
    default: module.FleetMetricsPanel,
  })),
);
const NetworkMetricsPanel = lazy(() =>
  import("./panels/observability/NetworkMetricsPanel").then((module) => ({
    default: module.NetworkMetricsPanel,
  })),
);
const AlertsPanel = lazy(() =>
  import("./panels/observability/AlertsPanel").then((module) => ({
    default: module.AlertsPanel,
  })),
);
const WebhooksPanel = lazy(() =>
  import("./panels/observability/WebhooksPanel").then((module) => ({
    default: module.WebhooksPanel,
  })),
);
const ObservabilityDashboardsPanel = lazy(() =>
  import("./panels/observability/ObservabilityDashboardsPanel").then(
    (module) => ({
      default: module.ObservabilityDashboardsPanel,
    }),
  ),
);
const AccessPanel = lazy(() =>
  import("./panels/AccessPanel").then((module) => ({
    default: module.AccessPanel,
  })),
);
const AuditLogPanel = lazy(() =>
  import("./panels/AuditLogPanel").then((module) => ({
    default: module.AuditLogPanel,
  })),
);
const BackupsPanel = lazy(() =>
  import("./panels/BackupsPanel").then((module) => ({
    default: module.BackupsPanel,
  })),
);
const TopologyPanel = lazy(() =>
  import("./panels/TopologyPanel").then((module) => ({
    default: module.TopologyPanel,
  })),
);
const SystemPanel = lazy(() =>
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
      default:
        return "Remote operations";
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
      case "runbooks":
        return "Runbooks";
      case "source_templates":
        return "Source templates";
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
  if (view === "System") {
    switch (subpage) {
      case "suite_config":
        return "High-risk suite settings, validation, save review, and reload impact";
      case "capacity":
        return "Control-plane limits, queue posture, artifact pressure, and worker lag";
      case "maintenance":
        return "Control-plane cleanup, object-store health, and reviewed maintenance work";
      case "preferences":
        return "Personal console preferences, display defaults, and workflow presentation";
      default:
        return "Service health, queue state, key control-plane KPIs, and attention signals";
    }
  }
  if (view === "Access") {
    switch (subpage) {
      case "operators":
        return "Human operator accounts, roles, scopes, MFA posture, and session revocation";
      case "vps_identities":
        return "VPS agent identity registration, rotation, revocation, and install evidence";
      case "gateway_sessions":
        return "Gateway stream state, agent connectivity evidence, and routing readiness";
      case "privilege_vault":
        return "Local privilege unlock, vault state, lock action, and safety notes";
      default:
        return "Operator, session, identity, gateway, and privilege authority posture";
    }
  }
  if (view === "Backups") {
    switch (subpage) {
      case "requests":
        return "Backup run history and reviewed one-time backup requests";
      case "policies":
        return "Backup policy registry, retention, schedule linkage, and prune review";
      case "artifacts":
        return "Backup artifact inventory, upload, hash, size, and transfer package workflows";
      case "restore":
        return "Restore planning, execution review, verification state, and rollback";
      case "migration":
        return "Replacement VPS migration planning, restore evidence, and cutover checks";
      default:
        return "Recoverability posture, coverage gaps, restore readiness, and backup workflow entry points";
    }
  }
  if (view === "Observability") {
    switch (subpage) {
      case "network_metrics":
        return "Latency, loss, speed, tunnel grouping, endpoint comparison, and alert overlays";
      case "alerts":
        return "Alert policies, active alert context, notification channels, and delivery evidence";
      case "webhooks":
        return "Event webhook rules, tests, retained delivery evidence, and maintenance independent from alert notification destinations";
      case "dashboards":
        return "Saved read-only observability widgets and shared dashboards";
      default:
        return "CPU, memory, disk, network resource charts, grouping controls, and top VPS analysis";
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

function shortCommandId(id: string) {
  return id.length > 12 ? id.slice(0, 8) : id;
}

function releaseTargetId(target: ReleaseRouteTarget): string {
  return typeof target === "string" ? target : target.id;
}

export function App() {
  const [activeView, setActiveView] = useState<ActiveView>("Home");
  const [activeSubpages, setActiveSubpages] = useState<
    Record<ActiveView, string>
  >({ ...defaultSubpages });
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [pendingJobDetailId, setPendingJobDetailId] = useState<string | null>(
    null,
  );
  const [jobDispatchPreset, setJobDispatchPreset] =
    useState<JobDispatchPreset | null>(null);
  const [networkPlanWorkflowIntent, setNetworkPlanWorkflowIntent] = useState<
    "create" | null
  >(null);
  const [privilegeMaterial, setPrivilegeMaterial] =
    useState<PrivilegeMaterial | null>(null);
  const dashboard = useDashboardData(activeView);
  const fleetViews = useFleetViews(dashboard.agents);
  const operatorPreferences = useMemo(
    () => ({
      ...DEFAULT_OPERATOR_PREFERENCES,
      ...(dashboard.operator?.preferences ?? {}),
    }),
    [dashboard.operator?.preferences],
  );
  const visibleAgents = fleetViews.filteredAgents;
  const selectedAgent = useMemo(
    () =>
      visibleAgents.find((agent) => agent.id === selectedAgentId) ??
      visibleAgents[0] ??
      null,
    [selectedAgentId, visibleAgents],
  );
  const selectedAgentForDetail = useMemo(
    () =>
      dashboard.agents.find((agent) => agent.id === selectedAgentId) ??
      selectedAgent,
    [dashboard.agents, selectedAgent, selectedAgentId],
  );
  const visibleSummary = useMemo(
    () => displaySummaryForAgents(visibleAgents, dashboard.summary.running_jobs),
    [dashboard.summary.running_jobs, visibleAgents],
  );
  const activeSubpage = normalizeSubpage(
    activeView,
    activeSubpages[activeView],
  );
  const pageTitle = getScopedPageTitle(activeView, activeSubpage);
  const hasFleetScope =
    fleetViews.fleetQuery.trim().length > 0 ||
    fleetViews.activeSavedViewId !== null;
  const shellSummary =
    hasFleetScope || activeView === "Home" || activeView === "Fleet"
      ? visibleSummary
      : dashboard.summary;
  const summaryScopeLabel = hasFleetScope ? "Current scope" : "Entire fleet";
  const onlineRatio = useMemo(() => {
    if (shellSummary.total === 0) {
      return "0%";
    }
    return `${Math.round((shellSummary.online / shellSummary.total) * 100)}%`;
  }, [shellSummary.online, shellSummary.total]);
  const pageDescription =
    activeView === "Fleet" && hasFleetScope
      ? `${visibleSummary.online} visible online / ${visibleSummary.total} visible / ${dashboard.summary.total} total`
      : activeView === "Fleet"
        ? `${visibleSummary.online} online / ${visibleSummary.total} total`
        : getScopedPageDescription(activeView, activeSubpage);

  useEffect(() => {
    setPreferredTimeZone(operatorPreferences.timezone);
  }, [operatorPreferences.timezone]);

  function updateVpsNameDisplayMode(mode: VpsNameDisplayMode) {
    void dashboard.updateOperatorPreferences({
      ...operatorPreferences,
      vps_name_display_mode: mode,
    });
  }

  function selectView(view: ActiveView, subpage?: string) {
    setActiveView(view);
    if (subpage) {
      setActiveSubpages((current) => ({
        ...current,
        [view]: normalizeSubpage(view, subpage),
      }));
    }
  }

  function selectSubpage(subpage: string) {
    setActiveSubpages((current) => ({
      ...current,
      [activeView]: normalizeSubpage(activeView, subpage),
    }));
  }

  function selectReleaseDestination(view: ActiveView, subpage?: string) {
    const destination = releaseDestination(view, subpage);
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

  function openJobDispatchPreset(preset: JobDispatchPresetInput) {
    setJobDispatchPreset({
      ...preset,
      requestId: crypto.randomUUID(),
    });
    selectView("Jobs", "dispatch");
  }

  function openPrivilegeUnlock() {
    selectView("Access", "privilege_vault");
  }

  function lockPrivilege() {
    setPrivilegeMaterial(null);
  }

  function openVpsDetail(target: ReleaseRouteTarget) {
    setSelectedAgentId(releaseTargetId(target));
    selectView("Fleet", "instance_detail");
  }

  function openRemoteTerminal(target: ReleaseRouteTarget) {
    setSelectedAgentId(releaseTargetId(target));
    selectView("Remote Operations", "terminal");
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
    setSelectedAgentId(releaseTargetId(target));
    selectView("Remote Operations", "processes");
  }

  function openBackupWorkflow(agent: AgentView) {
    setSelectedAgentId(agent.id);
    selectView("Backups", "requests");
  }

  function openNetworkWorkflow(agent: AgentView) {
    setSelectedAgentId(agent.id);
    selectView("Network", "graph");
  }

  const openCreateTunnelPlan = useCallback(() => {
    setNetworkPlanWorkflowIntent("create");
    selectView("Network", "tunnel_plans");
  }, []);

  function openConfigWorkflow(agent: AgentView) {
    setSelectedAgentId(agent.id);
    window.localStorage.setItem("vpsman.config.single.clientId", agent.id);
    selectView("Config", "per_vps");
  }

  function openNetworkEvidence(target?: ReleaseRouteTarget) {
    if (target) {
      setSelectedAgentId(releaseTargetId(target));
    }
    selectView("Network", "evidence");
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
        label: `${view} / ${subpage.label}`,
        detail: subpage.description,
        keywords: `${view} ${subpage.id} ${subpage.label}`,
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
      detail: `${schedule.enabled ? "enabled" : "disabled"} · ${schedule.cron_expr} · ${schedule.selector_expression}`,
      keywords: `${schedule.id} ${schedule.name} ${schedule.command_type} ${schedule.selector_expression} ${schedule.target_client_ids.join(" ")}`,
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
        backupArtifacts={dashboard.backupArtifacts}
        backups={dashboard.backups}
        dashboardError={dashboard.dashboardOverviewError}
        dashboardLoading={dashboard.dashboardOverviewLoading}
        dashboardOverview={dashboard.dashboardOverview}
        dashboardPreferences={dashboard.dashboardPreferences}
        dashboardWindow={dashboard.dashboardOverviewWindow}
        fileTransfers={dashboard.fileTransfers}
        fleetAlerts={dashboard.fleetAlerts}
        jobs={dashboard.jobs}
        schedules={dashboard.schedules}
        summary={dashboard.summary}
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
      />
    );
  }

  function renderFleetWorkspace(panelSubpage: string) {
    return (
      <FleetWorkspace
        activeSubpage={panelSubpage}
        agents={visibleAgents}
        apiError={dashboard.apiError}
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
        summary={dashboard.summary}
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
        apiError={dashboard.apiError}
        audits={dashboard.audits}
        backupArtifacts={dashboard.backupArtifacts}
        backups={dashboard.backups}
        fileTransfers={dashboard.fileTransfers}
        fleetAlerts={dashboard.fleetAlerts}
        jobs={dashboard.jobs}
        loading={
          dashboard.jobsLoading ||
          dashboard.backupsLoading ||
          dashboard.topologyLoading ||
          dashboard.auditLoading ||
          dashboard.tagsLoading
        }
        networkObservations={dashboard.networkObservations}
        networkTrends={dashboard.networkTrends}
        onOpenAudit={() => selectView("Audit", "events")}
        onOpenBackup={openBackupWorkflow}
        onOpenConfig={openConfigWorkflow}
        onOpenFiles={releaseRoutes.openFiles}
        onOpenFleetAlerts={() => selectView("Fleet", "alerts")}
        onOpenInstances={() => selectView("Fleet", "instances")}
        onOpenJob={releaseRoutes.openJobEvidence}
        onOpenJobs={() => selectView("Jobs", "history")}
        onOpenNetwork={openNetworkWorkflow}
        onOpenNetworkEvidence={releaseRoutes.openNetworkEvidence}
        onOpenProcesses={releaseRoutes.openProcess}
        onOpenTerminal={releaseRoutes.openTerminal}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        sourceStatus={dashboard.sourceStatus}
        sourceTemplateAssignments={dashboard.sourceTemplateAssignments}
        summary={dashboard.summary}
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
        error={dashboard.tagsError}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        runtimeConfigPatchGenerators={dashboard.runtimeConfigPatchGenerators}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        jobs={dashboard.jobs}
        loading={dashboard.tagsLoading}
        onSubmitRuntimeConfigPatch={dashboard.submitRuntimeConfigPatch}
        onCreateJob={dashboard.createJob}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onLoadJobTargets={dashboard.loadJobTargets}
        onDeleteRuntimeConfigPatchGenerator={
          dashboard.deleteRuntimeConfigPatchGenerator
        }
        onOpenJobDetails={openJobDetails}
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
          onRenderTemplateRuntimeConfig={dashboard.renderTemplateRuntimeConfig}
          onResolveBulk={dashboard.resolveBulkPreview}
          onTestTemplate={dashboard.testSourceTemplate}
          onUpdateTemplate={dashboard.updateSourceTemplate}
          templates={dashboard.sourceTemplates}
        />
      </section>
    );
  }

  function renderAgentUpdatesPanel() {
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
          suiteConfig={dashboard.suiteConfig}
          suiteConfigError={dashboard.suiteConfigError}
          suiteConfigLoading={dashboard.suiteConfigLoading}
        />
      </section>
    );
  }

  function renderRunbooksPanel() {
    return (
      <RunbooksPanel
        agents={dashboard.agents}
        commandTemplates={dashboard.commandTemplates}
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
        error={dashboard.dashboardOverviewError}
        loading={dashboard.dashboardOverviewLoading}
        onPreferencesChange={dashboard.updateDashboardPreferences}
        onRefresh={() => void dashboard.loadDashboardOverview()}
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
        dispatchPreset={jobDispatchPreset}
        fileTransferSources={dashboard.fileTransferSources}
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
        dispatchPreset={jobDispatchPreset}
        fileTransfers={dashboard.fileTransfers}
        fileTransferSources={dashboard.fileTransferSources}
        lastTerminalOutputEvent={dashboard.lastTerminalOutputEvent}
        loading={dashboard.jobsLoading}
        onCreateFileTransferHandoff={dashboard.createFileTransferHandoff}
        onCreateJob={dashboard.createJob}
        onDownloadFileBundle={dashboard.downloadFileDownloadBundle}
        onDownloadFileTransferSource={dashboard.downloadFileTransferSource}
        onDownloadOutputChunk={dashboard.downloadJobOutputChunk}
        onDispatchPresetApplied={() => setJobDispatchPreset(null)}
        onLoadJob={dashboard.loadJob}
        onLoadOutputs={dashboard.loadJobOutputs}
        onLoadTargets={dashboard.loadJobTargets}
        onLoadTerminalReplay={dashboard.loadTerminalReplay}
        onOpenDispatchPreset={openJobDispatchPreset}
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
        onUploadFileTransferSource={dashboard.uploadFileTransferSource}
        onDeleteCommandTemplate={dashboard.deleteCommandTemplate}
        onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
        privilegeMaterial={privilegeMaterial}
        processSupervisorInventory={dashboard.processSupervisorInventory}
        setPrivilegeMaterial={setPrivilegeMaterial}
        terminalSessions={dashboard.terminalSessions}
      />
    );
  }

  function renderSystemMaintenancePanel() {
    return (
      <section className="workspace singleColumn">
        <ServerJobsPanel
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
      />
    );
  }

  function renderNetworkPanel(panelSubpage: string) {
    return (
      <TopologyPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        error={dashboard.topologyError}
        jobs={dashboard.jobs}
        loading={dashboard.topologyLoading}
        initialPlanWorkflow={networkPlanWorkflowIntent}
        networkObservations={dashboard.networkObservations}
        networkTrends={dashboard.networkTrends}
        onInitialPlanWorkflowConsumed={() => setNetworkPlanWorkflowIntent(null)}
        ospfRecommendations={dashboard.ospfRecommendations}
        ospfUpdatePlans={dashboard.ospfUpdatePlans}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        onAllocateTunnelEndpoints={dashboard.allocateTunnelEndpoints}
        onCreateJob={dashboard.createJob}
        onCreateTunnelPlan={dashboard.createTunnelPlan}
        onExportTunnelPlan={dashboard.exportTunnelPlan}
        onLoadNetworkObservations={dashboard.loadNetworkObservations}
        onLoadNetworkTrends={dashboard.loadNetworkTrends}
        onLoadOspfRecommendations={dashboard.loadOspfRecommendations}
        onLoadOspfUpdatePlans={dashboard.loadOspfUpdatePlans}
        onLoadTopologyGraph={dashboard.loadTopologyGraph}
        onLoadOutputs={dashboard.loadJobOutputs}
        onLoadTargets={dashboard.loadJobTargets}
        onOpenJobDetails={openJobDetails}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onSelectSubpage={(subpage) =>
          selectReleaseDestination("Network", subpage)
        }
        onPromoteTelemetryTunnel={dashboard.promoteTelemetryTunnel}
        onPromoteTunnelPlanToCustomAdapter={
          dashboard.promoteTunnelPlanToCustomAdapter
        }
        onRefresh={dashboard.loadTunnelPlans}
        onSubmitRuntimeConfigPatch={dashboard.submitRuntimeConfigPatch}
        onSetTunnelPlanEnabled={dashboard.setTunnelPlanEnabled}
        onUpdateTunnelPlanOspfCost={dashboard.updateTunnelPlanOspfCost}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        topologyGraph={dashboard.topologyGraph}
        telemetryTunnels={dashboard.telemetryTunnels}
        tunnelPlans={dashboard.tunnelPlans}
      />
    );
  }

  function renderAuditPanel(panelSubpage: string) {
    return (
      <AuditLogPanel
        activeSubpage={panelSubpage}
        audits={dashboard.audits}
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
        backups={dashboard.backups}
        fileTransfers={dashboard.fileTransfers}
        migrationLinks={dashboard.migrationLinks}
        restorePlans={dashboard.restorePlans}
        error={dashboard.backupsError}
        loading={dashboard.backupsLoading}
        onCreateBackupRequest={dashboard.createBackupRequest}
        onCreateBackupPolicy={dashboard.createBackupPolicy}
        onCreateJob={dashboard.createJob}
        onCreateMigrationLink={dashboard.createMigrationLink}
        onCreateMigrationRun={dashboard.createMigrationRun}
        onCreateRestorePlan={dashboard.createRestorePlan}
        onDownloadBackupArtifact={dashboard.downloadBackupArtifact}
        onHandoffBackupArtifact={dashboard.handoffBackupArtifact}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onPruneBackupPolicies={dashboard.pruneBackupPolicies}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenJobArtifacts={() => selectView("Jobs", "artifacts")}
        onOpenJobDetails={openJobDetails}
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
        lastLiveEvent={dashboard.lastLiveEvent}
        loading={dashboard.accessLoading}
        onClearSession={dashboard.clearSession}
        onConfirmTotp={dashboard.confirmTotp}
        onUpsertAgentIdentity={dashboard.upsertAgentIdentity}
        onDisableTotp={dashboard.disableTotp}
        onOpenSystemConfig={() => selectView("System", "suite_config")}
        onOpenSystemPreferences={() => selectView("System", "preferences")}
        onOpenSystemSessions={() => selectView("Audit", "sessions")}
        onOpenSystemUsers={() => selectView("Access", "operators")}
        onRefresh={dashboard.loadCurrentOperator}
        onRevokeClientKey={dashboard.revokeClientKey}
        onSelectSubpage={(subpage) => selectView("Access", subpage)}
        onSetupTotp={dashboard.setupTotp}
        operator={dashboard.operator}
        operatorSessions={dashboard.operatorSessions}
        operators={dashboard.operators}
        privilegeMaterial={privilegeMaterial}
        clientKeyRevocations={dashboard.clientKeyRevocations}
        keyLifecycleReport={dashboard.keyLifecycleReport}
        setPrivilegeMaterial={setPrivilegeMaterial}
        wsState={dashboard.wsState}
      />
    );
  }

  function renderSystemPanel(panelSubpage: string) {
    return (
      <SystemPanel
        activeSubpage={panelSubpage}
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
        operatorSessions={dashboard.operatorSessions}
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
      if (activeSubpage === "instance_detail") {
        return renderVpsDetailPanel();
      }
      if (activeSubpage === "monitor") {
        return (
          <FleetMonitorPanel
            agents={visibleAgents}
            backups={dashboard.backups}
            failedJobCount={
              dashboard.jobs.filter((job) => isFailedJobStatus(job.status))
                .length
            }
            fileTransfers={dashboard.fileTransfers}
            fleetAlerts={dashboard.fleetAlerts}
            jobs={dashboard.jobs}
            runningJobCount={
              dashboard.jobs.filter((job) => isActiveJobStatus(job.status))
                .length || dashboard.summary.running_jobs
            }
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
            backupArtifacts={dashboard.backupArtifacts}
            fileTransferSources={dashboard.fileTransferSources}
            onOpenAgentUpdates={() => selectView("Automation", "agent_updates")}
            onOpenBackupsArtifacts={() => selectView("Backups", "artifacts")}
            onOpenTransfers={() => selectView("Remote Operations", "transfers")}
          />
        );
      }
      return renderJobPanel(jobSubpage(activeSubpage));
    }
    if (activeView === "Automation") {
      if (activeSubpage === "schedules") return renderSchedulesPanel();
      if (activeSubpage === "runbooks") return renderRunbooksPanel();
      if (activeSubpage === "source_templates")
        return renderSourceTemplatesPanel();
      if (activeSubpage === "agent_updates") return renderAgentUpdatesPanel();
      return renderRunbooksPanel();
    }
    if (activeView === "Network") {
      if (activeSubpage === "overview") {
        return (
          <NetworkOverviewPanel
            networkObservations={dashboard.networkObservations}
            networkTrends={dashboard.networkTrends}
            onCreateTunnelPlan={openCreateTunnelPlan}
            onSelectSubpage={(subpage) => selectView("Network", subpage)}
            ospfRecommendations={dashboard.ospfRecommendations}
            ospfUpdatePlans={dashboard.ospfUpdatePlans}
            telemetryTunnels={dashboard.telemetryTunnels}
            tunnelPlans={dashboard.tunnelPlans}
          />
        );
      }
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
      if (activeSubpage === "alerts") return renderAlertsPanel();
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
            error={dashboard.jobsError ?? dashboard.auditError}
            jobs={dashboard.jobs}
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
            jobs={dashboard.jobs}
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
            operatorAuthEvents={dashboard.operatorAuthEvents}
            operatorSessions={dashboard.operatorSessions}
            terminalSessions={dashboard.terminalSessions}
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
          onSessionUnlock={dashboard.handleAuthVaultUnlock}
          sessionVaultAvailable={dashboard.authVaultAvailable}
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
      <ConsoleShell
        activeSavedFleetViewId={fleetViews.activeSavedViewId}
        activeSubpage={activeSubpage}
        activeView={activeView}
        agents={dashboard.agents}
        apiToken={dashboard.apiToken}
        commandItems={commandItems}
        onlineRatio={onlineRatio}
        draftSavedFleetViewName={fleetViews.draftSavedViewName}
        filteredAgentCount={visibleAgents.length}
        fleetQuery={fleetViews.fleetQuery}
        hideFleetStatusSummary={
          activeView === "Fleet" && activeSubpage === "instance_detail"
        }
        pageDescription={pageDescription}
        pageTitle={pageTitle}
        onApplySavedFleetView={fleetViews.applySavedFleetView}
        onClearFleetView={fleetViews.clearFleetView}
        onClearSession={dashboard.clearSession}
        onDeleteSavedFleetView={fleetViews.deleteSavedFleetView}
        onFleetQueryChange={fleetViews.setFleetQuery}
        onLockPrivilege={lockPrivilege}
        onOpenAccessControls={openPrivilegeUnlock}
        onSaveFleetView={fleetViews.saveFleetView}
        onSelectView={selectView}
        onSavedFleetViewNameChange={fleetViews.setDraftSavedViewName}
        operatorPreferencesReady={dashboard.operator !== null}
        privilegeUnlocked={privilegeMaterial !== null}
        savedFleetViews={fleetViews.savedViews}
        summary={shellSummary}
        summaryScopeLabel={summaryScopeLabel}
      >
        <Suspense fallback={<ConsolePanelFallback view={activeView} />}>
          {renderActivePanel()}
        </Suspense>
      </ConsoleShell>
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
    ["overview", "graph", "tunnel_plans", "tests", "ospf", "evidence"].includes(
      subpage,
    )
  ) {
    return subpage;
  }
  return "overview";
}

function remoteOperationsReleaseSubpage(subpage: string) {
  if (
    ["terminal", "files", "transfers", "processes", "bulk_files"].includes(
      subpage,
    )
  ) {
    return subpage;
  }
  return "terminal";
}

function automationReleaseSubpage(subpage: string) {
  if (
    ["schedules", "runbooks", "source_templates", "agent_updates"].includes(
      subpage,
    )
  ) {
    return subpage;
  }
  return "schedules";
}

function observabilityReleaseSubpage(subpage: string) {
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
  return {
    never: agents.filter((agent) => !agent.last_seen_at).length,
    offline: states.filter((state) => state.label === "Offline").length,
    online: states.filter((state) => state.label === "Online").length,
    running_jobs: runningJobs,
    stale: states.filter((state) => state.label === "Stale").length,
    total: agents.length,
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
  if (["terminal", "files", "transfers", "processes"].includes(subpage))
    return subpage;
  return "terminal";
}

function jobSubpage(subpage: string) {
  if (["approvals", "dispatch", "scheduled_runs"].includes(subpage))
    return subpage;
  return "history";
}

function networkSubpage(subpage: string) {
  if (subpage === "tunnel_plans") return "plans";
  if (subpage === "tests") return "apply";
  if (["graph", "evidence", "ospf"].includes(subpage)) return subpage;
  return "graph";
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
