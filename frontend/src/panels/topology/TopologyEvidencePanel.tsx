import { type ReactNode, useMemo, useRef, useState } from "react";
import {
  Activity,
  ExternalLink,
  GitBranch,
  GitCompareArrows,
  MapIcon,
  RefreshCcw,
  ShieldCheck,
  SlidersHorizontal,
} from "lucide-react";
import {
  jobStatusBadgeClass,
  topologyObservationStateBadgeClass,
  topologyRuntimeStateBadgeClass,
} from "../../jobStatusPresentation";
import { ActionFeedback } from "../../components/ActionFeedback";
import { NetworkEvidenceRangeControls } from "../../components/NetworkEvidenceRangeControls";
import type { MonitoringWindow } from "../../components/MonitoringRangeTabs";
import { VpsCombobox } from "../../components/VpsCombobox";
import type {
  AgentView,
  JobHistoryRecord,
  JobOutputRecord,
  JobStatus,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  TopologyObservationState,
  TopologyRuntimeState,
  TunnelPlanRecord,
} from "../../types";
import {
  decodeOutputPreview,
  formatCompactTime,
  formatFullTime,
  formatTime,
  shortId,
  timestampMillis,
} from "../../utils";
import { readableTelemetryToken } from "../../topologyRuntime";
import {
  DEFAULT_NETWORK_EVIDENCE_WINDOW,
  NETWORK_EVIDENCE_OBSERVATION_LIMIT,
  defaultNetworkEvidenceEndAt,
  defaultNetworkEvidenceStartAt,
  networkEvidenceWindowLabel,
  type NetworkEvidenceHealth,
  type NetworkEvidenceKind,
  type NetworkEvidenceQuery,
  type NetworkEvidenceSource,
} from "../../networkEvidence";

const networkCommands = new Set([
  "runtime_config_sync",
  "network_status",
  "network_probe",
  "network_speed_test",
]);

const DEFAULT_NETWORK_MEASUREMENT_FRESH_AFTER_MS = 10 * 60 * 1_000;

