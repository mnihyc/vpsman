import { useMemo, useState } from "react";
import { Activity, Search, ShieldCheck } from "lucide-react";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../../bulkJobProgress";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import {
  TimeSeriesChart,
  type TimeSeriesChartLine,
} from "../../components/TimeSeriesChart";
import { consolePalette } from "../../colorPalette";
import {
  useReviewGenerationGuard,
  waitForReviewRender,
} from "../../hooks/useReviewGenerationGuard";
import { usePanelDisplaySettings } from "../../panelDisplay";
import {
  buildPrivilegeForJobOperation,
  type PrivilegeAssertion,
  type PrivilegeMaterial,
} from "../../privilege";
import { selectorExpressionForClientIds } from "../../searchExpression";
import {
  buildNetworkProbeOperation,
  buildNetworkSpeedTestOperation,
  buildNetworkStatusOperation,
  renderTunnelEndpointConfig,
} from "../../topologyNetworkJobs";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  JobOperation,
  JobTargetRecord,
  NetworkObservationTrendRecord,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  runPanelAction,
  shortId,
} from "../../utils";
import {
  clampJobMaxTimeoutSecs,
  clampInteger,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

export function TopologyNetworkTestControls({
  agents,
  networkTrends,
  onCreateJob,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  privilegeMaterial,
  setPrivilegeMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
  networkTrends: NetworkObservationTrendRecord[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const [selectedPlanId, setSelectedPlanId] = useState(
    () => tunnelPlans[0]?.id ?? "",
  );
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(60);
  const [probeCount, setProbeCount] = useState(3);
  const [probeIntervalMs, setProbeIntervalMs] = useState(500);
  const [speedDurationSecs, setSpeedDurationSecs] = useState(3);
  const [speedMaxBytesMiB, setSpeedMaxBytesMiB] = useState(16);
  const [speedRateLimitKbps, setSpeedRateLimitKbps] = useState(100_000);
  const [speedPort, setSpeedPort] = useState(5201);
  const [speedConnectTimeoutMs, setSpeedConnectTimeoutMs] = useState(5000);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [lastAction, setLastAction] = useState<NetworkAction>("status");
  const [networkSnapshot, setNetworkSnapshot] =
    useState<NetworkActionSnapshot | null>(null);
  const [jobProgress, setJobProgress] = useState<BulkJobProgress | null>(null);
  const [lastJobProgress, setLastJobProgress] =
    useState<BulkJobProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const selectedPlan =
    tunnelPlans.find((plan) => plan.id === selectedPlanId) ??
    tunnelPlans[0] ??
    null;
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);
  const endpoint = useMemo(
    () =>
      selectedPlan ? renderTunnelEndpointConfig(selectedPlan.plan, side) : null,
    [selectedPlan, side],
  );
  const planTargets = resolveAgentsById(
    agents,
    selectedPlan ? planClientIds(selectedPlan) : [],
  );
  const visibleJobProgress = jobProgress ?? lastJobProgress;
  const selectedPlanTrends = useMemo(
    () =>
      selectedPlan
        ? networkTrends.filter(
            (trend) =>
              trend.plan_id === selectedPlan.id ||
              trend.plan_name === selectedPlan.name,
          )
        : [],
    [networkTrends, selectedPlan],
  );
  const recentProbeTrend = useMemo(
    () =>
      latestTrend(
        selectedPlanTrends.filter((trend) => trend.kind === "network_probe"),
      ),
    [selectedPlanTrends],
  );
  const recentSpeedTrend = useMemo(
    () =>
      latestTrend(
        selectedPlanTrends.filter(
          (trend) => trend.kind === "network_speed_test",
        ),
      ),
    [selectedPlanTrends],
  );
  const evidenceSummary = formatRecentEvidence(
    recentProbeTrend,
    recentSpeedTrend,
  );
  const speedSafetySummary = formatSpeedSafety(
    speedDurationSecs,
    speedMaxBytesMiB,
    speedRateLimitKbps,
    speedPort,
    speedConnectTimeoutMs,
  );
  const baselineSummary = selectedPlan
    ? formatPlanBaseline(selectedPlan)
    : "Select a tunnel plan for baseline";
  const lastRunSummary = visibleJobProgress
    ? `${actionLabel(lastAction)} ${shortId(visibleJobProgress.jobId)} in progress`
    : lastJob
      ? `${actionLabel(lastAction)} ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} targets`
      : "No local network test run in this view";
  const status =
    actionError ??
    (reviewPending
      ? `Preparing ${actionLabel(lastAction).toLowerCase()} review`
      : visibleJobProgress
        ? `${actionLabel(lastAction)} result for job ${shortId(visibleJobProgress.jobId)}`
        : lastJob
          ? `${actionLabel(lastAction)} job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} targets`
          : selectedPlan && !selectedPlan.enabled
            ? "Plan disabled; inspect only"
            : privilegeMaterial
              ? "Ready"
              : "Inspect available; unlock for probe/speed");

  function submitStatus() {
    void runImmediateNetworkAction("status");
  }

  function submitProbe() {
    void runImmediateNetworkAction("probe");
  }

  function submitSpeedTest() {
    void openNetworkPrompt("speed_test");
  }

  function clearNetworkReview() {
    invalidateReviewGeneration();
    setNetworkSnapshot(null);
  }

  async function openNetworkPrompt(mode: NetworkAction) {
    setActionError(null);
    setLastAction(mode);
    const reviewGeneration = captureReviewGeneration();
    setReviewPending(true);
    try {
      await waitForReviewRender();
      await runPanelAction(setPending, setActionError, async () => {
        const snapshot = await buildNetworkActionSnapshot(
          mode,
          reviewGeneration,
        );
        if (snapshot) {
          setNetworkSnapshot(snapshot);
        }
      });
    } finally {
      setReviewPending(false);
    }
  }

  async function runImmediateNetworkAction(mode: NetworkAction) {
    setActionError(null);
    setLastAction(mode);
    await runPanelAction(setPending, setActionError, async () => {
      const snapshot = await buildNetworkActionSnapshot(mode);
      if (snapshot) {
        await executeNetworkSnapshot(snapshot);
      }
    });
  }

  async function buildNetworkActionSnapshot(
    mode: NetworkAction,
    reviewGeneration?: number,
  ): Promise<NetworkActionSnapshot | null> {
    const needsPrivilege = actionNeedsPrivilege(mode);
    if (needsPrivilege && !privilegeMaterial) {
      throw new Error("Privilege unlock is locked");
    }
    if (!selectedPlan || !endpoint) {
      throw new Error("Select a tunnel plan");
    }
    if (!selectedPlan.enabled && mode !== "status") {
      throw new Error("Tunnel plan is disabled");
    }
    const boundedProbeCount = clampInteger(probeCount, 1, 20);
    const boundedProbeIntervalMs = clampInteger(probeIntervalMs, 200, 10_000);
    const boundedSpeedDurationSecs = clampInteger(speedDurationSecs, 1, 30);
    const boundedSpeedMaxBytes =
      clampInteger(speedMaxBytesMiB, 1, 256) * 1024 * 1024;
    const boundedSpeedRateLimitKbps = clampInteger(
      speedRateLimitKbps,
      64,
      1_000_000,
    );
    const boundedSpeedPort = clampInteger(speedPort, 1024, 65_535);
    const boundedSpeedConnectTimeoutMs = clampInteger(
      speedConnectTimeoutMs,
      100,
      30_000,
    );
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
    const buildSubmission = async (
      planRecord: TunnelPlanRecord,
      planSide: TunnelEndpointSide,
    ): Promise<NetworkJobSubmission> => {
      const builtOperation =
        mode === "status"
          ? buildNetworkStatusOperation(planRecord.plan, planSide)
          : mode === "probe"
            ? buildNetworkProbeOperation(
                planRecord.plan,
                planSide,
                boundedProbeCount,
                boundedProbeIntervalMs,
              )
            : buildNetworkSpeedTestOperation(
                planRecord.plan,
                planSide,
                boundedSpeedDurationSecs,
                boundedSpeedMaxBytes,
                boundedSpeedRateLimitKbps,
                boundedSpeedPort,
                boundedSpeedConnectTimeoutMs,
              );
      const targetClientIds =
        mode === "speed_test"
          ? [
              builtOperation.endpoint.localClientId,
              builtOperation.endpoint.peerClientId,
            ]
          : [builtOperation.endpoint.localClientId];
      const selectorExpression = selectorExpressionForClientIds(targetClientIds);
      const builtPrivilege = needsPrivilege
        ? await buildPrivilegeForJobOperation({
            clientIds: targetClientIds,
            commandType: commandName(mode),
            forceUnprivileged: false,
            operation: builtOperation.operation,
            privilegeMaterial: privilegeMaterial!,
            selectorExpression,
            maxTimeoutSecs: boundedMaxTimeoutSecs,
          })
        : null;
      return {
        command: commandName(mode),
        confirmed: requiresConfirmation(mode),
        destructive: false,
        forceUnprivileged: !needsPrivilege,
        jobId: crypto.randomUUID(),
        maxTimeoutSecs: boundedMaxTimeoutSecs,
        operation: builtOperation.operation,
        payloadHashHex: builtPrivilege?.payloadHashHex ?? null,
        planName: planRecord.name,
        privilegeAssertion: builtPrivilege?.privilegeAssertion ?? null,
        privileged: needsPrivilege,
        selectorExpression,
        side: planSide,
        targetClientIds,
        targets: resolveAgentsById(agents, targetClientIds),
      };
    };
    const submissions = await Promise.all(
      [{ plan: selectedPlan, side }].map((candidate) =>
        buildSubmission(candidate.plan, candidate.side),
      ),
    );
    if (!submissions.length) {
      throw new Error("No tunnel plan is ready for testing");
    }
    if (
      reviewGeneration !== undefined &&
      !isReviewGenerationCurrent(reviewGeneration)
    ) {
      return null;
    }
    const snapshotTargets = resolveAgentsById(
      agents,
      uniqueClientIds(
        submissions.flatMap((submission) => submission.targetClientIds),
      ),
    );
    const scopeLabel =
      mode === "speed_test" ? "Selected plan endpoints" : "Selected endpoint";
    const planLabel = submissions[0]?.planName ?? "unknown";
    return {
      action: mode,
      detail: `${actionLabel(mode)} ${submissions[0]?.planName ?? "selected plan"} on ${vpsCountLabel(snapshotTargets.length)}.`,
      items: [
        { label: "Operation", value: actionLabel(mode) },
        { label: "Scope", value: scopeLabel },
        {
          label: "Targets",
          value: formatTargetAvailabilitySummary(snapshotTargets),
        },
        { label: "Plans", value: planLabel },
        {
          label: "Endpoint",
          value: submissions.length > 1 ? "Both endpoints" : side,
        },
        { label: "Baseline", value: formatPlanBaseline(selectedPlan) },
        { label: "Recent evidence", value: evidenceSummary },
        ...(mode === "probe"
          ? [
              {
                label: "Probe cadence",
                value: `${boundedProbeCount} packets, ${boundedProbeIntervalMs} ms interval`,
              },
            ]
          : []),
        ...(mode === "speed_test"
          ? [
              {
                label: "Safety cap",
                value: formatSpeedSafety(
                  boundedSpeedDurationSecs,
                  boundedSpeedMaxBytes / (1024 * 1024),
                  boundedSpeedRateLimitKbps,
                  boundedSpeedPort,
                  boundedSpeedConnectTimeoutMs,
                ),
              },
            ]
          : []),
        { label: "Max timeout", value: `${boundedMaxTimeoutSecs}s` },
        {
          label: "Required privilege",
          value: needsPrivilege
            ? `${commandName(mode)} unlocked locally`
            : "No local privilege required",
        },
      ],
      submissions,
      targets: snapshotTargets,
    };
  }

  function clearExecutionResults() {
    setJobProgress(null);
    setLastJobProgress(null);
    setLastJob(null);
  }

  async function submitNetworkChange(snapshot: NetworkActionSnapshot) {
    await runPanelAction(setPending, setActionError, async () => {
      await executeNetworkSnapshot(snapshot);
    });
  }

  async function executeNetworkSnapshot(snapshot: NetworkActionSnapshot) {
    setNetworkSnapshot(null);
    clearExecutionResults();
    const jobs: Array<{
      job: CreateJobResponse;
      submission: NetworkJobSubmission;
    }> = [];
    for (const submission of snapshot.submissions) {
      const job = await onCreateJob({
        argv: [],
        selector_expression: submission.selectorExpression,
        target_client_ids: submission.targetClientIds,
        command: submission.command,
        confirmed: submission.confirmed,
        destructive: submission.destructive,
        operation: submission.operation,
        force_unprivileged: submission.forceUnprivileged,
        job_id: submission.jobId,
        privileged: submission.privileged,
        privilege_assertion: submission.privilegeAssertion,
        max_timeout_secs: submission.maxTimeoutSecs,
      });
      jobs.push({ job, submission });
    }
    const lastSubmission =
      snapshot.submissions[snapshot.submissions.length - 1] ?? null;
    setLastPayloadHash(lastSubmission?.payloadHashHex ?? null);
    setLastAction(snapshot.action);
    for (const { job, submission } of jobs) {
      setLastJob(job);
      await trackNetworkProgress(
        job,
        submission.targets,
        submission.maxTimeoutSecs,
      );
    }
  }

  async function trackNetworkProgress(
    job: CreateJobResponse,
    targets: AgentView[],
    maxTimeoutSecsForSnapshot: number,
  ) {
    const targetCount = createJobTargetCount(job);
    setLastJobProgress(null);
    setJobProgress(
      buildBulkJobProgress({
        jobId: job.job_id,
        targetCount,
        targetRecords: [],
        targets,
        maxTimeoutSecs: maxTimeoutSecsForSnapshot,
      }),
    );
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        onProgress: setJobProgress,
        targetCount,
        targets,
        maxTimeoutSecs: maxTimeoutSecsForSnapshot,
      });
      setLastJobProgress(result.progress);
    } finally {
      setJobProgress(null);
    }
  }

  return (
    <section className="fleetPanel commandComposer">
      <div className="sectionHeader">
        <div>
          <h2>Network tests</h2>
          <span>{status}</span>
        </div>
        <ShieldCheck size={20} />
      </div>
      <form
        className="dispatchForm topologyNetworkTestForm"
        onSubmit={(event) => event.preventDefault()}
      >
        <div
          className="topologyNetworkReviewStrip"
          aria-label="Network test review contract"
        >
          <div className={privilegeMaterial ? "ready" : "attention"}>
            <span>Required privilege</span>
            <strong>
              {privilegeMaterial
                ? "Probe/speed unlocked"
                : "Inspect available"}
            </strong>
            <p>
              {privilegeMaterial
                ? "Probe and speed-test jobs will bind a local assertion to the submitted payload."
                : "Status inspection is read-only; unlock only before probe or speed-test jobs."}
            </p>
          </div>
          <div>
            <span>Expected baseline</span>
            <strong>{baselineSummary}</strong>
            <p>
              Configured plan values used to judge latency, loss, and bandwidth
              evidence.
            </p>
          </div>
          <div className="attention">
            <span>Speed safety cap</span>
            <strong>{speedSafetySummary}</strong>
            <p>
              Speed tests require explicit duration, byte, rate, port, and
              timeout caps.
            </p>
          </div>
          <div>
            <span>Recent evidence</span>
            <strong>{evidenceSummary}</strong>
            <p>From persisted topology observations for the selected plan.</p>
          </div>
          <div>
            <span>Last local run</span>
            <strong>{lastRunSummary}</strong>
            <p>
              Execution result stays on this screen and links back to Job
              history.
            </p>
          </div>
        </div>
        <NetworkTestTrendCharts
          expectedBandwidthMbps={selectedPlan?.plan.bandwidth_mbps ?? null}
          trends={selectedPlanTrends}
        />
        <div className="topologyNetworkTestGroups">
          <section
            className="topologyNetworkTestGroup"
            title="Select one saved plan for status, probe, or speed tests. Plan create/update/enable/disable applies tunnel config."
          >
            <div className="topologyNetworkTestGroupHeader">
              <strong>Test target</strong>
              <small>Required</small>
            </div>
            <div className="dispatchControls">
              <label>
                <span>Plan</span>
                <select
                  aria-label="Network test plan"
                  onChange={(event) => {
                    clearNetworkReview();
                    setSelectedPlanId(event.target.value);
                  }}
                  value={selectedPlanId}
                >
                  {tunnelPlans.map((plan) => (
                    <option key={plan.id} value={plan.id}>
                      {plan.name}
                      {plan.enabled ? "" : " (disabled)"}
                    </option>
                  ))}
                </select>
              </label>
              <label title="Maximum wall-clock job runtime sent to the backend for each reviewed network job.">
                <span>Max timeout</span>
                <input
                  aria-label="Network test max timeout seconds"
                  max={MAX_CONFIGURABLE_JOB_TIMEOUT_SECS}
                  min={1}
                  onChange={(event) => {
                    clearNetworkReview();
                    setMaxTimeoutSecs(Number(event.target.value));
                  }}
                  type="number"
                  value={maxTimeoutSecs}
                />
              </label>
            </div>
            {endpoint ? (
              <div className="operationNote compactTopologyNote">
                <strong>{selectedPlan?.name ?? "Selected plan"}</strong>
                <span title={agentBackendHint(agents, selectedPlan)}>
                  {clientLabel(selectedPlan?.left_client_id ?? "")} /{" "}
                  {clientLabel(selectedPlan?.right_client_id ?? "")}
                </span>
              </div>
            ) : null}
            <TargetImpactPreview
              mode="generic"
              targets={planTargets}
              title="Plan endpoint visibility"
            />
          </section>

          <section
            className="topologyNetworkTestGroup"
            title="Read-only checks for one selected endpoint side."
          >
            <div className="topologyNetworkTestGroupHeader">
              <strong>Checks</strong>
              <small>Single endpoint</small>
            </div>
            <div className="dispatchControls">
              <label title="Single-endpoint checks run from this side of the selected plan.">
                <span>Endpoint</span>
                <select
                  aria-label="Network test endpoint side"
                  onChange={(event) => {
                    clearNetworkReview();
                    setSide(event.target.value as TunnelEndpointSide);
                  }}
                  value={side}
                >
                  <option value="left">Left endpoint</option>
                  <option value="right">Right endpoint</option>
                </select>
              </label>
              <label title="Number of probe packets for network_probe.">
                <span>Probe count</span>
                <input
                  aria-label="Network probe count"
                  max={20}
                  min={1}
                  onChange={(event) => {
                    clearNetworkReview();
                    setProbeCount(Number(event.target.value));
                  }}
                  type="number"
                  value={probeCount}
                />
              </label>
              <label title="Delay between probe packets.">
                <span>Interval ms</span>
                <input
                  aria-label="Network probe interval milliseconds"
                  max={10_000}
                  min={200}
                  onChange={(event) => {
                    clearNetworkReview();
                    setProbeIntervalMs(Number(event.target.value));
                  }}
                  type="number"
                  value={probeIntervalMs}
                />
              </label>
            </div>
            <div className="topologyNetworkTestActionRow">
              <button
                className="secondaryAction"
                disabled={
                  pending ||
                  networkSnapshot !== null ||
                  !selectedPlan ||
                  !endpoint
                }
                onClick={submitStatus}
                title="Run read-only status inspection for the selected endpoint; no local privilege unlock is required."
                type="button"
              >
                <Search size={17} />
                Inspect status
              </button>
              <button
                className="secondaryAction"
                disabled={
                  pending ||
                  networkSnapshot !== null ||
                  !selectedPlan ||
                  !endpoint ||
                  !privilegeMaterial ||
                  !selectedPlan.enabled
                }
                onClick={submitProbe}
                title={
                  privilegeMaterial
                    ? "Review latency probe with bounded count and interval"
                    : "Unlock privilege before reviewing latency probe"
                }
                type="button"
              >
                <Activity size={17} />
                Run probe
              </button>
            </div>
          </section>

          <section
            className="topologyNetworkTestGroup"
            title="Speed tests are single-endpoint jobs and always require byte and rate safety caps."
          >
            <div className="topologyNetworkTestGroupHeader">
              <strong>Speed test</strong>
              <small>Safety capped</small>
            </div>
            <div className="dispatchControls">
              <label title="Maximum speed-test duration.">
                <span>Duration s</span>
                <input
                  aria-label="Network speed test duration seconds"
                  max={30}
                  min={1}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedDurationSecs(Number(event.target.value));
                  }}
                  type="number"
                  value={speedDurationSecs}
                />
              </label>
              <label title="Required per-run byte safety cap; uncapped speed tests are not submitted.">
                <span>Max data MiB</span>
                <input
                  aria-label="Network speed test max mebibytes"
                  max={256}
                  min={1}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedMaxBytesMiB(Number(event.target.value));
                  }}
                  type="number"
                  value={speedMaxBytesMiB}
                />
              </label>
              <label title="Required bandwidth safety cap.">
                <span>Rate limit Kbps</span>
                <input
                  aria-label="Network speed test rate limit Kbps"
                  max={1_000_000}
                  min={64}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedRateLimitKbps(Number(event.target.value));
                  }}
                  type="number"
                  value={speedRateLimitKbps}
                />
              </label>
              <label title="TCP port opened for the temporary speed-test server.">
                <span>TCP port</span>
                <input
                  aria-label="Network speed test TCP port"
                  max={65_535}
                  min={1024}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedPort(Number(event.target.value));
                  }}
                  type="number"
                  value={speedPort}
                />
              </label>
              <label title="Client connection timeout for the speed-test peer.">
                <span>Timeout ms</span>
                <input
                  aria-label="Network speed test connect timeout milliseconds"
                  max={30_000}
                  min={100}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedConnectTimeoutMs(Number(event.target.value));
                  }}
                  type="number"
                  value={speedConnectTimeoutMs}
                />
              </label>
            </div>
            <div className="topologyNetworkTestActionRow">
              <button
                className="secondaryAction"
                disabled={
                  pending ||
                  networkSnapshot !== null ||
                  !selectedPlan ||
                  !endpoint ||
                  !privilegeMaterial ||
                  !selectedPlan.enabled
                }
                onClick={submitSpeedTest}
                title={
                  privilegeMaterial
                    ? "Review capped speed test against both selected plan endpoints"
                    : "Unlock privilege before reviewing capped speed test"
                }
                type="button"
              >
                <Activity size={17} />
                Review speed test
              </button>
            </div>
          </section>
        </div>
        <ConfirmationPrompt
          confirmLabel={
            networkSnapshot
              ? actionConfirmLabel(networkSnapshot.action)
              : "Run network test"
          }
          detail={networkSnapshot?.detail ?? ""}
          expiresAtUnix={
            networkSnapshot
              ? minSubmissionExpiry(networkSnapshot.submissions)
              : undefined
          }
          items={networkSnapshot?.items ?? []}
          onCancel={() => setNetworkSnapshot(null)}
          onConfirm={() =>
            networkSnapshot && void submitNetworkChange(networkSnapshot)
          }
          open={networkSnapshot !== null}
          pending={pending}
          title={
            networkSnapshot
              ? `Confirm ${actionLabel(networkSnapshot.action).toLowerCase()}`
              : "Confirm network action"
          }
          tone="normal"
        />
        {visibleJobProgress && (
          <ExecutionResultPanel
            loading={jobProgress !== null}
            onClearResults={clearExecutionResults}
            onOpenJobDetails={onOpenJobDetails}
            progress={visibleJobProgress}
          />
        )}
      </form>
      <PrivilegeVaultBox
        lastPayloadHash={lastPayloadHash}
        onOpenUnlock={onOpenPrivilegeUnlock}
        onPrivilegeMaterialChange={(material) => {
          clearNetworkReview();
          setPrivilegeMaterial(material);
        }}
        privilegeMaterial={privilegeMaterial}
      />
    </section>
  );
}

