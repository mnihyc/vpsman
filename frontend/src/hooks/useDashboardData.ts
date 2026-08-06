import { useCallback, useEffect, useRef, useState } from "react";
import {
  ACCESS_TOKEN_STORAGE_KEY,
  REFRESH_TOKEN_STORAGE_KEY,
} from "../constants";
import { apiPost, isApiUnauthorized } from "../api";
import type {
  ActiveView,
  AuthResponse,
  DashboardRefreshIntervalSecs,
  WsJobOutputEvent,
} from "../types";
import { parseWsEvent } from "../utils";
import { useAccessData } from "./useAccessData";
import { useAuditData } from "./useAuditData";
import { useBackupsData } from "./useBackupsData";
import { useDashboardOverviewData } from "./useDashboardOverviewData";
import { useFleetData } from "./useFleetData";
import { useInventoryData } from "./useInventoryData";
import { useJobsData } from "./useJobsData";
import { usePortForwardingData } from "./usePortForwardingData";
import { useSchedulesData } from "./useSchedulesData";
import { useSystemData } from "./useSystemData";
import { useTopologyData } from "./useTopologyData";

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

function persistAuthSession(auth: AuthResponse): void {
  window.localStorage.setItem(ACCESS_TOKEN_STORAGE_KEY, auth.access_token);
  window.localStorage.setItem(REFRESH_TOKEN_STORAGE_KEY, auth.refresh_token);
}

function clearStoredAuthSession(): void {
  window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
  window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
}

