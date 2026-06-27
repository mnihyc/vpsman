import { Activity, RefreshCw } from "lucide-react";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../../components/TimeSeriesChart";
import { dashboardChartColors } from "../../colorPalette";
import type {
  DashboardLabelClusterRecord,
  DashboardOverviewRecord,
  DashboardPreferences,
  DashboardResourceMetric,
  DashboardResourceSeriesRecord,
  DashboardWindow,
} from "../../types";
import { formatCompactTime } from "../../utils";

type FleetMetricsPanelProps = {
  error: string | null;
  loading: boolean;
  onPreferencesChange: (patch: Partial<DashboardPreferences>) => void;
  onRefresh: () => void;
  onWindowChange: (window: DashboardWindow) => void;
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  window: DashboardWindow;
};

const fallbackDashboardWindows: DashboardWindow[] = ["15m", "1h", "6h", "24h", "7d", "14d", "30d", "all"];
const resourceMetricOptions: Array<{ label: string; value: DashboardResourceMetric }> = [
  { label: "CPU", value: "cpu_load" },
  { label: "Memory", value: "memory_used" },
  { label: "Disk", value: "disk_free" },
];

type ResourceChartData = {
  lines: TimeSeriesChartLine[];
  observedPoints: number;
  times: string[];
};

type ResourceEvidence = {
  dataAvailableValue: string;
  isSparse: boolean;
  lastSampleValue: string;
  sampleSpanLabel: string;
  selectedRangeLabel: string;
  sparseNotice: string | null;
};

