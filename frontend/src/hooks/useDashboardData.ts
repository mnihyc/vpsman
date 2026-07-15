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
  WsTerminalOutputEvent,
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
  const [lastTerminalOutputEvent, setLastTerminalOutputEvent] =
    useState<WsTerminalOutputEvent | null>(null);
  const dashboardOverviewReloadTimer = useRef<number | null>(null);
  const fleetReloadTimer = useRef<number | null>(null);
  const fleetTelemetryReloadTimer = useRef<number | null>(null);
  const fleetTelemetryReloadedAt = useRef(0);
  const inventoryReloadTimer = useRef<number | null>(null);
  const topologyReloadTimer = useRef<number | null>(null);
  const refreshAuthRef = useRef<Promise<void> | null>(null);
  const activeViewRef = useRef(activeView);

  useEffect(() => {
    activeViewRef.current = activeView;
  }, [activeView]);

  const forceAuthRequired = useCallback(() => {
    clearStoredAuthSession();
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
    refreshAuthRef.current = apiPost<AuthResponse>(
      "/api/v1/auth/refresh",
      "",
      { refresh_token: refreshToken },
    )
      .then((auth) => {
        persistAuthSession(auth);
        setApiToken(auth.access_token);
        setAuthRequired(false);
      })
      .catch((error) => {
        if (isApiUnauthorized(error)) {
          forceAuthRequired();
        }
      })
      .finally(() => {
        refreshAuthRef.current = null;
      });
    return refreshAuthRef.current;
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
      void topology.loadTunnelPlans();
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
      void topology.loadNetworkObservations();
      void topology.loadNetworkTrends();
      void topology.loadOspfRecommendations();
      void jobs.loadJobs();
      void backups.loadBackups();
    } else if (activeView === "Audit") {
      void audit.loadAudits();
      void jobs.loadJobs();
      void jobs.loadTerminalSessions();
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
    jobs.loadTerminalSessions,
    schedules.loadSchedules,
    system.loadSuiteConfig,
    system.loadSystemDashboard,
    topology.loadNetworkObservations,
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
        if (event.type === "terminal_output_recorded") {
          setLastTerminalOutputEvent(event);
          void jobs.loadTerminalSessions();
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
    jobs.loadTerminalSessions,
    scheduleDashboardOverviewReload,
    scheduleFleetReload,
    scheduleFleetTelemetryReload,
    scheduleInventoryReload,
    scheduleTopologyReload,
    portForwarding.loadPortForwardRules,
  ]);

  const handleAuth = useCallback(
    async (auth: AuthResponse) => {
      persistAuthSession(auth);
      access.setAuthenticatedOperator(auth.operator);
      setApiToken(auth.access_token);
      setAuthRequired(false);
    },
    [access.setAuthenticatedOperator],
  );

  const clearSession = useCallback(() => {
    clearStoredAuthSession();
    setApiToken("");
    setAuthRequired(true);
    access.clearOperator();
    fleet.clearFleet();
    dashboardOverview.clearDashboardOverview();
  }, [
    access.clearOperator,
    dashboardOverview.clearDashboardOverview,
    fleet.clearFleet,
  ]);

  return {
    accessError: access.accessError,
    accessLoading: access.accessLoading,
    agents: fleet.agents,
    apiError: fleet.apiError,
    apiToken,
    assignSourceTemplate: inventory.assignSourceTemplate,
    assignTag: inventory.assignTag,
    bulkMutateTags: inventory.bulkMutateTags,
    auditError: audit.auditError,
    auditLoading: audit.auditLoading,
    audits: audit.audits,
    historyExport: audit.historyExport,
    historyPruneResult: audit.historyPruneResult,
    historyRetentionPolicies: audit.historyRetentionPolicies,
    authRequired,
    backupArtifacts: backups.backupArtifacts,
    backupPolicies: backups.backupPolicies,
    backups: backups.backups,
    migrationLinks: backups.migrationLinks,
    restorePlans: backups.restorePlans,
    backupsError: backups.backupsError,
    backupsLoading: backups.backupsLoading,
    clearSession,
    clientKeyRevocations: access.clientKeyRevocations,
    clearOperatorTotp: access.clearOperatorTotp,
    cloneSourceTemplate: inventory.cloneSourceTemplate,
    confirmTotp: access.confirmTotp,
    createOperator: access.createOperator,
    updateAgentAlias: fleet.updateAgentAlias,
    upsertAgentIdentity: access.upsertAgentIdentity,
    createBackupRequest: backups.createBackupRequest,
    createBackupPolicy: backups.createBackupPolicy,
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
    createArtifactCleanupJob: jobs.createArtifactCleanupJob,
    createAgentUpdateRelease: jobs.createAgentUpdateRelease,
    createSourceTemplate: inventory.createSourceTemplate,
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
    createPortForwardRule: portForwarding.createPortForwardRule,
    updatePortForwardRule: portForwarding.updatePortForwardRule,
    mutatePortForwardRule: portForwarding.mutatePortForwardRule,
    bulkMutatePortForwardRules: portForwarding.bulkMutatePortForwardRules,
    resolvePortForwardHostname: portForwarding.resolvePortForwardHostname,
    deleteTunnelPlan: topology.deleteTunnelPlan,
    exportTunnelPlan: topology.exportTunnelPlan,
    refreshTunnelPlanOspfStatus: topology.refreshTunnelPlanOspfStatus,
    disableTotp: access.disableTotp,
    handleAuth,
    jobs: jobs.jobs,
    jobApprovals: jobs.jobApprovals,
    commandTemplates: jobs.commandTemplates,
    deleteCommandTemplate: jobs.deleteCommandTemplate,
    agentUpdateReleases: jobs.agentUpdateReleases,
    jobsError: jobs.jobsError,
    jobsLoading: jobs.jobsLoading,
    keyLifecycleReport: access.keyLifecycleReport,
    processSupervisorInventory: jobs.processSupervisorInventory,
    serverJobs: jobs.serverJobs,
    fileTransfers: jobs.fileTransfers,
    fileTransferSources: jobs.fileTransferSources,
    terminalSessions: jobs.terminalSessions,
    gatewaySessions: access.gatewaySessions,
    deleteAgent: fleet.deleteAgent,
    fleetAlerts: fleet.fleetAlerts,
    fleetAlertStates: fleet.fleetAlertStates,
    fleetAlertPolicies: fleet.fleetAlertPolicies,
    vpsRuleValues: fleet.vpsRuleValues,
    trafficAccounting: fleet.trafficAccounting,
    policyAlerts: fleet.policyAlerts,
    fleetAlertNotificationChannels: fleet.fleetAlertNotificationChannels,
    fleetAlertNotifications: fleet.fleetAlertNotifications,
    webhookRules: fleet.webhookRules,
    webhookRuleDeliveries: fleet.webhookRuleDeliveries,
    lastLiveEvent,
    lastJobOutputEvent,
    lastTerminalOutputEvent,
    loadAudits: audit.loadAudits,
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
    loadJobOutputs: jobs.loadJobOutputs,
    loadJobOutputComparison: jobs.loadJobOutputComparison,
    loadJobs: jobs.loadJobs,
    loadServerJobs: jobs.loadServerJobs,
    loadTerminalSessions: jobs.loadTerminalSessions,
    loadTerminalReplay: jobs.loadTerminalReplay,
    submitTerminalInput: jobs.submitTerminalInput,
    loadAgentUpdateReleases: jobs.loadAgentUpdateReleases,
    loadJobTargets: jobs.loadJobTargets,
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
    previewArtifactCleanup: jobs.previewArtifactCleanup,
    loadTagInventory: inventory.loadTagInventory,
    loadSourceTemplates: inventory.loadSourceTemplates,
    loadSchedules: schedules.loadSchedules,
    loadNetworkObservations: topology.loadNetworkObservations,
    loadNetworkTrends: topology.loadNetworkTrends,
    loadOspfRecommendations: topology.loadOspfRecommendations,
    loadOspfUpdatePlans: topology.loadOspfUpdatePlans,
    loadTopologyGraph: topology.loadTopologyGraph,
    loadTunnelPlans: topology.loadTunnelPlans,
    loadPortForwardRules: portForwarding.loadPortForwardRules,
    setTunnelPlanEnabled: topology.setTunnelPlanEnabled,
    updateTunnelConnectionAssessment: topology.updateTunnelConnectionAssessment,
    updateTunnelPlanOspfCost: topology.updateTunnelPlanOspfCost,
    updateTunnelPlan: topology.updateTunnelPlan,
    networkObservations: topology.networkObservations,
    networkTrends: topology.networkTrends,
    ospfRecommendations: topology.ospfRecommendations,
    ospfUpdatePlans: topology.ospfUpdatePlans,
    operator: access.operator,
    operatorAuthEvents: access.operatorAuthEvents,
    operators: access.operators,
    operatorSessions: access.operatorSessions,
    preferencesError: access.preferencesError,
    preferencesSaving: access.preferencesSaving,
    sourceTemplateAssignments: inventory.sourceTemplateAssignments,
    sourceTemplates: inventory.sourceTemplates,
    sourceStatus: inventory.sourceStatus,
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
    diffSourceTemplate: inventory.diffSourceTemplate,
    submitRuntimeConfigPatch: inventory.submitRuntimeConfigPatch,
    renderTemplateRuntimeConfig: inventory.renderTemplateRuntimeConfig,
    renderRuntimeConfigPatchGenerator: inventory.renderRuntimeConfigPatchGenerator,
    resolveBulkPreview: inventory.resolveBulkPreview,
    resolveJobTargets: inventory.resolveJobTargets,
    revokeClientKey: access.revokeClientKey,
    revokeOperatorSession: access.revokeOperatorSession,
    resetOperatorPassword: access.resetOperatorPassword,
    pruneHistoryRetention: audit.pruneHistoryRetention,
    setupTotp: access.setupTotp,
    testSourceTemplate: inventory.testSourceTemplate,
    schedules: schedules.schedules,
    schedulesError: schedules.schedulesError,
    schedulesLoading: schedules.schedulesLoading,
    summary: fleet.summary,
    systemDashboard: system.systemDashboard,
    systemDashboardError: system.systemDashboardError,
    systemDashboardLoading: system.systemDashboardLoading,
    systemDashboardPointDensity: system.systemDashboardPointDensity,
    systemDashboardWindow: system.systemDashboardWindow,
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
    runtimeConfigApplyStates: inventory.runtimeConfigApplyStates,
    runtimeConfigPatchGenerators: inventory.runtimeConfigPatchGenerators,
    telemetryRollups: fleet.telemetryRollups,
    topologyError: topology.topologyError,
    topologyGraph: topology.topologyGraph,
    topologyLoading: topology.topologyLoading,
    tunnelPlans: topology.tunnelPlans,
    portForwardRules: portForwarding.portForwardRules,
    portForwardError: portForwarding.portForwardError,
    portForwardLoading: portForwarding.portForwardLoading,
    updateSourceTemplate: inventory.updateSourceTemplate,
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
