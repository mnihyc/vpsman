import { useEffect, useRef, useState } from "react";
import {
  Activity,
  GitBranch,
  RefreshCcw,
  Route,
  SlidersHorizontal,
} from "lucide-react";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../../components/TimeSeriesChart";
import { ActionFeedback } from "../../components/ActionFeedback";
import { VpsCombobox } from "../../components/VpsCombobox";
import { NetworkEvidenceRangeControls } from "../../components/NetworkEvidenceRangeControls";
import type { MonitoringWindow } from "../../components/MonitoringRangeTabs";
import { consolePalette, dashboardChartColors } from "../../colorPalette";
import type {
  AgentView,
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
import { useByteCountFormatter } from "../../panelDisplay";
import { formatCompactTime, timestampMillis } from "../../utils";
import {
  pushHistoryEntry,
  useHistoryEntryState,
} from "../../historyEntryState";
import {
  DEFAULT_NETWORK_EVIDENCE_WINDOW,
  NETWORK_EVIDENCE_OBSERVATION_LIMIT,
  defaultNetworkEvidenceEndAt,
  defaultNetworkEvidenceStartAt,
  networkEvidenceWindowLabel,
  type NetworkEvidenceQuery,
} from "../../networkEvidence";

type NetworkMetricsPanelProps = {
  agents: AgentView[];
  error: string | null;
  networkObservations: NetworkObservationRecord[];
  networkTrends: NetworkObservationTrendRecord[];
  onLoadNetworkObservations: (query?: NetworkEvidenceQuery) => Promise<void>;
  onLoadNetworkTrends: (query?: NetworkEvidenceQuery) => Promise<void>;
  onOpenEvidence: () => void;
  onOpenOspf: () => void;
  onOpenTests: () => void;
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  requestsEnabled: boolean;
  telemetryTunnels: TelemetryTunnelRecord[];
  tunnelPlans: TunnelPlanRecord[];
};

type NetworkMetricFilters = {
  clientId: string;
  health: "all" | "healthy" | "degraded" | "unverified";
  planId: string;
  source: "all" | "automatic" | "manual";
};

const DEFAULT_NETWORK_FILTERS: NetworkMetricFilters = {
  clientId: "",
  health: "all",
  planId: "",
  source: "all",
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
  agents,
  error,
  networkObservations,
  networkTrends,
  onLoadNetworkObservations,
  onLoadNetworkTrends,
  onOpenEvidence,
  onOpenOspf,
  onOpenTests,
  ospfRecommendations,
  requestsEnabled,
  telemetryTunnels,
  tunnelPlans,
}: NetworkMetricsPanelProps) {
  const [selectedMetric, setSelectedMetric] = useState<NetworkChartMetric>(
    readNetworkMetricRoute,
  );
  const [filters, setFilters] = useHistoryEntryState<NetworkMetricFilters>(
    "observability.network-metrics.filters",
    DEFAULT_NETWORK_FILTERS,
  );
  const [draftFilters, setDraftFilters] = useState(filters);
  const [evidenceWindow, setEvidenceWindow] = useState<MonitoringWindow>(
    DEFAULT_NETWORK_EVIDENCE_WINDOW,
  );
  const [customStartAt, setCustomStartAt] = useState(
    defaultNetworkEvidenceStartAt,
  );
  const [customEndAt, setCustomEndAt] = useState(defaultNetworkEvidenceEndAt);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const refreshGenerationRef = useRef(0);
  const loadedEvidenceOwnerRef = useRef<{
    observations: NetworkMetricsPanelProps["onLoadNetworkObservations"];
    trends: NetworkMetricsPanelProps["onLoadNetworkTrends"];
  } | null>(null);

  useEffect(() => setDraftFilters(filters), [filters]);

  useEffect(() => {
    if (!requestsEnabled) return;
    const owner = loadedEvidenceOwnerRef.current;
    if (
      owner?.observations === onLoadNetworkObservations &&
      owner.trends === onLoadNetworkTrends
    ) {
      return;
    }
    loadedEvidenceOwnerRef.current = {
      observations: onLoadNetworkObservations,
      trends: onLoadNetworkTrends,
    };
    void refreshEvidence(evidenceWindow, filters);
    // The initial history entry is the authoritative applied filter state.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [onLoadNetworkObservations, onLoadNetworkTrends, requestsEnabled]);

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
  function currentEvidenceQuery(
    windowOverride: MonitoringWindow = evidenceWindow,
    selectedFilters: NetworkMetricFilters = filters,
  ): NetworkEvidenceQuery {
    return {
      clientId: selectedFilters.clientId,
      endAt: customEndAt,
      health:
        selectedFilters.health === "degraded"
          ? "unhealthy"
          : selectedFilters.health === "unverified"
            ? "unknown"
            : selectedFilters.health === "all"
              ? ""
              : selectedFilters.health,
      limit: NETWORK_EVIDENCE_OBSERVATION_LIMIT,
      planIds: selectedFilters.planId ? [selectedFilters.planId] : undefined,
      source: selectedFilters.source === "all" ? "" : selectedFilters.source,
      startAt: customStartAt,
      window: windowOverride,
    };
  }

  async function refreshEvidence(
    windowOverride = evidenceWindow,
    selectedFilters: NetworkMetricFilters = filters,
  ) {
    const generation = refreshGenerationRef.current + 1;
    refreshGenerationRef.current = generation;
    setRefreshing(true);
    setRefreshError(null);
    try {
      const query = currentEvidenceQuery(windowOverride, selectedFilters);
      await Promise.all([
        onLoadNetworkObservations(query),
        onLoadNetworkTrends({ ...query, limit: 10_000 }),
      ]);
      if (generation !== refreshGenerationRef.current) return;
      setFilters(selectedFilters);
    } catch (loadError) {
      if (generation !== refreshGenerationRef.current) return;
      setRefreshError(
        loadError instanceof Error
          ? loadError.message
          : "Network metrics unavailable",
      );
    } finally {
      if (generation === refreshGenerationRef.current) {
        setRefreshing(false);
      }
    }
  }

  function selectEvidenceWindow(next: MonitoringWindow) {
    setEvidenceWindow(next);
    if (next === "custom") {
      refreshGenerationRef.current += 1;
      setRefreshing(false);
      return;
    }
    void refreshEvidence(next, filters);
  }

  async function resetEvidenceFilters() {
    setDraftFilters(DEFAULT_NETWORK_FILTERS);
    await refreshEvidence(evidenceWindow, DEFAULT_NETWORK_FILTERS);
  }

  const enabledPlans = tunnelPlans.filter(
    (plan) => plan.enabled && !plan.deleted_at,
  );
  const enabledPlanIds = new Set(enabledPlans.map((plan) => plan.id));
  const unavailablePlanFilter =
    draftFilters.planId && !enabledPlanIds.has(draftFilters.planId)
      ? draftFilters.planId
      : null;
  const recordMatches = (
    clientIds: Array<string | null | undefined>,
    planId: string | null,
    health: "degraded" | "healthy" | "unverified",
  ) =>
    (!filters.clientId || clientIds.includes(filters.clientId)) &&
    (!filters.planId || planId === filters.planId) &&
    (filters.health === "all" || filters.health === health);
  const declaredObservations = networkObservations.filter(
    (observation) =>
      Boolean(observation.plan_id) &&
      enabledPlanIds.has(observation.plan_id ?? "") &&
      (filters.source === "all" || observation.source === filters.source) &&
      recordMatches(
        [observation.client_id, observation.peer_client_id],
        observation.plan_id,
        observation.healthy === null
          ? "unverified"
          : observation.healthy
            ? "healthy"
            : "degraded",
      ),
  );
  const declaredTrends = networkTrends.filter(
    (trend) =>
      Boolean(trend.plan_id) &&
      enabledPlanIds.has(trend.plan_id ?? "") &&
      recordMatches(
        [trend.client_id, trend.peer_client_id],
        trend.plan_id,
        trend.sample_count === 0
          ? "unverified"
          : trend.degraded_count > 0
            ? "degraded"
            : "healthy",
      ),
  );
  const latestReachability =
    latestReachabilityObservations(declaredObservations);
  const currentReachability = latestReachability.filter(isCurrentReachability);
  const currentReachabilityByEndpoint = new Map(
    currentReachability.map((observation) => [
      `${observation.plan_id}:${observation.client_id}`,
      observation,
    ]),
  );
  const declaredTunnels = telemetryTunnels.filter((tunnel) => {
    const reachability = currentReachabilityByEndpoint.get(
      `${tunnel.plan_id}:${tunnel.client_id}`,
    );
    return (
      Boolean(tunnel.plan_id) &&
      enabledPlanIds.has(tunnel.plan_id ?? "") &&
      recordMatches(
        [tunnel.client_id, tunnel.peer_client_id],
        tunnel.plan_id,
        reachability?.healthy == null
          ? "unverified"
          : reachability.healthy
            ? "healthy"
            : "degraded",
      )
    );
  });
  const declaredOspfRecommendations = ospfRecommendations.filter(
    (recommendation) =>
      enabledPlanIds.has(recommendation.plan_id) &&
      recordMatches(
        [recommendation.left_client_id, recommendation.right_client_id],
        recommendation.plan_id,
        recommendation.sample_count === 0
          ? "unverified"
          : recommendation.degraded_count > 0
            ? "degraded"
            : "healthy",
      ),
  );
  const activeAdvancedFilters =
    Number(Boolean(draftFilters.clientId)) +
    Number(Boolean(draftFilters.planId)) +
    Number(draftFilters.health !== "all") +
    Number(draftFilters.source !== "all");
  const groups = buildMetricGroups(
    declaredTrends,
    declaredObservations,
    declaredTunnels,
  );
  const overlays = buildOverlayRows(
    currentReachability,
    declaredTunnels,
    declaredOspfRecommendations,
  );
  const latencyChart = buildObservationChart(
    declaredObservations,
    (observation) => observation.latency_avg_ms,
    (observation) => observation.kind === "tunnel_reachability",
  );
  const lossChart = buildObservationChart(
    declaredObservations,
    (observation) =>
      observation.packet_loss_ratio === null
        ? null
        : observation.packet_loss_ratio * 100,
    (observation) => observation.kind === "tunnel_reachability",
  );
  const throughputChart = buildObservationChart(
    declaredObservations,
    (observation) => observation.throughput_mbps,
    (observation) => observation.kind === "network_speed_test",
  );
  const latestEvidence = latestTime([
    ...declaredObservations.map((observation) => observation.observed_at),
    ...declaredTrends.map((trend) => trend.latest_observed_at),
  ]);
  const oldestEvidence = oldestTime(
    declaredObservations.map((observation) => observation.observed_at),
  );
  const degradedCount =
    currentReachability.filter((observation) => observation.healthy === false)
      .length +
    declaredTunnels.filter((tunnel) => isTunnelRuntimeDegraded(tunnel)).length;
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
  const selectedObservations = declaredObservations.filter((observation) =>
    selectedMetric === "throughput"
      ? observation.kind === "network_speed_test"
      : observation.kind === "tunnel_reachability",
  );
  const latestSelectedObservation = latestObservation(selectedObservations);
  const evidence = buildNetworkEvidence(
    oldestTime(selectedChart.chart.times),
    latestTime(selectedChart.chart.times),
    selectedChart.chart,
    latestSelectedObservation?.stale_after_secs,
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
            <span>
              Read-only metrics for enabled declared tunnels. Retained evidence
              for disabled plans remains available on Network / Evidence.
            </span>
          </div>
          <div
            className="sectionActions"
            aria-label="Network metrics action links"
          >
            <button
              className="secondaryAction compactAction"
              onClick={onOpenTests}
              title="Open Network tests to run current tunnel diagnostics"
              type="button"
            >
              <Activity size={14} />
              Open Network tests
            </button>
            <button
              className="secondaryAction compactAction"
              onClick={onOpenOspf}
              title="Open reviewed OSPF routing-cost recommendations"
              type="button"
            >
              <Route size={14} />
              Open OSPF review
            </button>
            <button
              className="secondaryAction compactAction"
              onClick={onOpenEvidence}
              title="Open retained network evidence and command results"
              type="button"
            >
              <GitBranch size={14} />
              Open evidence
            </button>
          </div>
        </div>

        <div
          className="observabilityMetricsControls"
          aria-label="Network metrics controls"
        >
          <NetworkEvidenceRangeControls
            ariaLabel="Network metrics time range"
            endAt={customEndAt}
            onEndAtChange={setCustomEndAt}
            onStartAtChange={setCustomStartAt}
            onWindowChange={selectEvidenceWindow}
            startAt={customStartAt}
            window={evidenceWindow}
          />
          <details
            className="fleetMetricsAdvancedFilters"
            title="Restrict retained network evidence by endpoint, plan, source, or health"
          >
            <summary
              title={`${activeAdvancedFilters} advanced network metric filter${activeAdvancedFilters === 1 ? "" : "s"} active`}
            >
              <SlidersHorizontal size={14} />
              <span>Advanced filters</span>
              {activeAdvancedFilters > 0 ? (
                <b>{activeAdvancedFilters}</b>
              ) : null}
            </summary>
            <div className="dashboardControlBar fleetMetricsAdvancedFilterGrid networkMetricsAdvancedFilterGrid">
              <label>
                <span>VPS endpoint</span>
                <VpsCombobox
                  agents={agents}
                  ariaLabel="Network metrics VPS endpoint"
                  onChange={(clientId) =>
                    setDraftFilters((current) => ({
                      ...current,
                      clientId,
                    }))
                  }
                  placeholder="All VPS endpoints"
                  value={draftFilters.clientId}
                />
              </label>
              <label>
                <span>Tunnel plan</span>
                <select
                  aria-label="Network metrics tunnel plan"
                  onChange={(event) =>
                    setDraftFilters((current) => ({
                      ...current,
                      planId: event.target.value,
                    }))
                  }
                  value={draftFilters.planId}
                >
                  <option value="">All enabled plans</option>
                  {unavailablePlanFilter ? (
                    <option value={unavailablePlanFilter}>
                      Unavailable saved plan · {unavailablePlanFilter}
                    </option>
                  ) : null}
                  {enabledPlans.map((plan) => (
                    <option key={plan.id} value={plan.id}>
                      {plan.name}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                <span>Source</span>
                <select
                  aria-label="Network metrics evidence source"
                  onChange={(event) =>
                    setDraftFilters((current) => ({
                      ...current,
                      source: event.target
                        .value as NetworkMetricFilters["source"],
                    }))
                  }
                  value={draftFilters.source}
                >
                  <option value="all">Automatic and manual</option>
                  <option value="automatic">Automatic monitor</option>
                  <option value="manual">Manual test</option>
                </select>
              </label>
              <label>
                <span>Health</span>
                <select
                  aria-label="Network metrics health"
                  onChange={(event) =>
                    setDraftFilters((current) => ({
                      ...current,
                      health: event.target
                        .value as NetworkMetricFilters["health"],
                    }))
                  }
                  value={draftFilters.health}
                >
                  <option value="all">All states</option>
                  <option value="healthy">Healthy</option>
                  <option value="degraded">Degraded</option>
                  <option value="unverified">Unverified</option>
                </select>
              </label>
              <div className="dashboardScopeHint">
                {unavailablePlanFilter
                  ? "The saved plan filter is no longer enabled or visible. Reset or select another plan."
                  : "Range and filters are applied by the API. Every enabled plan remains eligible unless explicitly filtered."}
              </div>
              <button
                className="secondaryAction compactAction"
                disabled={activeAdvancedFilters === 0}
                onClick={() => void resetEvidenceFilters()}
                title={
                  activeAdvancedFilters === 0
                    ? "No advanced network metric filters are active"
                    : `Reset ${activeAdvancedFilters} active advanced network metric filter${activeAdvancedFilters === 1 ? "" : "s"}`
                }
                type="button"
              >
                Reset filters
              </button>
              <button
                className="secondaryAction compactAction"
                disabled={refreshing}
                onClick={() =>
                  void refreshEvidence(evidenceWindow, draftFilters)
                }
                title={
                  refreshing
                    ? "Network evidence is already refreshing"
                    : "Apply the selected range and advanced network filters"
                }
                type="button"
              >
                Apply filters
              </button>
            </div>
          </details>
        </div>
        <div className="observabilityMetricsRefreshState">
          <button
            className="secondaryAction compactAction"
            disabled={refreshing}
            onClick={() => void refreshEvidence()}
            title={
              refreshing
                ? "Network evidence is already refreshing"
                : "Refresh retained evidence for the current network range"
            }
            type="button"
          >
            <RefreshCcw size={14} />
            Refresh range
          </button>
          <span>
            {refreshing
              ? "Refreshing network metrics"
              : `Showing ${networkEvidenceWindowLabel(evidenceWindow)}`}
          </span>
        </div>
        <ActionFeedback
          className="observabilityMetricsError"
          message={refreshError ?? error}
          tone="danger"
        />
        {networkObservations.length >= NETWORK_EVIDENCE_OBSERVATION_LIMIT ? (
          <ActionFeedback
            message="This range reached the 250,000-observation display limit. Narrow the range or filters to inspect every sample."
            tone="warning"
          />
        ) : null}

        <div
          className="metricGrid observabilityMetricsSummary"
          aria-label="Network metrics summary"
        >
          <MetricTile
            detail={
              oldestEvidence && latestEvidence
                ? `${formatEvidenceTime(oldestEvidence)} to ${formatEvidenceTime(latestEvidence)}; latest ${formatCompactTime(latestEvidence)}`
                : "no retained observations"
            }
            label="Evidence range"
            value={evidenceWindowSummary(oldestEvidence, latestEvidence)}
          />
          <MetricTile
            detail="retained test/status records; charts include only records containing the selected metric"
            label="Observations"
            value={String(observationCount)}
          />
          <MetricTile
            detail="trend groups plus endpoint health"
            label="Degraded signals"
            value={String(degradedCount)}
          />
          <MetricTile
            detail="recommendations with non-zero cost delta"
            label="OSPF review"
            value={String(ospfDeltaCount)}
          />
        </div>

        {evidence.isStale && (
          <div className="observabilityStaleBanner" role="status">
            <div>
              <strong>Stale network evidence</strong>
              <span>
                Last selected-metric sample {evidence.lastSampleLabel}; retained
                window {evidence.windowLabel}.{" "}
                {selectedMetric === "throughput"
                  ? "Run a capped speed test"
                  : "Wait for the next automatic monitor or run a manual probe"}{" "}
                before using stale evidence to change routing.
              </span>
            </div>
            <div>
              <button
                className="secondaryAction compactAction"
                onClick={onOpenEvidence}
                type="button"
              >
                <GitBranch size={14} />
                Open evidence
              </button>
              <button
                className="secondaryAction compactAction"
                onClick={onOpenTests}
                type="button"
              >
                <Activity size={14} />
                Run test
              </button>
            </div>
          </div>
        )}

        <section
          className="dashboardSection observabilityChartSection"
          aria-labelledby="observability-network-charts-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-charts-title">
                Latency, loss, and throughput
              </h2>
              <span>
                Charts use retained observations only; run new diagnostics from
                Network / Tests.
              </span>
            </div>
            <div
              className="dashboardSectionTools"
              aria-label="Network metric selector"
            >
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
          <div
            className="observabilityNetworkChartGrid single"
            aria-label="Network metrics charts"
          >
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

        <section
          className="dashboardSection observabilityGroupSection"
          aria-labelledby="observability-network-groups-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-groups-title">Tunnel grouping</h2>
              <span>
                Grouped by explicit declared plan ID and endpoint pair.
              </span>
            </div>
          </div>
          <div
            className="observabilityNetworkGroupGrid"
            aria-label="Network metrics tunnel grouping"
          >
            {groups.map((group) => (
              <div
                className="observabilityNetworkGroupTile"
                key={group.key}
                title={`${group.label}, ${group.peerLabel}: ${group.sampleCount} retained samples and ${group.degradedCount} degraded`}
              >
                <span title={`Declared tunnel plan: ${group.label}`}>
                  {group.label}
                </span>
                <strong title={`Endpoint pair: ${group.peerLabel}`}>
                  {group.peerLabel}
                </strong>
                <small
                  title={`${group.sampleCount} retained samples; ${group.degradedCount} degraded; ${group.endpointCount} endpoints`}
                >
                  {group.sampleCount} samples, {group.degradedCount} degraded,{" "}
                  {group.endpointCount} endpoint
                  {group.endpointCount === 1 ? "" : "s"}
                </small>
                <dl>
                  <div
                    title={
                      group.latencyMs === null
                        ? "Average latency is unavailable because no retained latency measurements matched"
                        : `Average latency ${formatNullableMetric(group.latencyMs, "ms")}`
                    }
                  >
                    <dt title="Mean retained latency for this tunnel group">
                      Avg latency
                    </dt>
                    <dd
                      title={
                        group.latencyMs === null
                          ? "No retained latency measurement matched this group"
                          : `Average latency ${formatNullableMetric(group.latencyMs, "ms")}`
                      }
                    >
                      {formatNullableMetric(group.latencyMs, "ms")}
                    </dd>
                  </div>
                  <div
                    title={
                      group.lossRatio === null
                        ? "Average packet loss is unavailable because no retained loss measurements matched"
                        : `Average packet loss ${formatLoss(group.lossRatio)}`
                    }
                  >
                    <dt title="Mean retained packet loss for this tunnel group">
                      Avg loss
                    </dt>
                    <dd
                      title={
                        group.lossRatio === null
                          ? "No retained packet-loss measurement matched this group"
                          : `Average packet loss ${formatLoss(group.lossRatio)}`
                      }
                    >
                      {formatLoss(group.lossRatio)}
                    </dd>
                  </div>
                  <div
                    title={
                      group.throughputMbps === null
                        ? "Average throughput is unavailable because no retained speed measurement matched"
                        : `Average throughput ${formatNullableMetric(group.throughputMbps, "Mbps")}`
                    }
                  >
                    <dt title="Mean retained throughput for this tunnel group">
                      Avg throughput
                    </dt>
                    <dd
                      title={
                        group.throughputMbps === null
                          ? "No retained throughput measurement matched this group"
                          : `Average throughput ${formatNullableMetric(group.throughputMbps, "Mbps")}`
                      }
                    >
                      {formatNullableMetric(group.throughputMbps, "Mbps")}
                    </dd>
                  </div>
                </dl>
                <small
                  title={
                    group.latestObservedAt
                      ? `Latest retained observation ${formatEvidenceTime(group.latestObservedAt)}`
                      : "No retained observation exists for this tunnel group"
                  }
                >
                  {group.latestObservedAt
                    ? `Latest ${formatCompactTime(group.latestObservedAt)}`
                    : "No recent evidence"}
                </small>
              </div>
            ))}
            {!groups.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No tunnel groups</strong>
                <span>
                  Retained network trends have not been collected yet.
                </span>
              </div>
            )}
          </div>
        </section>

        <section
          className="dashboardSection observabilityGroupSection"
          aria-labelledby="observability-network-endpoints-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-endpoints-title">
                Endpoint comparison
              </h2>
              <span>
                Endpoint telemetry covers only declared plans. Failed or absent
                reachability probes remain unverified and do not assert that a
                tunnel is disconnected.
              </span>
            </div>
          </div>
          <div
            aria-label="Network endpoint comparison"
            className="observabilityEndpointTable"
            role={declaredTunnels.length > 0 ? "table" : undefined}
          >
            {declaredTunnels.length > 0 && (
              <div className="observabilityEndpointRow heading" role="row">
                <span role="columnheader">Endpoint</span>
                <span role="columnheader">Plan / interface</span>
                <span role="columnheader">Runtime</span>
                <span role="columnheader">Reachability</span>
                <span role="columnheader">Traffic</span>
              </div>
            )}
            {declaredTunnels.map((tunnel) => (
              <EndpointRow
                key={`${tunnel.client_id}:${tunnel.interface}:${tunnel.observed_at}`}
                observation={
                  latestReachability.find(
                    (observation) =>
                      observation.plan_id === tunnel.plan_id &&
                      observation.client_id === tunnel.client_id,
                  ) ?? null
                }
                tunnel={tunnel}
              />
            ))}
            {!declaredTunnels.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No endpoint telemetry</strong>
                <span>
                  Endpoint comparison appears after tunnel telemetry is
                  retained.
                </span>
              </div>
            )}
          </div>
        </section>

        <section
          className="dashboardSection observabilityGroupSection"
          aria-labelledby="observability-network-overlays-title"
        >
          <div className="dashboardSectionHeader">
            <div>
              <h2 id="observability-network-overlays-title">
                Network review signals
              </h2>
              <span>
                Derived from unhealthy observations, latency or adapter state,
                and OSPF cost changes.
              </span>
            </div>
          </div>
          <div
            className="observabilityOverlayList"
            aria-label="Network metrics review signals"
          >
            {overlays.map((overlay) => (
              <div
                className={`observabilityOverlayRow ${overlay.severity}`}
                key={overlay.key}
                title={`${overlay.source}: ${overlay.label}. ${overlay.detail}`}
              >
                <span title={`Review signal source: ${overlay.source}`}>
                  {overlay.source}
                </span>
                <strong
                  title={`${overlay.severity} network review signal: ${overlay.label}`}
                >
                  {overlay.label}
                </strong>
                <small title={overlay.detail}>{overlay.detail}</small>
              </div>
            ))}
            {!overlays.length && (
              <div className="emptyState compactEmpty">
                <Activity size={18} />
                <strong>No review signals</strong>
                <span>
                  No unhealthy observations, degraded declared endpoints, or
                  OSPF cost changes are present.
                </span>
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
    pushHistoryEntry(next);
  }
}

function networkMetricFiltersEqual(
  left: NetworkMetricFilters,
  right: NetworkMetricFilters,
): boolean {
  return (
    left.clientId === right.clientId &&
    left.health === right.health &&
    left.planId === right.planId &&
    left.source === right.source
  );
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
    <article className="dashboardCurveCard" title={`${title}: ${definition}`}>
      <div
        className="dashboardChartHeader"
        title={`${title} retained measurement coverage`}
      >
        <span title={definition}>{title}</span>
        <small
          title={
            observedPoints
              ? `${observedPoints} retained measurements across ${times.length} timestamps`
              : emptyLabel
          }
        >
          {observedPoints
            ? `${observedPoints} measurement${observedPoints === 1 ? "" : "s"} · ${times.length} timestamp${times.length === 1 ? "" : "s"}`
            : "No measurements"}
        </small>
      </div>
      <p className="observabilityRangeLine">
        Time filter: retained evidence · Window: {evidence.windowLabel} · Last
        sample: {evidence.lastSampleLabel}
      </p>
      <p className="observabilityMetricDefinition" title={definition}>
        Metric definition: {definition}
      </p>
      {evidence.isSparse && (
        <p className="observabilitySparseNotice">
          Sparse data: {evidence.pointLabel}. This chart shows points only; do
          not read it as a continuous trend.
        </p>
      )}
      {throughputBenchmark ? (
        <p
          aria-label="Network throughput benchmark"
          className="observabilitySparseNotice"
        >
          Average throughput {formatMetric(throughputBenchmark.throughputMbps)}{" "}
          Mbps · expected{" "}
          {formatMetric(throughputBenchmark.configuredBandwidthMbps)} Mbps ·{" "}
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

type NetworkEvidence = {
  isSparse: boolean;
  isStale: boolean;
  lastSampleLabel: string;
  pointLabel: string;
  windowLabel: string;
};

function EndpointRow({
  observation,
  tunnel,
}: {
  observation: NetworkObservationRecord | null;
  tunnel: TelemetryTunnelRecord;
}) {
  const formatBytes = useByteCountFormatter();
  const traffic =
    tunnel.rx_bytes === null || tunnel.tx_bytes === null
      ? "Counters excluded by network.interfaces"
      : `${formatBytes(tunnel.rx_bytes)} RX / ${formatBytes(tunnel.tx_bytes)} TX`;
  const reachability = formatReachabilityObservation(observation);
  return (
    <div
      className="observabilityEndpointRow"
      role="row"
      title={`${endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id)}; ${tunnel.plan_name ?? tunnel.interface}; ${formatEndpointRuntime(tunnel)}; ${reachability}; ${traffic}`}
    >
      <strong
        role="cell"
        title={`Endpoint ${endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id)}`}
      >
        <span aria-hidden="true" className="observabilityEndpointMobileLabel">
          Endpoint
        </span>
        <span>
          {endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id)}
        </span>
      </strong>
      <span
        role="cell"
        title={`Declared plan or interface: ${tunnel.plan_name ?? tunnel.interface}`}
      >
        <span aria-hidden="true" className="observabilityEndpointMobileLabel">
          Plan / interface
        </span>
        <span>{tunnel.plan_name ?? tunnel.interface}</span>
      </span>
      <span
        role="cell"
        title={`Latest reported tunnel runtime: ${formatEndpointRuntime(tunnel)}`}
      >
        <span aria-hidden="true" className="observabilityEndpointMobileLabel">
          Runtime
        </span>
        <span>{formatEndpointRuntime(tunnel)}</span>
      </span>
      <span
        role="cell"
        title={reachabilityObservationTitle(observation, reachability)}
      >
        <span aria-hidden="true" className="observabilityEndpointMobileLabel">
          Reachability
        </span>
        <span>{reachability}</span>
      </span>
      <span
        role="cell"
        title={traffic}
      >
        <span aria-hidden="true" className="observabilityEndpointMobileLabel">
          Traffic
        </span>
        <span>{traffic}</span>
      </span>
    </div>
  );
}

function buildObservationChart(
  observations: NetworkObservationRecord[],
  value: (observation: NetworkObservationRecord) => number | null,
  belongsToMetric: (observation: NetworkObservationRecord) => boolean,
): ObservationChartData {
  const entries = observations
    .filter(belongsToMetric)
    .map((observation) => ({ measurement: value(observation), observation }));
  const times = sortedUniqueTimes(
    entries.map(({ observation }) => observation.observed_at),
  );
  const groups = new Map<string, typeof entries>();
  for (const entry of entries) {
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
        group.find(({ observation }) => observation.observed_at === time)
          ?.measurement ?? null,
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
      const relatedObservations = observations.filter(
        (observation) => observationGroupKey(observation) === key,
      );
      const relatedTunnels = tunnels.filter(
        (tunnel) => tunnelGroupKey(tunnel) === key,
      );
      const trendSampleCount = group.reduce(
        (total, item) => total + item.sample_count,
        0,
      );
      return {
        degradedCount: group.length
          ? group.reduce((total, item) => total + item.degraded_count, 0)
          : relatedObservations.filter(
              (observation) => observation.healthy === false,
            ).length,
        endpointCount: new Set([
          ...group.flatMap((item) =>
            [item.client_id, item.peer_client_id].filter(Boolean),
          ),
          ...relatedObservations.flatMap((item) =>
            [item.client_id, item.peer_client_id].filter(Boolean),
          ),
          ...relatedTunnels.flatMap((item) =>
            [item.client_id, item.peer_client_id].filter(Boolean),
          ),
        ]).size,
        key,
        label:
          trend?.plan_name ??
          relatedObservations[0]?.plan_name ??
          relatedTunnels[0]?.plan_name ??
          "Declared tunnel",
        latestObservedAt: latestTime([
          ...group.map((item) => item.latest_observed_at),
          ...relatedObservations.map((item) => item.observed_at),
          ...relatedTunnels.map((item) => item.observed_at),
        ]),
        lossRatio:
          weightedTrendMetric(group, (item) => item.packet_loss_avg_ratio) ??
          averageMetric(
            relatedObservations.map((item) => item.packet_loss_ratio),
          ),
        peerLabel: endpointPairLabel(
          group,
          relatedObservations,
          relatedTunnels,
        ),
        sampleCount: trendSampleCount || relatedObservations.length,
        throughputMbps:
          weightedTrendMetric(group, (item) => item.throughput_avg_mbps) ??
          averageMetric(
            relatedObservations.map((item) => item.throughput_mbps),
          ),
        latencyMs:
          weightedTrendMetric(group, (item) => item.latency_avg_ms) ??
          averageMetric(relatedObservations.map((item) => item.latency_avg_ms)),
      };
    })
    .sort(
      (left, right) =>
        right.degradedCount - left.degradedCount ||
        left.label.localeCompare(right.label),
    );
}

function latestReachabilityObservations(
  observations: NetworkObservationRecord[],
): NetworkObservationRecord[] {
  const latest = new Map<string, NetworkObservationRecord>();
  for (const observation of observations) {
    if (observation.kind !== "tunnel_reachability") continue;
    const key = `${observation.plan_id ?? ""}:${observation.endpoint_side ?? observation.client_id}`;
    const current = latest.get(key);
    if (
      !current ||
      timestampMillis(observation.observed_at) >
        timestampMillis(current.observed_at)
    ) {
      latest.set(key, observation);
    }
  }
  return Array.from(latest.values());
}

function isCurrentReachability(observation: NetworkObservationRecord): boolean {
  const staleAfterMs = Math.max(1, observation.stale_after_secs ?? 180) * 1_000;
  return Date.now() - timestampMillis(observation.observed_at) <= staleAfterMs;
}

function latestObservation(
  observations: NetworkObservationRecord[],
): NetworkObservationRecord | null {
  return observations.reduce<NetworkObservationRecord | null>(
    (latest, observation) =>
      !latest ||
      timestampMillis(observation.observed_at) >
        timestampMillis(latest.observed_at)
        ? observation
        : latest,
    null,
  );
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
  const tunnelRows = tunnels.filter(isTunnelRuntimeDegraded).map((tunnel) => {
    return {
      detail: `${endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id)} ${tunnel.interface}: ${formatEndpointRuntime(tunnel)}`,
      key: `tunnel:${tunnel.client_id}:${tunnel.interface}:${tunnel.observed_at}`,
      label: tunnel.plan_name ?? tunnel.interface,
      severity: "critical" as const,
      source: "Declared endpoint degraded",
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

function isTunnelRuntimeDegraded(tunnel: TelemetryTunnelRecord): boolean {
  return (
    (tunnel.operstate !== null && tunnel.operstate !== "up") ||
    (tunnel.adapter_health?.configured === true &&
      tunnel.adapter_health.success === false) ||
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

function observationGroupLabel(
  key: string,
  group: NetworkObservationRecord[],
): string {
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
    return endpointDirectionLabel(
      observation.client_id,
      observation.peer_client_id,
    );
  }
  const tunnel =
    tunnels.find((item) => item.peer_client_id) ?? tunnels[0] ?? null;
  if (tunnel) {
    return endpointDirectionLabel(tunnel.client_id, tunnel.peer_client_id);
  }
  return "Direction not reported";
}

function averageMetric(values: Array<number | null>): number | null {
  const measured = values.filter(
    (value): value is number =>
      typeof value === "number" && Number.isFinite(value),
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
    if (typeof measurement !== "number" || !Number.isFinite(measurement))
      continue;
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

function formatReachabilityObservation(
  observation: NetworkObservationRecord | null,
): string {
  if (!observation) return "Unverified";
  if (!isCurrentReachability(observation)) {
    const priorState = observation.healthy === false ? "failed" : "reachable";
    return `Stale · previously ${priorState}`;
  }
  if (observation.latency_avg_ms === null) {
    return observation.healthy === false ? "Unhealthy" : "Observed";
  }
  const loss =
    observation.packet_loss_ratio === null
      ? "loss unavailable"
      : `${formatMetric(observation.packet_loss_ratio * 100)}% loss`;
  return `${formatMetric(observation.latency_avg_ms)} ms · ${loss}`;
}

function reachabilityObservationTitle(
  observation: NetworkObservationRecord | null,
  summary: string,
): string {
  if (!observation) {
    return "No fresh automatic or manual reachability observation";
  }
  const freshness = isCurrentReachability(observation)
    ? "current"
    : `stale after ${Math.max(1, observation.stale_after_secs ?? 180)} seconds`;
  return `${summary}; ${observation.source}; ${freshness}; observed ${formatCompactTime(observation.observed_at)}${observation.reason ? `; ${observation.reason}` : ""}`;
}

function formatNullableMetric(value: number | null, unit: string): string {
  return value === null ? "No measurement" : `${formatMetric(value)} ${unit}`;
}

function formatLoss(value: number | null | undefined): string {
  return value === null || value === undefined
    ? "No measurement"
    : `${formatMetric(value * 100)}%`;
}

function formatMetric(value: number): string {
  if (!Number.isFinite(value)) return "No measurement";
  return value >= 100
    ? value.toFixed(0)
    : value >= 10
      ? value.toFixed(1)
      : value.toFixed(2);
}

function buildNetworkEvidence(
  oldestEvidence: string | null,
  latestEvidence: string | null,
  chart: ObservationChartData,
  staleAfterSecs: number | null | undefined,
): NetworkEvidence {
  const latestMs = latestEvidence ? timestampMillis(latestEvidence) : NaN;
  const staleMs = Math.max(1, staleAfterSecs ?? 24 * 60 * 60) * 1_000;
  const totalPossiblePoints = Math.max(
    chart.observedPoints,
    chart.times.length * Math.max(1, chart.lines.length),
  );
  return {
    isSparse: chart.observedPoints > 0 && chart.observedPoints <= 3,
    isStale: Number.isFinite(latestMs) && Date.now() - latestMs > staleMs,
    lastSampleLabel: latestEvidence
      ? formatCompactTime(latestEvidence)
      : "No samples",
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
  return durationMs > 0
    ? `${formatDuration(durationMs)} retained`
    : "Single timestamp";
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

function observationDirectionLabel(
  observation: NetworkObservationRecord,
): string {
  return endpointDirectionLabel(
    observation.client_id,
    observation.peer_client_id,
  );
}

function formatEndpointRuntime(tunnel: TelemetryTunnelRecord): string {
  const ownership = readableNetworkToken(tunnel.ownership_mode);
  const operstate = tunnel.operstate
    ? readableNetworkToken(tunnel.operstate)
    : null;
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
