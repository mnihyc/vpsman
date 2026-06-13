import { useMemo, useState } from "react";
import { Activity, RefreshCcw } from "lucide-react";
import {
  jobStatusBadgeClass,
  topologyObservationStateBadgeClass,
  topologyRuntimeStateBadgeClass,
} from "../../jobStatusPresentation";
import type {
  JobHistoryRecord,
  JobOutputRecord,
  JobStatus,
  NetworkObservationRecord,
  NetworkObservationTrendRecord,
  NetworkOspfRecommendationRecord,
  NetworkOspfUpdatePlanRecord,
  TopologyObservationState,
  TopologyRuntimeState,
} from "../../types";
import { decodeOutputPreview, formatTime, shortId } from "../../utils";

const networkCommands = new Set([
  "network_apply",
  "network_rollback",
  "network_status",
  "network_probe",
  "network_speed_test",
]);

export function TopologyEvidencePanel({
  clientLabel,
  jobs,
  observations,
  onLoadObservations,
  onLoadOspfRecommendations,
  onLoadOspfUpdatePlans,
  onLoadOutputs,
  onLoadTrends,
  ospfRecommendations,
  ospfUpdatePlans,
  trends,
}: {
  clientLabel: (clientId: string) => string;
  jobs: JobHistoryRecord[];
  observations: NetworkObservationRecord[];
  onLoadObservations: () => Promise<void>;
  onLoadOspfRecommendations: () => Promise<void>;
  onLoadOspfUpdatePlans: () => Promise<void>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTrends: () => Promise<void>;
  ospfRecommendations: NetworkOspfRecommendationRecord[];
  ospfUpdatePlans: NetworkOspfUpdatePlanRecord[];
  trends: NetworkObservationTrendRecord[];
}) {
  const networkJobs = useMemo(
    () => jobs.filter((job) => networkCommands.has(job.command_type)).slice(0, 8),
    [jobs],
  );
  const [outputsByJob, setOutputsByJob] = useState<Record<string, JobOutputRecord[]>>({});
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const rows = networkJobs.map((job) => buildEvidenceRow(job, outputsByJob[job.id] ?? [], clientLabel));
  const ospfUpdateRows = ospfUpdatePlans.slice(0, 6).map(buildOspfUpdatePlanRow);
  const ospfRows = ospfRecommendations.slice(0, 6).map(buildOspfRecommendationRow);
  const observationRows = observations.slice(0, 8).map((observation) => buildObservationRow(observation, clientLabel));
  const trendRows = trends.slice(0, 6).map((trend) => buildTrendRow(trend, clientLabel));
  const probePoints = rows
    .filter((row) => row.kind === "network_probe" && typeof row.latencyAvgMs === "number")
    .map((row) => ({
      jobId: row.job.id,
      latencyAvgMs: row.latencyAvgMs ?? 0,
      lossRatio: row.lossRatio ?? 0,
    }))
    .concat(
      observations
        .filter((observation) => observation.kind === "network_probe" && typeof observation.latency_avg_ms === "number")
        .map((observation) => ({
          jobId: observation.id,
          latencyAvgMs: observation.latency_avg_ms ?? 0,
          lossRatio: observation.packet_loss_ratio ?? 0,
        })),
    );
  const maxLatency = Math.max(1, ...probePoints.map((point) => point.latencyAvgMs));
  const latencyGroups = useMemo(() => buildLatencyCurveGroups(observations, clientLabel), [clientLabel, observations]);
  const status =
    error ??
    (loading
      ? "Loading network outputs"
      : ospfUpdatePlans.length > 0
        ? `${ospfUpdatePlans.length} OSPF update plans`
      : ospfRecommendations.length > 0
        ? `${ospfRecommendations.length} OSPF recommendations`
      : trends.length > 0
        ? `${observations.length} observations / ${trends.length} trends`
        : observations.length > 0
          ? `${observations.length} persisted observations`
          : networkJobs.length === 0
            ? "No network jobs"
            : `${networkJobs.length} recent network jobs`);

  async function refreshEvidence() {
    setLoading(true);
    setError(null);
    try {
      const outputEntries = await Promise.all(
        networkJobs.map(async (job) => [job.id, await onLoadOutputs(job.id)] as const),
      );
      await Promise.all([
        onLoadObservations(),
        onLoadTrends(),
        onLoadOspfRecommendations(),
        onLoadOspfUpdatePlans(),
      ]);
      setOutputsByJob(Object.fromEntries(outputEntries));
    } catch (loadError) {
      setOutputsByJob({});
      setError(loadError instanceof Error ? loadError.message : "Topology evidence unavailable");
    } finally {
      setLoading(false);
    }
  }

  return (
    <section className="fleetPanel topologyEvidence">
      <div className="sectionHeader">
        <div>
          <h2>Topology evidence</h2>
          <span>{status}</span>
        </div>
        <button className="secondaryAction" disabled={loading} onClick={refreshEvidence} type="button">
          <RefreshCcw size={17} />
          Refresh evidence
        </button>
      </div>
      {probePoints.length > 0 && latencyGroups.length === 0 && (
        <div className="latencyCurve" aria-label="Network probe latency history">
          {probePoints.map((point) => (
            <span
              className={point.lossRatio > 0 ? "warn" : "ok"}
              key={point.jobId}
              style={{ height: `${Math.max(8, Math.round((point.latencyAvgMs / maxLatency) * 44))}px` }}
              title={`${formatMetric(point.latencyAvgMs)} ms avg`}
            />
          ))}
        </div>
      )}
      {latencyGroups.length > 0 && (
        <div className="latencyCurveGroups" aria-label="Per tunnel latency curves">
          {latencyGroups.map((group) => (
            <div className="latencyCurveCard" key={group.key}>
              <span className="latencyCurveTitle">
                <strong>{group.label}</strong>
                <small>{group.detail}</small>
              </span>
              <div className="latencyCurve compact" aria-label={`${group.label} latency curve`}>
                {group.points.map((point, index) => (
                  <span
                    className={point.lossRatio > 0 ? "warn" : "ok"}
                    key={`${group.key}-${index}`}
                    style={{ height: `${Math.max(8, Math.round((point.latencyAvgMs / group.maxLatency) * 38))}px` }}
                    title={`${formatMetric(point.latencyAvgMs)} ms avg`}
                  />
                ))}
              </div>
            </div>
          ))}
        </div>
      )}
      {ospfUpdateRows.length > 0 && (
        <div className="table historyTable trendTable">
          <div className="historyRow heading topologyEvidenceGrid">
            <span>OSPF update plan</span>
            <span>Status</span>
            <span>Cost</span>
            <span>Approval</span>
            <span>Latest</span>
          </div>
          {ospfUpdateRows.map((row) => (
            <div className="historyRow topologyEvidenceGrid" key={row.id}>
              <span className="historyPrimary">
                <strong>{row.planName}</strong>
                <small>{row.interfaceName}</small>
              </span>
              <span className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}>{row.signalStatus}</span>
              <span className="topologyMetric">
                <strong>{row.metric}</strong>
                <small>{row.metricDetail}</small>
              </span>
              <span className="topologyMetric">
                <strong>{row.target}</strong>
                <small>{row.targetDetail}</small>
              </span>
              <span>{row.latestObservedAt === null ? "pending" : formatTime(row.latestObservedAt)}</span>
            </div>
          ))}
        </div>
      )}
      {ospfRows.length > 0 && (
        <div className="table historyTable trendTable">
          <div className="historyRow heading topologyEvidenceGrid">
            <span>OSPF plan</span>
            <span>Confidence</span>
            <span>Cost</span>
            <span>Evidence</span>
            <span>Latest</span>
          </div>
          {ospfRows.map((row) => (
            <div className="historyRow topologyEvidenceGrid" key={row.id}>
              <span className="historyPrimary">
                <strong>{row.planName}</strong>
                <small>{row.interfaceName}</small>
              </span>
              <span className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}>{row.signalStatus}</span>
              <span className="topologyMetric">
                <strong>{row.metric}</strong>
                <small>{row.metricDetail}</small>
              </span>
              <span className="topologyMetric">
                <strong>{row.target}</strong>
                <small>{row.targetDetail}</small>
              </span>
              <span>{row.latestObservedAt === null ? "pending" : formatTime(row.latestObservedAt)}</span>
            </div>
          ))}
        </div>
      )}
      {trendRows.length > 0 && (
        <div className="table historyTable trendTable">
          <div className="historyRow heading topologyEvidenceGrid">
            <span>Trend</span>
            <span>Health</span>
            <span>Metric</span>
            <span>Endpoint</span>
            <span>Latest</span>
          </div>
          {trendRows.map((row) => (
            <div className="historyRow topologyEvidenceGrid" key={row.id}>
              <span className="historyPrimary">
                <strong>{row.kind}</strong>
                <small>{row.sampleCount} samples</small>
              </span>
              <span className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}>{row.signalStatus}</span>
              <span className="topologyMetric">
                <strong>{row.metric}</strong>
                <small>{row.metricDetail}</small>
              </span>
              <span className="topologyMetric">
                <strong>{row.target}</strong>
                <small>{row.targetDetail}</small>
              </span>
              <span>{formatTime(row.latestObservedAt)}</span>
            </div>
          ))}
        </div>
      )}
      {observationRows.length > 0 && (
        <div className="table historyTable observationTable">
          <div className="historyRow heading topologyEvidenceGrid">
            <span>Observation</span>
            <span>Signal</span>
            <span>Metric</span>
            <span>Target</span>
            <span>Observed</span>
          </div>
          {observationRows.map((row) => (
            <div className="historyRow topologyEvidenceGrid" key={row.id}>
              <span className="historyPrimary">
                <strong>{row.kind}</strong>
                <small>{shortId(row.jobId)}</small>
              </span>
              <span className={`status ${topologyObservationStateBadgeClass(row.signalStatus)}`}>{row.signalStatus}</span>
              <span className="topologyMetric">
                <strong>{row.metric}</strong>
                <small>{row.metricDetail}</small>
              </span>
              <span className="topologyMetric">
                <strong>{row.target}</strong>
                <small>{row.targetDetail}</small>
              </span>
              <span>{formatTime(row.observedAt)}</span>
            </div>
          ))}
        </div>
      )}
      <div className="table historyTable">
        <div className="historyRow heading topologyEvidenceGrid">
          <span>Command</span>
          <span>Signal</span>
          <span>Metric</span>
          <span>Target</span>
          <span>Created</span>
        </div>
        {rows.map((row) => (
          <div className="historyRow topologyEvidenceGrid" key={row.job.id}>
            <span className="historyPrimary">
              <strong>{row.job.command_type}</strong>
              <small>{shortId(row.job.id)}</small>
            </span>
            <span className={`status ${evidenceStatusBadgeClass(row)}`}>{row.signalStatus}</span>
            <span className="topologyMetric">
              <strong>{row.metric}</strong>
              <small>{row.metricDetail}</small>
            </span>
            <span className="topologyMetric">
              <strong>{row.target}</strong>
              <small>{row.targetDetail}</small>
            </span>
            <span>{formatTime(row.job.created_at)}</span>
          </div>
        ))}
        {rows.length === 0 && (
          <div className="emptyState">
            <Activity size={22} />
            <strong>No topology evidence</strong>
            <span>Network apply, rollback, status, and probe results will appear here.</span>
          </div>
        )}
      </div>
    </section>
  );
}

