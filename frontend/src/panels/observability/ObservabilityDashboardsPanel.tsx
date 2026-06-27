import {
  Activity,
  BarChart3,
  Copy,
  Download,
  Gauge,
  GitBranch,
  LayoutDashboard,
  RefreshCw,
  Server,
} from "lucide-react";
import { useMemo, useState } from "react";
import { ConsoleStatusBadge } from "../../components/ConsoleLayout";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../../components/TimeSeriesChart";
import { consolePalette, dashboardChartColors } from "../../colorPalette";
import type {
  DashboardLabelClusterRecord,
  DashboardNetworkRecord,
  DashboardOverviewRecord,
  DashboardPreferences,
  DashboardResourceMetric,
  DashboardResourceSeriesRecord,
  DashboardSummaryRecord,
  DashboardWindow,
} from "../../types";
import { formatCompactTime } from "../../utils";

type ObservabilityDashboardsPanelProps = {
  error: string | null;
  loading: boolean;
  onOpenFleetMetrics: () => void;
  onOpenNetworkMetrics: () => void;
  onRefresh: () => void;
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  window: DashboardWindow;
};

type DashboardPresetId =
  | "fleet_operations"
  | "resource_capacity"
  | "network_traffic"
  | "group_posture";
type DashboardSectionId = "widgets" | "handoff";

type DashboardCoverageSummary = {
  detail: string;
  isSparse: boolean;
  networkPointCount: number;
  resourcePointCount: number;
  tone: "warning" | "info";
  value: string;
};

type DashboardWidgetCoverageNote = {
  detail: string;
  tone: "warning" | "info";
};

type DashboardPreset = {
  description: string;
  id: DashboardPresetId;
  label: string;
  source: string;
  widgets: string[];
};

const dashboardPresets: DashboardPreset[] = [
  {
    description:
      "Fleet posture, alert load, job activity, and backup health for operator handoff.",
    id: "fleet_operations",
    label: "Fleet operations",
    source: "Dashboard overview summary and operations API",
    widgets: ["summary cards", "recent alerts table", "degraded VPS table"],
  },
  {
    description:
      "CPU, memory, or disk trends using the operator's current dashboard metric preference.",
    id: "resource_capacity",
    label: "Resource capacity",
    source: "Dashboard resource curve API",
    widgets: ["resource chart", "top VPS table", "threshold status cards"],
  },
  {
    description:
      "Fleet ingress/egress speed, traffic totals, and busiest network endpoints.",
    id: "network_traffic",
    label: "Network traffic",
    source: "Dashboard network curve API",
    widgets: ["network speed chart", "traffic chart", "top network VPS table"],
  },
  {
    description:
      "Provider, tag, country, status, and client group rollups for scan and comparison work.",
    id: "group_posture",
    label: "Group posture",
    source: "Dashboard label cluster API",
    widgets: ["group cards", "warning counts", "scope metadata"],
  },
];

const dashboardSections: Array<{
  description: string;
  id: DashboardSectionId;
  label: string;
}> = [
  {
    description: "Charts, tables, and source-backed posture cards",
    id: "widgets",
    label: "Widget layout",
  },
  {
    description: "Link and JSON manifest for handoff",
    id: "handoff",
    label: "Share / Export",
  },
];

