import {
  Activity,
  DatabaseBackup,
  FolderOpen,
  Gauge,
  MoreHorizontal,
  Network,
  Server,
  TerminalSquare,
} from "lucide-react";
import { useMemo, useState, type ReactNode } from "react";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import type {
  AgentView,
  BackupRequestRecord,
  FleetAlertRecord,
  JobHistoryRecord,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TelemetryTunnelRecord,
} from "../types";
import { displayNameOrUnnamed, formatTime } from "../utils";

type FleetMonitorPanelProps = {
  agents: AgentView[];
  ariaLabel?: string;
  description?: string;
  embedded?: boolean;
  backups?: BackupRequestRecord[];
  failedJobCount?: number;
  fileTransfers?: FileTransferSessionRecord[];
  fleetAlerts?: FleetAlertRecord[];
  jobs?: JobHistoryRecord[];
  maxCards?: number;
  runningJobCount?: number;
  telemetryNetworkRates: TelemetryNetworkRateRecord[];
  telemetryRollups: TelemetryRollupRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  title?: string;
  toolbarAction?: ReactNode;
  onOpenBackup: (agent: AgentView) => void;
  onOpenFiles: (agent: AgentView) => void;
  onOpenNetwork: (agent: AgentView) => void;
  onOpenProcesses: (agent: AgentView) => void;
  onOpenTerminal: (agent: AgentView) => void;
  onOpenVpsDetail: (agent: AgentView) => void;
};

export type FleetMonitorDensity = "compact" | "comfortable";
type FleetMonitorSort = "warning" | "traffic" | "cpu" | "memory" | "region" | "provider";

const monitorSortOptions: Array<{ label: string; value: FleetMonitorSort }> = [
  { label: "Warnings first", value: "warning" },
  { label: "Traffic", value: "traffic" },
  { label: "CPU", value: "cpu" },
  { label: "Memory", value: "memory" },
  { label: "Region", value: "region" },
  { label: "Provider", value: "provider" },
];

