import { useCallback, useEffect, useRef, useState } from "react";
import {
  ACCESS_TOKEN_STORAGE_KEY,
  REFRESH_TOKEN_STORAGE_KEY,
} from "../constants";
import type {
  ActiveView,
  AuthResponse,
  DashboardRefreshIntervalSecs,
  WsJobOutputEvent,
  WsTerminalOutputEvent,
} from "../types";
import { parseWsEvent } from "../utils";
import { clearAuthVault, hasAuthVault, saveAuthVault } from "../vault";
import { useAccessData } from "./useAccessData";
import { useAuditData } from "./useAuditData";
import { useBackupsData } from "./useBackupsData";
import { useDashboardOverviewData } from "./useDashboardOverviewData";
import { useFleetData } from "./useFleetData";
import { useInventoryData } from "./useInventoryData";
import { useJobsData } from "./useJobsData";
import { useSchedulesData } from "./useSchedulesData";
import { useSystemData } from "./useSystemData";
import { useTopologyData } from "./useTopologyData";

export function useDashboardData(activeView: ActiveView) {
  const [apiToken, setApiToken] = useState("");
  const [authRequired, setAuthRequired] = useState(false);
  const [authVaultAvailable, setAuthVaultAvailable] = useState(() =>
    hasAuthVault(),
  );
  const [wsState, setWsState] = useState("connecting");
  const [lastLiveEvent, setLastLiveEvent] = useState("waiting");
  const [lastJobOutputEvent, setLastJobOutputEvent] =
    useState<WsJobOutputEvent | null>(null);
  const [lastTerminalOutputEvent, setLastTerminalOutputEvent] =
    useState<WsTerminalOutputEvent | null>(null);
  const dashboardOverviewReloadTimer = useRef<number | null>(null);

  const requireAuth = useCallback(() => {
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
    setAuthVaultAvailable(hasAuthVault());
    setApiToken("");
    setAuthRequired(true);
  }, []);
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
  const topology = useTopologyData(apiToken, requireAuth, audit.loadAudits);
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

  useEffect(
    () => () => {
      if (dashboardOverviewReloadTimer.current !== null) {
        window.clearTimeout(dashboardOverviewReloadTimer.current);
      }
    },
    [],
  );

  useEffect(() => {
    let disposed = false;

    async function loadIfActive() {
      if (!disposed) {
        await fleet.loadFleet();
      }
    }

    loadIfActive();
    const timer = window.setInterval(loadIfActive, 15_000);
    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [fleet.loadFleet]);

  useEffect(() => {
    if (
      (authRequired && !apiToken) ||
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
    authRequired,
    dashboardOverview.dashboardPreferences.refreshIntervalSecs,
    dashboardOverview.loadDashboardOverview,
  ]);

  useEffect(() => {
    if (authRequired && !apiToken) {
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
      void system.loadSuiteConfig();
    } else if (activeView === "Automation") {
      void schedules.loadSchedules();
      void jobs.loadJobs();
      void inventory.loadTagInventory();
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
      void system.loadSuiteConfig();
    }
  }, [
    access.loadCurrentOperator,
    access.loadCurrentOperatorProfile,
    activeView,
    apiToken,
    authRequired,
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
  ]);

  useEffect(() => {
    if (authRequired && !apiToken) {
      setWsState("auth required");
      return;
    }
    let disposed = false;
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
    socket.addEventListener("open", () => {
      if (apiToken) {
        socket.send(JSON.stringify({ type: "auth", access_token: apiToken }));
      }
      setWsState("connected");
    });
    socket.addEventListener("close", () => {
      setWsState("closed");
      if (!disposed && apiToken) {
        void access.loadCurrentOperator();
      }
    });
    socket.addEventListener("error", () => setWsState("error"));
    socket.addEventListener("message", (message) => {
      const event = parseWsEvent(message.data);
      if (!event) {
        return;
      }
      setLastLiveEvent(event.type);
      if (event.type === "fleet_snapshot") {
        fleet.replaceFleetSnapshot(event.summary, event.agents);
        return;
      }
      if (
        event.type === "agent_updated" ||
        event.type === "telemetry_updated" ||
        event.type === "job_rejected"
      ) {
        void fleet.loadFleet();
      }
      if (
        activeView === "Home" &&
        (event.type === "agent_updated" ||
          event.type === "telemetry_updated" ||
          event.type === "job_rejected")
      ) {
        scheduleDashboardOverviewReload();
      }
      if (
        event.type === "agent_updated" ||
        event.type === "telemetry_updated"
      ) {
        void inventory.loadTagInventory();
      }
      if (event.type === "job_rejected") {
        void jobs.loadJobs();
        void audit.loadAudits();
      }
      if (event.type === "job_output_recorded") {
        setLastJobOutputEvent(event);
        if (activeView === "Jobs" || activeView === "Remote Operations") {
          void jobs.loadJobs();
        }
      }
      if (event.type === "terminal_output_recorded") {
        setLastTerminalOutputEvent(event);
        void jobs.loadTerminalSessions();
      }
      if (event.type === "job_finished") {
        void fleet.loadFleet();
        void jobs.loadJobs();
        void audit.loadAudits();
        if (activeView === "Home" || activeView === "Observability") {
          scheduleDashboardOverviewReload();
        }
      }
      if (event.type === "backup_artifact_recorded") {
        void backups.loadBackups();
        void audit.loadAudits();
        if (activeView === "Home" || activeView === "Observability") {
          scheduleDashboardOverviewReload();
        }
      }
    });
    return () => {
      disposed = true;
      socket.close();
    };
  }, [
    apiToken,
    authRequired,
    access.loadCurrentOperator,
    audit.loadAudits,
    fleet.loadFleet,
    fleet.replaceFleetSnapshot,
    backups.loadBackups,
    dashboardOverview.loadDashboardOverview,
    inventory.loadTagInventory,
    jobs.loadJobs,
    jobs.loadTerminalSessions,
    scheduleDashboardOverviewReload,
    activeView,
  ]);

  useEffect(() => {
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
  }, []);

  const handleAuth = useCallback(
    async (auth: AuthResponse, sessionVaultKey?: string) => {
      window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
      window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
      if (sessionVaultKey) {
        await saveAuthVault(auth, sessionVaultKey);
      } else {
        clearAuthVault();
      }
      access.setAuthenticatedOperator(auth.operator);
      setAuthVaultAvailable(hasAuthVault());
      setApiToken(auth.access_token);
      setAuthRequired(false);
    },
    [access.setAuthenticatedOperator],
  );

  const handleAuthVaultUnlock = useCallback(
    (auth: AuthResponse) => {
      access.setAuthenticatedOperator(auth.operator);
      setAuthVaultAvailable(hasAuthVault());
      setApiToken(auth.access_token);
      setAuthRequired(false);
    },
    [access.setAuthenticatedOperator],
  );

  const clearSession = useCallback(() => {
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
    clearAuthVault();
    setAuthVaultAvailable(false);
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
    authVaultAvailable,
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
    exportTunnelPlan: topology.exportTunnelPlan,
    promoteTelemetryTunnel: topology.promoteTelemetryTunnel,
    promoteTunnelPlanToCustomAdapter: topology.promoteTunnelPlanToCustomAdapter,
    disableTotp: access.disableTotp,
    handleAuth,
    handleAuthVaultUnlock,
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
    loadSchedules: schedules.loadSchedules,
    loadNetworkObservations: topology.loadNetworkObservations,
    loadNetworkTrends: topology.loadNetworkTrends,
    loadOspfRecommendations: topology.loadOspfRecommendations,
    loadOspfUpdatePlans: topology.loadOspfUpdatePlans,
    loadTopologyGraph: topology.loadTopologyGraph,
    loadTunnelPlans: topology.loadTunnelPlans,
    setTunnelPlanEnabled: topology.setTunnelPlanEnabled,
    updateTunnelPlanOspfCost: topology.updateTunnelPlanOspfCost,
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
