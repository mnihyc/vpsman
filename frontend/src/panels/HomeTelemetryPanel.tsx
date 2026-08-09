import { useMemo, useState, type ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowRight,
  DatabaseBackup,
  Network,
  RefreshCw,
  Server,
  Tags,
} from "lucide-react";
import { ConsoleActionDrawer, ConsoleStatusBadge } from "../components/ConsoleLayout";
import { ActionFeedback } from "../components/ActionFeedback";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../components/TimeSeriesChart";
import { VpsCombobox } from "../components/VpsCombobox";
import { consolePalette, dashboardChartColors } from "../colorPalette";
import {
  dashboardScopeLabel,
  dashboardScopeValueOptions,
  dashboardWindowAccessibleLabel,
  dashboardWindowLabel,
  dashboardWindowOptions,
  dateTimeLocalToIso,
  isoToDateTimeLocal,
} from "../dashboardQuery";
import type {
  AgentView,
  DashboardDrilldownRecord,
  DashboardLabelClusterRecord,
  DashboardOverviewRecord,
  DashboardPointDensity,
  DashboardPreferences,
  DashboardRefreshIntervalSecs,
  DashboardResourceMetric,
  DashboardResourceSeriesRecord,
  DashboardScopeKind,
  DashboardTrafficClientRecord,
  DashboardTrafficSeriesRecord,
  DashboardTrafficSort,
  DashboardWindow,
} from "../types";
import {
  formatByteRateFromBitsPerSecond,
  INTERFACE_RATE_DEFINITION,
  resourceMetricDefinition,
} from "../telemetryMetrics";
import { formatCompactTime } from "../utils";
import {
  ACCESS_REVOKED_RECOVERY_DETAIL,
  agentStatusPresentation,
} from "../agentDisplayState";

type HomeTelemetryPanelProps = {
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  onNavigate: (drilldown: DashboardDrilldownRecord) => void;
  onPreferencesChange: (patch: Partial<DashboardPreferences>) => void;
  onRefresh: () => void;
  onWindowChange: (window: DashboardWindow) => void;
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  window: DashboardWindow;
};

type DrawerMetric = {
  emptyReason?: string;
  label: string;
  value: string;
  tone?: "critical" | "warning" | "ok" | "info" | "neutral";
};

type DrawerState = {
  description: string;
  drilldown?: DashboardDrilldownRecord;
  metrics: DrawerMetric[];
  title: string;
};

const resourceMetricOptions: Array<{ value: DashboardResourceMetric; label: string }> = [
  { label: "CPU", value: "cpu_load" },
  { label: "Memory", value: "memory_used" },
  { label: "Disk", value: "disk_free" },
];
const trafficSortOptions: Array<{ value: DashboardTrafficSort; label: string }> = [
  { label: "Total", value: "total" },
  { label: "RX", value: "rx" },
  { label: "TX", value: "tx" },
];
const pointDensityOptions: Array<{ value: DashboardPointDensity; label: string }> = [
  { label: "Compact", value: "compact" },
  { label: "Balanced", value: "balanced" },
  { label: "Dense", value: "dense" },
];
const refreshIntervalOptions: Array<{ value: DashboardRefreshIntervalSecs; label: string }> = [
  { label: "5s", value: 5 },
  { label: "30s", value: 30 },
  { label: "1m", value: 60 },
];

