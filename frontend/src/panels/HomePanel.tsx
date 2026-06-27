import { useEffect, useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowRight,
  Clock3,
  DatabaseBackup,
  FolderOpen,
  Network,
  Play,
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
import { displayNameOrUnnamed, formatCompactTime, formatFullTime, shortId } from "../utils";

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
  metaTitle?: string;
  onOpen: () => void;
  tone: "critical" | "warning" | "info" | "ok";
};

type HomeActivityItem = {
  id: string;
  label: string;
  meta: string;
  onOpen: () => void;
  time: string;
  timeDateTime?: string;
  timeTitle?: string;
  type: string;
};

export function HomePanel({
  agents,
  auditLogs,
  backupArtifacts,
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  schedules,
  summary,
  systemDashboard,
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
  const runningWorkItems = useMemo(
    () =>
      buildRunningWorkItems({
        backups,
        fileTransfers,
        jobs,
        onOpenJobs,
        onOpenBackups,
        onOpenJobDetails,
        onOpenTransfers,
        runningJobCount: runningJobs,
      }),
    [
      backups,
      fileTransfers,
      jobs,
      onOpenBackups,
      onOpenJobDetails,
      onOpenJobs,
      onOpenTransfers,
      runningJobs,
    ],
  );
  const recentFailureItems = useMemo(
    () =>
      buildRecentFailureItems({
        backups,
        fileTransfers,
        fleetAlerts,
        jobs,
        onOpenBackups,
        onOpenFleetAlerts,
        onOpenJobDetails,
        onOpenTransfers,
      }),
    [
      backups,
      fileTransfers,
      fleetAlerts,
      jobs,
      onOpenBackups,
      onOpenFleetAlerts,
      onOpenJobDetails,
      onOpenTransfers,
    ],
  );

  return (
    <div className="homeWorkspace">
      <section className="homeReleaseLayer" aria-labelledby="home-release-title">
        <div className="homeCommandBand">
          <div className="homeCommandIntro">
            <h2 id="home-release-title">Fleet command home</h2>
            <p>
              Scan VPS health, pick a target, and jump into reviewed operations without hunting through subsystem pages.
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
            {!quickTarget && (
              <div className="homeQuietState" aria-label="Home empty scope notice">
                <ShieldAlert size={18} />
                <span>No VPS in the current scope. Adjust the fleet scope or wait for agents to report telemetry.</span>
              </div>
            )}
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

        <div className="homeWorkGrid">
          <HomeActionPanel
            badge={`${runningWorkItems.length} active`}
            emptyIcon={<Clock3 size={18} />}
            emptyText="No running jobs, transfers, or backup requests in loaded records."
            id="home-running-work-title"
            items={runningWorkItems}
            subtitle="Long-running jobs and transfer work that may need follow-up."
            title="Running work"
          />
          <HomeActionPanel
            badge={`${recentFailureItems.length} recent`}
            emptyIcon={<ShieldAlert size={18} />}
            emptyText="No recent failures or unacknowledged alerts in loaded records."
            id="home-recent-failures-title"
            items={recentFailureItems}
            subtitle="Failed jobs, transfers, backups, and active alerts routed to their owner pages."
            title="Recent failures"
          />
        </div>

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
                    <span className="homeActionMeta" title={item.metaTitle}>
                      {item.meta}
                    </span>
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
                    <time dateTime={item.timeDateTime} title={item.timeTitle}>
                      {item.time}
                    </time>
                  </button>
                ))}
              </div>
            )}
          </section>
        </div>
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

