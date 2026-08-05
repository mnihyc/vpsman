import { RefreshCw } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { apiGet } from "../api";
import { consolePalette, dashboardChartColors } from "../colorPalette";
import { ActionFeedback } from "../components/ActionFeedback";
import {
  MonitoringRangeTabs,
  type MonitoringWindow,
} from "../components/MonitoringRangeTabs";
import { ConsoleStatusBadge } from "../components/ConsoleLayout";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../components/TimeSeriesChart";
import type {
  AgentView,
  PingRollupView,
  TelemetryNetworkRateRecord,
  TelemetryRollupRecord,
  TrafficAccountingRecord,
  TrafficHistoryPointView,
} from "../types";
import { formatTime, timestampMillis } from "../utils";
import { useHistoryEntryState } from "../historyEntryState";
import {
  formatByteCount as formatBytes,
  formatByteRateFromBitsPerSecond,
} from "../telemetryMetrics";

type MonitoringSection = "resources" | "ping";
type PingMetric = "latency" | "loss";

type MonitoringRange = {
  window: string;
  source: string;
  start_unix: number;
  end_unix: number;
  step_secs: number;
  points: number;
};

type CurrentPing = {
  target_id: string;
  target_name: string;
  enabled: boolean;
  generation: number;
  state: string;
  status: string | null;
  latency_avg_ms: number | null;
  loss_ratio: number | null;
  reason: string | null;
  checked_at: string | null;
};

type ClientMonitoringResponse = {
  client: AgentView;
  range: MonitoringRange;
  resources: TelemetryRollupRecord[];
  network: TelemetryNetworkRateRecord[];
  traffic: TrafficAccountingRecord;
  traffic_history: TrafficHistoryPointView[];
  ping_targets: CurrentPing[];
  ping: PingRollupView[];
  primary_ping: CurrentPing | null;
};

type Timeline = {
  epochs: number[];
  times: string[];
};

type ChartData = {
  lines: TimeSeriesChartLine[];
  times: string[];
};

const CHART_POINTS = 720;

