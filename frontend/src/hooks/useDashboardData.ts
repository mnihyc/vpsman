import { useCallback, useEffect, useState } from "react";
import { ACCESS_TOKEN_STORAGE_KEY, REFRESH_TOKEN_STORAGE_KEY } from "../constants";
import type { ActiveView, AuthResponse, WsJobOutputEvent, WsTerminalOutputEvent } from "../types";
import { parseWsEvent } from "../utils";
import { clearAuthVault, hasAuthVault, saveAuthVault } from "../vault";
import { useAccessData } from "./useAccessData";
import { useAuditData } from "./useAuditData";
import { useBackupsData } from "./useBackupsData";
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

  const requireAuth = useCallback(() => setAuthRequired(true), []);
  const fleet = useFleetData(apiToken, requireAuth);
  const access = useAccessData(apiToken, requireAuth);
  const audit = useAuditData(apiToken, requireAuth);
  const inventory = useInventoryData(apiToken, requireAuth, fleet.loadFleet);
  const jobs = useJobsData(apiToken, requireAuth, fleet.loadFleet, audit.loadAudits);
  const schedules = useSchedulesData(apiToken, requireAuth, audit.loadAudits);
  const topology = useTopologyData(apiToken, requireAuth, audit.loadAudits);
  const backups = useBackupsData(apiToken, requireAuth, audit.loadAudits);

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
    if (authRequired && !apiToken) {
      return;
    }
    if (activeView === "Pools") {
      void inventory.loadPoolsAndTags();
    } else if (activeView === "Jobs") {
      void jobs.loadJobs();
      void jobs.loadAgentUpdateRollouts();
      void inventory.loadPoolsAndTags();
    } else if (activeView === "Schedules") {
      void schedules.loadSchedules();
      void inventory.loadPoolsAndTags();
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
      void inventory.loadPoolsAndTags();
    }
  }, [
    access.loadCurrentOperator,
    activeView,
    apiToken,
    authRequired,
    audit.loadAudits,
    backups.loadBackups,
    inventory.loadPoolsAndTags,
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
    const tokenQuery = apiToken ? `?access_token=${encodeURIComponent(apiToken)}` : "";
    const socket = new WebSocket(`${protocol}//${window.location.host}/ws${tokenQuery}`);
    socket.addEventListener("open", () => setWsState("connected"));
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
      if (event.type === "agent_updated" || event.type === "telemetry_updated") {
        void inventory.loadPoolsAndTags();
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
      }
      if (event.type === "backup_artifact_recorded") {
        void backups.loadBackups();
        void audit.loadAudits();
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
    inventory.loadPoolsAndTags,
    jobs.loadAgentUpdateRollouts,
    jobs.loadJobs,
    jobs.loadTerminalSessions,
    activeView,
  ]);

  useEffect(() => {
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
  }, []);

  const handleAuth = useCallback(async (auth: AuthResponse, sessionVaultKey?: string) => {
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
    if (sessionVaultKey) {
      await saveAuthVault(auth, sessionVaultKey);
    } else {
      clearAuthVault();
    }
    setAuthVaultAvailable(hasAuthVault());
    setApiToken(auth.access_token);
    setAuthRequired(false);
  }, []);

  const handleAuthVaultUnlock = useCallback((auth: AuthResponse) => {
    setAuthVaultAvailable(hasAuthVault());
    setApiToken(auth.access_token);
    setAuthRequired(false);
  }, []);

  const clearSession = useCallback(() => {
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
    clearAuthVault();
    setAuthVaultAvailable(false);
    setApiToken("");
    setAuthRequired(true);
    access.clearOperator();
    fleet.clearFleet();
  }, [access.clearOperator, fleet.clearFleet]);

  return {
    accessError: access.accessError,
    accessLoading: access.accessLoading,
    agents: fleet.agents,
    apiError: fleet.apiError,
    apiToken,
    assignDataSourcePreset: inventory.assignDataSourcePreset,
    assignPool: inventory.assignPool,
    assignTag: inventory.assignTag,
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
    pruneBackupPolicies: backups.pruneBackupPolicies,
    uploadBackupArtifact: backups.uploadBackupArtifact,
    uploadBackupArtifactChunked: backups.uploadBackupArtifactChunked,
    createJob: jobs.createJob,
    createAgentUpdateRelease: jobs.createAgentUpdateRelease,
    createAgentUpdateRolloutPolicy: jobs.createAgentUpdateRolloutPolicy,
    delegateAgentUpdateRollback: jobs.delegateAgentUpdateRollback,
    updateAgentUpdateRolloutControl: jobs.updateAgentUpdateRolloutControl,
    uploadAgentUpdateArtifact: jobs.uploadAgentUpdateArtifact,
    createDataSourcePreset: inventory.createDataSourcePreset,
    createPool: inventory.createPool,
    createSchedule: schedules.createSchedule,
    createTag: inventory.createTag,
    createTunnelPlan: topology.createTunnelPlan,
    promoteTelemetryTunnel: topology.promoteTelemetryTunnel,
    promoteTunnelPlanToAdapter: topology.promoteTunnelPlanToAdapter,
    delegateAgentUpdateActivation: jobs.delegateAgentUpdateActivation,
    disableTotp: access.disableTotp,
    dispatchScheduledJob: jobs.dispatchScheduledJob,
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
    proofRotations: access.proofRotations,
    processSupervisorInventory: jobs.processSupervisorInventory,
    fileTransfers: jobs.fileTransfers,
    fileTransferSources: jobs.fileTransferSources,
    terminalSessions: jobs.terminalSessions,
    gatewaySessions: access.gatewaySessions,
    enrollmentTokens: access.enrollmentTokens,
    fleetAlerts: fleet.fleetAlerts,
    fleetAlertStates: fleet.fleetAlertStates,
    fleetAlertPolicies: fleet.fleetAlertPolicies,
    fleetAlertNotificationChannels: fleet.fleetAlertNotificationChannels,
    fleetAlertNotifications: fleet.fleetAlertNotifications,
    lastLiveEvent,
    lastJobOutputEvent,
    lastTerminalOutputEvent,
    loadAudits: audit.loadAudits,
    loadHistoryExport: audit.loadHistoryExport,
    loadBackups: backups.loadBackups,
    loadCurrentOperator: access.loadCurrentOperator,
    downloadFileTransferHandoff: jobs.downloadFileTransferHandoff,
    downloadFileTransferSource: jobs.downloadFileTransferSource,
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
    uploadFileTransferSource: jobs.uploadFileTransferSource,
    loadPoolsAndTags: inventory.loadPoolsAndTags,
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
    dataSourceAssignments: inventory.dataSourceAssignments,
    dataSourcePresets: inventory.dataSourcePresets,
    dataSourceStatus: inventory.dataSourceStatus,
    diffDataSourcePreset: inventory.diffDataSourcePreset,
    pools: inventory.pools,
    poolsError: inventory.poolsError,
    poolsLoading: inventory.poolsLoading,
    renderDataSourceHotConfig: inventory.renderDataSourceHotConfig,
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
    telemetryRollups: fleet.telemetryRollups,
    topologyError: topology.topologyError,
    topologyGraph: topology.topologyGraph,
    topologyLoading: topology.topologyLoading,
    tunnelPlans: topology.tunnelPlans,
    updateDataSourcePreset: inventory.updateDataSourcePreset,
    upsertCommandTemplate: jobs.upsertCommandTemplate,
    upsertHistoryRetentionPolicy: audit.upsertHistoryRetentionPolicy,
    upsertFleetAlertPolicy: fleet.upsertFleetAlertPolicy,
    wsState,
  };
}