export function ObservabilityDashboardsPanel({
  error,
  loading,
  onOpenFleetMetrics,
  onOpenNetworkMetrics,
  onRefresh,
  overview,
  preferences,
  window,
}: ObservabilityDashboardsPanelProps) {
  const [selectedId, setSelectedId] =
    useState<DashboardPresetId>("fleet_operations");
  const [selectedSection, setSelectedSection] =
    useState<DashboardSectionId>("widgets");
  const [status, setStatus] = useState<string | null>(null);
  const selectedPreset =
    dashboardPresets.find((preset) => preset.id === selectedId) ??
    dashboardPresets[0];
  const dashboardScope = overview?.scope.label ?? scopeLabel(preferences);
  const dashboardRange = overview
    ? `${formatCompactTime(overview.time_range.start_at)} - ${formatCompactTime(overview.time_range.end_at)}`
    : `${windowLabel(window)} window`;
  const freshness = dashboardFreshnessSummary(overview, window);
  const sourceSummary = dashboardSourceSummary(overview);
  const exportPayload = useMemo(
    () =>
      buildDashboardExportPayload({
        overview,
        preferences,
        preset: selectedPreset,
        range: dashboardRange,
        scope: dashboardScope,
      }),
    [dashboardRange, dashboardScope, overview, preferences, selectedPreset],
  );

  function exportDashboard() {
    const payload = JSON.stringify(exportPayload, null, 2);
    const blob = new Blob([payload], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `vpsman-${selectedPreset.id}-dashboard.json`;
    anchor.click();
    URL.revokeObjectURL(url);
    setStatus(`Exported ${selectedPreset.label}`);
  }

  async function copyShareLink() {
    const url = new URL(globalThis.window.location.href);
    url.hash = `observability/dashboards/${selectedPreset.id}`;
    url.searchParams.set("dashboard", selectedPreset.id);
    url.searchParams.set("window", window);
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(url.toString());
      setStatus(`Copied ${selectedPreset.label} share link`);
      return;
    }
    setStatus("Clipboard is unavailable; use export JSON for handoff.");
  }

  return (
    <section className="workspace singleColumn observabilityDashboardsWorkspace">
      <div className="fleetPanel observabilityDashboardsPanel">
        <div className="sectionHeader">
          <div>
            <h2>Dashboards</h2>
            <span>
              Read-only dashboard presets assembled from current overview data.
            </span>
          </div>
          <div
            className="sectionActions"
            aria-label="Dashboard manager actions"
          >
            <button
              className="secondaryAction compactAction"
              disabled={loading}
              onClick={onRefresh}
              type="button"
            >
              <RefreshCw size={14} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
            <button
              className="secondaryAction compactAction"
              onClick={() => void copyShareLink()}
              type="button"
            >
              <Copy size={14} />
              <span>Share link</span>
            </button>
            <button
              className="secondaryAction compactAction"
              onClick={exportDashboard}
              type="button"
            >
              <Download size={14} />
              <span>Export JSON</span>
            </button>
          </div>
        </div>

        {error ? (
          <div className="panelError observabilityMetricsError" role="alert">
            {error}
          </div>
        ) : null}

        <div
          className="metricGrid observabilityMetricsSummary"
          aria-label="Dashboard manager summary"
        >
          <MetricTile
            detail="Read-only layouts from current overview data"
            label="Dashboard presets"
            value={String(dashboardPresets.length)}
          />
          <MetricTile
            detail={selectedPreset.source}
            label="Active preset"
            value={selectedPreset.label}
          />
          <MetricTile
            detail={`${dashboardScope}; ${dashboardRange}`}
            label="Scope and range"
            value={windowLabel(window)}
          />
          <MetricTile
            detail={freshness.detail}
            label="Data freshness"
            value={freshness.value}
          />
          <MetricTile
            detail={sourceSummary.detail}
            label="Source counts"
            value={sourceSummary.value}
          />
        </div>

        <DashboardSourceStrip coverage={freshness} overview={overview} />

        <section
          className="dashboardSection observabilityGroupSection"
          aria-labelledby="dashboard-presets-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-presets-title">Dashboard presets</h2>
              <span>
                Preset layouts are read-only until a custom dashboard API
                exists.
              </span>
            </div>
            <ConsoleStatusBadge tone="info">Read-only</ConsoleStatusBadge>
          </div>
          <label className="dashboardPresetMobileMenu">
            <span>Preset</span>
            <select
              aria-label="Dashboard preset"
              onChange={(event) => {
                setSelectedId(event.target.value as DashboardPresetId);
                setSelectedSection("widgets");
              }}
              value={selectedId}
            >
              {dashboardPresets.map((preset) => (
                <option key={preset.id} value={preset.id}>
                  {preset.label}
                </option>
              ))}
            </select>
          </label>
          <div
            className="dashboardPresetGrid"
            aria-label="Dashboard preset registry"
          >
            {dashboardPresets.map((preset) => (
              <button
                aria-pressed={selectedPreset.id === preset.id}
                className={
                  selectedPreset.id === preset.id
                    ? "dashboardPresetTile active"
                    : "dashboardPresetTile"
                }
                key={preset.id}
                onClick={() => {
                  setSelectedId(preset.id);
                  setSelectedSection("widgets");
                }}
                type="button"
              >
                <span className="dashboardPresetIcon">
                  {presetIcon(preset.id)}
                </span>
                <span>
                  <strong>{preset.label}</strong>
                  <small>{preset.description}</small>
                </span>
                <em>{preset.widgets.length} widgets</em>
              </button>
            ))}
          </div>
        </section>

        <div
          className="dashboardSectionSelector"
          aria-label="Dashboard section selector"
        >
          {dashboardSections.map((section) => (
            <button
              aria-pressed={selectedSection === section.id}
              className={selectedSection === section.id ? "active" : ""}
              key={section.id}
              onClick={() => setSelectedSection(section.id)}
              type="button"
            >
              <strong>{section.label}</strong>
              <small>{section.description}</small>
            </button>
          ))}
        </div>

        {selectedSection === "widgets" ? (
          <section
            className="dashboardSection observabilityGroupSection"
            aria-labelledby="dashboard-widget-layout-title"
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="dashboard-widget-layout-title">Widget layout</h2>
                <span>
                  {selectedPreset.label}: {selectedPreset.widgets.join(", ")}.
                </span>
              </div>
              <div
                className="sectionActions"
                aria-label="Dashboard drilldown links"
              >
                <button
                  className="secondaryAction compactAction"
                  onClick={onOpenFleetMetrics}
                  type="button"
                >
                  <Gauge size={14} />
                  Fleet metrics
                </button>
                <button
                  className="secondaryAction compactAction"
                  onClick={onOpenNetworkMetrics}
                  type="button"
                >
                  <GitBranch size={14} />
                  Network metrics
                </button>
              </div>
            </div>
            <DashboardWidgetLayout
              overview={overview}
              preferences={preferences}
              selectedId={selectedPreset.id}
              window={window}
            />
          </section>
        ) : (
          <section
            className="dashboardSection observabilityGroupSection"
            aria-labelledby="dashboard-handoff-title"
          >
            <div className="dashboardSectionHeader">
              <div>
                <h2 id="dashboard-handoff-title">Share / Export</h2>
                <span>Share a link or export the current preset metadata.</span>
              </div>
            </div>
            <div
              className="dashboardHandoffPanel"
              aria-label="Dashboard share and export details"
            >
              <span>
                <strong>Share scope</strong>
                <small>{dashboardScope}</small>
              </span>
              <span>
                <strong>Export format</strong>
                <small>
                  JSON manifest with preset id, widgets, preferences, source
                  counts, and overview data.
                </small>
              </span>
              <span>
                <strong>Controls</strong>
                <small>
                  Read-only: refresh, share, export, and navigation only.
                </small>
              </span>
            </div>
            {status ? (
              <small className="dashboardManagerStatus">{status}</small>
            ) : null}
          </section>
        )}
      </div>
    </section>
  );
}

