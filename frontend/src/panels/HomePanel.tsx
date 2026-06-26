import { useEffect, useMemo, useState } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowRight,
  Clock3,
  DatabaseBackup,
  FolderOpen,
  History,
  Network,
  Play,
  Server,
  ShieldAlert,
  TerminalSquare,
} from "lucide-react";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import { VpsCombobox } from "../components/VpsCombobox";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import type {
  AgentView,
  AuditLogRecord,
  BackupArtifactRecord,
  BackupRequestRecord,
  DashboardDrilldownRecord,
  DashboardOverviewRecord,
  DashboardPreferences,
  DashboardWindow,
  FleetAlertRecord,
  FleetSummary,
  JobHistoryRecord,
  SystemDashboardRecord,
  ScheduleRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";
import { displayNameOrUnnamed, formatCompactTime, shortId } from "../utils";
import { FleetMonitorPanel } from "./FleetMonitorPanel";
import { HomeTelemetryPanel } from "./HomeTelemetryPanel";

type HomePanelProps = {
  agents: AgentView[];
  allAgents: AgentView[];
  auditLogs: AuditLogRecord[];
  backupArtifacts: BackupArtifactRecord[];
  backups: BackupRequestRecord[];
  dashboardError: string | null;
  dashboardLoading: boolean;
  dashboardOverview: DashboardOverviewRecord | null;
  dashboardPreferences: DashboardPreferences;
  dashboardWindow: DashboardWindow;
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  schedules: ScheduleRecord[];
  summary: FleetSummary;
  systemDashboard: SystemDashboardRecord | null;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  onDashboardNavigate: (drilldown: DashboardDrilldownRecord) => void;
  onDashboardPreferencesChange: (patch: Partial<DashboardPreferences>) => void;
  onDashboardRefresh: () => void;
  onDashboardWindowChange: (window: DashboardWindow) => void;
  onOpenAudit: () => void;
  onOpenBackup: (agent: AgentView) => void;
  onOpenBackups: () => void;
  onOpenDispatch: (agent: AgentView) => void;
  onOpenFiles: (agent: AgentView) => void;
  onOpenFleetAlerts: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobs: () => void;
  onOpenNetwork: (agent: AgentView) => void;
  onOpenNetworkEvidence: (agent?: AgentView) => void;
  onOpenProcesses: (agent: AgentView) => void;
  onOpenSchedule: () => void;
  onOpenSystemCapacity: () => void;
  onOpenTerminal: (agent: AgentView) => void;
  onOpenTransfers: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
};

type HomeActionItem = {
  detail: string;
  id: string;
  label: string;
  meta: string;
  onOpen: () => void;
  tone: "critical" | "warning" | "info" | "ok";
};

type HomeActivityItem = {
  id: string;
  label: string;
  meta: string;
  onOpen: () => void;
  time: string;
  type: string;
};

export function HomePanel({
  agents,
  allAgents,
  auditLogs,
  backupArtifacts,
  backups,
  dashboardError,
  dashboardLoading,
  dashboardOverview,
  dashboardPreferences,
  dashboardWindow,
  fileTransfers,
  fleetAlerts,
  jobs,
  schedules,
  summary,
  systemDashboard,
  telemetryNetworkRates,
  telemetryRollups,
  telemetryTunnels,
  onDashboardNavigate,
  onDashboardPreferencesChange,
  onDashboardRefresh,
  onDashboardWindowChange,
  onOpenAudit,
  onOpenBackup,
  onOpenBackups,
  onOpenDispatch,
  onOpenFiles,
  onOpenFleetAlerts,
  onOpenJobDetails,
  onOpenJobs,
  onOpenNetwork,
  onOpenNetworkEvidence,
  onOpenProcesses,
  onOpenSchedule,
  onOpenSystemCapacity,
  onOpenTerminal,
  onOpenTransfers,
  onOpenVpsDetail,
}: HomePanelProps) {
  const [quickTargetId, setQuickTargetId] = useState("");
  const quickTarget = agents.find((agent) => agent.id === quickTargetId) ?? agents[0] ?? null;
  const visibleOnline = agents.filter((agent) => agent.status === "online").length;
  const visibleStale = agents.filter((agent) => agent.status === "stale" || agent.stale_since).length;
  const visibleOffline = agents.filter((agent) => agent.status === "offline").length;
  const runningJobs = jobs.filter((job) => isActiveJobStatus(job.status)).length || summary.running_jobs;
  const failedJobs = jobs.filter((job) => isFailedJobStatus(job.status)).length;
  const failedBackups = backups.filter((backup) => isFailedBackupStatus(backup.status)).length;
  const activeTransfers = fileTransfers.filter((transfer) => isActiveTransferStatus(transfer.status)).length;
  const criticalAlerts = fleetAlerts.filter((alert) => alert.severity === "critical" && alert.operator_state !== "acknowledged").length;
  const warningAlerts = fleetAlerts.filter((alert) => alert.severity !== "critical" && alert.operator_state !== "acknowledged").length;

  useEffect(() => {
    if (agents.length === 0) {
      setQuickTargetId("");
      return;
    }
    if (!agents.some((agent) => agent.id === quickTargetId)) {
      setQuickTargetId(agents[0].id);
    }
  }, [agents, quickTargetId]);

  const attentionItems = useMemo(
    () =>
      buildAttentionItems({
        agents,
        backups,
        fileTransfers,
        fleetAlerts,
        jobs,
        onOpenBackup,
        onOpenFleetAlerts,
        onOpenJobDetails,
        onOpenNetworkEvidence,
        onOpenTransfers,
        onOpenSystemCapacity,
        onOpenVpsDetail,
        systemDashboard,
      }),
    [
      agents,
      backups,
      fileTransfers,
      fleetAlerts,
      jobs,
      onOpenBackup,
      onOpenFleetAlerts,
      onOpenJobDetails,
      onOpenNetworkEvidence,
      onOpenTransfers,
      onOpenSystemCapacity,
      onOpenVpsDetail,
      systemDashboard,
    ],
  );
  const activityItems = useMemo(
    () =>
      buildActivityItems({
        auditLogs,
        backups,
        fileTransfers,
        jobs,
        onOpenAudit,
        onOpenBackups,
        onOpenJobDetails,
        onOpenSchedule,
        onOpenTransfers,
        schedules,
      }),
    [
      auditLogs,
      backups,
      fileTransfers,
      jobs,
      onOpenAudit,
      onOpenBackups,
      onOpenJobDetails,
      onOpenSchedule,
      onOpenTransfers,
      schedules,
    ],
  );

  return (
    <div className="homeWorkspace">
      <section className="homeReleaseLayer" aria-labelledby="home-release-title">
        <div className="homeCommandBand">
          <div className="homeCommandIntro">
            <h2 id="home-release-title">Fleet command home</h2>
            <p>
              Scan VPS health, pick a target, and jump into reviewed operations without leaving the release IA.
            </p>
            <div className="homeInlineStatus" aria-label="Home fleet posture">
              <ConsoleStatusBadge tone={visibleOnline === agents.length && criticalAlerts === 0 ? "ok" : "warning"}>
                {visibleOnline}/{agents.length} visible online
              </ConsoleStatusBadge>
              <ConsoleStatusBadge tone={criticalAlerts > 0 ? "critical" : warningAlerts > 0 ? "warning" : "ok"}>
                {criticalAlerts} critical / {warningAlerts} warning
              </ConsoleStatusBadge>
              <ConsoleStatusBadge tone={runningJobs > 0 ? "info" : "neutral"}>
                {runningJobs} running jobs
              </ConsoleStatusBadge>
            </div>
          </div>
          <div className="homeQuickActions" aria-label="Home quick actions">
            <label>
              <span>Target VPS</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Home quick action target"
                onChange={setQuickTargetId}
                placeholder="Select VPS"
                value={quickTarget?.id ?? ""}
              />
            </label>
            <div className="homeQuickActionGrid">
              <button
                className="primaryAction compactAction"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenTerminal(quickTarget)}
                type="button"
              >
                <TerminalSquare size={16} />
                <span>Open terminal</span>
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenFiles(quickTarget)}
                type="button"
              >
                <FolderOpen size={16} />
                <span>Browse files</span>
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenDispatch(quickTarget)}
                type="button"
              >
                <Play size={16} />
                <span>Dispatch command</span>
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenBackup(quickTarget)}
                type="button"
              >
                <DatabaseBackup size={16} />
                <span>Run backup</span>
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={!quickTarget}
                onClick={() => quickTarget && onOpenNetwork(quickTarget)}
                type="button"
              >
                <Network size={16} />
                <span>View network</span>
              </button>
            </div>
          </div>
        </div>

        <div className="homePostureStrip" aria-label="Home posture strip">
          <HomePostureMetric
            detail={`${visibleOnline} visible / ${summary.online} fleet`}
            label="Online"
            tone={visibleOnline === agents.length ? "ok" : "warning"}
            value={`${visibleOnline}/${agents.length}`}
          />
          <HomePostureMetric
            detail={`${visibleStale} stale, ${visibleOffline} offline`}
            label="Reachability gaps"
            tone={visibleStale || visibleOffline ? "warning" : "ok"}
            value={String(visibleStale + visibleOffline)}
          />
          <HomePostureMetric
            detail={`${criticalAlerts} critical, ${warningAlerts} warning`}
            label="Open alerts"
            tone={criticalAlerts ? "critical" : warningAlerts ? "warning" : "ok"}
            value={String(criticalAlerts + warningAlerts)}
          />
          <HomePostureMetric
            detail={`${failedJobs} failed in loaded history`}
            label="Running jobs"
            tone={failedJobs ? "critical" : runningJobs ? "info" : "ok"}
            value={String(runningJobs)}
          />
          <HomePostureMetric
            detail={`${failedBackups} failed, ${backupArtifacts.length} artifacts`}
            label="Backups"
            tone={failedBackups ? "critical" : "ok"}
            value={String(backups.length)}
          />
          <HomePostureMetric
            detail={`${activeTransfers} active transfer sessions`}
            label="Transfers"
            tone={activeTransfers ? "info" : "ok"}
            value={String(fileTransfers.length)}
          />
        </div>

        <FleetMonitorPanel
          agents={agents}
          ariaLabel="Home fleet scan"
          backups={backups}
          description="Komari-style VPS cards for health scanning before opening terminal, files, backup, process, or network workflows."
          embedded
          failedJobCount={failedJobs}
          fileTransfers={fileTransfers}
          fleetAlerts={fleetAlerts}
          jobs={jobs}
          maxCards={8}
          runningJobCount={runningJobs}
          telemetryNetworkRates={telemetryNetworkRates}
          telemetryRollups={telemetryRollups}
          telemetryTunnels={telemetryTunnels}
          title="Fleet scan"
          toolbarAction={
            <button className="secondaryAction compactAction" onClick={onOpenJobs} type="button">
              <History size={15} />
              <span>Job history</span>
            </button>
          }
          onOpenBackup={onOpenBackup}
          onOpenFiles={onOpenFiles}
          onOpenNetwork={onOpenNetwork}
          onOpenProcesses={onOpenProcesses}
          onOpenTerminal={onOpenTerminal}
          onOpenVpsDetail={onOpenVpsDetail}
        />

        <div className="homeReviewGrid">
          <section className="homeReviewPanel" aria-labelledby="home-attention-title">
            <div className="homePanelHeader">
              <div>
                <h2 id="home-attention-title">Needs attention</h2>
                <span>Failed work, stale agents, backup risk, degraded network, and access capability gaps.</span>
              </div>
              <ConsoleStatusBadge tone={attentionItems.length ? "warning" : "ok"}>
                {attentionItems.length} item{attentionItems.length === 1 ? "" : "s"}
              </ConsoleStatusBadge>
            </div>
            {attentionItems.length === 0 ? (
              <div className="homeQuietState">
                <ShieldAlert size={18} />
                <span>No loaded evidence needs attention.</span>
              </div>
            ) : (
              <div className="homeActionList">
                {attentionItems.map((item) => (
                  <button className={`homeActionRow ${item.tone}`} key={item.id} onClick={item.onOpen} type="button">
                    <span className="homeActionGlyph" aria-hidden="true">
                      {item.tone === "critical" ? <AlertTriangle size={16} /> : <Activity size={16} />}
                    </span>
                    <span className="homeActionText">
                      <strong>{item.label}</strong>
                      <small>{item.detail}</small>
                    </span>
                    <span className="homeActionMeta">{item.meta}</span>
                    <ArrowRight size={15} />
                  </button>
                ))}
              </div>
            )}
          </section>

          <section className="homeReviewPanel" aria-labelledby="home-activity-title">
            <div className="homePanelHeader">
              <div>
                <h2 id="home-activity-title">Recent activity</h2>
                <span>Audit, job, backup, transfer, and schedule evidence from loaded records.</span>
              </div>
              <ConsoleStatusBadge tone="neutral">{activityItems.length} shown</ConsoleStatusBadge>
            </div>
            {activityItems.length === 0 ? (
              <div className="homeQuietState">
                <Clock3 size={18} />
                <span>No recent activity loaded.</span>
              </div>
            ) : (
              <div className="homeActivityList">
                {activityItems.map((item) => (
                  <button className="homeActivityRow" key={item.id} onClick={item.onOpen} type="button">
                    <span className="homeActivityType">{item.type}</span>
                    <span className="homeActivityText">
                      <strong>{item.label}</strong>
                      <small>{item.meta}</small>
                    </span>
                    <time>{item.time}</time>
                  </button>
                ))}
              </div>
            )}
          </section>
        </div>
      </section>

      <section className="homeDashboardLayer" aria-label="Home telemetry widgets">
        <HomeTelemetryPanel
          agents={allAgents}
          error={dashboardError}
          loading={dashboardLoading}
          onNavigate={onDashboardNavigate}
          onPreferencesChange={onDashboardPreferencesChange}
          onRefresh={onDashboardRefresh}
          onWindowChange={onDashboardWindowChange}
          overview={dashboardOverview}
          preferences={dashboardPreferences}
          window={dashboardWindow}
        />
      </section>
    </div>
  );
}