export function VpsMonitoringDetailPanel({
  apiToken,
  clientId,
  section: forcedSection,
}: {
  apiToken: string;
  clientId: string;
  section?: MonitoringSection;
}) {
  const [window, setWindow] = useHistoryEntryState<MonitoringWindow>(
    `vps.monitoring.${clientId}.window`,
    "15m",
  );
  const [internalSection, setInternalSection] =
    useState<MonitoringSection>("resources");
  const section = forcedSection ?? internalSection;
  const [pingMetric, setPingMetric] = useHistoryEntryState<PingMetric>(
    `vps.monitoring.${clientId}.ping-metric`,
    "latency",
  );
  const initialCustomRange = useMemo(() => {
    const end = new Date();
    const start = new Date(end.getTime() - 24 * 60 * 60 * 1_000);
    return { end: toDateTimeLocal(end), start: toDateTimeLocal(start) };
  }, []);
  const [customStart, setCustomStart] = useHistoryEntryState(
    `vps.monitoring.${clientId}.custom-start`,
    initialCustomRange.start,
  );
  const [customEnd, setCustomEnd] = useHistoryEntryState(
    `vps.monitoring.${clientId}.custom-end`,
    initialCustomRange.end,
  );
  const [appliedCustomStart, setAppliedCustomStart] = useHistoryEntryState(
    `vps.monitoring.${clientId}.applied-custom-start`,
    initialCustomRange.start,
  );
  const [appliedCustomEnd, setAppliedCustomEnd] = useHistoryEntryState(
    `vps.monitoring.${clientId}.applied-custom-end`,
    initialCustomRange.end,
  );
  const [customError, setCustomError] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [data, setData] = useState<ClientMonitoringResponse | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const query = useMemo(
    () => monitoringQuery(window, appliedCustomStart, appliedCustomEnd),
    [appliedCustomEnd, appliedCustomStart, window],
  );

  useEffect(() => {
    let active = true;
    setData(null);
    setError(null);
    if (!clientId.trim() || !apiToken || !query.path) {
      setLoading(false);
      if (query.error) setError(query.error);
      return () => {
        active = false;
      };
    }

    let inFlight = false;
    const load = async () => {
      if (inFlight) return;
      inFlight = true;
      try {
        const response = await apiGet<ClientMonitoringResponse>(
          `/api/v1/clients/${encodeURIComponent(clientId)}/monitoring?${query.path}`,
          apiToken,
        );
        if (!active) return;
        setData(response);
        setError(null);
      } catch (cause) {
        if (!active) return;
        setError(errorMessage(cause));
      } finally {
        inFlight = false;
        if (active) setLoading(false);
      }
    };
    setLoading(true);
    void load();
    const refreshTimer =
      window === "15m"
        ? globalThis.setInterval(() => void load(), 60_000)
        : null;

    return () => {
      active = false;
      if (refreshTimer !== null) globalThis.clearInterval(refreshTimer);
    };
  }, [apiToken, clientId, query.error, query.path, refreshKey, window]);

  function selectWindow(next: MonitoringWindow) {
    setWindow(next);
  }

  function applyCustomRange() {
    const start = Date.parse(customStart);
    const end = Date.parse(customEnd);
    const maximumSpan = 3_650 * 24 * 60 * 60 * 1_000;
    if (!Number.isFinite(start) || !Number.isFinite(end)) {
      setCustomError("Choose valid start and end times.");
      return;
    }
    if (start >= end) {
      setCustomError("Custom monitoring start must be before end.");
      return;
    }
    if (end - start > maximumSpan) {
      setCustomError(
        "Custom monitoring range cannot exceed retained 10-year history.",
      );
      return;
    }
    setCustomError(null);
    setAppliedCustomStart(customStart);
    setAppliedCustomEnd(customEnd);
  }

  return (
    <section
      aria-busy={loading}
      aria-label={`Monitoring history for ${data?.client.display_name || clientId}`}
      className="vpsMonitoringDetailPanel"
    >
      <div className="dashboardToolbar vpsMonitoringToolbar">
        <div>
          <h2>Monitoring history</h2>
          <span>
            {data
              ? `${data.client.display_name || data.client.id} · ${rangeEvidence(data.range)}`
              : `Resource, network, traffic, and Ping evidence for ${clientId}`}
          </span>
        </div>
        <div className="dashboardToolbarActions">
          <MonitoringRangeTabs
            ariaLabel="VPS monitoring time range"
            className="vpsMonitoringRangeTabs"
            onChange={selectWindow}
            value={window}
          />
          <button
            className="secondaryAction compactAction"
            disabled={loading || Boolean(query.error)}
            onClick={() => setRefreshKey((current) => current + 1)}
            type="button"
          >
            <RefreshCw size={15} />
            <span>{loading ? "Loading" : "Refresh"}</span>
          </button>
        </div>
      </div>

      {window === "custom" ? (
        <div
          aria-label="Custom VPS monitoring time range"
          className="dashboardControlBar vpsMonitoringCustomRange"
        >
          <label>
            <span>Start</span>
            <input
              aria-label="Custom monitoring start"
              id="vps-monitoring-custom-start"
              max={customEnd || undefined}
              name="vps-monitoring-custom-start"
              onChange={(event) => {
                setCustomError(null);
                setCustomStart(event.target.value);
              }}
              type="datetime-local"
              value={customStart}
            />
          </label>
          <label>
            <span>End</span>
            <input
              aria-label="Custom monitoring end"
              id="vps-monitoring-custom-end"
              min={customStart || undefined}
              name="vps-monitoring-custom-end"
              onChange={(event) => {
                setCustomError(null);
                setCustomEnd(event.target.value);
              }}
              type="datetime-local"
              value={customEnd}
            />
          </label>
          <span className="dashboardScopeHint">
            Custom history is bounded by retained monitoring data.
          </span>
          <button
            className="secondaryAction compactAction"
            disabled={
              customStart === appliedCustomStart &&
              customEnd === appliedCustomEnd
            }
            onClick={applyCustomRange}
            type="button"
          >
            Apply range
          </button>
          {customError ? (
            <span className="fieldError" role="alert">
              {customError}
            </span>
          ) : null}
        </div>
      ) : null}

      <ActionFeedback
        className="localActionFeedback dashboardActionFeedback vpsMonitoringFeedback"
        message={error}
        tone="danger"
      />

      {!forcedSection ? (
        <div
          aria-label="VPS monitoring section"
          className="dashboardSectionSelector vpsMonitoringSectionSelector"
        >
          <button
            aria-pressed={section === "resources"}
            className={section === "resources" ? "active" : ""}
            onClick={() => setInternalSection("resources")}
            type="button"
          >
            <strong>Resources</strong>
            <small>CPU, memory, disk, network, and traffic</small>
          </button>
          <button
            aria-pressed={section === "ping"}
            className={section === "ping" ? "active" : ""}
            onClick={() => setInternalSection("ping")}
            type="button"
          >
            <strong>Ping</strong>
            <small>Assigned target latency, loss, and gaps</small>
          </button>
        </div>
      ) : null}

      {!data && loading ? (
        <div className="dashboardEmptyChart vpsMonitoringLoading">
          Loading monitoring evidence…
        </div>
      ) : null}

      {data && section === "resources" ? <ResourceHistory data={data} /> : null}
      {data && section === "ping" ? (
        <PingHistory
          data={data}
          metric={pingMetric}
          onMetricChange={setPingMetric}
        />
      ) : null}
    </section>
  );
}