function DashboardSourceStrip({
  coverage,
  overview,
}: {
  coverage: DashboardCoverageSummary;
  overview: DashboardOverviewRecord | null;
}) {
  const source = dashboardSourceCounts(overview);
  return (
    <div
      className="dashboardSourceStrip"
      aria-label="Dashboard source coverage"
    >
      <span>
        <strong>Fleet source</strong>
        <small>{source.fleetDetail}</small>
      </span>
      <span>
        <strong>Alert source</strong>
        <small>{source.alertDetail}</small>
      </span>
      <span>
        <strong>Job source</strong>
        <small>{source.jobDetail}</small>
      </span>
      <span className={coverage.tone === "warning" ? "warning" : ""}>
        <strong>Range coverage</strong>
        <small>{coverage.detail}</small>
      </span>
    </div>
  );
}

function DashboardWidgetLayout({
  overview,
  preferences,
  selectedId,
  window,
}: {
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  selectedId: DashboardPresetId;
  window: DashboardWindow;
}) {
  if (!overview) {
    return (
      <div className="emptyState compactEmpty">
        <LayoutDashboard size={18} />
        <strong>No dashboard overview</strong>
        <span>
          Refresh the dashboard overview API to populate dashboard preset
          widgets.
        </span>
      </div>
    );
  }
  if (selectedId === "resource_capacity") {
    return (
      <ResourceDashboard
        overview={overview}
        preferences={preferences}
        window={window}
      />
    );
  }
  if (selectedId === "network_traffic") {
    return <NetworkDashboard network={overview.network} window={window} />;
  }
  if (selectedId === "group_posture") {
    return <GroupDashboard clusters={overview.label_clusters} />;
  }
  return <FleetOperationsDashboard overview={overview} />;
}

function FleetOperationsDashboard({
  overview,
}: {
  overview: DashboardOverviewRecord;
}) {
  const operations = overview.operations;
  const source = dashboardSourceCounts(overview);
  const recentAlerts = operations.recent_alerts ?? [];
  const degradedAgents = operations.degraded_agents ?? [];
  return (
    <div
      className="dashboardWidgetGrid"
      aria-label="Fleet operations dashboard widgets"
    >
      <MetricTile
        detail={fleetHealthDetail(overview.summary)}
        label="Fleet health"
        value={fleetHealthValue(overview.summary)}
      />
      <MetricTile
        detail={`${countPhrase(operations.critical_alerts, "critical")} · ${countPhrase(operations.warning_alerts, "warning")} from operations`}
        label="Active alerts"
        value={countValue(operations.active_alerts)}
      />
      <MetricTile
        detail={source.jobDetail}
        label="Running jobs"
        value={countValue(source.runningJobs)}
      />
      <MetricTile
        detail={`${countPhrase(operations.backup_failed, "failed")} · ${countPhrase(operations.backup_pending, "pending")}`}
        label="Backup posture"
        value={countValue(operations.backup_completed)}
      />
      <div
        className="dashboardWidgetTable wideWidget"
        aria-label="Recent alert widget table"
      >
        <WidgetHeader
          title="Recent alerts"
          detail={countPhrase(recentAlerts.length, "retained row")}
        />
        {recentAlerts.slice(0, 8).map((alert) => (
          <div className="dashboardWidgetRow" key={alert.id}>
            <ConsoleStatusBadge
              tone={alert.severity === "critical" ? "critical" : "warning"}
            >
              {alert.severity || "warning"}
            </ConsoleStatusBadge>
            <strong>{alert.title || "Untitled alert"}</strong>
            <span>{alert.client_label ?? "fleet"}</span>
            <small>{formatCompactTime(alert.observed_at)}</small>
          </div>
        ))}
        {!recentAlerts.length ? (
          <WidgetEmpty label="No retained alerts" />
        ) : null}
      </div>
      <div
        className="dashboardWidgetTable"
        aria-label="Degraded VPS widget table"
      >
        <WidgetHeader
          title="Degraded VPS"
          detail={countPhrase(degradedAgents.length, "record")}
        />
        {degradedAgents.slice(0, 8).map((agent) => (
          <div className="dashboardWidgetRow" key={agent.client_id}>
            <ConsoleStatusBadge tone="warning">
              {dashboardAgentStatusLabel(agent.status)}
            </ConsoleStatusBadge>
            <strong>{agent.label || agent.client_id || "Unnamed VPS"}</strong>
            <span>
              {(agent.tags ?? []).slice(0, 3).join(", ") || "no labels"}
            </span>
          </div>
        ))}
        {!degradedAgents.length ? (
          <WidgetEmpty label="No degraded VPS rows" />
        ) : null}
      </div>
    </div>
  );
}