function HomeActionPanel({
  badge,
  emptyIcon,
  emptyText,
  id,
  items,
  subtitle,
  title,
}: {
  badge: string;
  emptyIcon: ReactNode;
  emptyText: string;
  id: string;
  items: HomeActionItem[];
  subtitle: string;
  title: string;
}) {
  return (
    <section className="homeReviewPanel" aria-labelledby={id}>
      <div className="homePanelHeader">
        <div>
          <h2 id={id}>{title}</h2>
          <span>{subtitle}</span>
        </div>
        <ConsoleStatusBadge tone={items.length ? "info" : "ok"}>{badge}</ConsoleStatusBadge>
      </div>
      {items.length === 0 ? (
        <div className="homeQuietState">
          {emptyIcon}
          <span>{emptyText}</span>
        </div>
      ) : (
        <div className="homeActionList">
          {items.map((item) => (
            <button className={`homeActionRow ${item.tone}`} key={item.id} onClick={item.onOpen} type="button">
              <span className="homeActionGlyph" aria-hidden="true">
                {item.tone === "critical" ? <AlertTriangle size={16} /> : <Activity size={16} />}
              </span>
              <span className="homeActionText">
                <strong>{item.label}</strong>
                <small>{item.detail}</small>
              </span>
              <span className="homeActionMeta" title={item.metaTitle}>
                {item.meta}
              </span>
              <ArrowRight size={15} />
            </button>
          ))}
        </div>
      )}
    </section>
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
        metaTitle: formatFullTime(alert.observed_at),
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
      metaTitle: agent.last_seen_at ? formatFullTime(agent.last_seen_at) : undefined,
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
      metaTitle: formatFullTime(job.completed_at ?? job.created_at),
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
      metaTitle: formatFullTime(transfer.observed_at),
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
        metaTitle: formatFullTime(backup.created_at),
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

function buildRunningWorkItems({
  backups,
  fileTransfers,
  jobs,
  onOpenBackups,
  onOpenJobDetails,
  onOpenJobs,
  onOpenTransfers,
  runningJobCount,
}: {
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  jobs: JobHistoryRecord[];
  onOpenBackups: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenJobs: () => void;
  onOpenTransfers: () => void;
  runningJobCount: number;
}): HomeActionItem[] {
  const jobItems = jobs
    .filter((job) => isActiveJobStatus(job.status))
    .map((job) => ({
      detail: `${readableJobCommand(job.command_type)} / ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      id: `running-job:${job.id}`,
      label: `Job ${shortId(job.id)} ${readableJobStatus(job.status)}`,
      meta: formatCompactTime(job.created_at),
      metaTitle: formatFullTime(job.created_at),
      onOpen: () => onOpenJobDetails(job.id),
      tone: "info",
    }) satisfies HomeActionItem);
  const transferItems = fileTransfers
    .filter((transfer) => isActiveTransferStatus(transfer.status))
    .map((transfer) => ({
      detail: `${readableTransferDirection(transfer.direction)} ${transfer.path}`,
      id: `running-transfer:${transfer.client_id}:${transfer.session_id}`,
      label: `Transfer ${shortId(transfer.session_id)} ${readableTransferStatus(transfer.status)}`,
      meta: formatCompactTime(transfer.observed_at),
      metaTitle: formatFullTime(transfer.observed_at),
      onOpen: onOpenTransfers,
      tone: "info",
    }) satisfies HomeActionItem);
  const backupItems = backups
    .filter((backup) => isActiveBackupStatus(backup.status))
    .map((backup) => ({
      detail: `${backup.client_id} / ${backup.paths.join(", ")}`,
      id: `running-backup:${backup.id}`,
      label: `Backup ${shortId(backup.id)} ${readableBackupStatus(backup.status)}`,
      meta: formatCompactTime(backup.created_at),
      metaTitle: formatFullTime(backup.created_at),
      onOpen: onOpenBackups,
      tone: "info",
    }) satisfies HomeActionItem);
  const summaryItems: HomeActionItem[] =
    runningJobCount > jobItems.length
      ? [
          {
            detail:
              jobItems.length > 0
                ? `${jobItems.length} active job record${jobItems.length === 1 ? "" : "s"} loaded; open Jobs for full target state.`
                : "Open Jobs for active target state and retained output.",
            id: "running-jobs:fleet-summary",
            label: `${runningJobCount} fleet job${runningJobCount === 1 ? "" : "s"} running`,
            meta: "Fleet summary",
            onOpen: onOpenJobs,
            tone: "info",
          },
        ]
      : [];
  return [...summaryItems, ...jobItems, ...transferItems, ...backupItems]
    .sort((left, right) => (right.metaTitle ?? "").localeCompare(left.metaTitle ?? ""))
    .slice(0, 6);
}

function buildRecentFailureItems({
  backups,
  fileTransfers,
  fleetAlerts,
  jobs,
  onOpenBackups,
  onOpenFleetAlerts,
  onOpenJobDetails,
  onOpenTransfers,
}: {
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  onOpenBackups: () => void;
  onOpenFleetAlerts: () => void;
  onOpenJobDetails: (jobId: string) => void;
  onOpenTransfers: () => void;
}): HomeActionItem[] {
  const jobItems = jobs
    .filter((job) => isFailedJobStatus(job.status))
    .map((job) => ({
      detail: `${readableJobCommand(job.command_type)} / ${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
      id: `failed-job:${job.id}`,
      label: `Job ${shortId(job.id)} ${readableJobStatus(job.status)}`,
      meta: formatCompactTime(job.completed_at ?? job.created_at),
      metaTitle: formatFullTime(job.completed_at ?? job.created_at),
      onOpen: () => onOpenJobDetails(job.id),
      tone: "critical",
    }) satisfies HomeActionItem);
  const transferItems = fileTransfers
    .filter((transfer) => transfer.status === "aborted" || transfer.status === "unknown")
    .map((transfer) => ({
      detail: `${readableTransferDirection(transfer.direction)} ${transfer.path}`,
      id: `failed-transfer:${transfer.client_id}:${transfer.session_id}`,
      label: `Transfer ${shortId(transfer.session_id)} ${readableTransferStatus(transfer.status)}`,
      meta: formatCompactTime(transfer.observed_at),
      metaTitle: formatFullTime(transfer.observed_at),
      onOpen: onOpenTransfers,
      tone: transfer.status === "unknown" ? "warning" : "critical",
    }) satisfies HomeActionItem);
  const backupItems = backups
    .filter((backup) => isFailedBackupStatus(backup.status))
    .map((backup) => ({
      detail: `${backup.client_id} / ${backup.paths.join(", ")}`,
      id: `failed-backup:${backup.id}`,
      label: `Backup ${shortId(backup.id)} ${readableBackupStatus(backup.status)}`,
      meta: formatCompactTime(backup.created_at),
      metaTitle: formatFullTime(backup.created_at),
      onOpen: onOpenBackups,
      tone: "critical",
    }) satisfies HomeActionItem);
  const alertItems = fleetAlerts
    .filter((alert) => alert.operator_state !== "acknowledged")
    .map((alert) => ({
      detail: `${readableAlertCategory(alert.category)} / ${alert.client_id ?? alert.target_id}`,
      id: `failure-alert:${alert.id}`,
      label: alert.title,
      meta: formatCompactTime(alert.observed_at),
      metaTitle: formatFullTime(alert.observed_at),
      onOpen: onOpenFleetAlerts,
      tone: alert.severity === "critical" ? "critical" : "warning",
    }) satisfies HomeActionItem);
  return [...jobItems, ...transferItems, ...backupItems, ...alertItems]
    .sort(compareAttentionItems)
    .slice(0, 6);
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
    label: `${readableJobCommand(job.command_type)} job ${readableJobStatus(job.status)}`,
    meta: `${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
    onOpen: () => onOpenJobDetails(job.id),
    time: job.completed_at ?? job.created_at,
    type: "Job",
  }));
  const backupItems = backups.map((backup) => ({
    id: `backup:${backup.id}`,
    label: `Backup ${readableBackupStatus(backup.status)}`,
    meta: `${backup.client_id} / ${backup.paths.join(", ")}`,
    onOpen: onOpenBackups,
    time: backup.created_at,
    type: "Backup",
  }));
  const transferItems = fileTransfers.map((transfer) => ({
    id: `transfer:${transfer.client_id}:${transfer.session_id}`,
    label: `${readableTransferDirection(transfer.direction)} transfer ${readableTransferStatus(transfer.status)}`,
    meta: transfer.path,
    onOpen: onOpenTransfers,
    time: transfer.observed_at,
    type: "Transfer",
  }));
  const auditItems = auditLogs.map((audit) => ({
    id: `audit:${audit.id}`,
    label: readableAuditAction(audit.action),
    meta: audit.target,
    onOpen: onOpenAudit,
    time: audit.created_at,
    type: "Audit",
  }));
  const scheduleItems = schedules.map((schedule) => ({
    id: `schedule:${schedule.id}`,
    label: `${schedule.name} ${schedule.enabled ? "enabled" : "paused"}`,
    meta: `${readableJobCommand(schedule.command_type)} / ${schedule.selector_expression}`,
    onOpen: onOpenSchedule,
    time: schedule.updated_at,
    type: "Schedule",
  }));
  return [...jobItems, ...backupItems, ...transferItems, ...auditItems, ...scheduleItems]
    .sort((left, right) => new Date(right.time).getTime() - new Date(left.time).getTime())
    .slice(0, 8)
    .map((item) => ({
      ...item,
      timeDateTime: item.time,
      time: formatCompactTime(item.time),
      timeTitle: formatFullTime(item.time),
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

function isActiveBackupStatus(status: string) {
  return ["queued", "running", "uploading", "collecting"].includes(status);
}

function isActiveTransferStatus(status: string) {
  return status === "started" || status === "transferring";
}

function readableJobCommand(commandType: string) {
  if (commandType === "shell_argv") {
    return "Shell command";
  }
  if (commandType === "scheduled_shell_argv") {
    return "Scheduled shell command";
  }
  if (commandType === "shell_pty") {
    return "Interactive shell";
  }
  return commandType
    .replace(/^scheduled_/, "scheduled ")
    .replace(/_/g, " ")
    .replace(/\bospf\b/gi, "OSPF")
    .replace(/\bvps\b/gi, "VPS")
    .replace(/\bapi\b/gi, "API")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}

function readableJobStatus(status: string) {
  const labels: Record<string, string> = {
    agent_lost: "agent lost",
    agent_timeout: "agent timeout",
    completed: "completed",
    control_timeout: "control timeout",
    deadline_expired: "deadline expired",
    dispatching: "dispatching",
    failed: "failed",
    queued: "queued",
    rejected: "rejected",
    running: "running",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function readableBackupStatus(status: string) {
  const labels: Record<string, string> = {
    artifact_metadata_recorded: "artifact recorded; upload not verified",
    completed: "completed",
    execution_canceled: "canceled",
    execution_failed: "failed",
    queued: "queued",
    running: "running",
    uploading: "uploading",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function readableTransferDirection(direction: string) {
  if (direction === "upload") {
    return "Upload";
  }
  if (direction === "download") {
    return "Download";
  }
  return direction.replace(/_/g, " ");
}

function readableTransferStatus(status: string) {
  const labels: Record<string, string> = {
    aborted: "aborted",
    committed: "completed",
    started: "started",
    transferring: "transferring",
    unknown: "status unknown",
  };
  return labels[status] ?? status.replace(/_/g, " ");
}

function readableAlertCategory(category: string) {
  return category
    .replace(/_/g, " ")
    .replace(/\bospf\b/gi, "OSPF")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}

function readableAuditAction(action: string) {
  return action
    .replace(/[._]/g, " ")
    .replace(/\bapi\b/gi, "API")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}
