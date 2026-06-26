import { Activity, BarChart3, Copy, Download, Gauge, GitBranch, LayoutDashboard, RefreshCw, Server } from "lucide-react";
import { useMemo, useState } from "react";
import { ConsoleStatusBadge } from "../../components/ConsoleLayout";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../../components/TimeSeriesChart";
import { consolePalette, dashboardChartColors } from "../../colorPalette";
import type {
  DashboardLabelClusterRecord,
  DashboardNetworkRecord,
  DashboardOverviewRecord,
  DashboardPreferences,
  DashboardResourceMetric,
  DashboardResourceSeriesRecord,
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

type DashboardPresetId = "fleet_operations" | "resource_capacity" | "network_traffic" | "group_posture";

type DashboardPreset = {
  description: string;
  id: DashboardPresetId;
  label: string;
  source: string;
  widgets: string[];
};

const dashboardPresets: DashboardPreset[] = [
  {
    description: "Fleet posture, alert load, job activity, and backup health for operator handoff.",
    id: "fleet_operations",
    label: "Fleet operations",
    source: "Dashboard overview summary and operations API",
    widgets: ["summary cards", "recent alerts table", "degraded VPS table"],
  },
  {
    description: "CPU, memory, or disk trends using the operator's current dashboard metric preference.",
    id: "resource_capacity",
    label: "Resource capacity",
    source: "Dashboard resource curve API",
    widgets: ["resource chart", "top VPS table", "threshold status cards"],
  },
  {
    description: "Fleet ingress/egress speed, traffic totals, and busiest network endpoints.",
    id: "network_traffic",
    label: "Network traffic",
    source: "Dashboard network curve API",
    widgets: ["network speed chart", "traffic chart", "top network VPS table"],
  },
  {
    description: "Provider, tag, country, status, and client group rollups for scan and comparison work.",
    id: "group_posture",
    label: "Group posture",
    source: "Dashboard label cluster API",
    widgets: ["group cards", "warning counts", "scope metadata"],
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
  const [selectedId, setSelectedId] = useState<DashboardPresetId>("fleet_operations");
  const [status, setStatus] = useState<string | null>(null);
  const selectedPreset = dashboardPresets.find((preset) => preset.id === selectedId) ?? dashboardPresets[0];
  const dashboardScope = overview?.scope.label ?? scopeLabel(preferences);
  const dashboardRange = overview
    ? `${formatCompactTime(overview.time_range.start_at)} - ${formatCompactTime(overview.time_range.end_at)}`
    : `${windowLabel(window)} window`;
  const widgetCount = selectedPreset.widgets.length;
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
            <span>Saved read-only dashboard views assembled from the dashboard overview preference model.</span>
          </div>
          <div className="sectionActions" aria-label="Dashboard manager actions">
            <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
              <RefreshCw size={14} />
              <span>{loading ? "Refreshing" : "Refresh"}</span>
            </button>
            <button className="secondaryAction compactAction" onClick={() => void copyShareLink()} type="button">
              <Copy size={14} />
              <span>Share link</span>
            </button>
            <button className="secondaryAction compactAction" onClick={exportDashboard} type="button">
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

        <div className="metricGrid observabilityMetricsSummary" aria-label="Dashboard manager summary">
          <MetricTile detail="Generated from current overview-compatible presets" label="Saved dashboards" value={String(dashboardPresets.length)} />
          <MetricTile detail={selectedPreset.source} label="Active dashboard" value={selectedPreset.label} />
          <MetricTile detail={`${dashboardScope}; ${dashboardRange}`} label="Scope and range" value={windowLabel(window)} />
          <MetricTile detail="Charts, tables, and posture cards; no mutation controls" label="Widgets" value={String(widgetCount)} />
        </div>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="saved-dashboards-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="saved-dashboards-title">Saved dashboards</h2>
              <span>Preset layouts are read-only until a saved-dashboard API exists.</span>
            </div>
            <ConsoleStatusBadge tone="info">Read-only</ConsoleStatusBadge>
          </div>
          <div className="savedDashboardGrid" aria-label="Saved dashboard registry">
            {dashboardPresets.map((preset) => (
              <button
                aria-pressed={selectedPreset.id === preset.id}
                className={selectedPreset.id === preset.id ? "savedDashboardTile active" : "savedDashboardTile"}
                key={preset.id}
                onClick={() => setSelectedId(preset.id)}
                type="button"
              >
                <span className="savedDashboardIcon">
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

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="dashboard-widget-layout-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-widget-layout-title">Widget layout</h2>
              <span>{selectedPreset.label}: {selectedPreset.widgets.join(", ")}.</span>
            </div>
            <div className="sectionActions" aria-label="Dashboard drilldown links">
              <button className="secondaryAction compactAction" onClick={onOpenFleetMetrics} type="button">
                <Gauge size={14} />
                Fleet metrics
              </button>
              <button className="secondaryAction compactAction" onClick={onOpenNetworkMetrics} type="button">
                <GitBranch size={14} />
                Network metrics
              </button>
            </div>
          </div>
          <DashboardWidgetLayout
            overview={overview}
            preferences={preferences}
            selectedId={selectedPreset.id}
          />
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="dashboard-handoff-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="dashboard-handoff-title">Share and export</h2>
              <span>Dashboard handoff is metadata-only; it does not create jobs, alerts, backups, terminals, or config mutations.</span>
            </div>
          </div>
          <div className="dashboardHandoffPanel" aria-label="Dashboard share and export details">
            <span>
              <strong>Share scope</strong>
              <small>{dashboardScope}</small>
            </span>
            <span>
              <strong>Export format</strong>
              <small>JSON manifest with current dashboard id, widgets, preferences, and overview counts.</small>
            </span>
            <span>
              <strong>Mutation boundary</strong>
              <small>Refresh, share, export, and navigation only. No privileged operation controls render here.</small>
            </span>
          </div>
          {status ? <small className="dashboardManagerStatus">{status}</small> : null}
        </section>
      </div>
    </section>
  );
}

function DashboardWidgetLayout({
  overview,
  preferences,
  selectedId,
}: {
  overview: DashboardOverviewRecord | null;
  preferences: DashboardPreferences;
  selectedId: DashboardPresetId;
}) {
  if (!overview) {
    return (
      <div className="emptyState compactEmpty">
        <LayoutDashboard size={18} />
        <strong>No dashboard overview</strong>
        <span>Refresh the dashboard overview API to populate saved dashboard widgets.</span>
      </div>
    );
  }
  if (selectedId === "resource_capacity") {
    return <ResourceDashboard overview={overview} preferences={preferences} />;
  }
  if (selectedId === "network_traffic") {
    return <NetworkDashboard network={overview.network} />;
  }
  if (selectedId === "group_posture") {
    return <GroupDashboard clusters={overview.label_clusters} />;
  }
  return <FleetOperationsDashboard overview={overview} />;
}

function FleetOperationsDashboard({ overview }: { overview: DashboardOverviewRecord }) {
  const operations = overview.operations;
  return (
    <div className="dashboardWidgetGrid" aria-label="Fleet operations dashboard widgets">
      <MetricTile detail={`${overview.summary.stale} stale, ${overview.summary.offline} offline`} label="Fleet health" value={`${overview.summary.online}/${overview.summary.total}`} />
      <MetricTile detail={`${operations.critical_alerts} critical, ${operations.warning_alerts} warning`} label="Active alerts" value={String(operations.active_alerts)} />
      <MetricTile detail="Pending or running jobs in loaded range" label="Running jobs" value={String(operations.running_jobs)} />
      <MetricTile detail={`${operations.backup_failed} failed, ${operations.backup_pending} pending`} label="Backup posture" value={String(operations.backup_completed)} />
      <div className="dashboardWidgetTable wideWidget" aria-label="Recent alert widget table">
        <WidgetHeader title="Recent alerts" detail={`${operations.recent_alerts.length} retained rows`} />
        {operations.recent_alerts.slice(0, 8).map((alert) => (
          <div className="dashboardWidgetRow" key={alert.id}>
            <ConsoleStatusBadge tone={alert.severity === "critical" ? "critical" : "warning"}>{alert.severity}</ConsoleStatusBadge>
            <strong>{alert.title}</strong>
            <span>{alert.client_label ?? "fleet"}</span>
            <small>{formatCompactTime(alert.observed_at)}</small>
          </div>
        ))}
        {!operations.recent_alerts.length ? <WidgetEmpty label="No retained alerts" /> : null}
      </div>
      <div className="dashboardWidgetTable" aria-label="Degraded VPS widget table">
        <WidgetHeader title="Degraded VPS" detail={`${operations.degraded_agents.length} records`} />
        {operations.degraded_agents.slice(0, 8).map((agent) => (
          <div className="dashboardWidgetRow" key={agent.client_id}>
            <ConsoleStatusBadge tone="warning">{agent.status}</ConsoleStatusBadge>
            <strong>{agent.label}</strong>
            <span>{agent.tags.slice(0, 3).join(", ") || "no tags"}</span>
          </div>
        ))}
        {!operations.degraded_agents.length ? <WidgetEmpty label="No degraded VPS rows" /> : null}
      </div>
    </div>
  );
}

function ResourceDashboard({
  overview,
  preferences,
}: {
  overview: DashboardOverviewRecord;
  preferences: DashboardPreferences;
}) {
  const chart = resourceChartData(overview.resource_curve.series);
  return (
    <div className="dashboardWidgetGrid" aria-label="Resource capacity dashboard widgets">
      <MetricTile detail={`${overview.resource_curve.sampled_clients} sampled, ${overview.resource_curve.excluded_clients} excluded`} label="Metric" value={resourceMetricTitle(preferences.resourceMetric)} />
      <MetricTile detail="Average across sampled VPSs" label="CPU load" value={formatResourceValue("cpu_load", overview.resources.cpu_load_avg)} />
      <MetricTile detail="Average used memory ratio" label="Memory used" value={formatResourceValue("memory_used", overview.resources.memory_used_ratio)} />
      <MetricTile detail="Average free disk ratio" label="Disk free" value={formatResourceValue("disk_free", overview.resources.disk_free_ratio)} />
      <div className="dashboardWidgetChart wideWidget" aria-label="Resource capacity chart widget">
        <WidgetHeader title={`${resourceMetricTitle(preferences.resourceMetric)} chart`} detail={`${overview.resource_curve.series.length} VPS series`} />
        <TimeSeriesChart
          ariaLabel="Saved dashboard resource chart"
          emptyLabel="No resource curve data"
          lines={chart.lines}
          times={chart.times}
          valueFormatter={(value) => formatResourceValue(preferences.resourceMetric, value)}
        />
      </div>
      <div className="dashboardWidgetTable" aria-label="Resource capacity top VPS table">
        <WidgetHeader title="Top resource VPS" detail={`${overview.resource_curve.top_limit} limit`} />
        {overview.resource_curve.series.slice(0, 8).map((series) => (
          <div className="dashboardWidgetRow" key={series.client_id}>
            <ConsoleStatusBadge tone={thresholdTone(series)}>{thresholdLabel(series)}</ConsoleStatusBadge>
            <strong>{series.label}</strong>
            <span>{formatResourceValue(preferences.resourceMetric, series.current)}</span>
            <small>Peak {formatResourceValue(preferences.resourceMetric, series.peak)}</small>
          </div>
        ))}
        {!overview.resource_curve.series.length ? <WidgetEmpty label="No resource rows" /> : null}
      </div>
    </div>
  );
}

function NetworkDashboard({ network }: { network: DashboardNetworkRecord }) {
  const speedLines: TimeSeriesChartLine[] = [
    { color: consolePalette.chart.blue, label: "Inbound speed", values: network.points.map((point) => point.rx_bps) },
    { color: consolePalette.chart.green, label: "Outbound speed", values: network.points.map((point) => point.tx_bps) },
  ];
  const trafficLines: TimeSeriesChartLine[] = [
    { color: consolePalette.chart.purple, label: "Inbound traffic", values: network.traffic_points.map((point) => point.rx_bytes) },
    { color: consolePalette.chart.orange, label: "Outbound traffic", values: network.traffic_points.map((point) => point.tx_bytes) },
  ];
  return (
    <div className="dashboardWidgetGrid" aria-label="Network traffic dashboard widgets">
      <MetricTile detail="Current retained ingress" label="RX speed" value={formatBps(network.rx_bps)} />
      <MetricTile detail="Current retained egress" label="TX speed" value={formatBps(network.tx_bps)} />
      <MetricTile detail="Top speed rows" label="Network VPS" value={String(network.top_clients.length)} />
      <MetricTile detail="Top traffic rows" label="Traffic VPS" value={String(network.traffic_top_clients.length)} />
      <div className="dashboardWidgetChart wideWidget" aria-label="Network speed chart widget">
        <WidgetHeader title="Network speed chart" detail="Ingress and egress bps over time" />
        <TimeSeriesChart
          ariaLabel="Saved dashboard network speed chart"
          emptyLabel="No network speed points"
          lines={speedLines}
          times={network.points.map((point) => point.bucket_start)}
          valueFormatter={(value) => formatBps(value)}
        />
      </div>
      <div className="dashboardWidgetChart wideWidget" aria-label="Network traffic chart widget">
        <WidgetHeader title="Network traffic chart" detail="Traffic totals over time" />
        <TimeSeriesChart
          ariaLabel="Saved dashboard network traffic chart"
          emptyLabel="No network traffic points"
          lines={trafficLines}
          times={network.traffic_points.map((point) => point.bucket_start)}
          valueFormatter={(value) => formatBytes(value)}
        />
      </div>
      <div className="dashboardWidgetTable wideWidget" aria-label="Top network VPS widget table">
        <WidgetHeader title="Top network VPS" detail={`${network.top_clients.length} speed rows`} />
        {network.top_clients.slice(0, 8).map((client) => (
          <div className="dashboardWidgetRow" key={client.client_id}>
            <ConsoleStatusBadge tone="info">{client.interfaces.length} interfaces</ConsoleStatusBadge>
            <strong>{client.label}</strong>
            <span>{formatBps(client.rx_bps)} in</span>
            <small>{formatBps(client.tx_bps)} out</small>
          </div>
        ))}
        {!network.top_clients.length ? <WidgetEmpty label="No network rows" /> : null}
      </div>
    </div>
  );
}

function GroupDashboard({ clusters }: { clusters: DashboardLabelClusterRecord[] }) {
  return (
    <div className="groupDashboardGrid" aria-label="Group posture dashboard widgets">
      {clusters.map((cluster, index) => (
        <div className="groupDashboardTile" key={`${cluster.kind}:${cluster.label}`}>
          <span>{cluster.kind}</span>
          <strong>{cluster.label}</strong>
          <small>{cluster.total} VPS; {cluster.online} online; {cluster.stale} stale</small>
          <dl>
            <div>
              <dt>Warnings</dt>
              <dd>{cluster.warnings}</dd>
            </div>
            <div>
              <dt>Jobs</dt>
              <dd>{cluster.running_jobs}</dd>
            </div>
            <div>
              <dt>Traffic</dt>
              <dd>{formatBps(cluster.rx_bps + cluster.tx_bps)}</dd>
            </div>
          </dl>
          <i style={{ backgroundColor: dashboardChartColors[index % dashboardChartColors.length] }} aria-hidden="true" />
        </div>
      ))}
      {!clusters.length ? (
        <div className="emptyState compactEmpty">
          <Server size={18} />
          <strong>No group widgets</strong>
          <span>Group posture appears after dashboard overview data is available.</span>
        </div>
      ) : null}
    </div>
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

function resourceChartData(series: DashboardResourceSeriesRecord[]): { lines: TimeSeriesChartLine[]; times: string[] } {
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

function formatResourceValue(metric: DashboardResourceMetric, value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value)) return "-";
  if (metric === "memory_used" || metric === "disk_free") return `${Math.round(value * 100)}%`;
  return value.toFixed(2);
}

function thresholdTone(series: DashboardResourceSeriesRecord): "critical" | "warning" | "ok" | "neutral" {
  const value = series.current;
  if (value === null) return "neutral";
  if (series.critical_threshold !== null && thresholdReached(value, series.critical_threshold, series.threshold_direction)) return "critical";
  if (series.warning_threshold !== null && thresholdReached(value, series.warning_threshold, series.threshold_direction)) return "warning";
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
  if (value === null || value === undefined || Number.isNaN(value)) return "-";
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)} Gbps`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} Mbps`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} Kbps`;
  return `${Math.round(value)} bps`;
}

function formatBytes(value: number | null | undefined): string {
  if (value === null || value === undefined || Number.isNaN(value)) return "-";
  if (value >= 1_000_000_000_000) return `${(value / 1_000_000_000_000).toFixed(1)} TB`;
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