type EvidenceRow = {
  job: JobHistoryRecord;
  kind: string;
  signalKind: "job" | "observation" | "runtime";
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
    latencyAvgMs: number;
    lossRatio: number;
  }[];
};

type ObservationRow = {
  id: string;
  jobId: string;
  kind: string;
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
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latestObservedAt: string;
};

type OspfRecommendationRow = {
  id: string;
  planName: string;
  interfaceName: string;
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latestObservedAt: string | null;
};

type OspfUpdatePlanRow = {
  id: string;
  planName: string;
  interfaceName: string;
  signalStatus: TopologyObservationState;
  metric: string;
  metricDetail: string;
  target: string;
  targetDetail: string;
  latestObservedAt: string | null;
};

function buildOspfUpdatePlanRow(plan: NetworkOspfUpdatePlanRecord): OspfUpdatePlanRow {
  const signalStatus =
    plan.status === "noop"
      ? "healthy"
      : plan.status === "review_degraded"
        ? "degraded"
        : plan.status === "needs_observation"
          ? "unknown"
          : "recorded";
  const delta = plan.cost_delta === 0 ? "unchanged" : plan.cost_delta > 0 ? `+${plan.cost_delta}` : String(plan.cost_delta);
  const privilegeState = plan.privilege_required ? "privilege-unlocked" : "read-only";
  return {
    id: plan.plan_id,
    planName: plan.plan_name,
    interfaceName: plan.interface_name,
    signalStatus,
    metric: `${plan.current_ospf_cost} -> ${plan.recommended_ospf_cost}`,
    metricDetail: `${delta}; ${plan.confidence}`,
    target: plan.requires_approval ? "approval required" : "no action",
    targetDetail: `${privilegeState}; ${plan.approval_scope.join(", ")}`,
    latestObservedAt: plan.evidence.latest_observed_at,
  };
}