export function TopologyEvidencePanel({
  agents,
  clientLabel,
  error,
  jobs,
  observations,
  onLoadObservations,
  onLoadOspfRecommendations,
  onLoadOspfUpdatePlans,
  onLoadOutputs,
  onLoadTrends,
  onOpenGraph,
  onOpenJobDetails,
  onOpenOspfApprovals,
  onOpenTests,
  onOpenTunnelPlans,
  ospfRecommendations,
  ospfUpdatePlans,
  trends,
  tunnelPlans,
}: {
  agents: AgentView[];
  clientLabel: (clientId: string) => string;
  error: string | null;
  jobs: JobHistoryRecord[];
  observations: NetworkObservationRecord[];
  onLoadObservations: (query?: NetworkEvidenceQuery) => Promise<void>;
  onLoadOspfRecommendations: () => Promise<void>;
  onLoadOspfUpdatePlans: () => Promise<void>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTrends: (query?: NetworkEvidenceQuery) => Promise<void>;
  onOpenGraph?: () => void;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenOspfApprovals?: () => void;
  onOpenTests?: () => void;
  onOpenTunnelPlans?: () => void;
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  trends: NetworkObservationTrendRecord[];
  tunnelPlans: TunnelPlanRecord[];
}) {
  const networkJobs = useMemo(
    () =>
      jobs.filter((job) => networkCommands.has(job.command_type)).slice(0, 8),
    [jobs],
  );
  const [outputsByJob, setOutputsByJob] = useState<
    Record<string, JobOutputRecord[]>
  >({});
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [outputError, setOutputError] = useState<string | null>(null);
  const [outputNotice, setOutputNotice] = useState<string | null>(null);
  const [outputLoading, setOutputLoading] = useState(false);
  const [evidenceWindow, setEvidenceWindow] = useState<MonitoringWindow>(
    DEFAULT_NETWORK_EVIDENCE_WINDOW,
  );
  const [customStartAt, setCustomStartAt] = useState(
    defaultNetworkEvidenceStartAt,
  );
  const [customEndAt, setCustomEndAt] = useState(defaultNetworkEvidenceEndAt);
  const [planId, setPlanId] = useState("");
  const [clientId, setClientId] = useState("");
  const [source, setSource] = useState<NetworkEvidenceSource>("");
  const [kind, setKind] = useState<NetworkEvidenceKind>("");
  const [health, setHealth] = useState<NetworkEvidenceHealth>("");
  const [searchQuery, setSearchQuery] = useState("");
  const [appliedPlanId, setAppliedPlanId] = useState("");
  const refreshGenerationRef = useRef(0);
  const throughputBaselines = useMemo(
    () => buildThroughputBaselineLookup(ospfRecommendations, ospfUpdatePlans),
    [ospfRecommendations, ospfUpdatePlans],
  );
  const rows = networkJobs.map((job) => {
    const outputs = outputsByJob[job.id];
    return buildEvidenceRow(
      job,
      outputs ?? [],
      clientLabel,
      outputs !== undefined,
      throughputBaselines,
    );
  });
  const selectedPlanIds = appliedPlanId ? new Set([appliedPlanId]) : null;
  const ospfUpdateRows = ospfUpdatePlans
    .filter((plan) => !selectedPlanIds || selectedPlanIds.has(plan.plan_id))
    .map(buildOspfUpdatePlanRow);
  const ospfRows = ospfRecommendations
    .filter(
      (recommendation) =>
        !selectedPlanIds || selectedPlanIds.has(recommendation.plan_id),
    )
    .map(buildOspfRecommendationRow);
  const observationRows = latestObservationRows(observations).map(
    (observation) =>
      buildObservationRow(observation, clientLabel, throughputBaselines),
  );
  const trendRows = trends.map((trend) =>
    buildTrendRow(trend, clientLabel, throughputBaselines),
  );
  const hasUnloadedOutput = rows.some(
    (row) => row.metric === "Output not loaded",
  );
  const hasTrendComparison = trendRows.length > 0;
  const freshness = buildNetworkEvidenceFreshness(observations);
  const timelineStages = buildTimelineStages({
    commandRows: rows,
    observationRows,
    ospfRecommendationRows: ospfRows,
    ospfUpdateRows,
    trendRows,
  });
  const probePoints: Array<{
    healthy?: boolean | null;
    jobId: string;
    latencyAvgMs: number | null;
    lossRatio: number | null;
    reason?: string | null;
  }> = [
    ...rows
      .filter(
        (row) =>
          row.kind === "network_probe" && typeof row.latencyAvgMs === "number",
      )
      .map((row) => ({
        jobId: row.job.id,
        latencyAvgMs: row.latencyAvgMs ?? 0,
        lossRatio: row.lossRatio ?? null,
      })),
    ...observations
      .filter((observation) => observation.kind === "tunnel_reachability")
      .map((observation) => ({
        healthy: observation.healthy,
        jobId: observation.id,
        latencyAvgMs: observation.latency_avg_ms,
        lossRatio: observation.packet_loss_ratio ?? null,
        reason: observation.reason,
      })),
  ];
  const maxLatency = Math.max(
    1,
    ...probePoints.flatMap((point) =>
      typeof point.latencyAvgMs === "number" ? [point.latencyAvgMs] : [],
    ),
  );
  const latencyGroups = useMemo(
    () => buildLatencyCurveGroups(observations, clientLabel),
    [clientLabel, observations],
  );
  const hasStandaloneProbeCurve =
    probePoints.length > 1 && latencyGroups.length === 0;
  const hasMeasurementEvidence =
    hasStandaloneProbeCurve || latencyGroups.length > 0 || trendRows.length > 0;
  const activeEvidenceFilters =
    Number(Boolean(planId)) +
    Number(Boolean(clientId)) +
    Number(Boolean(source)) +
    Number(Boolean(kind)) +
    Number(Boolean(health)) +
    Number(Boolean(searchQuery.trim()));
  const selectedRangeLabel = networkEvidenceWindowLabel(evidenceWindow);
  const status = `${observations.length} observations / ${trends.length} trend groups in ${selectedRangeLabel}`;
  const evidenceFeedbackMessage =
    refreshError ??
    error ??
    (refreshing ? "Refreshing network evidence" : null);
  const outputFeedbackMessage =
    outputError ??
    (outputLoading ? "Loading retained command output" : outputNotice);

  function currentEvidenceQuery(
    windowOverride: MonitoringWindow = evidenceWindow,
  ): NetworkEvidenceQuery {
    return {
      clientId,
      endAt: customEndAt,
      health,
      kind,
      limit: NETWORK_EVIDENCE_OBSERVATION_LIMIT,
      planIds: planId ? [planId] : undefined,
      query: searchQuery,
      source,
      startAt: customStartAt,
      window: windowOverride,
    };
  }

  async function refreshEvidence(windowOverride = evidenceWindow) {
    const generation = refreshGenerationRef.current + 1;
    refreshGenerationRef.current = generation;
    setRefreshing(true);
    setRefreshError(null);
    try {
      const query = currentEvidenceQuery(windowOverride);
      await Promise.all([
        onLoadObservations(query),
        onLoadTrends({ ...query, limit: 10_000 }),
        onLoadOspfRecommendations(),
        onLoadOspfUpdatePlans(),
      ]);
      if (generation !== refreshGenerationRef.current) return;
      setAppliedPlanId(planId);
    } catch (loadError) {
      if (generation !== refreshGenerationRef.current) return;
      setRefreshError(
        loadError instanceof Error
          ? loadError.message
          : "Network evidence unavailable",
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
    void refreshEvidence(next);
  }

  async function resetEvidenceFilters() {
    const generation = refreshGenerationRef.current + 1;
    refreshGenerationRef.current = generation;
    setPlanId("");
    setClientId("");
    setSource("");
    setKind("");
    setHealth("");
    setSearchQuery("");
    setAppliedPlanId("");
    setRefreshing(true);
    setRefreshError(null);
    try {
      const query: NetworkEvidenceQuery = {
        endAt: customEndAt,
        limit: NETWORK_EVIDENCE_OBSERVATION_LIMIT,
        startAt: customStartAt,
        window: evidenceWindow,
      };
      await Promise.all([
        onLoadObservations(query),
        onLoadTrends({ ...query, limit: 10_000 }),
        onLoadOspfRecommendations(),
        onLoadOspfUpdatePlans(),
      ]);
      if (generation !== refreshGenerationRef.current) return;
    } catch (loadError) {
      if (generation !== refreshGenerationRef.current) return;
      setRefreshError(
        loadError instanceof Error
          ? loadError.message
          : "Network evidence unavailable",
      );
    } finally {
      if (generation === refreshGenerationRef.current) {
        setRefreshing(false);
      }
    }
  }

  async function loadCommandOutputs() {
    setOutputLoading(true);
    setOutputError(null);
    setOutputNotice(null);
    try {
      const outputEntries = await Promise.all(
        networkJobs.map(
          async (job) => [job.id, await onLoadOutputs(job.id)] as const,
        ),
      );
      setOutputsByJob(Object.fromEntries(outputEntries));
      setOutputNotice(
        `Loaded retained output for ${outputEntries.length} network job${outputEntries.length === 1 ? "" : "s"}`,
      );
    } catch (loadError) {
      setOutputError(
        loadError instanceof Error
          ? loadError.message
          : "Retained command output unavailable",
      );
    } finally {
      setOutputLoading(false);
    }
  }

  function scrollToTrendComparison() {
    document.getElementById("topology-evidence-trends")?.scrollIntoView({
      block: "start",
      behavior: "smooth",
    });
  }

  return (
    <section className="fleetPanel topologyEvidence">
      <div className="sectionHeader">
        <div>
          <h2>Network evidence</h2>
          <span>{status}</span>
        </div>
        <div className="headerActionStack">
          <button
            className="secondaryAction"
            disabled={refreshing}
            onClick={() => void refreshEvidence()}
            title={
              refreshing
                ? "Network evidence is already refreshing"
                : "Refresh retained topology measurements and command evidence for the selected range"
            }
            type="button"
          >
            <RefreshCcw size={17} />
            Refresh evidence
          </button>
          <ActionFeedback
            message={evidenceFeedbackMessage}
            tone={
              refreshError || error
                ? "danger"
                : refreshing
                  ? "progress"
                  : "success"
            }
          />
        </div>
      </div>
      <div
        className="observabilityMetricsControls"
        aria-label="Network evidence controls"
      >
        <NetworkEvidenceRangeControls
          ariaLabel="Network evidence time range"
          endAt={customEndAt}
          onEndAtChange={setCustomEndAt}
          onStartAtChange={setCustomStartAt}
          onWindowChange={selectEvidenceWindow}
          startAt={customStartAt}
          window={evidenceWindow}
        />
        <details
          className="fleetMetricsAdvancedFilters"
          title="Restrict retained network evidence by plan, endpoint, source, measurement, health, or text"
        >
          <summary>
            <SlidersHorizontal size={14} />
            <span>Advanced filters</span>
            {activeEvidenceFilters > 0 ? <b>{activeEvidenceFilters}</b> : null}
          </summary>
          <div className="dashboardControlBar fleetMetricsAdvancedFilterGrid">
            <label>
              <span>Tunnel plan</span>
              <select
                aria-label="Network evidence tunnel plan"
                onChange={(event) => setPlanId(event.target.value)}
                value={planId}
              >
                <option value="">All visible tunnel plans</option>
                {tunnelPlans.map((plan) => (
                  <option key={plan.id} value={plan.id}>
                    {plan.name}
                    {plan.enabled ? "" : " · disabled"}
                  </option>
                ))}
              </select>
            </label>
            <label>
              <span>VPS endpoint</span>
              <VpsCombobox
                agents={agents}
                ariaLabel="Network evidence VPS endpoint"
                onChange={setClientId}
                placeholder="All VPS endpoints"
                value={clientId}
              />
            </label>
            <label>
              <span>Source</span>
              <select
                aria-label="Network evidence source"
                onChange={(event) =>
                  setSource(event.target.value as NetworkEvidenceSource)
                }
                value={source}
              >
                <option value="">Automatic and manual</option>
                <option value="automatic">Automatic monitor</option>
                <option value="manual">Manual test</option>
              </select>
            </label>
            <label>
              <span>Measurement</span>
              <select
                aria-label="Network evidence measurement kind"
                onChange={(event) =>
                  setKind(event.target.value as NetworkEvidenceKind)
                }
                value={kind}
              >
                <option value="">All measurement kinds</option>
                <option value="tunnel_reachability">Reachability</option>
                <option value="network_speed_test">Speed test</option>
                <option value="network_status">Runtime status</option>
              </select>
            </label>
            <label>
              <span>Health</span>
              <select
                aria-label="Network evidence health"
                onChange={(event) =>
                  setHealth(event.target.value as NetworkEvidenceHealth)
                }
                value={health}
              >
                <option value="">All states</option>
                <option value="healthy">Healthy</option>
                <option value="unhealthy">Unhealthy</option>
                <option value="unknown">Unknown</option>
              </select>
            </label>
            <label>
              <span>Search</span>
              <input
                aria-label="Search network evidence"
                onChange={(event) => setSearchQuery(event.target.value)}
                placeholder="Plan, interface, target, reason"
                type="search"
                value={searchQuery}
              />
            </label>
            <div className="dashboardScopeHint">
              The selected range and filters are applied by the API. With no
              plan filter, every visible plan is eligible and represented
              independently.
            </div>
            <button
              className="secondaryAction compactAction"
              disabled={refreshing}
              onClick={() => void refreshEvidence()}
              title={
                refreshing
                  ? "Network evidence is already refreshing"
                  : "Apply the selected range and advanced evidence filters"
              }
              type="button"
            >
              Apply filters
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={activeEvidenceFilters === 0}
              onClick={() => void resetEvidenceFilters()}
              title={
                activeEvidenceFilters === 0
                  ? "No advanced network evidence filters are active"
                  : `Reset ${activeEvidenceFilters} active advanced evidence filter${activeEvidenceFilters === 1 ? "" : "s"}`
              }
              type="button"
            >
              Reset filters
            </button>
          </div>
        </details>
      </div>
      {observations.length >= NETWORK_EVIDENCE_OBSERVATION_LIMIT ? (
        <ActionFeedback
          message="This range reached the 250,000-observation display limit. Narrow the range or filters to inspect every sample."
          tone="warning"
        />
      ) : null}
      {freshness?.stale && (
        <div
          className="topologyEvidenceFreshness warning"
          aria-label="Network evidence freshness"
          title={
            freshness.latestTimestamp
              ? `Latest evidence: ${formatFullTime(freshness.latestTimestamp)}`
              : undefined
          }
        >
          <strong>
            Reachability evidence was observed {freshness.observedLabel}.
          </strong>
          <span>{freshness.detail}</span>
        </div>
      )}
      <div
        className="topologyEvidenceTimeline"
        aria-label="Network evidence timeline"
      >
        <div className="topologyTimelineIntro">
          <strong>Evidence timeline</strong>
          <span>
            Read left to right: observed state, measured probes, speed evidence,
            status checks, cost recommendation, approval path.
          </span>
        </div>
        {timelineStages.map((stage) => (
          <div
            className={stage.tone ? stage.tone : undefined}
            key={stage.label}
          >
            <span>{stage.label}</span>
            <strong>{stage.value}</strong>
            <p>{stage.detail}</p>
          </div>
        ))}
      </div>
      <div
        className="topologyEvidenceActions"
        aria-label="Network evidence actions"
      >
        <button
          className="secondaryAction compactAction"
          disabled={!onOpenGraph}
          onClick={onOpenGraph}
          title="Open the read-only network topology graph"
          type="button"
        >
          <MapIcon size={16} />
          <span>Open graph</span>
        </button>
        <button
          className="secondaryAction compactAction"
          disabled={!onOpenTests}
          onClick={onOpenTests}
          title="Run reviewed status, probe, and speed diagnostics"
          type="button"
        >
          <Activity size={16} />
          <span>Run tests</span>
        </button>
        <button
          className="secondaryAction compactAction"
          disabled={!onOpenTunnelPlans}
          onClick={onOpenTunnelPlans}
          title="Open tunnel plans for declaration, lifecycle, allocation, and export workflows"
          type="button"
        >
          <GitBranch size={16} />
          <span>Tunnel plans</span>
        </button>
        <button
          className="secondaryAction compactAction"
          disabled={outputLoading || networkJobs.length === 0}
          onClick={loadCommandOutputs}
          title={
            networkJobs.length === 0
              ? "No network jobs have retained output"
              : "Load retained command output for the recent network jobs"
          }
          type="button"
        >
          <RefreshCcw size={16} />
          <span>{hasUnloadedOutput ? "Load output" : "Reload output"}</span>
        </button>
        <button
          className="secondaryAction compactAction"
          disabled={!hasTrendComparison}
          onClick={scrollToTrendComparison}
          title={
            hasTrendComparison
              ? "Jump to trend ranges that compare recent observations"
              : "No comparable trend ranges are available"
          }
          type="button"
        >
          <GitCompareArrows size={16} />
          <span>Compare to previous</span>
        </button>
        <button
          className="secondaryAction compactAction"
          disabled={!onOpenOspfApprovals}
          onClick={onOpenOspfApprovals}
          title={
            onOpenOspfApprovals
              ? "Open optional OSPF routing-cost control"
              : "OSPF routing-cost control is unavailable in this context"
          }
          type="button"
        >
          <ShieldCheck size={16} />
          <span>Open OSPF</span>
        </button>
      </div>
      <ActionFeedback
        className="localActionFeedback"
        message={outputFeedbackMessage}
        tone={outputError ? "danger" : outputLoading ? "progress" : "success"}
      />
      {(ospfUpdateRows.length > 0 || ospfRows.length > 0) && (
        <EvidenceGroup
          detail="Cost proposals are separated from measurements so confidence never substitutes for link health."
          title="Recommendation evidence"
        >
          {ospfUpdateRows.length > 0 && (
            <div
              aria-label="OSPF update plan evidence"
              className="table historyTable trendTable"
              role="table"
            >
              <div
                className="historyRow heading topologyEvidenceGrid"
                role="row"
              >
                <span role="columnheader">OSPF update plan</span>
                <span role="columnheader">Health</span>
                <span role="columnheader">Cost</span>
                <span role="columnheader">Approval</span>
                <span role="columnheader">Latest</span>
              </div>
              {ospfUpdateRows.map((row) => (
                <div
                  className="historyRow topologyEvidenceGrid"
                  key={row.id}
                  role="row"
                  title={`${row.planName}; interface ${row.interfaceName}; confidence ${row.confidence}. Health: ${row.signalLabel}; ${row.healthDetail}. Cost: ${row.metric}; ${row.metricDetail}. Approval: ${row.target}; ${row.targetDetail}. Latest: ${row.latestObservedAt ? formatFullTime(row.latestObservedAt) : "not reported"}.`}
                >
                  <span className="historyPrimary" role="cell">
                    <EvidenceMobileLabel>OSPF update plan</EvidenceMobileLabel>
                    <strong>{row.planName}</strong>
                    <small>{row.interfaceName}</small>
                    <small>{row.confidence}</small>
                  </span>
                  <span className="topologyEvidenceStatusCell" role="cell">
                    <EvidenceMobileLabel>Health</EvidenceMobileLabel>
                    <span
                      className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}
                    >
                      {row.signalLabel}
                    </span>
                  </span>
                  <span className="topologyMetric" role="cell">
                    <EvidenceMobileLabel>Cost</EvidenceMobileLabel>
                    <strong>{row.metric}</strong>
                    <small>{row.metricDetail}</small>
                  </span>
                  <span className="topologyMetric" role="cell">
                    <EvidenceMobileLabel>Approval</EvidenceMobileLabel>
                    <strong>{row.target}</strong>
                    <small>{row.targetDetail}</small>
                  </span>
                  <EvidenceTime label="Latest" value={row.latestObservedAt} />
                </div>
              ))}
            </div>
          )}
          {ospfRows.length > 0 && (
            <div
              aria-label="OSPF recommendation evidence"
              className="table historyTable trendTable"
              role="table"
            >
              <div
                className="historyRow heading topologyEvidenceGrid"
                role="row"
              >
                <span role="columnheader">OSPF recommendation</span>
                <span role="columnheader">Health</span>
                <span role="columnheader">Cost</span>
                <span role="columnheader">Evidence</span>
                <span role="columnheader">Latest</span>
              </div>
              {ospfRows.map((row) => (
                <div
                  className="historyRow topologyEvidenceGrid"
                  key={row.id}
                  role="row"
                  title={`${row.planName}; interface ${row.interfaceName}; confidence ${row.confidence}. Health: ${row.signalLabel}; ${row.healthDetail}. Cost: ${row.metric}; ${row.metricDetail}. Evidence: ${row.target}; ${row.targetDetail}. Latest: ${row.latestObservedAt ? formatFullTime(row.latestObservedAt) : "not reported"}.`}
                >
                  <span className="historyPrimary" role="cell">
                    <EvidenceMobileLabel>
                      OSPF recommendation
                    </EvidenceMobileLabel>
                    <strong>{row.planName}</strong>
                    <small>{row.interfaceName}</small>
                    <small>{row.confidence}</small>
                  </span>
                  <span className="topologyEvidenceStatusCell" role="cell">
                    <EvidenceMobileLabel>Health</EvidenceMobileLabel>
                    <span
                      className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}
                    >
                      {row.signalLabel}
                    </span>
                  </span>
                  <span className="topologyMetric" role="cell">
                    <EvidenceMobileLabel>Cost</EvidenceMobileLabel>
                    <strong>{row.metric}</strong>
                    <small>{row.metricDetail}</small>
                  </span>
                  <span className="topologyMetric" role="cell">
                    <EvidenceMobileLabel>Evidence</EvidenceMobileLabel>
                    <strong>{row.target}</strong>
                    <small>{row.targetDetail}</small>
                  </span>
                  <EvidenceTime label="Latest" value={row.latestObservedAt} />
                </div>
              ))}
            </div>
          )}
        </EvidenceGroup>
      )}
      {hasMeasurementEvidence && (
        <EvidenceGroup
          detail="Probe and speed-test trends use retained observations; empty curves are hidden until enough points exist."
          title="Measurement evidence"
        >
          {hasStandaloneProbeCurve && (
            <div
              className="latencyCurve"
              aria-label="Network probe latency history"
            >
              {probePoints.map((point) => (
                <span
                  className={
                    point.latencyAvgMs === null
                      ? "gap"
                      : point.lossRatio === null
                        ? "unknown"
                        : point.lossRatio > 0
                          ? "warn"
                          : "ok"
                  }
                  key={point.jobId}
                  style={{
                    height:
                      point.latencyAvgMs === null
                        ? "2px"
                        : `${Math.max(8, Math.round((point.latencyAvgMs / maxLatency) * 44))}px`,
                  }}
                  title={
                    point.latencyAvgMs === null
                      ? `Measurement gap${point.reason ? `; ${humanStatus(point.reason)}` : point.healthy === false ? "; probe failed" : ""}`
                      : `${formatMetric(point.latencyAvgMs)} ms avg; packet loss ${
                          point.lossRatio === null
                            ? "unknown"
                            : `${formatMetric(point.lossRatio * 100)}%`
                        }`
                  }
                />
              ))}
            </div>
          )}
          {latencyGroups.length > 0 && (
            <div
              className="latencyCurveGroups"
              aria-label="Per tunnel latency curves"
            >
              {latencyGroups.map((group) => (
                <div
                  className="latencyCurveCard"
                  key={group.key}
                  title={`${group.label}: ${group.detail}`}
                >
                  <span className="latencyCurveTitle">
                    <strong>{group.label}</strong>
                    <small>{group.detail}</small>
                  </span>
                  <div
                    className="latencyCurve compact"
                    aria-label={`${group.label} latency curve`}
                  >
                    {group.points.map((point, index) => (
                      <span
                        className={
                          point.latencyAvgMs === null
                            ? "gap"
                            : point.lossRatio === null
                              ? "unknown"
                              : point.lossRatio > 0
                                ? "warn"
                                : "ok"
                        }
                        key={`${group.key}-${index}`}
                        style={{
                          height:
                            point.latencyAvgMs === null
                              ? "2px"
                              : `${Math.max(8, Math.round((point.latencyAvgMs / group.maxLatency) * 38))}px`,
                        }}
                        title={
                          point.latencyAvgMs === null
                            ? `Measurement gap${point.reason ? `; ${humanStatus(point.reason)}` : point.healthy === false ? "; probe failed" : ""}`
                            : `${formatMetric(point.latencyAvgMs)} ms avg; packet loss ${
                                point.lossRatio === null
                                  ? "unknown"
                                  : `${formatMetric(point.lossRatio * 100)}%`
                              }`
                        }
                      />
                    ))}
                  </div>
                </div>
              ))}
            </div>
          )}
          {trendRows.length > 0 && (
            <div
              aria-label="Network observation trends"
              className="table historyTable trendTable"
              id="topology-evidence-trends"
              role="table"
            >
              <div
                className="historyRow heading topologyEvidenceGrid"
                role="row"
              >
                <span role="columnheader">Trend</span>
                <span role="columnheader">Health</span>
                <span role="columnheader">Metric</span>
                <span role="columnheader">Endpoint</span>
                <span role="columnheader">Latest</span>
              </div>
              {trendRows.map((row) => (
                <div
                  className="historyRow topologyEvidenceGrid"
                  key={row.id}
                  role="row"
                >
                  <span className="historyPrimary" role="cell">
                    <EvidenceMobileLabel>Trend</EvidenceMobileLabel>
                    <strong>{humanStatus(row.kind)}</strong>
                    <small>{row.sampleCount} samples</small>
                  </span>
                  <span className="topologyEvidenceStatusCell" role="cell">
                    <EvidenceMobileLabel>Health</EvidenceMobileLabel>
                    <span
                      className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}
                    >
                      {row.signalLabel}
                    </span>
                  </span>
                  <span className="topologyMetric" role="cell">
                    <EvidenceMobileLabel>Metric</EvidenceMobileLabel>
                    <strong>{row.metric}</strong>
                    <small>{row.metricDetail}</small>
                  </span>
                  <span className="topologyMetric" role="cell">
                    <EvidenceMobileLabel>Endpoint</EvidenceMobileLabel>
                    <strong>{row.target}</strong>
                    <small>{row.targetDetail}</small>
                  </span>
                  <EvidenceTime label="Latest" value={row.latestObservedAt} />
                </div>
              ))}
            </div>
          )}
        </EvidenceGroup>
      )}
      {observationRows.length > 0 && (
        <EvidenceGroup
          detail="Persisted status, probe, and speed-test observations remain separate from recommendations and related jobs."
          title="Status and probe results"
        >
          <div
            aria-label="Status and probe observations"
            className="table historyTable observationTable"
            role="table"
          >
            <div className="historyRow heading topologyEvidenceGrid" role="row">
              <span role="columnheader">Observation</span>
              <span role="columnheader">Signal</span>
              <span role="columnheader">Metric</span>
              <span role="columnheader">Target</span>
              <span role="columnheader">Observed</span>
            </div>
            {observationRows.map((row) => (
              <div
                className="historyRow topologyEvidenceGrid"
                key={row.id}
                role="row"
              >
                <span className="historyPrimary" role="cell">
                  <EvidenceMobileLabel>Observation</EvidenceMobileLabel>
                  <strong>{humanStatus(row.kind)}</strong>
                  <small>
                    {row.source === "automatic"
                      ? "automatic monitor"
                      : row.jobId
                        ? `manual job ${shortId(row.jobId)}`
                        : "manual observation"}
                  </small>
                </span>
                <span className="topologyEvidenceStatusCell" role="cell">
                  <EvidenceMobileLabel>Signal</EvidenceMobileLabel>
                  <span
                    className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}
                  >
                    {row.signalLabel}
                  </span>
                </span>
                <span className="topologyMetric" role="cell">
                  <EvidenceMobileLabel>Metric</EvidenceMobileLabel>
                  <strong>{row.metric}</strong>
                  <small>{row.metricDetail}</small>
                </span>
                <span className="topologyMetric" role="cell">
                  <EvidenceMobileLabel>Target</EvidenceMobileLabel>
                  <strong>{row.target}</strong>
                  <small>{row.targetDetail}</small>
                </span>
                <EvidenceTime label="Observed" value={row.observedAt} />
              </div>
            ))}
          </div>
        </EvidenceGroup>
      )}
      <EvidenceGroup
        detail="Command rows explain retained-output state and link to job detail without turning evidence review into a mutation page."
        title="Related command jobs"
      >
        <div
          aria-label="Related topology command jobs"
          className="table historyTable"
          role="table"
        >
          <div className="historyRow heading topologyEvidenceGrid" role="row">
            <span role="columnheader">Command</span>
            <span role="columnheader">Signal</span>
            <span role="columnheader">Metric</span>
            <span role="columnheader">Target</span>
            <span role="columnheader">Created</span>
          </div>
          {rows.map((row) => {
            const signalLabel =
              row.signalLabel ?? humanStatus(row.signalStatus);
            return (
              <div
                className="historyRow topologyEvidenceGrid"
                key={row.job.id}
                role="row"
                title={`${humanStatus(row.job.command_type)} job ${row.job.id}. Signal: ${signalLabel}. Metric: ${row.metric}; ${row.metricDetail}. Target: ${row.target}; ${row.targetDetail}. Created: ${formatFullTime(row.job.created_at)}.`}
              >
                <span className="historyPrimary" role="cell">
                  <EvidenceMobileLabel>Command</EvidenceMobileLabel>
                  <strong>{humanStatus(row.job.command_type)}</strong>
                  <small>job {shortId(row.job.id)}</small>
                  {onOpenJobDetails ? (
                    <button
                      className="secondaryAction compactAction"
                      onClick={() => onOpenJobDetails(row.job.id)}
                      title={`Open retained job detail for ${row.job.id}`}
                      type="button"
                    >
                      <ExternalLink size={14} />
                      <span>Open job details</span>
                    </button>
                  ) : null}
                </span>
                <span className="topologyEvidenceStatusCell" role="cell">
                  <EvidenceMobileLabel>Signal</EvidenceMobileLabel>
                  <span className={`status ${evidenceStatusBadgeClass(row)}`}>
                    {signalLabel}
                  </span>
                </span>
                <span className="topologyMetric" role="cell">
                  <EvidenceMobileLabel>Metric</EvidenceMobileLabel>
                  <strong>{row.metric}</strong>
                  <small>{row.metricDetail}</small>
                </span>
                <span className="topologyMetric" role="cell">
                  <EvidenceMobileLabel>Target</EvidenceMobileLabel>
                  <strong>{row.target}</strong>
                  <small>{row.targetDetail}</small>
                </span>
                <EvidenceTime label="Created" value={row.job.created_at} />
              </div>
            );
          })}
        </div>
        {rows.length === 0 && (
          <div className="emptyState">
            <Activity size={22} />
            <strong>No topology evidence</strong>
            <span>
              Sync, status, probe, and speed-test results will appear here.
            </span>
          </div>
        )}
      </EvidenceGroup>
    </section>
  );
}