function NetworkTestTrendCharts({
  expectedBandwidthMbps,
  trends,
}: {
  expectedBandwidthMbps?: number | null;
  trends: NetworkObservationTrendRecord[];
}) {
  const probeTrends = sortedTrends(
    trends.filter((trend) => trend.kind === "network_probe"),
  );
  const speedTrends = sortedTrends(
    trends.filter((trend) => trend.kind === "network_speed_test"),
  );
  const latencyTimes = trendTimes(probeTrends);
  const speedTimes = trendTimes(speedTrends);
  const latencyLines: TimeSeriesChartLine[] = [
    trendLine(
      probeTrends,
      "Average latency",
      consolePalette.chart.blue,
      (trend) => trend.latency_avg_ms,
    ),
    trendLine(
      probeTrends,
      "Maximum latency",
      consolePalette.chart.orange,
      (trend) => trend.latency_max_ms,
    ),
    trendLine(
      probeTrends,
      "Minimum latency",
      consolePalette.chart.green,
      (trend) => trend.latency_min_ms,
    ),
  ];
  const lossLines: TimeSeriesChartLine[] = [
    trendLine(probeTrends, "Packet loss", consolePalette.chart.red, (trend) =>
      trend.packet_loss_avg_ratio === null
        ? null
        : trend.packet_loss_avg_ratio * 100,
    ),
  ];
  const speedLines: TimeSeriesChartLine[] = [
    trendLine(
      speedTrends,
      "Average throughput",
      consolePalette.chart.purple,
      (trend) => trend.throughput_avg_mbps,
    ),
    trendLine(
      speedTrends,
      "Maximum throughput",
      consolePalette.chart.cyan,
      (trend) => trend.throughput_max_mbps,
    ),
  ];
  const throughputBaseline = throughputBaselineSummary(
    speedTrends,
    expectedBandwidthMbps ?? null,
  );

  return (
    <section
      className="topologyNetworkTrendCharts"
      aria-label="Network test trend charts"
    >
      <div className="topologyNetworkTrendChartsHeader">
        <div>
          <strong>Trend evidence</strong>
          <span>
            Persisted probe and speed-test ranges for the selected plan.
          </span>
        </div>
        <button
          className="secondaryAction compactAction"
          disabled
          title="Attaching evidence to a topology plan needs a backend evidence-attachment endpoint."
          type="button"
        >
          Attach evidence
        </button>
      </div>
      <div className="topologyNetworkTrendChartGrid">
        <NetworkTrendChartCard
          emptyLabel="No latency trend samples"
          lines={latencyLines}
          times={latencyTimes}
          title="Latency"
          valueFormatter={(value) =>
            value === null ? "-" : `${formatMetric(value)} ms`
          }
        />
        <NetworkTrendChartCard
          emptyLabel="No loss trend samples"
          lines={lossLines}
          times={latencyTimes}
          title="Packet loss"
          valueFormatter={(value) =>
            value === null ? "-" : `${formatMetric(value)}%`
          }
        />
        <NetworkTrendChartCard
          baselineAttention={throughputBaseline?.attention}
          baselineLabel={throughputBaseline?.label}
          emptyLabel="No speed trend samples"
          lines={speedLines}
          times={speedTimes}
          title="Throughput"
          valueFormatter={(value) =>
            value === null ? "-" : `${formatMetric(value)} Mbps`
          }
        />
      </div>
    </section>
  );
}

