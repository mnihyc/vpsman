import { useCallback, useEffect, useRef, useState } from "react";
import {
  ACCESS_TOKEN_STORAGE_KEY,
  REFRESH_TOKEN_STORAGE_KEY,
} from "../constants";
import { apiGet, apiPost, isApiUnauthorized } from "../api";
import {
  type HomeSnapshotRecord,
  unavailableSnapshotSource,
} from "../homeSnapshot";
import type {
  ActiveView,
  AuthResponse,
  DashboardRefreshIntervalSecs,
  JobDetailsInvalidationSignal,
  JobHistoryRecord,
  JobStatus,
} from "../types";
import { parseWsEvent } from "../utils";
import { type AccessProjection, useAccessData } from "./useAccessData";
import { useAuditData } from "./useAuditData";
import { type BackupProjection, useBackupsData } from "./useBackupsData";
import {
  dashboardPreferencesToParams,
  useDashboardOverviewData,
} from "./useDashboardOverviewData";
import { useFleetData } from "./useFleetData";
import { useInventoryData } from "./useInventoryData";
import { type JobProjectionSource, useJobsData } from "./useJobsData";
import { usePortForwardingData } from "./usePortForwardingData";
import { useSchedulesData } from "./useSchedulesData";
import { useSystemData } from "./useSystemData";
import {
  type TopologySource,
  useTopologyData,
} from "./useTopologyData";

const FLEET_FALLBACK_REFRESH_MS = 15_000;
const FLEET_FULL_RECONCILE_MS = 60_000;

function documentIsHidden(): boolean {
  return typeof document !== "undefined" && document.hidden;
}

function readStoredToken(key: string): string {
  if (typeof window === "undefined") {
    return "";
  }
  return window.localStorage.getItem(key) ?? "";
}

function readStoredAccessToken(): string {
  return readStoredToken(ACCESS_TOKEN_STORAGE_KEY);
}

function readStoredRefreshToken(): string {
  return readStoredToken(REFRESH_TOKEN_STORAGE_KEY);
}

function hasStoredAuthSession(): boolean {
  return Boolean(readStoredAccessToken() || readStoredRefreshToken());
}

function routeOwnsLiveJobHistory(view: ActiveView, subpage: string): boolean {
  return jobProjectionSourcesForRoute(view, subpage).includes("jobHistory");
}

function routeOwnsDashboardOverview(view: ActiveView, subpage: string): boolean {
  return (
    view === "Home" ||
    (view === "Observability" &&
      (subpage === "fleet_metrics" || subpage === "dashboards"))
  );
}

function viewNeedsFinishedJobClassification(
  view: ActiveView,
  subpage: string,
): boolean {
  return (
    (view === "Fleet" &&
      (subpage === "monitor" || subpage.startsWith("instance_detail"))) ||
    (view === "Config" && subpage !== "sources") ||
    (view === "Backups" &&
      backupProjectionSourcesForRoute(view, subpage).includes("requests")) ||
    (view === "Network" &&
      [
        "overview",
        "graph",
        "tunnel_plans",
        "tests",
        "ospf",
        "evidence",
      ].includes(subpage)) ||
    (view === "Observability" && subpage === "network_metrics")
  );
}

function backupArtifactProjectionSourcesForRoute(
  view: ActiveView,
  subpage: string,
): BackupProjection[] {
  return backupProjectionSourcesForRoute(view, subpage).filter(
    (source) => source === "requests" || source === "artifacts",
  );
}

function jobProjectionSourcesForRoute(
  view: ActiveView,
  subpage: string,
): JobProjectionSource[] {
  if (view === "Home") {
    return ["jobHistory", "fileTransfers", "terminalSessions"];
  }
  if (view === "Fleet") {
    return subpage === "monitor" || subpage.startsWith("instance_detail")
      ? ["jobHistory", "fileTransfers"]
      : [];
  }
  if (view === "Config") {
    return subpage === "sources" ? [] : ["jobHistory"];
  }
  if (view === "Remote Operations") {
    if (subpage === "terminal") return ["terminalSessions"];
    if (subpage === "files") return ["fileTransfers"];
    if (subpage === "transfers") {
      return ["fileTransfers", "fileTransferSources", "commandTemplates"];
    }
    if (subpage === "processes") {
      return [
        "processSupervisorInventory",
        "fileTransferSources",
        "commandTemplates",
      ];
    }
    return [];
  }
  if (view === "Jobs") {
    if (subpage === "approvals") return ["jobApprovals"];
    if (subpage === "dispatch") {
      return ["fileTransferSources", "commandTemplates"];
    }
    if (subpage === "artifacts") {
      return ["agentUpdateReleases", "fileTransferSources"];
    }
    return ["jobHistory"];
  }
  if (view === "Automation") {
    if (subpage === "schedules") return ["commandTemplates"];
    if (subpage === "rollouts") return ["jobHistory", "jobRollouts"];
    if (subpage === "agent_updates") {
      return ["jobHistory", "agentUpdateReleases"];
    }
    if (subpage === "runbooks") return ["jobHistory", "commandTemplates"];
    return [];
  }
  if (view === "Network") {
    return subpage === "evidence" ? ["jobHistory"] : [];
  }
  if (view === "Backups") {
    return subpage === "restore" || subpage === "migration"
      ? ["fileTransfers"]
      : [];
  }
  if (view === "Audit") {
    if (subpage === "job_evidence") return ["jobHistory"];
    if (subpage === "sessions") return ["jobHistory", "terminalSessions"];
    return [];
  }
  if (view === "Access") {
    return subpage === "overview" ? ["terminalSessions"] : [];
  }
  if (
    view === "System" &&
    (subpage === "maintenance:artifacts" || subpage === "maintenance:jobs")
  ) {
    return ["serverJobs"];
  }
  return [];
}

function backupProjectionSourcesForRoute(
  view: ActiveView,
  subpage: string,
): BackupProjection[] {
  if (view === "Home") return ["requests", "artifacts"];
  if (view === "Fleet") {
    if (subpage === "monitor") return ["requests"];
    if (subpage.startsWith("instance_detail")) {
      return ["requests", "artifacts"];
    }
    return [];
  }
  if (view === "Jobs" && subpage === "artifacts") return ["artifacts"];
  if (view !== "Backups") return [];
  if (subpage === "requests") return ["requests", "policies", "artifacts"];
  if (subpage === "policies") return ["policies"];
  if (subpage === "artifacts") return ["artifacts", "requests"];
  if (subpage === "restore") {
    return ["requests", "artifacts", "restorePlans"];
  }
  if (subpage === "migration") {
    return ["requests", "artifacts", "restorePlans", "migrationLinks"];
  }
  return ["requests", "policies", "artifacts", "restorePlans", "migrationLinks"];
}

function accessProjectionSourcesForRoute(
  view: ActiveView,
  subpage: string,
): AccessProjection[] {
  if (view === "Home" || view === "System") return ["profile"];
  if (view === "Audit" && subpage === "sessions") {
    return ["profile", "operatorSessions", "operatorAuthEvents"];
  }
  if (view !== "Access") return [];
  if (subpage === "operators") {
    return [
      "profile",
      "operators",
      "operatorSessions",
      "operatorAuthEvents",
    ];
  }
  if (subpage === "vps_identities") {
    return ["profile", "clientKeyRevocations", "keyLifecycleReport"];
  }
  if (subpage === "gateway_sessions") {
    return ["profile", "gatewaySessions", "keyLifecycleReport"];
  }
  if (subpage === "privilege_vault") return ["profile"];
  return [
    "profile",
    "operators",
    "operatorSessions",
    "clientKeyRevocations",
    "keyLifecycleReport",
    "gatewaySessions",
  ];
}

function topologyProjectionSourcesForRoute(
  view: ActiveView,
  subpage: string,
): TopologySource[] {
  if (view === "Fleet" && subpage.startsWith("instance_detail")) {
    return ["networkObservations", "networkTrends"];
  }
  if (view === "Observability" && subpage === "network_metrics") {
    return [
      "tunnelPlans",
      "networkObservations",
      "networkTrends",
      "ospfRecommendations",
    ];
  }
  if (view !== "Network") return [];
  if (subpage === "overview") {
    return ["tunnelPlans", "topologyGraph", "ospfUpdatePlans"];
  }
  if (subpage === "graph") return ["topologyGraph"];
  if (subpage === "tests") return ["tunnelPlans", "networkTrends"];
  if (subpage === "ospf") return ["tunnelPlans", "ospfUpdatePlans"];
  if (subpage === "evidence") {
    return [
      "tunnelPlans",
      "networkObservations",
      "networkTrends",
      "ospfRecommendations",
      "ospfUpdatePlans",
    ];
  }
  if (subpage === "tunnel_plans") {
    return ["tunnelPlans", "networkAdapterDefinitions", "topologyGraph"];
  }
  return [];
}

function persistAuthSession(auth: AuthResponse): void {
  window.localStorage.setItem(ACCESS_TOKEN_STORAGE_KEY, auth.access_token);
  window.localStorage.setItem(REFRESH_TOKEN_STORAGE_KEY, auth.refresh_token);
}

function clearStoredAuthSession(): void {
  window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
  window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
}

