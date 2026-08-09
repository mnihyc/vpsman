import { Activity, RefreshCw, SlidersHorizontal } from "lucide-react";
import { ActionFeedback } from "../../components/ActionFeedback";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../../components/TimeSeriesChart";
import { VpsCombobox } from "../../components/VpsCombobox";
import { dashboardChartColors } from "../../colorPalette";
import { formatLowerBoundCount } from "../../constants";
import {
  dashboardScopeLabel,
  dashboardScopeValueOptions,
  dashboardWindowAccessibleLabel,
  dashboardWindowLabel,
  dashboardWindowOptions,
  dateTimeLocalToIso,
  isoToDateTimeLocal,
} from "../../dashboardQuery";
import type {
  AgentView,
  DashboardLabelClusterRecord,
  DashboardOverviewRecord,
  DashboardPointDensity,
  DashboardPreferences,
  DashboardResourceMetric,
  DashboardResourceSeriesRecord,
  DashboardScopeKind,
  DashboardWindow,
} from "../../types";
import {
  formatByteRateFromBitsPerSecond,
  resourceMetricDefinition,
} from "../../telemetryMetrics";
import { formatCompactTime } from "../../utils";

type FleetMetricsPanelProps = {
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onPreferencesChange: (patch: Partial<DashboardPreferences>) => void;
  onRefresh: () => void;
  onOpenVpsDetail: (agent: AgentView | string) => void;
  onWindowChange: (window: DashboardWindow) => void;
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  window: DashboardWindow;
};