export function HomeTelemetryPanel({
  agents,
  error,
  loading,
  onNavigate,
  onPreferencesChange,
  onRefresh,
  onWindowChange,
  overview,
  preferences,
  window,
}: HomeTelemetryPanelProps) {
  const [drawer, setDrawer] = useState<DrawerState | null>(null);
  const summary = overview?.summary;
  const operations = overview?.operations;
  const resources = overview?.resources;
  const resourceCurve = overview?.resource_curve;
  const network = overview?.network;
  const fallbackDrilldown = overview?.drilldowns[0] ?? {
    label: "Open fleet instances",
    query: null,
    subpage: "instances",
    view: "Fleet",
  };
  const windowOptions = overview?.available_filters.windows.map((option) => option.value) ?? dashboardWindowOptions;
  const groupOptions = overview?.available_filters.group_by_options ?? fallbackGroupOptions();
  const scopeOptions = dashboardScopeValueOptions(preferences.scopeKind, overview);
  const customRangeActive = Boolean(preferences.startAt.trim());
  const selectedScopeLabel = dashboardScopeLabel(preferences, overview);
  const selectedGroupLabel = groupLabel(preferences.groupBy);
  const visibleAgentIds = useMemo(
    () => new Set(agents.map((agent) => agent.id)),
    [agents],
  );
  const visibleResourceSeries = useMemo(
    () =>
      (resourceCurve?.series ?? []).filter((series) =>
        visibleAgentIds.has(series.client_id),
      ),
    [resourceCurve?.series, visibleAgentIds],
  );
  const visibleRateClients = useMemo(
    () =>
      (network?.top_clients ?? []).filter((client) =>
        visibleAgentIds.has(client.client_id),
      ),
    [network?.top_clients, visibleAgentIds],
  );
  const visibleTrafficSeries = useMemo(
    () =>
      (network?.traffic_series ?? []).filter((series) =>
        visibleAgentIds.has(series.client_id),
      ),
    [network?.traffic_series, visibleAgentIds],
  );
  const visibleRecentAlerts = useMemo(
    () =>
      (operations?.recent_alerts ?? []).filter(
        (alert) =>
          alert.client_id === null || visibleAgentIds.has(alert.client_id),
      ),
    [operations?.recent_alerts, visibleAgentIds],
  );
  const visibleDegradedAgents = useMemo(
    () =>
      (operations?.degraded_agents ?? []).filter((agent) =>
        visibleAgentIds.has(agent.client_id),
      ),
    [operations?.degraded_agents, visibleAgentIds],
  );
  const resourceChart = useMemo(
    () => resourceChartData(visibleResourceSeries),
    [visibleResourceSeries],
  );
  const networkSpeedChart = useMemo(
    () => ({
      lines: [
        {
          color: consolePalette.chart.blue,
          exportLabel: "Avg inbound rate (bps)",
          label: "Avg inbound rate",
          values: (network?.points ?? []).map((point) => point.rx_bps),
        },
        {
          color: consolePalette.chart.green,
          exportLabel: "Avg outbound rate (bps)",
          label: "Avg outbound rate",
          values: (network?.points ?? []).map((point) => point.tx_bps),
        },
      ],
      times: (network?.points ?? []).map((point) => point.bucket_start),
    }),
    [network?.points],
  );
  const networkPeak = useMemo(() => maxNetworkPoint(network?.points ?? []), [network?.points]);
  const trafficChart = useMemo(
    () => trafficChartData(visibleTrafficSeries, preferences.trafficSort),
    [preferences.trafficSort, visibleTrafficSeries],
  );
  const trafficClients = useMemo(
    () =>
      sortTrafficClients(
        (network?.traffic_top_clients ?? []).filter((client) =>
          visibleAgentIds.has(client.client_id),
        ),
        preferences.trafficSort,
      ),
    [network?.traffic_top_clients, preferences.trafficSort, visibleAgentIds],
  );

  function openDrawer(next: DrawerState) {
    setDrawer(next);
  }

  return (
    <section className={drawer ? "workspace dashboardWorkspace" : "workspace singleColumn dashboardWorkspace"}>
      <div className="workspaceStack">
        <div className="dashboardToolbar">
          <div>
            <h2>Overview</h2>
            <span>
              {overview
                ? `${selectedScopeLabel}; grouped by ${selectedGroupLabel}; ${formatCompactTime(overview.time_range.start_at)} - ${formatCompactTime(overview.time_range.end_at)}`
                : "Loading home telemetry"}
            </span>
          </div>
          <div className="dashboardToolbarActions">
            <label className="dashboardToolbarSelect">
              <span>Refresh</span>
              <select
                aria-label="Home refresh interval"
                id="home-refresh-interval"
                name="home-refresh-interval"
                onChange={(event) =>
                  onPreferencesChange({
                    refreshIntervalSecs: Number(event.target.value) as DashboardRefreshIntervalSecs,
                  })
                }
                value={preferences.refreshIntervalSecs}
              >
                {refreshIntervalOptions.map((option) => (
                  <option key={option.value} value={option.value}>
                    {option.label}
                  </option>
                ))}
              </select>
            </label>
            <div className="timeRangeTabs" aria-label="Home time range">
              {windowOptions.map((option) => (
                <button
                  aria-label={dashboardWindowAccessibleLabel(option)}
                  aria-pressed={!customRangeActive && window === option}
                  className={!customRangeActive && window === option ? "active" : ""}
                  key={option}
                  onClick={() => onWindowChange(option)}
                  title={dashboardWindowAccessibleLabel(option)}
                  type="button"
                >
                  {dashboardWindowLabel(option)}
                </button>
              ))}
            </div>
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                loading ? "A dashboard refresh is already in progress" : undefined
              }
              disabled={loading}
              onClick={onRefresh}
              title="Refresh dashboard telemetry"
              type="button"
            >
              <RefreshCw size={16} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
          </div>
        </div>

        <div className="dashboardControlBar" aria-label="Home statistic controls">
          <label>
            <span>Group by</span>
            <select
              aria-label="Home group by"
              id="home-group-by"
              name="home-group-by"
              onChange={(event) => onPreferencesChange({ groupBy: event.target.value as DashboardPreferences["groupBy"] })}
              value={preferences.groupBy}
            >
              {groupOptions.map((option) => (
                <option key={option.value} value={option.value}>
                  {option.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Points</span>
            <select
              aria-label="Home chart point density"
              id="home-chart-point-density"
              name="home-chart-point-density"
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
            <span>Scope</span>
            <select
              aria-label="Home scope kind"
              id="home-scope-kind"
              name="home-scope-kind"
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
                ariaLabel="Home scope value"
                inputId="home-scope-value"
                onChange={(value) => onPreferencesChange({ scopeValue: value })}
                placeholder="Search scoped VPS"
                value={preferences.scopeValue}
              />
            </label>
          ) : (
            <label>
              <span>Scope value</span>
              <select
                aria-label="Home scope value"
                id="home-scope-value"
                name="home-scope-value"
                onChange={(event) => onPreferencesChange({ scopeValue: event.target.value })}
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
            <span>Start</span>
            <input
              aria-label="Home start date"
              id="home-start-date"
              name="home-start-date"
              onChange={(event) => onPreferencesChange({ startAt: dateTimeLocalToIso(event.target.value) })}
              type="datetime-local"
              value={isoToDateTimeLocal(preferences.startAt)}
            />
          </label>
          <label>
            <span>End</span>
            <input
              aria-label="Home end date"
              id="home-end-date"
              name="home-end-date"
              onChange={(event) => onPreferencesChange({ endAt: dateTimeLocalToIso(event.target.value) })}
              type="datetime-local"
              value={isoToDateTimeLocal(preferences.endAt)}
            />
          </label>
          <button
            className="secondaryAction compactAction"
            data-tooltip-disabled-reason={
              !customRangeActive && !preferences.endAt.trim()
                ? "The dashboard is already using its selected time window"
                : undefined
            }
            disabled={!customRangeActive && !preferences.endAt.trim()}
            onClick={() => onPreferencesChange({ endAt: "", startAt: "" })}
            type="button"
          >
            Use window
          </button>
        </div>

        <ActionFeedback
          className="localActionFeedback dashboardActionFeedback homeTelemetryActionFeedback"
          message={error}
          tone="danger"
        />

        <section className="dashboardSection" aria-labelledby="dashboard-operations-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-operations-title">Operational Health</h2>
              <span>Fleet reachability, active alerts, queued operations, and backup posture.</span>
            </div>
            {summary && (
              <ConsoleStatusBadge
                tone={
                  summary.warnings > 0 ||
                  summary.offline > 0 ||
                  summary.revoked > 0 ||
                  summary.stale > 0
                    ? "warning"
                    : "ok"
                }
              >
                {summary.online}/{summary.total} online
              </ConsoleStatusBadge>
            )}
          </div>
          <div className="dashboardCardGrid operationalGrid">
            <HomeMetricCard
              detail={`${summary?.online ?? 0} online, ${summary?.offline ?? 0} offline, ${summary?.stale ?? 0} stale, ${summary?.revoked ?? 0} access revoked`}
              icon={<Server size={19} />}
              label="Fleet health"
              onClick={() =>
                openDrawer({
                  description:
                    "Online, offline, stale, and access-revoked status across VPS clients.",
                  drilldown: fallbackDrilldown,
                  metrics: [
                    { label: "Online", tone: "ok", value: String(summary?.online ?? 0) },
                    { label: "Total", value: String(summary?.total ?? 0) },
                    { label: "Offline", tone: summary?.offline ? "neutral" : "ok", value: String(summary?.offline ?? 0) },
                    { label: "Stale", tone: summary?.stale ? "warning" : "ok", value: String(summary?.stale ?? 0) },
                    { label: "Access revoked", tone: summary?.revoked ? "warning" : "ok", value: String(summary?.revoked ?? 0) },
                    { label: "Warnings", tone: summary?.warnings ? "warning" : "ok", value: String(summary?.warnings ?? 0) },
                  ],
                  title: "Fleet health",
                })
              }
              tone={
                summary &&
                (summary.offline > 0 ||
                  summary.revoked > 0 ||
                  summary.stale > 0)
                  ? "warning"
                  : "ok"
              }
              value={summary ? `${onlinePercent(summary.online, summary.total)}%` : "No data"}
            />
            <HomeMetricCard
              detail={`${operations?.critical_alerts ?? 0} critical, ${operations?.warning_alerts ?? 0} warning`}
              icon={<AlertTriangle size={19} />}
              label="Active alerts"
              onClick={() =>
                openDrawer({
                  description: "Open fleet alerts generated from agent, resource, backup, update, and network state.",
                  drilldown: { label: "Review alerts", query: null, subpage: "alerts", view: "Fleet" },
                  metrics: [
                    { label: "Active", tone: operations?.active_alerts ? "warning" : "ok", value: String(operations?.active_alerts ?? 0) },
                    { label: "Critical", tone: operations?.critical_alerts ? "critical" : "ok", value: String(operations?.critical_alerts ?? 0) },
                    { label: "Warning", tone: operations?.warning_alerts ? "warning" : "ok", value: String(operations?.warning_alerts ?? 0) },
                  ],
                  title: "Active alerts",
                })
              }
              tone={operations && operations.critical_alerts > 0 ? "critical" : operations && operations.warning_alerts > 0 ? "warning" : "ok"}
              value={String(operations?.active_alerts ?? 0)}
            />
            <HomeMetricCard
              detail="Pending or running"
              icon={<Activity size={19} />}
              label="Running jobs"
              onClick={() =>
                openDrawer({
                  description: "Jobs still active in the control plane dispatch lifecycle.",
                  drilldown: { label: "Open job history", query: null, subpage: "history", view: "Jobs" },
                  metrics: [{ label: "Running", tone: operations?.running_jobs ? "info" : "ok", value: String(operations?.running_jobs ?? 0) }],
                  title: "Running jobs",
                })
              }
              tone={operations && operations.running_jobs > 0 ? "info" : "ok"}
              value={String(operations?.running_jobs ?? 0)}
            />
            <HomeMetricCard
              detail={`${operations?.backup_pending ?? 0} pending, ${operations?.backup_completed ?? 0} complete`}
              icon={<DatabaseBackup size={19} />}
              label="Backup posture"
              onClick={() =>
                openDrawer({
                  description: "Recent backup requests and artifact metadata state for the selected time range.",
                  drilldown: { label: "Open backup requests", query: null, subpage: "requests", view: "Backups" },
                  metrics: [
                    { label: "Pending", tone: operations?.backup_pending ? "info" : "neutral", value: String(operations?.backup_pending ?? 0) },
                    { label: "Completed", tone: "ok", value: String(operations?.backup_completed ?? 0) },
                    { label: "Failed", tone: operations?.backup_failed ? "critical" : "ok", value: String(operations?.backup_failed ?? 0) },
                  ],
                  title: "Backup posture",
                })
              }
              tone={operations && operations.backup_failed > 0 ? "critical" : "ok"}
              value={`${operations?.backup_completed ?? 0}/${(operations?.backup_completed ?? 0) + (operations?.backup_pending ?? 0)}`}
            />
            <HomeMetricCard
              detail={`${formatByteRateFromBitsPerSecond(network?.rx_bps)} in / ${formatByteRateFromBitsPerSecond(network?.tx_bps)} out`}
              icon={<Network size={19} />}
              label="Network activity"
              onClick={() =>
                openDrawer({
                  description: "Latest observed per-interface receive and transmit rate totals.",
                  drilldown: { label: "Inspect network evidence", query: null, subpage: "evidence", view: "Network" },
                  metrics: [
                    { label: "Inbound", tone: "info", value: formatByteRateFromBitsPerSecond(network?.rx_bps) },
                    { label: "Outbound", tone: "info", value: formatByteRateFromBitsPerSecond(network?.tx_bps) },
                    { label: "Top clients", value: String(network?.top_clients.length ?? 0) },
                  ],
                  title: "Network activity",
                })
              }
              tone="info"
              value={formatByteRateFromBitsPerSecond(
                network ? network.rx_bps + network.tx_bps : null,
              )}
            />
          </div>
        </section>

        <section className="dashboardSection" aria-labelledby="dashboard-resources-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-resources-title">Resource Usage</h2>
              <span>
                {resourceCurve?.sampled_clients ?? resources?.sampled_clients ?? 0} VPS plotted by {resourceMetricTitle(preferences.resourceMetric)}
                {resourceCurve?.excluded_clients ? `; ${resourceCurve.excluded_clients} excluded by preference` : ""}
              </span>
            </div>
            <div className="dashboardSectionTools" aria-label="Resource usage metric">
              {resourceMetricOptions.map((option) => (
                <button
                  aria-pressed={preferences.resourceMetric === option.value}
                  className={preferences.resourceMetric === option.value ? "active" : ""}
                  key={option.value}
                  onClick={() => onPreferencesChange({ resourceMetric: option.value })}
                  title={resourceMetricDefinition(option.value)}
                  type="button"
                >
                  {option.label}
                </button>
              ))}
            </div>
          </div>
          <div className="dashboardNetworkPanel resourceCurvePanel">
            <div className="dashboardCurveCard">
              <div className="dashboardChartHeader">
                <span>{resourceMetricTitle(preferences.resourceMetric)} by VPS</span>
                <button
                  className="secondaryAction compactAction"
                  onClick={() =>
                    openDrawer({
                      description: `Top ${resourceCurve?.top_limit ?? 0} VPS for ${resourceMetricTitle(preferences.resourceMetric)} over the selected time range.`,
                      metrics: [
                        { label: "Sampled VPS", value: String(resourceCurve?.sampled_clients ?? 0) },
                        { label: "Excluded VPS", value: String(resourceCurve?.excluded_clients ?? 0) },
                        { label: "Fleet CPU average", value: formatLoad(resources?.cpu_load_avg) },
                        { label: "Fleet memory used", value: formatPercent(resources?.memory_used_ratio) },
                        { label: "Fleet disk free", value: formatPercent(resources?.disk_free_ratio) },
                      ],
                      title: `${resourceMetricTitle(preferences.resourceMetric)} curves`,
                    })
                  }
                  type="button"
                >
                  Inspect
                </button>
              </div>
              <p
                className="observabilityMetricDefinition"
                title={resourceMetricDefinition(preferences.resourceMetric)}
              >
                Metric definition: {resourceMetricDefinition(preferences.resourceMetric)}
              </p>
              <TimeSeriesChart
                ariaLabel="Resource usage curve"
                emptyLabel="No resource telemetry after current filters and exclusions"
                lines={resourceChart.lines}
                times={resourceChart.times}
                valueFormatter={(value) => formatResourceValue(preferences.resourceMetric, value)}
              />
            </div>
            <div className="dashboardTopClients">
              <div className="dashboardSideRailHeader">
                <strong>Top VPS</strong>
                <span>{visibleResourceSeries.length} visible</span>
              </div>
              {visibleResourceSeries.map((series) => (
                <button
                  className="dashboardClientRow"
                  key={series.client_id}
                  onClick={() =>
                    openDrawer({
                      description: `${series.points.length} resource samples for ${series.client_id}.`,
                      drilldown: series.drilldown,
                      metrics: [
                        { label: "Current", value: formatResourceValue(preferences.resourceMetric, series.current) },
                        { label: resourcePeakLabel(preferences.resourceMetric), value: formatResourceValue(preferences.resourceMetric, series.peak) },
                        { label: "Warning", tone: "warning", value: formatResourceValue(preferences.resourceMetric, series.warning_threshold) },
                        { label: "Critical", tone: "critical", value: formatResourceValue(preferences.resourceMetric, series.critical_threshold) },
                        { label: "Direction", value: series.threshold_direction },
                      ],
                      title: series.label,
                    })
                  }
                  type="button"
                >
                  <span>
                    <strong>{series.label}</strong>
                    <small>{resourcePeakLabel(preferences.resourceMetric)} {formatResourceValue(preferences.resourceMetric, series.peak)}</small>
                  </span>
                  <b>{formatResourceValue(preferences.resourceMetric, series.current)}</b>
                </button>
              ))}
            </div>
          </div>
        </section>

        <section className="dashboardSection" aria-labelledby="dashboard-network-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-network-title">Network</h2>
              <span>
                Rate is an interval average from interface-counter deltas; Traffic is byte volume over the selected range.
              </span>
            </div>
            <div className="dashboardHeaderTools">
              <div className="dashboardSectionTools" aria-label="Network view">
                <button
                  aria-pressed={preferences.networkView === "speed"}
                  className={preferences.networkView === "speed" ? "active" : ""}
                  onClick={() => onPreferencesChange({ networkView: "speed" })}
                  type="button"
                >
                  Rate
                </button>
                <button
                  aria-pressed={preferences.networkView === "traffic"}
                  className={preferences.networkView === "traffic" ? "active" : ""}
                  onClick={() => onPreferencesChange({ networkView: "traffic" })}
                  type="button"
                >
                  Traffic
                </button>
              </div>
              {preferences.networkView === "traffic" && (
                <div className="dashboardSectionTools" aria-label="Traffic sort">
                  {trafficSortOptions.map((option) => (
                    <button
                      aria-pressed={preferences.trafficSort === option.value}
                      className={preferences.trafficSort === option.value ? "active" : ""}
                      key={option.value}
                      onClick={() => onPreferencesChange({ trafficSort: option.value })}
                      type="button"
                    >
                      {option.label}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <ConsoleStatusBadge tone={network?.points.length ? "info" : "neutral"}>
              {preferences.networkView === "speed"
                ? `${network?.points.length ?? 0} rate points`
                : `${network?.traffic_top_clients.length ?? 0} traffic clients`}
            </ConsoleStatusBadge>
          </div>
          <div className="dashboardNetworkPanel">
            {preferences.networkView === "speed" ? (
              <>
                <div className="dashboardCurveCard">
                  <div className="dashboardChartHeader">
                    <span title={INTERFACE_RATE_DEFINITION}>Network rate</span>
                    <button
                      className="secondaryAction compactAction"
                      onClick={() =>
                        openDrawer({
                          description: `Aggregated receive and transmit rate history for the selected time range. ${INTERFACE_RATE_DEFINITION}`,
                          drilldown: { label: "Open network evidence", query: null, subpage: "evidence", view: "Network" },
                          metrics: [
                            { label: "Inbound avg", tone: "info", value: formatByteRateFromBitsPerSecond(network?.rx_bps) },
                            { label: "Outbound avg", tone: "info", value: formatByteRateFromBitsPerSecond(network?.tx_bps) },
                            { label: "Peak bucket", value: formatByteRateFromBitsPerSecond(networkPeak) },
                          ],
                          title: "Network rate",
                        })
                      }
                      type="button"
                    >
                      Inspect
                    </button>
                  </div>
                  <TimeSeriesChart
                    ariaLabel="Network interval-average rate curve"
                    emptyLabel="No network rate samples for this time gap"
                    lines={networkSpeedChart.lines}
                    times={networkSpeedChart.times}
                    valueFormatter={formatByteRateFromBitsPerSecond}
                  />
                </div>
                <div className="dashboardTopClients">
                  <div className="dashboardSideRailHeader">
                    <strong>Top rate</strong>
                    <span>{visibleRateClients.length} visible</span>
                  </div>
                  {visibleRateClients.map((client) => (
                    <button className="dashboardClientRow" key={client.client_id} onClick={() => openDrawer({
                      description: `${client.interfaces.length} observed interface${client.interfaces.length === 1 ? "" : "s"}.`,
                      drilldown: client.drilldown,
                      metrics: [
                        { label: "Inbound avg", tone: "info", value: formatByteRateFromBitsPerSecond(client.rx_bps) },
                        { label: "Outbound avg", tone: "info", value: formatByteRateFromBitsPerSecond(client.tx_bps) },
                        { label: "Interfaces", value: client.interfaces.join(", ") || "No interfaces" },
                      ],
                      title: client.label,
                    })} type="button">
                      <span>
                        <strong>{client.label}</strong>
                        <small>{client.interfaces.join(", ") || client.client_id}</small>
                      </span>
                      <b>{formatByteRateFromBitsPerSecond(client.rx_bps + client.tx_bps)}</b>
                    </button>
                  ))}
                </div>
              </>
            ) : (
              <>
                <div className="dashboardCurveCard">
                  <div className="dashboardChartHeader">
                    <span>Network traffic by VPS</span>
                    <button
                      className="secondaryAction compactAction"
                      onClick={() =>
                        openDrawer({
                          description: "Traffic curves plot accumulated bytes by VPS for the selected time range.",
                          drilldown: { label: "Open network evidence", query: null, subpage: "evidence", view: "Network" },
                          metrics: [
                            { label: "Rank metric", value: trafficSortLabel(preferences.trafficSort) },
                            { label: "Traffic VPS", value: String(trafficClients.length) },
                            { label: "Total traffic", value: formatBytes(trafficClients.reduce((sum, client) => sum + client.rx_bytes + client.tx_bytes, 0)) },
                          ],
                          title: "Network traffic",
                        })
                      }
                      type="button"
                    >
                      Inspect
                    </button>
                  </div>
                  <TimeSeriesChart
                    ariaLabel="Network traffic curve"
                    emptyLabel="No network traffic samples for this time gap"
                    lines={trafficChart.lines}
                    times={trafficChart.times}
                    valueFormatter={(value) => formatBytes(value ?? 0)}
                  />
                </div>
                <div className="dashboardTopClients">
                  <div className="dashboardSideRailHeader">
                    <strong>Top traffic</strong>
                    <span>{trafficSortLabel(preferences.trafficSort)}</span>
                  </div>
                  {trafficClients.map((client) => (
                    <button
                      className="dashboardClientRow"
                      key={client.client_id}
                      onClick={() =>
                        openDrawer({
                          description: `${client.interfaces.join(", ") || client.client_id}`,
                          drilldown: client.drilldown,
                          metrics: [
                            { label: "RX traffic", tone: "info", value: formatBytes(client.rx_bytes) },
                            { label: "TX traffic", tone: "info", value: formatBytes(client.tx_bytes) },
                            { label: "Total traffic", value: formatBytes(client.rx_bytes + client.tx_bytes) },
                          ],
                          title: client.label,
                        })
                      }
                      type="button"
                    >
                      <span>
                        <strong>{client.label}</strong>
                        <small>{client.interfaces.join(", ") || client.client_id}</small>
                      </span>
                      <b>{formatBytes(trafficValue(client, preferences.trafficSort))}</b>
                    </button>
                  ))}
                </div>
              </>
            )}
          </div>
        </section>

        <section className="dashboardSection" aria-labelledby="dashboard-clusters-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-clusters-title">Grouped Statistics</h2>
              <span>{groupDescription(preferences.groupBy, overview?.scope.label ?? "All VPS")}</span>
            </div>
          </div>
          <div className="dashboardClusterGrid">
            {(overview?.label_clusters ?? []).map((cluster) => (
              <ClusterButton cluster={cluster} key={`${cluster.kind}-${cluster.label}`} onOpen={openDrawer} />
            ))}
          </div>
        </section>

        {operations &&
          (visibleRecentAlerts.length > 0 ||
            visibleDegradedAgents.length > 0) && (
          <section className="dashboardSection dashboardLists" aria-label="Operational lists">
            <div>
              <h2>Recent Alerts</h2>
              <div className="dashboardList">
                {visibleRecentAlerts.map((alert) => (
                  <button
                    className="dashboardListRow"
                    key={alert.id}
                    onClick={() =>
                      openDrawer({
                        description: `${alert.category} observed ${formatCompactTime(alert.observed_at)}`,
                        drilldown: alert.drilldown,
                        metrics: [
                          { label: "Severity", tone: severityTone(alert.severity), value: alert.severity },
                          { label: "Category", value: alert.category },
                          { label: "Client", value: alert.client_label ? `${alert.client_label} / ${alert.client_id}` : alert.client_id ?? "Fleet scope" },
                        ],
                        title: alert.client_label ? `${alert.client_label}: ${alert.title}` : alert.title,
                      })
                    }
                    type="button"
                  >
                    <ConsoleStatusBadge tone={severityTone(alert.severity)}>{alert.severity}</ConsoleStatusBadge>
                    <span className="dashboardAlertText">
                      <strong>{alert.client_label ?? "Fleet"}</strong>
                      <small>{alert.title}</small>
                    </span>
                  </button>
                ))}
              </div>
            </div>
            <div>
              <h2>Degraded VPS</h2>
              <div className="dashboardList">
                {visibleDegradedAgents.map((agent) => {
                  const status = agentStatusPresentation(agent.status);
                  return (
                    <button
                      className="dashboardListRow"
                      key={agent.client_id}
                      onClick={() =>
                        openDrawer({
                          description:
                            agent.status.trim().toLowerCase() === "revoked"
                              ? ACCESS_REVOKED_RECOVERY_DETAIL
                              : agent.tags.join(", ") || agent.client_id,
                          drilldown: agent.drilldown,
                          metrics: [
                            {
                              label: "Status",
                              tone: status.tone,
                              value: status.label,
                            },
                            {
                              emptyReason: "This VPS has no assigned tags",
                              label: "Tags",
                              value: agent.tags.join(", ") || "-",
                            },
                          ],
                          title: agent.label,
                        })
                      }
                      type="button"
                    >
                      <ConsoleStatusBadge tone={status.tone}>
                        {status.label}
                      </ConsoleStatusBadge>
                      <span>{agent.label}</span>
                    </button>
                  );
                })}
              </div>
            </div>
          </section>
        )}
      </div>

      <ConsoleActionDrawer
        description={drawer?.description}
        footer={
          drawer?.drilldown ? (
            <button
              aria-label={drawer.drilldown.label}
              className="primaryAction"
              onClick={() => onNavigate(drawer.drilldown as DashboardDrilldownRecord)}
              type="button"
            >
              <ArrowRight size={16} />
              <span>{drawer.drilldown.label}</span>
            </button>
          ) : null
        }
        onClose={() => setDrawer(null)}
        open={drawer !== null}
        title={drawer?.title ?? ""}
      >
        <div className="dashboardDrawerMetrics">
          {(drawer?.metrics ?? []).map((metric) => (
            <div
              className="dashboardDrawerMetric"
              data-tooltip-empty-reason={metric.emptyReason}
              key={metric.label}
              title={`${metric.label}: ${
                metric.value === "-" ? metric.emptyReason ?? "No value is available" : metric.value
              }`}
            >
              <span>{metric.label}</span>
              <strong>{metric.value}</strong>
              {metric.tone && <ConsoleStatusBadge tone={metric.tone}>{metric.tone}</ConsoleStatusBadge>}
            </div>
          ))}
        </div>
      </ConsoleActionDrawer>
    </section>
  );
}

function HomeMetricCard({
  detail,
  icon,
  label,
  onClick,
  tone,
  value,
}: {
  detail: string;
  icon: ReactNode;
  label: string;
  onClick: () => void;
  tone: "critical" | "warning" | "ok" | "info" | "neutral";
  value: string;
}) {
  return (
    <button
      className={`dashboardMetricCard ${tone}`}
      onClick={onClick}
      title={`${label}: ${value}. ${detail}`}
      type="button"
    >
      <span className="dashboardMetricIcon">{icon}</span>
      <span>
        <small>{label}</small>
        <strong>{value}</strong>
        <em>{detail}</em>
      </span>
    </button>
  );
}

function ClusterButton({
  cluster,
  onOpen,
}: {
  cluster: DashboardLabelClusterRecord;
  onOpen: (drawer: DrawerState) => void;
}) {
  return (
    <button
      className={cluster.kind === "all" ? "dashboardClusterCard aggregate" : "dashboardClusterCard"}
      onClick={() =>
        onOpen({
          description: cluster.query ?? "All VPS clients in the current fleet.",
          drilldown: cluster.drilldown,
          metrics: clusterDrawerMetrics(cluster),
          title: cluster.label,
        })
      }
      type="button"
    >
      <span className="dashboardMetricIcon"><Tags size={18} /></span>
      <span>
        <small>{cluster.kind}</small>
        <strong>{cluster.label}</strong>
        <em>{clusterSummary(cluster)}</em>
      </span>
      <b title={`${cluster.warnings} open alerts`}>
        {cluster.warnings} alerts
      </b>
    </button>
  );
}

function clusterDrawerMetrics(cluster: DashboardLabelClusterRecord): DrawerMetric[] {
  if (cluster.kind === "date") {
    return [
      { label: "Telemetry samples", value: String(cluster.total) },
      { label: "Completed backups", tone: "ok", value: String(cluster.online) },
      { label: "Pending backups", tone: cluster.stale ? "info" : "neutral", value: String(cluster.stale) },
      { label: "Alerts", tone: cluster.warnings ? "warning" : "ok", value: String(cluster.warnings) },
      { label: "Running jobs", tone: cluster.running_jobs ? "info" : "neutral", value: String(cluster.running_jobs) },
      { label: "Avg network rate", tone: "info", value: formatByteRateFromBitsPerSecond(cluster.rx_bps + cluster.tx_bps) },
    ];
  }
  return [
    { label: "Online", tone: "ok", value: `${cluster.online}/${cluster.total}` },
    { label: "Offline", tone: cluster.offline ? "neutral" : "ok", value: String(cluster.offline) },
    { label: "Access revoked", tone: cluster.revoked ? "warning" : "ok", value: String(cluster.revoked) },
    { label: "Stale", tone: cluster.stale ? "warning" : "ok", value: String(cluster.stale) },
    { label: "Warnings", tone: cluster.warnings ? "warning" : "ok", value: String(cluster.warnings) },
    { label: "Running jobs", tone: cluster.running_jobs ? "info" : "neutral", value: String(cluster.running_jobs) },
    { label: "Avg network rate", tone: "info", value: formatByteRateFromBitsPerSecond(cluster.rx_bps + cluster.tx_bps) },
  ];
}

function clusterSummary(cluster: DashboardLabelClusterRecord): string {
  if (cluster.kind === "date") {
    return `${cluster.total} samples, ${cluster.warnings} alerts`;
  }
  return `${cluster.online}/${cluster.total} online · ${cluster.offline} offline · ${cluster.stale} stale · ${cluster.revoked} access revoked`;
}

function maxNetworkPoint(points: Array<{ rx_bps: number; tx_bps: number }>): number {
  return Math.max(1, ...points.map((point) => Math.max(point.rx_bps, point.tx_bps, point.rx_bps + point.tx_bps)));
}

function resourceChartData(series: DashboardResourceSeriesRecord[]): { lines: TimeSeriesChartLine[]; times: string[] } {
  const times = sortedUniqueTimes(series.flatMap((entry) => entry.points.map((point) => point.bucket_start)));
  const lines = series.map((entry, index) => {
    const points = new Map(entry.points.map((point) => [point.bucket_start, point.value]));
    return {
      color: dashboardChartColors[index % dashboardChartColors.length],
      label: entry.label,
      values: times.map((time) => points.get(time) ?? null),
    };
  });
  return { lines, times };
}

function trafficChartData(
  series: DashboardTrafficSeriesRecord[],
  sort: DashboardTrafficSort,
): { lines: TimeSeriesChartLine[]; times: string[] } {
  const times = sortedUniqueTimes(series.flatMap((entry) => entry.points.map((point) => point.bucket_start)));
  const lines = series.map((entry, index) => {
    const points = new Map(entry.points.map((point) => [point.bucket_start, trafficPointValue(point, sort)]));
    return {
      color: dashboardChartColors[index % dashboardChartColors.length],
      label: entry.label,
      values: times.map((time) => points.get(time) ?? null),
    };
  });
  return { lines, times };
}

function sortedUniqueTimes(times: string[]): string[] {
  return Array.from(new Set(times)).sort((left, right) => Date.parse(left) - Date.parse(right));
}

function onlinePercent(online: number, total: number): number {
  return total > 0 ? Math.round((online / total) * 100) : 0;
}

function formatLoad(value: number | null | undefined): string {
  return value === null || value === undefined ? "No data" : value.toFixed(value >= 10 ? 0 : 2);
}

function formatPercent(value: number | null | undefined): string {
  return value === null || value === undefined ? "No data" : `${Math.round(value * 100)}%`;
}

function formatResourceValue(metric: DashboardResourceMetric, value: number | null | undefined): string {
  if (value === null || value === undefined) {
    return "No data";
  }
  return metric === "cpu_load" ? formatLoad(value) : formatPercent(value);
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

function sortTrafficClients(clients: DashboardTrafficClientRecord[], sort: DashboardTrafficSort): DashboardTrafficClientRecord[] {
  return [...clients].sort((left, right) => {
    const valueDiff = trafficValue(right, sort) - trafficValue(left, sort);
    return valueDiff || left.label.localeCompare(right.label);
  });
}

function trafficValue(client: DashboardTrafficClientRecord, sort: DashboardTrafficSort): number {
  if (sort === "rx") {
    return client.rx_bytes;
  }
  if (sort === "tx") {
    return client.tx_bytes;
  }
  return client.rx_bytes + client.tx_bytes;
}

function trafficPointValue(
  point: { rx_bytes: number; tx_bytes: number },
  sort: DashboardTrafficSort,
): number {
  if (sort === "rx") {
    return point.rx_bytes;
  }
  if (sort === "tx") {
    return point.tx_bytes;
  }
  return point.rx_bytes + point.tx_bytes;
}

function trafficSortLabel(sort: DashboardTrafficSort): string {
  return trafficSortOptions.find((option) => option.value === sort)?.label ?? "Total";
}

function formatBytes(value: number): string {
  if (value >= 1_000_000_000_000) {
    return `${(value / 1_000_000_000_000).toFixed(1)} TB`;
  }
  if (value >= 1_000_000_000) {
    return `${(value / 1_000_000_000).toFixed(1)} GB`;
  }
  if (value >= 1_000_000) {
    return `${(value / 1_000_000).toFixed(1)} MB`;
  }
  if (value >= 1_000) {
    return `${(value / 1_000).toFixed(1)} KB`;
  }
  return `${Math.round(value)} B`;
}

function groupLabel(value: DashboardPreferences["groupBy"]): string {
  return fallbackGroupOptions().find((option) => option.value === value)?.label ?? value;
}

function groupDescription(value: DashboardPreferences["groupBy"], scopeLabel: string): string {
  switch (value) {
    case "labels":
      return `${scopeLabel} grouped by provider, country, and custom tags, followed by full aggregation.`;
    case "tags":
      return `${scopeLabel} grouped by custom tags only.`;
    case "countries":
      return `${scopeLabel} grouped by country:* tags.`;
    case "providers":
      return `${scopeLabel} grouped by provider:* tags.`;
    case "clients":
      return `${scopeLabel} grouped one VPS per card.`;
    case "status":
      return `${scopeLabel} grouped by client connection status.`;
    case "date":
      return `${scopeLabel} grouped into date buckets across the selected range.`;
  }
}

function fallbackGroupOptions() {
  return [
    { description: "Provider, country, and custom tags together", label: "Labels", value: "labels" },
    { description: "Non-provider and non-country tags", label: "Custom tags", value: "tags" },
    { description: "country:* tag distribution", label: "Countries", value: "countries" },
    { description: "provider:* tag distribution", label: "Providers", value: "providers" },
    { description: "One group per VPS in the selected scope", label: "VPS clients", value: "clients" },
    { description: "Online, offline, and stale client states", label: "Status", value: "status" },
    { description: "Time buckets across the selected range", label: "Date buckets", value: "date" },
  ] satisfies Array<{ description: string; label: string; value: DashboardPreferences["groupBy"] }>;
}

function severityTone(value: string): "critical" | "warning" | "ok" | "info" | "neutral" {
  if (value === "critical") {
    return "critical";
  }
  if (value === "warning") {
    return "warning";
  }
  if (value === "info") {
    return "info";
  }
  return "neutral";
}