export function FleetMetricsPanel({
  error,
  loading,
  onPreferencesChange,
  onRefresh,
  onWindowChange,
  overview,
  preferences,
  window,
}: FleetMetricsPanelProps) {
  const resourceCurve = overview?.resource_curve ?? null;
  const resourceChart = resourceChartData(resourceCurve?.series ?? []);
  const sampledClients = resourceCurve?.sampled_clients ?? overview?.resources.sampled_clients ?? 0;
  const resourceEvidence = buildResourceEvidence(
    overview,
    window,
    resourceChart,
    sampledClients,
  );
  const windowOptions = overview?.available_filters.windows.map((option) => option.value) ?? fallbackDashboardWindows;
  const groupOptions = overview?.available_filters.group_by_options ?? [];
  const excludedClients = resourceCurve?.excluded_clients ?? 0;
  const generatedAt = overview?.generated_at ? formatCompactTime(overview.generated_at) : "No refresh evidence";
  const timeRange = overview
    ? `${formatCompactTime(overview.time_range.start_at)} - ${formatCompactTime(overview.time_range.end_at)}`
    : "Waiting for metrics";
  const selectedGroupLabel = groupOptions.find((option) => option.value === preferences.groupBy)?.label ?? preferences.groupBy;

  return (
    <section className="workspace singleColumn observabilityMetricsWorkspace">
      <div className="fleetPanel observabilityMetricsPanel">
        <div className="sectionHeader">
          <div>
            <h2>Fleet metrics</h2>
            <span>Read-only CPU, memory, disk, and fleet grouping analysis from retained telemetry.</span>
          </div>
          <div className="sectionActions">
            <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
              <RefreshCw size={14} />
              Refresh
            </button>
          </div>
        </div>

        {error && (
          <div className="panelError observabilityMetricsError" role="alert">
            {error}
          </div>
        )}

        <div className="observabilityMetricsControls" aria-label="Fleet metrics controls">
          <div className="timeRangeTabs" aria-label="Fleet metrics time range">
            {windowOptions.map((option) => (
              <button
                aria-pressed={window === option}
                className={window === option ? "active" : ""}
                key={option}
                onClick={() => onWindowChange(option)}
                type="button"
              >
                {windowLabel(option)}
              </button>
            ))}
          </div>
          <div className="dashboardSectionTools" aria-label="Fleet resource metric">
            {resourceMetricOptions.map((option) => (
              <button
                aria-pressed={preferences.resourceMetric === option.value}
                className={preferences.resourceMetric === option.value ? "active" : ""}
                key={option.value}
                onClick={() => onPreferencesChange({ resourceMetric: option.value })}
                type="button"
              >
                {option.label}
              </button>
            ))}
          </div>
          <label className="dashboardToolbarSelect">
            <span>Group by</span>
            <select
              aria-label="Fleet metrics group by"
              onChange={(event) =>
                onPreferencesChange({
                  groupBy: event.target.value as DashboardPreferences["groupBy"],
                })
              }
              value={preferences.groupBy}
            >
              {!groupOptions.length && (
                <option value={preferences.groupBy}>
                  {preferences.groupBy}
                </option>
              )}
              {groupOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
        </div>

        <div className="metricGrid observabilityMetricsSummary" aria-label="Fleet metrics summary">
          <MetricTile label="Current metric" value={resourceMetricTitle(preferences.resourceMetric)} detail={`${sampledClients} VPS sampled`} />
          <MetricTile label="Selected range" value={windowLabel(window)} detail={resourceEvidence.selectedRangeLabel || timeRange} />
          <MetricTile
            label="Telemetry freshness"
            value={loading ? "Refreshing" : resourceEvidence.lastSampleValue}
            detail={`Data available ${resourceEvidence.dataAvailableValue}; overview ${generatedAt}`}
          />
          <MetricTile
            label="Grouping"
            value={selectedGroupLabel}
            detail={`${overview?.label_clusters.length ?? 0} groups; ${excludedClients} excluded; fleet warning state ${overview?.summary.warnings ?? 0}`}
          />
        </div>

        <WarningDefinitionStrip overview={overview} />

        <section className="dashboardSection observabilityChartSection" aria-labelledby="observability-fleet-resource-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-fleet-resource-title">{resourceMetricTitle(preferences.resourceMetric)} by VPS</h2>
              <span>
                Live scan cards stay in Home and Fleet / Monitor; this page analyzes retained telemetry without mutation controls.
              </span>
            </div>
          </div>
          <div className="dashboardNetworkPanel resourceCurvePanel">
            <div className="dashboardCurveCard">
              <div className="dashboardChartHeader">
                <span>{resourceMetricTitle(preferences.resourceMetric)} trend</span>
                <small>{sampledClients} sampled VPS</small>
              </div>
              <p className="observabilityRangeLine">
                Selected: {windowLabel(window)} · Data available: {resourceEvidence.dataAvailableValue} · Last sample: {resourceEvidence.lastSampleValue}
              </p>
              <div className="observabilityChartEvidence" aria-label="Fleet resource freshness">
                <div>
                  <span>Selected window</span>
                  <strong>{windowLabel(window)}</strong>
                  <small>{resourceEvidence.selectedRangeLabel}</small>
                </div>
                <div className={resourceEvidence.isSparse ? "warning" : undefined}>
                  <span>Data available</span>
                  <strong>{resourceEvidence.dataAvailableValue}</strong>
                  <small>{resourceEvidence.sampleSpanLabel}</small>
                </div>
                <div className={resourceEvidence.isSparse ? "warning" : undefined}>
                  <span>Chart treatment</span>
                  <strong>{resourceEvidence.isSparse ? "Points only" : "Trend line"}</strong>
                  <small>{resourceEvidence.isSparse ? "Sparse evidence; no trend implied" : "Enough samples for line reading"}</small>
                </div>
              </div>
              {resourceEvidence.sparseNotice && (
                <p className="observabilitySparseNotice">{resourceEvidence.sparseNotice}</p>
              )}
              <TimeSeriesChart
                ariaLabel="Fleet resource usage curve"
                emptyLabel="No resource telemetry after current filters and exclusions"
                lines={resourceChart.lines}
                pointsOnly={resourceEvidence.isSparse}
                times={resourceChart.times}
                valueFormatter={(value) => formatResourceValue(preferences.resourceMetric, value)}
              />
            </div>
            <div className="dashboardTopClients" aria-label="Top resource VPS list">
              <div className="dashboardSideRailHeader">
                <strong>Top VPS</strong>
                <span>{resourceCurve?.series.length ?? 0} shown</span>
              </div>
              {(resourceCurve?.series ?? []).map((series) => (
                <div className="dashboardClientRow staticRow" key={series.client_id}>
                  <span>
                    <strong>{series.label}</strong>
                    <small>
                      {resourcePeakLabel(preferences.resourceMetric)} {formatResourceValue(preferences.resourceMetric, series.peak)}
                    </small>
                  </span>
                  <b>{formatResourceValue(preferences.resourceMetric, series.current)}</b>
                </div>
              ))}
              {!resourceCurve?.series.length && (
                <div className="emptyState compactEmpty">
                  <Activity size={18} />
                  <strong>No resource rows</strong>
                  <span>Telemetry is unavailable for the current scope and time range.</span>
                </div>
              )}
            </div>
          </div>
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-fleet-groups-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-fleet-groups-title">Fleet grouping</h2>
              <span>Provider, tag, country, status, and client group summaries for scan/comparison work.</span>
            </div>
          </div>
          <div className="observabilityGroupGrid" aria-label="Fleet metrics group breakdown">
            {(overview?.label_clusters ?? []).map((cluster) => (
              <GroupTile cluster={cluster} key={`${cluster.kind}:${cluster.label}`} />
            ))}
            {!overview?.label_clusters.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No groups</strong>
                <span>Group summaries are unavailable until dashboard telemetry loads.</span>
              </div>
            )}
          </div>
        </section>
      </div>
    </section>
  );
}

