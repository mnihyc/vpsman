import { useEffect, useState } from "react";
import { Activity, GitBranch, Route } from "lucide-react";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../../components/TimeSeriesChart";
import { consolePalette, dashboardChartColors } from "../../colorPalette";
import type {
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  TelemetryTunnelRecord,
  TunnelPlanRecord,
} from "../../types";
import {
  networkObservationMetricDefinition,
  type NetworkObservationMetric,
} from "../../telemetryMetrics";
import { latencyStatusLabel, telemetryReasonLabel } from "../../topologyRuntime";
import { formatCompactTime, timestampMillis } from "../../utils";

type NetworkMetricsPanelProps = {
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  onOpenEvidence: () => void;
  onOpenOspf: () => void;
  onOpenTests: () => void;
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
};

type NetworkMetricGroup = {
  degradedCount: number;
  endpointCount: number;
  key: string;
  label: string;
  latestObservedAt: string | null;
  lossRatio: number | null;
  peerLabel: string;
  sampleCount: number;
  throughputMbps: number | null;
  latencyMs: number | null;
};

type OverlayRow = {
  detail: string;
  key: string;
  label: string;
  severity: "info" | "warning" | "critical";
  source: string;
};

type ObservationChartData = {
  lines: TimeSeriesChartLine[];
  observedPoints: number;
  times: string[];
};

type NetworkChartMetric = NetworkObservationMetric;
type ThroughputBenchmark = {
  configuredBandwidthMbps: number;
  latestObservedAt: string | null;
  status: "degraded" | "ok";
  throughputMbps: number;
};

