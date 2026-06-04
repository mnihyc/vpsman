import { useMemo, useState } from "react";
import { ConsoleShell } from "./components/ConsoleShell";
import { AuthPanel } from "./panels/AuthPanel";
import { FleetWorkspace } from "./panels/FleetWorkspace";
import { JobHistoryPanel } from "./panels/JobHistoryPanel";
import { PoolsTagsPanel } from "./panels/PoolsTagsPanel";
import { SchedulesPanel } from "./panels/SchedulesPanel";
import { AccessPanel } from "./panels/AccessPanel";
import { AuditLogPanel } from "./panels/AuditLogPanel";
import { BackupsPanel } from "./panels/BackupsPanel";
import { TopologyPanel } from "./panels/TopologyPanel";
import type { ActiveView } from "./types";
import { getHeroCopy, getHeroTitle } from "./utils";
import { useDashboardData } from "./hooks/useDashboardData";
import { useFleetViews } from "./hooks/useFleetViews";

export function App() {
  const [activeView, setActiveView] = useState<ActiveView>("Fleet");
  const [selectedAgentId, setSelectedAgentId] = useState<string | null>(null);
  const dashboard = useDashboardData(activeView);
  const fleetViews = useFleetViews(dashboard.agents, dashboard.pools);
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
  const heroTitle = getHeroTitle(activeView);
  const hasFleetScope = fleetViews.fleetQuery.trim().length > 0 || fleetViews.activeSavedViewId !== null;
  const heroCopy =
    activeView === "Fleet" && hasFleetScope
      ? `${visibleSummary.connected} visible connected / ${visibleSummary.total} visible / ${dashboard.summary.total} total`
      : activeView === "Fleet"
        ? `${dashboard.summary.connected} connected / ${dashboard.summary.total} total`
      : getHeroCopy(activeView);

  return (
    <ConsoleShell
      activeSavedFleetViewId={fleetViews.activeSavedViewId}
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
      onSaveFleetView={fleetViews.saveFleetView}
      onSelectView={setActiveView}
      onSavedFleetViewNameChange={fleetViews.setDraftSavedViewName}
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
          {activeView === "Fleet" && (
            <FleetWorkspace
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
          {activeView === "Pools" && (
            <PoolsTagsPanel
              agents={visibleAgents}
              dataSourceAssignments={dashboard.dataSourceAssignments}
              dataSourcePresets={dashboard.dataSourcePresets}
              dataSourceStatus={dashboard.dataSourceStatus}
              error={dashboard.poolsError}
              loading={dashboard.poolsLoading}
              onAssignDataSourcePreset={dashboard.assignDataSourcePreset}
              onAssignPool={dashboard.assignPool}
              onAssignTag={dashboard.assignTag}
              onCloneDataSourcePreset={dashboard.cloneDataSourcePreset}
              onCreateJob={dashboard.createJob}
              onCreateDataSourcePreset={dashboard.createDataSourcePreset}
              onCreatePool={dashboard.createPool}
              onCreateTag={dashboard.createTag}
              onDiffDataSourcePreset={dashboard.diffDataSourcePreset}
              onRefresh={dashboard.loadPoolsAndTags}
              onRenderDataSourceHotConfig={dashboard.renderDataSourceHotConfig}
              onResolveBulk={dashboard.resolveBulkPreview}
              onTestDataSourcePreset={dashboard.testDataSourcePreset}
              onUpdateDataSourcePreset={dashboard.updateDataSourcePreset}
              pools={dashboard.pools}
              tags={dashboard.tags}
            />
          )}
          {activeView === "Jobs" && (
            <JobHistoryPanel
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
              pools={dashboard.pools}
              processSupervisorInventory={dashboard.processSupervisorInventory}
              terminalSessions={dashboard.terminalSessions}
              tags={dashboard.tags}
            />
          )}
          {activeView === "Schedules" && (
            <SchedulesPanel
              agents={visibleAgents}
              error={dashboard.schedulesError}
              loading={dashboard.schedulesLoading}
              onCreateSchedule={dashboard.createSchedule}
              onRefresh={dashboard.loadSchedules}
              pools={dashboard.pools}
              schedules={dashboard.schedules}
              tags={dashboard.tags}
            />
          )}
          {activeView === "Topology" && (
            <TopologyPanel
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
              onPruneBackupPolicies={dashboard.pruneBackupPolicies}
              onRefresh={dashboard.loadBackups}
              onUploadBackupArtifact={dashboard.uploadBackupArtifact}
              onUploadBackupArtifactChunked={dashboard.uploadBackupArtifactChunked}
            />
          )}
          {activeView === "Access" && (
            <AccessPanel
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
              pools={dashboard.pools}
              proofRotations={dashboard.proofRotations}
              sessionVaultAvailable={dashboard.authVaultAvailable}
              wsState={dashboard.wsState}
            />
          )}
        </>
      )}
    </ConsoleShell>
  );
}