function EvidenceGroup({
  children,
  detail,
  title,
}: {
  children: ReactNode;
  detail: string;
  title: string;
}) {
  return (
    <section className="topologyEvidenceGroup" aria-label={title}>
      <div className="topologyEvidenceGroupHeader">
        <strong>{title}</strong>
        <span>{detail}</span>
      </div>
      {children}
    </section>
  );
}

function EvidenceTime({
  fallback = "pending",
  label,
  value,
}: {
  fallback?: string;
  label: string;
  value: string | null;
}) {
  if (!value) {
    return (
      <span className="topologyEvidenceTimeCell" role="cell">
        <EvidenceMobileLabel>{label}</EvidenceMobileLabel>
        <span>{fallback}</span>
      </span>
    );
  }
  return (
    <span className="topologyEvidenceTimeCell" role="cell">
      <EvidenceMobileLabel>{label}</EvidenceMobileLabel>
      <time dateTime={value} title={formatFullTime(value)}>
        {formatCompactTime(value)}
      </time>
    </span>
  );
}

function EvidenceMobileLabel({ children }: { children: ReactNode }) {
  return (
    <span aria-hidden="true" className="topologyEvidenceMobileLabel">
      {children}
    </span>
  );
}

type EvidenceRow = {
  job: JobHistoryRecord;
  kind: string;
  signalKind: "job" | "observation" | "runtime";
  signalLabel?: string;
  signalStatus: JobStatus | TopologyObservationState | TopologyRuntimeState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latencyAvgMs?: number;
  lossRatio?: number;
};