export function NetworkMetricsPanel({
  networkObservations,
  networkTrends,
  onOpenEvidence,
  onOpenOspf,
  onOpenTests,
  ospfRecommendations,
  telemetryTunnels,
  tunnelPlans,
}: NetworkMetricsPanelProps) {
  const [selectedMetric, setSelectedMetric] = useState<NetworkChartMetric>(
    readNetworkMetricRoute,
  );

  useEffect(() => {
    const applyRoute = () => setSelectedMetric(readNetworkMetricRoute());
    window.addEventListener("popstate", applyRoute);
    window.addEventListener("hashchange", applyRoute);
    return () => {
      window.removeEventListener("popstate", applyRoute);
      window.removeEventListener("hashchange", applyRoute);
    };
  }, []);

  function selectMetric(metric: NetworkChartMetric) {
    if (metric === selectedMetric) {
      return;
    }
    writeNetworkMetricRoute(metric);
    setSelectedMetric(metric);
  }
  const enabledPlanIds = new Set(
    tunnelPlans
      .filter((plan) => plan.enabled && !plan.deleted_at)
      .map((plan) => plan.id),
  );
  const declaredObservations = networkObservations.filter(
    (observation) =>
      Boolean(observation.plan_id) && enabledPlanIds.has(observation.plan_id ?? ""),
  );
  const declaredTrends = networkTrends.filter(
    (trend) => Boolean(trend.plan_id) && enabledPlanIds.has(trend.plan_id ?? ""),
  );
  const declaredTunnels = telemetryTunnels.filter(
    (tunnel) => Boolean(tunnel.plan_id) && enabledPlanIds.has(tunnel.plan_id ?? ""),
  );
  const declaredOspfRecommendations = ospfRecommendations.filter(
    (recommendation) => enabledPlanIds.has(recommendation.plan_id),
  );
  const groups = buildMetricGroups(declaredTrends, declaredObservations, declaredTunnels);
  const overlays = buildOverlayRows(
    declaredObservations,
    declaredTunnels,
    declaredOspfRecommendations,
  );
  const latencyChart = buildObservationChart(
    declaredObservations,
    (observation) => observation.latency_avg_ms,
  );
  const lossChart = buildObservationChart(declaredObservations, (observation) =>
    observation.packet_loss_ratio === null ? null : observation.packet_loss_ratio * 100,
  );
  const throughputChart = buildObservationChart(
    declaredObservations,
    (observation) => observation.throughput_mbps,
  );
  const latestEvidence = latestTime([
    ...declaredObservations.map((observation) => observation.observed_at),
    ...declaredTrends.map((trend) => trend.latest_observed_at),
  ]);
  const oldestEvidence = oldestTime(declaredObservations.map((observation) => observation.observed_at));
  const degradedCount =
    groups.reduce((total, group) => total + group.degradedCount, 0) +
    declaredTunnels.filter((tunnel) => isTunnelDegraded(tunnel)).length;
  const ospfDeltaCount = declaredOspfRecommendations.filter(
    (recommendation) => recommendation.cost_delta !== 0,
  ).length;
  const observationCount = declaredObservations.length;
  const chartOptions = [
    {
      chart: latencyChart,
      definition: networkObservationMetricDefinition("latency"),
      emptyLabel: "No latency observations",
      key: "latency" as const,
      title: "Latency",
      valueFormatter: (value: number | null) =>
        value === null ? "No measurement" : `${formatMetric(value)} ms`,
    },
    {
      chart: lossChart,
      definition: networkObservationMetricDefinition("loss"),
      emptyLabel: "No packet-loss observations",
      key: "loss" as const,
      title: "Packet loss",
      valueFormatter: (value: number | null) =>
        value === null ? "No measurement" : `${formatMetric(value)}%`,
    },
    {
      chart: throughputChart,
      definition: networkObservationMetricDefinition("throughput"),
      emptyLabel: "No throughput observations",
      key: "throughput" as const,
      title: "Throughput",
      valueFormatter: (value: number | null) =>
        value === null ? "No measurement" : `${formatMetric(value)} Mbps`,
    },
  ];
  const selectedChart =
    chartOptions.find((option) => option.key === selectedMetric) ??
    chartOptions[0];
  const evidence = buildNetworkEvidence(
    oldestTime(selectedChart.chart.times),
    latestTime(selectedChart.chart.times),
    selectedChart.chart,
  );
  const throughputBenchmark = buildThroughputBenchmark(
    declaredOspfRecommendations,
  );

  return (
    <section className="workspace singleColumn observabilityNetworkMetricsWorkspace">
      <div className="fleetPanel observabilityNetworkMetricsPanel">
        <div className="sectionHeader">
          <div>
            <h2>Network metrics</h2>
            <span>Read-only metrics for enabled declared tunnels. Retained evidence for disabled plans remains available on Network / Evidence.</span>
          </div>
          <div className="sectionActions" aria-label="Network metrics action links">
            <button className="secondaryAction compactAction" onClick={onOpenTests} type="button">
              <Activity size={14} />
              Open Network tests
            </button>
            <button className="secondaryAction compactAction" onClick={onOpenOspf} type="button">
              <Route size={14} />
              Open OSPF review
            </button>
            <button className="secondaryAction compactAction" onClick={onOpenEvidence} type="button">
              <GitBranch size={14} />
              Open evidence
            </button>
          </div>
        </div>

        <div className="metricGrid observabilityMetricsSummary" aria-label="Network metrics summary">
          <MetricTile
            detail={
              oldestEvidence && latestEvidence
                ? `${formatEvidenceTime(oldestEvidence)} to ${formatEvidenceTime(latestEvidence)}; latest ${formatCompactTime(latestEvidence)}`
                : "no retained observations"
            }
            label="Evidence range"
            value={evidenceWindowSummary(oldestEvidence, latestEvidence)}
          />
          <MetricTile detail="retained test/status records; charts include only records containing the selected metric" label="Observations" value={String(observationCount)} />
          <MetricTile detail="trend groups plus endpoint health" label="Degraded signals" value={String(degradedCount)} />
          <MetricTile detail="recommendations with non-zero cost delta" label="OSPF review" value={String(ospfDeltaCount)} />
        </div>

        {evidence.isStale && (
          <div className="observabilityStaleBanner" role="status">
            <div>
              <strong>Stale network evidence</strong>
              <span>
                Last selected-metric sample {evidence.lastSampleLabel}; retained window {evidence.windowLabel}. Run a capped test for current latency, loss, or throughput before changing routing.
              </span>
            </div>
            <div>
              <button className="secondaryAction compactAction" onClick={onOpenEvidence} type="button">
                <GitBranch size={14} />
                Open evidence
              </button>
              <button className="secondaryAction compactAction" onClick={onOpenTests} type="button">
                <Activity size={14} />
                Run test
              </button>
            </div>
          </div>
        )}

        <section className="dashboardSection observabilityChartSection" aria-labelledby="observability-network-charts-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-charts-title">Latency, loss, and throughput</h2>
              <span>Charts use retained observations only; run new diagnostics from Network / Tests.</span>
            </div>
            <div className="dashboardSectionTools" aria-label="Network metric selector">
              {chartOptions.map((option) => (
                <button
                  aria-pressed={selectedMetric === option.key}
                  className={selectedMetric === option.key ? "active" : ""}
                  key={option.key}
                  onClick={() => selectMetric(option.key)}
                  title={option.definition}
                  type="button"
                >
                  {option.title}
                </button>
              ))}
            </div>
          </div>
          <div className="observabilityNetworkChartGrid single" aria-label="Network metrics charts">
            <NetworkChartCard
              emptyLabel={selectedChart.emptyLabel}
              definition={selectedChart.definition}
              evidence={evidence}
              exportFileName={`network-${selectedMetric}`}
              lines={selectedChart.chart.lines}
              observedPoints={selectedChart.chart.observedPoints}
              pointsOnly={evidence.isSparse}
              times={selectedChart.chart.times}
              title={selectedChart.title}
              throughputBenchmark={
                selectedMetric === "throughput" ? throughputBenchmark : null
              }
              valueFormatter={selectedChart.valueFormatter}
            />
          </div>
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-network-groups-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-groups-title">Tunnel grouping</h2>
              <span>Grouped by explicit declared plan ID and endpoint pair.</span>
            </div>
          </div>
          <div className="observabilityNetworkGroupGrid" aria-label="Network metrics tunnel grouping">
            {groups.map((group) => (
              <div className="observabilityNetworkGroupTile" key={group.key}>
                <span>{group.label}</span>
                <strong>{group.peerLabel}</strong>
                <small>
                  {group.sampleCount} samples, {group.degradedCount} degraded, {group.endpointCount} endpoint{group.endpointCount === 1 ? "" : "s"}
                </small>
                <dl>
                  <div>
                    <dt>Avg latency</dt>
                    <dd>{formatNullableMetric(group.latencyMs, "ms")}</dd>
                  </div>
                  <div>
                    <dt>Avg loss</dt>
                    <dd>{formatLoss(group.lossRatio)}</dd>
                  </div>
                  <div>
                    <dt>Avg throughput</dt>
                    <dd>{formatNullableMetric(group.throughputMbps, "Mbps")}</dd>
                  </div>
                </dl>
                <small>{group.latestObservedAt ? `Latest ${formatCompactTime(group.latestObservedAt)}` : "No recent evidence"}</small>
              </div>
            ))}
            {!groups.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No tunnel groups</strong>
                <span>Retained network trends have not been collected yet.</span>
              </div>
            )}
          </div>
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-network-endpoints-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-endpoints-title">Endpoint comparison</h2>
              <span>Endpoint telemetry covers only declared plans. Failed or absent reachability probes remain unverified and do not assert that a tunnel is disconnected.</span>
            </div>
          </div>
          <div className="observabilityEndpointTable" aria-label="Network endpoint comparison">
            {declaredTunnels.map((tunnel) => (
              <EndpointRow key={`${tunnel.client_id}:${tunnel.interface}:${tunnel.observed_at}`} tunnel={tunnel} />
            ))}
            {!declaredTunnels.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No endpoint telemetry</strong>
                <span>Endpoint comparison appears after tunnel telemetry is retained.</span>
              </div>
            )}
          </div>
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-network-overlays-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-overlays-title">Network review signals</h2>
              <span>Derived from unhealthy observations, latency or adapter state, and OSPF cost changes.</span>
            </div>
          </div>
          <div className="observabilityOverlayList" aria-label="Network metrics review signals">
            {overlays.map((overlay) => (
              <div className={`observabilityOverlayRow ${overlay.severity}`} key={overlay.key}>
                <span>{overlay.source}</span>
                <strong>{overlay.label}</strong>
                <small>{overlay.detail}</small>
              </div>
            ))}
            {!overlays.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No review signals</strong>
                <span>No unhealthy observations, degraded declared endpoints, or OSPF cost changes are present.</span>
              </div>
            )}
          </div>
        </section>
      </div>
    </section>
  );
}

