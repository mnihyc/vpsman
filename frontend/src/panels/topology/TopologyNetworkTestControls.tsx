import { useEffect, useMemo, useState } from "react";
import { Activity, GitBranch, Search, ShieldCheck } from "lucide-react";
import {
  bulkOutcomeSummary,
  buildBulkJobProgress,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../../bulkJobProgress";
import { ActionFeedback } from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ConsoleStatusBadge } from "../../components/ConsoleLayout";
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
  JobOutputRecord,
  JobTargetRecord,
  NetworkObservationTrendRecord,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import { networkObservationMetricDefinition } from "../../telemetryMetrics";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  decodeOutputPreview,
  runPanelAction,
  shortId,
  timestampMillis,
} from "../../utils";
import {
  clampJobMaxTimeoutSecs,
  clampInteger,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

export function TopologyNetworkTestControls({
  agents,
  loading,
  networkTrends,
  onCreateJob,
  onLoadNetworkTrends,
  onLoadOutputs,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  onOpenTunnelPlans,
  privilegeMaterial,
  setPrivilegeMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
  loading: boolean;
  networkTrends: NetworkObservationTrendRecord[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadNetworkTrends: () => Promise<void>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenTunnelPlans: () => void;
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
  const [lastOutputs, setLastOutputs] = useState<JobOutputRecord[]>([]);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const [reviewAction, setReviewAction] = useState<NetworkAction | null>(null);
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
    ? `${actionLabel(lastAction)} ${shortId(visibleJobProgress.jobId)} ${jobProgress ? "in progress" : bulkOutcomeSummary(visibleJobProgress)}`
    : lastJob
      ? `${actionLabel(lastAction)} ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} target${lastJob.target_count === 1 ? "" : "s"}`
      : "No local network test run in this view";
  const networkHeaderStatus = selectedPlan && !selectedPlan.enabled
    ? "Plan disabled; inspect only"
    : privilegeMaterial
      ? "Dispatch ready"
      : "Inspect available; unlock for probe/speed";
  const networkTestFeedbackMessage =
    actionError ?? (reviewPending && reviewAction
      ? `Preparing ${actionLabel(reviewAction).toLowerCase()} review`
      : null);

  useEffect(() => {
    if (tunnelPlans.length === 0) {
      if (selectedPlanId) setSelectedPlanId("");
      return;
    }
    if (!tunnelPlans.some((plan) => plan.id === selectedPlanId)) {
      setSelectedPlanId(tunnelPlans[0].id);
    }
  }, [selectedPlanId, tunnelPlans]);

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
    setReviewAction(mode);
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
      setReviewAction(null);
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
          ? buildNetworkStatusOperation(planRecord.id, planRecord.plan, planSide)
          : mode === "probe"
            ? buildNetworkProbeOperation(
                planRecord.id,
                planRecord.plan,
                planSide,
                boundedProbeCount,
                boundedProbeIntervalMs,
              )
            : buildNetworkSpeedTestOperation(
                planRecord.id,
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
          value: mode === "speed_test" || submissions.length > 1
            ? "Both endpoints"
            : side,
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
    setLastOutputs([]);
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
    const outputs: JobOutputRecord[] = [];
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
      outputs.push(...await trackNetworkProgress(
        job,
        submission.targets,
        submission.maxTimeoutSecs,
      ));
      setLastOutputs(outputs.slice());
    }
    try {
      await onLoadNetworkTrends();
    } catch (error) {
      setActionError(
        `Test completed, but trend evidence could not refresh: ${error instanceof Error ? error.message : "request failed"}`,
      );
    }
  }

  async function trackNetworkProgress(
    job: CreateJobResponse,
    targets: AgentView[],
    maxTimeoutSecsForSnapshot: number,
  ): Promise<JobOutputRecord[]> {
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
        onLoadOutputs,
        onProgress: setJobProgress,
        targetCount,
        targets,
        maxTimeoutSecs: maxTimeoutSecsForSnapshot,
      });
      setLastJobProgress(result.progress);
      return await loadCompletedNetworkOutputs(job.job_id, onLoadOutputs);
    } finally {
      setJobProgress(null);
    }
  }

  if (tunnelPlans.length === 0) {
    return (
      <section className="fleetPanel commandComposer">
        <div className="sectionHeader">
          <div>
            <h2>Network tests</h2>
            <span>{loading ? "Loading tunnel plans" : "No saved tunnel plans"}</span>
          </div>
          <ShieldCheck aria-hidden="true" size={20} />
        </div>
        {loading ? (
          <ActionFeedback className="localActionFeedback" message="Loading tunnel plans" tone="progress" />
        ) : (
          <div className="emptyState compactEmptyState">
            <strong>Create a declared tunnel before inspecting it</strong>
            <span>Status can verify enabled state or disabled-plan cleanup. Probe and speed jobs require an enabled plan.</span>
            <button className="primaryAction compactAction" onClick={onOpenTunnelPlans} type="button">
              <GitBranch size={15} />
              Open tunnel plans
            </button>
          </div>
        )}
      </section>
    );
  }

  return (
    <section className="fleetPanel commandComposer">
      <div className="sectionHeader">
        <div>
          <h2>Network tests</h2>
          <span>{networkHeaderStatus}</span>
        </div>
        <div className="headerActionStack">
          <ShieldCheck size={20} />
        </div>
      </div>
      <form
        className="dispatchForm topologyNetworkTestForm"
        onSubmit={(event) => event.preventDefault()}
      >
        <ActionFeedback
          className="localActionFeedback"
          message={networkTestFeedbackMessage}
          tone={actionError ? "danger" : "progress"}
        />
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
              <label title="Maximum wall-clock runtime included in each reviewed network job.">
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
                <span title={runtimeOwnershipHint(selectedPlan)}>
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
                  !selectedPlan?.enabled
                    ? "Enable this plan before running a latency probe"
                    : privilegeMaterial
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
            title="Speed tests coordinate both selected plan endpoints and always require byte and rate safety caps."
          >
            <div className="topologyNetworkTestGroupHeader">
              <strong>Speed test</strong>
              <small>Paired endpoints</small>
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
                  !selectedPlan?.enabled
                    ? "Enable this plan before running a speed test"
                    : privilegeMaterial
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
        {networkSnapshot === null && visibleJobProgress && (
          <ExecutionResultPanel
            context={`Network ${actionLabel(lastAction).toLowerCase()}`}
            loading={jobProgress !== null}
            onClearResults={clearExecutionResults}
            onOpenJobDetails={onOpenJobDetails}
            progress={visibleJobProgress}
          >
            <NetworkExecutionEvidence
              clientLabel={clientLabel}
              outputs={lastOutputs}
            />
          </ExecutionResultPanel>
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

async function loadCompletedNetworkOutputs(
  jobId: string,
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>,
): Promise<JobOutputRecord[]> {
  let outputs: JobOutputRecord[] = [];
  for (let attempt = 0; attempt < 6; attempt += 1) {
    try {
      outputs = await onLoadOutputs(jobId);
    } catch {
      // The terminal target result remains visible while retained output catches up.
    }
    if (outputs.some((output) => output.stream === "status" && output.done)) {
      break;
    }
    await new Promise((resolve) => globalThis.setTimeout(resolve, 250));
  }
  return outputs;
}

function NetworkExecutionEvidence({
  clientLabel,
  outputs,
}: {
  clientLabel: (clientId: string) => string;
  outputs: JobOutputRecord[];
}) {
  const rows = networkExecutionRows(outputs, clientLabel);
  return (
    <div className="topologyNetworkResultEvidence" aria-label="Per-endpoint network test evidence">
      <div className="topologyNetworkResultHeader">
        <strong>Endpoint evidence</strong>
        <span>{rows.length > 0 ? `${rows.length} retained result${rows.length === 1 ? "" : "s"}` : "No retained status output"}</span>
      </div>
      {rows.length > 0 ? rows.map((row) => (
        <div className="topologyNetworkResultRow" key={row.id}>
          <span className="historyPrimary">
            <strong title={row.target}>{row.target}</strong>
            <small title={row.detail}>{row.detail}</small>
          </span>
          <span className="historyPrimary topologyNetworkResultMetric">
            <strong title={row.metric}>{row.metric}</strong>
            <small>{row.kind}</small>
          </span>
          <ConsoleStatusBadge tone={row.tone}>{row.status}</ConsoleStatusBadge>
        </div>
      )) : (
        <span className="topologyNetworkResultEmpty">
          The job is terminal, but no structured status output is retained. Open job details for target state and raw output evidence.
        </span>
      )}
    </div>
  );
}

type NetworkExecutionRow = {
  detail: string;
  id: string;
  kind: string;
  metric: string;
  status: string;
  target: string;
  tone: "ok" | "warning" | "critical" | "info";
};

function networkExecutionRows(
  outputs: JobOutputRecord[],
  clientLabel: (clientId: string) => string,
): NetworkExecutionRow[] {
  return outputs.flatMap<NetworkExecutionRow>((output, index) => {
    if (output.stream !== "status") return [];
    let parsed: Record<string, unknown>;
    try {
      const value = JSON.parse(decodeOutputPreview(output.data_base64));
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      parsed = value as Record<string, unknown>;
    } catch {
      return [];
    }
    const type = stringValue(parsed.type) ?? "network_result";
    const clientId = stringValue(parsed.client_id) ?? output.client_id;
    const peerId = stringValue(parsed.peer_client_id);
    const target = peerId
      ? `${clientLabel(clientId)} -> ${clientLabel(peerId)}`
      : clientLabel(clientId);
    if (type === "network_status") {
      const runtime = recordValue(parsed.runtime);
      const summary = recordValue(runtime.summary);
      const reasons = stringArrayValue(summary.reasons);
      const status = stringValue(summary.status) ?? "unknown";
      const healthy = summary.healthy === true;
      return [{
        detail: reasons.length > 0
          ? reasons.map(readableNetworkToken).join(", ")
          : `Interface ${stringValue(parsed.interface) ?? "unknown"}; no runtime drift reason reported`,
        id: `${output.client_id}:${type}:${index}`,
        kind: "Runtime status",
        metric: `${stringValue(parsed.interface) ?? "interface"} · ${readableNetworkToken(status)}`,
        status: healthy ? "Healthy" : readableNetworkToken(status),
        target,
        tone: healthy ? "ok" : "warning",
      }];
    }
    if (type === "network_probe") {
      const probe = recordValue(parsed.parsed);
      const latency = numberValue(probe.latency_avg_ms);
      const loss = numberValue(probe.packet_loss_ratio);
      const healthy = probe.healthy === true || parsed.success === true;
      return [{
        detail: `${loss === null ? "Loss unavailable" : `${formatMetric(loss * 100)}% loss`}; target ${stringValue(parsed.target) ?? "peer tunnel address"}`,
        id: `${output.client_id}:${type}:${index}`,
        kind: "Probe",
        metric: latency === null ? "Latency unavailable" : `${formatMetric(latency)} ms average`,
        status: healthy ? "Healthy" : "Probe failed",
        target,
        tone: healthy ? "ok" : "critical",
      }];
    }
    if (type === "network_speed_test") {
      const throughput = numberValue(parsed.throughput_mbps);
      const bytes = numberValue(parsed.bytes);
      const success = parsed.success === true;
      const message = stringValue(parsed.message);
      return [{
        detail: `${readableNetworkToken(stringValue(parsed.role) ?? "endpoint")} role; ${bytes === null ? "bytes unavailable" : `${formatNetworkBytes(bytes)} transferred`}${message ? `; ${message}` : ""}`,
        id: `${output.client_id}:${type}:${index}`,
        kind: "Speed test",
        metric: throughput === null ? "Throughput unavailable" : `${formatMetric(throughput)} Mbps`,
        status: success ? "Completed" : "Failed",
        target,
        tone: success ? "ok" : "critical",
      }];
    }
    return [{
      detail: stringValue(parsed.message) ?? "Structured network status output retained",
      id: `${output.client_id}:${type}:${index}`,
      kind: readableNetworkToken(type),
      metric: "Result retained",
      status: output.exit_code === 0 ? "Completed" : "Needs review",
      target,
      tone: output.exit_code === 0 ? "ok" : "warning",
    }];
  });
}

function recordValue(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function stringArrayValue(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string" && item.length > 0)
    : [];
}

function numberValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readableNetworkToken(value: string): string {
  return value.replace(/_/g, " ").replace(/^./, (letter: string) => letter.toUpperCase());
}

function formatNetworkBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${formatMetric(value / 1024)} KiB`;
  return `${formatMetric(value / (1024 * 1024))} MiB`;
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
            Persisted probe and capped throughput-test ranges for the selected plan.
          </span>
        </div>
      </div>
      <div className="topologyNetworkTrendChartGrid">
        <NetworkTrendChartCard
          definition={networkObservationMetricDefinition("latency")}
          emptyLabel="No latency trend samples"
          lines={latencyLines}
          times={latencyTimes}
          title="Latency"
          valueFormatter={(value) =>
            value === null ? "-" : `${formatMetric(value)} ms`
          }
        />
        <NetworkTrendChartCard
          definition={networkObservationMetricDefinition("loss")}
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
          definition={networkObservationMetricDefinition("throughput")}
          emptyLabel="No throughput trend samples"
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
  definition,
  emptyLabel,
  lines,
  times,
  title,
  valueFormatter,
}: {
  baselineAttention?: boolean;
  baselineLabel?: string | null;
  definition: string;
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
      <p className="observabilityMetricDefinition" title={definition}>
        Metric definition: {definition}
      </p>
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

function runtimeOwnershipHint(plan: TunnelPlanRecord | null): string {
  if (!plan) {
    return "No tunnel plan selected";
  }
  const manager = plan.plan.runtime_control?.manager ?? "agent_iproute2_managed";
  return `Runtime ownership: ${manager.replace(/_/g, " ")}`;
}

function latestTrend(
  trends: NetworkObservationTrendRecord[],
): NetworkObservationTrendRecord | null {
  return trends.reduce<NetworkObservationTrendRecord | null>(
    (latest, trend) => {
      if (!latest) {
        return trend;
      }
      return timestampMillis(trend.latest_observed_at) >
        timestampMillis(latest.latest_observed_at)
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
      timestampMillis(left.latest_observed_at) -
      timestampMillis(right.latest_observed_at),
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
  const ospf = plan.plan.ospf ?? plan.input.ospf;
  if (!ospf) {
    return `${formatBandwidthMbps(bandwidth)}, OSPF off`;
  }
  const ospfCost =
    plan.plan.recommended_ospf_cost ?? plan.recommended_ospf_cost;
  return `${formatBandwidthMbps(bandwidth)}, ${formatMetric(ospf.planned_latency_ms)} ms target, ${formatLossRatio(ospf.planned_packet_loss_ratio)} loss, OSPF ${ospfCost ?? "unverified"}`;
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
      `Throughput ${formatNullableMetric(speedTrend.throughput_avg_mbps, "Mbps avg")}, ${formatNullableMetric(speedTrend.throughput_max_mbps, "Mbps max")}`,
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