function ResourceHistory({ data }: { data: ClientMonitoringResponse }) {
  const timeline = useMemo(() => buildTimeline(data.range), [data.range]);
  const resources = useMemo(
    () => resourceCharts(data.resources, timeline, data.range.step_secs),
    [data.range.step_secs, data.resources, timeline],
  );
  const network = useMemo(
    () => networkChart(data.network, timeline, data.range.step_secs),
    [data.network, data.range.step_secs, timeline],
  );
  const trafficHistory = useMemo(
    () => trafficChart(data.traffic_history, timeline, data.range.step_secs),
    [data.range.step_secs, data.traffic_history, timeline],
  );
  const trafficResets = data.traffic_history.reduce(
    (sum, row) => sum + Math.max(0, row.reset_count),
    0,
  );
  const sampleCount = data.resources.reduce(
    (sum, row) => sum + Math.max(0, row.sample_count),
    0,
  );
  const cpuSampleCount = data.resources.reduce(
    (sum, row) => sum + Math.max(0, row.cpu_usage_sample_count),
    0,
  );
  const interfaces = new Set(data.network.map((row) => row.interface)).size;

  return (
    <section
      aria-labelledby="vps-monitoring-resources-title"
      className="dashboardSection vpsMonitoringSection"
    >
      <div className="dashboardSectionHeader">
        <div>
          <h2 id="vps-monitoring-resources-title">Resource history</h2>
          <span>
            {sampleCount} contributing samples · {cpuSampleCount} real CPU
            samples · {interfaces} network{" "}
            {interfaces === 1 ? "interface" : "interfaces"}
          </span>
        </div>
        <ConsoleStatusBadge tone={data.resources.length ? "info" : "neutral"}>
          {data.resources.length
            ? `${data.resources.length} resource points`
            : "No resource data"}
        </ConsoleStatusBadge>
      </div>
      <div className="dashboardWidgetGrid vpsMonitoringChartGrid">
        <MonitoringChart
          data={resources.cpu}
          emptyLabel="CPU utilization is unavailable for this range"
          exportFileName={`${safeFilePart(data.client.id)}-cpu-${data.range.window}`}
          title="CPU utilization"
          valueFormatter={formatPercent}
        />
        <MonitoringChart
          data={resources.load}
          emptyLabel="Load history is unavailable for this range"
          exportFileName={`${safeFilePart(data.client.id)}-load-${data.range.window}`}
          title="Load 1 / 5 / 15"
          valueFormatter={formatLoad}
        />
        <MonitoringChart
          data={resources.memory}
          emptyLabel="Memory history is unavailable for this range"
          exportFileName={`${safeFilePart(data.client.id)}-memory-${data.range.window}`}
          title="Memory used"
          valueFormatter={formatPercent}
        />
        <MonitoringChart
          data={resources.disk}
          detail="Aggregate reported-filesystem semantics"
          emptyLabel="Aggregate disk history is unavailable for this range"
          exportFileName={`${safeFilePart(data.client.id)}-disk-${data.range.window}`}
          title="Disk used"
          valueFormatter={formatPercent}
        />
        <MonitoringChart
          data={resources.connections}
          detail="Agent-observed socket counts; unavailable samples remain gaps"
          emptyLabel="TCP and UDP connection history is unavailable for this range"
          exportFileName={`${safeFilePart(data.client.id)}-connections-${data.range.window}`}
          title="TCP / UDP connections"
          valueFormatter={formatConnectionCount}
        />
        <MonitoringChart
          className="wideWidget"
          data={network}
          detail={`${interfaces} coherent network ${interfaces === 1 ? "interface" : "interfaces"}, aggregated by direction`}
          emptyLabel="Network rate history is unavailable for this range"
          exportFileName={`${safeFilePart(data.client.id)}-network-${data.range.window}`}
          title="Network RX / TX"
          valueFormatter={formatByteRateFromBitsPerSecond}
        />
        <MonitoringChart
          className="wideWidget"
          data={trafficHistory}
          detail={
            trafficResets
              ? `RX and TX shown by default; select Total volume to compare their sum · ${trafficResets} counter reset ${trafficResets === 1 ? "interval" : "intervals"} preserved as incomplete evidence`
              : "RX and TX shown by default; select Total volume to compare their sum · only interfaces selected by authoritative traffic-accounting rules"
          }
          emptyLabel={
            trafficConfigured(data.traffic)
              ? "Traffic volume history is unavailable for this range"
              : "Traffic unconfigured"
          }
          exportFileName={`${safeFilePart(data.client.id)}-traffic-${data.range.window}`}
          title="Traffic volume"
          valueFormatter={formatBytesNullable}
        />
        <TrafficCycle clientId={data.client.id} traffic={data.traffic} />
      </div>
    </section>
  );
}

