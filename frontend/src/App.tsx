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
import { ApiResponseError, apiPost } from "./api";
import type {
  ActiveView,
  AgentView,
  FleetSummary,
  OperatorView,
} from "./types";
import {
  buildPrivilegeAssertion,
  canonicalDbPrivilegeIntent,
  derivePrivilegeMaterial,
  normalizeHex,
  type DerivedPrivilegeMaterial,
  type PrivilegeMaterial,
} from "./privilege";
import {
  defaultSubpages,
  FLEET_DETAIL_LIMIT,
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
import { parsePublicShareRouteHash } from "./publicShareRoute";
import { agentDisplayState } from "./agentDisplayState";
import type {
  JobDispatchPreset,
  JobDispatchPresetInput,
} from "./jobDispatchPreset";
import { pushHistoryEntry, replaceHistoryEntry } from "./historyEntryState";
import { retryableLazy } from "./lazyImport";
import { presentFleetAlert } from "./alertPresentation";
import { presentAudit, type AuditEvidenceReference } from "./auditPresentation";
import {
  createVpsRuleSearchContextValue,
  VpsRuleSearchProvider,
} from "./vpsRuleSearchContext";

type ReleaseRouteTarget = AgentView | string;

const PRIVILEGE_GRANT_STORAGE_KEY = "vpsman.privilegeGrant";
const PRIVILEGE_UNLOCK_ACTION = "privilege.unlock";

type PrivilegeGrant = {
  material: DerivedPrivilegeMaterial;
  operatorId: string;
};

type StoredPrivilegeGrant = PrivilegeGrant & {
  version: 1;
};

type PrivilegeVerificationResponse = {
  verified: boolean;
};

class PrivilegeVerificationDeniedError extends Error {}
class PrivilegeVerificationSupersededError extends Error {}

type StoredPrivilegeGrantRead = {
  clearInvalidRecord: boolean;
  error: string | null;
  grant: PrivilegeGrant | null;
};

function readStoredPrivilegeGrant(): StoredPrivilegeGrantRead {
  if (typeof window === "undefined") {
    return { clearInvalidRecord: false, error: null, grant: null };
  }
  let raw: string | null;
  try {
    raw = window.localStorage.getItem(PRIVILEGE_GRANT_STORAGE_KEY);
  } catch {
    return {
      clearInvalidRecord: false,
      error:
        "Browser storage is unavailable, so the saved privilege unlock could not be read. The console remains locked.",
      grant: null,
    };
  }
  if (!raw) {
    return { clearInvalidRecord: false, error: null, grant: null };
  }
  try {
    const stored = JSON.parse(raw) as Partial<StoredPrivilegeGrant>;
    if (
      stored.version !== 1 ||
      typeof stored.operatorId !== "string" ||
      !stored.operatorId.trim() ||
      typeof stored.material?.superKeyHex !== "string"
    ) {
      throw new Error("record shape is invalid");
    }
    const superKeyHex = normalizeHex(stored.material.superKeyHex);
    if (superKeyHex.length !== 64) {
      throw new Error("derived signing key is invalid");
    }
    return {
      clearInvalidRecord: false,
      error: null,
      grant: {
        material: { superKeyHex },
        operatorId: stored.operatorId,
      },
    };
  } catch {
    return {
      clearInvalidRecord: true,
      error:
        "The saved privilege unlock was invalid and has been cleared. Enter the current super password and privilege salt again.",
      grant: null,
    };
  }
}

function persistPrivilegeGrant(grant: PrivilegeGrant | null): void {
  if (typeof window === "undefined") {
    return;
  }
  if (!grant) {
    window.localStorage.removeItem(PRIVILEGE_GRANT_STORAGE_KEY);
    return;
  }
  const stored: StoredPrivilegeGrant = {
    ...grant,
    version: 1,
  };
  window.localStorage.setItem(
    PRIVILEGE_GRANT_STORAGE_KEY,
    JSON.stringify(stored),
  );
}

async function verifyPrivilegeMaterial(
  apiToken: string,
  operatorId: string,
  material: PrivilegeMaterial,
): Promise<DerivedPrivilegeMaterial> {
  const derivedMaterial = await derivePrivilegeMaterial(material);
  const intent = canonicalDbPrivilegeIntent({
    action: PRIVILEGE_UNLOCK_ACTION,
    confirmed: true,
    target: operatorId,
  });
  const privilegeAssertion = await buildPrivilegeAssertion({
    intent,
    privilegeMaterial: derivedMaterial,
    ttlSecs: 60,
  });
  try {
    const response = await apiPost<PrivilegeVerificationResponse>(
      "/api/v1/auth/privilege/verify",
      apiToken,
      { privilege_assertion: privilegeAssertion },
    );
    if (response.verified !== true) {
      throw new Error(
        "Privilege verification returned no approval. The console remains locked.",
      );
    }
    return derivedMaterial;
  } catch (error) {
    if (
      error instanceof ApiResponseError &&
      error.status === 403 &&
      error.code.startsWith("privilege_verification")
    ) {
      throw new PrivilegeVerificationDeniedError(
        "Super password or privilege salt did not match. Check both values and try again.",
      );
    }
    throw error;
  }
}

function combineErrors(
  ...errors: Array<string | null | undefined>
): string | null {
  const messages = Array.from(
    new Set(
      errors.filter((error): error is string => Boolean(error && error.trim())),
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
const SystemMaintenancePanel = retryableLazy(() =>
  import("./panels/SystemMaintenancePanel").then((module) => ({
    default: module.SystemMaintenancePanel,
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
const ConfigurationSourcesPanel = retryableLazy(() =>
  import("./panels/ConfigurationSourcesPanel").then((module) => ({
    default: module.ConfigurationSourcesPanel,
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
const PingTargetsPanel = retryableLazy(() =>
  import("./panels/observability/PingTargetsPanel").then((module) => ({
    default: module.PingTargetsPanel,
  })),
);
const SharedViewsPanel = retryableLazy(() =>
  import("./panels/observability/SharedViewsPanel").then((module) => ({
    default: module.SharedViewsPanel,
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
    switch (subpage.split(":")[0]) {
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
      case "ping_targets":
        return "Ping targets";
      case "shared_views":
        return "Shared views";
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
      case "os_updates":
        return "Native package support, reviewed update candidates, and explicit application";
      case "agent_updates":
        return "Release metadata, update checks, rollout, rollback, and job evidence";
      default:
        return "Schedules, target previews, lifecycle controls, and run evidence";
    }
  }
  if (view === "System") {
    switch (subpage.split(":")[0]) {
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
        return "Gateway installer defaults, agent connectivity, streams, and routing readiness";
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
      case "ping_targets":
        return "Reusable Ping definitions, frozen assignments, primary probes, and runtime evidence";
      case "shared_views":
        return "Persistent public monitoring views, frozen-target updates, copyable links, expiry, and access evidence";
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
    replaceHistoryEntry(url);
    return;
  }
  pushHistoryEntry(url);
}

function subpageRouteSegment(view: ActiveView, subpage: string): string {
  if (view === "Fleet" && subpage.startsWith("instance_detail:")) {
    const clientId = subpage.slice("instance_detail:".length).trim();
    return `instance-detail/${encodeURIComponent(clientId)}`;
  }
  if (view === "Jobs" && subpage.startsWith("history:job:")) {
    const jobId = subpage.slice("history:job:".length).trim();
    return `history/${encodeURIComponent(jobId)}`;
  }
  if (view === "Audit" && subpage.startsWith("events:id:")) {
    const auditId = subpage.slice("events:id:".length).trim();
    return `events/${encodeURIComponent(auditId)}`;
  }
  if (view === "Config" && subpage.startsWith("rules:id:")) {
    const clientId = subpage.slice("rules:id:".length).trim();
    return `rules/${encodeURIComponent(clientId)}`;
  }
  if (view === "Observability" && subpage.startsWith("alerts:policy:")) {
    const policyId = subpage.slice("alerts:policy:".length).trim();
    return `alert-policy/${encodeURIComponent(policyId)}`;
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
  if (view === "Jobs" && decoded === "history" && resourceSegment) {
    return `history:job:${decodeRouteSegment(resourceSegment)}`;
  }
  if (view === "Audit" && decoded === "events" && resourceSegment) {
    return `events:id:${decodeRouteSegment(resourceSegment)}`;
  }
  if (view === "Config" && decoded === "rules" && resourceSegment) {
    return `rules:id:${decodeRouteSegment(resourceSegment)}`;
  }
  if (
    view === "Observability" &&
    decoded === "alert-policy" &&
    resourceSegment
  ) {
    return `alerts:policy:${decodeRouteSegment(resourceSegment)}`;
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
  const [sidebarFocusRequest, setSidebarFocusRequest] = useState(0);
  const sidebarFocusRouteRef = useRef(
    `${initialRouteRef.current?.view ?? "Home"}:${
      initialRouteRef.current?.subpage.split(":")[0] ?? "overview"
    }`,
  );
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [workflowTargetIntent, setWorkflowTargetIntent] =
    useState<WorkflowTargetIntent | null>(null);
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
  const [networkAdapterWorkflowIntent, setNetworkAdapterWorkflowIntent] =
    useState<"runtime_tunnel" | "routing_cost" | null>(null);
  const [sharedViewSeed, setSharedViewSeed] = useState<string | null>(null);
  const [privilegeGrant, setPrivilegeGrant] = useState<PrivilegeGrant | null>(
    null,
  );
  const [storedPrivilegeGrant, setStoredPrivilegeGrant] = useState(() =>
    readStoredPrivilegeGrant(),
  );
  const [privilegeUnlockOpen, setPrivilegeUnlockOpen] = useState(false);
  const [privilegeRestoreError, setPrivilegeRestoreError] = useState<
    string | null
  >(storedPrivilegeGrant.error);
  const privilegeOperationGenerationRef = useRef(0);
  const privilegeRestoreInFlightRef = useRef<{
    key: string;
    promise: Promise<void>;
  } | null>(null);
  const closePrivilegeUnlock = useCallback(() => {
    setPrivilegeUnlockOpen(false);
    setPrivilegeRestoreError(null);
  }, []);
  const activeSubpage = normalizeSubpage(
    activeView,
    activeSubpages[activeView],
  );
  const dashboard = useDashboardData(activeView, activeSubpage);
  const privilegeAuthContextRef = useRef({
    apiToken: dashboard.apiToken,
    operatorId: dashboard.operator?.id ?? null,
  });
  privilegeAuthContextRef.current = {
    apiToken: dashboard.apiToken,
    operatorId: dashboard.operator?.id ?? null,
  };
  const privilegeMaterial =
    privilegeGrant &&
    dashboard.apiToken &&
    dashboard.operator?.id === privilegeGrant.operatorId
      ? privilegeGrant.material
      : null;
  const clearPrivilegeMaterial = useCallback(() => {
    privilegeOperationGenerationRef.current += 1;
    privilegeRestoreInFlightRef.current = null;
    persistPrivilegeGrant(null);
    setPrivilegeGrant(null);
    setStoredPrivilegeGrant({
      clearInvalidRecord: false,
      error: null,
      grant: null,
    });
  }, []);
  const setPrivilegeMaterial = useCallback(
    async (material: PrivilegeMaterial | null) => {
      if (!material) {
        clearPrivilegeMaterial();
        return;
      }
      if (!dashboard.apiToken || !dashboard.operator?.id) {
        throw new Error(
          "An authenticated operator profile is required before privilege can be verified.",
        );
      }
      const apiToken = dashboard.apiToken;
      const operatorId = dashboard.operator.id;
      const generation = privilegeOperationGenerationRef.current + 1;
      privilegeOperationGenerationRef.current = generation;
      const operationIsCurrent = () => {
        const current = privilegeAuthContextRef.current;
        return (
          privilegeOperationGenerationRef.current === generation &&
          Boolean(current.apiToken) &&
          current.operatorId === operatorId
        );
      };
      let verifiedMaterial: DerivedPrivilegeMaterial;
      try {
        verifiedMaterial = await verifyPrivilegeMaterial(
          apiToken,
          operatorId,
          material,
        );
      } catch (error) {
        if (!operationIsCurrent()) {
          throw new PrivilegeVerificationSupersededError();
        }
        throw error;
      }
      if (!operationIsCurrent()) {
        throw new PrivilegeVerificationSupersededError();
      }
      const grant = {
        material: verifiedMaterial,
        operatorId,
      };
      persistPrivilegeGrant(grant);
      setPrivilegeGrant(grant);
      setPrivilegeRestoreError(null);
    },
    [clearPrivilegeMaterial, dashboard.apiToken, dashboard.operator?.id],
  );
  useEffect(() => {
    if (!storedPrivilegeGrant.clearInvalidRecord) {
      return;
    }
    try {
      persistPrivilegeGrant(null);
    } catch {
      setPrivilegeRestoreError(
        "The saved privilege unlock is invalid, but browser storage prevented clearing it. Privilege remains locked; allow local storage and retry.",
      );
    }
  }, [storedPrivilegeGrant.clearInvalidRecord]);
  useEffect(() => {
    if (!dashboard.apiToken && dashboard.authRequired) {
      clearPrivilegeMaterial();
      setPrivilegeUnlockOpen(false);
    }
  }, [clearPrivilegeMaterial, dashboard.apiToken, dashboard.authRequired]);
  useEffect(() => {
    if (
      privilegeGrant &&
      dashboard.operator &&
      privilegeGrant.operatorId !== dashboard.operator.id
    ) {
      clearPrivilegeMaterial();
      setPrivilegeUnlockOpen(false);
    }
  }, [clearPrivilegeMaterial, dashboard.operator, privilegeGrant]);
  useEffect(() => {
    const stored = storedPrivilegeGrant.grant;
    if (!stored || !dashboard.apiToken || !dashboard.operator?.id) {
      return undefined;
    }
    if (stored.operatorId !== dashboard.operator.id) {
      persistPrivilegeGrant(null);
      setStoredPrivilegeGrant({
        clearInvalidRecord: false,
        error: null,
        grant: null,
      });
      return undefined;
    }
    let disposed = false;
    const restoreKey = `${stored.operatorId}:${stored.material.superKeyHex}`;
    let restore = privilegeRestoreInFlightRef.current;
    if (!restore || restore.key !== restoreKey) {
      restore = {
        key: restoreKey,
        promise: setPrivilegeMaterial(stored.material),
      };
      privilegeRestoreInFlightRef.current = restore;
    }
    const restorePromise = restore.promise;
    void restorePromise
      .then(() => {
        if (!disposed) {
          setStoredPrivilegeGrant({
            clearInvalidRecord: false,
            error: null,
            grant: null,
          });
        }
      })
      .catch((error: unknown) => {
        if (disposed) {
          return;
        }
        if (error instanceof PrivilegeVerificationSupersededError) {
          return;
        }
        setPrivilegeGrant(null);
        setStoredPrivilegeGrant({
          clearInvalidRecord: false,
          error: null,
          grant: null,
        });
        const denied = error instanceof PrivilegeVerificationDeniedError;
        if (denied) {
          persistPrivilegeGrant(null);
        }
        setPrivilegeRestoreError(
          denied
            ? `The saved privilege unlock is no longer accepted and was cleared. ${error.message}`
            : error instanceof Error
              ? `The saved privilege unlock could not be verified. It remains saved; refresh to retry when the verifier is available. ${error.message}`
              : "The saved privilege unlock could not be verified. It remains saved; refresh to retry when the verifier is available.",
        );
        setPrivilegeUnlockOpen(true);
      })
      .finally(() => {
        if (privilegeRestoreInFlightRef.current?.promise === restorePromise) {
          privilegeRestoreInFlightRef.current = null;
        }
      });
    return () => {
      disposed = true;
    };
  }, [
    dashboard.apiToken,
    dashboard.operator?.id,
    setPrivilegeMaterial,
    storedPrivilegeGrant.grant,
  ]);
  useEffect(() => {
    if (privilegeRestoreError && dashboard.apiToken && dashboard.operator?.id) {
      setPrivilegeUnlockOpen(true);
    }
  }, [dashboard.apiToken, dashboard.operator?.id, privilegeRestoreError]);
  useEffect(() => {
    const handlePrivilegeStorage = (event: StorageEvent) => {
      if (
        event.key !== PRIVILEGE_GRANT_STORAGE_KEY ||
        event.storageArea !== window.localStorage
      ) {
        return;
      }
      privilegeOperationGenerationRef.current += 1;
      privilegeRestoreInFlightRef.current = null;
      setPrivilegeGrant(null);
      if (event.newValue === null) {
        setStoredPrivilegeGrant({
          clearInvalidRecord: false,
          error: null,
          grant: null,
        });
        setPrivilegeRestoreError(null);
        setPrivilegeUnlockOpen(false);
        return;
      }
      const stored = readStoredPrivilegeGrant();
      setStoredPrivilegeGrant(stored);
      setPrivilegeRestoreError(stored.error);
    };
    window.addEventListener("storage", handlePrivilegeStorage);
    return () => {
      window.removeEventListener("storage", handlePrivilegeStorage);
    };
  }, []);
  const vpsRuleSearchContext = useMemo(
    () =>
      createVpsRuleSearchContextValue(
        dashboard.vpsRuleValues,
        dashboard.vpsRuleEvidenceAvailable,
      ),
    [dashboard.vpsRuleEvidenceAvailable, dashboard.vpsRuleValues],
  );
  const fleetViews = useFleetViews(dashboard.agents, vpsRuleSearchContext);
  const operatorPreferences = useMemo(
    () => sanitizeOperatorPreferences(dashboard.operator?.preferences),
    [dashboard.operator?.preferences],
  );
  const visibleAgents = fleetViews.filteredAgents;
  const monitorVisibleAgents = useMemo(
    () => visibleAgents.filter((agent) => agent.status !== "suspended"),
    [visibleAgents],
  );
  const monitorAllAgents = useMemo(
    () => dashboard.agents.filter((agent) => agent.status !== "suspended"),
    [dashboard.agents],
  );
  const suspendedClientIds = useMemo(
    () =>
      new Set(
        dashboard.agents
          .filter((agent) => agent.status === "suspended")
          .map((agent) => agent.id),
      ),
    [dashboard.agents],
  );
  const monitorNetworkObservations = useMemo(
    () =>
      dashboard.networkObservations.filter(
        (observation) =>
          !suspendedClientIds.has(observation.client_id) &&
          (observation.peer_client_id === null ||
            !suspendedClientIds.has(observation.peer_client_id)),
      ),
    [dashboard.networkObservations, suspendedClientIds],
  );
  const monitorNetworkTrends = useMemo(
    () =>
      dashboard.networkTrends.filter(
        (trend) =>
          !suspendedClientIds.has(trend.client_id) &&
          (trend.peer_client_id === null ||
            !suspendedClientIds.has(trend.peer_client_id)),
      ),
    [dashboard.networkTrends, suspendedClientIds],
  );
  const monitorOspfRecommendations = useMemo(
    () =>
      dashboard.ospfRecommendations.filter(
        (recommendation) =>
          !suspendedClientIds.has(recommendation.left_client_id) &&
          !suspendedClientIds.has(recommendation.right_client_id),
      ),
    [dashboard.ospfRecommendations, suspendedClientIds],
  );
  const monitorTelemetryTunnels = useMemo(
    () =>
      dashboard.telemetryTunnels.filter(
        (tunnel) =>
          !suspendedClientIds.has(tunnel.client_id) &&
          (tunnel.peer_client_id === null ||
            !suspendedClientIds.has(tunnel.peer_client_id)),
      ),
    [dashboard.telemetryTunnels, suspendedClientIds],
  );
  const monitorTunnelPlans = useMemo(
    () =>
      dashboard.tunnelPlans.filter(
        (plan) =>
          !suspendedClientIds.has(plan.left_client_id) &&
          !suspendedClientIds.has(plan.right_client_id),
      ),
    [dashboard.tunnelPlans, suspendedClientIds],
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
      ? (dashboard.agents.find((agent) => agent.id === clientId) ?? null)
      : null;
  }, [activeSubpage, activeView, dashboard.agents]);
  useEffect(() => {
    if (!dashboard.fleetCoreEvidenceAvailable) {
      return;
    }
    const visibleClientIds = new Set(dashboard.agents.map((agent) => agent.id));
    setSelectedAgentId((current) =>
      current && !visibleClientIds.has(current) ? null : current,
    );
    setWorkflowTargetIntent((current) =>
      current && !visibleClientIds.has(current.clientId) ? null : current,
    );
    setTransferTargetIntent((current) =>
      current && !visibleClientIds.has(current.clientId) ? null : current,
    );
    const detailClientId =
      activeView === "Fleet" ? fleetDetailClientId(activeSubpage) : null;
    if (!detailClientId || visibleClientIds.has(detailClientId)) {
      return;
    }
    setActiveSubpages((current) => ({
      ...current,
      Fleet: "instances",
    }));
    requestSidebarFocus("Fleet", "instances");
    writeConsoleRoute("Fleet", "instances", "replace");
  }, [
    activeSubpage,
    activeView,
    dashboard.agents,
    dashboard.fleetCoreEvidenceAvailable,
  ]);
  const visibleSummary = useMemo(
    () =>
      displaySummaryForAgents(visibleAgents, dashboard.summary.running_jobs),
    [dashboard.summary.running_jobs, visibleAgents],
  );
  const monitorVisibleSummary = useMemo(
    () =>
      displaySummaryForAgents(
        monitorVisibleAgents,
        dashboard.summary.running_jobs,
      ),
    [dashboard.summary.running_jobs, monitorVisibleAgents],
  );
  const monitorAllSummary = useMemo(
    () =>
      displaySummaryForAgents(monitorAllAgents, dashboard.summary.running_jobs),
    [dashboard.summary.running_jobs, monitorAllAgents],
  );
  const pageTitle = getScopedPageTitle(activeView, activeSubpage);
  const hasFleetScope =
    fleetViews.fleetQuery.trim().length > 0 ||
    fleetViews.activeSavedViewId !== null;
  const canManageAlertLifecycle = operatorCanManageAlertLifecycle(
    dashboard.operator,
  );
  const canManageAlertEventSchedules = operatorCanManageAlertEventSchedules(
    dashboard.operator,
  );
  const runtimeConfigEvidenceState = dashboard.runtimeConfigApplyLoading
    ? "loading"
    : dashboard.runtimeConfigApplyEvidenceAvailable
      ? "available"
      : "unavailable";
  const configInventoryEvidenceState =
    dashboard.runtimeConfigPatchGeneratorsLoading
    ? "loading"
    : dashboard.runtimeConfigPatchGeneratorsEvidenceAvailable &&
        dashboard.runtimeConfigPatchGeneratorsError === null
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
    fleetAlerts: dashboard.fleetAlertsTruncated,
    jobs: dashboard.jobsTruncated,
    schedules: dashboard.schedulesTruncated,
  };
  const shellSummary =
    activeView === "Home"
      ? monitorVisibleSummary
      : activeView === "Observability" &&
          (activeSubpage === "fleet_metrics" ||
            activeSubpage === "network_metrics" ||
            activeSubpage === "dashboards")
        ? hasFleetScope
          ? monitorVisibleSummary
          : monitorAllSummary
        : hasFleetScope || activeView === "Fleet"
          ? visibleSummary
          : dashboard.summary;
  const summaryScopeLabel = hasFleetScope ? "Current scope" : "Entire fleet";
  const shellAlertCounts = useMemo(() => {
    const scopedClientIds = new Set(visibleAgents.map((agent) => agent.id));
    const activeAlerts = dashboard.fleetAlerts.filter(
      (alert) =>
        presentFleetAlert(alert).actionable &&
        (!hasFleetScope ||
          (alert.client_id !== null && scopedClientIds.has(alert.client_id))),
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
      scopeIncomplete: hasFleetScope && dashboard.fleetAlertsTruncated,
      total: activeAlerts.length,
      truncated: dashboard.fleetAlertsTruncated,
      warning,
    };
  }, [
    dashboard.fleetAlerts,
    dashboard.fleetAlertsTruncated,
    hasFleetScope,
    visibleAgents,
  ]);
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
          alert.client_id !== null && scopedClientIds.has(alert.client_id),
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
    activeView === "Fleet" && activeSubpage === "monitor"
      ? getScopedPageDescription(activeView, activeSubpage)
      : activeView === "Fleet" && !dashboard.fleetCoreEvidenceAvailable
        ? "Fleet inventory evidence unavailable; retry before assuming the fleet is empty"
        : activeView === "Fleet" && hasFleetScope
          ? `${visibleSummary.online} visible live / ${visibleSummary.offline} offline / ${visibleSummary.stale} stale / ${visibleSummary.suspended} suspended / ${visibleSummary.revoked} access revoked / ${visibleSummary.never + visibleSummary.unknown} no contact / ${visibleSummary.total} visible / ${dashboard.summary.total} total`
          : activeView === "Fleet"
            ? `${visibleSummary.online} live / ${visibleSummary.offline} offline / ${visibleSummary.stale} stale / ${visibleSummary.suspended} suspended / ${visibleSummary.revoked} access revoked / ${visibleSummary.never + visibleSummary.unknown} no contact / ${visibleSummary.total} total`
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
      if (parsePublicShareRouteHash(window.location.hash)) {
        return;
      }
      const route = readConsoleRouteFromLocation();
      if (!route) {
        setActiveView("Home");
        setActiveSubpages((current) => ({
          ...current,
          Home: "overview",
        }));
        requestSidebarFocus("Home", "overview");
        writeConsoleRoute("Home", "overview", "replace");
        return;
      }
      setActiveView(route.view);
      setActiveSubpages((current) => ({
        ...current,
        [route.view]: route.subpage,
      }));
      requestSidebarFocus(route.view, route.subpage);
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
    requestSidebarFocus(view, nextSubpage);
    writeConsoleRoute(view, nextSubpage);
  }

  function requestSidebarFocus(view: ActiveView, subpage: string) {
    const routeKey = `${view}:${subpage.split(":")[0]}`;
    if (sidebarFocusRouteRef.current === routeKey) {
      return;
    }
    sidebarFocusRouteRef.current = routeKey;
    setSidebarFocusRequest((current) => current + 1);
  }

  function selectSubpage(subpage: string) {
    const nextSubpage = normalizeSubpage(activeView, subpage);
    setWorkflowTargetIntent(null);
    setActiveSubpages((current) => ({
      ...current,
      [activeView]: nextSubpage,
    }));
    requestSidebarFocus(activeView, nextSubpage);
    writeConsoleRoute(activeView, nextSubpage);
  }

  function openRolloutDetails(jobId: string) {
    selectView("Automation", "rollouts");
    const url = new URL(window.location.href);
    url.searchParams.set("rollout_job", jobId);
    replaceHistoryEntry(`${url.pathname}${url.search}${url.hash}`);
  }

  function selectReleaseDestination(
    view: ActiveView,
    subpage?: string,
    targetClientId?: string,
  ) {
    const destination = releaseDestination(view, subpage);
    if (targetClientId) {
      if (
        destination.view === "Fleet" &&
        destination.subpage === "instance_detail"
      ) {
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
      if (
        destination.view === "Backups" &&
        destination.subpage === "requests"
      ) {
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
    selectView("Jobs", `history:job:${jobId}`);
  }

  function openJobDetails(jobId: string) {
    openJobEvidence(jobId);
  }

  function openAuditEvidenceReference(reference: AuditEvidenceReference) {
    if (reference.kind === "Job") {
      openJobDetails(reference.value);
    }
  }

  function openJobHistory() {
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
    clearPrivilegeMaterial();
    setPrivilegeUnlockOpen(false);
    setPrivilegeRestoreError(null);
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

  function openAuditEvidence(auditId?: string) {
    selectView("Audit", auditId ? `events:id:${auditId}` : "events");
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
    const vpsItems = dashboard.agents.map((agent) => {
      const displayState = agentDisplayState(agent);
      return {
        id: `vps:${agent.id}`,
        group: "VPS" as const,
        label: agent.display_name || agent.id,
        detail: `${displayState.label} · ${displayState.detail} · ${agent.id}${agent.tags.length ? ` · ${agent.tags.join(", ")}` : ""}`,
        keywords: `server agent instance ${agent.id} ${agent.status} ${displayState.label} ${agent.tags.join(" ")} ${agent.last_ip ?? ""} ${agent.registration_ip ?? ""}`,
        onSelect: () => {
          fleetViews.setFleetQuery(`id:${agent.id}`);
          releaseRoutes.openVpsDetail(agent);
        },
      };
    });
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
      keywords: `${session.client_id} ${session.session_id} ${session.state} ${session.last_status} terminal_open ${session.argv.join(" ")}`,
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
    const auditItems = dashboard.audits.map((audit) => {
      const presentation = presentAudit(audit);
      return {
        id: `audit:${audit.id}`,
        group: "Audit" as const,
        label: presentation.actionLabel,
        detail: `${presentation.actorLabel} · ${presentation.targetLabel} · ${presentation.outcomeLabel}`,
        keywords: `${audit.id} ${audit.action} ${audit.target} ${presentation.actorLabel} ${presentation.outcomeLabel} ${audit.actor_id ?? ""} ${audit.command_hash ?? ""}`,
        onSelect: () => releaseRoutes.openAuditEvidence(audit.id),
      };
    });
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
      } · ${
        schedule.trigger_kind === "event"
          ? `alert edge: ${schedule.event_expression ?? "invalid"}`
          : `cron: ${schedule.cron_expr ?? "invalid"}`
      } · ${schedule.selector_expression}`,
      keywords: `${schedule.id} ${schedule.name} ${schedule.command_type} ${
        schedule.cadence_error ?? ""
      } ${schedule.event_expression ?? ""} ${schedule.cron_expr ?? ""} ${schedule.selector_expression} ${schedule.target_client_ids.join(" ")}`,
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
        agents={monitorVisibleAgents}
        allAgents={dashboard.agents}
        apiToken={dashboard.apiToken}
        auditLogs={dashboard.audits}
        backupArtifacts={homeScopedRecords.backupArtifacts}
        backups={homeScopedRecords.backups}
        backupsEvidenceAvailable={homeBackupsEvidenceAvailable}
        homeError={combineErrors(
          dashboard.jobsError,
          dashboard.backupsError,
          dashboard.auditError,
          dashboard.schedulesError,
          dashboard.systemDashboardError,
        )}
        initialMonitoringCards={dashboard.initialHomeMonitoringCards}
        initialMonitoringCardsPending={dashboard.initialHomeSnapshotPending}
        dashboardOverview={dashboard.dashboardOverview}
        dashboardPreferences={dashboard.dashboardPreferences}
        dashboardWindow={dashboard.dashboardOverviewWindow}
        fileTransfers={homeScopedRecords.fileTransfers}
        fleetError={dashboard.apiError}
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
        showCountryFlags={operatorPreferences.show_country_flags}
        scopeFiltered={hasFleetScope}
        summary={monitorVisibleSummary}
        systemDashboard={dashboard.systemDashboard}
        telemetryNetworkRates={dashboard.telemetryNetworkRates}
        telemetryError={dashboard.dashboardOverviewError}
        telemetryLoading={dashboard.dashboardOverviewLoading}
        telemetryRollups={dashboard.telemetryRollups}
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
        apiToken={dashboard.apiToken}
        apiError={dashboard.apiError}
        canManageAlertPolicies={canManageAlertLifecycle}
        fleetCoreEvidenceAvailable={dashboard.fleetCoreEvidenceAvailable}
        configurationSources={dashboard.configurationSources}
        fleetAlerts={dashboard.fleetAlerts}
        fleetPageResetKey={fleetViews.fleetQuery}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        currentPolicyAlerts={dashboard.currentPolicyAlerts}
        currentPolicyAlertsEvidenceAvailable={
          dashboard.currentPolicyAlertsEvidenceAvailable
        }
        currentPolicyAlertsTruncated={dashboard.currentPolicyAlertsTruncated}
        policyAlerts={dashboard.policyAlerts}
        policyAlertsTruncated={dashboard.policyAlertsTruncated}
        policyAlertsEvidenceAvailable={dashboard.policyAlertsEvidenceAvailable}
        trafficAccounting={dashboard.trafficAccounting}
        vpsRuleValues={dashboard.vpsRuleValues}
        fleetAlertNotificationChannels={
          dashboard.fleetAlertNotificationChannels
        }
        fleetAlertNotifications={dashboard.fleetAlertNotifications}
        fleetAlertNotificationsTruncated={
          dashboard.fleetAlertNotificationsTruncated
        }
        webhookRules={dashboard.webhookRules}
        webhookRuleDeliveries={dashboard.webhookRuleDeliveries}
        webhookRuleDeliveriesTruncated={
          dashboard.webhookRuleDeliveriesTruncated
        }
        lastLiveEvent={dashboard.lastLiveEvent}
        onCreateJob={dashboard.createJob}
        onBulkMutateTags={dashboard.bulkMutateTags}
        onDeleteAgents={dashboard.deleteAgents}
        onMutateAgentSuspensions={dashboard.mutateAgentSuspensions}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onLoadJobTargets={dashboard.loadJobTargets}
        onNavigatePanel={selectReleaseDestination}
        onRegisterVps={() => {
          setAccessIdentityWorkflowIntent("register");
          selectView("Access", "vps_identities");
        }}
        onOpenJobDispatchPreset={openJobDispatchPreset}
        onOpenJobDetails={openJobDetails}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onRefreshTagOrder={dashboard.loadTagOrder}
        onLoadEffectiveAgentConfig={dashboard.loadEffectiveAgentConfig}
        onLoadConfigurationSources={dashboard.loadConfigurationSources}
        onSelectAgent={setSelectedAgentId}
        onUpdateAgentAlias={dashboard.updateAgentAlias}
        privilegeMaterial={privilegeMaterial}
        scopeActive={hasFleetScope}
        onBulkMutateFleetAlertNotificationChannels={
          dashboard.bulkMutateFleetAlertNotificationChannels
        }
        onBulkMutateFleetAlertPolicies={dashboard.bulkMutateFleetAlertPolicies}
        onBulkMutateWebhookRules={dashboard.bulkMutateWebhookRules}
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
        onUpsertFleetAlertNotificationChannel={
          dashboard.upsertFleetAlertNotificationChannel
        }
        onUpsertFleetAlertPolicy={dashboard.upsertFleetAlertPolicy}
        onUpsertWebhookRule={dashboard.upsertWebhookRule}
        selectedAgent={selectedAgent}
        summary={visibleSummary}
        tags={dashboard.tags}
        tagOrderError={dashboard.tagOrderError}
        tagOrderLoading={dashboard.tagOrderLoading}
        targetAgents={dashboard.agents}
        telemetryNetworkRates={dashboard.telemetryNetworkRates}
        telemetryRollups={dashboard.telemetryRollups}
        telemetryTunnels={dashboard.telemetryTunnels}
        telemetryUptimes={dashboard.telemetryUptimes}
        wsState={dashboard.wsState}
      />
    );
  }

  function renderTagsPanel(panelSubpage: string) {
    const assignmentsOwnSchedules = panelSubpage === "assignments";
    return (
      <FleetGroupsPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        error={combineErrors(
          dashboard.tagOrderError,
          assignmentsOwnSchedules ? dashboard.schedulesError : null,
        )}
        loading={
          dashboard.tagOrderLoading ||
          (assignmentsOwnSchedules && dashboard.schedulesLoading)
        }
        namespaceNaturalSortEnabled={dashboard.namespaceNaturalSortEnabled}
        onAssignTag={dashboard.assignTag}
        onCreateTag={dashboard.createTag}
        onBulkMutateTags={dashboard.bulkMutateTags}
        onDeleteTag={dashboard.deleteTag}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSchedules={() => selectView("Automation", "schedules")}
        onRefresh={() => {
          void dashboard.loadTagOrder();
          if (assignmentsOwnSchedules) {
            void dashboard.loadSchedules();
          }
        }}
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
        apiToken={dashboard.apiToken}
        apiError={combineErrors(
          dashboard.apiError,
          dashboard.jobsError,
          dashboard.backupsError,
          dashboard.auditError,
          dashboard.configurationSourcesError,
          dashboard.runtimeConfigApplyError,
          dashboard.topologyError,
        )}
        audits={dashboard.audits}
        backupArtifacts={dashboard.backupArtifacts}
        backups={dashboard.backups}
        fileTransfers={dashboard.fileTransfers}
        fleetAlerts={dashboard.fleetAlerts}
        fleetAlertsEvidenceAvailable={dashboard.fleetAlertsEvidenceAvailable}
        fleetAlertsTruncated={dashboard.fleetAlertsTruncated}
        fleetAlertHistory={dashboard.fleetAlertHistory}
        fleetAlertHistoryEvidenceAvailable={
          dashboard.fleetAlertHistoryEvidenceAvailable
        }
        fleetAlertHistoryTruncated={dashboard.fleetAlertHistoryTruncated}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        jobs={dashboard.jobs}
        recordBounds={recordPageBounds}
        requestsEnabled={dashboard.documentVisible}
        loading={
          dashboard.jobsLoading ||
          dashboard.backupsLoading ||
          dashboard.topologyLoading ||
          dashboard.auditLoading ||
          dashboard.configurationSourcesLoading ||
          dashboard.runtimeConfigApplyLoading
        }
        networkObservations={dashboard.networkObservations}
        networkTrends={dashboard.networkTrends}
        onOpenAudit={releaseRoutes.openAuditEvidence}
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
        onLoadConfigurationSources={dashboard.loadConfigurationSources}
        onOpenNetwork={openNetworkWorkflow}
        onOpenNetworkEvidence={releaseRoutes.openNetworkEvidence}
        onOpenProcesses={releaseRoutes.openProcess}
        onOpenTerminal={releaseRoutes.openTerminal}
        currentPolicyAlerts={dashboard.currentPolicyAlerts}
        currentPolicyAlertsEvidenceAvailable={
          dashboard.currentPolicyAlertsEvidenceAvailable
        }
        currentPolicyAlertsTruncated={dashboard.currentPolicyAlertsTruncated}
        policyAlerts={dashboard.policyAlerts}
        policyAlertsEvidenceAvailable={dashboard.policyAlertsEvidenceAvailable}
        policyAlertsTruncated={dashboard.policyAlertsTruncated}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        runtimeConfigEvidenceState={runtimeConfigEvidenceState}
        configurationSources={dashboard.configurationSources}
        summary={visibleSummary}
        telemetryNetworkRates={dashboard.telemetryNetworkRates}
        telemetryRollups={dashboard.telemetryRollups}
        telemetryTunnels={dashboard.telemetryTunnels}
        telemetryUptimes={dashboard.telemetryUptimes}
        vpsRuleValues={dashboard.vpsRuleValues}
      />
    );
  }

  function renderConfigPanel(panelSubpage: string) {
    const overviewOwnsInventory = panelSubpage === "overview";
    const bulkOwnsPatchGenerators = panelSubpage === "bulk";
    const configReadError = overviewOwnsInventory
      ? combineErrors(
          dashboard.apiError,
          dashboard.runtimeConfigPatchGeneratorsError,
          dashboard.runtimeConfigApplyError,
          dashboard.configurationPresetsError,
          dashboard.configurationSourcesError,
          dashboard.jobsError,
        )
      : bulkOwnsPatchGenerators
        ? combineErrors(
            dashboard.apiError,
            dashboard.runtimeConfigPatchGeneratorsError,
          )
        : dashboard.apiError;
    const configReadLoading = overviewOwnsInventory
      ? dashboard.runtimeConfigPatchGeneratorsLoading ||
        dashboard.runtimeConfigApplyLoading ||
        dashboard.configurationPresetsLoading ||
        dashboard.configurationSourcesLoading ||
        dashboard.jobsLoading
      : bulkOwnsPatchGenerators
        ? dashboard.runtimeConfigPatchGeneratorsLoading
        : false;
    return (
      <ConfigPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        trafficAccounting={dashboard.trafficAccounting}
        vpsRuleValues={dashboard.vpsRuleValues}
        configurationPresets={dashboard.configurationPresets}
        configurationPresetsEvidenceState={
          dashboard.configurationPresetsLoading
            ? "loading"
            : dashboard.configurationPresetsEvidenceAvailable
              ? "available"
              : "unavailable"
        }
        configurationSources={dashboard.configurationSources}
        configurationSourcesEvidenceState={
          dashboard.configurationSourcesLoading
            ? "loading"
            : dashboard.configurationSourcesEvidenceAvailable
              ? "available"
              : "unavailable"
        }
        fleetConfigEvidenceAvailable={
          dashboard.fleetCoreEvidenceAvailable &&
          dashboard.configPolicyEvidenceAvailable
        }
        inventoryEvidenceState={configInventoryEvidenceState}
        error={configReadError}
        runtimeConfigApplyStates={dashboard.runtimeConfigApplyStates}
        runtimeConfigEvidenceState={runtimeConfigEvidenceState}
        runtimeConfigPatchGenerators={dashboard.runtimeConfigPatchGenerators}
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        jobs={dashboard.jobs}
        loading={configReadLoading}
        onApplyRuntimeConfigBulkOverride={
          dashboard.applyRuntimeConfigBulkOverride
        }
        onApplyRuntimeConfigOverride={dashboard.applyRuntimeConfigOverride}
        onCreateJob={dashboard.createJob}
        onLoadExactJobTargetStatuses={dashboard.loadExactJobTargetStatuses}
        onLoadJobOutputs={dashboard.loadJobOutputs}
        onLoadJobTargets={dashboard.loadJobTargets}
        onLoadConfigurationInventory={dashboard.loadConfigurationInventory}
        onLoadRuntimeConfigClientWorkspace={
          dashboard.loadRuntimeConfigClientWorkspace
        }
        onDeleteRuntimeConfigPatchGenerator={
          dashboard.deleteRuntimeConfigPatchGenerator
        }
        onOpenJobDetails={openJobDetails}
        onOpenJobHistory={openJobHistory}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenAlerts={() => selectView("Observability", "alerts")}
        onRefresh={
          overviewOwnsInventory
            ? async () => {
                await Promise.all([
                  dashboard.loadRuntimeConfigApplyStates(),
                  dashboard.loadRuntimeConfigPatchGenerators(),
                  dashboard.loadConfigurationInventory(),
                  dashboard.loadJobHistory(),
                ]);
              }
            : bulkOwnsPatchGenerators
              ? dashboard.loadRuntimeConfigPatchGenerators
              : null
        }
        onBulkUnsetVpsRules={dashboard.bulkUnsetVpsRules}
        onBulkUpsertVpsRules={dashboard.bulkUpsertVpsRules}
        onDryRunVpsRules={dashboard.dryRunVpsRules}
        onLoadEffectiveVpsRules={dashboard.loadEffectiveVpsRules}
        onRenderRuntimeConfigPatchGenerator={
          dashboard.renderRuntimeConfigPatchGenerator
        }
        onPreviewRuntimeConfigBulkOverride={
          dashboard.previewRuntimeConfigBulkOverride
        }
        onPreviewRuntimeConfigOverride={dashboard.previewRuntimeConfigOverride}
        onSelectSubpage={(subpage) =>
          selectReleaseDestination("Config", subpage)
        }
        onUpsertRuntimeConfigPatchGenerator={
          dashboard.upsertRuntimeConfigPatchGenerator
        }
        privilegeMaterial={privilegeMaterial}
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
        canManageAlertPolicies={canManageAlertLifecycle}
        fleetAlertNotificationChannels={
          dashboard.fleetAlertNotificationChannels
        }
        fleetAlertNotifications={dashboard.fleetAlertNotifications}
        fleetAlertNotificationsTruncated={
          dashboard.fleetAlertNotificationsTruncated
        }
        fleetAlertPolicies={dashboard.fleetAlertPolicies}
        fleetAlerts={dashboard.fleetAlerts}
        fleetAlertsEvidenceAvailable={dashboard.fleetAlertsEvidenceAvailable}
        fleetAlertsTruncated={dashboard.fleetAlertsTruncated}
        fleetAlertHistory={dashboard.fleetAlertHistory}
        fleetAlertHistoryEvidenceAvailable={
          dashboard.fleetAlertHistoryEvidenceAvailable
        }
        fleetAlertHistoryTruncated={dashboard.fleetAlertHistoryTruncated}
        onBulkMutateFleetAlertNotificationChannels={
          dashboard.bulkMutateFleetAlertNotificationChannels
        }
        onBulkMutateFleetAlertPolicies={dashboard.bulkMutateFleetAlertPolicies}
        onDispatchFleetAlertNotifications={
          dashboard.dispatchFleetAlertNotifications
        }
        onDryRunFleetAlertPolicy={dashboard.dryRunFleetAlertPolicy}
        onOpenFleetAlerts={() => selectView("Fleet", "alerts")}
        onPolicyFocusChange={(policyId) =>
          selectView(
            "Observability",
            policyId ? `alerts:policy:${policyId}` : "alerts",
          )
        }
        onProcessFleetAlertNotifications={
          dashboard.processFleetAlertNotifications
        }
        onUpsertFleetAlertNotificationChannel={
          dashboard.upsertFleetAlertNotificationChannel
        }
        onUpsertFleetAlertPolicy={dashboard.upsertFleetAlertPolicy}
        currentPolicyAlerts={dashboard.currentPolicyAlerts}
        currentPolicyAlertsEvidenceAvailable={
          dashboard.currentPolicyAlertsEvidenceAvailable
        }
        currentPolicyAlertsTruncated={dashboard.currentPolicyAlertsTruncated}
        policyFocusId={policyFocusId}
        policyAlerts={dashboard.policyAlerts}
        policyAlertsTruncated={dashboard.policyAlertsTruncated}
        policyAlertsEvidenceAvailable={dashboard.policyAlertsEvidenceAvailable}
      />
    );
  }

  function renderWebhooksPanel() {
    return (
      <WebhooksPanel
        agents={dashboard.agents}
        apiError={dashboard.apiError}
        onBulkMutateWebhookRules={dashboard.bulkMutateWebhookRules}
        onDispatchWebhookRules={dashboard.dispatchWebhookRules}
        onDryRunWebhookRule={dashboard.dryRunWebhookRule}
        onProcessWebhookRuleDeliveries={dashboard.processWebhookRuleDeliveries}
        onRotateWebhookDeliveryHistory={dashboard.rotateWebhookDeliveryHistory}
        onUpsertWebhookRule={dashboard.upsertWebhookRule}
        webhookRuleDeliveries={dashboard.webhookRuleDeliveries}
        webhookRuleDeliveriesTruncated={
          dashboard.webhookRuleDeliveriesTruncated
        }
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

  function renderConfigurationSourcesPanel() {
    return (
      <ConfigurationSourcesPanel
        agents={dashboard.agents}
        error={combineErrors(
          dashboard.configurationPresetsError,
          dashboard.configurationSourcesError,
        )}
        loading={
          dashboard.configurationPresetsLoading ||
          dashboard.configurationSourcesLoading
        }
        onApplyOverride={dashboard.applyConfigurationSourceOverride}
        onClonePreset={dashboard.cloneConfigurationPreset}
        onCreatePreset={dashboard.createConfigurationPreset}
        onDeletePreset={dashboard.deleteConfigurationPreset}
        onLoadEffectiveConfig={dashboard.loadEffectiveAgentConfig}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onPreviewOverride={dashboard.previewConfigurationSourceOverride}
        onPreviewPreset={dashboard.previewConfigurationPreset}
        onRefresh={dashboard.loadConfigurationInventory}
        onUpdatePreset={dashboard.updateConfigurationPreset}
        presets={dashboard.configurationPresets}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        sources={dashboard.configurationSources}
      />
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
          error={dashboard.jobsError}
          jobs={dashboard.jobs}
          loading={dashboard.jobsLoading}
          onCreateAgentUpdateRelease={dashboard.createAgentUpdateRelease}
          onOpenDispatchPreset={openJobDispatchPreset}
          onOpenJobDetails={openJobDetails}
          onOpenJobHistory={() => selectView("Jobs", "history")}
          onRefresh={() => {
            void dashboard.loadAgentUpdateReleases();
            void dashboard.loadJobHistory();
          }}
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
          jobHistoryError={dashboard.jobsError}
          jobHistoryLoading={dashboard.jobsLoading}
          jobs={dashboard.jobs}
          onCancelJob={dashboard.cancelJob}
          onLoadRollouts={dashboard.loadJobRollouts}
          onRetryJobHistory={dashboard.loadJobHistory}
          onOpenJobDetails={openJobDetails}
          onUpdateRollout={dashboard.updateJobRollout}
          requestsEnabled={dashboard.documentVisible}
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
        error={dashboard.jobsError}
        loading={dashboard.jobsLoading}
        onOpenDispatchPreset={openJobDispatchPreset}
        onOpenJobsDispatch={() => selectView("Jobs", "dispatch")}
        onOpenRemoteTerminal={() => selectView("Remote Operations", "terminal")}
        onOpenSchedules={() => selectView("Automation", "schedules")}
        onRefresh={() => {
          void dashboard.loadCommandTemplates();
          void dashboard.loadJobHistory();
        }}
      />
    );
  }

  function renderFleetMetricsPanel() {
    return (
      <FleetMetricsPanel
        agents={monitorVisibleAgents}
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
        agents={monitorAllAgents}
        error={dashboard.topologyError}
        networkObservations={monitorNetworkObservations}
        networkTrends={monitorNetworkTrends}
        onLoadNetworkObservations={dashboard.loadNetworkObservations}
        onLoadNetworkTrends={dashboard.loadNetworkTrends}
        onOpenEvidence={() => selectView("Network", "evidence")}
        onOpenOspf={() => selectView("Network", "ospf")}
        onOpenTests={() => selectView("Network", "tests")}
        ospfRecommendations={monitorOspfRecommendations}
        requestsEnabled={dashboard.documentVisible}
        telemetryTunnels={monitorTelemetryTunnels}
        tunnelPlans={monitorTunnelPlans}
      />
    );
  }

  function renderPingTargetsPanel() {
    return (
      <PingTargetsPanel
        agents={dashboard.agents}
        apiToken={dashboard.apiToken}
        onResolveTargets={dashboard.resolveJobTargets}
        requestsEnabled={dashboard.documentVisible}
      />
    );
  }

  function renderSharedViewsPanel() {
    return (
      <SharedViewsPanel
        agents={dashboard.agents}
        apiToken={dashboard.apiToken}
        initialSelectorExpression={sharedViewSeed ?? "*"}
        onInitialSelectorConsumed={() => setSharedViewSeed(null)}
        onResolveTargets={dashboard.resolveBulkPreview}
        requestsEnabled={dashboard.documentVisible}
      />
    );
  }

  function renderJobPanel(panelSubpage: string) {
    return (
      <JobsPanel
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        error={combineErrors(
          dashboard.jobsError,
          panelSubpage === "scheduled_runs"
            ? dashboard.schedulesError
            : null,
        )}
        jobApprovals={dashboard.jobApprovals}
        jobs={dashboard.jobs}
        schedules={dashboard.schedules}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        dispatchPreset={jobDispatchPreset}
        fileTransferSources={dashboard.fileTransferSources}
        fileTransferSourcesTruncated={dashboard.fileTransferSourcesTruncated}
        jobDetailsInvalidation={dashboard.jobDetailsInvalidation}
        loading={
          dashboard.jobsLoading ||
          (panelSubpage === "scheduled_runs" && dashboard.schedulesLoading)
        }
        onApproveJobApproval={dashboard.approveJobApproval}
        onCreateJob={dashboard.createJob}
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
        onOpenSchedules={() => selectView("Automation", "schedules")}
        onOpenRollout={openRolloutDetails}
        onOpenVpsDetail={releaseRoutes.openVpsDetail}
        onOpenRemoteOperations={(subpage) =>
          selectView("Remote Operations", subpage)
        }
        onRefresh={() => {
          if (panelSubpage === "approvals") {
            void dashboard.loadJobApprovals();
          } else if (panelSubpage === "dispatch") {
            void dashboard.loadCommandTemplates();
            void dashboard.loadFileTransferSources();
          } else {
            void dashboard.loadJobHistory();
            if (panelSubpage === "scheduled_runs") {
              void dashboard.loadSchedules();
            }
          }
        }}
        onResolveTargets={dashboard.resolveJobTargets}
        onRejectJobApproval={dashboard.rejectJobApproval}
        onSelectSubpage={(subpage) => selectReleaseDestination("Jobs", subpage)}
        onDeleteCommandTemplate={dashboard.deleteCommandTemplate}
        onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
        privilegeMaterial={privilegeMaterial}
        setPrivilegeMaterial={setPrivilegeMaterial}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
      />
    );
  }

  function renderRemoteOperationsPanel(panelSubpage: string) {
    return (
      <RemoteOperationsPanel
        accessToken={dashboard.terminalAccessToken}
        activeSubpage={panelSubpage}
        agents={dashboard.agents}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        dispatchPreset={jobDispatchPreset}
        fleetEvidenceAvailable={dashboard.fleetCoreEvidenceAvailable}
        fileTransfers={dashboard.fileTransfers}
        fileTransfersTruncated={dashboard.fileTransfersTruncated}
        fileTransferSources={dashboard.fileTransferSources}
        fileTransferSourcesTruncated={dashboard.fileTransferSourcesTruncated}
        loading={dashboard.jobsLoading}
        readError={dashboard.jobsError}
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
        onRefresh={() => {
          if (panelSubpage === "terminal") {
            void dashboard.loadTerminalSessions();
          } else if (panelSubpage === "files") {
            void dashboard.loadFileTransfers();
          } else if (panelSubpage === "transfers") {
            void dashboard.loadFileTransfers();
            void dashboard.loadFileTransferSources();
            void dashboard.loadCommandTemplates();
          } else if (panelSubpage === "processes") {
            void dashboard.loadProcessSupervisorInventory();
            void dashboard.loadFileTransferSources();
            void dashboard.loadCommandTemplates();
          }
        }}
        onResolveTargets={dashboard.resolveJobTargets}
        onSaveFileTransferHandoff={dashboard.saveFileTransferHandoff}
        onSelectSubpage={(subpage) =>
          selectReleaseDestination("Remote Operations", subpage)
        }
        onTransferTargetConsumed={() => setTransferTargetIntent(null)}
        onUploadFileTransferSource={dashboard.uploadFileTransferSource}
        onDeleteCommandTemplate={dashboard.deleteCommandTemplate}
        onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
        privilegeMaterial={privilegeMaterial}
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
      <SystemMaintenancePanel
        activeSubpage={activeSubpage}
        agents={dashboard.agents}
        apiToken={dashboard.apiToken}
        jobs={dashboard.serverJobs}
        jobsError={dashboard.serverJobsError}
        jobsLoading={dashboard.jobsLoading}
        onCancelJob={dashboard.cancelServerJob}
        onCreateCleanupJob={dashboard.createArtifactCleanupJob}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onPreviewCleanup={dashboard.previewArtifactCleanup}
        onRefreshJobs={dashboard.loadServerJobs}
        onRefreshSchedules={dashboard.loadSchedules}
        onResolveManyTargets={dashboard.resolveManyJobTargets}
        onSelectSubpage={(subpage) => selectView("System", subpage)}
        privilegeMaterial={privilegeMaterial}
        requestsEnabled={dashboard.documentVisible}
      />
    );
  }

  function renderSchedulesPanel() {
    return (
      <SchedulesPanel
        activeSubpage="registry"
        agents={dashboard.agents}
        canManageAlertEventSchedules={canManageAlertEventSchedules}
        commandTemplates={dashboard.commandTemplates}
        commandTemplatesTruncated={dashboard.commandTemplatesTruncated}
        error={combineErrors(dashboard.schedulesError, dashboard.jobsError)}
        loading={dashboard.schedulesLoading || dashboard.jobsLoading}
        onApplyScheduleNow={dashboard.applyScheduleNow}
        onCreateSchedule={dashboard.createSchedule}
        onDeferSchedule={dashboard.deferSchedule}
        onDeleteSchedule={dashboard.deleteSchedule}
        onDisableSchedule={dashboard.disableSchedule}
        onEnableSchedule={dashboard.enableSchedule}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenScheduledRuns={() => selectView("Jobs", "scheduled_runs")}
        onPreviewEventTemplate={dashboard.previewEventScheduleTemplate}
        onRefresh={async () => {
          await Promise.all([
            dashboard.loadSchedules(),
            dashboard.loadCommandTemplates(),
          ]);
        }}
        onResolveManyTargets={dashboard.resolveManyJobTargets}
        onResolveTargets={dashboard.resolveJobTargets}
        onUpdateSchedule={dashboard.updateSchedule}
        onBulkUpdateScheduleTargets={dashboard.bulkUpdateScheduleTargets}
        privilegeMaterial={privilegeMaterial}
        schedules={dashboard.schedules}
        schedulesTruncated={dashboard.schedulesTruncated}
      />
    );
  }

  function renderNetworkPanel(panelSubpage: string) {
    const topologyPanelError = combineErrors(
      dashboard.topologyError,
      panelSubpage === "evidence" ? dashboard.jobsError : null,
      panelSubpage === "tunnel_plans"
        ? dashboard.configurationSourcesError
        : null,
      panelSubpage === "graph" ? dashboard.runtimeConfigApplyError : null,
    );
    const topologyPanelLoading =
      dashboard.topologyLoading ||
      (panelSubpage === "evidence" && dashboard.jobsLoading) ||
      (panelSubpage === "tunnel_plans" &&
        dashboard.configurationSourcesLoading) ||
      (panelSubpage === "graph" && dashboard.runtimeConfigApplyLoading);
    return (
      <div className="workspace singleColumn">
        <TopologyPanel
          activeSubpage={panelSubpage}
          requestsEnabled={dashboard.documentVisible}
          agents={dashboard.agents}
          apiToken={dashboard.apiToken}
          configurationSources={dashboard.configurationSources}
          configurationSourcesEvidenceState={
            dashboard.configurationSourcesLoading
              ? "loading"
              : dashboard.configurationSourcesEvidenceAvailable
                ? "available"
                : "unavailable"
          }
          error={topologyPanelError}
          initialAdapterKind={networkAdapterWorkflowIntent}
          jobs={dashboard.jobs}
          loading={topologyPanelLoading}
          initialPlanWorkflow={networkPlanWorkflowIntent}
          initialTargetIntent={
            workflowTargetIntent?.destination === "network_graph"
              ? workflowTargetIntent
              : null
          }
          networkObservations={dashboard.networkObservations}
          networkTrends={dashboard.networkTrends}
          onInitialAdapterKindConsumed={() =>
            setNetworkAdapterWorkflowIntent(null)
          }
          onInitialPlanWorkflowConsumed={() =>
            setNetworkPlanWorkflowIntent(null)
          }
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
          onCreateNetworkAdapterDefinition={
            dashboard.createNetworkAdapterDefinition
          }
          onCreateTunnelPlan={dashboard.createTunnelPlan}
          onClearTunnelPlanEvidence={dashboard.clearTunnelPlanEvidence}
          onDeleteNetworkAdapterDefinition={
            dashboard.deleteNetworkAdapterDefinition
          }
          onDeleteTunnelPlan={dashboard.deleteTunnelPlan}
          onExportTunnelPlan={dashboard.exportTunnelPlan}
          onLoadNetworkObservations={dashboard.loadNetworkObservations}
          onLoadConfigurationSources={dashboard.loadConfigurationSources}
          onLoadNetworkTrends={dashboard.loadNetworkTrends}
          onQueryNetworkObservations={dashboard.queryNetworkObservations}
          onLoadOspfRecommendations={dashboard.loadOspfRecommendations}
          onLoadOspfUpdatePlans={dashboard.loadOspfUpdatePlans}
          onLoadRuntimeConfigApplyStates={
            dashboard.loadRuntimeConfigApplyStates
          }
          onLoadNetworkAdapterDefinitions={
            dashboard.loadNetworkAdapterDefinitions
          }
          onLoadJobHistory={dashboard.loadJobHistory}
          onLoadTopologyGraph={dashboard.loadTopologyGraph}
          onLoadOutputs={dashboard.loadJobOutputs}
          onLoadTargets={dashboard.loadJobTargets}
          onOpenCreateTunnelPlan={openCreateTunnelPlan}
          onOpenConfigurationSources={() => selectView("Config", "sources")}
          onOpenJobDetails={openJobDetails}
          onOpenJobHistory={() => selectView("Jobs", "history")}
          onOpenPrivilegeUnlock={openPrivilegeUnlock}
          onOpenAdapterDefinitions={(kind) => {
            setNetworkAdapterWorkflowIntent(kind);
            selectView("Network", "tunnel_plans");
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
          onRotateTunnelPlanCredentials={dashboard.rotateTunnelPlanCredentials}
          onSetTunnelPlanEnabled={dashboard.setTunnelPlanEnabled}
          onUpdateTunnelConnectionAssessment={
            dashboard.updateTunnelConnectionAssessment
          }
          onUpdateTunnelPlanOspfCost={dashboard.updateTunnelPlanOspfCost}
          onUpdateTunnelPlan={dashboard.updateTunnelPlan}
          onUpdateNetworkAdapterDefinition={
            dashboard.updateNetworkAdapterDefinition
          }
          networkAdapterDefinitions={dashboard.networkAdapterDefinitions}
          privilegeMaterial={privilegeMaterial}
          setPrivilegeMaterial={setPrivilegeMaterial}
          topologyGraph={dashboard.topologyGraph}
          telemetryTunnels={dashboard.telemetryTunnels}
          tunnelPlanCorruptions={dashboard.tunnelPlanCorruptions}
          tunnelPlans={dashboard.tunnelPlans}
        />
      </div>
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
        onCloseAuditEvent={() => selectView("Audit", "events")}
        onLoadAuditEvent={dashboard.loadAuditEvent}
        onOpenAuditEvent={(auditId) =>
          selectView("Audit", `events:id:${auditId}`)
        }
        onOpenEvidence={openAuditEvidenceReference}
        onPruneHistoryRetention={dashboard.pruneHistoryRetention}
        onRefresh={
          panelSubpage === "events" || panelSubpage.startsWith("events:id:")
            ? dashboard.loadAuditLogs
            : dashboard.loadAudits
        }
        onUpsertHistoryRetentionPolicy={dashboard.upsertHistoryRetentionPolicy}
      />
    );
  }

  function renderBackupsPanel(panelSubpage: string) {
    const ownsFileTransfers =
      panelSubpage === "restore" || panelSubpage === "migration";
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
        error={combineErrors(
          dashboard.backupsError,
          ownsFileTransfers ? dashboard.jobsError : null,
        )}
        initialTargetIntent={
          workflowTargetIntent?.destination === "backup_requests"
            ? workflowTargetIntent
            : null
        }
        loading={
          dashboard.backupsLoading ||
          (ownsFileTransfers && dashboard.jobsLoading)
        }
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
        onRefresh={async () => {
          if (panelSubpage === "overview") {
            await dashboard.loadBackups();
            return;
          }
          if (panelSubpage === "requests") {
            await Promise.all([
              dashboard.loadBackupRequests(),
              dashboard.loadBackupPolicies(),
              dashboard.loadBackupArtifacts(),
            ]);
            return;
          }
          if (panelSubpage === "policies") {
            await dashboard.loadBackupPolicies();
            return;
          }
          if (panelSubpage === "artifacts") {
            await Promise.all([
              dashboard.loadBackupArtifacts(),
              dashboard.loadBackupRequests(),
            ]);
            return;
          }
          const loads = [
            dashboard.loadBackupRequests(),
            dashboard.loadBackupArtifacts(),
            dashboard.loadRestorePlans(),
            dashboard.loadFileTransfers(),
          ];
          if (panelSubpage === "migration") {
            loads.push(dashboard.loadMigrationLinks());
          }
          await Promise.all(loads);
        }}
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
    const overviewOwnsTerminalSessions = panelSubpage === "overview";
    return (
      <AccessPanel
        activeSubpage={panelSubpage}
        apiToken={dashboard.apiToken}
        error={combineErrors(
          dashboard.accessError,
          overviewOwnsTerminalSessions ? dashboard.jobsError : null,
        )}
        gatewaySessions={dashboard.gatewaySessions}
        initialIdentityWorkflow={accessIdentityWorkflowIntent}
        lastLiveEvent={dashboard.lastLiveEvent}
        loading={
          dashboard.accessLoading ||
          (overviewOwnsTerminalSessions && dashboard.jobsLoading)
        }
        onClearSession={clearOperatorSession}
        onClearOperatorTotps={dashboard.clearOperatorTotps}
        onConfirmTotp={dashboard.confirmTotp}
        onCreateOperator={dashboard.createOperator}
        onUpsertAgentIdentity={dashboard.upsertAgentIdentity}
        onDisableTotp={dashboard.disableTotp}
        onInitialIdentityWorkflowConsumed={() =>
          setAccessIdentityWorkflowIntent(null)
        }
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onOpenSystemSessions={() => selectView("Audit", "sessions")}
        onOpenTerminalSessions={() =>
          selectView("Remote Operations", "terminal")
        }
        onRefresh={async () => {
          if (panelSubpage === "vps_identities") {
            await dashboard.loadAccessVpsIdentities();
            return;
          }
          if (panelSubpage === "gateway_sessions") {
            await dashboard.loadAccessGatewaySessions();
            return;
          }
          if (panelSubpage === "privilege_vault") {
            await dashboard.loadCurrentOperatorProfile();
            return;
          }
          await Promise.all([
            dashboard.loadAccessOverview(),
            dashboard.loadTerminalSessions(),
          ]);
        }}
        onResetOperatorPassword={dashboard.resetOperatorPassword}
        onRevokeClientKey={dashboard.revokeClientKey}
        onRevokeOperatorSessions={dashboard.revokeOperatorSessions}
        onSelectSubpage={(subpage) => selectView("Access", subpage)}
        onSetOperatorStatuses={dashboard.setOperatorStatuses}
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
        onClearOperatorTotps={dashboard.clearOperatorTotps}
        onCreateOperator={dashboard.createOperator}
        onLoadSuiteConfig={() => void dashboard.loadSuiteConfig()}
        onRefreshPreferencesSources={() => {
          void dashboard.loadCurrentOperatorProfile();
          void dashboard.loadTagOrder();
        }}
        onOpenPrivilegeUnlock={openPrivilegeUnlock}
        onResetOperatorPassword={dashboard.resetOperatorPassword}
        onRevokeOperatorSessions={dashboard.revokeOperatorSessions}
        onSelectView={selectView}
        onSetOperatorStatuses={dashboard.setOperatorStatuses}
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
        tagsError={dashboard.tagOrderError}
        tagsLoading={dashboard.tagOrderLoading}
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
            agents={monitorVisibleAgents}
            apiToken={dashboard.apiToken}
            apiError={combineErrors(
              dashboard.apiError,
              dashboard.jobsError,
              dashboard.backupsError,
            )}
            backups={dashboard.backups}
            failedJobCount={
              dashboard.jobs.filter((job) => isFailedJobStatus(job.status))
                .length
            }
            fileTransfers={dashboard.fileTransfers}
            fleetAlerts={dashboard.fleetAlerts}
            jobs={dashboard.jobs}
            runningJobCount={Math.max(
              dashboard.jobs.filter((job) => isActiveJobStatus(job.status))
                .length,
              dashboard.summary.running_jobs,
            )}
            showCountryFlags={operatorPreferences.show_country_flags}
            telemetryRollups={dashboard.telemetryRollups}
            title="VPS cards"
            onOpenVpsDetail={releaseRoutes.openVpsDetail}
            onOpenSharedViews={(selectorExpression) => {
              setSharedViewSeed(selectorExpression);
              selectView("Observability", "shared_views");
            }}
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
            alertsEvidenceAvailable={dashboard.fleetAlertsEvidenceAvailable}
            alertsTruncated={dashboard.fleetAlertsTruncated}
            canManageAlertLifecycle={canManageAlertLifecycle}
            eventReviewError={dashboard.fleetAlertEventReviewError}
            eventReviewHasMore={dashboard.fleetAlertEventReviewHasMore}
            eventReviewItems={dashboard.fleetAlertEventReviewItems}
            eventReviewLoading={dashboard.fleetAlertEventReviewLoading}
            eventReviewLimitNotice={dashboard.fleetAlertEventReviewLimitNotice}
            eventReviewStarted={dashboard.fleetAlertEventReviewStarted}
            eventReviewVerified={dashboard.fleetAlertEventReviewVerified}
            eventSearchHasMore={dashboard.fleetAlertEventSearchHasMore}
            eventSearchItems={dashboard.fleetAlertEventSearchItems}
            eventSearchQuery={dashboard.fleetAlertEventSearchQuery}
            eventSearchScannedCount={
              dashboard.fleetAlertEventSearchScannedCount
            }
            history={dashboard.fleetAlertHistory}
            historyEvidenceAvailable={
              dashboard.fleetAlertHistoryEvidenceAvailable
            }
            historyTruncated={dashboard.fleetAlertHistoryTruncated}
            onActivateEvents={dashboard.activateFleetAlertEventReview}
            onDeactivateEvents={dashboard.deactivateFleetAlertEventReview}
            onLoadOlderEvents={dashboard.loadOlderFleetAlertEvents}
            onSearchOlderEvents={dashboard.searchOlderFleetAlertEvents}
            onSyncEvents={dashboard.syncFleetAlertEvents}
            onOpenAlertPolicies={() => selectView("Observability", "alerts")}
            onOpenVpsDetail={releaseRoutes.openVpsDetail}
            onResolve={dashboard.resolveFleetAlert}
            onUpdateBulk={dashboard.bulkUpdateFleetAlertStates}
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
            error={combineErrors(dashboard.jobsError, dashboard.backupsError)}
            loading={dashboard.jobsLoading || dashboard.backupsLoading}
            onOpenAgentUpdates={() => selectView("Automation", "agent_updates")}
            onOpenBackupsArtifacts={() => selectView("Backups", "artifacts")}
            onOpenTransfers={() => selectView("Remote Operations", "transfers")}
          />
        );
      }
      return renderJobPanel(activeSubpage);
    }
    if (activeView === "Automation") {
      if (activeSubpage === "rollouts") return renderRolloutsPanel();
      if (activeSubpage === "schedules") return renderSchedulesPanel();
      if (activeSubpage === "runbooks") return renderRunbooksPanel();
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
      if (activeSubpage === "sources") return renderConfigurationSourcesPanel();
      return renderConfigPanel(configSubpage(activeSubpage));
    }
    if (activeView === "Observability") {
      if (activeSubpage === "fleet_metrics") return renderFleetMetricsPanel();
      if (activeSubpage === "network_metrics")
        return renderNetworkMetricsPanel();
      if (activeSubpage === "ping_targets") return renderPingTargetsPanel();
      if (activeSubpage === "shared_views") return renderSharedViewsPanel();
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
      if (activeSubpage === "events" || activeSubpage.startsWith("events:id:"))
        return renderAuditPanel(activeSubpage);
      if (activeSubpage === "job_evidence") {
        return (
          <div className="workspace singleColumn">
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
                void dashboard.loadJobHistory();
                void dashboard.loadAuditLogs();
              }}
            />
          </div>
        );
      }
      if (activeSubpage === "retention_export")
        return renderAuditPanel("retention");
      if (activeSubpage === "sessions") {
        return (
          <div className="workspace singleColumn">
            <SessionEvidencePanel
              agents={dashboard.agents}
              audits={dashboard.audits}
              auditsTruncated={dashboard.auditsTruncated}
              error={combineErrors(
                dashboard.auditError,
                dashboard.jobsError,
                dashboard.accessError,
              )}
              jobs={dashboard.jobs}
              jobsTruncated={dashboard.jobsTruncated}
              loading={
                dashboard.jobsLoading ||
                dashboard.auditLoading ||
                dashboard.accessLoading
              }
              onClearSession={clearOperatorSession}
              onOpenPrivilegeUnlock={openPrivilegeUnlock}
              onRefresh={() => {
                void dashboard.loadAuditLogs();
                void dashboard.loadJobHistory();
                void dashboard.loadTerminalSessions();
                void dashboard.loadAccessAuditSessions();
              }}
              onRevokeOperatorSessions={dashboard.revokeOperatorSessions}
              operator={dashboard.operator}
              operatorAuthEvents={dashboard.operatorAuthEvents}
              operatorAuthEventsTruncated={
                dashboard.operatorAuthEventsTruncated
              }
              operatorSessions={dashboard.operatorSessions}
              operatorSessionsTruncated={dashboard.operatorSessionsTruncated}
              privilegeMaterial={privilegeMaterial}
              terminalSessions={dashboard.terminalSessions}
              terminalSessionsTruncated={dashboard.terminalSessionsTruncated}
            />
          </div>
        );
      }
      return renderAuditPanel("events");
    }
    if (activeView === "Access") {
      if (activeSubpage === "operators") return renderSystemPanel("users");
      return renderAccessPanel(activeSubpage);
    }
    if (activeView === "System") {
      if (activeSubpage.split(":")[0] === "maintenance")
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
      <VpsRuleSearchProvider value={vpsRuleSearchContext}>
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
          fleetAlertsEvidenceAvailable={scopedFleetAlertsEvidenceAvailable}
          fleetCoreEvidenceAvailable={dashboard.fleetCoreEvidenceAvailable}
          fleetQuery={fleetViews.fleetQuery}
          hideFleetStatusSummary={
            activeView === "Fleet" &&
            (activeSubpage === "monitor" ||
              activeSubpage.startsWith("instance_detail"))
          }
          pageDescription={pageDescription}
          pageTitle={pageTitle}
          onApplySavedFleetView={fleetViews.applySavedFleetView}
          onClearFleetView={fleetViews.clearFleetView}
          onDeleteSavedFleetView={fleetViews.deleteSavedFleetView}
          onFleetQueryChange={fleetViews.setFleetQuery}
          onOpenAccessControls={openPrivilegeUnlock}
          onRetryAuthRefresh={() => void dashboard.retryAuthRefresh()}
          onSaveFleetView={fleetViews.saveFleetView}
          onSelectView={selectView}
          onSavedFleetViewNameChange={fleetViews.setDraftSavedViewName}
          operatorPreferencesReady={dashboard.operator !== null}
          privilegeUnlocked={privilegeMaterial !== null}
          savedFleetViews={fleetViews.savedViews}
          sidebarFocusRequest={sidebarFocusRequest}
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
          error={privilegeRestoreError}
          onClose={closePrivilegeUnlock}
          onPrivilegeMaterialChange={setPrivilegeMaterial}
          open={privilegeUnlockOpen}
        />
      </VpsRuleSearchProvider>
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
  if (subpage.startsWith("rules:id:")) {
    return subpage;
  }
  if (
    ["overview", "sources", "per_vps", "bulk_patch", "rules"].includes(subpage)
  ) {
    return subpage;
  }
  return "overview";
}

function jobReleaseDestination(subpage: string): {
  view: ActiveView;
  subpage: string;
} {
  if (subpage.startsWith("history:job:")) {
    return { view: "Jobs", subpage };
  }
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
    [
      "rollouts",
      "schedules",
      "runbooks",
      "os_updates",
      "agent_updates",
    ].includes(subpage)
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
      "ping_targets",
      "shared_views",
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
    ["capacity", "suite_config", "preferences"].includes(subpage) ||
    subpage.split(":")[0] === "maintenance"
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

function operatorCanManageAlertLifecycle(
  operator: OperatorView | null,
): boolean {
  const scopes = new Set(operator?.scopes ?? []);
  const hasAllScopes = scopes.has("*");
  return Boolean(
    operator &&
    (operator.role === "operator" || operator.role === "admin") &&
    (hasAllScopes ||
      (scopes.has("fleet:read") &&
        scopes.has("backups:read") &&
        scopes.has("integrations:write"))),
  );
}

function operatorCanManageAlertEventSchedules(
  operator: OperatorView | null,
): boolean {
  const scopes = new Set(operator?.scopes ?? []);
  const hasAllScopes = scopes.has("*");
  return Boolean(
    operator &&
    (operator.role === "operator" || operator.role === "admin") &&
    (hasAllScopes ||
      (scopes.has("fleet:read") &&
        scopes.has("backups:read") &&
        scopes.has("jobs:write") &&
        scopes.has("schedules:write"))),
  );
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
  const suspended = states.filter(
    (state) => state.label === "Suspended",
  ).length;
  const revoked = states.filter(
    (state) => state.label === "Access revoked",
  ).length;
  const unknown =
    agents.length - online - offline - never - suspended - revoked - stale;
  return {
    never,
    offline,
    online,
    suspended,
    revoked,
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
  if (subpage === "rules") return subpage;
  return "overview";
}

function remoteOperationsSubpage(subpage: string) {
  if (subpage === "bulk_files") return "multi_files";
  if (
    [
      "terminal",
      "files",
      "transfers",
      "processes",
      "services",
      "storage",
    ].includes(subpage)
  )
    return subpage;
  return "terminal";
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

function systemSubpage(subpage: string) {
  if (subpage === "capacity") return "capacity";
  if (subpage === "suite_config") return "config";
  if (subpage === "preferences") return "operator";
  return "dashboard";
}