function ResourceDashboard({
  overview,
  preferences,
  window,
}: {
  overview: DashboardOverviewRecord;
  preferences: DashboardPreferences;
  window: DashboardWindow;
}) {
  const chart = resourceChartData(overview.resource_curve.series);
  const coverage = widgetCoverageNote({
    bucketTimes: chart.times,
    label: "Resource curve",
    pointCount: resourceCurvePointCount(overview.resource_curve.series),
    selectedWindow: window,
  });
  return (
    <div
      className="dashboardWidgetGrid"
      aria-label="Resource capacity dashboard widgets"
    >
      <MetricTile
        detail={`${countPhrase(overview.resource_curve.sampled_clients, "sampled")} · ${countPhrase(overview.resource_curve.excluded_clients, "excluded")}`}
        label="Metric"
        value={resourceMetricTitle(preferences.resourceMetric)}
      />
      <MetricTile
        detail="Average across sampled VPSs"
        label="CPU load"
        value={formatResourceValue("cpu_load", overview.resources.cpu_load_avg)}
      />
      <MetricTile
        detail="Average used memory ratio"
        label="Memory used"
        value={formatResourceValue(
          "memory_used",
          overview.resources.memory_used_ratio,
        )}
      />
      <MetricTile
        detail="Average free disk ratio"
        label="Disk free"
        value={formatResourceValue(
          "disk_free",
          overview.resources.disk_free_ratio,
        )}
      />
      <div
        className="dashboardWidgetChart wideWidget"
        aria-label="Resource capacity chart widget"
      >
        <WidgetHeader
          title={`${resourceMetricTitle(preferences.resourceMetric)} chart`}
          detail={`${overview.resource_curve.series.length} VPS series`}
        />
        <WidgetCoverageNote note={coverage} />
        <TimeSeriesChart
          ariaLabel="Dashboard preset resource chart"
          emptyLabel="No resource curve data"
          lines={chart.lines}
          times={chart.times}
          valueFormatter={(value) =>
            formatResourceValue(preferences.resourceMetric, value)
          }
        />
      </div>
      <div
        className="dashboardWidgetTable"
        aria-label="Resource capacity top VPS table"
      >
        <WidgetHeader
          title="Top resource VPS"
          detail={`${countValue(overview.resource_curve.top_limit)} row limit`}
        />
        {overview.resource_curve.series.slice(0, 8).map((series) => (
          <div className="dashboardWidgetRow" key={series.client_id}>
            <ConsoleStatusBadge tone={thresholdTone(series)}>
              {thresholdLabel(series)}
            </ConsoleStatusBadge>
            <strong>{series.label || series.client_id || "Unnamed VPS"}</strong>
            <span>
              {formatResourceValue(preferences.resourceMetric, series.current)}
            </span>
            <small>
              Peak{" "}
              {formatResourceValue(preferences.resourceMetric, series.peak)}
            </small>
          </div>
        ))}
        {!overview.resource_curve.series.length ? (
          <WidgetEmpty label="No resource rows" />
        ) : null}
      </div>
    </div>
  );
}