function readNetworkMetricRoute(): NetworkChartMetric {
  if (typeof window === "undefined") {
    return "latency";
  }
  const metric = new URLSearchParams(window.location.search).get(
    "network_metric",
  );
  return metric === "loss" || metric === "throughput" ? metric : "latency";
}

function writeNetworkMetricRoute(metric: NetworkChartMetric) {
  if (typeof window === "undefined") {
    return;
  }
  const url = new URL(window.location.href);
  if (metric === "latency") {
    url.searchParams.delete("network_metric");
  } else {
    url.searchParams.set("network_metric", metric);
  }
  const next = `${url.pathname}${url.search}${url.hash}`;
  if (
    `${window.location.pathname}${window.location.search}${window.location.hash}` !==
    next
  ) {
    window.history.pushState(null, "", next);
  }
}

function NetworkChartCard({
  definition,
  emptyLabel,
  evidence,
  exportFileName,
  lines,
  observedPoints,
  pointsOnly,
  times,
  title,
  throughputBenchmark,
  valueFormatter,
}: {
  definition: string;
  emptyLabel: string;
  evidence: NetworkEvidence;
  exportFileName: string;
  lines: TimeSeriesChartLine[];
  observedPoints: number;
  pointsOnly: boolean;
  times: string[];
  title: string;
  throughputBenchmark: ThroughputBenchmark | null;
  valueFormatter: (value: number | null) => string;
}) {
  return (
    <article className="dashboardCurveCard">
      <div className="dashboardChartHeader">
        <span>{title}</span>
        <small>
          {observedPoints
            ? `${observedPoints} measurement${observedPoints === 1 ? "" : "s"} · ${times.length} timestamp${times.length === 1 ? "" : "s"}`
            : "No measurements"}
        </small>
      </div>
      <p className="observabilityRangeLine">
        Time filter: retained evidence · Window: {evidence.windowLabel} · Last sample: {evidence.lastSampleLabel}
      </p>
      <p className="observabilityMetricDefinition" title={definition}>
        Metric definition: {definition}
      </p>
      {evidence.isSparse && (
        <p className="observabilitySparseNotice">
          Sparse data: {evidence.pointLabel}. This chart shows points only; do not read it as a continuous trend.
        </p>
      )}
      {throughputBenchmark ? (
        <p
          aria-label="Network throughput benchmark"
          className="observabilitySparseNotice"
        >
          Average throughput {formatMetric(throughputBenchmark.throughputMbps)} Mbps ·
          expected {formatMetric(throughputBenchmark.configuredBandwidthMbps)} Mbps ·{" "}
          {throughputBenchmark.status} · sample{" "}
          {throughputBenchmark.latestObservedAt
            ? formatCompactTime(throughputBenchmark.latestObservedAt)
            : "age unknown"}
        </p>
      ) : null}
      <TimeSeriesChart
        ariaLabel={`Network metrics ${title.toLowerCase()} chart`}
        emptyLabel={emptyLabel}
        exportFileName={exportFileName}
        height={170}
        lines={lines}
        pointsOnly={pointsOnly}
        times={times}
        valueFormatter={valueFormatter}
      />
    </article>
  );
}