function PingHistory({
  data,
  metric,
  onMetricChange,
}: {
  data: ClientMonitoringResponse;
  metric: PingMetric;
  onMetricChange: (metric: PingMetric) => void;
}) {
  const currentGenerationRows = useMemo(
    () => currentPingGenerationRows(data.ping, data.ping_targets),
    [data.ping, data.ping_targets],
  );
  const targetSummaries = useMemo(
    () =>
      pingTargetSummaries(
        currentGenerationRows,
        data.ping_targets,
        data.primary_ping,
      ),
    [currentGenerationRows, data.ping_targets, data.primary_ping],
  );
  const chart = useMemo(
    () => pingChart(currentGenerationRows, data.range, metric),
    [currentGenerationRows, data.range, metric],
  );
  const samples = currentGenerationRows.reduce(
    (sum, row) => sum + Math.max(0, row.sample_count),
    0,
  );
  const successes = currentGenerationRows.reduce(
    (sum, row) => sum + Math.max(0, row.success_count),
    0,
  );

  return (
    <section
      aria-labelledby="vps-monitoring-ping-title"
      className="dashboardSection vpsMonitoringSection"
    >
      <div className="dashboardSectionHeader">
        <div>
          <h2 id="vps-monitoring-ping-title">Ping history</h2>
          <span>
            {targetSummaries.length} visible Ping{" "}
            {targetSummaries.length === 1 ? "target" : "targets"} · {successes}/
            {samples} successful probes in the selected range
          </span>
        </div>
        <div
          aria-label="Ping chart metric"
          className="segmented vpsMonitoringPingMetric"
          role="group"
        >
          <button
            aria-pressed={metric === "latency"}
            className={metric === "latency" ? "selected" : ""}
            onClick={() => onMetricChange("latency")}
            type="button"
          >
            Latency
          </button>
          <button
            aria-pressed={metric === "loss"}
            className={metric === "loss" ? "selected" : ""}
            onClick={() => onMetricChange("loss")}
            type="button"
          >
            Loss
          </button>
        </div>
      </div>

      <div
        aria-label="Current Ping target evidence"
        className="vpsMonitoringPingTargets"
      >
        {targetSummaries.map((target) => (
          <div
            className="vpsMonitoringPingTarget"
            key={`${target.targetId}:${target.generation}`}
            title={target.reason || undefined}
          >
            <span>
              <i style={{ background: target.color }} />
              <strong title={target.name}>{target.name}</strong>
              {target.primary ? <em>Primary</em> : null}
            </span>
            <ConsoleStatusBadge tone={pingStatusTone(target.status)}>
              {pingStatusLabel(target.status)}
            </ConsoleStatusBadge>
            <small>
              {formatMilliseconds(target.latency)} · {formatLoss(target.loss)}
            </small>
          </div>
        ))}
        {!targetSummaries.length ? (
          <p className="dashboardEmptyChart">
            No assigned Ping target history is available for this VPS. Assign a
            target in Observability / Ping targets.
          </p>
        ) : null}
      </div>

      <div className="dashboardWidgetGrid vpsMonitoringPingChartGrid">
        <MonitoringChart
          className="wideWidget"
          data={chart}
          detail="Each target keeps a stable color; missing buckets remain visible gaps"
          emptyLabel={`No Ping ${metric} evidence is available for this range`}
          exportFileName={`${safeFilePart(data.client.id)}-ping-${metric}-${data.range.window}`}
          title={metric === "latency" ? "Ping latency" : "Ping packet loss"}
          valueFormatter={
            metric === "latency" ? formatMilliseconds : formatPercent
          }
        />
      </div>
    </section>
  );
}

