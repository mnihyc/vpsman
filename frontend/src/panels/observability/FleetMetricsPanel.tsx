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
  const windowOptions = overview?.available_filters.windows.map((option) => option.value) ?? fallbackDashboardWindows;
  const groupOptions = overview?.available_filters.group_by_options ?? [];
  const sampledClients = resourceCurve?.sampled_clients ?? overview?.resources.sampled_clients ?? 0;
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
          <MetricTile label="Time range" value={windowLabel(window)} detail={timeRange} />
          <MetricTile label="Telemetry freshness" value={loading ? "Refreshing" : generatedAt} detail="last dashboard overview sample" />
          <MetricTile label="Grouping" value={selectedGroupLabel} detail={`${overview?.label_clusters.length ?? 0} groups; ${excludedClients} excluded`} />
        </div>

        <section className="dashboardSection observabilityChartSection" aria-labelledby="observability-fleet-resource-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-fleet-resource-title">{resourceMetricTitle(preferences.resourceMetric)} by VPS</h2>
              <span>
                Legend and hover values are available in the chart; rows keep threshold and drilldown context read-only.
              </span>
            </div>
          </div>
          <div className="dashboardNetworkPanel resourceCurvePanel">
            <div className="dashboardCurveCard">
              <div className="dashboardChartHeader">
                <span>{resourceMetricTitle(preferences.resourceMetric)} trend</span>
                <small>{sampledClients} sampled VPS</small>
              </div>
              <TimeSeriesChart
                ariaLabel="Fleet resource usage curve"
                emptyLabel="No resource telemetry after current filters and exclusions"
                lines={resourceChart.lines}
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
        {cluster.online}/{cluster.total} online, {cluster.warnings} warnings, {formatBitsPerSecond(cluster.rx_bps + cluster.tx_bps)}
      </small>
    </div>
  );
}

function resourceChartData(series: DashboardResourceSeriesRecord[]): { lines: TimeSeriesChartLine[]; times: string[] } {
  const times = sortedUniqueTimes(series.flatMap((entry) => entry.points.map((point) => point.bucket_start)));
  return {
    lines: series.map((entry, index) => ({
      color: dashboardChartColors[index % dashboardChartColors.length],
      label: entry.label,
      values: times.map((time) => entry.points.find((point) => point.bucket_start === time)?.value ?? null),
    })),
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