function buildOspfRecommendationRow(recommendation: NetworkOspfRecommendationRecord): OspfRecommendationRow {
  const signalStatus =
    recommendation.confidence === "measured"
      ? recommendation.degraded_count > 0
        ? "degraded"
        : "healthy"
      : recommendation.confidence === "no_recent_observations"
        ? "unknown"
        : "recorded";
  const delta = recommendation.cost_delta === 0
    ? "unchanged"
    : recommendation.cost_delta > 0
      ? `+${recommendation.cost_delta}`
      : String(recommendation.cost_delta);
  const evidence = recommendation.latency_avg_ms !== null
    ? `${formatMetric(recommendation.latency_avg_ms)} ms; ${formatLoss(recommendation.packet_loss_avg_ratio)} loss`
    : recommendation.reason;
  const throughput = recommendation.throughput_avg_mbps === null
    ? `burst ${recommendation.effective_bandwidth}`
    : `${formatMetric(recommendation.throughput_avg_mbps)} Mbps avg; burst ${recommendation.effective_bandwidth}`;
  return {
    id: recommendation.plan_id,
    planName: recommendation.plan_name,
    interfaceName: recommendation.interface_name,
    signalStatus,
    metric: `${recommendation.plan_ospf_cost} -> ${recommendation.recommended_ospf_cost}`,
    metricDetail: `${delta}; ${recommendation.configured_bandwidth} configured`,
    target: evidence,
    targetDetail: `${throughput}; ${recommendation.sample_count} samples`,
    latestObservedAt: recommendation.latest_observed_at,
  };
}

