import { useEffect, useMemo, useState } from "react";
import { ConsoleShell } from "./components/ConsoleShell";
import { AuthPanel } from "./panels/AuthPanel";
import { DashboardPanel } from "./panels/DashboardPanel";
import { FleetWorkspace } from "./panels/FleetWorkspace";
import { ConfigPanel } from "./panels/ConfigPanel";
import { JobHistoryPanel } from "./panels/JobHistoryPanel";
import { TagsPanel } from "./panels/TagsPanel";
import { SchedulesPanel } from "./panels/SchedulesPanel";
import { AccessPanel } from "./panels/AccessPanel";
import { AuditLogPanel } from "./panels/AuditLogPanel";
import { BackupsPanel } from "./panels/BackupsPanel";
import { TopologyPanel } from "./panels/TopologyPanel";
import { SystemPanel } from "./panels/SystemPanel";
import { PanelDisplayProvider } from "./panelDisplay";
import type { ActiveView } from "./types";
import type { PrivilegeMaterial } from "./privilege";
import { defaultSubpages, normalizeSubpage } from "./constants";
import {
  DEFAULT_OPERATOR_PREFERENCES,
  getHeroCopy,
  getHeroTitle,
  setPreferredTimeZone,
  type VpsNameDisplayMode,
} from "./utils";
import { useDashboardData } from "./hooks/useDashboardData";
import { useFleetViews } from "./hooks/useFleetViews";

function getScopedHeroTitle(view: ActiveView, subpage: string): string {
  if (view === "System") {
    switch (subpage) {
      case "config":
        return "System config";
      case "operator":
        return "System preferences";
      default:
        return "System dashboard";
    }
  }
  if (view !== "Jobs") {
    return getHeroTitle(view);
  }
  switch (subpage) {
    case "dispatch":
      return "Command dispatch";
    case "files":
      return "VPS file browser";
    case "multi_files":
      return "Multi-file actions";
    case "updates":
      return "Agent updates";
    case "transfers":
      return "File transfer history";
    case "terminal":
      return "Terminal sessions";
    case "processes":
      return "Process supervisor";
    case "server_jobs":
      return "Server jobs";
    case "approvals":
      return "Schedule runs";
    default:
      return "Job history";
  }
}