type LatencyCurveGroup = {
  key: string;
  label: string;
  detail: string;
  maxLatency: number;
  points: {
    healthy: boolean | null;
    latencyAvgMs: number | null;
    lossRatio: number | null;
    reason: string | null;
  }[];
};

type ObservationRow = {
  id: string;
  jobId: string | null;
  source: string;
  kind: string;
  signalLabel: string;
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  observedAt: string;
};

type TrendRow = {
  id: string;
  kind: string;
  sampleCount: number;
  signalLabel: string;
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latestObservedAt: string;
};

type OspfRecommendationRow = {
  confidence: string;
  healthDetail: string;
  id: string;
  planName: string;
  interfaceName: string;
  signalLabel: string;
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latestObservedAt: string | null;
};

type OspfUpdatePlanRow = {
  confidence: string;
  healthDetail: string;
  id: string;
  planName: string;
  interfaceName: string;
  signalLabel: string;
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latestObservedAt: string | null;
};

type TimelineStage = {
  detail: string;
  label: string;
  tone?: "attention" | "ready";
  value: string;
};

type ThroughputBaseline = {
  configuredBandwidthMbps: number;
  effectiveBandwidthMbps: number;
};

type ThroughputBaselineIdentity = {
  interfaceName?: string | null;
  planId?: string | null;
  planName?: string | null;
  topologyIdentityHash?: string | null;
};

