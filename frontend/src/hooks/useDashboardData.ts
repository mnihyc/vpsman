import { useCallback, useEffect, useRef, useState } from "react";
import { ACCESS_TOKEN_STORAGE_KEY, REFRESH_TOKEN_STORAGE_KEY } from "../constants";
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
import { useTopologyData } from "./useTopologyData";

export function useDashboardData(activeView: ActiveView) {
  const [apiToken, setApiToken] = useState("");
  const [authRequired, setAuthRequired] = useState(false);
  const [authVaultAvailable, setAuthVaultAvailable] = useState(() => hasAuthVault());
  const [wsState, setWsState] = useState("connecting");
  const [lastLiveEvent, setLastLiveEvent] = useState("waiting");
  const [lastJobOutputEvent, setLastJobOutputEvent] = useState<WsJobOutputEvent | null>(null);
  const [lastTerminalOutputEvent, setLastTerminalOutputEvent] = useState<WsTerminalOutputEvent | null>(null);
  const dashboardOverviewReloadTimer = useRef<number | null>(null);

  const requireAuth = useCallback(() => setAuthRequired(true), []);
  const access = useAccessData(apiToken, requireAuth);
  const dashboardOverview = useDashboardOverviewData(apiToken, requireAuth);
  const fleet = useFleetData(apiToken, requireAuth);
  const audit = useAuditData(apiToken, requireAuth);
  const inventory = useInventoryData(apiToken, requireAuth, fleet.loadFleet);
  const jobs = useJobsData(apiToken, requireAuth, fleet.loadFleet, audit.loadAudits);
  const schedules = useSchedulesData(apiToken, requireAuth, audit.loadAudits);
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
    let cancelled = false;

    async function loadIfActive() {
      if (!cancelled) {
        await fleet.loadFleet();
      }
    }

    loadIfActive();
    const timer = window.setInterval(loadIfActive, 15_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [fleet.loadFleet]);

  useEffect(() => {
    if ((authRequired && !apiToken) || activeView !== "Dashboard") {
      return;
    }
    let cancelled = false;
    let timer: number | null = null;

    async function loadAndSchedule() {
      await dashboardOverview.loadDashboardOverview();
      if (cancelled) {
        return;
      }
      timer = window.setTimeout(
        loadAndSchedule,
        dashboardRefreshIntervalMs(dashboardOverview.dashboardPreferences.refreshIntervalSecs),
      );
    }

    void loadAndSchedule();
    return () => {
      cancelled = true;
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
    if (activeView === "Fleet") {
      void inventory.loadTagInventory();
    } else if (activeView === "Config") {
      void inventory.loadTagInventory();
      void jobs.loadJobs();
    } else if (activeView === "Tags") {
      void inventory.loadTagInventory();
    } else if (activeView === "Jobs") {
      void jobs.loadJobs();
      void jobs.loadAgentUpdateRollouts();
      void inventory.loadTagInventory();
    } else if (activeView === "Schedules") {
      void schedules.loadSchedules();
      void jobs.loadJobs();
      void inventory.loadTagInventory();
    } else if (activeView === "Topology") {
      void topology.loadTunnelPlans();
      void topology.loadNetworkObservations();
      void topology.loadNetworkTrends();
      void topology.loadOspfRecommendations();
      void topology.loadOspfUpdatePlans();
      void topology.loadTopologyGraph();
      void jobs.loadJobs();
    } else if (activeView === "Backups") {
      void backups.loadBackups();
    } else if (activeView === "Audit") {
      void audit.loadAudits();
    } else if (activeView === "Access") {
      void access.loadCurrentOperator();
      void inventory.loadTagInventory();
    } else if (activeView === "Preferences") {
      void access.loadCurrentOperatorProfile();
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
    jobs.loadJobs,
    jobs.loadAgentUpdateRollouts,
    schedules.loadSchedules,
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
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    const socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
    socket.addEventListener("open", () => {
      if (apiToken) {
        socket.send(JSON.stringify({ type: "auth", access_token: apiToken }));
      }
      setWsState("connected");
    });
    socket.addEventListener("close", () => setWsState("closed"));
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
      if (event.type === "agent_updated" || event.type === "telemetry_updated" || event.type === "job_rejected") {
        void fleet.loadFleet();
      }
      if (
        activeView === "Dashboard" &&
        (event.type === "agent_updated" || event.type === "telemetry_updated" || event.type === "job_rejected")
      ) {
        scheduleDashboardOverviewReload();
      }
      if (event.type === "agent_updated" || event.type === "telemetry_updated") {
        void inventory.loadTagInventory();
      }
      if (event.type === "job_rejected") {
        void jobs.loadJobs();
        void audit.loadAudits();
      }
      if (event.type === "job_output_recorded") {
        setLastJobOutputEvent(event);
        if (activeView === "Jobs") {
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
        void jobs.loadAgentUpdateRollouts();
        void audit.loadAudits();
        if (activeView === "Dashboard") {
          scheduleDashboardOverviewReload();
        }
      }
      if (event.type === "backup_artifact_recorded") {
        void backups.loadBackups();
        void audit.loadAudits();
        if (activeView === "Dashboard") {
          scheduleDashboardOverviewReload();
        }
      }
    });
    return () => socket.close();
  }, [
    apiToken,
    authRequired,
    audit.loadAudits,
    fleet.loadFleet,
    fleet.replaceFleetSnapshot,
    backups.loadBackups,
    dashboardOverview.loadDashboardOverview,
    inventory.loadTagInventory,
    jobs.loadAgentUpdateRollouts,
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
  }, [access.clearOperator, dashboardOverview.clearDashboardOverview, fleet.clearFleet]);

  return {
    accessError: access.accessError,
    accessLoading: access.accessLoading,
    agents: fleet.agents,
    apiError: fleet.apiError,
    apiToken,
    assignDataSourcePreset: inventory.assignDataSourcePreset,
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
    cancelJob: jobs.cancelJob,
    clientKeyRevocations: access.clientKeyRevocations,
    cloneDataSourcePreset: inventory.cloneDataSourcePreset,
    confirmTotp: access.confirmTotp,
    createOperator: access.createOperator,
    updateAgentAlias: fleet.updateAgentAlias,
    createEnrollmentToken: access.createEnrollmentToken,
    createBackupRequest: backups.createBackupRequest,
    createBackupPolicy: backups.createBackupPolicy,
    createFileTransferHandoff: jobs.createFileTransferHandoff,
    createMigrationLink: backups.createMigrationLink,
    createRestorePlan: backups.createRestorePlan,
    downloadBackupArtifact: backups.downloadBackupArtifact,
    handoffBackupArtifact: backups.handoffBackupArtifact,
    prepareBackupArtifactRestore: backups.prepareBackupArtifactRestore,
    pruneBackupPolicies: backups.pruneBackupPolicies,
    uploadBackupArtifact: backups.uploadBackupArtifact,
    uploadBackupArtifactChunked: backups.uploadBackupArtifactChunked,
    createJob: jobs.createJob,
    createAgentUpdateRelease: jobs.createAgentUpdateRelease,
    createAgentUpdateRolloutPolicy: jobs.createAgentUpdateRolloutPolicy,
    updateAgentUpdateRolloutControl: jobs.updateAgentUpdateRolloutControl,
    uploadAgentUpdateArtifact: jobs.uploadAgentUpdateArtifact,
    createDataSourcePreset: inventory.createDataSourcePreset,
    createSchedule: schedules.createSchedule,
    updateSchedule: schedules.updateSchedule,
    enableSchedule: schedules.enableSchedule,
    disableSchedule: schedules.disableSchedule,
    deferSchedule: schedules.deferSchedule,
    applyScheduleNow: schedules.applyScheduleNow,
    deleteSchedule: schedules.deleteSchedule,
    createTag: inventory.createTag,
    createTunnelPlan: topology.createTunnelPlan,
    promoteTelemetryTunnel: topology.promoteTelemetryTunnel,
    promoteTunnelPlanToAdapter: topology.promoteTunnelPlanToAdapter,
    disableTotp: access.disableTotp,
    handleAuth,
    handleAuthVaultUnlock,
    jobs: jobs.jobs,
    commandTemplates: jobs.commandTemplates,
    agentUpdateReleases: jobs.agentUpdateReleases,
    agentUpdateRolloutPolicies: jobs.agentUpdateRolloutPolicies,
    agentUpdateRollouts: jobs.agentUpdateRollouts,
    jobsError: jobs.jobsError,
    jobsLoading: jobs.jobsLoading,
    keyLifecycleReport: access.keyLifecycleReport,
    processSupervisorInventory: jobs.processSupervisorInventory,
    fileTransfers: jobs.fileTransfers,
    fileTransferSources: jobs.fileTransferSources,
    terminalSessions: jobs.terminalSessions,
    gatewaySessions: access.gatewaySessions,
    deleteAgent: fleet.deleteAgent,
    enrollmentSettings: access.enrollmentSettings,
    enrollmentTokens: access.enrollmentTokens,
    fleetAlerts: fleet.fleetAlerts,
    fleetAlertStates: fleet.fleetAlertStates,
    fleetAlertPolicies: fleet.fleetAlertPolicies,
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
    downloadJobOutputArtifact: jobs.downloadJobOutputArtifact,
    saveFileTransferHandoff: jobs.saveFileTransferHandoff,
    loadJob: jobs.loadJob,
    loadJobOutputs: jobs.loadJobOutputs,
    loadJobOutputComparison: jobs.loadJobOutputComparison,
    loadJobs: jobs.loadJobs,
    loadTerminalReplay: jobs.loadTerminalReplay,
    loadAgentUpdateRollouts: jobs.loadAgentUpdateRollouts,
    loadJobTargets: jobs.loadJobTargets,
    updateFleetAlertState: fleet.updateFleetAlertState,
    upsertFleetAlertNotificationChannel: fleet.upsertFleetAlertNotificationChannel,
    dispatchFleetAlertNotifications: fleet.dispatchFleetAlertNotifications,
    processFleetAlertNotifications: fleet.processFleetAlertNotifications,
    upsertWebhookRule: fleet.upsertWebhookRule,
    dryRunWebhookRule: fleet.dryRunWebhookRule,
    dispatchWebhookRules: fleet.dispatchWebhookRules,
    processWebhookRuleDeliveries: fleet.processWebhookRuleDeliveries,
    rotateWebhookDeliveryHistory: fleet.rotateWebhookDeliveryHistory,
    uploadFileTransferSource: jobs.uploadFileTransferSource,
    loadTagInventory: inventory.loadTagInventory,
    loadSchedules: schedules.loadSchedules,
    loadNetworkObservations: topology.loadNetworkObservations,
    loadNetworkTrends: topology.loadNetworkTrends,
    loadOspfRecommendations: topology.loadOspfRecommendations,
    loadOspfUpdatePlans: topology.loadOspfUpdatePlans,
    loadTopologyGraph: topology.loadTopologyGraph,
    loadTunnelPlans: topology.loadTunnelPlans,
    networkObservations: topology.networkObservations,
    networkTrends: topology.networkTrends,
    ospfRecommendations: topology.ospfRecommendations,
    ospfUpdatePlans: topology.ospfUpdatePlans,
    operator: access.operator,
    operators: access.operators,
    operatorSessions: access.operatorSessions,
    preferencesError: access.preferencesError,
    preferencesSaving: access.preferencesSaving,
    dataSourceAssignments: inventory.dataSourceAssignments,
    dataSourcePresets: inventory.dataSourcePresets,
    dataSourceStatus: inventory.dataSourceStatus,
    deleteHotConfigRuleTemplate: inventory.deleteHotConfigRuleTemplate,
    deleteTag: inventory.deleteTag,
    dashboardOverview: dashboardOverview.dashboardOverview,
    dashboardOverviewError: dashboardOverview.dashboardOverviewError,
    dashboardOverviewLoading: dashboardOverview.dashboardOverviewLoading,
    dashboardOverviewWindow: dashboardOverview.dashboardOverviewWindow,
    dashboardPreferences: dashboardOverview.dashboardPreferences,
    loadDashboardOverview: dashboardOverview.loadDashboardOverview,
    setDashboardOverviewWindow: dashboardOverview.setDashboardOverviewWindow,
    updateDashboardPreferences: dashboardOverview.updateDashboardPreferences,
    diffDataSourcePreset: inventory.diffDataSourcePreset,
    renderDataSourceHotConfig: inventory.renderDataSourceHotConfig,
    renderHotConfigRuleTemplate: inventory.renderHotConfigRuleTemplate,
    resolveBulkPreview: inventory.resolveBulkPreview,
    resolveJobTargets: inventory.resolveJobTargets,
    revokeClientKey: access.revokeClientKey,
    revokeOperatorSession: access.revokeOperatorSession,
    pruneHistoryRetention: audit.pruneHistoryRetention,
    setupTotp: access.setupTotp,
    testDataSourcePreset: inventory.testDataSourcePreset,
    schedules: schedules.schedules,
    schedulesError: schedules.schedulesError,
    schedulesLoading: schedules.schedulesLoading,
    summary: fleet.summary,
    telemetryNetworkRates: fleet.telemetryNetworkRates,
    telemetryTunnels: fleet.telemetryTunnels,
    tags: inventory.tags,
    tagsError: inventory.tagsError,
    tagsLoading: inventory.tagsLoading,
    hotConfigRuleTemplates: inventory.hotConfigRuleTemplates,
    telemetryRollups: fleet.telemetryRollups,
    topologyError: topology.topologyError,
    topologyGraph: topology.topologyGraph,
    topologyLoading: topology.topologyLoading,
    tunnelPlans: topology.tunnelPlans,
    updateDataSourcePreset: inventory.updateDataSourcePreset,
    updateEnrollmentSettings: access.updateEnrollmentSettings,
    upsertHotConfigRuleTemplate: inventory.upsertHotConfigRuleTemplate,
    upsertCommandTemplate: jobs.upsertCommandTemplate,
    upsertHistoryRetentionPolicy: audit.upsertHistoryRetentionPolicy,
    upsertFleetAlertPolicy: fleet.upsertFleetAlertPolicy,
    updateOperatorPreferences: access.updateOperatorPreferences,
    wsState,
  };
}

function dashboardRefreshIntervalMs(value: DashboardRefreshIntervalSecs): number {
  return value * 1000;
}