function NetworkDashboard({
  network,
  window,
}: {
  network: DashboardNetworkRecord;
  window: DashboardWindow;
}) {
  const speedLines: TimeSeriesChartLine[] = [
    {
      color: consolePalette.chart.blue,
      label: "Inbound speed",
      values: network.points.map((point) => point.rx_bps),
    },
    {
      color: consolePalette.chart.green,
      label: "Outbound speed",
      values: network.points.map((point) => point.tx_bps),
    },
  ];
  const trafficLines: TimeSeriesChartLine[] = [
    {
      color: consolePalette.chart.purple,
      label: "Inbound traffic",
      values: network.traffic_points.map((point) => point.rx_bytes),
    },
    {
      color: consolePalette.chart.orange,
      label: "Outbound traffic",
      values: network.traffic_points.map((point) => point.tx_bytes),
    },
  ];
  const speedCoverage = widgetCoverageNote({
    bucketTimes: network.points.map((point) => point.bucket_start),
    label: "Network speed",
    pointCount: network.points.length,
    selectedWindow: window,
  });
  const trafficCoverage = widgetCoverageNote({
    bucketTimes: network.traffic_points.map((point) => point.bucket_start),
    label: "Network traffic",
    pointCount: network.traffic_points.length,
    selectedWindow: window,
  });
  return (
    <div
      className="dashboardWidgetGrid"
      aria-label="Network traffic dashboard widgets"
    >
      <MetricTile
        detail="Current retained ingress"
        label="RX speed"
        value={formatBps(network.rx_bps)}
      />
      <MetricTile
        detail="Current retained egress"
        label="TX speed"
        value={formatBps(network.tx_bps)}
      />
      <MetricTile
        detail="Top speed rows"
        label="Network VPS"
        value={countValue(network.top_clients.length)}
      />
      <MetricTile
        detail="Top traffic rows"
        label="Traffic VPS"
        value={countValue(network.traffic_top_clients.length)}
      />
      <div
        className="dashboardWidgetChart wideWidget"
        aria-label="Network speed chart widget"
      >
        <WidgetHeader
          title="Network speed chart"
          detail="Ingress and egress bps over time"
        />
        <WidgetCoverageNote note={speedCoverage} />
        <TimeSeriesChart
          ariaLabel="Dashboard preset network speed chart"
          emptyLabel="No network speed points"
          lines={speedLines}
          times={network.points.map((point) => point.bucket_start)}
          valueFormatter={(value) => formatBps(value)}
        />
      </div>
      <div
        className="dashboardWidgetChart wideWidget"
        aria-label="Network traffic chart widget"
      >
        <WidgetHeader
          title="Network traffic chart"
          detail="Traffic totals over time"
        />
        <WidgetCoverageNote note={trafficCoverage} />
        <TimeSeriesChart
          ariaLabel="Dashboard preset network traffic chart"
          emptyLabel="No network traffic points"
          lines={trafficLines}
          times={network.traffic_points.map((point) => point.bucket_start)}
          valueFormatter={(value) => formatBytes(value)}
        />
      </div>
      <div
        className="dashboardWidgetTable wideWidget"
        aria-label="Top network VPS widget table"
      >
        <WidgetHeader
          title="Top network VPS"
          detail={countPhrase(network.top_clients.length, "speed row")}
        />
        {network.top_clients.slice(0, 8).map((client) => (
          <div className="dashboardWidgetRow" key={client.client_id}>
            <ConsoleStatusBadge tone="info">
              {countPhrase(client.interfaces.length, "interface")}
            </ConsoleStatusBadge>
            <strong>{client.label || client.client_id || "Unnamed VPS"}</strong>
            <span>{formatBps(client.rx_bps)} in</span>
            <small>{formatBps(client.tx_bps)} out</small>
          </div>
        ))}
        {!network.top_clients.length ? (
          <WidgetEmpty label="No network rows" />
        ) : null}
      </div>
    </div>
  );
}

function GroupDashboard({
  clusters,
}: {
  clusters: DashboardLabelClusterRecord[];
}) {
  return (
    <div
      className="groupDashboardGrid"
      aria-label="Group posture dashboard widgets"
    >
      {clusters.map((cluster, index) => (
        <div
          className="groupDashboardTile"
          key={`${cluster.kind}:${cluster.label}`}
        >
          <span>{cluster.kind || "group"}</span>
          <strong>{cluster.label || "Unnamed group"}</strong>
          <small>
            {countPhrase(cluster.total, "VPS")} ·{" "}
            {countPhrase(cluster.online, "online")} ·{" "}
            {countPhrase(cluster.stale, "stale")}
          </small>
          <dl>
            <div>
              <dt>Warnings</dt>
              <dd>{countValue(cluster.warnings)}</dd>
            </div>
            <div>
              <dt>Jobs</dt>
              <dd>{countValue(cluster.running_jobs)}</dd>
            </div>
            <div>
              <dt>Traffic</dt>
              <dd>{formatBps(cluster.rx_bps + cluster.tx_bps)}</dd>
            </div>
          </dl>
          <i
            style={{
              backgroundColor:
                dashboardChartColors[index % dashboardChartColors.length],
            }}
            aria-hidden="true"
          />
        </div>
      ))}
      {!clusters.length ? (
        <div className="emptyState compactEmpty">
          <Server size={18} />
          <strong>No group widgets</strong>
          <span>
            Group posture appears after dashboard overview data is available.
          </span>
        </div>
      ) : null}
    </div>
  );
}