function HomePostureMetric({
  detail,
  label,
  tone,
  value,
}: {
  detail: string;
  label: string;
  tone: "critical" | "warning" | "info" | "ok";
  value: string;
}) {
  return (
    <div className={`homePostureMetric ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function buildAttentionItems({
  agents,
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  onOpenBackup,
  onOpenFleetAlerts,
  onOpenJobDetails,
  onOpenNetworkEvidence,
  onOpenTransfers,
  onOpenSystemCapacity,
  onOpenVpsDetail,
  systemDashboard,
}: {
  agents: AgentView[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  onOpenBackup: (agent: AgentView) => void;
  onOpenFleetAlerts: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenNetworkEvidence: (agent?: AgentView) => void;
  onOpenTransfers: () => void;
  onOpenSystemCapacity: () => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  systemDashboard: SystemDashboardRecord | null;
}): HomeActionItem[] {
  const agentById = new Map(agents.map((agent) => [agent.id, agent]));
  const alertItems = fleetAlerts
    .filter((alert) => alert.operator_state !== "acknowledged")
    .map((alert) => {
      const alertAgent = alert.client_id ? agentById.get(alert.client_id) : undefined;
      return {
        detail: `${alert.category} / ${alert.client_id ? displayNameOrUnnamed(alertAgent?.display_name ?? alert.client_id) : alert.target_id}`,
        id: `alert:${alert.id}`,
        label: alert.title,
        meta: formatCompactTime(alert.observed_at),
        onOpen:
          alert.category === "network"
            ? () => onOpenNetworkEvidence(alertAgent)
            : onOpenFleetAlerts,
        tone: alert.severity === "critical" ? "critical" : "warning",
      } satisfies HomeActionItem;
    });
  const agentItems = agents
    .filter((agent) => agent.status !== "online" || agent.stale_since || agent.capabilities.privilege_mode === "unknown")
    .map((agent) => ({
      detail: agent.stale_reason ?? `${agent.status}; privilege ${agent.capabilities.privilege_mode}`,
      id: `agent:${agent.id}`,
      label: `${displayNameOrUnnamed(agent.display_name)} needs review`,
      meta: agent.last_seen_at ? formatCompactTime(agent.last_seen_at) : "no heartbeat",
      onOpen: () => onOpenVpsDetail(agent),
      tone: agent.status === "offline" ? "critical" : "warning",
    }) satisfies HomeActionItem);
  const jobItems = jobs
    .filter((job) => isFailedJobStatus(job.status))
    .map((job) => ({
      detail: `${job.command_type} / ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      id: `job:${job.id}`,
      label: `Job ${shortId(job.id)} failed`,
      meta: formatCompactTime(job.completed_at ?? job.created_at),
      onOpen: () => onOpenJobDetails(job.id),
      tone: "critical",
    }) satisfies HomeActionItem);
  const transferItems = fileTransfers
    .filter((transfer) => transfer.status === "aborted" || transfer.status === "unknown")
    .map((transfer) => ({
      detail: `${transfer.direction} ${transfer.path}`,
      id: `transfer:${transfer.client_id}:${transfer.session_id}`,
      label: `Transfer ${shortId(transfer.session_id)} needs retry`,
      meta: formatCompactTime(transfer.observed_at),
      onOpen: onOpenTransfers,
      tone: transfer.status === "unknown" ? "warning" : "critical",
    }) satisfies HomeActionItem);
  const backupItems = backups
    .filter((backup) => isFailedBackupStatus(backup.status))
    .map((backup) => {
      const agent = agentById.get(backup.client_id);
      return {
        detail: `${displayNameOrUnnamed(agent?.display_name ?? backup.client_id)} / ${backup.paths.join(", ")}`,
        id: `backup:${backup.id}`,
        label: `Backup ${shortId(backup.id)} failed`,
        meta: formatCompactTime(backup.created_at),
        onOpen: () => (agent ? onOpenBackup(agent) : undefined),
        tone: "critical",
      } satisfies HomeActionItem;
    });
  const systemItems = buildSystemAttentionItems(systemDashboard, onOpenSystemCapacity);
  return [...alertItems, ...agentItems, ...jobItems, ...transferItems, ...backupItems, ...systemItems]
    .sort(compareAttentionItems)
    .slice(0, 8);
}