function NetworkTrendChartCard({
  baselineAttention = false,
  baselineLabel,
  emptyLabel,
  lines,
  times,
  title,
  valueFormatter,
}: {
  baselineAttention?: boolean;
  baselineLabel?: string | null;
  emptyLabel: string;
  lines: TimeSeriesChartLine[];
  times: string[];
  title: string;
  valueFormatter: (value: number | null) => string;
}) {
  const singleSample = times.length === 1;
  const sampleValues = latestLineValues(lines, valueFormatter);
  return (
    <article className="topologyNetworkTrendChartCard">
      <div className="topologyNetworkTrendChartHeader">
        <strong>{title}</strong>
        <span>
          {times.length > 0
            ? `${times.length} sample${times.length === 1 ? "" : "s"}`
            : "No samples"}
        </span>
      </div>
      {singleSample ? (
        <div
          aria-label={`Network test ${title.toLowerCase()} single sample`}
          className="topologyNetworkSingleSample"
        >
          <strong className={baselineAttention ? "attention" : undefined}>
            {baselineLabel ?? "Single evidence bucket"}
          </strong>
          <span>No trend line yet; capture another run to compare movement.</span>
          {sampleValues.length > 0 ? (
            <dl>
              {sampleValues.map((sample) => (
                <div key={sample.label}>
                  <dt>{sample.label}</dt>
                  <dd>{sample.value}</dd>
                </div>
              ))}
            </dl>
          ) : (
            <p>{emptyLabel}</p>
          )}
        </div>
      ) : (
        <TimeSeriesChart
          ariaLabel={`Network test ${title.toLowerCase()} trend`}
          emptyLabel={emptyLabel}
          height={156}
          lines={lines}
          times={times}
          valueFormatter={valueFormatter}
        />
      )}
    </article>
  );
}