function dashboardFreshnessSummary(
  overview: DashboardOverviewRecord | null,
  selectedWindow: DashboardWindow,
): DashboardCoverageSummary {
  if (!overview) {
    return {
      detail: `Selected: ${windowLabel(selectedWindow)}`,
      isSparse: false,
      networkPointCount: 0,
      resourcePointCount: 0,
      tone: "info",
      value: "No data",
    };
  }
  const resourcePointCount = resourceCurvePointCount(
    overview.resource_curve.series,
  );
  const networkPointCount =
    overview.network.points.length + overview.network.traffic_points.length;
  const apiRangeSeconds = secondsBetweenIso(
    overview.time_range.start_at,
    overview.time_range.end_at,
  );
  const selectedSeconds = windowDurationSeconds(selectedWindow);
  const pointCount = resourcePointCount + networkPointCount;
  const bucketTimes = uniqueSortedTimes([
    ...resourceCurveBucketTimes(overview.resource_curve.series),
    ...overview.network.points.map((point) => point.bucket_start),
    ...overview.network.traffic_points.map((point) => point.bucket_start),
  ]);
  const sampledSeconds = secondsBetweenIso(
    bucketTimes[0],
    bucketTimes[bucketTimes.length - 1],
  );
  const isSparse = isSparseCoverage({
    bucketCount: bucketTimes.length,
    pointCount,
    rangeSeconds: sampledSeconds,
    selectedSeconds,
  });
  return {
    detail: `${isSparse ? "Sparse" : "Available"} ${windowLabel(selectedWindow)}: ${formatDuration(sampledSeconds)} sampled · ${countPhrase(pointCount, "sample")} (${countPhrase(resourcePointCount, "resource sample")} · ${countPhrase(networkPointCount, "network sample")}) · API range ${formatDuration(apiRangeSeconds)}`,
    isSparse,
    networkPointCount,
    resourcePointCount,
    tone: isSparse ? "warning" : "info",
    value: isSparse
      ? "Sparse range"
      : `Generated ${formatCompactTime(overview.generated_at)}`,
  };
}

function dashboardSourceSummary(overview: DashboardOverviewRecord | null): {
  detail: string;
  value: string;
} {
  if (!overview) {
    return {
      detail:
        "Fleet, alert, and job counts load from the dashboard overview API",
      value: "No data",
    };
  }
  const source = dashboardSourceCounts(overview);
  return {
    detail: `${source.fleetDetail} · ${countPhrase(overview.operations.active_alerts, "active alert")} · ${countPhrase(source.runningJobs, "running job")}`,
    value: `${countValue(overview.summary.total)} VPS`,
  };
}

function dashboardSourceCounts(overview: DashboardOverviewRecord | null): {
  alertDetail: string;
  fleetDetail: string;
  jobDetail: string;
  runningJobs: number | null;
} {
  if (!overview) {
    return {
      alertDetail: "Operations alert counts unavailable",
      fleetDetail: "Fleet summary unavailable",
      jobDetail: "Running job counts unavailable",
      runningJobs: null,
    };
  }
  const summaryJobs = finiteCount(overview.summary.running_jobs);
  const operationsJobs = finiteCount(overview.operations.running_jobs);
  const runningJobs = summaryJobs ?? operationsJobs;
  const jobDetail =
    summaryJobs !== null &&
    operationsJobs !== null &&
    summaryJobs !== operationsJobs
      ? `Summary ${summaryJobs}; operations ${operationsJobs}; showing summary`
      : summaryJobs !== null
        ? `${countPhrase(summaryJobs, "running job")} from fleet summary`
        : operationsJobs !== null
          ? `${countPhrase(operationsJobs, "running job")} from operations`
          : "Running job counts unavailable";
  return {
    alertDetail: `${countPhrase(overview.operations.active_alerts, "active alert")} · ${countPhrase(overview.operations.critical_alerts, "critical")} · ${countPhrase(overview.operations.warning_alerts, "warning")} from operations`,
    fleetDetail: `${fleetHealthValue(overview.summary)} online · ${countPhrase(overview.summary.stale, "stale")} · ${countPhrase(summaryOfflineCount(overview.summary), "offline")} from summary`,
    jobDetail,
    runningJobs,
  };
}

function WidgetCoverageNote({ note }: { note: DashboardWidgetCoverageNote }) {
  return (
    <div className={`dashboardWidgetNote ${note.tone}`}>{note.detail}</div>
  );
}