function MonitoringChart({
  className,
  data,
  detail,
  emptyLabel,
  exportFileName,
  title,
  valueFormatter,
}: {
  className?: string;
  data: ChartData;
  detail?: string;
  emptyLabel: string;
  exportFileName: string;
  title: string;
  valueFormatter: (value: number | null) => string;
}) {
  return (
    <div className={`dashboardWidgetChart${className ? ` ${className}` : ""}`}>
      <div className="dashboardWidgetHeader">
        <strong>{title}</strong>
        {detail ? <small>{detail}</small> : null}
      </div>
      <TimeSeriesChart
        ariaLabel={`${title} monitoring history`}
        emptyLabel={emptyLabel}
        exportFileName={exportFileName}
        lines={data.lines}
        times={data.times}
        valueFormatter={valueFormatter}
      />
    </div>
  );
}

function TrafficCycle({
  clientId,
  traffic,
}: {
  clientId: string;
  traffic: TrafficAccountingRecord;
}) {
  const configured = trafficConfigured(traffic);
  const percent = finiteNumber(traffic.cycle_percent);
  const width = percent === null ? 0 : Math.min(100, Math.max(0, percent));
  const overQuota = percent !== null && percent > 100;

  return (
    <div className="dashboardWidgetChart wideWidget vpsMonitoringTrafficCycle">
      <div className="dashboardWidgetHeader">
        <strong>Traffic volume / cycle</strong>
        <small>
          {configured
            ? `${formatTime(traffic.cycle_start)} – ${formatTime(traffic.cycle_end)}`
            : "Authoritative traffic accounting"}
        </small>
      </div>
      {!configured ? (
        <div className="dashboardEmptyChart">Traffic unconfigured</div>
      ) : (
        <>
          <div className="vpsMonitoringTrafficSummary">
            <span>
              <small>Counted RX</small>
              <strong>{formatBytes(traffic.rx_bytes)}</strong>
              <em>{quotaDetail(traffic.rx_bytes, traffic.quota_rx_bytes)}</em>
            </span>
            <span>
              <small>Counted TX</small>
              <strong>{formatBytes(traffic.tx_bytes)}</strong>
              <em>{quotaDetail(traffic.tx_bytes, traffic.quota_tx_bytes)}</em>
            </span>
            <span>
              <small>Counted total</small>
              <strong>{formatBytes(traffic.total_bytes)}</strong>
              <em>
                {quotaDetail(traffic.total_bytes, traffic.quota_total_bytes)}
              </em>
            </span>
            <span>
              <small>Cycle ends</small>
              <strong>{formatTime(traffic.cycle_end)}</strong>
              <em>
                {traffic.state === "ok"
                  ? "Current accounting evidence"
                  : humanizeMonitoringState(traffic.state)}
              </em>
            </span>
          </div>
          {percent !== null ? (
            <div
              className={`vpsMonitoringTrafficProgress${overQuota ? " overLimit" : ""}`}
            >
              <span
                aria-label={`${formatPercent(percent)} of the limiting traffic quota used`}
                aria-valuemax={100}
                aria-valuemin={0}
                aria-valuenow={width}
                aria-valuetext={formatPercent(percent)}
                className="vpsMonitoringTrafficTrack"
                role="progressbar"
              >
                <i style={{ width: `${width}%` }} />
              </span>
              <strong>{formatPercent(percent)}</strong>
              <small>
                {overQuota ? "Quota exceeded" : "Limiting quota used"}
              </small>
            </div>
          ) : (
            <p className="vpsMonitoringTrafficNote incomplete">
              Traffic is accounted for, but no quota is configured.
            </p>
          )}
          {traffic.incomplete_reasons.length ? (
            <p className="vpsMonitoringTrafficNote">
              Incomplete: {traffic.incomplete_reasons.join("; ")}
            </p>
          ) : null}
        </>
      )}
      <span className="srOnly">Traffic accounting for {clientId}</span>
    </div>
  );
}

function trafficConfigured(traffic: TrafficAccountingRecord): boolean {
  return traffic.selectors.length > 0 && traffic.reset_day !== null;
}