type NetworkEvidenceFreshness = {
  detail: string;
  latestTimestamp: string | null;
  observedLabel: string;
  stale: boolean;
};

function buildTimelineStages({
  commandRows,
  observationRows,
  ospfRecommendationRows,
  ospfUpdateRows,
  trendRows,
}: {
  commandRows: EvidenceRow[];
  observationRows: ObservationRow[];
  ospfRecommendationRows: OspfRecommendationRow[];
  ospfUpdateRows: OspfUpdatePlanRow[];
  trendRows: TrendRow[];
}): TimelineStage[] {
  const persistedProbeCount = observationRows.filter(
    (row) => row.kind === "tunnel_reachability",
  ).length;
  const persistedSpeedCount = observationRows.filter(
    (row) => row.kind === "network_speed_test",
  ).length;
  const statusCount =
    observationRows.filter((row) => row.kind === "network_status").length +
    commandRows.filter(
      (row) =>
        row.kind === "network_status" || row.kind === "runtime_config_sync",
    ).length;
  const unloadedOutputCount = commandRows.filter(
    (row) => row.metric === "Output not loaded",
  ).length;
  const probeTrend = trendRows.find(
    (row) => row.kind === "tunnel_reachability",
  );
  const speedTrend = trendRows.find((row) => row.kind === "network_speed_test");
  return [
    {
      detail:
        observationRows.length > 0
          ? `Latest persisted observation ${formatTime(latestObservedAt(observationRows))}`
          : "No persisted topology observations yet",
      label: "Observation",
      value: `${observationRows.length} records`,
    },
    {
      detail: probeTrend
        ? `${probeTrend.metric}; ${probeTrend.metricDetail}`
        : "Run or refresh network probes to build latency evidence",
      label: "Probe",
      value: `${persistedProbeCount} persisted`,
      tone: persistedProbeCount > 0 ? "ready" : undefined,
    },
    {
      detail: speedTrend
        ? `${speedTrend.signalLabel}; ${speedTrend.metric}; ${speedTrend.metricDetail}`
        : "Run speed tests to add measured throughput evidence",
      label: "Speed test",
      value: `${persistedSpeedCount} persisted`,
      tone: persistedSpeedCount > 0 ? "ready" : undefined,
    },
    {
      detail:
        statusCount > 0
          ? "Runtime status evidence is available in observations or retained command output."
          : "No status check evidence loaded.",
      label: "Status check",
      value: `${statusCount} checks`,
    },
    {
      detail: ospfRecommendationRows[0]
        ? `${ospfRecommendationRows[0].metric}; ${ospfRecommendationRows[0].target}`
        : "No cost recommendation generated",
      label: "Recommended cost",
      value: `${ospfRecommendationRows.length} plans`,
    },
    {
      detail:
        ospfUpdateRows.length > 0
          ? "Apply the reviewed recommendation in Network / OSPF."
          : "No approval-required cost update pending.",
      label: "Approval",
      tone: ospfUpdateRows.length > 0 ? "attention" : undefined,
      value: `${ospfUpdateRows.length} pending`,
    },
    {
      detail:
        unloadedOutputCount > 0
          ? "Use Load output to fetch retained job output for visible commands."
          : "All visible command outputs are loaded, parsed, or accounted for.",
      label: "Command output",
      tone: unloadedOutputCount > 0 ? "attention" : "ready",
      value:
        unloadedOutputCount > 0
          ? `${unloadedOutputCount} outputs not loaded`
          : "Loaded",
    },
  ];
}

function buildOspfUpdatePlanRow(
  plan: NetworkOspfUpdatePlanRecord,
): OspfUpdatePlanRow {
  const proposalStatus =
    plan.status === "noop"
      ? "healthy"
      : plan.status === "review_degraded" ||
          plan.status === "adapter_unavailable"
        ? "degraded"
        : plan.status === "needs_adapter_status" ||
            plan.status === "automatic_waiting_evidence"
          ? "unknown"
          : "recorded";
  const bandwidthHealth = bandwidthEvidenceHealth({
    configuredBandwidthMbps: plan.evidence.configured_bandwidth_mbps,
    effectiveBandwidthMbps: plan.evidence.effective_bandwidth_mbps,
    measuredThroughputMbps: plan.evidence.throughput_avg_mbps,
  });
  const signalStatus =
    bandwidthHealth.signalStatus === "degraded" ? "degraded" : proposalStatus;
  const delta =
    plan.maximum_cost_delta === 0
      ? "unchanged"
      : `max ${plan.maximum_cost_delta}`;
  const privilegeState = plan.privilege_required
    ? "privilege required"
    : plan.control_mode === "automatic"
      ? "server controlled"
      : "read-only";
  return {
    id: plan.recommendation_id,
    planName: plan.plan_name,
    interfaceName: plan.interface_name,
    confidence: `Confidence ${humanStatus(plan.confidence)}`,
    healthDetail: bandwidthHealth.detail,
    signalLabel: bandwidthHealth.label,
    signalStatus,
    metric: `${plan.left_current_ospf_cost ?? "?"} / ${plan.right_current_ospf_cost ?? "?"} -> ${plan.recommended_ospf_cost}`,
    metricDetail: `${delta}; ${bandwidthHealth.summary}`,
    target: plan.requires_approval ? "approval required" : "no action",
    targetDetail: plan.requires_approval
      ? `${bandwidthHealth.summary}; ${evidenceSampleSummary(plan.evidence.sample_count, plan.evidence.latest_observed_at)}; ${privilegeState}; ${plan.approval_scope.length} approval scopes`
      : `${bandwidthHealth.summary}; ${evidenceSampleSummary(plan.evidence.sample_count, plan.evidence.latest_observed_at)}; ${privilegeState}`,
    latestObservedAt: plan.evidence.latest_observed_at,
  };
}

function buildOspfRecommendationRow(
  recommendation: NetworkOspfRecommendationRecord,
): OspfRecommendationRow {
  const bandwidthHealth = bandwidthEvidenceHealth({
    configuredBandwidthMbps: recommendation.configured_bandwidth_mbps,
    effectiveBandwidthMbps: recommendation.effective_bandwidth_mbps,
    measuredThroughputMbps: recommendation.throughput_avg_mbps,
  });
  const signalStatus =
    bandwidthHealth.signalStatus === "degraded"
      ? "degraded"
      : recommendation.confidence === "measured"
        ? recommendation.degraded_count > 0
          ? "degraded"
          : "healthy"
        : recommendation.confidence === "no_recent_observations"
          ? "unknown"
          : "recorded";
  const delta =
    recommendation.cost_delta === 0
      ? "unchanged"
      : recommendation.cost_delta > 0
        ? `+${recommendation.cost_delta}`
        : String(recommendation.cost_delta);
  const evidence =
    recommendation.latency_avg_ms !== null
      ? `${formatMetric(recommendation.latency_avg_ms)} ms; ${formatLoss(recommendation.packet_loss_avg_ratio)} loss`
      : recommendation.reason;
  const throughput =
    recommendation.throughput_avg_mbps === null
      ? `burst ${formatBandwidthMbps(recommendation.effective_bandwidth_mbps)}`
      : `${formatMetric(recommendation.throughput_avg_mbps)} Mbps avg; burst ${formatBandwidthMbps(recommendation.effective_bandwidth_mbps)}`;
  return {
    id: recommendation.recommendation_id,
    planName: recommendation.plan_name,
    interfaceName: recommendation.interface_name,
    confidence: `Confidence ${humanStatus(recommendation.confidence)}`,
    healthDetail: bandwidthHealth.detail,
    signalLabel: bandwidthHealth.label,
    signalStatus,
    metric: `${recommendation.plan_ospf_cost} -> ${recommendation.recommended_ospf_cost}`,
    metricDetail: `${delta}; ${bandwidthHealth.summary}`,
    target: evidence,
    targetDetail: `${bandwidthHealth.summary}; ${evidenceSampleSummary(recommendation.sample_count, recommendation.latest_observed_at)}; ${recommendation.reason || throughput}`,
    latestObservedAt: recommendation.latest_observed_at,
  };
}