function widgetCoverageNote({
  bucketTimes,
  label,
  pointCount,
  selectedWindow,
}: {
  bucketTimes: string[];
  label: string;
  pointCount: number;
  selectedWindow: DashboardWindow;
}): DashboardWidgetCoverageNote {
  const uniqueTimes = uniqueSortedTimes(bucketTimes);
  const rangeSeconds = secondsBetweenIso(
    uniqueTimes[0],
    uniqueTimes[uniqueTimes.length - 1],
  );
  const selectedSeconds = windowDurationSeconds(selectedWindow);
  const sparse = isSparseCoverage({
    bucketCount: uniqueTimes.length,
    pointCount,
    rangeSeconds,
    selectedSeconds,
  });
  if (pointCount === 0) {
    return {
      detail: `${label}: no samples for ${windowLabel(selectedWindow)}`,
      tone: "warning",
    };
  }
  if (sparse) {
    return {
      detail: `Sparse ${windowLabel(selectedWindow)}: ${uniqueTimes.length} buckets over ${formatDuration(rangeSeconds)}. Drill down before judging trend.`,
      tone: "warning",
    };
  }
  return {
    detail: `${label}: ${uniqueTimes.length} buckets over ${formatDuration(rangeSeconds)}`,
    tone: "info",
  };
}

function resourceCurvePointCount(
  series: DashboardResourceSeriesRecord[],
): number {
  return series.reduce((total, item) => total + item.points.length, 0);
}

function resourceCurveBucketTimes(
  series: DashboardResourceSeriesRecord[],
): string[] {
  return uniqueSortedTimes(
    series.flatMap((item) => item.points.map((point) => point.bucket_start)),
  );
}

function uniqueSortedTimes(times: Array<string | undefined>): string[] {
  return Array.from(
    new Set(times.filter((time): time is string => Boolean(time))),
  ).sort((left, right) => Date.parse(left) - Date.parse(right));
}

function secondsBetweenIso(
  start: string | undefined,
  end: string | undefined,
): number {
  if (!start || !end) {
    return 0;
  }
  const startTime = Date.parse(start);
  const endTime = Date.parse(end);
  if (
    !Number.isFinite(startTime) ||
    !Number.isFinite(endTime) ||
    endTime <= startTime
  ) {
    return 0;
  }
  return Math.round((endTime - startTime) / 1000);
}

function isSparseCoverage({
  bucketCount,
  pointCount,
  rangeSeconds,
  selectedSeconds,
}: {
  bucketCount: number;
  pointCount: number;
  rangeSeconds: number;
  selectedSeconds: number | null;
}): boolean {
  if (pointCount === 0) {
    return true;
  }
  if (selectedSeconds === null || selectedSeconds <= 0) {
    return bucketCount < 2;
  }
  if (bucketCount < 2 && selectedSeconds >= 3600) {
    return true;
  }
  return rangeSeconds > 0 && rangeSeconds < selectedSeconds * 0.25;
}

function windowDurationSeconds(window: DashboardWindow): number | null {
  if (window === "15m") return 15 * 60;
  if (window === "1h") return 60 * 60;
  if (window === "6h") return 6 * 60 * 60;
  if (window === "24h") return 24 * 60 * 60;
  if (window === "7d") return 7 * 24 * 60 * 60;
  if (window === "14d") return 14 * 24 * 60 * 60;
  if (window === "30d") return 30 * 24 * 60 * 60;
  return null;
}

function formatDuration(seconds: number): string {
  if (seconds <= 0) {
    return "no continuous range";
  }
  if (seconds < 60) {
    return "under 1 min";
  }
  if (seconds < 60 * 60) {
    return `${Math.round(seconds / 60)} min`;
  }
  if (seconds < 24 * 60 * 60) {
    const hours = seconds / (60 * 60);
    return `${hours < 10 ? hours.toFixed(1) : Math.round(hours)} hours`;
  }
  const days = seconds / (24 * 60 * 60);
  return `${days < 10 ? days.toFixed(1) : Math.round(days)} days`;
}

function fleetHealthValue(summary: DashboardSummaryRecord): string {
  const online = finiteCount(summary.online);
  const total = finiteCount(summary.total);
  if (online === null || total === null) {
    return "No data";
  }
  return `${online}/${total}`;
}

function fleetHealthDetail(summary: DashboardSummaryRecord): string {
  return `${countPhrase(summary.stale, "stale")} · ${countPhrase(summaryOfflineCount(summary), "offline")}`;
}

function summaryOfflineCount(summary: DashboardSummaryRecord): number | null {
  const directOffline = finiteCount(summary.offline);
  if (directOffline !== null) {
    return directOffline;
  }
  const total = finiteCount(summary.total);
  const online = finiteCount(summary.online);
  const stale = finiteCount(summary.stale);
  if (total === null || online === null || stale === null) {
    return null;
  }
  return Math.max(0, total - online - stale);
}

function dashboardAgentStatusLabel(status: string | null | undefined): string {
  if (!status) {
    return "State unavailable";
  }
  if (status === "offline") {
    return "Offline";
  }
  if (status === "stale") {
    return "Stale";
  }
  if (status === "online") {
    return "Online";
  }
  return status.replace(/_/g, " ");
}

function countValue(value: number | null | undefined): string {
  const count = finiteCount(value);
  return count === null ? "No data" : String(count);
}

function countPhrase(value: number | null | undefined, unit: string): string {
  const count = finiteCount(value);
  if (count === null) {
    return `${capitalize(unit)} unavailable`;
  }
  return `${count} ${pluralize(unit, count)}`;
}