function resourceCharts(
  rows: TelemetryRollupRecord[],
  timeline: Timeline,
  stepSecs: number,
): {
  connections: ChartData;
  cpu: ChartData;
  disk: ChartData;
  load: ChartData;
  memory: ChartData;
} {
  const bucketed = latestRowsByEpoch(rows, stepSecs);
  const times = timeline.times;
  return {
    connections: {
      times,
      lines: [
        resourceLine(
          "TCP",
          consolePalette.chart.purple,
          timeline,
          bucketed,
          (row) => finiteNumber(row.tcp_sockets_latest),
        ),
        resourceLine(
          "UDP",
          consolePalette.chart.green,
          timeline,
          bucketed,
          (row) => finiteNumber(row.udp_sockets_latest),
        ),
      ],
    },
    cpu: {
      times,
      lines: [
        {
          color: consolePalette.chart.blue,
          label: "CPU used",
          values: timeline.epochs.map((epoch) => {
            const value = finiteNumber(bucketed.get(epoch)?.cpu_usage_avg);
            return value === null ? null : value * 100;
          }),
        },
      ],
    },
    load: {
      times,
      lines: [
        resourceLine(
          "Load 1",
          consolePalette.chart.blue,
          timeline,
          bucketed,
          (row) => row.cpu_load_1_avg,
        ),
        resourceLine(
          "Load 5",
          consolePalette.chart.orange,
          timeline,
          bucketed,
          (row) => row.cpu_load_5_avg,
        ),
        resourceLine(
          "Load 15",
          consolePalette.chart.purple,
          timeline,
          bucketed,
          (row) => row.cpu_load_15_avg,
        ),
      ],
    },
    memory: {
      times,
      lines: [
        resourceLine(
          "Memory used",
          consolePalette.chart.green,
          timeline,
          bucketed,
          (row) =>
            row.memory_total_bytes_max > 0
              ? row.memory_used_ratio_avg * 100
              : null,
        ),
      ],
    },
    disk: {
      times,
      lines: [
        resourceLine(
          "Aggregate disk used",
          consolePalette.chart.purple,
          timeline,
          bucketed,
          (row) =>
            row.disk_total_bytes_max > 0
              ? row.disk_used_ratio_avg * 100
              : null,
        ),
      ],
    },
  };
}

function resourceLine(
  label: string,
  color: string,
  timeline: Timeline,
  rows: Map<number, TelemetryRollupRecord>,
  value: (row: TelemetryRollupRecord) => number | null,
): TimeSeriesChartLine {
  return {
    color,
    label,
    values: timeline.epochs.map((epoch) => {
      const row = rows.get(epoch);
      return row ? finiteNumber(value(row)) : null;
    }),
  };
}

function networkChart(
  rows: TelemetryNetworkRateRecord[],
  timeline: Timeline,
  stepSecs: number,
): ChartData {
  const buckets = new Map<number, { rx: number; tx: number }>();
  for (const row of rows) {
    const epoch = chartEpoch(row.bucket_start, stepSecs);
    if (epoch === null) continue;
    const current = buckets.get(epoch) ?? { rx: 0, tx: 0 };
    current.rx += Math.max(0, row.rx_bps_avg);
    current.tx += Math.max(0, row.tx_bps_avg);
    buckets.set(epoch, current);
  }
  return {
    times: timeline.times,
    lines: [
      {
        color: consolePalette.chart.blue,
        exportLabel: "RX rate (bps)",
        label: "RX rate",
        values: timeline.epochs.map((epoch) => buckets.get(epoch)?.rx ?? null),
      },
      {
        color: consolePalette.chart.green,
        exportLabel: "TX rate (bps)",
        label: "TX rate",
        values: timeline.epochs.map((epoch) => buckets.get(epoch)?.tx ?? null),
      },
    ],
  };
}

function trafficChart(
  rows: TrafficHistoryPointView[],
  timeline: Timeline,
  stepSecs: number,
): ChartData {
  const buckets = new Map<number, TrafficHistoryPointView>();
  for (const row of rows) {
    const epoch = chartEpoch(row.bucket_start, stepSecs);
    if (epoch !== null) buckets.set(epoch, row);
  }
  return {
    times: timeline.times,
    lines: [
      {
        color: consolePalette.chart.orange,
        initiallyHidden: true,
        label: "Total volume",
        values: timeline.epochs.map(
          (epoch) => buckets.get(epoch)?.total_bytes ?? null,
        ),
      },
      {
        color: consolePalette.chart.blue,
        label: "RX volume",
        values: timeline.epochs.map(
          (epoch) => buckets.get(epoch)?.rx_bytes ?? null,
        ),
      },
      {
        color: consolePalette.chart.green,
        label: "TX volume",
        values: timeline.epochs.map(
          (epoch) => buckets.get(epoch)?.tx_bytes ?? null,
        ),
      },
    ],
  };
}

function currentPingGenerationRows(
  rows: PingRollupView[],
  targets: CurrentPing[],
): PingRollupView[] {
  const generations = new Map(
    targets.map((target) => [target.target_id, target.generation]),
  );
  return rows.filter(
    (row) => generations.get(row.target_id) === row.generation,
  );
}