function bandwidthEvidenceHealth({
  configuredBandwidthMbps,
  effectiveBandwidthMbps,
  measuredThroughputMbps,
}: {
  configuredBandwidthMbps: number;
  effectiveBandwidthMbps: number;
  measuredThroughputMbps: number | null;
}): {
  detail: string;
  label: string;
  signalStatus: TopologyObservationState;
  summary: string;
} {
  const measured = measuredThroughputMbps ?? effectiveBandwidthMbps;
  if (
    !Number.isFinite(configuredBandwidthMbps) ||
    configuredBandwidthMbps <= 0 ||
    !Number.isFinite(measured)
  ) {
    return {
      detail: "Configured bandwidth baseline is unavailable.",
      label: "Baseline unavailable",
      signalStatus: "recorded",
      summary: "baseline unavailable",
    };
  }
  const percent = Math.round((measured / configuredBandwidthMbps) * 100);
  const measuredLabel =
    measuredThroughputMbps === null
      ? formatBandwidthMbps(effectiveBandwidthMbps)
      : `${formatMetric(measuredThroughputMbps)} Mbps avg`;
  const summary = `${measuredLabel} - ${percent}% of expected ${formatBandwidthMbps(configuredBandwidthMbps)}`;
  const effectiveDetail =
    measuredThroughputMbps === null
      ? `effective ${formatBandwidthMbps(effectiveBandwidthMbps)}`
      : `effective ${formatBandwidthMbps(effectiveBandwidthMbps)}; measured ${formatMetric(measuredThroughputMbps)} Mbps avg`;
  const signalStatus = percent < 80 ? "degraded" : "healthy";
  return {
    detail: `${summary}; ${effectiveDetail}`,
    label:
      signalStatus === "degraded"
        ? "Degraded throughput"
        : measuredThroughputMbps === null
          ? "Configured bandwidth"
          : "Throughput healthy",
    signalStatus,
    summary,
  };
}

function formatBandwidthMbps(value: number): string {
  return `${Math.round(value)} Mbps`;
}

function evidenceSampleSummary(
  sampleCount: number,
  latestObservedAt: string | null,
): string {
  const latest = latestObservedAt
    ? `; latest ${formatCompactTime(latestObservedAt)}`
    : "";
  return `${sampleCount} sample${sampleCount === 1 ? "" : "s"}${latest}`;
}

function buildThroughputBaselineLookup(
  recommendations: NetworkOspfRecommendationRecord[],
  updatePlans: NetworkOspfUpdatePlanRecord[],
): Map<string, ThroughputBaseline> {
  const lookup = new Map<string, ThroughputBaseline>();
  for (const recommendation of recommendations) {
    addThroughputBaseline(
      lookup,
      {
        configuredBandwidthMbps: recommendation.configured_bandwidth_mbps,
        effectiveBandwidthMbps: recommendation.effective_bandwidth_mbps,
      },
      {
        interfaceName: recommendation.interface_name,
        planId: recommendation.plan_id,
        planName: recommendation.plan_name,
      },
    );
  }
  for (const plan of updatePlans) {
    addThroughputBaseline(
      lookup,
      {
        configuredBandwidthMbps: plan.evidence.configured_bandwidth_mbps,
        effectiveBandwidthMbps: plan.evidence.effective_bandwidth_mbps,
      },
      {
        interfaceName: plan.interface_name,
        planId: plan.plan_id,
        planName: plan.plan_name,
      },
    );
  }
  return lookup;
}

function addThroughputBaseline(
  lookup: Map<string, ThroughputBaseline>,
  baseline: ThroughputBaseline,
  identity: ThroughputBaselineIdentity,
) {
  if (
    !Number.isFinite(baseline.configuredBandwidthMbps) ||
    !Number.isFinite(baseline.effectiveBandwidthMbps)
  ) {
    return;
  }
  for (const key of throughputBaselineKeys(identity)) {
    if (!lookup.has(key)) {
      lookup.set(key, baseline);
    }
  }
}

function throughputBaselineFor(
  identity: ThroughputBaselineIdentity,
  lookup: Map<string, ThroughputBaseline>,
): ThroughputBaseline | null {
  for (const key of throughputBaselineKeys(identity)) {
    const baseline = lookup.get(key);
    if (baseline) {
      return baseline;
    }
  }
  return null;
}

function throughputBaselineKeys(
  identity: ThroughputBaselineIdentity,
): string[] {
  const keys: string[] = [];
  if (identity.planId) {
    keys.push(`plan-id:${identity.planId}`);
  }
  if (identity.topologyIdentityHash) {
    keys.push(`topology:${identity.topologyIdentityHash}`);
  }
  if (identity.planName && identity.interfaceName) {
    keys.push(`plan-interface:${identity.planName}:${identity.interfaceName}`);
  }
  if (identity.planName) {
    keys.push(`plan-name:${identity.planName}`);
  }
  return keys;
}

function throughputSampleSignalLabel(
  sampleStatus: TopologyObservationState,
  observedAt: string,
  throughputHealth: ReturnType<typeof bandwidthEvidenceHealth> | null,
): string {
  const sampleLabel = sampleFreshnessLabel(sampleStatus, observedAt);
  if (throughputHealth?.signalStatus === "degraded") {
    return `${sampleLabel} · degraded throughput`;
  }
  if (throughputHealth?.signalStatus === "healthy") {
    return `${sampleLabel} · throughput within baseline`;
  }
  if (isEvidenceTimestampStale(observedAt)) {
    return `${sampleLabel} · not enough current evidence`;
  }
  return sampleLabel;
}

function sampleFreshnessLabel(
  status: TopologyObservationState,
  observedAt: string,
): string {
  if (isEvidenceTimestampStale(observedAt)) {
    return status === "degraded" ? "Stale failed sample" : "Stale sample";
  }
  if (status === "healthy") {
    return "Valid sample";
  }
  if (status === "degraded") {
    return "Failed sample";
  }
  return "Recorded sample";
}

function buildNetworkEvidenceFreshness(
  observations: NetworkObservationRecord[],
): NetworkEvidenceFreshness | null {
  const latest = observations
    .filter((observation) => observation.kind === "tunnel_reachability")
    .reduce<NetworkObservationRecord | null>((current, observation) => {
      if (!current) {
        return observation;
      }
      return timestampMillis(observation.observed_at) >
        timestampMillis(current.observed_at)
        ? observation
        : current;
    }, null);
  if (!latest) {
    return null;
  }
  const latestMs = timestampMillis(latest.observed_at);
  const staleAfterMs = Math.max(1, latest.stale_after_secs ?? 180) * 1_000;
  const ageMs = Date.now() - latestMs;
  const stale = Number.isFinite(ageMs) && ageMs > staleAfterMs;
  const observedLabel = formatCompactTime(latest.observed_at);
  return {
    detail: stale
      ? `The newest reachability sample in this range is older than its ${Math.round(staleAfterMs / 1_000)} second validity window. It remains historical evidence but does not represent current link health.`
      : `Current reachability evidence is ${latest.source} and was observed ${observedLabel}.`,
    latestTimestamp: latest.observed_at,
    observedLabel,
    stale,
  };
}

function isEvidenceTimestampStale(
  timestamp: string | null | undefined,
): boolean {
  if (!timestamp) {
    return false;
  }
  const ms = timestampMillis(timestamp);
  return (
    Number.isFinite(ms) &&
    Date.now() - ms > DEFAULT_NETWORK_MEASUREMENT_FRESH_AFTER_MS
  );
}