function buildThroughputBenchmark(
  recommendations: NetworkOspfRecommendationRecord[],
): ThroughputBenchmark | null {
  const candidates = recommendations.filter(
    (recommendation) =>
      typeof recommendation.throughput_avg_mbps === "number" &&
      recommendation.configured_bandwidth_mbps > 0,
  );
  const recommendation = candidates.sort(
    (left, right) =>
      throughputRatio(left) - throughputRatio(right) ||
      (right.cost_delta ?? 0) - (left.cost_delta ?? 0),
  )[0];
  if (!recommendation || recommendation.throughput_avg_mbps === null) {
    return null;
  }
  const ratio =
    recommendation.throughput_avg_mbps /
    recommendation.configured_bandwidth_mbps;
  return {
    configuredBandwidthMbps: recommendation.configured_bandwidth_mbps,
    latestObservedAt: recommendation.latest_observed_at,
    status:
      ratio < 0.8 ||
      recommendation.degraded_count > 0 ||
      recommendation.cost_delta !== 0
        ? "degraded"
        : "ok",
    throughputMbps: recommendation.throughput_avg_mbps,
  };
}

function throughputRatio(
  recommendation: NetworkOspfRecommendationRecord,
): number {
  return recommendation.throughput_avg_mbps === null ||
    recommendation.configured_bandwidth_mbps <= 0
    ? Number.POSITIVE_INFINITY
    : recommendation.throughput_avg_mbps /
        recommendation.configured_bandwidth_mbps;
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

type NetworkEvidence = {
  isSparse: boolean;
  isStale: boolean;
  lastSampleLabel: string;
  pointLabel: string;
  windowLabel: string;
};

function EndpointRow({ tunnel }: { tunnel: TelemetryTunnelRecord }) {
  const traffic = `${formatBytes(tunnel.rx_bytes)} RX / ${formatBytes(tunnel.tx_bytes)} TX`;
  const reachability = formatEndpointLatency(tunnel);
  return (
    <div className="observabilityEndpointRow">
      <strong>{endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id)}</strong>
      <span>{tunnel.plan_name ?? tunnel.interface}</span>
      <span>{formatEndpointRuntime(tunnel)}</span>
      <span title={endpointLatencyTitle(tunnel, reachability)}>{reachability}</span>
      <span>{traffic}</span>
    </div>
  );
}