function pingChart(
  rows: PingRollupView[],
  range: MonitoringRange,
  metric: PingMetric,
): ChartData {
  const timeline = buildTimeline(range);
  const targets = [
    ...new Map(rows.map((row) => [row.target_id, row])).values(),
  ].sort((left, right) => left.target_name.localeCompare(right.target_name));
  const rowsByTarget = new Map<string, Map<number, PingRollupView>>();
  for (const row of rows) {
    const epoch = chartEpoch(row.bucket_start, range.step_secs);
    if (epoch === null) continue;
    const targetRows =
      rowsByTarget.get(row.target_id) ?? new Map<number, PingRollupView>();
    const previous = targetRows.get(epoch);
    if (
      !previous ||
      timestampMillis(row.latest_checked_at) >=
        timestampMillis(previous.latest_checked_at)
    ) {
      targetRows.set(epoch, row);
    }
    rowsByTarget.set(row.target_id, targetRows);
  }
  return {
    times: timeline.times,
    lines: targets.map((target) => ({
      color: colorForTarget(target.target_id),
      label: target.target_name,
      values: timeline.epochs.map((epoch) => {
        const row = rowsByTarget.get(target.target_id)?.get(epoch);
        if (!row) return null;
        const value =
          metric === "latency" ? row.latency_avg_ms : row.loss_ratio_avg * 100;
        return finiteNumber(value);
      }),
    })),
  };
}

function pingTargetSummaries(
  rows: PingRollupView[],
  targets: CurrentPing[],
  primary: CurrentPing | null,
) {
  const latest = new Map<string, PingRollupView>();
  for (const row of rows) {
    const previous = latest.get(row.target_id);
    if (
      !previous ||
      timestampMillis(row.latest_checked_at) >
        timestampMillis(previous.latest_checked_at)
    ) {
      latest.set(row.target_id, row);
    }
  }
  return targets
    .map((target) => {
      const row = latest.get(target.target_id);
      const summary = row ? pingSummaryFromRow(row) : null;
      return {
        color: colorForTarget(target.target_id),
        generation: target.generation,
        latency: target.latency_avg_ms ?? summary?.latency ?? null,
        loss:
          target.loss_ratio === null
            ? (summary?.loss ?? null)
            : target.loss_ratio * 100,
        name: target.target_name,
        primary: primary?.target_id === target.target_id,
        reason: target.reason ?? summary?.reason ?? null,
        status: target.enabled ? target.status || target.state : "disabled",
        targetId: target.target_id,
      };
    })
    .sort(comparePingSummaries);
}

function pingSummaryFromRow(row: PingRollupView) {
  return {
    color: colorForTarget(row.target_id),
    generation: row.generation,
    latency: row.latency_avg_ms,
    loss: row.loss_ratio_avg * 100,
    name: row.target_name,
    primary: row.is_primary,
    reason: row.latest_reason,
    status: row.latest_status,
    targetId: row.target_id,
  };
}

function comparePingSummaries(
  left: { name: string; primary: boolean },
  right: { name: string; primary: boolean },
) {
  if (left.primary !== right.primary) return left.primary ? -1 : 1;
  return left.name.localeCompare(right.name);
}

function latestRowsByEpoch(
  rows: TelemetryRollupRecord[],
  stepSecs: number,
): Map<number, TelemetryRollupRecord> {
  const result = new Map<number, TelemetryRollupRecord>();
  for (const row of rows) {
    const epoch = chartEpoch(row.bucket_start, stepSecs);
    if (epoch === null) continue;
    const previous = result.get(epoch);
    if (
      !previous ||
      timestampMillis(row.latest_observed_at) >=
        timestampMillis(previous.latest_observed_at)
    ) {
      result.set(epoch, row);
    }
  }
  return result;
}

function buildTimeline(range: MonitoringRange): Timeline {
  const step = Math.max(60, Math.floor(range.step_secs));
  const first = Math.floor(range.start_unix / step) * step;
  const last = Math.floor(range.end_unix / step) * step;
  const epochs: number[] = [];
  for (
    let epoch = first;
    epoch <= last && epochs.length <= 1_442;
    epoch += step
  ) {
    epochs.push(epoch);
  }
  return {
    epochs,
    times: epochs.map((epoch) => new Date(epoch * 1_000).toISOString()),
  };
}

function chartEpoch(value: string, stepSecs: number): number | null {
  const millis = timestampMillis(value);
  if (!Number.isFinite(millis)) return null;
  const seconds = Math.floor(millis / 1_000);
  const step = Math.max(60, Math.floor(stepSecs));
  return Math.floor(seconds / step) * step;
}