export function useDashboardData(activeView: ActiveView, activeSubpage: string) {
  const [apiToken, setApiToken] = useState(() => readStoredAccessToken());
  const [authRequired, setAuthRequired] = useState(
    () => !hasStoredAuthSession(),
  );
  const [wsState, setWsState] = useState("connecting");
  const [lastLiveEvent, setLastLiveEvent] = useState("waiting");
  const [jobDetailsInvalidation, setJobDetailsInvalidation] =
    useState<JobDetailsInvalidationSignal | null>(null);
  const [authRefreshError, setAuthRefreshError] = useState<string | null>(null);
  const [logoutWarning, setLogoutWarning] = useState<string | null>(null);
  const [documentVisible, setDocumentVisible] = useState(
    () => !documentIsHidden(),
  );
  const dashboardOverviewReloadTimer = useRef<number | null>(null);
  const fleetReloadTimer = useRef<number | null>(null);
  const networkEvidenceReloadTimer = useRef<number | null>(null);
  const networkEvidenceReloadedAt = useRef(0);
  const hasEnabledTunnelPlansRef = useRef(false);
  const refreshAuthRef = useRef<Promise<void> | null>(null);
  const authGenerationRef = useRef(0);
  const jobDetailsInvalidationGenerationRef = useRef(0);
  const homeSnapshotGenerationRef = useRef(0);
  const homeSnapshotStartedKeyRef = useRef("");
  const homeVisitRef = useRef({ token: "", active: false, sequence: 0 });
  const routeVisitRef = useRef({ token: "", route: "", sequence: 0 });
  const [homeSnapshotSettledKey, setHomeSnapshotSettledKey] = useState("");
  const [homeSnapshotPendingKey, setHomeSnapshotPendingKey] = useState("");
  const [homeMonitoringCards, setHomeMonitoringCards] = useState<
    HomeSnapshotRecord["monitoring_cards"] | null
  >(null);
  const clearDashboardDataRef = useRef<() => void>(() => undefined);
  const setAuthenticatedOperatorRef = useRef<
    (operator: AuthResponse["operator"]) => void
  >(() => undefined);
  const activeViewRef = useRef(activeView);
  const activeSubpageRef = useRef(activeSubpage);
  const hiddenFleetRefreshPendingRef = useRef(false);
  const hiddenOverviewRefreshPendingRef = useRef(false);
  const hiddenNetworkEvidenceRefreshPendingRef = useRef(false);
  const hiddenBackupRefreshPendingRef = useRef(false);
  const hiddenAuditRefreshPendingRef = useRef(false);
  const hiddenOperatorProfileRefreshPendingRef = useRef(false);
  const routeHydrationKeyRef = useRef("");
  const suiteConfigHydrationKeyRef = useRef("");
  const globalProfileHydrationTokenRef = useRef("");
  const hiddenJobDetailIdsRef = useRef(new Set<string>());
  const hiddenJobHistoryEventsRef = useRef(
    new Map<
      string,
      { refreshRenderedEffects: boolean; status: JobStatus }
    >(),
  );
  const hiddenResolvedJobEffectsRef = useRef(
    new Map<string, JobHistoryRecord>(),
  );
  const overviewVisibilityCatchupRef = useRef(false);

  if (homeVisitRef.current.token !== apiToken) {
    homeVisitRef.current = { token: apiToken, active: false, sequence: 0 };
  }
  if (activeView === "Home" && !homeVisitRef.current.active) {
    homeVisitRef.current.active = true;
    homeVisitRef.current.sequence += 1;
  } else if (activeView !== "Home") {
    homeVisitRef.current.active = false;
  }
  const homeVisitKey =
    apiToken && activeView === "Home"
      ? `${apiToken}:${homeVisitRef.current.sequence}`
      : "";
  const homeSnapshotOwnsVisit = Boolean(homeVisitKey);
  const activeRouteKey = `${activeView}\u0000${activeSubpage}`;
  if (
    routeVisitRef.current.token !== apiToken ||
    routeVisitRef.current.route !== activeRouteKey
  ) {
    routeVisitRef.current = {
      token: apiToken,
      route: activeRouteKey,
      sequence: routeVisitRef.current.sequence + 1,
    };
  }
  const routeVisitKey = apiToken
    ? `${apiToken}\u0000${routeVisitRef.current.sequence}`
    : "";

  useEffect(() => {
    activeViewRef.current = activeView;
    activeSubpageRef.current = activeSubpage;
  }, [activeSubpage, activeView]);

  const forceAuthRequired = useCallback(() => {
    authGenerationRef.current += 1;
    refreshAuthRef.current = null;
    clearDashboardDataRef.current();
    clearStoredAuthSession();
    setAuthRefreshError(null);
    setLogoutWarning(null);
    setWsState("auth required");
    setApiToken("");
    setAuthRequired(true);
  }, []);

  const refreshStoredAuth = useCallback(() => {
    if (refreshAuthRef.current) {
      return refreshAuthRef.current;
    }
    const refreshToken = readStoredRefreshToken();
    if (!refreshToken) {
      forceAuthRequired();
      return Promise.resolve();
    }
    const authGeneration = authGenerationRef.current;
    setAuthRefreshError(null);
    const request = apiPost<AuthResponse>("/api/v1/auth/refresh", "", {
      refresh_token: refreshToken,
    })
      .then((auth) => {
        if (authGeneration !== authGenerationRef.current) {
          return;
        }
        persistAuthSession(auth);
        setAuthRefreshError(null);
        setAuthenticatedOperatorRef.current(auth.operator);
        setApiToken(auth.access_token);
        setAuthRequired(false);
      })
      .catch((error) => {
        if (authGeneration !== authGenerationRef.current) {
          return;
        }
        if (isApiUnauthorized(error)) {
          forceAuthRequired();
          return;
        }
        setAuthRefreshError(
          error instanceof Error
            ? `Session refresh failed: ${error.message}`
            : "Session refresh failed without usable error detail. Retry after checking API availability.",
        );
      })
      .finally(() => {
        if (refreshAuthRef.current === trackedRequest) {
          refreshAuthRef.current = null;
        }
      });
    const trackedRequest = request;
    refreshAuthRef.current = trackedRequest;
    return trackedRequest;
  }, [forceAuthRequired]);

  const requireAuth = useCallback(() => {
    void refreshStoredAuth();
  }, [refreshStoredAuth]);
  const access = useAccessData(apiToken, requireAuth);
  setAuthenticatedOperatorRef.current = access.setAuthenticatedOperator;
  const activeAccessProjectionSources = accessProjectionSourcesForRoute(
    activeView,
    activeSubpage,
  );
  const activeAccessError = access.accessSourcesError(
    activeAccessProjectionSources,
  );
  const activeAccessLoading =
    (activeView === "Home" && access.accessLoading) ||
    access.accessSourcesLoading(activeAccessProjectionSources);
  const dashboardOverview = useDashboardOverviewData(apiToken, requireAuth);
  const fleet = useFleetData(apiToken, requireAuth);
  const audit = useAuditData(apiToken, requireAuth);
  const inventory = useInventoryData(
    apiToken,
    requireAuth,
    fleet.reconcileAgentTagMutation,
  );
  const jobs = useJobsData(
    apiToken,
    requireAuth,
    fleet.loadFleet,
    audit.loadAuditLogs,
  );
  const activeJobProjectionSources = jobProjectionSourcesForRoute(
    activeView,
    activeSubpage,
  );
  const activeJobsError = jobs.jobSourcesError(activeJobProjectionSources);
  const activeJobsLoading =
    (activeView === "Home" && jobs.jobsLoading) ||
    jobs.jobSourcesLoading(activeJobProjectionSources);
  const schedules = useSchedulesData(
    apiToken,
    requireAuth,
    audit.loadAuditLogs,
  );
  const system = useSystemData(apiToken, requireAuth);
  const topology = useTopologyData(
    apiToken,
    requireAuth,
    audit.loadAuditLogs,
    inventory.loadRuntimeConfigApplyStates,
  );
  const activeTopologyProjectionSources = topologyProjectionSourcesForRoute(
    activeView,
    activeSubpage,
  );
  const activeTopologyError = topology.topologySourcesError(
    activeTopologyProjectionSources,
  );
  const activeTopologyLoading = topology.topologySourcesLoading(
    activeTopologyProjectionSources,
  );
  useEffect(() => {
    hasEnabledTunnelPlansRef.current = topology.tunnelPlans.some(
      (plan) => plan.enabled && !plan.deleted_at,
    );
  }, [topology.tunnelPlans]);
  const portForwarding = usePortForwardingData(
    apiToken,
    requireAuth,
    audit.loadAuditLogs,
  );
  const backups = useBackupsData(apiToken, requireAuth, audit.loadAuditLogs);
  const activeBackupProjectionSources = backupProjectionSourcesForRoute(
    activeView,
    activeSubpage,
  );
  const activeBackupsError = backups.backupSourcesError(
    activeBackupProjectionSources,
  );
  const activeBackupsLoading =
    ((activeView === "Home" ||
      (activeView === "Backups" && activeSubpage === "overview")) &&
      backups.backupsLoading) ||
    backups.backupSourcesLoading(activeBackupProjectionSources);
  const clearDashboardData = useCallback(() => {
    homeSnapshotGenerationRef.current += 1;
    homeSnapshotStartedKeyRef.current = "";
    homeVisitRef.current = { token: "", active: false, sequence: 0 };
    routeVisitRef.current = { token: "", route: "", sequence: 0 };
    setHomeSnapshotSettledKey("");
    setHomeSnapshotPendingKey("");
    setHomeMonitoringCards(null);
    hiddenFleetRefreshPendingRef.current = false;
    hiddenOverviewRefreshPendingRef.current = false;
    hiddenNetworkEvidenceRefreshPendingRef.current = false;
    hiddenBackupRefreshPendingRef.current = false;
    hiddenAuditRefreshPendingRef.current = false;
    hiddenOperatorProfileRefreshPendingRef.current = false;
    routeHydrationKeyRef.current = "";
    suiteConfigHydrationKeyRef.current = "";
    globalProfileHydrationTokenRef.current = "";
    hiddenJobDetailIdsRef.current.clear();
    hiddenJobHistoryEventsRef.current.clear();
    hiddenResolvedJobEffectsRef.current.clear();
    overviewVisibilityCatchupRef.current = false;
    for (const timer of [
      dashboardOverviewReloadTimer,
      fleetReloadTimer,
      networkEvidenceReloadTimer,
    ]) {
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
        timer.current = null;
      }
    }
    setLastLiveEvent("waiting");
    setJobDetailsInvalidation(null);
    access.clearAccess();
    dashboardOverview.clearDashboardOverview();
    fleet.clearFleet();
    audit.clearAudits();
    inventory.clearInventory();
    jobs.clearJobs();
    schedules.clearSchedules();
    system.clearSystem();
    topology.clearTopology();
    portForwarding.clearPortForwarding();
    backups.clearBackups();
  }, [
    access.clearAccess,
    audit.clearAudits,
    backups.clearBackups,
    dashboardOverview.clearDashboardOverview,
    fleet.clearFleet,
    inventory.clearInventory,
    jobs.clearJobs,
    portForwarding.clearPortForwarding,
    schedules.clearSchedules,
    system.clearSystem,
    topology.clearTopology,
  ]);
  clearDashboardDataRef.current = clearDashboardData;

  const scheduleDashboardOverviewReload = useCallback(() => {
    if (documentIsHidden()) {
      hiddenOverviewRefreshPendingRef.current = true;
      return;
    }
    if (dashboardOverviewReloadTimer.current !== null) {
      window.clearTimeout(dashboardOverviewReloadTimer.current);
    }
    dashboardOverviewReloadTimer.current = window.setTimeout(() => {
      dashboardOverviewReloadTimer.current = null;
      if (documentIsHidden()) {
        hiddenOverviewRefreshPendingRef.current = true;
        return;
      }
      void dashboardOverview.loadDashboardOverview();
    }, 250);
  }, [dashboardOverview.loadDashboardOverview]);
  const scheduleFleetReload = useCallback(() => {
    if (documentIsHidden()) {
      hiddenFleetRefreshPendingRef.current = true;
      return;
    }
    if (fleetReloadTimer.current !== null) {
      return;
    }
    fleetReloadTimer.current = window.setTimeout(() => {
      fleetReloadTimer.current = null;
      if (documentIsHidden()) {
        hiddenFleetRefreshPendingRef.current = true;
        return;
      }
      void fleet.loadFleet(true);
    }, 750);
  }, [fleet.loadFleet]);
  const scheduleNetworkEvidenceReload = useCallback(() => {
    if (!hasEnabledTunnelPlansRef.current) {
      return;
    }
    if (documentIsHidden()) {
      hiddenNetworkEvidenceRefreshPendingRef.current = true;
      return;
    }
    if (networkEvidenceReloadTimer.current !== null) {
      return;
    }
    const elapsed = Date.now() - networkEvidenceReloadedAt.current;
    const delay = Math.max(0, 60_000 - elapsed);
    networkEvidenceReloadTimer.current = window.setTimeout(() => {
      networkEvidenceReloadTimer.current = null;
      if (documentIsHidden()) {
        hiddenNetworkEvidenceRefreshPendingRef.current = true;
        return;
      }
      const currentView = activeViewRef.current;
      if (currentView !== "Network" && currentView !== "Observability") {
        return;
      }
      const currentSubpage = activeSubpageRef.current;
      if (
        currentView === "Observability" &&
        currentSubpage !== "network_metrics"
      ) {
        return;
      }
      networkEvidenceReloadedAt.current = Date.now();
      void topology.refreshRenderedNetworkEvidence(currentSubpage);
    }, delay);
  }, [topology.refreshRenderedNetworkEvidence]);

  const refreshBackupArtifactProjectionsForRoute = useCallback(
    (view: ActiveView, subpage: string) => {
      const sources = backupArtifactProjectionSourcesForRoute(view, subpage);
      const ownsRequests = sources.includes("requests");
      const ownsArtifacts = sources.includes("artifacts");
      if (ownsRequests && ownsArtifacts) {
        void backups.loadBackupRequestArtifactProjections();
      } else if (ownsRequests) {
        void backups.loadBackupRequests();
      } else if (ownsArtifacts) {
        void backups.loadBackupArtifacts();
      }
    },
    [
      backups.loadBackupArtifacts,
      backups.loadBackupRequestArtifactProjections,
      backups.loadBackupRequests,
    ],
  );

  const refreshRenderedJobEffects = useCallback(
    (job: JobHistoryRecord) => {
      const currentView = activeViewRef.current;
      const currentSubpage = activeSubpageRef.current;
      if (
        job.command_type === "runtime_config_sync" &&
        ((currentView === "Config" && currentSubpage !== "sources") ||
          (currentView === "Fleet" &&
            currentSubpage.startsWith("instance_detail")) ||
          (currentView === "Network" && currentSubpage === "graph"))
      ) {
        void inventory.loadRuntimeConfigApplyStates();
      }
      if (
        job.command_type === "runtime_config_sync" &&
        currentView === "Network" &&
        currentSubpage === "tunnel_plans"
      ) {
        void topology.loadTunnelPlans();
      }
      if (
        job.command_type === "backup" &&
        [
          "partial_success",
          "rejected",
          "failed",
          "agent_timeout",
          "control_timeout",
          "canceled",
        ].includes(job.status) &&
        backupProjectionSourcesForRoute(
          currentView,
          currentSubpage,
        ).includes("requests")
      ) {
        void backups.loadBackupRequests();
      }
      if (
        ((currentView === "Network" &&
          [
            "overview",
            "graph",
            "tunnel_plans",
            "tests",
            "ospf",
            "evidence",
          ].includes(currentSubpage)) ||
          (currentView === "Observability" &&
            currentSubpage === "network_metrics"))
      ) {
        void topology.refreshNetworkJobEvidence(
          job.command_type,
          currentSubpage,
        );
      }
    },
    [
      backups.loadBackupRequests,
      inventory.loadRuntimeConfigApplyStates,
      topology.loadTunnelPlans,
      topology.refreshNetworkJobEvidence,
    ],
  );

  useEffect(
    () => () => {
      if (dashboardOverviewReloadTimer.current !== null) {
        window.clearTimeout(dashboardOverviewReloadTimer.current);
      }
      if (fleetReloadTimer.current !== null) {
        window.clearTimeout(fleetReloadTimer.current);
      }
      if (networkEvidenceReloadTimer.current !== null) {
        window.clearTimeout(networkEvidenceReloadTimer.current);
      }
    },
    [],
  );

  useEffect(() => {
    const handleVisibilityChange = () => {
      const visible = !documentIsHidden();
      setDocumentVisible(visible);
      if (!visible) {
        const homeSnapshotDeferred = Boolean(
          homeVisitKey && homeSnapshotStartedKeyRef.current !== homeVisitKey,
        );
        hiddenFleetRefreshPendingRef.current = Boolean(
          apiToken && !homeSnapshotDeferred,
        );
        hiddenOverviewRefreshPendingRef.current = Boolean(
          apiToken &&
          !homeSnapshotDeferred &&
          routeOwnsDashboardOverview(
            activeViewRef.current,
            activeSubpageRef.current,
          ),
        );
        for (const timer of [
          dashboardOverviewReloadTimer,
          fleetReloadTimer,
          networkEvidenceReloadTimer,
        ]) {
          if (timer.current !== null) {
            if (timer === networkEvidenceReloadTimer) {
              hiddenNetworkEvidenceRefreshPendingRef.current = true;
            }
            window.clearTimeout(timer.current);
            timer.current = null;
          }
        }
        return;
      }
      const homeSnapshotInFlight = Boolean(
        homeVisitKey &&
        homeSnapshotStartedKeyRef.current === homeVisitKey &&
        homeSnapshotSettledKey !== homeVisitKey,
      );
      if (
        !homeSnapshotInFlight &&
        hiddenFleetRefreshPendingRef.current &&
        apiToken
      ) {
        hiddenFleetRefreshPendingRef.current = false;
        void fleet.loadFleet(true);
      }
      const overviewRefreshPending = hiddenOverviewRefreshPendingRef.current;
      const currentRouteOwnsOverview = routeOwnsDashboardOverview(
        activeViewRef.current,
        activeSubpageRef.current,
      );
      if (!currentRouteOwnsOverview) {
        hiddenOverviewRefreshPendingRef.current = false;
      }
      if (
        !homeSnapshotInFlight &&
        overviewRefreshPending &&
        apiToken &&
        currentRouteOwnsOverview
      ) {
        hiddenOverviewRefreshPendingRef.current = false;
        overviewVisibilityCatchupRef.current = true;
        void dashboardOverview.loadDashboardOverview();
      }
      const currentView = activeViewRef.current;
      const currentSubpage = activeSubpageRef.current;
      const currentVisitWasHydrated =
        routeHydrationKeyRef.current === routeVisitKey;
      if (hiddenNetworkEvidenceRefreshPendingRef.current && apiToken) {
        hiddenNetworkEvidenceRefreshPendingRef.current = false;
        if (
          currentVisitWasHydrated &&
          hasEnabledTunnelPlansRef.current &&
          (currentView === "Network" ||
            (currentView === "Observability" &&
              currentSubpage === "network_metrics"))
        ) {
          networkEvidenceReloadedAt.current = Date.now();
          void topology.refreshRenderedNetworkEvidence(currentSubpage);
        }
      }
      if (hiddenBackupRefreshPendingRef.current && apiToken) {
        hiddenBackupRefreshPendingRef.current = false;
        if (currentVisitWasHydrated) {
          refreshBackupArtifactProjectionsForRoute(
            currentView,
            currentSubpage,
          );
        }
      }
      if (hiddenAuditRefreshPendingRef.current && apiToken) {
        hiddenAuditRefreshPendingRef.current = false;
        if (currentVisitWasHydrated && currentView === "Audit") {
          void audit.loadAuditLogs();
        }
      }
      if (hiddenOperatorProfileRefreshPendingRef.current && apiToken) {
        hiddenOperatorProfileRefreshPendingRef.current = false;
        const newVisitHydratesProfile =
          !currentVisitWasHydrated &&
          (currentView === "Access" ||
            currentView === "System" ||
            (currentView === "Audit" && currentSubpage === "sessions"));
        if (!newVisitHydratesProfile) {
          void access.loadCurrentOperatorProfile();
        }
      }
      if (hiddenJobDetailIdsRef.current.size > 0) {
        const jobIds = [...hiddenJobDetailIdsRef.current];
        hiddenJobDetailIdsRef.current.clear();
        if (currentView === "Jobs") {
          jobDetailsInvalidationGenerationRef.current += 1;
          setJobDetailsInvalidation({
            generation: jobDetailsInvalidationGenerationRef.current,
            job_ids: jobIds,
          });
        }
      }
      const jobEvents = [...hiddenJobHistoryEventsRef.current];
      hiddenJobHistoryEventsRef.current.clear();
      const resolvedJobEffects = [
        ...hiddenResolvedJobEffectsRef.current.values(),
      ];
      hiddenResolvedJobEffectsRef.current.clear();
      if (currentVisitWasHydrated) {
        for (const job of resolvedJobEffects) {
          refreshRenderedJobEffects(job);
        }
        for (const [jobId, event] of jobEvents) {
          void jobs
            .refreshJobHistoryAfterEvent(jobId, event.status)
            .then((job) => {
              if (job && event.refreshRenderedEffects) {
                refreshRenderedJobEffects(job);
              }
            });
        }
      }
    };
    document.addEventListener("visibilitychange", handleVisibilityChange);
    handleVisibilityChange();
    return () =>
      document.removeEventListener("visibilitychange", handleVisibilityChange);
  }, [
    apiToken,
    access.loadCurrentOperatorProfile,
    audit.loadAuditLogs,
    backups.loadBackupRequests,
    dashboardOverview.loadDashboardOverview,
    fleet.loadFleet,
    homeSnapshotSettledKey,
    homeVisitKey,
    jobs.refreshJobHistoryAfterEvent,
    refreshRenderedJobEffects,
    refreshBackupArtifactProjectionsForRoute,
    routeVisitKey,
    topology.refreshRenderedNetworkEvidence,
  ]);

  useEffect(() => {
    if (!apiToken && hasStoredAuthSession()) {
      void refreshStoredAuth();
    }
  }, [apiToken, refreshStoredAuth]);

  useEffect(() => {
    if (
      !apiToken ||
      !documentVisible ||
      !homeSnapshotOwnsVisit ||
      homeSnapshotStartedKeyRef.current === homeVisitKey
    ) {
      return;
    }
    homeSnapshotStartedKeyRef.current = homeVisitKey;
    setHomeSnapshotPendingKey(homeVisitKey);
    const generation = homeSnapshotGenerationRef.current + 1;
    homeSnapshotGenerationRef.current = generation;
    setHomeMonitoringCards(null);
    const hydrationFences = {
      access: access.beginHomeOperatorHydration(),
      audit: audit.beginHomeAuditHydration(),
      backups: backups.beginHomeBackupsHydration(),
      dashboardOverview:
        dashboardOverview.beginHomeDashboardOverviewHydration(),
      fleet: fleet.beginHomeFleetHydration(),
      jobs: jobs.beginHomeJobsHydration(),
      schedules: schedules.beginHomeSchedulesHydration(),
      system: system.beginHomeSystemDashboardHydration(),
    };
    const requestIsCurrent = () => {
      return (
        generation === homeSnapshotGenerationRef.current &&
        readStoredAccessToken() === apiToken &&
        activeViewRef.current === "Home" &&
        homeVisitRef.current.token === apiToken &&
        `${apiToken}:${homeVisitRef.current.sequence}` === homeVisitKey
      );
    };
    const params = dashboardPreferencesToParams(
      dashboardOverview.dashboardPreferences,
    );
    void apiGet<HomeSnapshotRecord>(
      `/api/v1/home/snapshot?${params.toString()}`,
      apiToken,
    )
      .then((snapshot) => {
        if (!requestIsCurrent()) {
          return;
        }
        setHomeMonitoringCards(snapshot.monitoring_cards);
        access.hydrateHomeOperator(hydrationFences.access, snapshot.operator);
        fleet.hydrateHomeFleet(hydrationFences.fleet, snapshot);
        jobs.hydrateHomeJobs(
          hydrationFences.jobs,
          snapshot.jobs,
          snapshot.file_transfers,
          snapshot.terminal_sessions,
        );
        backups.hydrateHomeBackups(
          hydrationFences.backups,
          snapshot.backups,
          snapshot.backup_artifacts,
        );
        audit.hydrateHomeAudit(hydrationFences.audit, snapshot.audit);
        schedules.hydrateHomeSchedules(
          hydrationFences.schedules,
          snapshot.schedules,
        );
        system.hydrateHomeSystemDashboard(
          hydrationFences.system,
          snapshot.system_dashboard,
        );
        dashboardOverview.hydrateHomeDashboardOverview(
          hydrationFences.dashboardOverview,
          snapshot.dashboard_overview,
        );
        setHomeSnapshotSettledKey(homeVisitKey);
      })
      .catch((error) => {
        if (!requestIsCurrent()) {
          return;
        }
        if (isApiUnauthorized(error)) {
          requireAuth();
          return;
        }
        const message =
          error instanceof Error
            ? `Home snapshot: ${error.message}`
            : "Home snapshot unavailable";
        setHomeMonitoringCards(unavailableSnapshotSource(message));
        access.hydrateHomeOperator(hydrationFences.access, null, message);
        fleet.hydrateHomeFleet(hydrationFences.fleet, {
          summary: unavailableSnapshotSource(message),
          agents: unavailableSnapshotSource(message),
          telemetry_rollups: unavailableSnapshotSource(message),
          telemetry_network_rates: unavailableSnapshotSource(message),
          fleet_alerts: unavailableSnapshotSource(message),
        });
        jobs.hydrateHomeJobs(
          hydrationFences.jobs,
          unavailableSnapshotSource(message),
          unavailableSnapshotSource(message),
          unavailableSnapshotSource(message),
        );
        backups.hydrateHomeBackups(
          hydrationFences.backups,
          unavailableSnapshotSource(message),
          unavailableSnapshotSource(message),
        );
        audit.hydrateHomeAudit(
          hydrationFences.audit,
          unavailableSnapshotSource(message),
        );
        schedules.hydrateHomeSchedules(
          hydrationFences.schedules,
          unavailableSnapshotSource(message),
        );
        system.hydrateHomeSystemDashboard(
          hydrationFences.system,
          unavailableSnapshotSource(message),
        );
        dashboardOverview.hydrateHomeDashboardOverview(
          hydrationFences.dashboardOverview,
          unavailableSnapshotSource(message),
        );
        setHomeSnapshotSettledKey(homeVisitKey);
      })
      .finally(() => {
        if (generation === homeSnapshotGenerationRef.current) {
          setHomeSnapshotPendingKey((current) =>
            current === homeVisitKey ? "" : current,
          );
        }
      });
  }, [
    access.beginHomeOperatorHydration,
    access.hydrateHomeOperator,
    apiToken,
    audit.beginHomeAuditHydration,
    audit.hydrateHomeAudit,
    backups.beginHomeBackupsHydration,
    backups.hydrateHomeBackups,
    dashboardOverview.beginHomeDashboardOverviewHydration,
    dashboardOverview.dashboardPreferences,
    dashboardOverview.hydrateHomeDashboardOverview,
    documentVisible,
    fleet.beginHomeFleetHydration,
    fleet.hydrateHomeFleet,
    homeSnapshotOwnsVisit,
    homeVisitKey,
    jobs.beginHomeJobsHydration,
    jobs.hydrateHomeJobs,
    requireAuth,
    schedules.beginHomeSchedulesHydration,
    schedules.hydrateHomeSchedules,
    system.beginHomeSystemDashboardHydration,
    system.hydrateHomeSystemDashboard,
  ]);

  useEffect(() => {
    const routeOwnsProfile =
      activeView === "Access" ||
      activeView === "System" ||
      (activeView === "Audit" && activeSubpage === "sessions");
    if (!apiToken) {
      globalProfileHydrationTokenRef.current = "";
      return;
    }
    if (access.operator) {
      globalProfileHydrationTokenRef.current = apiToken;
      return;
    }
    if (
      !documentVisible ||
      homeSnapshotOwnsVisit ||
      routeOwnsProfile ||
      globalProfileHydrationTokenRef.current === apiToken
    ) {
      return;
    }
    globalProfileHydrationTokenRef.current = apiToken;
    void access.loadCurrentOperatorProfile();
  }, [
    access.loadCurrentOperatorProfile,
    access.operator,
    activeSubpage,
    activeView,
    apiToken,
    documentVisible,
    homeSnapshotOwnsVisit,
  ]);

  useEffect(() => {
    if (!apiToken || !documentVisible || homeSnapshotOwnsVisit) {
      return;
    }
    void fleet.loadFleet();
  }, [apiToken, documentVisible, fleet.loadFleet, homeSnapshotOwnsVisit]);

  useEffect(() => {
    if (!apiToken || wsState !== "connected" || !documentVisible) {
      return;
    }
    const timer = window.setInterval(() => {
      if (documentIsHidden()) {
        hiddenFleetRefreshPendingRef.current = true;
        return;
      }
      void fleet.loadFleet();
    }, FLEET_FULL_RECONCILE_MS);
    return () => window.clearInterval(timer);
  }, [apiToken, documentVisible, fleet.loadFleet, wsState]);

  useEffect(() => {
    if (!apiToken || wsState === "connected" || !documentVisible) {
      return;
    }
    let disposed = false;
    let tick = 1;
    let refreshInFlight = false;

    async function loadFallbackSnapshot() {
      if (disposed || refreshInFlight) {
        return;
      }
      if (documentIsHidden()) {
        hiddenFleetRefreshPendingRef.current = true;
        return;
      }
      refreshInFlight = true;
      try {
        if (tick % 4 === 0) {
          await fleet.loadFleet();
        } else {
          await fleet.loadFleetTelemetry();
        }
        tick += 1;
      } finally {
        refreshInFlight = false;
      }
    }

    const timer = window.setInterval(
      loadFallbackSnapshot,
      FLEET_FALLBACK_REFRESH_MS,
    );
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [
    apiToken,
    documentVisible,
    fleet.loadFleet,
    fleet.loadFleetTelemetry,
    wsState,
  ]);

  useEffect(() => {
    if (
      !apiToken ||
      !documentVisible ||
      !routeOwnsDashboardOverview(activeView, activeSubpage)
    ) {
      return;
    }
    let disposed = false;
    let timer: number | null = null;

    async function loadAndSchedule() {
      if (documentIsHidden()) {
        hiddenOverviewRefreshPendingRef.current = true;
        return;
      }
      await dashboardOverview.loadDashboardOverview();
      if (disposed || documentIsHidden()) {
        if (!disposed) hiddenOverviewRefreshPendingRef.current = true;
        return;
      }
      timer = window.setTimeout(
        loadAndSchedule,
        dashboardRefreshIntervalMs(
          dashboardOverview.dashboardPreferences.refreshIntervalSecs,
        ),
      );
    }

    if (overviewVisibilityCatchupRef.current) {
      overviewVisibilityCatchupRef.current = false;
      timer = window.setTimeout(
        loadAndSchedule,
        dashboardRefreshIntervalMs(
          dashboardOverview.dashboardPreferences.refreshIntervalSecs,
        ),
      );
    } else if (homeSnapshotOwnsVisit) {
      if (homeSnapshotSettledKey !== homeVisitKey) {
        return;
      }
      timer = window.setTimeout(
        loadAndSchedule,
        dashboardRefreshIntervalMs(
          dashboardOverview.dashboardPreferences.refreshIntervalSecs,
        ),
      );
    } else {
      void loadAndSchedule();
    }
    return () => {
      disposed = true;
      if (timer !== null) {
        window.clearTimeout(timer);
      }
    };
  }, [
    activeView,
    activeSubpage,
    apiToken,
    documentVisible,
    dashboardOverview.dashboardPreferences.refreshIntervalSecs,
    dashboardOverview.loadDashboardOverview,
    homeSnapshotOwnsVisit,
    homeSnapshotSettledKey,
    homeVisitKey,
  ]);

  useEffect(() => {
    if (!apiToken || !documentVisible) {
      return;
    }
    if (routeHydrationKeyRef.current === routeVisitKey) {
      return;
    }
    routeHydrationKeyRef.current = routeVisitKey;
    if (activeView === "Home") {
      // The generation-fenced aggregate snapshot owns every Home activation.
    } else if (activeView === "Fleet") {
      if (activeSubpage === "instances") {
        void inventory.loadTagOrder();
      } else if (activeSubpage === "monitor") {
        void jobs.loadJobHistory();
        void backups.loadBackupRequests();
        void jobs.loadFileTransfers();
      } else if (activeSubpage.startsWith("group")) {
        void inventory.loadTagOrder();
        void schedules.loadSchedules();
      } else if (activeSubpage.startsWith("instance_detail")) {
        void inventory.loadRuntimeConfigApplyStates();
        void jobs.loadJobHistory();
        void jobs.loadFileTransfers();
        void backups.loadBackupRequestArtifactProjections();
        void audit.loadAuditLogs();
        void topology.loadNetworkObservations();
        void topology.loadNetworkTrends();
      }
    } else if (activeView === "Config") {
      if (activeSubpage !== "sources") {
        void inventory.loadTagInventory();
        void jobs.loadJobHistory();
      }
    } else if (activeView === "Remote Operations") {
      if (activeSubpage === "terminal") {
        void jobs.loadTerminalSessions();
      } else if (activeSubpage === "files") {
        void jobs.loadFileTransfers();
      } else if (
        activeSubpage === "transfers" ||
        activeSubpage === "processes"
      ) {
        if (activeSubpage === "transfers") void jobs.loadFileTransfers();
        if (activeSubpage === "processes") {
          void jobs.loadProcessSupervisorInventory();
        }
        void jobs.loadFileTransferSources();
        void jobs.loadCommandTemplates();
      }
    } else if (activeView === "Jobs") {
      if (activeSubpage === "history" || activeSubpage === "scheduled_runs") {
        void jobs.loadJobHistory();
        if (activeSubpage === "scheduled_runs") {
          void schedules.loadSchedules();
        }
      } else if (activeSubpage === "approvals") {
        void jobs.loadJobApprovals();
      } else if (activeSubpage === "dispatch") {
        void jobs.loadCommandTemplates();
        void jobs.loadFileTransferSources();
      } else if (activeSubpage === "artifacts") {
        void jobs.loadAgentUpdateReleases();
        void jobs.loadFileTransferSources();
      } else {
        void jobs.loadJobHistory();
      }
      if (activeSubpage === "artifacts") {
        void backups.loadBackupArtifacts();
      }
    } else if (activeView === "Automation") {
      if (activeSubpage === "schedules") {
        void schedules.loadSchedules();
        void jobs.loadCommandTemplates();
      } else if (activeSubpage === "rollouts") {
        void jobs.loadJobHistory();
      } else if (activeSubpage === "agent_updates") {
        void jobs.loadJobHistory();
        void jobs.loadAgentUpdateReleases();
      } else if (activeSubpage === "runbooks") {
        void jobs.loadJobHistory();
        void jobs.loadCommandTemplates();
      }
    } else if (activeView === "Network") {
      // The mounted Network subpage owns its exact projection sources.
      if (activeSubpage === "evidence") void jobs.loadJobHistory();
    } else if (activeView === "Backups") {
      if (activeSubpage === "overview") {
        void backups.loadBackups();
      } else if (activeSubpage === "requests") {
        void backups.loadBackupRequests();
        void backups.loadBackupPolicies();
        void backups.loadBackupArtifacts();
      } else if (activeSubpage === "policies") {
        void backups.loadBackupPolicies();
      } else if (activeSubpage === "artifacts") {
        void backups.loadBackupArtifacts();
        void backups.loadBackupRequests();
      } else if (activeSubpage === "restore") {
        void backups.loadBackupRequests();
        void backups.loadBackupArtifacts();
        void backups.loadRestorePlans();
        void jobs.loadFileTransfers();
      } else if (activeSubpage === "migration") {
        void backups.loadBackupRequests();
        void backups.loadBackupArtifacts();
        void backups.loadRestorePlans();
        void backups.loadMigrationLinks();
        void jobs.loadFileTransfers();
      }
    } else if (activeView === "Observability") {
      if (activeSubpage === "network_metrics") {
        void topology.loadTunnelPlans();
        void topology.loadOspfRecommendations();
      }
    } else if (activeView === "Audit") {
      if (activeSubpage === "retention_export") {
        void audit.loadAudits();
      } else {
        void audit.loadAuditLogs();
      }
      if (activeSubpage === "job_evidence" || activeSubpage === "sessions") {
        void jobs.loadJobHistory();
      }
      if (activeSubpage === "sessions") {
        void jobs.loadTerminalSessions();
        void access.loadAccessAuditSessions();
      }
    } else if (activeView === "Access") {
      if (activeSubpage === "operators") {
        void access.loadAccessOperators();
      } else if (activeSubpage === "vps_identities") {
        void access.loadAccessVpsIdentities();
      } else if (activeSubpage === "gateway_sessions") {
        void access.loadAccessGatewaySessions();
      } else if (activeSubpage === "privilege_vault") {
        void access.loadCurrentOperatorProfile();
      } else {
        void access.loadAccessOverview();
      }
      if (activeSubpage === "overview") void jobs.loadTerminalSessions();
    } else if (activeView === "System") {
      void access.loadCurrentOperatorProfile();
      if (activeSubpage === "overview" || activeSubpage === "capacity") {
        void system.loadSystemDashboard();
      } else if (activeSubpage === "preferences") {
        void inventory.loadTagOrder();
      } else if (
        activeSubpage === "maintenance:artifacts" ||
        activeSubpage === "maintenance:jobs"
      ) {
        void jobs.loadServerJobs();
      }
    }
  }, [
    access.loadAccessAuditSessions,
    access.loadAccessGatewaySessions,
    access.loadAccessOperators,
    access.loadAccessOverview,
    access.loadAccessVpsIdentities,
    access.loadCurrentOperatorProfile,
    activeSubpage,
    activeView,
    apiToken,
    documentVisible,
    audit.loadAuditLogs,
    audit.loadAudits,
    backups.loadBackupArtifacts,
    backups.loadBackupPolicies,
    backups.loadBackupRequestArtifactProjections,
    backups.loadBackupRequests,
    backups.loadBackups,
    backups.loadMigrationLinks,
    backups.loadRestorePlans,
    inventory.loadTagInventory,
    inventory.loadTagOrder,
    inventory.loadRuntimeConfigApplyStates,
    jobs.loadFileTransfers,
    jobs.loadAgentUpdateReleases,
    jobs.loadCommandTemplates,
    jobs.loadFileTransferSources,
    jobs.loadJobApprovals,
    jobs.loadJobHistory,
    jobs.loadProcessSupervisorInventory,
    jobs.loadServerJobs,
    jobs.loadTerminalSessions,
    schedules.loadSchedules,
    system.loadSystemDashboard,
    topology.loadNetworkObservations,
    topology.loadNetworkTrends,
    topology.loadOspfRecommendations,
    topology.loadTunnelPlans,
    routeVisitKey,
  ]);

  useEffect(() => {
    if (
      !apiToken ||
      !documentVisible ||
      access.operator?.role !== "admin" ||
      !(
        (activeView === "Automation" && activeSubpage === "agent_updates") ||
        (activeView === "System" && activeSubpage === "suite_config")
      )
    ) {
      return;
    }
    if (suiteConfigHydrationKeyRef.current === routeVisitKey) {
      return;
    }
    suiteConfigHydrationKeyRef.current = routeVisitKey;
    void system.loadSuiteConfig();
  }, [
    access.operator?.role,
    activeSubpage,
    activeView,
    apiToken,
    documentVisible,
    routeVisitKey,
    system.loadSuiteConfig,
  ]);

  useEffect(() => {
    if (!apiToken) {
      setWsState("auth required");
      return;
    }
    let disposed = false;
    let reconnectAttempt = 0;
    let reconnectTimer: number | null = null;
    let socket: WebSocket | null = null;
    let hasConnected = false;
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const connect = () => {
      if (disposed) {
        return;
      }
      setWsState(reconnectAttempt === 0 ? "connecting" : "reconnecting");
      socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
      socket.addEventListener("open", () => {
        if (disposed) {
          socket?.close();
          return;
        }
        const recovering = hasConnected;
        hasConnected = true;
        reconnectAttempt = 0;
        socket?.send(JSON.stringify({ type: "auth", access_token: apiToken }));
        setWsState("connected");
        if (recovering) {
          if (documentIsHidden()) {
            hiddenFleetRefreshPendingRef.current = true;
          } else {
            void fleet.loadFleetTelemetry(true);
          }
        }
      });
      socket.addEventListener("close", () => {
        if (disposed) {
          return;
        }
        setWsState("reconnecting");
        if (documentIsHidden()) {
          hiddenOperatorProfileRefreshPendingRef.current = true;
        } else {
          void access.loadCurrentOperatorProfile();
        }
        const delay = Math.min(1_000 * 2 ** reconnectAttempt, 15_000);
        reconnectAttempt += 1;
        reconnectTimer = window.setTimeout(connect, delay);
      });
      socket.addEventListener("error", () => {
        if (!disposed) {
          setWsState("reconnecting");
        }
      });
      socket.addEventListener("message", (message) => {
        if (disposed) {
          return;
        }
        const event = parseWsEvent(message.data);
        if (!event) {
          return;
        }
        const currentView = activeViewRef.current;
        setLastLiveEvent(
          event.type === "fleet_telemetry_invalidated"
            ? "telemetry_updated"
            : event.type === "job_details_invalidated"
              ? "job_output_recorded"
              : event.type,
        );
        if (event.type === "fleet_snapshot") {
          fleet.replaceFleetSnapshot(event.summary, event.agents);
          return;
        }
        if (event.type === "fleet_telemetry_invalidated") {
          if (documentIsHidden()) {
            hiddenFleetRefreshPendingRef.current = true;
          } else {
            void fleet.loadFleetTelemetry(true);
          }
          const currentSubpage = activeSubpageRef.current;
          if (
            (currentView === "Network" &&
              [
                "overview",
                "graph",
                "tunnel_plans",
                "tests",
                "ospf",
                "evidence",
              ].includes(currentSubpage)) ||
            (currentView === "Observability" &&
              currentSubpage === "network_metrics")
          ) {
            scheduleNetworkEvidenceReload();
          }
        } else if (event.type === "fleet_state_invalidated") {
          // Suspension and deletion responses update the initiating view
          // immediately. They also change dependent alert/job state, so this
          // typed event owns one coalesced authoritative full reconciliation
          // for every browser, without separate tag-order, runtime-apply, or
          // patch-generator reads or an independent overview refresh.
          scheduleFleetReload();
        } else if (
          event.type === "agent_updated" ||
          event.type === "job_rejected"
        ) {
          scheduleFleetReload();
        }
        if (
          routeOwnsDashboardOverview(
            currentView,
            activeSubpageRef.current,
          ) &&
          (event.type === "agent_updated" || event.type === "job_rejected")
        ) {
          scheduleDashboardOverviewReload();
        }
        if (event.type === "job_rejected") {
          jobs.reconcileJobStatusEvent(
            event.job_id,
            event.status,
          );
          if (documentIsHidden()) {
            if (
              routeOwnsLiveJobHistory(
                currentView,
                activeSubpageRef.current,
              )
            ) {
              hiddenJobHistoryEventsRef.current.set(
                event.job_id,
                { refreshRenderedEffects: false, status: event.status },
              );
            }
          } else if (
            routeOwnsLiveJobHistory(
              currentView,
              activeSubpageRef.current,
            )
          ) {
            void jobs.refreshJobHistoryAfterEvent(
              event.job_id,
              event.status,
            );
          }
          if (currentView === "Audit") {
            if (documentIsHidden()) {
              hiddenAuditRefreshPendingRef.current = true;
            } else {
              void audit.loadAuditLogs();
            }
          }
        }
        if (event.type === "job_details_invalidated") {
          if (documentIsHidden()) {
            if (currentView === "Jobs") {
              for (const jobId of event.job_ids) {
                hiddenJobDetailIdsRef.current.add(jobId);
              }
            }
          } else if (currentView === "Jobs") {
            jobDetailsInvalidationGenerationRef.current += 1;
            setJobDetailsInvalidation({
              generation: jobDetailsInvalidationGenerationRef.current,
              job_ids: event.job_ids,
            });
          }
        }
        if (event.type === "job_finished") {
          scheduleFleetReload();
          const eventVisible = !documentIsHidden();
          const loadedJob = jobs.reconcileJobStatusEvent(
            event.job_id,
            event.status,
          );
          const refreshHistory =
            eventVisible &&
            (routeOwnsLiveJobHistory(
              currentView,
              activeSubpageRef.current,
            ) ||
              (!loadedJob &&
                viewNeedsFinishedJobClassification(
                  currentView,
                  activeSubpageRef.current,
                )));
          if (
            !eventVisible &&
            (routeOwnsLiveJobHistory(
              currentView,
              activeSubpageRef.current,
            ) ||
              viewNeedsFinishedJobClassification(
                currentView,
                activeSubpageRef.current,
              ))
          ) {
            hiddenJobHistoryEventsRef.current.set(event.job_id, {
              refreshRenderedEffects: true,
              status: event.status,
            });
          }
          const refreshedJob = refreshHistory
            ? jobs.refreshJobHistoryAfterEvent(event.job_id, event.status)
            : Promise.resolve(loadedJob);
          void refreshedJob.then((job) => {
            if (!job) {
              return;
            }
            if (!eventVisible) {
              return;
            }
            if (documentIsHidden()) {
              hiddenResolvedJobEffectsRef.current.set(job.id, job);
              return;
            }
            refreshRenderedJobEffects(job);
          });
          if (currentView === "Audit") {
            if (documentIsHidden()) {
              hiddenAuditRefreshPendingRef.current = true;
            } else {
              void audit.loadAuditLogs();
            }
          }
          if (
            routeOwnsDashboardOverview(
              currentView,
              activeSubpageRef.current,
            )
          ) {
            scheduleDashboardOverviewReload();
          }
        }
        if (event.type === "backup_artifact_recorded") {
          const currentSubpage = activeSubpageRef.current;
          const artifactSources = backupArtifactProjectionSourcesForRoute(
            currentView,
            currentSubpage,
          );
          if (artifactSources.length > 0) {
            if (documentIsHidden()) {
              hiddenBackupRefreshPendingRef.current = true;
            } else {
              refreshBackupArtifactProjectionsForRoute(
                currentView,
                currentSubpage,
              );
            }
          }
          if (currentView === "Audit") {
            if (documentIsHidden()) {
              hiddenAuditRefreshPendingRef.current = true;
            } else {
              void audit.loadAuditLogs();
            }
          }
          if (
            routeOwnsDashboardOverview(
              currentView,
              activeSubpageRef.current,
            )
          ) {
            scheduleDashboardOverviewReload();
          }
        }
      });
    };
    connect();
    return () => {
      disposed = true;
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
      }
      if (socket?.readyState === WebSocket.OPEN) {
        socket.close();
      }
    };
  }, [
    apiToken,
    access.loadCurrentOperatorProfile,
    audit.loadAuditLogs,
    fleet.replaceFleetSnapshot,
    fleet.loadFleetTelemetry,
    dashboardOverview.loadDashboardOverview,
    jobs.reconcileJobStatusEvent,
    jobs.refreshJobHistoryAfterEvent,
    refreshRenderedJobEffects,
    refreshBackupArtifactProjectionsForRoute,
    scheduleDashboardOverviewReload,
    scheduleFleetReload,
    scheduleNetworkEvidenceReload,
  ]);

  const handleAuth = useCallback(
    async (auth: AuthResponse) => {
      authGenerationRef.current += 1;
      refreshAuthRef.current = null;
      clearDashboardData();
      persistAuthSession(auth);
      setAuthRefreshError(null);
      setLogoutWarning(null);
      access.setAuthenticatedOperator(auth.operator);
      setWsState("connecting");
      setApiToken(auth.access_token);
      setAuthRequired(false);
    },
    [access.setAuthenticatedOperator, clearDashboardData],
  );

  const clearSession = useCallback(() => {
    const logoutToken = readStoredAccessToken() || apiToken;
    authGenerationRef.current += 1;
    const logoutGeneration = authGenerationRef.current;
    refreshAuthRef.current = null;
    clearDashboardData();
    clearStoredAuthSession();
    setAuthRefreshError(null);
    setLogoutWarning(null);
    setWsState("auth required");
    setApiToken("");
    setAuthRequired(true);
    if (logoutToken) {
      void apiPost<void>("/api/v1/auth/logout", logoutToken, {}).catch(() => {
        if (authGenerationRef.current === logoutGeneration) {
          setLogoutWarning(
            "Signed out locally, but the server could not revoke this session. It may remain active until it expires; sign in again to review Audit > Sessions when the API is available.",
          );
        }
      });
    }
  }, [apiToken, clearDashboardData]);

  return {
    accessError: activeAccessError,
    accessLoading: activeAccessLoading,
    agents: fleet.agents,
    apiError: fleet.apiError,
    apiToken,
    applyConfigurationSourceOverride:
      inventory.applyConfigurationSourceOverride,
    assignTag: inventory.assignTag,
    bulkMutateTags: inventory.bulkMutateTags,
    auditError: audit.auditError,
    auditEvidenceAvailable: audit.auditEvidenceAvailable,
    auditLoading: audit.auditLoading,
    audits: audit.audits,
    auditsTruncated: audit.auditsTruncated,
    historyExport: audit.historyExport,
    historyPruneResult: audit.historyPruneResult,
    historyRetentionPolicies: audit.historyRetentionPolicies,
    authRequired,
    authRefreshError,
    backupArtifacts: backups.backupArtifacts,
    backupArtifactsTruncated: backups.backupArtifactsTruncated,
    backupPolicies: backups.backupPolicies,
    backupPoliciesTruncated: backups.backupPoliciesTruncated,
    backups: backups.backups,
    backupsTruncated: backups.backupsTruncated,
    migrationLinks: backups.migrationLinks,
    restorePlans: backups.restorePlans,
    backupsError: activeBackupsError,
    backupsEvidenceAvailable: backups.backupsEvidenceAvailable,
    backupsLoading: activeBackupsLoading,
    clearSession,
    clearTunnelPlanEvidence: topology.clearTunnelPlanEvidence,
    clientKeyRevocations: access.clientKeyRevocations,
    clearOperatorTotp: access.clearOperatorTotp,
    clearOperatorTotps: access.clearOperatorTotps,
    cloneConfigurationPreset: inventory.cloneConfigurationPreset,
    configurationPresets: inventory.configurationPresets,
    configurationPresetsEvidenceAvailable:
      inventory.configurationPresetsEvidenceAvailable,
    configurationPresetsError: inventory.configurationPresetsError,
    configurationPresetsLoading: inventory.configurationPresetsLoading,
    configurationSourcesEvidenceAvailable:
      inventory.configurationSourcesEvidenceAvailable,
    configurationSourcesError: inventory.configurationSourcesError,
    configurationSourcesLoading: inventory.configurationSourcesLoading,
    configurationSources: inventory.configurationSources,
    confirmTotp: access.confirmTotp,
    documentVisible,
    createOperator: access.createOperator,
    updateAgentAlias: fleet.updateAgentAlias,
    upsertAgentIdentity: access.upsertAgentIdentity,
    createBackupRequest: backups.createBackupRequest,
    createBackupPolicy: backups.createBackupPolicy,
    updateBackupPolicy: backups.updateBackupPolicy,
    createFileTransferHandoff: jobs.createFileTransferHandoff,
    createMigrationLink: backups.createMigrationLink,
    createMigrationRun: backups.createMigrationRun,
    createRestorePlan: backups.createRestorePlan,
    downloadBackupArtifact: backups.downloadBackupArtifact,
    handoffBackupArtifact: backups.handoffBackupArtifact,
    pruneBackupPolicies: backups.pruneBackupPolicies,
    uploadBackupArtifact: backups.uploadBackupArtifact,
    uploadBackupArtifactChunked: backups.uploadBackupArtifactChunked,
    createJob: jobs.createJob,
    createJobApproval: jobs.createJobApproval,
    approveJobApproval: jobs.approveJobApproval,
    rejectJobApproval: jobs.rejectJobApproval,
    retryAuthRefresh: refreshStoredAuth,
    createArtifactCleanupJob: jobs.createArtifactCleanupJob,
    createAgentUpdateRelease: jobs.createAgentUpdateRelease,
    createConfigurationPreset: inventory.createConfigurationPreset,
    createSchedule: schedules.createSchedule,
    previewEventScheduleTemplate: schedules.previewEventScheduleTemplate,
    updateSchedule: schedules.updateSchedule,
    bulkUpdateScheduleTargets: schedules.bulkUpdateScheduleTargets,
    enableSchedule: schedules.enableSchedule,
    disableSchedule: schedules.disableSchedule,
    deferSchedule: schedules.deferSchedule,
    applyScheduleNow: schedules.applyScheduleNow,
    deleteSchedule: schedules.deleteSchedule,
    createTag: inventory.createTag,
    updateTagOrder: inventory.updateTagOrder,
    allocateTunnelEndpoints: topology.allocateTunnelEndpoints,
    createTunnelPlan: topology.createTunnelPlan,
    createNetworkAdapterDefinition: topology.createNetworkAdapterDefinition,
    createPortForwardRule: portForwarding.createPortForwardRule,
    updatePortForwardRule: portForwarding.updatePortForwardRule,
    mutatePortForwardRule: portForwarding.mutatePortForwardRule,
    bulkMutatePortForwardRules: portForwarding.bulkMutatePortForwardRules,
    resolvePortForwardHostname: portForwarding.resolvePortForwardHostname,
    deleteTunnelPlan: topology.deleteTunnelPlan,
    deleteNetworkAdapterDefinition: topology.deleteNetworkAdapterDefinition,
    exportTunnelPlan: topology.exportTunnelPlan,
    refreshTunnelPlanOspfStatus: topology.refreshTunnelPlanOspfStatus,
    rotateTunnelPlanCredentials: topology.rotateTunnelPlanCredentials,
    disableTotp: access.disableTotp,
    handleAuth,
    initialHomeMonitoringCards: homeSnapshotOwnsVisit
      ? homeMonitoringCards
      : undefined,
    initialHomeSnapshotPending:
      homeSnapshotOwnsVisit && homeSnapshotPendingKey === homeVisitKey,
    jobs: jobs.jobs,
    jobsTruncated: jobs.jobsTruncated,
    jobApprovals: jobs.jobApprovals,
    commandTemplates: jobs.commandTemplates,
    commandTemplatesTruncated: jobs.commandTemplatesTruncated,
    deleteCommandTemplate: jobs.deleteCommandTemplate,
    agentUpdateReleases: jobs.agentUpdateReleases,
    agentUpdateReleasesTruncated: jobs.agentUpdateReleasesTruncated,
    jobsError: activeJobsError,
    jobsEvidenceAvailable: jobs.jobsEvidenceAvailable,
    jobsLoading: activeJobsLoading,
    keyLifecycleReport: access.keyLifecycleReport,
    processSupervisorInventory: jobs.processSupervisorInventory,
    processSupervisorInventoryTruncated:
      jobs.processSupervisorInventoryTruncated,
    serverJobs: jobs.serverJobs,
    serverJobsError: jobs.serverJobsError,
    fileTransfers: jobs.fileTransfers,
    fileTransfersTruncated: jobs.fileTransfersTruncated,
    fileTransferSources: jobs.fileTransferSources,
    fileTransferSourcesTruncated: jobs.fileTransferSourcesTruncated,
    terminalSessions: jobs.terminalSessions,
    terminalSessionsTruncated: jobs.terminalSessionsTruncated,
    jobRolloutsTruncated: jobs.jobRolloutsTruncated,
    gatewaySessions: access.gatewaySessions,
    deleteAgents: fleet.deleteAgents,
    mutateAgentSuspensions: fleet.mutateAgentSuspensions,
    fleetAlertsEvidenceAvailable: fleet.fleetAlertsEvidenceAvailable,
    fleetAlerts: fleet.fleetAlerts,
    fleetAlertsTruncated: fleet.fleetAlertsTruncated,
    fleetAlertHistory: fleet.fleetAlertHistory,
    fleetAlertHistoryTruncated: fleet.fleetAlertHistoryTruncated,
    fleetAlertHistoryEvidenceAvailable:
      fleet.fleetAlertHistoryEvidenceAvailable,
    fleetAlertEventReviewItems: fleet.fleetAlertEventReviewItems,
    fleetAlertEventReviewHasMore: fleet.fleetAlertEventReviewHasMore,
    fleetAlertEventReviewStarted: fleet.fleetAlertEventReviewStarted,
    fleetAlertEventReviewLoading: fleet.fleetAlertEventReviewLoading,
    fleetAlertEventReviewError: fleet.fleetAlertEventReviewError,
    fleetAlertEventReviewLimitNotice: fleet.fleetAlertEventReviewLimitNotice,
    fleetAlertEventReviewVerified: fleet.fleetAlertEventReviewVerified,
    fleetAlertEventSearchHasMore: fleet.fleetAlertEventSearchHasMore,
    fleetAlertEventSearchQuery: fleet.fleetAlertEventSearchQuery,
    fleetAlertEventSearchScannedCount: fleet.fleetAlertEventSearchScannedCount,
    fleetAlertEventSearchItems: fleet.fleetAlertEventSearchItems,
    activateFleetAlertEventReview: fleet.activateFleetAlertEventReview,
    deactivateFleetAlertEventReview: fleet.deactivateFleetAlertEventReview,
    loadOlderFleetAlertEvents: fleet.loadOlderFleetAlertEvents,
    searchOlderFleetAlertEvents: fleet.searchOlderFleetAlertEvents,
    syncFleetAlertEvents: fleet.syncFleetAlertEvents,
    fleetAlertPolicies: fleet.fleetAlertPolicies,
    configPolicyEvidenceAvailable: fleet.configPolicyEvidenceAvailable,
    vpsRuleEvidenceAvailable: fleet.vpsRuleEvidenceAvailable,
    fleetCoreEvidenceAvailable: fleet.fleetCoreEvidenceAvailable,
    vpsRuleValues: fleet.vpsRuleValues,
    trafficAccounting: fleet.trafficAccounting,
    policyAlerts: fleet.policyAlerts,
    policyAlertsTruncated: fleet.policyAlertsTruncated,
    policyAlertsEvidenceAvailable: fleet.policyAlertsEvidenceAvailable,
    currentPolicyAlerts: fleet.currentPolicyAlerts,
    currentPolicyAlertsTruncated: fleet.currentPolicyAlertsTruncated,
    currentPolicyAlertsEvidenceAvailable:
      fleet.currentPolicyAlertsEvidenceAvailable,
    fleetAlertNotificationChannels: fleet.fleetAlertNotificationChannels,
    fleetAlertNotifications: fleet.fleetAlertNotifications,
    fleetAlertNotificationsTruncated: fleet.fleetAlertNotificationsTruncated,
    webhookRules: fleet.webhookRules,
    webhookRuleDeliveries: fleet.webhookRuleDeliveries,
    webhookRuleDeliveriesTruncated: fleet.webhookRuleDeliveriesTruncated,
    lastLiveEvent,
    jobDetailsInvalidation,
    terminalAccessToken: apiToken,
    loadAudits: audit.loadAudits,
    loadAuditLogs: audit.loadAuditLogs,
    loadAuditEvent: audit.loadAuditEvent,
    loadHistoryExport: audit.loadHistoryExport,
    loadBackupArtifacts: backups.loadBackupArtifacts,
    loadBackupPolicies: backups.loadBackupPolicies,
    loadBackupRequests: backups.loadBackupRequests,
    loadBackups: backups.loadBackups,
    loadMigrationLinks: backups.loadMigrationLinks,
    loadRestorePlans: backups.loadRestorePlans,
    loadAccessAuditSessions: access.loadAccessAuditSessions,
    loadAccessGatewaySessions: access.loadAccessGatewaySessions,
    loadAccessOperators: access.loadAccessOperators,
    loadAccessOverview: access.loadAccessOverview,
    loadAccessVpsIdentities: access.loadAccessVpsIdentities,
    loadCurrentOperator: access.loadCurrentOperator,
    loadCurrentOperatorProfile: access.loadCurrentOperatorProfile,
    downloadFileTransferHandoff: jobs.downloadFileTransferHandoff,
    downloadFileTransferSource: jobs.downloadFileTransferSource,
    downloadFileDownloadBundle: jobs.downloadFileDownloadBundle,
    downloadJobOutputChunk: jobs.downloadJobOutputChunk,
    downloadJobOutputStream: jobs.downloadJobOutputStream,
    downloadFileDownloadForClient: jobs.downloadFileDownloadForClient,
    downloadJobOutputArchive: jobs.downloadJobOutputArchive,
    downloadJobTargetStatuses: jobs.downloadJobTargetStatuses,
    saveFileTransferHandoff: jobs.saveFileTransferHandoff,
    loadJob: jobs.loadJob,
    loadJobRollout: jobs.loadJobRollout,
    loadJobRollouts: jobs.loadJobRollouts,
    loadJobOutputs: jobs.loadJobOutputs,
    loadJobOutputComparison: jobs.loadJobOutputComparison,
    loadExactJobTargetStatuses: jobs.loadExactJobTargetStatuses,
    loadCommandTemplates: jobs.loadCommandTemplates,
    loadFileTransfers: jobs.loadFileTransfers,
    loadFileTransferSources: jobs.loadFileTransferSources,
    loadJobApprovals: jobs.loadJobApprovals,
    loadJobHistory: jobs.loadJobHistory,
    loadJobs: jobs.loadJobs,
    loadProcessSupervisorInventory: jobs.loadProcessSupervisorInventory,
    loadServerJobs: jobs.loadServerJobs,
    loadTerminalSessions: jobs.loadTerminalSessions,
    loadTerminalReplay: jobs.loadTerminalReplay,
    loadAgentUpdateReleases: jobs.loadAgentUpdateReleases,
    loadJobTargets: jobs.loadJobTargets,
    loadHostProcessInventory: jobs.loadHostProcessInventory,
    loadHostPackageUpdatePlan: jobs.loadHostPackageUpdatePlan,
    loadHostPackageUpdatePlans: jobs.loadHostPackageUpdatePlans,
    loadHostServiceInventory: jobs.loadHostServiceInventory,
    loadHostStorageInventory: jobs.loadHostStorageInventory,
    jobRollouts: jobs.jobRollouts,
    updateJobRollout: jobs.updateJobRollout,
    bulkUpdateFleetAlertStates: fleet.bulkUpdateFleetAlertStates,
    resolveFleetAlert: fleet.resolveFleetAlert,
    dryRunFleetAlertPolicy: fleet.dryRunFleetAlertPolicy,
    upsertFleetAlertNotificationChannel:
      fleet.upsertFleetAlertNotificationChannel,
    bulkMutateFleetAlertNotificationChannels:
      fleet.bulkMutateFleetAlertNotificationChannels,
    dispatchFleetAlertNotifications: fleet.dispatchFleetAlertNotifications,
    processFleetAlertNotifications: fleet.processFleetAlertNotifications,
    upsertWebhookRule: fleet.upsertWebhookRule,
    bulkMutateWebhookRules: fleet.bulkMutateWebhookRules,
    dryRunWebhookRule: fleet.dryRunWebhookRule,
    dispatchWebhookRules: fleet.dispatchWebhookRules,
    processWebhookRuleDeliveries: fleet.processWebhookRuleDeliveries,
    rotateWebhookDeliveryHistory: fleet.rotateWebhookDeliveryHistory,
    uploadFileTransferSource: jobs.uploadFileTransferSource,
    cancelServerJob: jobs.cancelServerJob,
    cancelJob: jobs.cancelJob,
    previewArtifactCleanup: jobs.previewArtifactCleanup,
    loadTagInventory: inventory.loadTagInventory,
    loadTagOrder: inventory.loadTagOrder,
    deleteConfigurationPreset: inventory.deleteConfigurationPreset,
    loadConfigurationInventory: inventory.loadConfigurationInventory,
    loadConfigurationSources: inventory.loadConfigurationSources,
    loadRuntimeConfigApplyStates: inventory.loadRuntimeConfigApplyStates,
    loadSchedules: schedules.loadSchedules,
    loadNetworkObservations: topology.loadNetworkObservations,
    queryNetworkObservations: topology.queryNetworkObservations,
    loadNetworkAdapterDefinitions: topology.loadNetworkAdapterDefinitions,
    loadNetworkTrends: topology.loadNetworkTrends,
    loadOspfRecommendations: topology.loadOspfRecommendations,
    loadOspfUpdatePlans: topology.loadOspfUpdatePlans,
    loadTopologyGraph: topology.loadTopologyGraph,
    loadTunnelPlans: topology.loadTunnelPlans,
    loadPortForwardRules: portForwarding.loadPortForwardRules,
    setTunnelPlanEnabled: topology.setTunnelPlanEnabled,
    updateTunnelConnectionAssessment: topology.updateTunnelConnectionAssessment,
    updateNetworkAdapterDefinition: topology.updateNetworkAdapterDefinition,
    updateTunnelPlanOspfCost: topology.updateTunnelPlanOspfCost,
    updateTunnelPlan: topology.updateTunnelPlan,
    networkObservations: topology.networkObservations,
    networkAdapterDefinitions: topology.networkAdapterDefinitions,
    networkTrends: topology.networkTrends,
    ospfRecommendations: topology.ospfRecommendations,
    ospfUpdatePlans: topology.ospfUpdatePlans,
    operator: access.operator,
    operatorAuthEvents: access.operatorAuthEvents,
    operatorAuthEventsTruncated: access.operatorAuthEventsTruncated,
    operators: access.operators,
    operatorSessions: access.operatorSessions,
    operatorSessionsTruncated: access.operatorSessionsTruncated,
    preferencesError: access.preferencesError,
    preferencesSaving: access.preferencesSaving,
    deleteRuntimeConfigPatchGenerator:
      inventory.deleteRuntimeConfigPatchGenerator,
    deleteTag: inventory.deleteTag,
    dashboardOverview: dashboardOverview.dashboardOverview,
    dashboardOverviewError: dashboardOverview.dashboardOverviewError,
    dashboardOverviewLoading: dashboardOverview.dashboardOverviewLoading,
    dashboardOverviewWindow: dashboardOverview.dashboardOverviewWindow,
    dashboardPreferences: dashboardOverview.dashboardPreferences,
    loadDashboardOverview: dashboardOverview.loadDashboardOverview,
    setDashboardOverviewWindow: dashboardOverview.setDashboardOverviewWindow,
    updateDashboardPreferences: dashboardOverview.updateDashboardPreferences,
    loadEffectiveAgentConfig: inventory.loadEffectiveAgentConfig,
    loadRuntimeConfigClientWorkspace:
      inventory.loadRuntimeConfigClientWorkspace,
    previewRuntimeConfigOverride: inventory.previewRuntimeConfigOverride,
    applyRuntimeConfigOverride: inventory.applyRuntimeConfigOverride,
    previewRuntimeConfigBulkOverride:
      inventory.previewRuntimeConfigBulkOverride,
    applyRuntimeConfigBulkOverride: inventory.applyRuntimeConfigBulkOverride,
    previewConfigurationPreset: inventory.previewConfigurationPreset,
    previewConfigurationSourceOverride:
      inventory.previewConfigurationSourceOverride,
    renderRuntimeConfigPatchGenerator:
      inventory.renderRuntimeConfigPatchGenerator,
    resolveBulkPreview: inventory.resolveBulkPreview,
    resolveManyJobTargets: inventory.resolveManyJobTargets,
    resolveJobTargets: inventory.resolveJobTargets,
    revokeClientKey: access.revokeClientKey,
    revokeOperatorSession: access.revokeOperatorSession,
    revokeOperatorSessions: access.revokeOperatorSessions,
    resetOperatorPassword: access.resetOperatorPassword,
    pruneHistoryRetention: audit.pruneHistoryRetention,
    setupTotp: access.setupTotp,
    schedules: schedules.schedules,
    schedulesTruncated: schedules.schedulesTruncated,
    schedulesError: schedules.schedulesError,
    schedulesEvidenceAvailable: schedules.schedulesEvidenceAvailable,
    schedulesLoading: schedules.schedulesLoading,
    summary: fleet.summary,
    systemDashboard: system.systemDashboard,
    systemDashboardError: system.systemDashboardError,
    systemDashboardLoading: system.systemDashboardLoading,
    systemDashboardPointDensity: system.systemDashboardPointDensity,
    systemDashboardWindow: system.systemDashboardWindow,
    logoutWarning,
    setSystemDashboardPointDensity: system.setSystemDashboardPointDensity,
    setSystemDashboardWindow: system.setSystemDashboardWindow,
    setOperatorStatus: access.setOperatorStatus,
    setOperatorStatuses: access.setOperatorStatuses,
    loadSystemDashboard: system.loadSystemDashboard,
    suiteConfig: system.suiteConfig,
    suiteConfigError: system.suiteConfigError,
    suiteConfigLoading: system.suiteConfigLoading,
    loadSuiteConfig: system.loadSuiteConfig,
    validateSuiteConfig: system.validateSuiteConfig,
    updateSuiteConfig: system.updateSuiteConfig,
    telemetryNetworkRates: fleet.telemetryNetworkRates,
    telemetryTunnels: fleet.telemetryTunnels,
    telemetryUptimes: fleet.telemetryUptimes,
    tags: inventory.tags,
    namespaceNaturalSortEnabled: inventory.namespaceNaturalSortEnabled,
    tagsError: inventory.tagsError,
    tagsLoading: inventory.tagsLoading,
    tagInventoryEvidenceAvailable: inventory.tagInventoryEvidenceAvailable,
    runtimeConfigApplyEvidenceAvailable:
      inventory.runtimeConfigApplyEvidenceAvailable,
    runtimeConfigApplyError: inventory.runtimeConfigApplyError,
    runtimeConfigApplyLoading: inventory.runtimeConfigApplyLoading,
    runtimeConfigApplyStates: inventory.runtimeConfigApplyStates,
    runtimeConfigPatchGenerators: inventory.runtimeConfigPatchGenerators,
    telemetryRollups: fleet.telemetryRollups,
    topologyError: activeTopologyError,
    topologyGraph: topology.topologyGraph,
    topologyLoading: activeTopologyLoading,
    tunnelPlanCorruptions: topology.tunnelPlanCorruptions,
    tunnelPlans: topology.tunnelPlans,
    portForwardRules: portForwarding.portForwardRules,
    portForwardError: portForwarding.portForwardError,
    portForwardLoading: portForwarding.portForwardLoading,
    updateConfigurationPreset: inventory.updateConfigurationPreset,
    updateOperator: access.updateOperator,
    upsertRuntimeConfigPatchGenerator:
      inventory.upsertRuntimeConfigPatchGenerator,
    upsertCommandTemplate: jobs.upsertCommandTemplate,
    upsertHistoryRetentionPolicy: audit.upsertHistoryRetentionPolicy,
    upsertFleetAlertPolicy: fleet.upsertFleetAlertPolicy,
    loadEffectiveVpsRules: fleet.loadEffectiveVpsRules,
    dryRunVpsRules: fleet.dryRunVpsRules,
    bulkUpsertVpsRules: fleet.bulkUpsertVpsRules,
    bulkUnsetVpsRules: fleet.bulkUnsetVpsRules,
    bulkMutateFleetAlertPolicies: fleet.bulkMutateFleetAlertPolicies,
    updateOperatorPreferences: access.updateOperatorPreferences,
    wsState,
  };
}

function dashboardRefreshIntervalMs(
  value: DashboardRefreshIntervalSecs,
): number {
  return value * 1000;
}