function buildObservationChart(
  observations: NetworkObservationRecord[],
  value: (observation: NetworkObservationRecord) => number | null,
): ObservationChartData {
  const measured = observations
    .map((observation) => ({ measurement: value(observation), observation }))
    .filter(
      (entry): entry is { measurement: number; observation: NetworkObservationRecord } =>
        typeof entry.measurement === "number" && Number.isFinite(entry.measurement),
    );
  const times = sortedUniqueTimes(measured.map(({ observation }) => observation.observed_at));
  const groups = new Map<string, typeof measured>();
  for (const entry of measured) {
    const key = observationSeriesKey(entry.observation);
    groups.set(key, [...(groups.get(key) ?? []), entry]);
  }
  const planSeriesCounts = new Map<string, number>();
  for (const group of groups.values()) {
    const planKey = observationGroupKey(group[0].observation);
    planSeriesCounts.set(planKey, (planSeriesCounts.get(planKey) ?? 0) + 1);
  }
  const lines = Array.from(groups.entries()).map(([key, group], index) => ({
    color: dashboardChartColors[index % dashboardChartColors.length],
    label: observationSeriesLabel(
      key,
      group.map(({ observation }) => observation),
      planSeriesCounts,
    ),
    values: times.map(
      (time) =>
        group.find(({ observation }) => observation.observed_at === time)?.measurement ?? null,
    ),
  }));
  return {
    lines,
    observedPoints: lines.reduce(
      (total, line) =>
        total + line.values.filter((entry) => Number.isFinite(entry)).length,
      0,
    ),
    times,
  };
}