function buildSystemAttentionItems(
  systemDashboard: SystemDashboardRecord | null,
  onOpenSystemCapacity: () => void,
): HomeActionItem[] {
  if (!systemDashboard) {
    return [];
  }
  const dispatch = systemDashboard.current.dispatch;
  const gateway = systemDashboard.current.gateway_events;
  const droppedEvents = (gateway.dropped_events ?? 0) + (gateway.telemetry_dropped_events ?? 0);
  const criticalGatewayFailures = gateway.critical_failures ?? 0;
  const queueDepth = dispatch.queue_depth + (gateway.current_queue_depth ?? 0);
  const items: HomeActionItem[] = [];
  if (criticalGatewayFailures > 0 || droppedEvents > 0) {
    items.push({
      detail: `${droppedEvents} dropped events, ${criticalGatewayFailures} critical failures`,
      id: "system:gateway-drops",
      label: "Gateway event drops need review",
      meta: "System / Capacity",
      onOpen: onOpenSystemCapacity,
      tone: criticalGatewayFailures > 0 ? "critical" : "warning",
    });
  }
  if (queueDepth > 0) {
    items.push({
      detail: `${dispatch.active_jobs} active jobs, ${queueDepth} queued dispatch/gateway events`,
      id: "system:dispatch-queue",
      label: "Control-plane queue pressure",
      meta: "System / Capacity",
      onOpen: onOpenSystemCapacity,
      tone: queueDepth > 10 ? "critical" : "warning",
    });
  }
  return items;
}