type NetworkAction = "status" | "probe" | "speed_test";

type NetworkJobSubmission = {
  command: string;
  confirmed: boolean;
  destructive: boolean;
  forceUnprivileged: boolean;
  jobId: string;
  maxTimeoutSecs: number;
  operation: JobOperation;
  payloadHashHex: string | null;
  planName: string;
  privilegeAssertion: PrivilegeAssertion | null;
  privileged: boolean;
  selectorExpression: string;
  side: TunnelEndpointSide;
  targetClientIds: string[];
  targets: AgentView[];
};

type NetworkActionSnapshot = {
  action: NetworkAction;
  detail: string;
  items: Array<{ label: string; value: string }>;
  submissions: NetworkJobSubmission[];
  targets: AgentView[];
};

function commandName(mode: NetworkAction) {
  if (mode === "probe") {
    return "network_probe";
  }
  if (mode === "speed_test") {
    return "network_speed_test";
  }
  return "network_status";
}

function actionLabel(mode: NetworkAction) {
  if (mode === "probe") {
    return "Probe";
  }
  if (mode === "speed_test") {
    return "Speed test";
  }
  return "Status";
}

function actionConfirmLabel(mode: NetworkAction): string {
  if (mode === "probe") {
    return "Probe latency";
  }
  if (mode === "speed_test") {
    return "Run speed test";
  }
  return "Inspect side";
}