function buildMetricGroups(
  trends: NetworkObservationTrendRecord[],
  observations: NetworkObservationRecord[],
  tunnels: TelemetryTunnelRecord[],
): NetworkMetricGroup[] {
  const grouped = new Map<string, NetworkObservationTrendRecord[]>();
  for (const trend of trends) {
    const key = trendGroupKey(trend);
    if (!key) continue;
    grouped.set(key, [...(grouped.get(key) ?? []), trend]);
  }
  for (const observation of observations) {
    const key = observationGroupKey(observation);
    if (!key) continue;
    if (!grouped.has(key)) {
      grouped.set(key, []);
    }
  }
  return Array.from(grouped.entries())
    .map(([key, group]) => {
      const trend = group[0] ?? null;
      const relatedObservations = observations.filter((observation) => observationGroupKey(observation) === key);
      const relatedTunnels = tunnels.filter((tunnel) => tunnelGroupKey(tunnel) === key);
      const trendSampleCount = group.reduce((total, item) => total + item.sample_count, 0);
      return {
        degradedCount: group.length
          ? group.reduce((total, item) => total + item.degraded_count, 0)
          : relatedObservations.filter((observation) => observation.healthy === false).length,
        endpointCount: new Set([
          ...group.flatMap((item) => [item.client_id, item.peer_client_id].filter(Boolean)),
          ...relatedObservations.flatMap((item) => [item.client_id, item.peer_client_id].filter(Boolean)),
          ...relatedTunnels.flatMap((item) => [item.client_id, item.peer_client_id].filter(Boolean)),
        ]).size,
        key,
        label: trend?.plan_name ?? relatedObservations[0]?.plan_name ?? relatedTunnels[0]?.plan_name ?? "Declared tunnel",
        latestObservedAt: latestTime([
          ...group.map((item) => item.latest_observed_at),
          ...relatedObservations.map((item) => item.observed_at),
          ...relatedTunnels.map((item) => item.observed_at),
        ]),
        lossRatio:
          weightedTrendMetric(group, (item) => item.packet_loss_avg_ratio) ??
          averageMetric(relatedObservations.map((item) => item.packet_loss_ratio)),
        peerLabel: endpointPairLabel(group, relatedObservations, relatedTunnels),
        sampleCount: trendSampleCount || relatedObservations.length,
        throughputMbps:
          weightedTrendMetric(group, (item) => item.throughput_avg_mbps) ??
          averageMetric(relatedObservations.map((item) => item.throughput_mbps)),
        latencyMs:
          weightedTrendMetric(group, (item) => item.latency_avg_ms) ??
          averageMetric(relatedObservations.map((item) => item.latency_avg_ms)),
      };
    })
    .sort((left, right) => (right.degradedCount - left.degradedCount) || left.label.localeCompare(right.label));
}

function buildOverlayRows(
  observations: NetworkObservationRecord[],
  tunnels: TelemetryTunnelRecord[],
  recommendations: NetworkOspfRecommendationRecord[],
): OverlayRow[] {
  const observationRows = observations
    .filter((observation) => observation.healthy === false)
    .map((observation) => ({
      detail: `${observationDirectionLabel(observation)} ${observation.interface_name ?? observation.kind} at ${formatCompactTime(observation.observed_at)}`,
      key: `observation:${observation.id}`,
      label: observation.plan_name ?? observation.target ?? observation.kind,
      severity: "critical" as const,
      source: "Unhealthy observation",
    }));
  const tunnelRows = tunnels
    .filter(isTunnelDegraded)
    .map((tunnel) => {
      const runtimeDegraded = isTunnelRuntimeDegraded(tunnel);
      return {
        detail: `${endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id)} ${tunnel.interface}: ${runtimeDegraded ? formatEndpointRuntime(tunnel) : formatEndpointLatency(tunnel)}`,
        key: `tunnel:${tunnel.client_id}:${tunnel.interface}:${tunnel.observed_at}`,
        label: tunnel.plan_name ?? tunnel.interface,
        severity: runtimeDegraded ? "critical" as const : "warning" as const,
        source: runtimeDegraded ? "Declared endpoint degraded" : "Reachability needs review",
      };
    });
  const ospfRows = recommendations
    .filter((recommendation) => recommendation.cost_delta !== 0)
    .map((recommendation) => ({
      detail: `${recommendation.interface_name}: ${recommendation.plan_ospf_cost} -> ${recommendation.recommended_ospf_cost}; ${recommendation.reason}`,
      key: `ospf:${recommendation.plan_id}:${recommendation.interface_name}`,
      label: recommendation.plan_name,
      severity: "info" as const,
      source: "OSPF delta",
    }));
  return [...observationRows, ...tunnelRows, ...ospfRows];
}

function isTunnelDegraded(tunnel: TelemetryTunnelRecord): boolean {
  return (
    isTunnelRuntimeDegraded(tunnel) ||
    tunnel.latency_status === "down" ||
    tunnel.latency_status === "missed" ||
    tunnel.packet_loss_ratio !== null && tunnel.packet_loss_ratio !== undefined && tunnel.packet_loss_ratio > 0
  );
}

function isTunnelRuntimeDegraded(tunnel: TelemetryTunnelRecord): boolean {
  return (
    tunnel.operstate !== null && tunnel.operstate !== "up" ||
    tunnel.adapter_health?.configured === true && tunnel.adapter_health.success === false ||
    Boolean(tunnel.traffic_status && tunnel.traffic_status !== "ok")
  );
}