function buildTrendRow(trend: NetworkObservationTrendRecord, clientLabel: (clientId: string) => string): TrendRow {
  const signalStatus =
    trend.degraded_count > 0 ? "degraded" : trend.healthy_count > 0 ? "healthy" : "recorded";
  const metric = trend.throughput_avg_mbps !== null
    ? `${formatMetric(trend.throughput_avg_mbps)} Mbps avg`
    : trend.latency_avg_ms !== null
      ? `${formatMetric(trend.latency_avg_ms)} ms avg`
      : `${trend.sample_count} samples`;
  const metricDetail = trend.throughput_max_mbps !== null
    ? `${formatMetric(trend.throughput_max_mbps)} Mbps max; ${formatBytes(trend.bytes_total)} total`
    : trend.latency_min_ms !== null && trend.latency_max_ms !== null
      ? `${formatMetric(trend.latency_min_ms)}-${formatMetric(trend.latency_max_ms)} ms; ${formatLoss(trend.packet_loss_avg_ratio)} loss`
      : `${trend.healthy_count} healthy / ${trend.degraded_count} degraded`;
  return {
    id: `${trend.kind}:${trend.plan_name ?? ""}:${trend.client_id}:${trend.peer_client_id ?? ""}`,
    kind: trend.kind,
    sampleCount: trend.sample_count,
    signalStatus,
    metric,
    metricDetail,
    target: trend.plan_name ?? trend.interface_name ?? "network",
    targetDetail: endpointLabel(trend.client_id, trend.peer_client_id, clientLabel),
    latestObservedAt: trend.latest_observed_at,
  };
}