export function useDashboardData(activeView: ActiveView) {
  const [apiToken, setApiToken] = useState(() => readStoredAccessToken());
  const [authRequired, setAuthRequired] = useState(
    () => !hasStoredAuthSession(),
  );
  const [wsState, setWsState] = useState("connecting");
  const [lastLiveEvent, setLastLiveEvent] = useState("waiting");
  const [lastJobOutputEvent, setLastJobOutputEvent] =
    useState<WsJobOutputEvent | null>(null);
  const [authRefreshError, setAuthRefreshError] = useState<string | null>(null);
  const [logoutWarning, setLogoutWarning] = useState<string | null>(null);
  const dashboardOverviewReloadTimer = useRef<number | null>(null);
  const fleetReloadTimer = useRef<number | null>(null);
  const fleetTelemetryReloadTimer = useRef<number | null>(null);
  const fleetTelemetryReloadedAt = useRef(0);
  const inventoryReloadTimer = useRef<number | null>(null);
  const topologyReloadTimer = useRef<number | null>(null);
  const refreshAuthRef = useRef<Promise<void> | null>(null);
  const authGenerationRef = useRef(0);
  const clearDashboardDataRef = useRef<() => void>(() => undefined);
  const activeViewRef = useRef(activeView);

  useEffect(() => {
    activeViewRef.current = activeView;
  }, [activeView]);

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
    const request = apiPost<AuthResponse>(
      "/api/v1/auth/refresh",
      "",
      { refresh_token: refreshToken },
    )
      .then((auth) => {
        if (authGeneration !== authGenerationRef.current) {
          return;
        }
        persistAuthSession(auth);
        setAuthRefreshError(null);
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
  const dashboardOverview = useDashboardOverviewData(apiToken, requireAuth);
  const fleet = useFleetData(apiToken, requireAuth);
  const audit = useAuditData(apiToken, requireAuth);
  const inventory = useInventoryData(apiToken, requireAuth, fleet.loadFleet);
  const jobs = useJobsData(
    apiToken,
    requireAuth,
    fleet.loadFleet,
    audit.loadAudits,
  );
  const schedules = useSchedulesData(apiToken, requireAuth, audit.loadAudits);
  const system = useSystemData(apiToken, requireAuth);
  const topology = useTopologyData(
    apiToken,
    requireAuth,
    audit.loadAudits,
    inventory.loadRuntimeConfigApplyStates,
  );
  const portForwarding = usePortForwardingData(
    apiToken,
    requireAuth,
    audit.loadAudits,
  );
  const backups = useBackupsData(apiToken, requireAuth, audit.loadAudits);
  const clearDashboardData = useCallback(() => {
    for (const timer of [
      dashboardOverviewReloadTimer,
      fleetReloadTimer,
      fleetTelemetryReloadTimer,
      inventoryReloadTimer,
      topologyReloadTimer,
    ]) {
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
        timer.current = null;
      }
    }
    fleetTelemetryReloadedAt.current = 0;
    setLastLiveEvent("waiting");
    setLastJobOutputEvent(null);
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
    if (dashboardOverviewReloadTimer.current !== null) {
      window.clearTimeout(dashboardOverviewReloadTimer.current);
    }
    dashboardOverviewReloadTimer.current = window.setTimeout(() => {
      dashboardOverviewReloadTimer.current = null;
      void dashboardOverview.loadDashboardOverview();
    }, 250);
  }, [dashboardOverview.loadDashboardOverview]);
  const scheduleFleetReload = useCallback(() => {
    if (fleetReloadTimer.current !== null) {
      return;
    }
    if (fleetTelemetryReloadTimer.current !== null) {
      window.clearTimeout(fleetTelemetryReloadTimer.current);
      fleetTelemetryReloadTimer.current = null;
    }
    fleetReloadTimer.current = window.setTimeout(() => {
      fleetReloadTimer.current = null;
      fleetTelemetryReloadedAt.current = Date.now();
      void fleet.loadFleet();
    }, 750);
  }, [fleet.loadFleet]);
  const scheduleFleetTelemetryReload = useCallback(() => {
    if (fleetTelemetryReloadTimer.current !== null) {
      return;
    }
    const elapsed = Date.now() - fleetTelemetryReloadedAt.current;
    const delay = Math.max(0, 5_000 - elapsed);
    fleetTelemetryReloadTimer.current = window.setTimeout(() => {
      fleetTelemetryReloadTimer.current = null;
      fleetTelemetryReloadedAt.current = Date.now();
      void fleet.loadFleetTelemetry();
    }, delay);
  }, [fleet.loadFleetTelemetry]);
  const scheduleInventoryReload = useCallback(() => {
    if (inventoryReloadTimer.current !== null) {
      return;
    }
    inventoryReloadTimer.current = window.setTimeout(() => {
      inventoryReloadTimer.current = null;
      void inventory.loadTagInventory();
    }, 1_000);
  }, [inventory.loadTagInventory]);
  const scheduleTopologyReload = useCallback(() => {
    if (topologyReloadTimer.current !== null) {
      return;
    }
    topologyReloadTimer.current = window.setTimeout(() => {
      topologyReloadTimer.current = null;
      void Promise.all([
        topology.loadTunnelPlans(),
        topology.loadNetworkAdapterDefinitions(),
        topology.loadTopologyGraph(),
        topology.loadNetworkObservations(),
        topology.loadNetworkTrends(),
        topology.loadOspfRecommendations(),
        topology.loadOspfUpdatePlans(),
        portForwarding.loadPortForwardRules(),
      ]);
    }, 500);
  }, [
    topology.loadNetworkObservations,
    topology.loadNetworkAdapterDefinitions,
    topology.loadNetworkTrends,
    topology.loadOspfRecommendations,
    topology.loadOspfUpdatePlans,
    topology.loadTopologyGraph,
    topology.loadTunnelPlans,
    portForwarding.loadPortForwardRules,
  ]);

  useEffect(
    () => () => {
      if (dashboardOverviewReloadTimer.current !== null) {
        window.clearTimeout(dashboardOverviewReloadTimer.current);
      }
      if (fleetReloadTimer.current !== null) {
        window.clearTimeout(fleetReloadTimer.current);
      }
      if (fleetTelemetryReloadTimer.current !== null) {
        window.clearTimeout(fleetTelemetryReloadTimer.current);
      }
      if (inventoryReloadTimer.current !== null) {
        window.clearTimeout(inventoryReloadTimer.current);
      }
      if (topologyReloadTimer.current !== null) {
        window.clearTimeout(topologyReloadTimer.current);
      }
    },
    [],
  );

  useEffect(() => {
    if (!apiToken && hasStoredAuthSession()) {
      void refreshStoredAuth();
    }
  }, [apiToken, refreshStoredAuth]);

  useEffect(() => {
    if (!apiToken) {
      return;
    }
    void access.loadCurrentOperatorProfile();
  }, [access.loadCurrentOperatorProfile, apiToken]);

  useEffect(() => {
    if (!apiToken) {
      return;
    }
    let disposed = false;
    let tick = 0;
    let refreshInFlight = false;

    async function loadIfActive() {
      if (disposed || refreshInFlight) {
        return;
      }
      refreshInFlight = true;
      try {
        if (tick === 0 || tick % 4 === 0) {
          await fleet.loadFleet();
        } else {
          await fleet.loadFleetTelemetry();
        }
        tick += 1;
      } finally {
        refreshInFlight = false;
      }
    }

    loadIfActive();
    const timer = window.setInterval(loadIfActive, 15_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [apiToken, fleet.loadFleet, fleet.loadFleetTelemetry]);

  useEffect(() => {
    if (
      !apiToken ||
      (activeView !== "Home" && activeView !== "Observability")
    ) {
      return;
    }
    let disposed = false;
    let timer: number | null = null;

    async function loadAndSchedule() {
      await dashboardOverview.loadDashboardOverview();
      if (disposed) {
        return;
      }
      timer = window.setTimeout(
        loadAndSchedule,
        dashboardRefreshIntervalMs(
          dashboardOverview.dashboardPreferences.refreshIntervalSecs,
        ),
      );
    }

    void loadAndSchedule();
    return () => {
      disposed = true;
      if (timer !== null) {
        window.clearTimeout(timer);
      }
    };
  }, [
    activeView,
    apiToken,
    dashboardOverview.dashboardPreferences.refreshIntervalSecs,
    dashboardOverview.loadDashboardOverview,
  ]);

  useEffect(() => {
    if (!apiToken) {
      return;
    }
    if (activeView === "Home") {
      void jobs.loadJobs();
      void backups.loadBackups();
      void audit.loadAudits();
      void schedules.loadSchedules();
      void system.loadSystemDashboard();
    } else if (activeView === "Fleet") {
      void inventory.loadTagInventory();
    } else if (activeView === "Config") {
      void inventory.loadTagInventory();
      void jobs.loadJobs();
    } else if (activeView === "Remote Operations") {
      void jobs.loadJobs();
      void inventory.loadTagInventory();
    } else if (activeView === "Jobs") {
      void jobs.loadJobs();
      void backups.loadBackups();
      void inventory.loadTagInventory();
    } else if (activeView === "Automation") {
      void schedules.loadSchedules();
      void jobs.loadJobs();
      void inventory.loadTagInventory();
      if (access.operator?.role === "admin") {
        void system.loadSuiteConfig();
      }
    } else if (activeView === "Network") {
      void inventory.loadRuntimeConfigApplyStates();
      void portForwarding.loadPortForwardRules();
      void topology.loadTunnelPlans();
      void topology.loadNetworkAdapterDefinitions();
      void topology.loadNetworkObservations();
      void topology.loadNetworkTrends();
      void topology.loadOspfRecommendations();
      void topology.loadOspfUpdatePlans();
      void topology.loadTopologyGraph();
      void jobs.loadJobs();
    } else if (activeView === "Backups") {
      void backups.loadBackups();
      void jobs.loadJobs();
    } else if (activeView === "Observability") {
      void inventory.loadTagInventory();
      void topology.loadTunnelPlans();
      void topology.loadNetworkObservations();
      void topology.loadNetworkTrends();
      void topology.loadOspfRecommendations();
      void jobs.loadJobs();
      void backups.loadBackups();
    } else if (activeView === "Audit") {
      void audit.loadAudits();
      void jobs.loadJobs();
      void access.loadCurrentOperator();
    } else if (activeView === "Access") {
      void access.loadCurrentOperator();
      void inventory.loadTagInventory();
    } else if (activeView === "System") {
      void access.loadCurrentOperator();
      void inventory.loadTagInventory();
      void system.loadSystemDashboard();
      if (access.operator?.role === "admin") {
        void system.loadSuiteConfig();
      }
    }
  }, [
    access.loadCurrentOperator,
    access.operator?.role,
    activeView,
    apiToken,
    audit.loadAudits,
    backups.loadBackups,
    inventory.loadTagInventory,
    inventory.loadRuntimeConfigApplyStates,
    jobs.loadJobs,
    schedules.loadSchedules,
    system.loadSuiteConfig,
    system.loadSystemDashboard,
    topology.loadNetworkObservations,
    topology.loadNetworkAdapterDefinitions,
    topology.loadNetworkTrends,
    topology.loadOspfRecommendations,
    topology.loadOspfUpdatePlans,
    topology.loadTopologyGraph,
    topology.loadTunnelPlans,
    portForwarding.loadPortForwardRules,
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
        reconnectAttempt = 0;
        socket?.send(JSON.stringify({ type: "auth", access_token: apiToken }));
        setWsState("connected");
      });
      socket.addEventListener("close", () => {
        if (disposed) {
          return;
        }
        setWsState("reconnecting");
        void access.loadCurrentOperator();
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
        setLastLiveEvent(event.type);
        if (event.type === "fleet_snapshot") {
          fleet.replaceFleetSnapshot(event.summary, event.agents);
          return;
        }
        if (event.type === "telemetry_updated") {
          scheduleFleetTelemetryReload();
        } else if (
          event.type === "agent_updated" ||
          event.type === "job_rejected"
        ) {
          scheduleFleetReload();
        }
        if (
          (currentView === "Home" || currentView === "Observability") &&
          (event.type === "agent_updated" ||
            event.type === "job_rejected")
        ) {
          scheduleDashboardOverviewReload();
        }
        if (
          event.type === "agent_updated" &&
          activeViewUsesInventoryData(currentView)
        ) {
          scheduleInventoryReload();
        }
        if (event.type === "job_rejected") {
          void jobs.loadJobs();
          void audit.loadAudits();
        }
        if (event.type === "job_output_recorded") {
          setLastJobOutputEvent(event);
          if (currentView === "Jobs" || currentView === "Remote Operations") {
            void jobs.loadJobs();
          }
        }
        if (event.type === "job_finished") {
          scheduleFleetReload();
          void jobs.loadJobs();
          void audit.loadAudits();
          if (activeViewUsesInventoryData(currentView)) {
            scheduleInventoryReload();
          }
          if (currentView === "Home" || currentView === "Observability") {
            scheduleDashboardOverviewReload();
          }
          if (currentView === "Network") {
            scheduleTopologyReload();
          }
        }
        if (event.type === "backup_artifact_recorded") {
          void backups.loadBackups();
          void audit.loadAudits();
          if (currentView === "Home" || currentView === "Observability") {
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
    access.loadCurrentOperator,
    audit.loadAudits,
    fleet.replaceFleetSnapshot,
    backups.loadBackups,
    dashboardOverview.loadDashboardOverview,
    jobs.loadJobs,
    scheduleDashboardOverviewReload,
    scheduleFleetReload,
    scheduleFleetTelemetryReload,
    scheduleInventoryReload,
    scheduleTopologyReload,
    portForwarding.loadPortForwardRules,
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
      void apiPost<void>("/api/v1/auth/logout", logoutToken, {}).catch(
        () => {
          if (authGenerationRef.current === logoutGeneration) {
            setLogoutWarning(
              "Signed out locally, but the server could not revoke this session. It may remain active until it expires; sign in again to review Audit > Sessions when the API is available.",
            );
          }
        },
      );
    }
  }, [
    apiToken,
    clearDashboardData,
  ]);

  return {
    accessError: access.accessError,
    accessLoading: access.accessLoading,
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
    backupsError: backups.backupsError,
    backupsEvidenceAvailable: backups.backupsEvidenceAvailable,
    backupsLoading: backups.backupsLoading,
    clearSession,
    clientKeyRevocations: access.clientKeyRevocations,
    clearOperatorTotp: access.clearOperatorTotp,
    cloneConfigurationPreset: inventory.cloneConfigurationPreset,
    configurationPresets: inventory.configurationPresets,
    configurationSourcesEvidenceAvailable:
      inventory.configurationSourcesEvidenceAvailable,
    configurationSourcesError: inventory.configurationSourcesError,
    configurationSourcesLoading: inventory.configurationSourcesLoading,
    configurationSources: inventory.configurationSources,
    confirmTotp: access.confirmTotp,
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
    updateSchedule: schedules.updateSchedule,
    updateScheduleTargets: schedules.updateScheduleTargets,
    enableSchedule: schedules.enableSchedule,
    disableSchedule: schedules.disableSchedule,
    deferSchedule: schedules.deferSchedule,
    applyScheduleNow: schedules.applyScheduleNow,
    deleteSchedule: schedules.deleteSchedule,
    createTag: inventory.createTag,
    updateTagOrder: inventory.updateTagOrder,
    allocateTunnelEndpoints: topology.allocateTunnelEndpoints,
    createTunnelPlan: topology.createTunnelPlan,
    createNetworkAdapterDefinition:
      topology.createNetworkAdapterDefinition,
    createPortForwardRule: portForwarding.createPortForwardRule,
    updatePortForwardRule: portForwarding.updatePortForwardRule,
    mutatePortForwardRule: portForwarding.mutatePortForwardRule,
    bulkMutatePortForwardRules: portForwarding.bulkMutatePortForwardRules,
    resolvePortForwardHostname: portForwarding.resolvePortForwardHostname,
    deleteTunnelPlan: topology.deleteTunnelPlan,
    deleteNetworkAdapterDefinition:
      topology.deleteNetworkAdapterDefinition,
    exportTunnelPlan: topology.exportTunnelPlan,
    refreshTunnelPlanOspfStatus: topology.refreshTunnelPlanOspfStatus,
    rotateTunnelPlanCredentials: topology.rotateTunnelPlanCredentials,
    disableTotp: access.disableTotp,
    handleAuth,
    jobs: jobs.jobs,
    jobsTruncated: jobs.jobsTruncated,
    jobApprovals: jobs.jobApprovals,
    commandTemplates: jobs.commandTemplates,
    commandTemplatesTruncated: jobs.commandTemplatesTruncated,
    deleteCommandTemplate: jobs.deleteCommandTemplate,
    agentUpdateReleases: jobs.agentUpdateReleases,
    agentUpdateReleasesTruncated: jobs.agentUpdateReleasesTruncated,
    jobsError: jobs.jobsError,
    jobsEvidenceAvailable: jobs.jobsEvidenceAvailable,
    jobsLoading: jobs.jobsLoading,
    keyLifecycleReport: access.keyLifecycleReport,
    processSupervisorInventory: jobs.processSupervisorInventory,
    processSupervisorInventoryTruncated: jobs.processSupervisorInventoryTruncated,
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
    deleteAgent: fleet.deleteAgent,
    fleetAlertsEvidenceAvailable: fleet.fleetAlertsEvidenceAvailable,
    fleetAlerts: fleet.fleetAlerts,
    fleetAlertStates: fleet.fleetAlertStates,
    fleetAlertPolicies: fleet.fleetAlertPolicies,
    configPolicyEvidenceAvailable: fleet.configPolicyEvidenceAvailable,
    fleetCoreEvidenceAvailable: fleet.fleetCoreEvidenceAvailable,
    vpsRuleValues: fleet.vpsRuleValues,
    trafficAccounting: fleet.trafficAccounting,
    policyAlerts: fleet.policyAlerts,
    fleetAlertNotificationChannels: fleet.fleetAlertNotificationChannels,
    fleetAlertNotifications: fleet.fleetAlertNotifications,
    webhookRules: fleet.webhookRules,
    webhookRuleDeliveries: fleet.webhookRuleDeliveries,
    lastLiveEvent,
    lastJobOutputEvent,
    terminalAccessToken: apiToken,
    loadAudits: audit.loadAudits,
    loadAuditEvent: audit.loadAuditEvent,
    loadHistoryExport: audit.loadHistoryExport,
    loadBackups: backups.loadBackups,
    loadCurrentOperator: access.loadCurrentOperator,
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
    loadJobs: jobs.loadJobs,
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
    updateFleetAlertState: fleet.updateFleetAlertState,
    dryRunFleetAlertPolicy: fleet.dryRunFleetAlertPolicy,
    upsertFleetAlertNotificationChannel:
      fleet.upsertFleetAlertNotificationChannel,
    deleteFleetAlertNotificationChannel:
      fleet.deleteFleetAlertNotificationChannel,
    dispatchFleetAlertNotifications: fleet.dispatchFleetAlertNotifications,
    processFleetAlertNotifications: fleet.processFleetAlertNotifications,
    upsertWebhookRule: fleet.upsertWebhookRule,
    deleteWebhookRule: fleet.deleteWebhookRule,
    dryRunWebhookRule: fleet.dryRunWebhookRule,
    dispatchWebhookRules: fleet.dispatchWebhookRules,
    processWebhookRuleDeliveries: fleet.processWebhookRuleDeliveries,
    rotateWebhookDeliveryHistory: fleet.rotateWebhookDeliveryHistory,
    uploadFileTransferSource: jobs.uploadFileTransferSource,
    cancelServerJob: jobs.cancelServerJob,
    cancelJob: jobs.cancelJob,
    previewArtifactCleanup: jobs.previewArtifactCleanup,
    loadTagInventory: inventory.loadTagInventory,
    deleteConfigurationPreset: inventory.deleteConfigurationPreset,
    loadConfigurationSources: inventory.loadConfigurationSources,
    loadRuntimeConfigApplyStates: inventory.loadRuntimeConfigApplyStates,
    loadSchedules: schedules.loadSchedules,
    loadNetworkObservations: topology.loadNetworkObservations,
    loadNetworkAdapterDefinitions:
      topology.loadNetworkAdapterDefinitions,
    loadNetworkTrends: topology.loadNetworkTrends,
    loadOspfRecommendations: topology.loadOspfRecommendations,
    loadOspfUpdatePlans: topology.loadOspfUpdatePlans,
    loadTopologyGraph: topology.loadTopologyGraph,
    loadTunnelPlans: topology.loadTunnelPlans,
    loadPortForwardRules: portForwarding.loadPortForwardRules,
    setTunnelPlanEnabled: topology.setTunnelPlanEnabled,
    updateTunnelConnectionAssessment: topology.updateTunnelConnectionAssessment,
    updateNetworkAdapterDefinition:
      topology.updateNetworkAdapterDefinition,
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
    deleteRuntimeConfigPatchGenerator: inventory.deleteRuntimeConfigPatchGenerator,
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
    submitRuntimeConfigPatch: inventory.submitRuntimeConfigPatch,
    previewConfigurationPreset: inventory.previewConfigurationPreset,
    previewConfigurationSourceOverride:
      inventory.previewConfigurationSourceOverride,
    renderRuntimeConfigPatchGenerator: inventory.renderRuntimeConfigPatchGenerator,
    resolveBulkPreview: inventory.resolveBulkPreview,
    resolveJobTargets: inventory.resolveJobTargets,
    revokeClientKey: access.revokeClientKey,
    revokeOperatorSession: access.revokeOperatorSession,
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
    loadSystemDashboard: system.loadSystemDashboard,
    suiteConfig: system.suiteConfig,
    suiteConfigError: system.suiteConfigError,
    suiteConfigLoading: system.suiteConfigLoading,
    loadSuiteConfig: system.loadSuiteConfig,
    validateSuiteConfig: system.validateSuiteConfig,
    updateSuiteConfig: system.updateSuiteConfig,
    telemetryNetworkRates: fleet.telemetryNetworkRates,
    telemetryTunnels: fleet.telemetryTunnels,
    tags: inventory.tags,
    tagsError: inventory.tagsError,
    tagsLoading: inventory.tagsLoading,
    tagInventoryEvidenceAvailable:
      inventory.tagInventoryEvidenceAvailable,
    runtimeConfigApplyEvidenceAvailable:
      inventory.runtimeConfigApplyEvidenceAvailable,
    runtimeConfigApplyError: inventory.runtimeConfigApplyError,
    runtimeConfigApplyLoading: inventory.runtimeConfigApplyLoading,
    runtimeConfigApplyStates: inventory.runtimeConfigApplyStates,
    runtimeConfigPatchGenerators: inventory.runtimeConfigPatchGenerators,
    telemetryRollups: fleet.telemetryRollups,
    topologyError: topology.topologyError,
    topologyGraph: topology.topologyGraph,
    topologyLoading: topology.topologyLoading,
    tunnelPlanCorruptions: topology.tunnelPlanCorruptions,
    tunnelPlans: topology.tunnelPlans,
    portForwardRules: portForwarding.portForwardRules,
    portForwardError: portForwarding.portForwardError,
    portForwardLoading: portForwarding.portForwardLoading,
    updateConfigurationPreset: inventory.updateConfigurationPreset,
    updateOperator: access.updateOperator,
    upsertRuntimeConfigPatchGenerator: inventory.upsertRuntimeConfigPatchGenerator,
    upsertCommandTemplate: jobs.upsertCommandTemplate,
    upsertHistoryRetentionPolicy: audit.upsertHistoryRetentionPolicy,
    upsertFleetAlertPolicy: fleet.upsertFleetAlertPolicy,
    dryRunVpsRules: fleet.dryRunVpsRules,
    bulkUpsertVpsRules: fleet.bulkUpsertVpsRules,
    bulkUnsetVpsRules: fleet.bulkUnsetVpsRules,
    deleteFleetAlertPolicy: fleet.deleteFleetAlertPolicy,
    updateOperatorPreferences: access.updateOperatorPreferences,
    wsState,
  };
}

function dashboardRefreshIntervalMs(
  value: DashboardRefreshIntervalSecs,
): number {
  return value * 1000;
}

function activeViewUsesInventoryData(activeView: ActiveView): boolean {
  return (
    activeView === "Fleet" ||
    activeView === "Config" ||
    activeView === "Remote Operations" ||
    activeView === "Jobs" ||
    activeView === "Automation" ||
    activeView === "Observability" ||
    activeView === "Access" ||
    activeView === "System"
  );
}