function buildActivityItems({
  auditLogs,
  backups,
  fileTransfers,
  jobs,
  onOpenAudit,
  onOpenBackups,
  onOpenJobDetails,
  onOpenSchedule,
  onOpenTransfers,
  schedules,
}: {
  auditLogs: AuditLogRecord[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  jobs: JobHistoryRecord[];
  onOpenAudit: () => void;
  onOpenBackups: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenSchedule: () => void;
  onOpenTransfers: () => void;
  schedules: ScheduleRecord[];
}): HomeActivityItem[] {
  const jobItems = jobs.map((job) => ({
    id: `job:${job.id}`,
    label: `${job.command_type} job ${job.status}`,
    meta: `${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
    onOpen: () => onOpenJobDetails(job.id),
    time: job.completed_at ?? job.created_at,
    type: "Job",
  }));
  const backupItems = backups.map((backup) => ({
    id: `backup:${backup.id}`,
    label: `Backup ${backup.status.replace(/_/g, " ")}`,
    meta: `${backup.client_id} / ${backup.paths.join(", ")}`,
    onOpen: onOpenBackups,
    time: backup.created_at,
    type: "Backup",
  }));
  const transferItems = fileTransfers.map((transfer) => ({
    id: `transfer:${transfer.client_id}:${transfer.session_id}`,
    label: `${transfer.direction} transfer ${transfer.status}`,
    meta: transfer.path,
    onOpen: onOpenTransfers,
    time: transfer.observed_at,
    type: "Transfer",
  }));
  const auditItems = auditLogs.map((audit) => ({
    id: `audit:${audit.id}`,
    label: audit.action.replace(/_/g, " "),
    meta: audit.target,
    onOpen: onOpenAudit,
    time: audit.created_at,
    type: "Audit",
  }));
  const scheduleItems = schedules.map((schedule) => ({
    id: `schedule:${schedule.id}`,
    label: `${schedule.name} ${schedule.enabled ? "enabled" : "disabled"}`,
    meta: `${schedule.command_type} / ${schedule.selector_expression}`,
    onOpen: onOpenSchedule,
    time: schedule.updated_at,
    type: "Schedule",
  }));
  return [...jobItems, ...backupItems, ...transferItems, ...auditItems, ...scheduleItems]
    .sort((left, right) => new Date(right.time).getTime() - new Date(left.time).getTime())
    .slice(0, 8)
    .map((item) => ({
      ...item,
      time: formatCompactTime(item.time),
    }));
}

function compareAttentionItems(left: HomeActionItem, right: HomeActionItem) {
  return attentionRank(right.tone) - attentionRank(left.tone) || left.label.localeCompare(right.label);
}

function attentionRank(tone: HomeActionItem["tone"]) {
  if (tone === "critical") return 3;
  if (tone === "warning") return 2;
  if (tone === "info") return 1;
  return 0;
}

function isActiveJobStatus(status: string) {
  return ["queued", "dispatching", "running"].includes(status);
}

function isFailedJobStatus(status: string) {
  return ["failed", "rejected", "agent_lost", "agent_timeout", "control_timeout", "deadline_expired"].includes(status);
}

function isFailedBackupStatus(status: string) {
  return status === "execution_failed" || status === "execution_canceled";
}

function isActiveTransferStatus(status: string) {
  return status === "started" || status === "transferring";
}
