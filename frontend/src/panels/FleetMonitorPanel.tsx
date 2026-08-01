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
import { useMemo, type ReactNode } from "react";
import { agentDisplayState, type AgentDisplayState } from "../agentDisplayState";
import { ActionFeedback } from "../components/ActionFeedback";
import {
  formatLowerBoundCount,
  isActionableFleetAlertState,
} from "../constants";
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
import { INTERFACE_RATE_DEFINITION } from "../telemetryMetrics";
import { useHistoryEntryState } from "../historyEntryState";
import { displayNameOrUnnamed, formatTime, timestampMillis } from "../utils";

type FleetMonitorPanelProps = {
  agents: AgentView[];
  apiError?: string | null;
  ariaLabel?: string;
  description?: string;
  embedded?: boolean;
  backups?: BackupRequestRecord[];
  failedJobCount?: number;
  fileTransfers?: FileTransferSessionRecord[];
  fleetAlerts?: FleetAlertRecord[];
  jobs?: JobHistoryRecord[];
  maxCards?: number;
  recordBounds: MonitorRecordBounds;
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
type MonitorRecordBounds = {
  backups: boolean;
  fileTransfers: boolean;
  fleetAlerts: boolean;
};

const monitorSortOptions: Array<{ label: string; value: FleetMonitorSort }> = [
  { label: "Warnings first", value: "warning" },
  { label: "Traffic", value: "traffic" },
  { label: "1m load", value: "cpu" },
  { label: "Memory", value: "memory" },
  { label: "Region", value: "region" },
  { label: "Provider", value: "provider" },
];
const NETWORK_SNAPSHOT_COHERENCE_MS = 180_000;

export function FleetMonitorPanel({
  agents,
  apiError = null,
  ariaLabel = "VPS monitor cards",
  description = "VPS health cards for scanning state, resources, network, and alerts before opening terminal or file workflows.",
  embedded = false,
  backups = [],
  failedJobCount,
  fileTransfers = [],
  fleetAlerts = [],
  jobs = [],
  maxCards,
  recordBounds,
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
  const historySlot = embedded ? "home.fleet-monitor" : "fleet.monitor";
  const [density, setDensity] = useHistoryEntryState<FleetMonitorDensity>(
    `${historySlot}.density`,
    "comfortable",
  );
  const [sortMode, setSortMode] = useHistoryEntryState<FleetMonitorSort>(
    `${historySlot}.sort`,
    "warning",
  );
  const rollups = latestRollupsByClient(telemetryRollups);
  const rates = latestRatesByClient(telemetryNetworkRates);
  const tunnels = latestTunnelsByClient(telemetryTunnels);
  const cardSignals = buildCardSignals({
    backups,
    failedJobCount,
    fileTransfers,
    fleetAlerts,
    jobs,
    recordBounds,
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
      <ActionFeedback
        className="localActionFeedback"
        message={apiError}
        tone="danger"
      />

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
  const displayState = agentDisplayState(agent);
  const provider = tagValue(agent.tags, "provider") ?? "provider unset";
  const region = tagValue(agent.tags, "country") ?? tagValue(agent.tags, "region") ?? "region unset";
  const visibleTags = agent.tags.slice(0, density === "compact" ? 2 : 4);
  const hiddenTags = agent.tags.slice(visibleTags.length);
  const hiddenTagCount = Math.max(0, agent.tags.length - visibleTags.length);
  const currentRates = coherentNetworkRates(rates);
  const networkBps =
    currentRates.length > 0
      ? currentRates.reduce((total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg, 0)
      : null;
  const latency = averageLatency(tunnels);
  const memoryUsed = rollup
    ? percent(rollup.memory_total_bytes_max - rollup.memory_available_bytes_avg, rollup.memory_total_bytes_max)
    : null;
  const diskUsed = rollup
    ? percent(rollup.disk_total_bytes_max - rollup.disk_available_bytes_avg, rollup.disk_total_bytes_max)
    : null;
  const resourceFreshness = rollup?.latest_observed_at ?? null;
  const networkFreshness = latestTimestamp(currentRates.map((rate) => rate.bucket_start));
  const tunnelFreshness = latestTimestamp(tunnels.map((tunnel) => tunnel.observed_at));
  const resourceTelemetryState = monitorTelemetryState(displayState, resourceFreshness);
  const networkTelemetryState = monitorTelemetryState(displayState, networkFreshness);
  const tunnelTelemetryState = monitorTelemetryState(displayState, tunnelFreshness);
  const telemetryState = monitorTelemetrySummary(
    resourceTelemetryState,
    networkTelemetryState,
    latestTimestamp([resourceFreshness, networkFreshness]),
  );
  const statusTone =
    (telemetryState.kind === "partial" || telemetryState.kind === "stale") &&
    monitorStatusTone(agent, displayState) === "online"
      ? "warning"
      : monitorStatusTone(agent, displayState);
  const lastContact = agent.last_seen_at ?? agent.stale_since ?? null;

  return (
    <article
      aria-label={`${displayNameOrUnnamed(agent.display_name)} ${displayState.label} monitor card`}
      className={`vpsMonitorCard ${statusTone} ${density}`}
    >
      <button className="vpsMonitorCardMain" onClick={() => onOpenVpsDetail(agent)} type="button">
        <span className="vpsMonitorStatus" title={displayState.detail}>
          <span aria-hidden="true" />
          {density === "compact" && displayState.label === "Contact unknown"
            ? "No contact"
            : displayState.label}
        </span>
        <strong title={displayNameOrUnnamed(agent.display_name)}>
          {displayNameOrUnnamed(agent.display_name)}
        </strong>
        <small title={`${provider} / ${region}`}>{provider} / {region}</small>
      </button>
      <div className="vpsMonitorTags" aria-label={`Tags for ${displayNameOrUnnamed(agent.display_name)}`}>
        {visibleTags.length === 0 ? (
          <span>untagged</span>
        ) : (
          visibleTags.map((tag) => <span key={tag} title={tag}>{tag}</span>)
        )}
        {hiddenTagCount > 0 && (
          <span title={hiddenTags.join(", ")}>+{hiddenTagCount}</span>
        )}
      </div>
      <div className="vpsMonitorMetrics">
        <MonitorMetric
          icon={<Gauge size={15} />}
          label="1m load"
          stale={resourceTelemetryState.kind === "stale"}
          title="Linux 1-minute load average, not CPU utilization percentage"
          value={rollup ? rollup.cpu_load_1_avg.toFixed(2) : "n/a"}
        />
        <MonitorMetric icon={<Activity size={15} />} label="Memory" stale={resourceTelemetryState.kind === "stale"} value={memoryUsed ?? "n/a"} />
        <MonitorMetric icon={<Server size={15} />} label="Disk" stale={resourceTelemetryState.kind === "stale"} value={diskUsed ?? "n/a"} />
        <MonitorMetric
          icon={<Network size={15} />}
          label="Network"
          stale={networkTelemetryState.kind === "stale"}
          title={`${INTERFACE_RATE_DEFINITION} Latest RX plus TX interval-average rates are summed across ${currentRates.length} concurrently reported interface${currentRates.length === 1 ? "" : "s"}; virtual paths can overlap.`}
          value={networkBps === null ? "n/a" : formatRate(networkBps)}
        />
      </div>
      {density === "compact" ? (
        <div className="vpsMonitorEvidence compactSummary">
          <span title={tunnelTelemetryState.title}>
            {latency === null
              ? "Latency n/a"
              : `${latency.toFixed(1)} ms avg${tunnelTelemetryState.kind === "stale" ? " · last-known" : ""}`}
            {" · "}
            {formatMonitorContactEvidence(agent, displayState, lastContact)}
          </span>
          <span title={telemetryState.title}>
            <span className={`telemetryEvidence ${telemetryState.kind}`}>
              {telemetryState.label}
            </span>{" "}
            · {signals.fleetJobText}
          </span>
        </div>
      ) : (
        <div className="vpsMonitorEvidence comfortableSummary">
          <span title={tunnelTelemetryState.title}>
            {latency === null
              ? "Latency n/a"
              : `${latency.toFixed(1)} ms avg${tunnelTelemetryState.kind === "stale" ? " · last-known" : ""}`}
          </span>
          <span>{formatMonitorContactEvidence(agent, displayState, lastContact)}</span>
          <span className={`telemetryEvidence ${telemetryState.kind}`} title={telemetryState.title}>
            {telemetryState.label}
          </span>
          <span>{signals.fleetJobText}</span>
          <span>{agent.stale_reason ?? signals.statusText}</span>
        </div>
      )}
      <div className="vpsMonitorSignals" aria-label={`Operational signals for ${displayNameOrUnnamed(agent.display_name)}`}>
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
        <details className="vpsMonitorMore">
          <summary aria-label={`More actions for ${displayNameOrUnnamed(agent.display_name)}`}>
            <MoreHorizontal size={15} />
            <span>More</span>
          </summary>
          <div className="vpsMonitorMoreMenu">
            <button onClick={() => onOpenProcesses(agent)} title="Open process supervisor workflow" type="button">
              <Activity size={15} />
              <span>Processes</span>
            </button>
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
  stale = false,
  title,
  value,
}: {
  icon: JSX.Element;
  label: string;
  stale?: boolean;
  title?: string;
  value: string;
}) {
  const metricTitle = [title, stale ? "Last-known value; current telemetry is stale" : null]
    .filter(Boolean)
    .join(". ");
  return (
    <span className={`vpsMonitorMetric${stale ? " stale" : ""}`} title={metricTitle || undefined}>
      {icon}
      <span>{label}</span>
      <strong>{value}</strong>
    </span>
  );
}

type MonitorTelemetryState = {
  kind: "fresh" | "missing" | "partial" | "stale";
  label: string;
  title: string;
};

function monitorTelemetryState(
  displayState: AgentDisplayState,
  latestAt: string | null,
): MonitorTelemetryState {
  if (!latestAt) {
    return {
      kind: "missing",
      label: "Telemetry unavailable",
      title: "This VPS has not reported retained resource or network telemetry",
    };
  }
  const latestMs = timestampMillis(latestAt);
  if (!Number.isFinite(latestMs)) {
    return {
      kind: "stale",
      label: "Telemetry time invalid",
      title: "The latest telemetry timestamp is invalid and cannot be treated as current",
    };
  }
  const ageMs = Math.max(0, Date.now() - latestMs);
  const stale = displayState.label !== "Online" || ageMs > 3 * 60_000;
  return {
    kind: stale ? "stale" : "fresh",
    label: `Telemetry ${stale ? "stale" : "current"} · ${formatTime(latestAt)}`,
    title: stale
      ? "Last-known telemetry is retained for diagnosis and is not current state"
      : "Latest telemetry is within the current-state freshness window",
  };
}

function monitorTelemetrySummary(
  resource: MonitorTelemetryState,
  network: MonitorTelemetryState,
  latestAt: string | null,
): MonitorTelemetryState {
  if (resource.kind === "missing" && network.kind === "missing") {
    return {
      kind: "missing",
      label: "Telemetry unavailable",
      title: "Resource and network telemetry have not been reported",
    };
  }
  const latestLabel = latestAt ? ` · ${formatTime(latestAt)}` : "";
  if (resource.kind === "stale" || network.kind === "stale") {
    return {
      kind: "stale",
      label: `Telemetry stale${latestLabel}`,
      title: `Resource: ${resource.title}. Network: ${network.title}`,
    };
  }
  if (resource.kind === "missing" || network.kind === "missing") {
    return {
      kind: "partial",
      label: `Telemetry partial${latestLabel}`,
      title: `Resource: ${resource.title}. Network: ${network.title}`,
    };
  }
  return {
    kind: "fresh",
    label: `Telemetry current${latestLabel}`,
    title: "Latest resource and network telemetry are within the current-state freshness window",
  };
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
  fleetJobText: string;
  statusText: string;
  transferText: string;
  transferTone: "critical" | "warning" | "info" | "ok" | "neutral";
};

type CardSignalContext = {
  global: {
    failedJobs: number;
    recordBounds: MonitorRecordBounds;
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
  recordBounds,
  runningJobCount,
}: {
  backups: BackupRequestRecord[];
  failedJobCount?: number;
  fileTransfers: FileTransferSessionRecord[];
  fleetAlerts: FleetAlertRecord[];
  jobs: JobHistoryRecord[];
  recordBounds: MonitorRecordBounds;
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
        alerts: fleetAlerts.filter(
          (alert) =>
            alert.client_id === clientId &&
            isActionableFleetAlertState(alert.operator_state),
        ),
        backups: backups.filter((backup) => backup.client_id === clientId),
        failedJobs,
        recordBounds,
        runningJobs,
        transfers: fileTransfers.filter((transfer) => transfer.client_id === clientId),
      }),
    );
  }
  return { global: { failedJobs, recordBounds, runningJobs }, records };
}

function defaultCardSignal(global: CardSignalContext["global"]): VpsMonitorCardSignal {
  return buildClientSignal({
    alerts: [],
    backups: [],
    failedJobs: global.failedJobs,
    recordBounds: global.recordBounds,
    runningJobs: global.runningJobs,
    transfers: [],
  });
}

function buildClientSignal({
  alerts,
  backups,
  failedJobs,
  recordBounds,
  runningJobs,
  transfers,
}: {
  alerts: FleetAlertRecord[];
  backups: BackupRequestRecord[];
  failedJobs: number;
  recordBounds: MonitorRecordBounds;
  runningJobs: number;
  transfers: FileTransferSessionRecord[];
}): VpsMonitorCardSignal {
  const criticalAlerts = alerts.filter((alert) => alert.severity === "critical").length;
  const warningAlerts = alerts.filter((alert) => alert.severity === "warning").length;
  const infoAlerts = alerts.length - criticalAlerts - warningAlerts;
  const failedBackups = backups.filter((backup) => isFailedBackupStatus(backup.status)).length;
  const failedTransfers = transfers.filter((transfer) => isFailedTransferStatus(transfer.status)).length;
  const activeTransfers = transfers.filter((transfer) => isActiveTransferStatus(transfer.status)).length;
  const recordPageCapped =
    recordBounds.fleetAlerts ||
    recordBounds.backups ||
    recordBounds.fileTransfers;
  const knownIssue =
    criticalAlerts > 0 ||
    warningAlerts > 0 ||
    infoAlerts > 0 ||
    failedBackups > 0 ||
    failedTransfers > 0;
  const fleetJobText =
    failedJobs > 0
      ? `Fleet-wide jobs: ${failedJobs} failed`
      : runningJobs > 0
        ? `Fleet-wide jobs: ${runningJobs} running`
        : "Fleet-wide jobs: idle";
  return {
    alertText:
      criticalAlerts > 0
        ? `${formatLowerBoundCount(criticalAlerts, recordBounds.fleetAlerts)} critical`
        : warningAlerts > 0
          ? `${formatLowerBoundCount(warningAlerts, recordBounds.fleetAlerts)} warning`
          : infoAlerts > 0
            ? `${formatLowerBoundCount(infoAlerts, recordBounds.fleetAlerts)} info`
            : recordBounds.fleetAlerts
              ? "None in loaded page"
              : "Clear",
    alertTone:
      criticalAlerts > 0
        ? "critical"
        : warningAlerts > 0
          ? "warning"
          : infoAlerts > 0 || recordBounds.fleetAlerts
            ? "info"
            : "neutral",
    backupText:
      failedBackups > 0
        ? `${formatLowerBoundCount(failedBackups, recordBounds.backups)} failed`
        : backups.length > 0
          ? `${formatLowerBoundCount(backups.length, recordBounds.backups)} recorded`
          : recordBounds.backups
            ? "None in loaded page"
            : "No run",
    backupTone:
      failedBackups > 0
        ? "critical"
        : recordBounds.backups
          ? "info"
          : "neutral",
    fleetJobText,
    statusText:
      knownIssue
        ? `${criticalAlerts} critical / ${warningAlerts} warning / ${infoAlerts} info alerts; ${failedBackups} backup failures; ${failedTransfers} transfer failures${recordPageCapped ? "; counts use capped loaded pages" : ""}`
        : recordPageCapped
          ? "No card-local warnings in loaded pages; older records may not be shown"
          : "No card-local alert, backup, or transfer warnings",
    transferText:
      failedTransfers > 0
        ? `${formatLowerBoundCount(failedTransfers, recordBounds.fileTransfers)} failed`
        : activeTransfers > 0
          ? `${formatLowerBoundCount(activeTransfers, recordBounds.fileTransfers)} active`
          : recordBounds.fileTransfers
            ? "No issue loaded"
            : "Clear",
    transferTone:
      failedTransfers > 0
        ? "critical"
        : activeTransfers > 0 || recordBounds.fileTransfers
          ? "info"
          : "neutral",
  };
}

function latestRollupsByClient(records: TelemetryRollupRecord[]) {
  const latest = new Map<string, TelemetryRollupRecord>();
  for (const record of records) {
    const current = latest.get(record.client_id);
    if (
      !current ||
      timestampMillis(record.latest_observed_at) >
        timestampMillis(current.latest_observed_at)
    ) {
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
    if (
      !current ||
      timestampMillis(record.bucket_start) > timestampMillis(current.bucket_start)
    ) {
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
  return coherentNetworkRates(rates).reduce(
    (total, rate) => total + rate.rx_bps_avg + rate.tx_bps_avg,
    0,
  );
}

function coherentNetworkRates(rates: TelemetryNetworkRateRecord[]) {
  const latest = Math.max(
    ...rates.map((rate) => timestampMillis(rate.bucket_start)).filter(Number.isFinite),
  );
  if (!Number.isFinite(latest)) {
    return [];
  }
  return rates.filter(
    (rate) =>
      latest - timestampMillis(rate.bucket_start) <=
      NETWORK_SNAPSHOT_COHERENCE_MS,
  );
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
  const displayState = agentDisplayState(agent);
  if (displayState.label === "Offline") return 3;
  if (displayState.tone === "warning" || agent.stale_since || agent.stale_reason) return 2;
  if (agent.capabilities.privilege_mode === "unknown") return 1;
  return 0;
}

function monitorStatusTone(agent: AgentView, displayState = agentDisplayState(agent)) {
  if (displayState.label === "Online") return "online";
  if (displayState.label === "Stale") return "stale";
  if (displayState.label === "Offline") return "offline";
  if (displayState.tone === "warning" || agent.stale_since || agent.stale_reason) return "warning";
  if (agent.capabilities.privilege_mode === "unknown") return "warning";
  return "offline";
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

function formatMonitorContactEvidence(
  agent: AgentView,
  displayState: AgentDisplayState,
  lastContact: string | null,
) {
  if (displayState.label === "Contact unknown") {
    return "Contact unknown; no gateway timestamp";
  }
  if (lastContact) {
    return `Last contact ${formatTime(lastContact)}`;
  }
  return displayState.detail;
}

function latestTimestamp(values: Array<string | null | undefined>) {
  const latest = values
    .map((value) => (value ? timestampMillis(value) : Number.NaN))
    .filter((value) => Number.isFinite(value))
    .sort((left, right) => right - left)[0];
  return latest === undefined ? null : new Date(latest).toISOString();
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