export function FleetMonitorPanel({
  agents,
  ariaLabel = "VPS monitor cards",
  description = "Komari-style cards for fast VPS scanning; destructive operations stay in reviewed workflows.",
  embedded = false,
  backups = [],
  failedJobCount,
  fileTransfers = [],
  fleetAlerts = [],
  jobs = [],
  maxCards,
  runningJobCount,
  telemetryNetworkRates,
  telemetryRollups,
  telemetryTunnels,
  title = "Fleet monitor",
  toolbarAction,
  onOpenBackup,
  onOpenFiles,
  onOpenNetwork,
  onOpenProcesses,
  onOpenTerminal,
  onOpenVpsDetail,
}: FleetMonitorPanelProps) {
  const [density, setDensity] = useState<FleetMonitorDensity>("comfortable");
  const [sortMode, setSortMode] = useState<FleetMonitorSort>("warning");
  const rollups = latestRollupsByClient(telemetryRollups);
  const rates = latestRatesByClient(telemetryNetworkRates);
  const tunnels = latestTunnelsByClient(telemetryTunnels);
  const cardSignals = buildCardSignals({
    backups,
    failedJobCount,
    fileTransfers,
    fleetAlerts,
    jobs,
    runningJobCount,
  });
  const sortedAgents = useMemo(
    () =>
      [...agents].sort(
        compareMonitorAgents({
          mode: sortMode,
          rates,
          rollups,
          signals: cardSignals,
        }),
      ),
    [agents, cardSignals, rates, rollups, sortMode],
  );
  const visibleAgents = typeof maxCards === "number" ? sortedAgents.slice(0, maxCards) : sortedAgents;
  const hiddenCount = Math.max(0, sortedAgents.length - visibleAgents.length);
  const rootClassName = embedded
    ? "fleetMonitorWorkspace embedded"
    : "workspace singleColumn fleetMonitorWorkspace";

  return (
    <section className={rootClassName}>
      <div className="fleetMonitorToolbar">
        <div>
          <h2>{title}</h2>
          <span>{description}</span>
        </div>
        <div className="fleetMonitorToolbarRight">
          <div className="fleetMonitorControls" aria-label={`${title} controls`}>
            <label>
              <span>Sort</span>
              <select
                aria-label={`${title} sort`}
                onChange={(event) =>
                  setSortMode(event.target.value as FleetMonitorSort)
                }
                value={sortMode}
              >
                {monitorSortOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <div
              aria-label={`${title} density`}
              className="segmented vpsMonitorDensityControl"
              role="group"
            >
              {(["compact", "comfortable"] as const).map((option) => (
                <button
                  aria-pressed={density === option}
                  className={density === option ? "selected" : ""}
                  key={option}
                  onClick={() => setDensity(option)}
                  type="button"
                >
                  {option === "compact" ? "Compact" : "Comfortable"}
                </button>
              ))}
            </div>
          </div>
          <div className="fleetMonitorSummary" aria-label={`${title} summary`}>
            <strong>{agents.length}</strong>
            <span>{hiddenCount > 0 ? `${hiddenCount} more hidden` : "visible VPSs"}</span>
          </div>
          {toolbarAction}
        </div>
      </div>

      {sortedAgents.length === 0 ? (
        <div className="emptyState">
          <Server size={22} />
          <strong>No VPS cards to show</strong>
          <span>Adjust the fleet scope or wait for agents to report telemetry.</span>
        </div>
      ) : (
        <div
          className={`vpsMonitorGrid ${density}`}
          aria-label={ariaLabel}
          data-density={density}
          data-sort={sortMode}
        >
          {visibleAgents.map((agent) => (
            <VpsMonitorCard
              agent={agent}
              density={density}
              key={agent.id}
              onOpenBackup={onOpenBackup}
              onOpenFiles={onOpenFiles}
              onOpenNetwork={onOpenNetwork}
              onOpenProcesses={onOpenProcesses}
              onOpenTerminal={onOpenTerminal}
              onOpenVpsDetail={onOpenVpsDetail}
              rates={rates.get(agent.id) ?? []}
              rollup={rollups.get(agent.id) ?? null}
              signals={cardSignals.records.get(agent.id) ?? defaultCardSignal(cardSignals.global)}
              tunnels={tunnels.get(agent.id) ?? []}
            />
          ))}
        </div>
      )}
    </section>
  );
}

export type VpsMonitorCardProps = {
  agent: AgentView;
  density: FleetMonitorDensity;
  onOpenBackup: (agent: AgentView) => void;
  onOpenFiles: (agent: AgentView) => void;
  onOpenNetwork: (agent: AgentView) => void;
  onOpenProcesses: (agent: AgentView) => void;
  onOpenTerminal: (agent: AgentView) => void;
  onOpenVpsDetail: (agent: AgentView) => void;
  rates: TelemetryNetworkRateRecord[];
  rollup: TelemetryRollupRecord | null;
  signals: VpsMonitorCardSignal;
  tunnels: TelemetryTunnelRecord[];
};

export function VpsMonitorCard({
  agent,
  density,
  onOpenBackup,
  onOpenFiles,
  onOpenNetwork,
  onOpenProcesses,
  onOpenTerminal,
  onOpenVpsDetail,
  rates,
  rollup,
  signals,
  tunnels,
}: VpsMonitorCardProps) {
  const statusTone = monitorStatusTone(agent);
  const provider = tagValue(agent.tags, "provider") ?? "provider unset";
  const region = tagValue(agent.tags, "country") ?? tagValue(agent.tags, "region") ?? "region unset";
  const visibleTags = agent.tags.slice(0, density === "compact" ? 2 : 4);
  const hiddenTagCount = Math.max(0, agent.tags.length - visibleTags.length);
  const networkBps = rates.reduce((total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg, 0);
  const latency = averageLatency(tunnels);
  const memoryUsed = rollup
    ? percent(rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg, rollup.memory_total_bytes_max)
    : null;
  const diskUsed = rollup
    ? percent(rollup.disk_total_bytes_max - rollup.disk_available_bytes_avg, rollup.disk_total_bytes_max)
    : null;
  const freshness = rollup?.latest_observed_at ?? agent.last_seen_at ?? agent.stale_since ?? null;

  return (
    <article
      aria-label={`${displayNameOrUnnamed(agent.display_name)} ${agent.status} monitor card`}
      className={`vpsMonitorCard ${statusTone} ${density}`}
    >
      <button className="vpsMonitorCardMain" onClick={() => onOpenVpsDetail(agent)} type="button">
        <span className="vpsMonitorStatus">
          <span aria-hidden="true" />
          {agent.status}
        </span>
        <strong>{displayNameOrUnnamed(agent.display_name)}</strong>
        <small>{provider} / {region}</small>
      </button>
      <div className="vpsMonitorTags" aria-label={`Tags for ${displayNameOrUnnamed(agent.display_name)}`}>
        {visibleTags.length === 0 ? (
          <span>untagged</span>
        ) : (
          visibleTags.map((tag) => <span key={tag}>{tag}</span>)
        )}
        {hiddenTagCount > 0 && <span>+{hiddenTagCount}</span>}
      </div>
      <div className="vpsMonitorMetrics">
        <MonitorMetric icon={<Gauge size={15} />} label="CPU" value={rollup ? rollup.cpu_load_1_avg.toFixed(2) : "n/a"} />
        <MonitorMetric icon={<Activity size={15} />} label="Memory" value={memoryUsed ?? "n/a"} />
        <MonitorMetric icon={<Server size={15} />} label="Disk" value={diskUsed ?? "n/a"} />
        <MonitorMetric icon={<Network size={15} />} label="Network" value={formatRate(networkBps)} />
      </div>
      <div className="vpsMonitorEvidence">
        <span>{latency === null ? "Latency n/a" : `${latency.toFixed(1)} ms avg`}</span>
        <span>{freshness ? `Telemetry ${formatTime(freshness)}` : "Telemetry not reported"}</span>
        <span>{agent.stale_reason ?? signals.statusText}</span>
      </div>
      <div className="vpsMonitorSignals" aria-label={`Operational signals for ${displayNameOrUnnamed(agent.display_name)}`}>
        <MonitorSignal tone={signals.jobTone} label="Jobs" value={signals.jobText} />
        <MonitorSignal tone={signals.alertTone} label="Alerts" value={signals.alertText} />
        <MonitorSignal tone={signals.backupTone} label="Backup" value={signals.backupText} />
        <MonitorSignal tone={signals.transferTone} label="Transfer" value={signals.transferText} />
      </div>
      <div className="vpsMonitorActions" aria-label={`Quick actions for ${displayNameOrUnnamed(agent.display_name)}`}>
        <button onClick={() => onOpenTerminal(agent)} title="Open terminal workflow" type="button">
          <TerminalSquare size={15} />
          <span>Terminal</span>
        </button>
        <button onClick={() => onOpenFiles(agent)} title="Open file browser workflow" type="button">
          <FolderOpen size={15} />
          <span>Files</span>
        </button>
        <button onClick={() => onOpenProcesses(agent)} title="Open process supervisor workflow" type="button">
          <Activity size={15} />
          <span>Processes</span>
        </button>
        <details className="vpsMonitorMore">
          <summary aria-label={`More actions for ${displayNameOrUnnamed(agent.display_name)}`}>
            <MoreHorizontal size={15} />
            <span>More</span>
          </summary>
          <div className="vpsMonitorMoreMenu">
            <button onClick={() => onOpenBackup(agent)} title="Open backup workflow" type="button">
              <DatabaseBackup size={15} />
              <span>Backup</span>
            </button>
            <button onClick={() => onOpenNetwork(agent)} title="Open network workflow" type="button">
              <Network size={15} />
              <span>Network</span>
            </button>
            <button onClick={() => onOpenVpsDetail(agent)} title="Open instance detail" type="button">
              <Server size={15} />
              <span>Detail</span>
            </button>
          </div>
        </details>
      </div>
    </article>
  );
}

function MonitorMetric({
  icon,
  label,
  value,
}: {
  icon: JSX.Element;
  label: string;
  value: string;
}) {
  return (
    <span className="vpsMonitorMetric">
      {icon}
      <span>{label}</span>
      <strong>{value}</strong>
    </span>
  );
}

function MonitorSignal({
  label,
  tone,
  value,
}: {
  label: string;
  tone: "critical" | "warning" | "info" | "ok" | "neutral";
  value: string;
}) {
  return (
    <span className={`vpsMonitorSignal ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </span>
  );
}

export type VpsMonitorCardSignal = {
  alertText: string;
  alertTone: "critical" | "warning" | "info" | "ok" | "neutral";
  backupText: string;
  backupTone: "critical" | "warning" | "info" | "ok" | "neutral";
  jobText: string;
  jobTone: "critical" | "warning" | "info" | "ok" | "neutral";
  statusText: string;
  transferText: string;
  transferTone: "critical" | "warning" | "info" | "ok" | "neutral";
};

type CardSignalContext = {
  global: {
    failedJobs: number;
    runningJobs: number;
  };
  records: Map<string, VpsMonitorCardSignal>;
};

function buildCardSignals({
  backups,
  failedJobCount,
  fileTransfers,
  fleetAlerts,
  jobs,
  runningJobCount,
}: {
  backups: BackupRequestRecord[];
  failedJobCount?: number;
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  runningJobCount?: number;
}): CardSignalContext {
  const runningJobs = runningJobCount ?? jobs.filter((job) => isActiveJobStatus(job.status)).length;
  const failedJobs = failedJobCount ?? jobs.filter((job) => isFailedJobStatus(job.status)).length;
  const clientIds = new Set<string>([
    ...backups.map((record) => record.client_id),
    ...fileTransfers.map((record) => record.client_id),
    ...fleetAlerts.flatMap((record) => (record.client_id ? [record.client_id] : [])),
  ]);
  const records = new Map<string, VpsMonitorCardSignal>();
  for (const clientId of clientIds) {
    records.set(
      clientId,
      buildClientSignal({
        alerts: fleetAlerts.filter((alert) => alert.client_id === clientId && alert.operator_state !== "acknowledged"),
        backups: backups.filter((backup) => backup.client_id === clientId),
        failedJobs,
        runningJobs,
        transfers: fileTransfers.filter((transfer) => transfer.client_id === clientId),
      }),
    );
  }
  return { global: { failedJobs, runningJobs }, records };
}

function defaultCardSignal(global: CardSignalContext["global"]): VpsMonitorCardSignal {
  return buildClientSignal({
    alerts: [],
    backups: [],
    failedJobs: global.failedJobs,
    runningJobs: global.runningJobs,
    transfers: [],
  });
}

function buildClientSignal({
  alerts,
  backups,
  failedJobs,
  runningJobs,
  transfers,
}: {
  alerts: FleetAlertRecord[];
  backups: BackupRequestRecord[];
  failedJobs: number;
  runningJobs: number;
  transfers: FileTransferSessionRecord[];
}): VpsMonitorCardSignal {
  const criticalAlerts = alerts.filter((alert) => alert.severity === "critical").length;
  const warningAlerts = alerts.length - criticalAlerts;
  const failedBackups = backups.filter((backup) => isFailedBackupStatus(backup.status)).length;
  const failedTransfers = transfers.filter((transfer) => isFailedTransferStatus(transfer.status)).length;
  const activeTransfers = transfers.filter((transfer) => isActiveTransferStatus(transfer.status)).length;
  return {
    alertText:
      criticalAlerts > 0 ? `${criticalAlerts} critical` : warningAlerts > 0 ? `${warningAlerts} warning` : "Clear",
    alertTone: criticalAlerts > 0 ? "critical" : warningAlerts > 0 ? "warning" : "ok",
    backupText: failedBackups > 0 ? `${failedBackups} failed` : backups.length > 0 ? `${backups.length} recorded` : "No run",
    backupTone: failedBackups > 0 ? "critical" : backups.length > 0 ? "ok" : "neutral",
    jobText: failedJobs > 0 ? `${failedJobs} failed` : runningJobs > 0 ? `${runningJobs} running` : "Idle",
    jobTone: failedJobs > 0 ? "critical" : runningJobs > 0 ? "info" : "ok",
    statusText:
      criticalAlerts > 0 || warningAlerts > 0
        ? `${criticalAlerts} critical / ${warningAlerts} warning signals`
        : "No card-local warnings",
    transferText: failedTransfers > 0 ? `${failedTransfers} failed` : activeTransfers > 0 ? `${activeTransfers} active` : "Clear",
    transferTone: failedTransfers > 0 ? "critical" : activeTransfers > 0 ? "info" : "ok",
  };
}

function latestRollupsByClient(records: TelemetryRollupRecord[]) {
  const latest = new Map<string, TelemetryRollupRecord>();
  for (const record of records) {
    const current = latest.get(record.client_id);
    if (!current || record.latest_observed_at > current.latest_observed_at) {
      latest.set(record.client_id, record);
    }
  }
  return latest;
}

function latestRatesByClient(records: TelemetryNetworkRateRecord[]) {
  const latest = new Map<string, Map<string, TelemetryNetworkRateRecord>>();
  for (const record of records) {
    const byInterface = latest.get(record.client_id) ?? new Map<string, TelemetryNetworkRateRecord>();
    const current = byInterface.get(record.interface);
    if (!current || record.bucket_start > current.bucket_start) {
      byInterface.set(record.interface, record);
    }
    latest.set(record.client_id, byInterface);
  }
  return new Map(
    Array.from(latest.entries()).map(([clientId, byInterface]) => [
      clientId,
      Array.from(byInterface.values()),
    ]),
  );
}

function latestTunnelsByClient(records: TelemetryTunnelRecord[]) {
  const grouped = new Map<string, TelemetryTunnelRecord[]>();
  for (const record of records) {
    grouped.set(record.client_id, [...(grouped.get(record.client_id) ?? []), record]);
  }
  return grouped;
}

function compareMonitorAgents({
  mode,
  rates,
  rollups,
  signals,
}: {
  mode: FleetMonitorSort;
  rates: Map<string, TelemetryNetworkRateRecord[]>;
  rollups: Map<string, TelemetryRollupRecord>;
  signals: CardSignalContext;
}) {
  return (left: AgentView, right: AgentView) => {
    if (mode === "provider") {
      return (
        providerSortValue(left).localeCompare(providerSortValue(right)) ||
        regionSortValue(left).localeCompare(regionSortValue(right)) ||
        displayNameOrUnnamed(left.display_name).localeCompare(displayNameOrUnnamed(right.display_name))
      );
    }
    if (mode === "region") {
      return (
        regionSortValue(left).localeCompare(regionSortValue(right)) ||
        providerSortValue(left).localeCompare(providerSortValue(right)) ||
        displayNameOrUnnamed(left.display_name).localeCompare(displayNameOrUnnamed(right.display_name))
      );
    }
    const warningDelta =
      monitorWarningRank(right, signals) - monitorWarningRank(left, signals);
    if (mode === "warning" && warningDelta !== 0) return warningDelta;
    const leftTraffic = networkRateTotal(rates.get(left.id) ?? []);
    const rightTraffic = networkRateTotal(rates.get(right.id) ?? []);
    if (mode === "traffic" && rightTraffic !== leftTraffic) return rightTraffic - leftTraffic;
    const leftRollup = rollups.get(left.id);
    const rightRollup = rollups.get(right.id);
    const leftCpu = leftRollup?.cpu_load_1_avg ?? -1;
    const rightCpu = rightRollup?.cpu_load_1_avg ?? -1;
    if (mode === "cpu" && rightCpu !== leftCpu) return rightCpu - leftCpu;
    const leftMemory = memoryUsedRatio(leftRollup);
    const rightMemory = memoryUsedRatio(rightRollup);
    if (mode === "memory" && rightMemory !== leftMemory) return rightMemory - leftMemory;
    const statusDelta = monitorStatusRank(right) - monitorStatusRank(left);
    if (statusDelta !== 0) return statusDelta;
    if (warningDelta !== 0) return warningDelta;
    if (rightTraffic !== leftTraffic) return rightTraffic - leftTraffic;
    if (rightCpu !== leftCpu) return rightCpu - leftCpu;
    return displayNameOrUnnamed(left.display_name).localeCompare(displayNameOrUnnamed(right.display_name));
  };
}

function networkRateTotal(rates: TelemetryNetworkRateRecord[]) {
  return rates.reduce((total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg, 0);
}

function memoryUsedRatio(rollup: TelemetryRollupRecord | undefined) {
  if (!rollup || rollup.memory_total_bytes_max <= 0) {
    return -1;
  }
  return (
    (rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg) /
    rollup.memory_total_bytes_max
  );
}

function providerSortValue(agent: AgentView) {
  return tagValue(agent.tags, "provider") ?? "provider unset";
}

function regionSortValue(agent: AgentView) {
  return tagValue(agent.tags, "country") ?? tagValue(agent.tags, "region") ?? "region unset";
}

function monitorWarningRank(agent: AgentView, signals: CardSignalContext) {
  const localSignals = signals.records.get(agent.id) ?? defaultCardSignal(signals.global);
  return (
    monitorStatusRank(agent) * 10 +
    signalToneRank(localSignals.alertTone) +
    signalToneRank(localSignals.jobTone) +
    signalToneRank(localSignals.backupTone) +
    signalToneRank(localSignals.transferTone)
  );
}

function signalToneRank(tone: VpsMonitorCardSignal["alertTone"]) {
  if (tone === "critical") return 4;
  if (tone === "warning") return 3;
  if (tone === "info") return 2;
  if (tone === "neutral") return 1;
  return 0;
}

function monitorStatusRank(agent: AgentView) {
  if (agent.status !== "online") return 3;
  if (agent.stale_since || agent.stale_reason) return 2;
  if (agent.capabilities.privilege_mode === "unknown") return 1;
  return 0;
}

function monitorStatusTone(agent: AgentView) {
  if (agent.status !== "online") return "offline";
  if (agent.stale_since || agent.stale_reason) return "stale";
  if (agent.capabilities.privilege_mode === "unknown") return "warning";
  return "online";
}

function tagValue(tags: string[], key: string) {
  const prefix = `${key}:`;
  return tags.find((tag) => tag.toLowerCase().startsWith(prefix))?.slice(prefix.length) ?? null;
}

function percent(used: number, total: number) {
  if (!Number.isFinite(used) || !Number.isFinite(total) || total <= 0) {
    return null;
  }
  return `${Math.max(0, Math.min(100, Math.round((used / total) * 100)))}%`;
}

function averageLatency(tunnels: TelemetryTunnelRecord[]) {
  const values = tunnels
    .map((tunnel) => tunnel.latency_avg_ms)
    .filter((value): value is number => typeof value === "number" && Number.isFinite(value));
  if (values.length === 0) {
    return null;
  }
  return values.reduce((total, value) => total + value, 0) / values.length;
}

function formatRate(value: number) {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 bps";
  }
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)} Gbps`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} Mbps`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} Kbps`;
  return `${Math.round(value)} bps`;
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

function isFailedTransferStatus(status: string) {
  return status === "aborted" || status === "unknown";
}