function monitoringQuery(
  window: MonitoringWindow,
  customStart: string,
  customEnd: string,
): { error: string | null; path: string | null } {
  const params = new URLSearchParams();
  params.set("points", String(CHART_POINTS));
  if (window !== "custom") {
    params.set("window", window);
    return { error: null, path: params.toString() };
  }
  const start = new Date(customStart).getTime();
  const end = new Date(customEnd).getTime();
  if (
    !customStart ||
    !customEnd ||
    !Number.isFinite(start) ||
    !Number.isFinite(end)
  ) {
    return {
      error: "Select both a valid custom start and end time.",
      path: null,
    };
  }
  if (start > end) {
    return {
      error: "Custom monitoring start must not be after its end.",
      path: null,
    };
  }
  params.set("window", "custom");
  params.set("start_unix", String(Math.floor(start / 1_000)));
  params.set("end_unix", String(Math.floor(end / 1_000)));
  return { error: null, path: params.toString() };
}

function toDateTimeLocal(value: Date): string {
  const offsetMillis = value.getTimezoneOffset() * 60_000;
  return new Date(value.getTime() - offsetMillis).toISOString().slice(0, 16);
}

function rangeEvidence(range: MonitoringRange): string {
  const source =
    range.source === "raw"
      ? "fine realtime samples"
      : "retained minute history";
  return `${source} · ${formatDuration(range.step_secs)} chart buckets · ${formatTime(String(range.start_unix))} – ${formatTime(String(range.end_unix))}`;
}

function finiteNumber(value: number | null | undefined): number | null {
  return value === null || value === undefined || !Number.isFinite(value)
    ? null
    : value;
}

function colorForTarget(targetId: string): string {
  let hash = 0;
  for (let index = 0; index < targetId.length; index += 1) {
    hash = (hash * 31 + targetId.charCodeAt(index)) >>> 0;
  }
  return dashboardChartColors[hash % dashboardChartColors.length];
}

function pingStatusTone(
  status: string,
): "critical" | "warning" | "ok" | "info" | "neutral" {
  const normalized = status.trim().toLowerCase();
  if (["ok", "up", "success", "reachable"].includes(normalized)) return "ok";
  if (["degraded", "partial", "loss"].includes(normalized)) return "warning";
  if (
    ["down", "error", "failed", "failure", "timeout", "unreachable"].includes(
      normalized,
    )
  )
    return "critical";
  if (["checking", "probing"].includes(normalized)) return "info";
  return "neutral";
}

function pingStatusLabel(status: string): string {
  const normalized = status.trim().toLowerCase();
  return ["ok", "up", "success", "reachable"].includes(normalized)
    ? "Reachable"
    : humanizeMonitoringState(status);
}

function humanizeMonitoringState(value: string): string {
  const normalized = value.trim().replace(/[_-]+/g, " ");
  return normalized
    ? `${normalized.charAt(0).toUpperCase()}${normalized.slice(1)}`
    : "Unknown";
}

function formatPercent(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "No data"
    : `${value.toFixed(value >= 100 ? 0 : 1)}%`;
}

function formatLoad(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "No data"
    : value.toFixed(2);
}

function formatConnectionCount(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "No data"
    : Math.max(0, Math.round(value)).toLocaleString();
}

function formatMilliseconds(value: number | null): string {
  return value === null || !Number.isFinite(value)
    ? "No data"
    : `${value.toFixed(value >= 100 ? 0 : 1)} ms`;
}

function formatLoss(value: number | null): string {
  return value === null ? "loss unavailable" : `${formatPercent(value)} loss`;
}

function formatBytesNullable(value: number | null): string {
  return value === null ? "No data" : formatBytes(value);
}

function quotaDetail(used: number, quota: number | null): string {
  if (quota === -1) return "Unlimited";
  return quota && quota > 0
    ? `${formatPercent((used / quota) * 100)} of ${formatBytes(quota)}`
    : "No directional quota";
}

function formatDuration(seconds: number): string {
  if (seconds >= 86_400 && seconds % 86_400 === 0)
    return `${seconds / 86_400}d`;
  if (seconds >= 3_600 && seconds % 3_600 === 0) return `${seconds / 3_600}h`;
  if (seconds >= 60 && seconds % 60 === 0) return `${seconds / 60}m`;
  return `${seconds}s`;
}

function safeFilePart(value: string): string {
  return (
    value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "") || "vps"
  );
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error
    ? cause.message
    : "Monitoring history returned no diagnostic detail. No data is assumed; refresh the current VPS before retrying.";
}