function MetricTile({ detail, label, value }: { detail: string; label: string; value: string }) {
  return (
    <div className="metricCard">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function GroupTile({ cluster }: { cluster: DashboardLabelClusterRecord }) {
  return (
    <div className="observabilityGroupTile">
      <span>{cluster.kind}</span>
      <strong>{cluster.label}</strong>
      <small>
        {cluster.online}/{cluster.total} online, {cluster.warnings} warning observations, {formatBitsPerSecond(cluster.rx_bps + cluster.tx_bps)} retained traffic
      </small>
    </div>
  );
}

function WarningDefinitionStrip({ overview }: { overview: DashboardOverviewRecord | null }) {
  const activeAlerts = overview?.operations.active_alerts ?? 0;
  const warningAlerts = overview?.operations.warning_alerts ?? 0;
  const affectedVpsCount = uniqueAffectedVpsCount(overview);
  const warningObservations =
    overview?.label_clusters.reduce((total, cluster) => total + cluster.warnings, 0) ?? 0;
  const fleetWarningState = overview?.summary.warnings ?? 0;
  const definitions = [
    {
      detail: `${overview?.operations.critical_alerts ?? 0} critical, ${warningAlerts} warning`,
      label: "Active alerts",
      value: String(activeAlerts),
    },
    {
      detail: "unique VPSs named by recent alerts or degraded-agent evidence",
      label: "Affected VPSs",
      value: String(affectedVpsCount),
    },
    {
      detail: "group rows can overlap across provider, country, tag, and all-fleet buckets",
      label: "Warning observations",
      value: String(warningObservations),
    },
    {
      detail: "same scoped warning count used by the shell fleet status badge",
      label: "Fleet warning state",
      value: String(fleetWarningState),
    },
  ];

  return (
    <div className="observabilityWarningDefinitions" aria-label="Fleet metrics warning definitions">
      {definitions.map((definition) => (
        <div key={definition.label}>
          <span>{definition.label}</span>
          <strong>{definition.value}</strong>
          <small>{definition.detail}</small>
        </div>
      ))}
    </div>
  );
}

function resourceChartData(series: DashboardResourceSeriesRecord[]): ResourceChartData {
  const times = sortedUniqueTimes(series.flatMap((entry) => entry.points.map((point) => point.bucket_start)));
  const lines = series.map((entry, index) => ({
    color: dashboardChartColors[index % dashboardChartColors.length],
    label: entry.label,
    values: times.map((time) => entry.points.find((point) => point.bucket_start === time)?.value ?? null),
  }));
  return {
    lines,
    observedPoints: lines.reduce(
      (total, line) =>
        total + line.values.filter((value) => Number.isFinite(value)).length,
      0,
    ),
    times,
  };
}

function sortedUniqueTimes(times: string[]): string[] {
  return Array.from(new Set(times)).sort((left, right) => Date.parse(left) - Date.parse(right));
}

function formatResourceValue(metric: DashboardResourceMetric, value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return "No data";
  }
  return metric === "cpu_load" ? formatLoad(value) : formatPercent(value);
}

function formatLoad(value: number): string {
  return value.toFixed(value >= 10 ? 0 : 2);
}

function formatPercent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

function resourceMetricTitle(metric: DashboardResourceMetric): string {
  switch (metric) {
    case "cpu_load":
      return "CPU load";
    case "memory_used":
      return "Memory used";
    case "disk_free":
      return "Disk free";
  }
}

function resourcePeakLabel(metric: DashboardResourceMetric): string {
  return metric === "disk_free" ? "Lowest" : "Peak";
}

function buildResourceEvidence(
  overview: DashboardOverviewRecord | null,
  window: DashboardWindow,
  chart: ResourceChartData,
  sampledClients: number,
): ResourceEvidence {
  const firstSample = chart.times[0] ?? null;
  const lastSample = chart.times[chart.times.length - 1] ?? null;
  const selectedStartMs = overview ? Date.parse(overview.time_range.start_at) : NaN;
  const selectedEndMs = overview ? Date.parse(overview.time_range.end_at) : NaN;
  const selectedDurationMs =
    Number.isFinite(selectedStartMs) && Number.isFinite(selectedEndMs)
      ? Math.max(0, selectedEndMs - selectedStartMs)
      : 0;
  const firstSampleMs = firstSample ? Date.parse(firstSample) : NaN;
  const lastSampleMs = lastSample ? Date.parse(lastSample) : NaN;
  const sampleDurationMs =
    Number.isFinite(firstSampleMs) && Number.isFinite(lastSampleMs)
      ? Math.max(0, lastSampleMs - firstSampleMs)
      : 0;
  const dataAvailableValue = !lastSample
    ? "none"
    : chart.times.length === 1
      ? "single sample"
      : formatDuration(sampleDurationMs);
  const pointsPerVps =
    sampledClients > 0 ? Math.round(chart.observedPoints / sampledClients) : chart.times.length;
  const isSparse =
    chart.observedPoints > 0 &&
    (pointsPerVps <= 3 ||
      (selectedDurationMs > 0 &&
        sampleDurationMs > 0 &&
        sampleDurationMs / selectedDurationMs < 0.25));

  return {
    dataAvailableValue,
    isSparse,
    lastSampleValue: lastSample ? formatCompactTime(lastSample) : "No samples",
    sampleSpanLabel:
      firstSample && lastSample
        ? `Samples ${formatEvidenceTime(firstSample)} to ${formatEvidenceTime(lastSample)}`
        : "No retained samples",
    selectedRangeLabel: overview
      ? `${formatEvidenceTime(overview.time_range.start_at)} to ${formatEvidenceTime(overview.time_range.end_at)}`
      : "No selected range evidence",
    sparseNotice: isSparse
      ? `Sparse data: ${pointsPerVps} sample${pointsPerVps === 1 ? "" : "s"} per VPS across the selected ${windowLabel(window)}. Treat this as point evidence, not a continuous trend.`
      : null,
  };
}

function uniqueAffectedVpsCount(overview: DashboardOverviewRecord | null): number {
  if (!overview) {
    return 0;
  }
  const ids = new Set<string>();
  for (const alert of overview.operations.recent_alerts) {
    if (alert.client_id) {
      ids.add(alert.client_id);
    }
  }
  for (const agent of overview.operations.degraded_agents) {
    ids.add(agent.client_id);
  }
  return ids.size;
}

function formatEvidenceTime(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat(undefined, {
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
  }).format(date);
}

function formatDuration(valueMs: number): string {
  if (valueMs <= 0) {
    return "single point";
  }
  const totalMinutes = Math.max(1, Math.round(valueMs / 60_000));
  if (totalMinutes < 60) {
    return `${totalMinutes}m`;
  }
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours < 24) {
    return minutes ? `${hours}h ${minutes}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const remainingHours = hours % 24;
  return remainingHours ? `${days}d ${remainingHours}h` : `${days}d`;
}

function formatBitsPerSecond(value: number): string {
  if (value >= 1_000_000_000) {
    return `${(value / 1_000_000_000).toFixed(1)} Gbps`;
  }
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(1)} Mbps`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(1)} Kbps`;
  }
  return `${Math.round(value)} bps`;
}

function windowLabel(window: DashboardWindow): string {
  return window === "all" ? "All" : window;
}