function requiresConfirmation(mode: NetworkAction) {
  return mode === "speed_test";
}

function actionNeedsPrivilege(mode: NetworkAction) {
  return mode !== "status";
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function uniqueClientIds(clientIds: string[]): string[] {
  return Array.from(new Set(clientIds));
}

function minSubmissionExpiry(
  submissions: NetworkJobSubmission[],
): number | undefined {
  const expiries = submissions
    .map((submission) => submission.privilegeAssertion?.expires_unix)
    .filter(
      (value): value is number =>
        typeof value === "number" && Number.isFinite(value),
    );
  return expiries.length ? Math.min(...expiries) : undefined;
}

function planClientIds(plan: TunnelPlanRecord): string[] {
  return [plan.left_client_id, plan.right_client_id];
}

function agentBackendHint(
  agents: AgentView[],
  plan: TunnelPlanRecord | null,
): string {
  if (!plan) {
    return "agent defaults";
  }
  const backendForClient = (clientId: string) =>
    agents.find((candidate) => candidate.id === clientId)?.capabilities
      .network_backend ?? "ifupdown";
  return `backend L ${backendForClient(plan.left_client_id)} / R ${backendForClient(plan.right_client_id)}`;
}

function latestTrend(
  trends: NetworkObservationTrendRecord[],
): NetworkObservationTrendRecord | null {
  return trends.reduce<NetworkObservationTrendRecord | null>(
    (latest, trend) => {
      if (!latest) {
        return trend;
      }
      return Date.parse(trend.latest_observed_at) >
        Date.parse(latest.latest_observed_at)
        ? trend
        : latest;
    },
    null,
  );
}

function sortedTrends(
  trends: NetworkObservationTrendRecord[],
): NetworkObservationTrendRecord[] {
  return [...trends].sort(
    (left, right) =>
      Date.parse(left.latest_observed_at) -
      Date.parse(right.latest_observed_at),
  );
}

function trendTimes(trends: NetworkObservationTrendRecord[]): string[] {
  return trends.map((trend) => trend.latest_observed_at);
}

function trendLine(
  trends: NetworkObservationTrendRecord[],
  label: string,
  color: string,
  valueFor: (trend: NetworkObservationTrendRecord) => number | null,
): TimeSeriesChartLine {
  return {
    color,
    label,
    values: trends.map(valueFor),
  };
}

function latestLineValues(
  lines: TimeSeriesChartLine[],
  valueFormatter: (value: number | null) => string,
): Array<{ label: string; value: string }> {
  return lines
    .map((line) => {
      const value = line.values[line.values.length - 1] ?? null;
      return {
        label: line.label,
        value: valueFormatter(value),
      };
    })
    .filter((sample) => sample.value !== "-");
}

function throughputBaselineSummary(
  speedTrends: NetworkObservationTrendRecord[],
  expectedBandwidthMbps: number | null,
): { attention: boolean; label: string } | null {
  const latestSpeedTrend = latestTrend(speedTrends);
  if (
    !latestSpeedTrend ||
    latestSpeedTrend.throughput_avg_mbps === null ||
    expectedBandwidthMbps === null ||
    expectedBandwidthMbps <= 0
  ) {
    return null;
  }
  const percentOfExpected = Math.round(
    (latestSpeedTrend.throughput_avg_mbps / expectedBandwidthMbps) * 100,
  );
  return {
    attention: percentOfExpected < 80,
    label: `${formatMetric(latestSpeedTrend.throughput_avg_mbps)} Mbps avg - ${percentOfExpected}% of expected ${formatBandwidthMbps(expectedBandwidthMbps)}`,
  };
}

function formatPlanBaseline(plan: TunnelPlanRecord): string {
  const bandwidth = plan.plan.bandwidth_mbps ?? plan.input.bandwidth_mbps;
  const latencyMs = plan.plan.latency_ms ?? plan.input.latency_ms;
  const packetLossRatio =
    plan.plan.packet_loss_ratio ?? plan.input.packet_loss_ratio;
  const ospfCost =
    plan.plan.recommended_ospf_cost ?? plan.recommended_ospf_cost;
  return `${formatBandwidthMbps(bandwidth)}, ${formatMetric(latencyMs)} ms target, ${formatLossRatio(packetLossRatio)} loss, OSPF ${ospfCost}`;
}

function formatRecentEvidence(
  probeTrend: NetworkObservationTrendRecord | null,
  speedTrend: NetworkObservationTrendRecord | null,
): string {
  const parts: string[] = [];
  if (probeTrend) {
    parts.push(
      `Probe ${formatNullableMetric(probeTrend.latency_avg_ms, "ms avg")}, ${formatLossRatio(probeTrend.packet_loss_avg_ratio)} loss`,
    );
  }
  if (speedTrend) {
    parts.push(
      `Speed ${formatNullableMetric(speedTrend.throughput_avg_mbps, "Mbps avg")}, ${formatNullableMetric(speedTrend.throughput_max_mbps, "Mbps max")}`,
    );
  }
  return parts.length > 0 ? parts.join("; ") : "No persisted evidence yet";
}

function formatSpeedSafety(
  durationSecs: number,
  maxBytesMiB: number,
  rateLimitKbps: number,
  port: number,
  connectTimeoutMs: number,
): string {
  return `${durationSecs}s, ${formatMetric(maxBytesMiB)} MiB cap, ${formatRateLimit(rateLimitKbps)}, TCP ${port}, timeout ${connectTimeoutMs} ms`;
}

function formatBandwidthMbps(value: number): string {
  return `${Math.round(value)} Mbps`;
}

function formatRateLimit(kbps: number): string {
  if (kbps >= 1000) {
    return `${formatMetric(kbps / 1000)} Mbps cap`;
  }
  return `${formatMetric(kbps)} Kbps cap`;
}

function formatNullableMetric(value: number | null, unit: string): string {
  return value === null
    ? `${unit} unavailable`
    : `${formatMetric(value)} ${unit}`;
}

function formatLossRatio(value: number | null | undefined): string {
  return value === null || value === undefined
    ? "loss unavailable"
    : `${formatMetric(value * 100)}%`;
}

function formatMetric(value: number): string {
  return Number.isInteger(value)
    ? String(value)
    : value.toFixed(value < 10 ? 2 : 1);
}