function trendGroupKey(trend: NetworkObservationTrendRecord): string {
  return trend.plan_id ?? "";
}

function observationGroupKey(observation: NetworkObservationRecord): string {
  return observation.plan_id ?? "";
}

function observationSeriesKey(observation: NetworkObservationRecord): string {
  return [
    observation.plan_id ?? "",
    observation.client_id,
    observation.peer_client_id ?? "",
    observation.role ?? "",
  ].join(":");
}

function tunnelGroupKey(tunnel: TelemetryTunnelRecord): string {
  return tunnel.plan_id ?? "";
}

function observationGroupLabel(key: string, group: NetworkObservationRecord[]): string {
  const first = group[0] ?? null;
  return first?.plan_name ?? first?.interface_name ?? key;
}

function observationSeriesLabel(
  key: string,
  group: NetworkObservationRecord[],
  planSeriesCounts: Map<string, number>,
): string {
  const first = group[0] ?? null;
  if (!first) return key;
  const planKey = observationGroupKey(first);
  const planLabel = observationGroupLabel(planKey, group);
  if ((planSeriesCounts.get(planKey) ?? 0) <= 1) {
    return planLabel;
  }
  return `${planLabel} · ${endpointDirectionLabel(first.client_id, first.peer_client_id)}`;
}

function endpointPairLabel(
  trends: NetworkObservationTrendRecord[],
  observations: NetworkObservationRecord[],
  tunnels: TelemetryTunnelRecord[],
): string {
  const trend = trends.find((item) => item.peer_client_id) ?? trends[0] ?? null;
  if (trend) {
    return endpointDirectionLabel(trend.client_id, trend.peer_client_id);
  }
  const observation =
    observations.find((item) => item.peer_client_id) ?? observations[0] ?? null;
  if (observation) {
    return endpointDirectionLabel(observation.client_id, observation.peer_client_id);
  }
  const tunnel = tunnels.find((item) => item.peer_client_id) ?? tunnels[0] ?? null;
  if (tunnel) {
    return endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id);
  }
  return "Direction not reported";
}

function averageMetric(values: Array<number | null>): number | null {
  const measured = values.filter(
    (value): value is number => typeof value === "number" && Number.isFinite(value),
  );
  return measured.length
    ? measured.reduce((total, value) => total + value, 0) / measured.length
    : null;
}

function weightedTrendMetric(
  trends: NetworkObservationTrendRecord[],
  value: (trend: NetworkObservationTrendRecord) => number | null,
): number | null {
  let weightedTotal = 0;
  let weightTotal = 0;
  for (const trend of trends) {
    const measurement = value(trend);
    if (typeof measurement !== "number" || !Number.isFinite(measurement)) continue;
    const weight = Math.max(1, trend.sample_count);
    weightedTotal += measurement * weight;
    weightTotal += weight;
  }
  return weightTotal ? weightedTotal / weightTotal : null;
}

function sortedUniqueTimes(times: string[]): string[] {
  return Array.from(new Set(times.filter(Boolean))).sort(
    (left, right) => timestampMillis(left) - timestampMillis(right),
  );
}

function latestTime(times: string[]): string | null {
  const sorted = sortedUniqueTimes(times);
  return sorted[sorted.length - 1] ?? null;
}

function oldestTime(times: string[]): string | null {
  return sortedUniqueTimes(times)[0] ?? null;
}

function formatEndpointLatency(tunnel: TelemetryTunnelRecord): string {
  if (typeof tunnel.latency_avg_ms === "number") {
    return `${formatMetric(tunnel.latency_avg_ms)} ms, ${formatLoss(tunnel.packet_loss_ratio)}`;
  }
  switch (tunnel.latency_status) {
    case "down":
    case "missed":
    case "failed":
      return `Unverified; ${latencyStatusLabel(tunnel.latency_status).toLowerCase()}, no measurement`;
    case "disabled":
      return "Unverified; probe disabled";
    case "unconfigured":
      return "Unverified; probe not configured";
    default:
      return "Unverified; no measurement";
  }
}

