import { Activity, GitBranch, Route } from "lucide-react";
import { TimeSeriesChart, type TimeSeriesChartLine } from "../../components/TimeSeriesChart";
import { consolePalette, dashboardChartColors } from "../../colorPalette";
import type {
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  TelemetryTunnelRecord,
} from "../../types";
import { formatCompactTime } from "../../utils";

type NetworkMetricsPanelProps = {
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  onOpenEvidence: () => void;
  onOpenOspf: () => void;
  onOpenTests: () => void;
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  telemetryTunnels: TelemetryTunnelRecord[];
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

export function NetworkMetricsPanel({
  networkObservations,
  networkTrends,
  onOpenEvidence,
  onOpenOspf,
  onOpenTests,
  ospfRecommendations,
  telemetryTunnels,
}: NetworkMetricsPanelProps) {
  const groups = buildMetricGroups(networkTrends, networkObservations, telemetryTunnels);
  const overlays = buildOverlayRows(networkObservations, telemetryTunnels, ospfRecommendations);
  const latencyChart = buildObservationChart(
    networkObservations,
    (observation) => observation.latency_avg_ms,
  );
  const lossChart = buildObservationChart(networkObservations, (observation) =>
    observation.packet_loss_ratio === null ? null : observation.packet_loss_ratio * 100,
  );
  const throughputChart = buildObservationChart(
    networkObservations,
    (observation) => observation.throughput_mbps,
  );
  const latestEvidence = latestTime([
    ...networkObservations.map((observation) => observation.observed_at),
    ...networkTrends.map((trend) => trend.latest_observed_at),
  ]);
  const oldestEvidence = oldestTime(networkObservations.map((observation) => observation.observed_at));
  const degradedCount =
    groups.reduce((total, group) => total + group.degradedCount, 0) +
    telemetryTunnels.filter((tunnel) => isTunnelDegraded(tunnel)).length;
  const ospfDeltaCount = ospfRecommendations.filter((recommendation) => recommendation.cost_delta !== 0).length;
  const observationCount = networkObservations.length;

  return (
    <section className="workspace singleColumn observabilityNetworkMetricsWorkspace">
      <div className="fleetPanel observabilityNetworkMetricsPanel">
        <div className="sectionHeader">
          <div>
            <h2>Network metrics</h2>
            <span>Read-only tunnel latency, loss, throughput, endpoint comparison, and OSPF evidence overlays.</span>
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
            detail={latestEvidence ? `latest ${formatCompactTime(latestEvidence)}` : "no retained observations"}
            label="Evidence range"
            value={oldestEvidence && latestEvidence ? `${formatCompactTime(oldestEvidence)} - ${formatCompactTime(latestEvidence)}` : "No data"}
          />
          <MetricTile detail="latency, loss, speed, and status records" label="Observations" value={String(observationCount)} />
          <MetricTile detail="trend groups plus endpoint health" label="Degraded signals" value={String(degradedCount)} />
          <MetricTile detail="recommendations with non-zero cost delta" label="OSPF overlays" value={String(ospfDeltaCount)} />
        </div>

        <section className="dashboardSection observabilityChartSection" aria-labelledby="observability-network-charts-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-charts-title">Latency, loss, and speed</h2>
              <span>Charts use retained observations only; run new diagnostics from Network / Tests.</span>
            </div>
          </div>
          <div className="observabilityNetworkChartGrid" aria-label="Network metrics charts">
            <NetworkChartCard
              emptyLabel="No latency observations"
              lines={latencyChart.lines}
              times={latencyChart.times}
              title="Latency"
              valueFormatter={(value) => (value === null ? "-" : `${formatMetric(value)} ms`)}
            />
            <NetworkChartCard
              emptyLabel="No packet-loss observations"
              lines={lossChart.lines}
              times={lossChart.times}
              title="Packet loss"
              valueFormatter={(value) => (value === null ? "-" : `${formatMetric(value)}%`)}
            />
            <NetworkChartCard
              emptyLabel="No throughput observations"
              lines={throughputChart.lines}
              times={throughputChart.times}
              title="Throughput"
              valueFormatter={(value) => (value === null ? "-" : `${formatMetric(value)} Mbps`)}
            />
          </div>
        </section>

        <section className="dashboardSection observabilityGroupSection" aria-labelledby="observability-network-groups-title">
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-groups-title">Tunnel grouping</h2>
              <span>Grouped by saved plan, topology identity, interface, and endpoint pair when backend plan IDs are unavailable.</span>
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
                    <dt>Latency</dt>
                    <dd>{formatNullableMetric(group.latencyMs, "ms")}</dd>
                  </div>
                  <div>
                    <dt>Loss</dt>
                    <dd>{formatLoss(group.lossRatio)}</dd>
                  </div>
                  <div>
                    <dt>Speed</dt>
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
              <span>Endpoint telemetry highlights managed, observed, and promotion-required tunnel sides.</span>
            </div>
          </div>
          <div className="observabilityEndpointTable" aria-label="Network endpoint comparison">
            <span>Endpoint</span>
            <span>Tunnel</span>
            <span>Runtime</span>
            <span>Latency</span>
            <span>Traffic</span>
            {telemetryTunnels.map((tunnel) => (
              <EndpointRow key={`${tunnel.client_id}:${tunnel.interface}:${tunnel.observed_at}`} tunnel={tunnel} />
            ))}
            {!telemetryTunnels.length && (
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
              <h2 id="observability-network-overlays-title">Alert overlays</h2>
              <span>Derived from unhealthy observations, latency/adapter state, promotion requirements, and OSPF deltas.</span>
            </div>
          </div>
          <div className="observabilityOverlayList" aria-label="Network metrics alert overlays">
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
                <strong>No overlays</strong>
                <span>No unhealthy observations, pending promotions, or OSPF deltas are present.</span>
              </div>
            )}
          </div>
        </section>
      </div>
    </section>
  );
}

function NetworkChartCard({
  emptyLabel,
  lines,
  times,
  title,
  valueFormatter,
}: {
  emptyLabel: string;
  lines: TimeSeriesChartLine[];
  times: string[];
  title: string;
  valueFormatter: (value: number | null) => string;
}) {
  return (
    <article className="dashboardCurveCard">
      <div className="dashboardChartHeader">
        <span>{title}</span>
        <small>{times.length ? `${times.length} point${times.length === 1 ? "" : "s"}` : "No points"}</small>
      </div>
      <TimeSeriesChart
        ariaLabel={`Network metrics ${title.toLowerCase()} chart`}
        emptyLabel={emptyLabel}
        height={170}
        lines={lines}
        times={times}
        valueFormatter={valueFormatter}
      />
    </article>
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

function EndpointRow({ tunnel }: { tunnel: TelemetryTunnelRecord }) {
  const traffic = `${formatBytes(tunnel.rx_bytes)} RX / ${formatBytes(tunnel.tx_bytes)} TX`;
  return (
    <>
      <strong>{tunnel.client_id}</strong>
      <span>{tunnel.plan_name ?? tunnel.interface}</span>
      <span>{tunnel.promotion_required ? "promotion required" : tunnel.plan_correlation}</span>
      <span>{formatEndpointLatency(tunnel)}</span>
      <span>{traffic}</span>
    </>
  );
}

function buildObservationChart(
  observations: NetworkObservationRecord[],
  value: (observation: NetworkObservationRecord) => number | null,
): { lines: TimeSeriesChartLine[]; times: string[] } {
  const times = sortedUniqueTimes(observations.map((observation) => observation.observed_at));
  const groups = new Map<string, NetworkObservationRecord[]>();
  for (const observation of observations) {
    const key = observationGroupKey(observation);
    groups.set(key, [...(groups.get(key) ?? []), observation]);
  }
  return {
    lines: Array.from(groups.entries()).map(([key, group], index) => ({
      color: dashboardChartColors[index % dashboardChartColors.length],
      label: observationGroupLabel(key, group),
      values: times.map((time) => value(group.find((observation) => observation.observed_at === time) ?? emptyObservation())),
    })),
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
    grouped.set(key, [...(grouped.get(key) ?? []), trend]);
  }
  for (const observation of observations) {
    const key = observationGroupKey(observation);
    if (!grouped.has(key)) {
      grouped.set(key, []);
    }
  }
  return Array.from(grouped.entries())
    .map(([key, group]) => {
      const trend = group[0] ?? null;
      const relatedObservations = observations.filter((observation) => observationGroupKey(observation) === key);
      const relatedTunnels = tunnels.filter((tunnel) => tunnelGroupKey(tunnel) === key);
      return {
        degradedCount: group.reduce((total, item) => total + item.degraded_count, 0) + relatedObservations.filter((observation) => observation.healthy === false).length,
        endpointCount: new Set([
          ...group.flatMap((item) => [item.client_id, item.peer_client_id].filter(Boolean)),
          ...relatedObservations.flatMap((item) => [item.client_id, item.peer_client_id].filter(Boolean)),
          ...relatedTunnels.flatMap((item) => [item.client_id, item.peer_client_id].filter(Boolean)),
        ]).size,
        key,
        label: trend?.plan_name ?? relatedObservations[0]?.plan_name ?? relatedTunnels[0]?.plan_name ?? "Unplanned tunnel",
        latestObservedAt: latestTime([
          ...group.map((item) => item.latest_observed_at),
          ...relatedObservations.map((item) => item.observed_at),
          ...relatedTunnels.map((item) => item.observed_at),
        ]),
        lossRatio: firstNumber(group.map((item) => item.packet_loss_avg_ratio), relatedObservations.map((item) => item.packet_loss_ratio)),
        peerLabel: endpointPairLabel(group, relatedObservations, relatedTunnels),
        sampleCount: group.reduce((total, item) => total + item.sample_count, 0) || relatedObservations.length,
        throughputMbps: firstNumber(group.map((item) => item.throughput_avg_mbps), relatedObservations.map((item) => item.throughput_mbps)),
        latencyMs: firstNumber(group.map((item) => item.latency_avg_ms), relatedObservations.map((item) => item.latency_avg_ms)),
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
      detail: `${observation.client_id} ${observation.interface_name ?? observation.kind} at ${formatCompactTime(observation.observed_at)}`,
      key: `observation:${observation.id}`,
      label: observation.plan_name ?? observation.target ?? observation.kind,
      severity: "critical" as const,
      source: "Unhealthy observation",
    }));
  const tunnelRows = tunnels
    .filter((tunnel) => isTunnelDegraded(tunnel) || tunnel.promotion_required)
    .map((tunnel) => ({
      detail: `${tunnel.client_id} ${tunnel.interface}: ${tunnel.latency_status ?? tunnel.operstate ?? tunnel.plan_correlation}`,
      key: `tunnel:${tunnel.client_id}:${tunnel.interface}:${tunnel.observed_at}`,
      label: tunnel.plan_name ?? tunnel.interface,
      severity: tunnel.promotion_required ? "warning" as const : "critical" as const,
      source: tunnel.promotion_required ? "Promotion required" : "Endpoint degraded",
    }));
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
    tunnel.operstate !== null && tunnel.operstate !== "up" ||
    tunnel.latency_status === "down" ||
    tunnel.latency_status === "missed" ||
    tunnel.packet_loss_ratio !== null && tunnel.packet_loss_ratio !== undefined && tunnel.packet_loss_ratio > 0
  );
}

function trendGroupKey(trend: NetworkObservationTrendRecord): string {
  return trend.plan_id ?? trend.topology_identity_hash ?? `${trend.client_id}:${trend.peer_client_id ?? "peer"}:${trend.interface_name ?? trend.kind}`;
}

function observationGroupKey(observation: NetworkObservationRecord): string {
  return observation.plan_id ?? observation.topology_identity_hash ?? `${observation.client_id}:${observation.peer_client_id ?? "peer"}:${observation.interface_name ?? observation.kind}`;
}

function tunnelGroupKey(tunnel: TelemetryTunnelRecord): string {
  return tunnel.plan_id ?? `${tunnel.client_id}:${tunnel.peer_client_id ?? "peer"}:${tunnel.interface}`;
}

function observationGroupLabel(key: string, group: NetworkObservationRecord[]): string {
  const first = group[0] ?? null;
  return first?.plan_name ?? first?.interface_name ?? key;
}

function endpointPairLabel(
  trends: NetworkObservationTrendRecord[],
  observations: NetworkObservationRecord[],
  tunnels: TelemetryTunnelRecord[],
): string {
  const endpoints = new Set<string>();
  for (const item of trends) {
    endpoints.add(item.client_id);
    if (item.peer_client_id) endpoints.add(item.peer_client_id);
  }
  for (const item of observations) {
    endpoints.add(item.client_id);
    if (item.peer_client_id) endpoints.add(item.peer_client_id);
  }
  for (const item of tunnels) {
    endpoints.add(item.client_id);
    if (item.peer_client_id) endpoints.add(item.peer_client_id);
  }
  return endpoints.size ? Array.from(endpoints).join(" <-> ") : "Endpoint pair unavailable";
}

function firstNumber(primary: Array<number | null>, fallback: Array<number | null>): number | null {
  return primary.find((value): value is number => typeof value === "number") ?? fallback.find((value): value is number => typeof value === "number") ?? null;
}

function emptyObservation(): NetworkObservationRecord {
  return {
    bytes: null,
    client_id: "",
    healthy: null,
    id: "",
    interface_name: null,
    job_id: "",
    kind: "",
    latency_avg_ms: null,
    metadata: {},
    observed_at: "",
    packet_loss_ratio: null,
    peer_client_id: null,
    plan_id: null,
    plan_name: null,
    role: null,
    seq: 0,
    target: null,
    throughput_mbps: null,
    topology_identity_hash: null,
  };
}

function sortedUniqueTimes(times: string[]): string[] {
  return Array.from(new Set(times.filter(Boolean))).sort((left, right) => Date.parse(left) - Date.parse(right));
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
  return tunnel.latency_status ?? "No latency";
}

function formatNullableMetric(value: number | null, unit: string): string {
  return value === null ? "-" : `${formatMetric(value)} ${unit}`;
}

function formatLoss(value: number | null | undefined): string {
  return value === null || value === undefined ? "-" : `${formatMetric(value * 100)}%`;
}

function formatMetric(value: number): string {
  if (!Number.isFinite(value)) return "-";
  return value >= 100 ? value.toFixed(0) : value >= 10 ? value.toFixed(1) : value.toFixed(2);
}

function formatBytes(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)} GB`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)} MB`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} KB`;
  return `${Math.round(value)} B`;
}