function buildObservationRow(observation: NetworkObservationRecord, clientLabel: (clientId: string) => string): ObservationRow {
  const signalStatus =
    observation.healthy === true ? "healthy" : observation.healthy === false ? "degraded" : "recorded";
  if (observation.kind === "network_probe") {
    return {
      id: observation.id,
      jobId: observation.job_id,
      kind: observation.kind,
      signalStatus,
      metric:
        observation.latency_avg_ms === null ? "No latency" : `${formatMetric(observation.latency_avg_ms)} ms`,
      metricDetail:
        observation.packet_loss_ratio === null
          ? "loss unavailable"
          : `${formatMetric(observation.packet_loss_ratio * 100)}% loss`,
      target: observation.target ?? "peer tunnel",
      targetDetail: endpointLabel(observation.client_id, observation.peer_client_id, clientLabel),
      observedAt: observation.observed_at,
    };
  }
  if (observation.kind === "network_speed_test") {
    return {
      id: observation.id,
      jobId: observation.job_id,
      kind: observation.kind,
      signalStatus,
      metric:
        observation.throughput_mbps === null ? "No throughput" : `${formatMetric(observation.throughput_mbps)} Mbps`,
      metricDetail: observation.bytes === null ? "bytes unavailable" : `${formatBytes(observation.bytes)}`,
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
    asNumber(summary.external_import_candidate_count),
    `${manager ?? "runtime"}; ${observation.interface_name ?? "interface unavailable"}`,
  );
  return {
    id: observation.id,
    jobId: observation.job_id,
    kind: observation.kind,
    signalStatus,
    metric:
      observation.healthy === true && applied
        ? "Managed blocks match"
        : runtimeStatus
          ? `Runtime ${humanStatus(runtimeStatus)}`
          : observation.healthy === true
            ? "Runtime healthy"
            : "Recorded status",
    metricDetail: runtimeDetail,
    target: observation.plan_name ?? "tunnel plan",
    targetDetail: endpointLabel(observation.client_id, observation.peer_client_id, clientLabel),
    observedAt: observation.observed_at,
  };
}

function buildLatencyCurveGroups(
  observations: NetworkObservationRecord[],
  clientLabel: (clientId: string) => string,
): LatencyCurveGroup[] {
  const grouped = new Map<string, NetworkObservationRecord[]>();
  for (const observation of observations) {
    if (observation.kind !== "network_probe" || typeof observation.latency_avg_ms !== "number") {
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
        .sort((left, right) => left.observed_at.localeCompare(right.observed_at))
        .slice(-24);
      const points = sorted.map((row) => ({
        latencyAvgMs: row.latency_avg_ms ?? 0,
        lossRatio: row.packet_loss_ratio ?? 0,
      }));
      const latest = sorted[sorted.length - 1];
      return {
        key,
        label: latest.plan_name ?? latest.interface_name ?? "network probe",
        detail: endpointLabel(latest.client_id, latest.peer_client_id, clientLabel),
        maxLatency: Math.max(1, ...points.map((point) => point.latencyAvgMs)),
        points,
      };
    })
    .sort((left, right) => right.points.length - left.points.length || left.label.localeCompare(right.label))
    .slice(0, 8);
}

function buildEvidenceRow(
  job: JobHistoryRecord,
  outputs: JobOutputRecord[],
  clientLabel: (clientId: string) => string,
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
      signalStatus: asBoolean(parsed.healthy) ? "healthy" : "degraded",
      metric: latencyAvgMs === null ? "No latency" : `${formatMetric(latencyAvgMs)} ms`,
      metricDetail: lossRatio === null ? "loss unavailable" : `${formatMetric(lossRatio * 100)}% loss`,
      target: asString(parsedStatus.target) ?? "peer tunnel",
      targetDetail: endpointLabel(asString(parsedStatus.client_id), asString(parsedStatus.peer_client_id), clientLabel),
      latencyAvgMs: latencyAvgMs ?? undefined,
      lossRatio: lossRatio ?? undefined,
    };
  }
  if (isNetworkStatus(parsedStatus)) {
    const runtime = asRecord(parsedStatus.runtime);
    const iface = asRecord(runtime.interface);
    const bird2 = asRecord(runtime.bird2);
    const summary = asRecord(runtime.summary);
    const runtimeStatus = asString(summary.status);
    const runtimeHealthy = asOptionalBoolean(summary.healthy);
    const reasons = asStringArray(summary.reasons);
    const interfaceState = asString(iface.operstate) ?? (asBoolean(iface.exists) ? "present" : "absent");
    const applied = asBoolean(parsedStatus.applied);
    const statusHealthy = runtimeHealthy ?? applied;
    const runtimeDetail = runtimeSummaryDetail(
      reasons,
      asNumber(summary.external_import_candidate_count),
      `interface ${interfaceState}; bird2 ${healthLabel(bird2.healthy)}`,
    );
    return {
      job,
      kind: "network_status",
      signalKind: "runtime",
      signalStatus: statusHealthy ? "healthy" : "drift",
      metric:
        applied && statusHealthy
          ? "Managed blocks match"
          : runtimeStatus
            ? `Runtime ${humanStatus(runtimeStatus)}`
            : "Needs review",
      metricDetail: runtimeDetail,
      target: asString(parsedStatus.interface) ?? "interface",
      targetDetail: endpointLabel(asString(parsedStatus.client_id), asString(parsedStatus.peer_client_id), clientLabel),
    };
  }
  const speedStatuses = parseStatusOutputs(outputs).filter(isSpeedTestStatus);
  if (speedStatuses.length > 0) {
    const clientStatus = speedStatuses.find((status) => asString(status.role) === "client") ?? speedStatuses[0];
    const serverStatus = speedStatuses.find((status) => asString(status.role) === "server");
    const throughputMbps = asNumber(clientStatus.throughput_mbps);
    const bytes = asNumber(clientStatus.bytes);
    const allSucceeded = speedStatuses.length >= 2 && speedStatuses.every((status) => asBoolean(status.success));
    return {
      job,
      kind: "network_speed_test",
      signalKind: "observation",
      signalStatus: allSucceeded ? "healthy" : "degraded",
      metric: throughputMbps === null ? "No throughput" : `${formatMetric(throughputMbps)} Mbps`,
      metricDetail: bytes === null ? "bytes unavailable" : `${formatBytes(bytes)} sent`,
      target: `${asString(clientStatus.server_address) ?? "server"}:${asNumber(clientStatus.port) ?? "port"}`,
      targetDetail: endpointLabel(
        asString(clientStatus.client_id),
        asString(serverStatus?.client_id) ?? asString(clientStatus.peer_client_id),
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
    metric: outputs.length === 0 ? "Output not loaded" : `${outputs.length} chunks`,
    metricDetail: outputs.length === 0 ? "Refresh evidence to load retained output" : "Retained job output",
    target: `${job.target_count} target${job.target_count === 1 ? "" : "s"}`,
    targetDetail: shortId(job.payload_hash),
  };
}

function evidenceStatusBadgeClass(row: EvidenceRow): string {
  switch (row.signalKind) {
    case "job":
      return jobStatusBadgeClass(row.signalStatus as JobStatus);
    case "observation":
      return topologyObservationStateBadgeClass(row.signalStatus as TopologyObservationState);
    case "runtime":
      return topologyRuntimeStateBadgeClass(row.signalStatus as TopologyRuntimeState);
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

function parseStatusOutputs(outputs: JobOutputRecord[]): Record<string, unknown>[] {
  const statuses: Record<string, unknown>[] = [];
  for (const output of outputs) {
    if (output.stream !== "status") {
      continue;
    }
    try {
      statuses.push(asRecord(JSON.parse(decodeOutputPreview(output.data_base64))));
    } catch {
      continue;
    }
  }
  return statuses;
}

function isProbeStatus(value: unknown): value is Record<string, unknown> {
  return asRecord(value).type === "network_probe";
}

function isNetworkStatus(value: unknown): value is Record<string, unknown> {
  return asRecord(value).type === "network_status";
}

function isSpeedTestStatus(value: unknown): value is Record<string, unknown> {
  return asRecord(value).type === "network_speed_test";
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
}

function asString(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string" && item.trim().length > 0)
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
  return value.replace(/_/g, " ");
}

function runtimeSummaryDetail(reasons: string[], importCandidateCount: number | null, fallback: string): string {
  const parts = reasons.map(humanStatus);
  if (importCandidateCount !== null && importCandidateCount > 0) {
    parts.push(`${importCandidateCount} import candidate${importCandidateCount === 1 ? "" : "s"}`);
  }
  return parts.length > 0 ? parts.join(", ") : fallback;
}

function formatMetric(value: number): string {
  return Number.isInteger(value) ? String(value) : value.toFixed(value < 10 ? 2 : 1);
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