function endpointLatencyTitle(
  tunnel: TelemetryTunnelRecord,
  reachability: string,
): string {
  const reason = tunnel.latency_reason
    ? ` Reason: ${telemetryReasonLabel(tunnel.latency_reason)}.`
    : "";
  return `${reachability}.${reason} A failed or absent probe is not proof that the tunnel is disconnected.`;
}

function formatNullableMetric(value: number | null, unit: string): string {
  return value === null ? "No measurement" : `${formatMetric(value)} ${unit}`;
}

function formatLoss(value: number | null | undefined): string {
  return value === null || value === undefined ? "No measurement" : `${formatMetric(value * 100)}%`;
}

function formatMetric(value: number): string {
  if (!Number.isFinite(value)) return "No measurement";
  return value >= 100 ? value.toFixed(0) : value >= 10 ? value.toFixed(1) : value.toFixed(2);
}

function buildNetworkEvidence(
  oldestEvidence: string | null,
  latestEvidence: string | null,
  chart: ObservationChartData,
): NetworkEvidence {
  const latestMs = latestEvidence ? timestampMillis(latestEvidence) : NaN;
  const staleMs = 24 * 60 * 60 * 1000;
  const totalPossiblePoints = Math.max(
    chart.observedPoints,
    chart.times.length * Math.max(1, chart.lines.length),
  );
  return {
    isSparse: chart.observedPoints > 0 && chart.observedPoints <= 3,
    isStale: Number.isFinite(latestMs) && Date.now() - latestMs > staleMs,
    lastSampleLabel: latestEvidence ? formatCompactTime(latestEvidence) : "No samples",
    pointLabel:
      chart.observedPoints === totalPossiblePoints
        ? `${chart.observedPoints} measured point${chart.observedPoints === 1 ? "" : "s"}`
        : `${chart.observedPoints}/${totalPossiblePoints} measured points present`,
    windowLabel:
      oldestEvidence && latestEvidence
        ? `${formatEvidenceTime(oldestEvidence)} to ${formatEvidenceTime(latestEvidence)}`
        : "No retained evidence",
  };
}

function evidenceWindowSummary(
  oldestEvidence: string | null,
  latestEvidence: string | null,
): string {
  if (!oldestEvidence || !latestEvidence) {
    return "No data";
  }
  const durationMs = Math.max(
    0,
    timestampMillis(latestEvidence) - timestampMillis(oldestEvidence),
  );
  if (!Number.isFinite(durationMs)) {
    return "Invalid timestamps";
  }
  return durationMs > 0 ? `${formatDuration(durationMs)} retained` : "Single timestamp";
}

function formatDuration(valueMs: number): string {
  const totalMinutes = Math.max(1, Math.round(valueMs / 60_000));
  if (totalMinutes < 60) return `${totalMinutes}m`;
  const totalHours = Math.round(totalMinutes / 60);
  if (totalHours < 24) return `${totalHours}h`;
  const totalDays = Math.round(totalHours / 24);
  return `${totalDays}d`;
}

function endpointDirectionLabel(
  clientId: string,
  peerClientId: string | null | undefined,
): string {
  return `${clientId} -> ${peerClientId ?? "peer not reported"}`;
}

function observationDirectionLabel(observation: NetworkObservationRecord): string {
  return endpointDirectionLabel(observation.client_id, observation.peer_client_id);
}

function formatEndpointRuntime(tunnel: TelemetryTunnelRecord): string {
  const ownership = readableNetworkToken(tunnel.ownership_mode);
  const operstate = tunnel.operstate ? readableNetworkToken(tunnel.operstate) : null;
  return operstate ? `${ownership}; ${operstate}` : ownership;
}

function readableNetworkToken(value: string): string {
  switch (value) {
    case "down":
      return "Down";
    case "missed":
      return "Missing";
    case "up":
      return "Up";
    case "healthy":
      return "Healthy";
    case "no_latency":
      return "No latency sample";
    default:
      return value
        .split("_")
        .filter(Boolean)
        .map((part, index) =>
          index === 0 ? part.charAt(0).toUpperCase() + part.slice(1) : part,
        )
        .join(" ");
  }
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

function formatBytes(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)} GB`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} MB`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} KB`;
  return `${Math.round(value)} B`;
}
