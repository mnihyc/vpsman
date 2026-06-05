import { useEffect, useMemo, useState } from "react";
import { ConsoleShell } from "./components/ConsoleShell";
import { AuthPanel } from "./panels/AuthPanel";
import { DashboardPanel } from "./panels/DashboardPanel";
import { FleetWorkspace } from "./panels/FleetWorkspace";
import { JobHistoryPanel } from "./panels/JobHistoryPanel";
import { TagsPanel } from "./panels/TagsPanel";
import { SchedulesPanel } from "./panels/SchedulesPanel";
import { AccessPanel } from "./panels/AccessPanel";
import { AuditLogPanel } from "./panels/AuditLogPanel";
import { BackupsPanel } from "./panels/BackupsPanel";
import { TopologyPanel } from "./panels/TopologyPanel";
import { PreferencesPanel } from "./panels/PreferencesPanel";
import { PanelDisplayProvider } from "./panelDisplay";
import type { ActiveView } from "./types";
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

export function App() {
  const [activeView, setActiveView] = useState<ActiveView>("Dashboard");
  const [activeSubpages, setActiveSubpages] = useState<Record<ActiveView, string>>({ ...defaultSubpages });
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const dashboard = useDashboardData(activeView);
  const fleetViews = useFleetViews(dashboard.agents);
  const operatorPreferences = dashboard.operator?.preferences ?? DEFAULT_OPERATOR_PREFERENCES;
  const visibleAgents = fleetViews.filteredAgents;
  const selectedAgent = useMemo(
    () => visibleAgents.find((agent) => agent.id === selectedAgentId) ?? visibleAgents[0] ?? null,
    [selectedAgentId, visibleAgents],
  );
  const visibleSummary = useMemo(
    () => ({
      connected: visibleAgents.filter((agent) => agent.status === "connected").length,
      total: visibleAgents.length,
    }),
    [visibleAgents],
  );
  const connectedRatio = useMemo(() => {
    if (dashboard.summary.total === 0) {
      return "0%";
    }
    return `${Math.round((dashboard.summary.connected / dashboard.summary.total) * 100)}%`;
  }, [dashboard.summary.connected, dashboard.summary.total]);
  const activeSubpage = normalizeSubpage(activeView, activeSubpages[activeView]);
  const heroTitle = getHeroTitle(activeView);
  const hasFleetScope = fleetViews.fleetQuery.trim().length > 0 || fleetViews.activeSavedViewId !== null;
  const heroCopy =
    activeView === "Fleet" && hasFleetScope
      ? `${visibleSummary.connected} visible connected / ${visibleSummary.total} visible / ${dashboard.summary.total} total`
      : activeView === "Fleet"
        ? `${dashboard.summary.connected} connected / ${dashboard.summary.total} total`
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
      setActiveSubpages((current) => ({ ...current, [view]: normalizeSubpage(view, subpage) }));
    }
  }

  function selectSubpage(subpage: string) {
    setActiveSubpages((current) => ({ ...current, [activeView]: normalizeSubpage(activeView, subpage) }));
  }

  function navigateDashboardTarget(target: { query: string | null; subpage: string; view: string }) {
    if (!isActiveView(target.view)) {
      return;
    }
    if (target.view === "Fleet" && target.query) {
      fleetViews.setFleetQuery(target.query);
    }
    selectView(target.view, target.subpage);
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
      apiToken={dashboard.apiToken}
      connectedRatio={connectedRatio}
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
      onOpenAccessControls={() => selectView("Access", "proof")}
      onSaveFleetView={fleetViews.saveFleetView}
      onSelectSubpage={selectSubpage}
      onSelectView={selectView}
      onSavedFleetViewNameChange={fleetViews.setDraftSavedViewName}
      operatorPreferencesReady={dashboard.operator !== null}
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
              fleetAlerts={dashboard.fleetAlerts}
              fleetAlertStates={dashboard.fleetAlertStates}
              fleetAlertPolicies={dashboard.fleetAlertPolicies}
              fleetAlertNotificationChannels={dashboard.fleetAlertNotificationChannels}
              fleetAlertNotifications={dashboard.fleetAlertNotifications}
              lastLiveEvent={dashboard.lastLiveEvent}
              onSelectAgent={setSelectedAgentId}
              onUpdateAgentAlias={dashboard.updateAgentAlias}
              scopeActive={hasFleetScope}
              onDispatchFleetAlertNotifications={dashboard.dispatchFleetAlertNotifications}
              onProcessFleetAlertNotifications={dashboard.processFleetAlertNotifications}
              onUpdateFleetAlertState={dashboard.updateFleetAlertState}
              onUpsertFleetAlertNotificationChannel={dashboard.upsertFleetAlertNotificationChannel}
              onUpsertFleetAlertPolicy={dashboard.upsertFleetAlertPolicy}
              selectedAgent={selectedAgent}
              summary={dashboard.summary}
              telemetryNetworkRates={dashboard.telemetryNetworkRates}
              telemetryRollups={dashboard.telemetryRollups}
              telemetryTunnels={dashboard.telemetryTunnels}
              wsState={dashboard.wsState}
            />
          )}
          {activeView === "Tags" && (
            <TagsPanel
              activeSubpage={activeSubpage}
              agents={visibleAgents}
              dataSourceAssignments={dashboard.dataSourceAssignments}
              dataSourcePresets={dashboard.dataSourcePresets}
              dataSourceStatus={dashboard.dataSourceStatus}
              error={dashboard.tagsError}
              loading={dashboard.tagsLoading}
              onAssignDataSourcePreset={dashboard.assignDataSourcePreset}
              onAssignTag={dashboard.assignTag}
              onCloneDataSourcePreset={dashboard.cloneDataSourcePreset}
              onCreateJob={dashboard.createJob}
              onCreateDataSourcePreset={dashboard.createDataSourcePreset}
              onCreateTag={dashboard.createTag}
              onDiffDataSourcePreset={dashboard.diffDataSourcePreset}
              onRefresh={dashboard.loadTagInventory}
              onRenderDataSourceHotConfig={dashboard.renderDataSourceHotConfig}
              onResolveBulk={dashboard.resolveBulkPreview}
              onTestDataSourcePreset={dashboard.testDataSourcePreset}
              onUpdateDataSourcePreset={dashboard.updateDataSourcePreset}
              tags={dashboard.tags}
            />
          )}
          {activeView === "Jobs" && (
            <JobHistoryPanel
              activeSubpage={activeSubpage}
              agents={visibleAgents}
              error={dashboard.jobsError}
              agentUpdateReleases={dashboard.agentUpdateReleases}
              agentUpdateRolloutPolicies={dashboard.agentUpdateRolloutPolicies}
              agentUpdateRollouts={dashboard.agentUpdateRollouts}
              jobs={dashboard.jobs}
              commandTemplates={dashboard.commandTemplates}
              fileTransfers={dashboard.fileTransfers}
              fileTransferSources={dashboard.fileTransferSources}
              lastJobOutputEvent={dashboard.lastJobOutputEvent}
              lastTerminalOutputEvent={dashboard.lastTerminalOutputEvent}
              loading={dashboard.jobsLoading}
              onCancelJob={dashboard.cancelJob}
              onCreateFileTransferHandoff={dashboard.createFileTransferHandoff}
              onCreateJob={dashboard.createJob}
              onCreateAgentUpdateRelease={dashboard.createAgentUpdateRelease}
              onCreateAgentUpdateRolloutPolicy={dashboard.createAgentUpdateRolloutPolicy}
              onDelegateAgentUpdateActivation={dashboard.delegateAgentUpdateActivation}
              onDelegateAgentUpdateRollback={dashboard.delegateAgentUpdateRollback}
              onUpdateAgentUpdateRolloutControl={dashboard.updateAgentUpdateRolloutControl}
              onUploadAgentUpdateArtifact={dashboard.uploadAgentUpdateArtifact}
              onDispatchScheduledJob={dashboard.dispatchScheduledJob}
              onDownloadOutputArtifact={dashboard.downloadJobOutputArtifact}
              onDownloadFileTransferSource={dashboard.downloadFileTransferSource}
              onSaveFileTransferHandoff={dashboard.saveFileTransferHandoff}
              onLoadJob={dashboard.loadJob}
              onLoadOutputs={dashboard.loadJobOutputs}
              onLoadOutputComparison={dashboard.loadJobOutputComparison}
              onLoadTargets={dashboard.loadJobTargets}
              onLoadTerminalReplay={dashboard.loadTerminalReplay}
              onRefresh={dashboard.loadJobs}
              onResolveTargets={dashboard.resolveJobTargets}
              onUploadFileTransferSource={dashboard.uploadFileTransferSource}
              onUpsertCommandTemplate={dashboard.upsertCommandTemplate}
              processSupervisorInventory={dashboard.processSupervisorInventory}
              terminalSessions={dashboard.terminalSessions}
              tags={dashboard.tags}
            />
          )}
          {activeView === "Schedules" && (
            <SchedulesPanel
              activeSubpage={activeSubpage}
              agents={visibleAgents}
              error={dashboard.schedulesError}
              loading={dashboard.schedulesLoading}
              onCreateSchedule={dashboard.createSchedule}
              onRefresh={dashboard.loadSchedules}
              schedules={dashboard.schedules}
              tags={dashboard.tags}
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
              onPromoteTelemetryTunnel={dashboard.promoteTelemetryTunnel}
              onPromoteTunnelPlanToAdapter={dashboard.promoteTunnelPlanToAdapter}
              onRefresh={dashboard.loadTunnelPlans}
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
              onUpsertHistoryRetentionPolicy={dashboard.upsertHistoryRetentionPolicy}
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
              onPrepareBackupArtifactRestore={dashboard.prepareBackupArtifactRestore}
              onPruneBackupPolicies={dashboard.pruneBackupPolicies}
              onRefresh={dashboard.loadBackups}
              onUploadBackupArtifact={dashboard.uploadBackupArtifact}
              onUploadBackupArtifactChunked={dashboard.uploadBackupArtifactChunked}
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
              onCreateEnrollmentToken={dashboard.createEnrollmentToken}
              onCreateOperator={dashboard.createOperator}
              onDisableTotp={dashboard.disableTotp}
              onRefresh={dashboard.loadCurrentOperator}
              onRevokeClientKey={dashboard.revokeClientKey}
              onRevokeOperatorSession={dashboard.revokeOperatorSession}
              onSetupTotp={dashboard.setupTotp}
              operator={dashboard.operator}
              clientKeyRevocations={dashboard.clientKeyRevocations}
              enrollmentTokens={dashboard.enrollmentTokens}
              keyLifecycleReport={dashboard.keyLifecycleReport}
              operatorSessions={dashboard.operatorSessions}
              operators={dashboard.operators}
              proofRotations={dashboard.proofRotations}
              sessionVaultAvailable={dashboard.authVaultAvailable}
              wsState={dashboard.wsState}
            />
          )}
          {activeView === "Preferences" && <PreferencesPanel operator={dashboard.operator} />}
        </>
      )}
    </ConsoleShell>
    </PanelDisplayProvider>
  );
}

function isActiveView(value: string): value is ActiveView {
  return value in defaultSubpages;
}