function finiteCount(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(0, Math.round(value))
    : null;
}

function pluralize(unit: string, count: number): string {
  if (count === 1) {
    return unit;
  }
  if (
    [
      "critical",
      "excluded",
      "failed",
      "offline",
      "online",
      "pending",
      "sampled",
      "stale",
      "VPS",
    ].includes(unit)
  ) {
    return unit;
  }
  return `${unit}s`;
}

function capitalize(value: string): string {
  return value ? `${value[0].toUpperCase()}${value.slice(1)}` : "Value";
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
    <div className="metricCard">
      <span>{label}</span>
      <strong>{value}</strong>
      <small>{detail}</small>
    </div>
  );
}

function WidgetHeader({ detail, title }: { detail: string; title: string }) {
  return (
    <div className="dashboardWidgetHeader">
      <strong>{title}</strong>
      <small>{detail}</small>
    </div>
  );
}

function WidgetEmpty({ label }: { label: string }) {
  return <div className="dashboardWidgetEmpty">{label}</div>;
}

function presetIcon(id: DashboardPresetId) {
  if (id === "resource_capacity") return <Gauge size={17} />;
  if (id === "network_traffic") return <GitBranch size={17} />;
  if (id === "group_posture") return <BarChart3 size={17} />;
  return <Activity size={17} />;
}

function buildDashboardExportPayload({
  overview,
  preferences,
  preset,
  range,
  scope,
}: {
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  preset: DashboardPreset;
  range: string;
  scope: string;
}) {
  return {
    exported_at: new Date().toISOString(),
    dashboard: {
      id: preset.id,
      label: preset.label,
      source: preset.source,
      widgets: preset.widgets,
    },
    scope,
    range,
    preferences,
    overview_summary: overview
      ? {
          generated_at: overview.generated_at,
          operations: overview.operations,
          resources: overview.resources,
          summary: overview.summary,
        }
      : null,
  };
}

function resourceChartData(series: DashboardResourceSeriesRecord[]): {
  lines: TimeSeriesChartLine[];
  times: string[];
} {
  const times = series[0]?.points.map((point) => point.bucket_start) ?? [];
  const lines = series.map((item, index) => ({
    color: dashboardChartColors[index % dashboardChartColors.length],
    label: item.label,
    values: item.points.map((point) => point.value),
  }));
  return { lines, times };
}

function resourceMetricTitle(metric: DashboardResourceMetric): string {
  if (metric === "memory_used") return "Memory";
  if (metric === "disk_free") return "Disk";
  return "CPU";
}

function formatResourceValue(
  metric: DashboardResourceMetric,
  value: number | null | undefined,
): string {
  if (value === null || value === undefined || Number.isNaN(value))
    return "No data";
  if (metric === "memory_used" || metric === "disk_free")
    return `${Math.round(value * 100)}%`;
  return value.toFixed(2);
}

function thresholdTone(
  series: DashboardResourceSeriesRecord,
): "critical" | "warning" | "ok" | "neutral" {
  const value = series.current;
  if (value === null) return "neutral";
  if (
    series.critical_threshold !== null &&
    thresholdReached(
      value,
      series.critical_threshold,
      series.threshold_direction,
    )
  )
    return "critical";
  if (
    series.warning_threshold !== null &&
    thresholdReached(
      value,
      series.warning_threshold,
      series.threshold_direction,
    )
  )
    return "warning";
  return "ok";
}

function thresholdReached(value: number, threshold: number, direction: string) {
  return direction === "below" ? value <= threshold : value >= threshold;
}

function thresholdLabel(series: DashboardResourceSeriesRecord): string {
  const tone = thresholdTone(series);
  if (tone === "critical") return "critical";
  if (tone === "warning") return "warning";
  if (tone === "neutral") return "missing";
  return "ok";
}

function formatBps(value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value))
    return "No data";
  if (value >= 1_000_000_000)
    return `${(value / 1_000_000_000).toFixed(1)} Gbps`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} Mbps`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} Kbps`;
  return `${Math.round(value)} bps`;
}

function formatBytes(value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value))
    return "No data";
  if (value >= 1_000_000_000_000)
    return `${(value / 1_000_000_000_000).toFixed(1)} TB`;
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)} GB`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} MB`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} KB`;
  return `${Math.round(value)} B`;
}

function scopeLabel(preferences: DashboardPreferences): string {
  if (preferences.scopeKind === "all") return "All VPS";
  return `${preferences.scopeKind}:${preferences.scopeValue || "unselected"}`;
}

function windowLabel(window: DashboardWindow): string {
  if (window === "15m") return "15 min";
  if (window === "1h") return "1 hour";
  if (window === "6h") return "6 hours";
  if (window === "24h") return "24 hours";
  if (window === "7d") return "7 days";
  if (window === "14d") return "14 days";
  if (window === "30d") return "30 days";
  return "All time";
}