function buildTrendRow(
  trend: NetworkObservationTrendRecord,
  clientLabel: (clientId: string) => string,
  throughputBaselines: Map<string, ThroughputBaseline>,
): TrendRow {
  const baseline = throughputBaselineFor(
    {
      interfaceName: trend.interface_name,
      planId: trend.plan_id,
      planName: trend.plan_name,
      topologyIdentityHash: trend.topology_identity_hash,
    },
    throughputBaselines,
  );
  const throughputHealth =
    trend.kind === "network_speed_test" &&
    trend.throughput_avg_mbps !== null &&
    baseline
      ? bandwidthEvidenceHealth({
          configuredBandwidthMbps: baseline.configuredBandwidthMbps,
          effectiveBandwidthMbps: baseline.effectiveBandwidthMbps,
          measuredThroughputMbps: trend.throughput_avg_mbps,
        })
      : null;
  const sampleStatus =
    trend.degraded_count > 0
      ? "degraded"
      : trend.healthy_count > 0
        ? "healthy"
        : "recorded";
  const signalStatus =
    throughputHealth?.signalStatus === "degraded" ? "degraded" : sampleStatus;
  const metric =
    trend.throughput_avg_mbps !== null
      ? `${formatMetric(trend.throughput_avg_mbps)} Mbps avg`
      : trend.latency_avg_ms !== null
        ? `${formatMetric(trend.latency_avg_ms)} ms avg`
        : `${trend.sample_count} samples`;
  const metricDetail =
    trend.throughput_max_mbps !== null
      ? `${formatMetric(trend.throughput_max_mbps)} Mbps max; ${formatBytes(trend.bytes_total)} total${throughputHealth ? `; ${throughputHealth.summary}` : ""}`
      : trend.latency_min_ms !== null && trend.latency_max_ms !== null
        ? `${formatMetric(trend.latency_min_ms)}-${formatMetric(trend.latency_max_ms)} ms; ${formatLoss(trend.packet_loss_avg_ratio)} loss`
        : `${trend.healthy_count} healthy / ${trend.degraded_count} degraded`;
  return {
    id: `${trend.kind}:${trend.plan_name ?? ""}:${trend.client_id}:${trend.peer_client_id ?? ""}`,
    kind: trend.kind,
    sampleCount: trend.sample_count,
    signalLabel:
      trend.kind === "network_speed_test"
        ? throughputSampleSignalLabel(
            sampleStatus,
            trend.latest_observed_at,
            throughputHealth,
          )
        : humanStatus(signalStatus),
    signalStatus,
    metric,
    metricDetail,
    target: trend.plan_name ?? trend.interface_name ?? "network",
    targetDetail: endpointLabel(
      trend.client_id,
      trend.peer_client_id,
      clientLabel,
    ),
    latestObservedAt: trend.latest_observed_at,
  };
}

function latestObservationRows(
  observations: NetworkObservationRecord[],
): NetworkObservationRecord[] {
  const latest = new Map<string, NetworkObservationRecord>();
  for (const observation of observations) {
    const key = [
      observation.plan_id ?? "unplanned",
      observation.topology_identity_hash ?? "identity",
      observation.kind,
      observation.endpoint_side ?? observation.client_id,
    ].join(":");
    const current = latest.get(key);
    if (
      !current ||
      timestampMillis(observation.observed_at) >
        timestampMillis(current.observed_at)
    ) {
      latest.set(key, observation);
    }
  }
  return Array.from(latest.values()).sort(
    (left, right) =>
      timestampMillis(right.observed_at) - timestampMillis(left.observed_at) ||
      (left.plan_name ?? "").localeCompare(right.plan_name ?? "") ||
      left.client_id.localeCompare(right.client_id),
  );
}

function buildObservationRow(
  observation: NetworkObservationRecord,
  clientLabel: (clientId: string) => string,
  throughputBaselines: Map<string, ThroughputBaseline>,
): ObservationRow {
  const signalStatus =
    observation.healthy === true
      ? "healthy"
      : observation.healthy === false
        ? "degraded"
        : "recorded";
  if (observation.kind === "tunnel_reachability") {
    const lossDetail =
      observation.packet_loss_ratio === null
        ? "loss unavailable"
        : `${formatMetric(observation.packet_loss_ratio * 100)}% loss`;
    return {
      id: observation.id,
      jobId: observation.job_id,
      source: observation.source,
      kind: observation.kind,
      signalLabel: humanStatus(signalStatus),
      signalStatus,
      metric:
        observation.latency_avg_ms === null
          ? "No latency"
          : `${formatMetric(observation.latency_avg_ms)} ms`,
      metricDetail: observation.reason
        ? `${lossDetail}; ${humanStatus(observation.reason)}`
        : lossDetail,
      target: observation.target ?? "peer tunnel",
      targetDetail: endpointLabel(
        observation.client_id,
        observation.peer_client_id,
        clientLabel,
      ),
      observedAt: observation.observed_at,
    };
  }
  if (observation.kind === "network_speed_test") {
    const baseline = throughputBaselineFor(
      {
        interfaceName: observation.interface_name,
        planId: observation.plan_id,
        planName: observation.plan_name,
        topologyIdentityHash: observation.topology_identity_hash,
      },
      throughputBaselines,
    );
    const throughputHealth =
      observation.throughput_mbps !== null && baseline
        ? bandwidthEvidenceHealth({
            configuredBandwidthMbps: baseline.configuredBandwidthMbps,
            effectiveBandwidthMbps: baseline.effectiveBandwidthMbps,
            measuredThroughputMbps: observation.throughput_mbps,
          })
        : null;
    const speedSignalStatus =
      throughputHealth?.signalStatus === "degraded" ? "degraded" : signalStatus;
    return {
      id: observation.id,
      jobId: observation.job_id,
      source: observation.source,
      kind: observation.kind,
      signalLabel: throughputSampleSignalLabel(
        signalStatus,
        observation.observed_at,
        throughputHealth,
      ),
      signalStatus: speedSignalStatus,
      metric:
        observation.throughput_mbps === null
          ? "No throughput"
          : `${formatMetric(observation.throughput_mbps)} Mbps`,
      metricDetail:
        observation.bytes === null
          ? throughputHealth
            ? `bytes unavailable; ${throughputHealth.summary}`
            : "bytes unavailable"
          : `${formatBytes(observation.bytes)}${throughputHealth ? `; ${throughputHealth.summary}` : ""}`,
      target: observation.target ?? "speed endpoint",
      targetDetail: `${observation.role ?? "role"} ${endpointLabel(observation.client_id, observation.peer_client_id, clientLabel)}`,
      observedAt: observation.observed_at,
    };
  }
  const metadata = asRecord(observation.metadata);
  const runtime = asRecord(metadata.runtime);
  const summary = asRecord(runtime.summary);
  const runtimeStatus = asString(summary.status);
  const applied = asBoolean(metadata.applied);
  const reasons = asStringArray(summary.reasons);
  const manager = asString(summary.manager);
  const runtimeDetail = runtimeSummaryDetail(
    reasons,
    `${manager ?? "runtime"}; ${observation.interface_name ?? "interface unavailable"}`,
  );
  return {
    id: observation.id,
    jobId: observation.job_id,
    source: observation.source,
    kind: observation.kind,
    signalLabel: humanStatus(signalStatus),
    signalStatus,
    metric:
      observation.healthy === true && applied
        ? "Managed blocks match"
        : runtimeStatus
          ? `Runtime ${humanStatus(runtimeStatus).toLowerCase()}`
          : observation.healthy === true
            ? "Runtime healthy"
            : "Recorded status",
    metricDetail: runtimeDetail,
    target: observation.plan_name ?? "tunnel plan",
    targetDetail: endpointLabel(
      observation.client_id,
      observation.peer_client_id,
      clientLabel,
    ),
    observedAt: observation.observed_at,
  };
}

function buildLatencyCurveGroups(
  observations: NetworkObservationRecord[],
  clientLabel: (clientId: string) => string,
): LatencyCurveGroup[] {
  const grouped = new Map<string, NetworkObservationRecord[]>();
  for (const observation of observations) {
    if (observation.kind !== "tunnel_reachability") {
      continue;
    }
    const key = [
      observation.plan_name ?? "unplanned",
      observation.interface_name ?? "interface",
      observation.client_id,
      observation.peer_client_id ?? "peer",
      observation.target ?? "target",
    ].join(":");
    grouped.set(key, [...(grouped.get(key) ?? []), observation]);
  }
  return Array.from(grouped.entries())
    .map(([key, rows]) => {
      const sorted = rows
        .slice()
        .sort((left, right) =>
          left.observed_at.localeCompare(right.observed_at),
        )
        .slice(-24);
      const points = sorted.map((row) => ({
        healthy: row.healthy,
        latencyAvgMs: row.latency_avg_ms,
        lossRatio: row.packet_loss_ratio ?? null,
        reason: row.reason,
      }));
      const latest = sorted[sorted.length - 1];
      return {
        key,
        label: latest.plan_name ?? latest.interface_name ?? "network probe",
        detail: endpointLabel(
          latest.client_id,
          latest.peer_client_id,
          clientLabel,
        ),
        maxLatency: Math.max(
          1,
          ...points.flatMap((point) =>
            typeof point.latencyAvgMs === "number" ? [point.latencyAvgMs] : [],
          ),
        ),
        points,
      };
    })
    .filter((group) => group.points.length > 1)
    .sort(
      (left, right) =>
        left.label.localeCompare(right.label) ||
        left.detail.localeCompare(right.detail),
    );
}