export function App() {
  const [activeView, setActiveView] = useState<ActiveView>("Dashboard");
  const [activeSubpages, setActiveSubpages] = useState<
    Record<ActiveView, string>
  >({ ...defaultSubpages });
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const [pendingJobDetailId, setPendingJobDetailId] = useState<string | null>(
    null,
  );
  const [privilegeMaterial, setPrivilegeMaterial] =
    useState<PrivilegeMaterial | null>(null);
  const dashboard = useDashboardData(activeView);
  const fleetViews = useFleetViews(dashboard.agents);
  const operatorPreferences = {
    ...DEFAULT_OPERATOR_PREFERENCES,
    ...(dashboard.operator?.preferences ?? {}),
  };
  const visibleAgents = fleetViews.filteredAgents;
  const selectedAgent = useMemo(
    () =>
      visibleAgents.find((agent) => agent.id === selectedAgentId) ??
      visibleAgents[0] ??
      null,
    [selectedAgentId, visibleAgents],
  );
  const visibleSummary = useMemo(
    () => ({
      online: visibleAgents.filter((agent) => agent.status === "online").length,
      total: visibleAgents.length,
    }),
    [visibleAgents],
  );
  const onlineRatio = useMemo(() => {
    if (dashboard.summary.total === 0) {
      return "0%";
    }
    return `${Math.round((dashboard.summary.online / dashboard.summary.total) * 100)}%`;
  }, [dashboard.summary.online, dashboard.summary.total]);
  const activeSubpage = normalizeSubpage(
    activeView,
    activeSubpages[activeView],
  );
  const heroTitle = getScopedHeroTitle(activeView, activeSubpage);
  const hasFleetScope =
    fleetViews.fleetQuery.trim().length > 0 ||
    fleetViews.activeSavedViewId !== null;
  const heroCopy =
    activeView === "Fleet" && hasFleetScope
      ? `${visibleSummary.online} visible online / ${visibleSummary.total} visible / ${dashboard.summary.total} total`
      : activeView === "Fleet"
        ? `${dashboard.summary.online} online / ${dashboard.summary.total} total`
        : getHeroCopy(activeView);

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

  function navigateDashboardTarget(target: {
    query: string | null;
    subpage: string;
    view: string;
  }) {
    if (!isActiveView(target.view)) {
      return;
    }
    if (target.view === "Fleet" && target.query) {
      fleetViews.setFleetQuery(target.query);
    }
    selectView(target.view, target.subpage);
  }

  function openJobDetails(jobId: string) {
    setPendingJobDetailId(jobId);
    selectView("Jobs", "history");
  }

  function openPrivilegeUnlock() {
    selectView("Access", "privilege");
  }

  function lockPrivilege() {
    setPrivilegeMaterial(null);
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
        onlineRatio={onlineRatio}
        draftSavedFleetViewName={fleetViews.draftSavedViewName}
        filteredAgentCount={visibleAgents.length}
        fleetQuery={fleetViews.fleetQuery}
        heroCopy={heroCopy}
        heroTitle={heroTitle}
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
        summary={dashboard.summary}
      >
        {dashboard.authRequired && !dashboard.apiToken ? (
          <AuthPanel
            apiError={dashboard.apiError}
            onAuth={dashboard.handleAuth}
            onSessionUnlock={dashboard.handleAuthVaultUnlock}
            sessionVaultAvailable={dashboard.authVaultAvailable}
          />
        ) : (
          <>
            {activeView === "Dashboard" && (
              <DashboardPanel
                error={dashboard.dashboardOverviewError}
                loading={dashboard.dashboardOverviewLoading}
                onNavigate={navigateDashboardTarget}
                onRefresh={() => void dashboard.loadDashboardOverview()}
                onPreferencesChange={dashboard.updateDashboardPreferences}
                onWindowChange={dashboard.setDashboardOverviewWindow}
                overview={dashboard.dashboardOverview}
                preferences={dashboard.dashboardPreferences}
                window={dashboard.dashboardOverviewWindow}
              />
            )}
            {activeView === "Fleet" && (
              <FleetWorkspace
                activeSubpage={activeSubpage}
                agents={visibleAgents}
                apiError={dashboard.apiError}
                dataSourceAssignments={dashboard.dataSourceAssignments}
                dataSourceStatus={dashboard.dataSourceStatus}
                fleetAlerts={dashboard.fleetAlerts}
                fleetAlertStates={dashboard.fleetAlertStates}
                fleetAlertPolicies={dashboard.fleetAlertPolicies}
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
                onNavigatePanel={selectView}
                onOpenJobDetails={openJobDetails}
                onOpenPrivilegeUnlock={openPrivilegeUnlock}
                onRenderDataSourceHotConfig={
                  dashboard.renderDataSourceHotConfig
                }
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
                onProcessFleetAlertNotifications={
                  dashboard.processFleetAlertNotifications
                }
                onProcessWebhookRuleDeliveries={
                  dashboard.processWebhookRuleDeliveries
                }
                onRotateWebhookDeliveryHistory={
                  dashboard.rotateWebhookDeliveryHistory
                }
                onUpdateFleetAlertState={dashboard.updateFleetAlertState}
                onUpsertFleetAlertNotificationChannel={
                  dashboard.upsertFleetAlertNotificationChannel
                }
                onUpsertFleetAlertPolicy={dashboard.upsertFleetAlertPolicy}
                onUpsertWebhookRule={dashboard.upsertWebhookRule}
                selectedAgent={selectedAgent}
                summary={dashboard.summary}
                tags={dashboard.tags}
                telemetryNetworkRates={dashboard.telemetryNetworkRates}
                telemetryRollups={dashboard.telemetryRollups}
                telemetryTunnels={dashboard.telemetryTunnels}
                wsState={dashboard.wsState}
              />
            )}
            {activeView === "Config" && (
              <ConfigPanel
                activeSubpage={activeSubpage}
                agents={visibleAgents}
                dataSourceAssignments={dashboard.dataSourceAssignments}
                dataSourcePresets={dashboard.dataSourcePresets}
                dataSourceStatus={dashboard.dataSourceStatus}
                error={dashboard.tagsError}
                hotConfigRuleTemplates={dashboard.hotConfigRuleTemplates}
                jobs={dashboard.jobs}
                loading={dashboard.tagsLoading}
                onAssignDataSourcePreset={dashboard.assignDataSourcePreset}
                onCloneDataSourcePreset={dashboard.cloneDataSourcePreset}
                onCreateJob={dashboard.createJob}
                onCreateDataSourcePreset={dashboard.createDataSourcePreset}
                onDiffDataSourcePreset={dashboard.diffDataSourcePreset}
                onLoadJobOutputs={dashboard.loadJobOutputs}
                onLoadJobTargets={dashboard.loadJobTargets}
                onDeleteHotConfigRuleTemplate={
                  dashboard.deleteHotConfigRuleTemplate
                }
                onOpenJobDetails={openJobDetails}
                onOpenPrivilegeUnlock={openPrivilegeUnlock}
                onRefresh={dashboard.loadTagInventory}
                onRenderDataSourceHotConfig={
                  dashboard.renderDataSourceHotConfig
                }
                onRenderHotConfigRuleTemplate={
                  dashboard.renderHotConfigRuleTemplate
                }
                onResolveBulk={dashboard.resolveBulkPreview}
                onTestDataSourcePreset={dashboard.testDataSourcePreset}
                onUpdateDataSourcePreset={dashboard.updateDataSourcePreset}
                onUpsertHotConfigRuleTemplate={
                  dashboard.upsertHotConfigRuleTemplate
                }
                privilegeMaterial={privilegeMaterial}
                setPrivilegeMaterial={setPrivilegeMaterial}
              />
            )}
            {activeView === "Tags" && (
              <TagsPanel
                activeSubpage={activeSubpage}
                agents={visibleAgents}
                error={dashboard.tagsError}
                loading={dashboard.tagsLoading}
                onAssignTag={dashboard.assignTag}
                onCreateTag={dashboard.createTag}
                onBulkMutateTags={dashboard.bulkMutateTags}
                onDeleteTag={dashboard.deleteTag}
                onOpenPrivilegeUnlock={openPrivilegeUnlock}
                onOpenSchedules={() => selectView("Schedules")}
                onRefresh={dashboard.loadTagInventory}
                onResolveBulk={dashboard.resolveBulkPreview}
                privilegeMaterial={privilegeMaterial}
                tags={dashboard.tags}
              />
            )}
            {activeView === "Jobs" && (
              <JobHistoryPanel
                activeSubpage={activeSubpage}
                agents={visibleAgents}
                error={dashboard.jobsError}
                agentUpdateReleases={dashboard.agentUpdateReleases}
                jobs={dashboard.jobs}
                commandTemplates={dashboard.commandTemplates}
                fileTransfers={dashboard.fileTransfers}
                fileTransferSources={dashboard.fileTransferSources}
                lastJobOutputEvent={dashboard.lastJobOutputEvent}
                lastTerminalOutputEvent={dashboard.lastTerminalOutputEvent}
                loading={dashboard.jobsLoading}
                onCreateFileTransferHandoff={
                  dashboard.createFileTransferHandoff
                }
                onCreateJob={dashboard.createJob}
                onCreateArtifactCleanupJob={dashboard.createArtifactCleanupJob}
                onCreateAgentUpdateRelease={dashboard.createAgentUpdateRelease}
                onUploadAgentUpdateArtifact={
                  dashboard.uploadAgentUpdateArtifact
                }
                onDownloadFileBundle={dashboard.downloadFileDownloadBundle}
                onDownloadOutputArtifact={dashboard.downloadJobOutputArtifact}
                onDownloadFileTransferSource={
                  dashboard.downloadFileTransferSource
                }
                onSaveFileTransferHandoff={dashboard.saveFileTransferHandoff}
                onLoadJob={dashboard.loadJob}
                onLoadOutputs={dashboard.loadJobOutputs}
                onLoadOutputComparison={dashboard.loadJobOutputComparison}
                onLoadTargets={dashboard.loadJobTargets}
                onLoadTerminalReplay={dashboard.loadTerminalReplay}
                onCancelServerJob={dashboard.cancelServerJob}
                onSelectedJobDetailsOpened={() => setPendingJobDetailId(null)}
                onPreviewArtifactCleanup={dashboard.previewArtifactCleanup}
                onRefresh={dashboard.loadJobs}
                onResolveTargets={dashboard.resolveJobTargets}
                onSelectSubpage={selectSubpage}
                onUploadFileTransferSource={dashboard.uploadFileTransferSource}
                onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
                pendingSelectedJobId={pendingJobDetailId}
                privilegeMaterial={privilegeMaterial}
                processSupervisorInventory={
                  dashboard.processSupervisorInventory
                }
                serverJobs={dashboard.serverJobs}
                setPrivilegeMaterial={setPrivilegeMaterial}
                onOpenPrivilegeUnlock={openPrivilegeUnlock}
                terminalSessions={dashboard.terminalSessions}
              />
            )}
            {activeView === "Schedules" && (
              <SchedulesPanel
                activeSubpage={activeSubpage}
                agents={visibleAgents}
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
                onRefresh={dashboard.loadSchedules}
                onUpdateSchedule={dashboard.updateSchedule}
                onUpdateScheduleTargets={dashboard.updateScheduleTargets}
                privilegeMaterial={privilegeMaterial}
                schedules={dashboard.schedules}
              />
            )}
            {activeView === "Topology" && (
              <TopologyPanel
                activeSubpage={activeSubpage}
                agents={visibleAgents}
                error={dashboard.topologyError}
                jobs={dashboard.jobs}
                loading={dashboard.topologyLoading}
                networkObservations={dashboard.networkObservations}
                networkTrends={dashboard.networkTrends}
                ospfRecommendations={dashboard.ospfRecommendations}
                ospfUpdatePlans={dashboard.ospfUpdatePlans}
                onCreateJob={dashboard.createJob}
                onCreateTunnelPlan={dashboard.createTunnelPlan}
                onLoadNetworkObservations={dashboard.loadNetworkObservations}
                onLoadNetworkTrends={dashboard.loadNetworkTrends}
                onLoadOspfRecommendations={dashboard.loadOspfRecommendations}
                onLoadOspfUpdatePlans={dashboard.loadOspfUpdatePlans}
                onLoadTopologyGraph={dashboard.loadTopologyGraph}
                onLoadOutputs={dashboard.loadJobOutputs}
                onLoadTargets={dashboard.loadJobTargets}
                onOpenJobDetails={openJobDetails}
                onOpenPrivilegeUnlock={openPrivilegeUnlock}
                onPromoteTelemetryTunnel={dashboard.promoteTelemetryTunnel}
                onPromoteTunnelPlanToAdapter={
                  dashboard.promoteTunnelPlanToAdapter
                }
                onRefresh={dashboard.loadTunnelPlans}
                privilegeMaterial={privilegeMaterial}
                setPrivilegeMaterial={setPrivilegeMaterial}
                topologyGraph={dashboard.topologyGraph}
                telemetryTunnels={dashboard.telemetryTunnels}
                tunnelPlans={dashboard.tunnelPlans}
              />
            )}
            {activeView === "Audit" && (
              <AuditLogPanel
                activeSubpage={activeSubpage}
                audits={dashboard.audits}
                error={dashboard.auditError}
                historyExport={dashboard.historyExport}
                historyPruneResult={dashboard.historyPruneResult}
                historyRetentionPolicies={dashboard.historyRetentionPolicies}
                loading={dashboard.auditLoading}
                onExportHistory={dashboard.loadHistoryExport}
                onPruneHistoryRetention={dashboard.pruneHistoryRetention}
                onRefresh={dashboard.loadAudits}
                onUpsertHistoryRetentionPolicy={
                  dashboard.upsertHistoryRetentionPolicy
                }
              />
            )}
            {activeView === "Backups" && (
              <BackupsPanel
                activeSubpage={activeSubpage}
                agents={visibleAgents}
                artifacts={dashboard.backupArtifacts}
                backupPolicies={dashboard.backupPolicies}
                backups={dashboard.backups}
                migrationLinks={dashboard.migrationLinks}
                restorePlans={dashboard.restorePlans}
                error={dashboard.backupsError}
                loading={dashboard.backupsLoading}
                onCreateBackupRequest={dashboard.createBackupRequest}
                onCreateBackupPolicy={dashboard.createBackupPolicy}
                onCreateJob={dashboard.createJob}
                onCreateMigrationLink={dashboard.createMigrationLink}
                onCreateRestorePlan={dashboard.createRestorePlan}
                onDownloadBackupArtifact={dashboard.downloadBackupArtifact}
                onHandoffBackupArtifact={dashboard.handoffBackupArtifact}
                onLoadJobOutputs={dashboard.loadJobOutputs}
                onPrepareBackupArtifactRestore={
                  dashboard.prepareBackupArtifactRestore
                }
                onPruneBackupPolicies={dashboard.pruneBackupPolicies}
                onOpenPrivilegeUnlock={openPrivilegeUnlock}
                onRefresh={dashboard.loadBackups}
                privilegeMaterial={privilegeMaterial}
                setPrivilegeMaterial={setPrivilegeMaterial}
                onUploadBackupArtifact={dashboard.uploadBackupArtifact}
                onUploadBackupArtifactChunked={
                  dashboard.uploadBackupArtifactChunked
                }
              />
            )}
            {activeView === "Access" && (
              <AccessPanel
                activeSubpage={activeSubpage}
                apiToken={dashboard.apiToken}
                error={dashboard.accessError}
                gatewaySessions={dashboard.gatewaySessions}
                lastLiveEvent={dashboard.lastLiveEvent}
                loading={dashboard.accessLoading}
                onClearSession={dashboard.clearSession}
                onConfirmTotp={dashboard.confirmTotp}
                onUpsertAgentIdentity={dashboard.upsertAgentIdentity}
                onCreateOperator={dashboard.createOperator}
                onDisableTotp={dashboard.disableTotp}
                onRefresh={dashboard.loadCurrentOperator}
                onRevokeClientKey={dashboard.revokeClientKey}
                onRevokeOperatorSession={dashboard.revokeOperatorSession}
                onSetupTotp={dashboard.setupTotp}
                operator={dashboard.operator}
                privilegeMaterial={privilegeMaterial}
                clientKeyRevocations={dashboard.clientKeyRevocations}
                keyLifecycleReport={dashboard.keyLifecycleReport}
                operatorSessions={dashboard.operatorSessions}
                operators={dashboard.operators}
                sessionVaultAvailable={dashboard.authVaultAvailable}
                setPrivilegeMaterial={setPrivilegeMaterial}
                wsState={dashboard.wsState}
              />
            )}
            {activeView === "System" && (
              <SystemPanel
                activeSubpage={activeSubpage}
                dashboard={dashboard.systemDashboard}
                dashboardError={dashboard.systemDashboardError}
                dashboardLoading={dashboard.systemDashboardLoading}
                dashboardPointDensity={dashboard.systemDashboardPointDensity}
                dashboardWindow={dashboard.systemDashboardWindow}
                onDashboardPointDensityChange={
                  dashboard.setSystemDashboardPointDensity
                }
                onDashboardRefresh={() => void dashboard.loadSystemDashboard()}
                onDashboardWindowChange={dashboard.setSystemDashboardWindow}
                onLoadSuiteConfig={() => void dashboard.loadSuiteConfig()}
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                onUpdateSuiteConfig={dashboard.updateSuiteConfig}
                onValidateSuiteConfig={dashboard.validateSuiteConfig}
                operator={dashboard.operator}
                privilegeMaterial={privilegeMaterial}
                suiteConfig={dashboard.suiteConfig}
                suiteConfigError={dashboard.suiteConfigError}
                suiteConfigLoading={dashboard.suiteConfigLoading}
              />
            )}
          </>
        )}
      </ConsoleShell>
    </PanelDisplayProvider>
  );
}

function isActiveView(value: string): value is ActiveView {
  return value in defaultSubpages;
}