const resourceMetricOptions: Array<{
  label: string;
  value: DashboardResourceMetric;
}> = [
  { label: "CPU", value: "cpu_load" },
  { label: "Memory", value: "memory_used" },
  { label: "Disk", value: "disk_free" },
];
const pointDensityOptions: Array<{
  label: string;
  value: DashboardPointDensity;
}> = [
  { label: "Compact", value: "compact" },
  { label: "Balanced", value: "balanced" },
  { label: "Dense", value: "dense" },
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

type TelemetryFreshness = {
  detail: string;
  label: string;
  stale: boolean;
};

export function FleetMetricsPanel({
  agents,
  error,
  loading,
  onPreferencesChange,
  onRefresh,
  onOpenVpsDetail,
  onWindowChange,
  overview,
  preferences,
  window,
}: FleetMetricsPanelProps) {
  const resourceCurve = overview?.resource_curve ?? null;
  const resourceChart = resourceChartData(resourceCurve?.series ?? []);
  const latestSampleAt = resourceCurve?.latest_sample_at ?? null;
  const sampledClients =
    resourceCurve?.sampled_clients ?? overview?.resources.sampled_clients ?? 0;
  const customRangeActive = Boolean(
    preferences.startAt.trim() || preferences.endAt.trim(),
  );
  const selectedRangeName = customRangeActive
    ? "Custom"
    : dashboardWindowLabel(window);
  const resourceEvidence = buildResourceEvidence(
    overview,
    selectedRangeName,
    resourceChart,
    sampledClients,
    latestSampleAt,
  );
  const freshness = buildTelemetryFreshness(overview, latestSampleAt);
  const windowOptions =
    overview?.available_filters.windows.map((option) => option.value) ??
    dashboardWindowOptions;
  const groupOptions = overview?.available_filters.group_by_options ?? [];
  const excludedClients = resourceCurve?.excluded_clients ?? 0;
  const matchedClients = overview?.scope.matched_clients ?? 0;
  const selectedScopeLabel = dashboardScopeLabel(preferences, overview);
  const scopeOptions = dashboardScopeValueOptions(
    preferences.scopeKind,
    overview,
  );
  const activeAdvancedFilters =
    Number(
      preferences.scopeKind !== "all" && Boolean(preferences.scopeValue.trim()),
    ) +
    Number(customRangeActive) +
    Number(preferences.pointDensity !== "balanced");
  const generatedAt = overview?.generated_at
    ? formatCompactTime(overview.generated_at)
    : "No refresh evidence";
  const timeRange = overview
    ? `${formatCompactTime(overview.time_range.start_at)} - ${formatCompactTime(overview.time_range.end_at)}`
    : "Waiting for metrics";
  const selectedGroupLabel =
    groupOptions.find((option) => option.value === preferences.groupBy)
      ?.label ?? preferences.groupBy;

  return (
    <section className="workspace singleColumn observabilityMetricsWorkspace">
      <div className="fleetPanel observabilityMetricsPanel">
        <div className="sectionHeader">
          <div>
            <h2>Fleet metrics</h2>
            <span>
              Read-only CPU, memory, disk, and fleet grouping analysis from
              retained telemetry.
            </span>
          </div>
          <div className="sectionActions">
            <span
              className="fleetMetricsScopeSummary"
              title={`Current metrics scope: ${selectedScopeLabel}`}
            >
              Scope: {selectedScopeLabel}
            </span>
            <button
              className="secondaryAction compactAction"
              disabled={loading}
              onClick={onRefresh}
              title={
                loading
                  ? "Fleet metrics are already refreshing"
                  : "Refresh retained fleet metrics"
              }
              type="button"
            >
              <RefreshCw size={14} />
              Refresh
            </button>
          </div>
        </div>

        <ActionFeedback
          className="localActionFeedback dashboardActionFeedback fleetMetricsActionFeedback"
          message={error}
          tone="danger"
        />

        {freshness.stale && (
          <div className="observabilityStaleBanner" role="status">
            <div>
              <strong>{freshness.label}</strong>
              <span>{freshness.detail}</span>
            </div>
            <div>
              <span>{selectedScopeLabel}</span>
            </div>
          </div>
        )}

        <div
          className="observabilityMetricsControls"
          aria-label="Fleet metrics controls"
        >
          <div className="timeRangeTabs" aria-label="Fleet metrics time range">
            {windowOptions.map((option) => (
              <button
                aria-label={dashboardWindowAccessibleLabel(option)}
                aria-pressed={!customRangeActive && window === option}
                className={
                  !customRangeActive && window === option ? "active" : ""
                }
                key={option}
                onClick={() => onWindowChange(option)}
                title={dashboardWindowAccessibleLabel(option)}
                type="button"
              >
                {dashboardWindowLabel(option)}
              </button>
            ))}
          </div>
          <div
            className="dashboardSectionTools"
            aria-label="Fleet resource metric"
            title="Choose the retained resource metric plotted for each VPS"
          >
            {resourceMetricOptions.map((option) => (
              <button
                aria-pressed={preferences.resourceMetric === option.value}
                className={
                  preferences.resourceMetric === option.value ? "active" : ""
                }
                key={option.value}
                onClick={() =>
                  onPreferencesChange({ resourceMetric: option.value })
                }
                title={resourceMetricDefinition(option.value)}
                type="button"
              >
                {option.label}
              </button>
            ))}
          </div>
          <label
            className="dashboardToolbarSelect"
            title="Group retained fleet evidence by the selected VPS attribute"
          >
            <span>Group by</span>
            <select
              aria-label="Fleet metrics group by"
              onChange={(event) =>
                onPreferencesChange({
                  groupBy: event.target
                    .value as DashboardPreferences["groupBy"],
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
          <details
            className="fleetMetricsAdvancedFilters"
            title="Restrict fleet metrics by scope, point density, or custom dates"
          >
            <summary
              title={`${activeAdvancedFilters} advanced fleet metric filter${activeAdvancedFilters === 1 ? "" : "s"} active`}
            >
              <SlidersHorizontal size={14} />
              <span>Advanced filters</span>
              {activeAdvancedFilters > 0 && <b>{activeAdvancedFilters}</b>}
            </summary>
            <div className="dashboardControlBar fleetMetricsAdvancedFilterGrid">
              <label>
                <span>Scope</span>
                <select
                  aria-label="Fleet metrics scope kind"
                  onChange={(event) =>
                    onPreferencesChange({
                      scopeKind: event.target.value as DashboardScopeKind,
                      scopeValue: "",
                    })
                  }
                  value={preferences.scopeKind}
                >
                  <option value="all">All VPS</option>
                  <option value="provider">Provider</option>
                  <option value="country">Country</option>
                  <option value="tag">Tag</option>
                  <option value="client">VPS ID/name</option>
                </select>
              </label>
              {preferences.scopeKind === "all" ? (
                <div className="dashboardScopeHint">Full fleet selected</div>
              ) : preferences.scopeKind === "client" ? (
                <label>
                  <span>Scope value</span>
                  <VpsCombobox
                    agents={agents}
                    ariaLabel="Fleet metrics scope value"
                    onChange={(value) =>
                      onPreferencesChange({ scopeValue: value })
                    }
                    placeholder="Search scoped VPS"
                    value={preferences.scopeValue}
                  />
                </label>
              ) : (
                <label>
                  <span>Scope value</span>
                  <select
                    aria-label="Fleet metrics scope value"
                    onChange={(event) =>
                      onPreferencesChange({ scopeValue: event.target.value })
                    }
                    value={preferences.scopeValue}
                  >
                    <option value="">Select {preferences.scopeKind}</option>
                    {scopeOptions.map((option) => (
                      <option key={option.query} value={option.value}>
                        {option.label} ({option.count})
                      </option>
                    ))}
                  </select>
                </label>
              )}
              <label>
                <span>Points</span>
                <select
                  aria-label="Fleet metrics point density"
                  onChange={(event) =>
                    onPreferencesChange({
                      pointDensity: event.target.value as DashboardPointDensity,
                    })
                  }
                  value={preferences.pointDensity}
                >
                  {pointDensityOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>Start</span>
                <input
                  aria-label="Fleet metrics start date"
                  onChange={(event) =>
                    onPreferencesChange({
                      startAt: dateTimeLocalToIso(event.target.value),
                    })
                  }
                  type="datetime-local"
                  value={isoToDateTimeLocal(preferences.startAt)}
                />
              </label>
              <label>
                <span>End</span>
                <input
                  aria-label="Fleet metrics end date"
                  onChange={(event) =>
                    onPreferencesChange({
                      endAt: dateTimeLocalToIso(event.target.value),
                    })
                  }
                  type="datetime-local"
                  value={isoToDateTimeLocal(preferences.endAt)}
                />
              </label>
              <button
                className="secondaryAction compactAction"
                disabled={activeAdvancedFilters === 0}
                onClick={() =>
                  onPreferencesChange({
                    endAt: "",
                    pointDensity: "balanced",
                    scopeKind: "all",
                    scopeValue: "",
                    startAt: "",
                  })
                }
                title={
                  activeAdvancedFilters === 0
                    ? "No advanced fleet metric filters are active"
                    : `Reset ${activeAdvancedFilters} active advanced fleet metric filter${activeAdvancedFilters === 1 ? "" : "s"}`
                }
                type="button"
              >
                Reset filters
              </button>
            </div>
          </details>
        </div>

        <section
          className="dashboardSection observabilityChartSection"
          aria-labelledby="observability-fleet-resource-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-fleet-resource-title">
                {resourceMetricTitle(preferences.resourceMetric)} by VPS
              </h2>
              <span>
                Live scan cards stay in Home and Fleet / Monitor; this page
                analyzes retained telemetry without mutation controls.
              </span>
            </div>
          </div>
          <div className="dashboardNetworkPanel resourceCurvePanel">
            <div className="dashboardCurveCard">
              <div
                className="dashboardChartHeader"
                title={`${resourceMetricTitle(preferences.resourceMetric)} retained trend for ${sampledClients} sampled VPS${sampledClients === 1 ? "" : "s"}`}
              >
                <span
                  title={resourceMetricDefinition(preferences.resourceMetric)}
                >
                  {resourceMetricTitle(preferences.resourceMetric)} trend
                </span>
                <small
                  title={`${sampledClients} VPS${sampledClients === 1 ? "" : "s"} contribute retained samples`}
                >
                  {sampledClients} sampled VPS
                </small>
              </div>
              <p className="observabilityRangeLine">
                Selected: {selectedRangeName} · Data available:{" "}
                {resourceEvidence.dataAvailableValue} · Last sample:{" "}
                {resourceEvidence.lastSampleValue}
              </p>
              <p
                className="observabilityMetricDefinition"
                title={resourceMetricDefinition(preferences.resourceMetric)}
              >
                Metric definition:{" "}
                {resourceMetricDefinition(preferences.resourceMetric)}
              </p>
              <div
                className="observabilityChartEvidence"
                aria-label="Fleet resource freshness"
              >
                <div
                  title={`Selected window: ${selectedRangeName}. ${resourceEvidence.selectedRangeLabel}`}
                >
                  <span title="Requested metrics time window">
                    Selected window
                  </span>
                  <strong title={selectedRangeName}>{selectedRangeName}</strong>
                  <small title={resourceEvidence.selectedRangeLabel}>
                    {resourceEvidence.selectedRangeLabel}
                  </small>
                </div>
                <div
                  className={resourceEvidence.isSparse ? "warning" : undefined}
                  title={`Retained data available: ${resourceEvidence.dataAvailableValue}. ${resourceEvidence.sampleSpanLabel}`}
                >
                  <span title="Actual retained telemetry coverage">
                    Data available
                  </span>
                  <strong title={resourceEvidence.dataAvailableValue}>
                    {resourceEvidence.dataAvailableValue}
                  </strong>
                  <small title={resourceEvidence.sampleSpanLabel}>
                    {resourceEvidence.sampleSpanLabel}
                  </small>
                </div>
                <div
                  className={resourceEvidence.isSparse ? "warning" : undefined}
                  title={
                    resourceEvidence.isSparse
                      ? "Sparse retained evidence is plotted as isolated points; continuity is not implied"
                      : "Retained sample density supports a connected trend line"
                  }
                >
                  <span title="How retained samples are rendered">
                    Chart treatment
                  </span>
                  <strong
                    title={
                      resourceEvidence.isSparse
                        ? "Sparse evidence is rendered as points only"
                        : "Samples are rendered as a trend line"
                    }
                  >
                    {resourceEvidence.isSparse ? "Points only" : "Trend line"}
                  </strong>
                  <small
                    title={
                      resourceEvidence.isSparse
                        ? "Sparse evidence; no trend implied"
                        : "Enough samples for line reading"
                    }
                  >
                    {resourceEvidence.isSparse
                      ? "Sparse evidence; no trend implied"
                      : "Enough samples for line reading"}
                  </small>
                </div>
              </div>
              {resourceEvidence.sparseNotice && (
                <p className="observabilitySparseNotice">
                  {resourceEvidence.sparseNotice}
                </p>
              )}
              <TimeSeriesChart
                ariaLabel="Fleet resource usage curve"
                emptyLabel="No resource telemetry after current filters and exclusions"
                exportFileName={`fleet-${preferences.resourceMetric.replace(/_/g, "-")}`}
                lines={resourceChart.lines}
                pointsOnly={resourceEvidence.isSparse}
                times={resourceChart.times}
                valueFormatter={(value) =>
                  formatResourceValue(preferences.resourceMetric, value)
                }
              />
            </div>
            <div
              className="dashboardTopClients"
              aria-label="Top resource VPS list"
            >
              <div className="dashboardSideRailHeader">
                <strong>Top VPS</strong>
                <span>{resourceCurve?.series.length ?? 0} shown</span>
              </div>
              {(resourceCurve?.series ?? []).map((series) => (
                <button
                  aria-label={`Open ${series.label} instance detail`}
                  className="dashboardClientRow"
                  key={series.client_id}
                  onClick={() => onOpenVpsDetail(series.client_id)}
                  title={`Open instance detail for ${series.label}`}
                  type="button"
                >
                  <span>
                    <strong>{series.label}</strong>
                    <small>
                      {resourcePeakLabel(preferences.resourceMetric)}{" "}
                      {formatResourceValue(
                        preferences.resourceMetric,
                        series.peak,
                      )}
                    </small>
                  </span>
                  <b>
                    {formatResourceValue(
                      preferences.resourceMetric,
                      series.current,
                    )}
                  </b>
                </button>
              ))}
              {!resourceCurve?.series.length && (
                <div className="emptyState compactEmpty">
                  <Activity size={18} />
                  <strong>No resource rows</strong>
                  <span>
                    Telemetry is unavailable for the current scope and time
                    range.
                  </span>
                </div>
              )}
            </div>
          </div>
        </section>

        <div
          className="metricGrid observabilityMetricsSummary"
          aria-label="Fleet metrics summary"
        >
          <MetricTile
            label="Current metric"
            value={resourceMetricTitle(preferences.resourceMetric)}
            detail={`${sampledClients} charted · ${excludedClients} excluded`}
          />
          <MetricTile
            label="Scope"
            value={selectedScopeLabel}
            detail={`${matchedClients} matched · ${sampledClients} with retained samples`}
          />
          <MetricTile
            label="Selected range"
            value={selectedRangeName}
            detail={resourceEvidence.selectedRangeLabel || timeRange}
          />
          <MetricTile
            label="Telemetry freshness"
            value={loading ? "Refreshing" : freshness.label}
            detail={`Data available ${resourceEvidence.dataAvailableValue} · overview ${generatedAt}`}
          />
          <MetricTile
            label="Grouping"
            value={selectedGroupLabel}
            detail={`${overview?.label_clusters.length ?? 0} groups; ${excludedClients} excluded; ${vpsCountLabel((overview?.summary.offline ?? 0) + (overview?.summary.stale ?? 0) + (overview?.summary.revoked ?? 0))} unavailable`}
          />
        </div>

        <WarningDefinitionStrip overview={overview} />

        <section
          className="dashboardSection observabilityGroupSection"
          aria-labelledby="observability-fleet-groups-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-fleet-groups-title">Fleet grouping</h2>
              <span>
                Provider, tag, country, status, and client group summaries for
                scan/comparison work.
              </span>
            </div>
          </div>
          <div
            className="observabilityGroupGrid"
            aria-label="Fleet metrics group breakdown"
          >
            {(overview?.label_clusters ?? []).map((cluster) => (
              <GroupTile
                cluster={cluster}
                key={`${cluster.kind}:${cluster.label}`}
              />
            ))}
            {!overview?.label_clusters.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No groups</strong>
                <span>
                  Group summaries are unavailable until dashboard telemetry
                  loads.
                </span>
              </div>
            )}
          </div>
        </section>
      </div>
    </section>
  );
}

function MetricTile({
  detail,
  label,
  value,
}: {
  detail: string;
  label: string;
  value: string;
}) {
  return (
    <div className="metricCard" title={`${label}: ${value}. ${detail}`}>
      <span title={label}>{label}</span>
      <strong title={`${label}: ${value}`}>{value}</strong>
      <small title={detail}>{detail}</small>
    </div>
  );
}

function GroupTile({ cluster }: { cluster: DashboardLabelClusterRecord }) {
  const boundedSuffix = cluster.counts_truncated
    ? " in the loaded operations page"
    : "";
  return (
    <div
      className="observabilityGroupTile"
      title={`${cluster.kind} group ${cluster.label}: ${cluster.online}/${cluster.total} online`}
    >
      <span title={`Grouping dimension: ${cluster.kind}`}>{cluster.kind}</span>
      <strong title={`Group value: ${cluster.label}`}>{cluster.label}</strong>
      <small
        title={
          cluster.kind === "date"
            ? `${cluster.total} network samples in this date group`
            : `${cluster.online} of ${cluster.total} VPSs online in this group`
        }
      >
        {cluster.kind === "date"
          ? `${cluster.total} network samples, ${formatLowerBoundCount(cluster.warnings, cluster.counts_truncated)} alerts${boundedSuffix}, ${formatLowerBoundCount(cluster.running_jobs, cluster.counts_truncated)} running jobs${boundedSuffix}`
          : `${cluster.online}/${cluster.total} online, ${cluster.offline} offline, ${cluster.stale} stale, ${cluster.revoked} access revoked, ${formatLowerBoundCount(cluster.warnings, cluster.counts_truncated)} alerts${boundedSuffix}, ${formatLowerBoundCount(cluster.running_jobs, cluster.counts_truncated)} active job assignments${boundedSuffix}`}
        , {formatByteRateFromBitsPerSecond(cluster.rx_bps + cluster.tx_bps)}{" "}
        aggregate interval-average rate
      </small>
    </div>
  );
}

function WarningDefinitionStrip({
  overview,
}: {
  overview: DashboardOverviewRecord | null;
}) {
  const activeAlerts = overview?.operations.active_alerts ?? 0;
  const alertsTruncated = overview?.operations.alerts_truncated ?? false;
  const criticalAlerts = overview?.operations.critical_alerts ?? 0;
  const warningAlerts = overview?.operations.warning_alerts ?? 0;
  const infoAlerts = Math.max(0, activeAlerts - criticalAlerts - warningAlerts);
  const affectedVpsCount = uniqueAffectedVpsCount(overview);
  const groupedAlerts =
    overview?.label_clusters.reduce(
      (total, cluster) => total + cluster.warnings,
      0,
    ) ?? 0;
  const groupedAlertsTruncated =
    overview?.label_clusters.some((cluster) => cluster.counts_truncated) ??
    false;
  const unavailableVps =
    (overview?.summary.offline ?? 0) +
    (overview?.summary.stale ?? 0) +
    (overview?.summary.revoked ?? 0);
  const definitions = [
    {
      detail: `${criticalAlerts} critical, ${warningAlerts} warning, ${infoAlerts} info${alertsTruncated ? " in loaded page" : ""}`,
      label: "Active alerts",
      value: formatLowerBoundCount(activeAlerts, alertsTruncated),
    },
    {
      detail:
        "unique VPSs named by the recent alert and degraded-agent rows shown",
      label: "VPS in shown evidence",
      value: String(affectedVpsCount),
    },
    {
      detail: `group rows shown can overlap across provider, country, tag, and all-fleet buckets${groupedAlertsTruncated ? "; alert counts use the loaded operations page" : ""}`,
      label: "Alerts in shown groups",
      value: formatLowerBoundCount(groupedAlerts, groupedAlertsTruncated),
    },
    {
      detail:
        "offline, stale, and access-revoked VPSs in the retained overview scope",
      label: "Unavailable VPS",
      value: String(unavailableVps),
    },
  ];

  return (
    <div
      className="observabilityWarningDefinitions"
      aria-label="Fleet metrics availability definitions"
    >
      {definitions.map((definition) => (
        <div
          key={definition.label}
          title={`${definition.label}: ${definition.value}. ${definition.detail}`}
        >
          <span title={definition.label}>{definition.label}</span>
          <strong title={`${definition.label}: ${definition.value}`}>
            {definition.value}
          </strong>
          <small title={definition.detail}>{definition.detail}</small>
        </div>
      ))}
    </div>
  );
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function resourceChartData(
  series: DashboardResourceSeriesRecord[],
): ResourceChartData {
  const times = sortedUniqueTimes(
    series.flatMap((entry) => entry.points.map((point) => point.bucket_start)),
  );
  const lines = series.map((entry, index) => ({
    color: dashboardChartColors[index % dashboardChartColors.length],
    label: entry.label,
    values: times.map(
      (time) =>
        entry.points.find((point) => point.bucket_start === time)?.value ??
        null,
    ),
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
  return Array.from(new Set(times)).sort(
    (left, right) => Date.parse(left) - Date.parse(right),
  );
}

function formatResourceValue(
  metric: DashboardResourceMetric,
  value: number | null | undefined,
): string {
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
  selectedRangeName: string,
  chart: ResourceChartData,
  sampledClients: number,
  latestSampleAt: string | null,
): ResourceEvidence {
  const firstSample = chart.times[0] ?? null;
  const lastSample =
    latestSampleAt ?? chart.times[chart.times.length - 1] ?? null;
  const selectedStartMs = overview
    ? Date.parse(overview.time_range.start_at)
    : NaN;
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
  const retainedBuckets = chart.times.length;
  const isSparse =
    chart.observedPoints > 0 &&
    (retainedBuckets <= 3 ||
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
      ? `Sparse data: ${retainedBuckets} retained time bucket${retainedBuckets === 1 ? "" : "s"} across ${vpsCountLabel(sampledClients)} in the selected ${selectedRangeName}. Treat this as point evidence, not a continuous trend.`
      : null,
  };
}

function buildTelemetryFreshness(
  overview: DashboardOverviewRecord | null,
  lastSample: string | null,
): TelemetryFreshness {
  if (!overview) {
    return {
      detail: "Waiting for the first retained telemetry query",
      label: "Waiting",
      stale: false,
    };
  }
  const generatedMs = Date.parse(overview.generated_at);
  const rangeEndMs = Date.parse(overview.time_range.end_at);
  const currentRange =
    Number.isFinite(generatedMs) &&
    Number.isFinite(rangeEndMs) &&
    Math.abs(generatedMs - rangeEndMs) <= 5 * 60_000;
  if (!lastSample) {
    return {
      detail: currentRange
        ? "No retained samples match the current scope and range"
        : "No retained samples match this historical scope and range",
      label: "No samples",
      stale: currentRange,
    };
  }
  const lastSampleMs = Date.parse(lastSample);
  if (!currentRange) {
    return {
      detail: `Historical query; latest matching sample ${formatEvidenceTime(lastSample)}`,
      label: "Historical range",
      stale: false,
    };
  }
  const lagMs =
    Number.isFinite(rangeEndMs) && Number.isFinite(lastSampleMs)
      ? Math.max(0, rangeEndMs - lastSampleMs)
      : 0;
  if (lagMs <= 3 * 60_000) {
    return {
      detail: `Latest sample ${formatEvidenceTime(lastSample)}`,
      label: "Current",
      stale: false,
    };
  }
  const lag = formatDuration(lagMs);
  return {
    detail: `Latest sample is ${lag} behind the selected range end; values remain visible as last-known evidence`,
    label: `Telemetry ${lag} behind`,
    stale: true,
  };
}

function uniqueAffectedVpsCount(
  overview: DashboardOverviewRecord | null,
): number {
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