function buildEvidenceRow(
  job: JobHistoryRecord,
  outputs: JobOutputRecord[],
  clientLabel: (clientId: string) => string,
  outputsLoaded: boolean,
  throughputBaselines: Map<string, ThroughputBaseline>,
): EvidenceRow {
  const parsedStatus = parseStatusOutput(outputs);
  if (isProbeStatus(parsedStatus)) {
    const parsed = asRecord(parsedStatus.parsed);
    const latencyAvgMs = asNumber(parsed.latency_avg_ms);
    const lossRatio = asNumber(parsed.packet_loss_ratio);
    return {
      job,
      kind: "network_probe",
      signalKind: "observation",
      signalLabel: asBoolean(parsed.healthy) ? "Valid sample" : "Failed sample",
      signalStatus: asBoolean(parsed.healthy) ? "healthy" : "degraded",
      metric:
        latencyAvgMs === null
          ? "No latency"
          : `${formatMetric(latencyAvgMs)} ms`,
      metricDetail:
        lossRatio === null
          ? "loss unavailable"
          : `${formatMetric(lossRatio * 100)}% loss`,
      target: asString(parsedStatus.target) ?? "peer tunnel",
      targetDetail: endpointLabel(
        asString(parsedStatus.client_id),
        asString(parsedStatus.peer_client_id),
        clientLabel,
      ),
      latencyAvgMs: latencyAvgMs ?? undefined,
      lossRatio: lossRatio ?? undefined,
    };
  }
  if (isNetworkStatus(parsedStatus)) {
    const runtime = asRecord(parsedStatus.runtime);
    const iface = asRecord(runtime.interface);
    const summary = asRecord(runtime.summary);
    const runtimeStatus = asString(summary.status);
    const runtimeHealthy = asOptionalBoolean(summary.healthy);
    const reasons = asStringArray(summary.reasons);
    const interfaceState =
      asString(iface.operstate) ??
      (asBoolean(iface.exists) ? "present" : "absent");
    const applied = asBoolean(parsedStatus.applied);
    const statusHealthy = runtimeHealthy ?? applied;
    const runtimeDetail = runtimeSummaryDetail(
      reasons,
      `interface ${interfaceState}; adapter ${humanStatus(asString(summary.adapter_state) ?? "unknown")}`,
    );
    return {
      job,
      kind: "network_status",
      signalKind: "runtime",
      signalStatus: statusHealthy ? "healthy" : "drift",
      metric:
        applied && statusHealthy
          ? "Declared state matches"
          : runtimeStatus
            ? `Runtime ${humanStatus(runtimeStatus).toLowerCase()}`
            : "Needs review",
      metricDetail: runtimeDetail,
      target: asString(parsedStatus.interface) ?? "interface",
      targetDetail: endpointLabel(
        asString(parsedStatus.client_id),
        asString(parsedStatus.peer_client_id),
        clientLabel,
      ),
    };
  }
  const speedStatuses = parseStatusOutputs(outputs).filter(isSpeedTestStatus);
  if (speedStatuses.length > 0) {
    const clientStatus =
      speedStatuses.find((status) => asString(status.role) === "client") ??
      speedStatuses[0];
    const serverStatus = speedStatuses.find(
      (status) => asString(status.role) === "server",
    );
    const throughputMbps = asNumber(clientStatus.throughput_mbps);
    const bytes = asNumber(clientStatus.bytes);
    const allSucceeded =
      speedStatuses.length >= 2 &&
      speedStatuses.every((status) => asBoolean(status.success));
    const baseline = throughputBaselineFor(
      {
        interfaceName: asString(clientStatus.interface),
        planName: asString(clientStatus.plan),
      },
      throughputBaselines,
    );
    const throughputHealth =
      throughputMbps !== null && baseline
        ? bandwidthEvidenceHealth({
            configuredBandwidthMbps: baseline.configuredBandwidthMbps,
            effectiveBandwidthMbps: baseline.effectiveBandwidthMbps,
            measuredThroughputMbps: throughputMbps,
          })
        : null;
    const sampleStatus: TopologyObservationState = allSucceeded
      ? "healthy"
      : "degraded";
    const signalStatus =
      throughputHealth?.signalStatus === "degraded" ? "degraded" : sampleStatus;
    return {
      job,
      kind: "network_speed_test",
      signalKind: "observation",
      signalLabel: throughputSampleSignalLabel(
        sampleStatus,
        job.created_at,
        throughputHealth,
      ),
      signalStatus,
      metric:
        throughputMbps === null
          ? "No throughput"
          : `${formatMetric(throughputMbps)} Mbps`,
      metricDetail:
        bytes === null
          ? throughputHealth
            ? `bytes unavailable; ${throughputHealth.summary}`
            : "bytes unavailable"
          : `${formatBytes(bytes)} sent${throughputHealth ? `; ${throughputHealth.summary}` : ""}`,
      target: `${asString(clientStatus.server_address) ?? "server"}:${asNumber(clientStatus.port) ?? "port"}`,
      targetDetail: endpointLabel(
        asString(clientStatus.client_id),
        asString(serverStatus?.client_id) ??
          asString(clientStatus.peer_client_id),
        clientLabel,
        "server",
      ),
    };
  }
  return {
    job,
    kind: job.command_type,
    signalKind: "job",
    signalStatus: job.status,
    metric: !outputsLoaded
      ? "Output not loaded"
      : outputs.length === 0
        ? "No retained output"
        : `${outputs.length} chunks`,
    metricDetail: !outputsLoaded
      ? "Use Load output to fetch retained output"
      : outputs.length === 0
        ? "Retained output unavailable for this job"
        : "Retained job output",
    target: `${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
    targetDetail: shortId(job.payload_hash),
  };
}

function evidenceStatusBadgeClass(row: EvidenceRow): string {
  switch (row.signalKind) {
    case "job":
      return jobStatusBadgeClass(row.signalStatus as JobStatus);
    case "observation":
      return topologyObservationStateBadgeClass(
        row.signalStatus as TopologyObservationState,
      );
    case "runtime":
      return topologyRuntimeStateBadgeClass(
        row.signalStatus as TopologyRuntimeState,
      );
  }
}

function endpointLabel(
  clientId: string | null | undefined,
  peerClientId: string | null | undefined,
  clientLabel: (clientId: string) => string,
  peerFallback = "peer",
): string {
  const left = clientId ? clientLabel(clientId) : "Unknown VPS";
  const right = peerClientId ? clientLabel(peerClientId) : peerFallback;
  return `${left} -> ${right}`;
}

function parseStatusOutput(outputs: JobOutputRecord[]): unknown {
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    try {
      return JSON.parse(decodeOutputPreview(output.data_base64));
    } catch {
      continue;
    }
  }
  return null;
}

function parseStatusOutputs(
  outputs: JobOutputRecord[],
): Record<string, unknown>[] {
  const statuses: Record<string, unknown>[] = [];
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    try {
      statuses.push(
        asRecord(JSON.parse(decodeOutputPreview(output.data_base64))),
      );
    } catch {
      continue;
    }
  }
  return statuses;
}

function isProbeStatus(value: unknown): value is Record<string, unknown> {
  const type = asRecord(value).type;
  return type === "tunnel_reachability";
}

function isNetworkStatus(value: unknown): value is Record<string, unknown> {
  return asRecord(value).type === "network_status";
}

function isSpeedTestStatus(value: unknown): value is Record<string, unknown> {
  return asRecord(value).type === "network_speed_test";
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function asString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is string =>
          typeof item === "string" && item.trim().length > 0,
      )
    : [];
}

function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function asBoolean(value: unknown): boolean {
  return value === true;
}

function asOptionalBoolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function healthLabel(value: unknown): string {
  if (value === true) {
    return "healthy";
  }
  if (value === false) {
    return "degraded";
  }
  return "unknown";
}

function humanStatus(value: string): string {
  return readableTelemetryToken(value);
}

function runtimeSummaryDetail(reasons: string[], fallback: string): string {
  const parts = reasons.map(humanStatus);
  return parts.length > 0 ? parts.join(", ") : fallback;
}

function formatMetric(value: number): string {
  return Number.isInteger(value)
    ? String(value)
    : value.toFixed(value < 10 ? 2 : 1);
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024) {
    return `${formatMetric(value / 1024 / 1024)} MiB`;
  }
  if (value >= 1024) {
    return `${formatMetric(value / 1024)} KiB`;
  }
  return `${value} B`;
}

function formatLoss(value: number | null): string {
  return value === null ? "loss unavailable" : `${formatMetric(value * 100)}%`;
}

function latestObservedAt(rows: ObservationRow[]): string {
  return rows.reduce(
    (latest, row) => (row.observedAt > latest ? row.observedAt : latest),
    rows[0]?.observedAt ?? "",
  );
}
