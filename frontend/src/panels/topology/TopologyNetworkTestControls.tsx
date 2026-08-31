import { useEffect, useMemo, useRef, useState } from "react";
import { Activity, GitBranch, Search, ShieldCheck } from "lucide-react";
import {
  buildBulkJobProgress,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobSet,
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
import {
  useByteCountFormatter,
  usePanelDisplaySettings,
  type ByteCountFormatter,
} from "../../panelDisplay";
import { scrollIntoViewWithMotion } from "../../motion";
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
  type NetworkSpeedDirection,
} from "../../topologyNetworkJobs";
import { runtimeManagerLabel } from "../../topologyRuntime";
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
  timestampMillis,
} from "../../utils";
import { MAX_CONFIGURABLE_JOB_TIMEOUT_SECS } from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

export function TopologyNetworkTestControls({
  agents,
  error,
  loading,
  networkTrends,
  onCreateJob,
  onLoadNetworkTrends,
  onLoadOutputs,
  onLoadTargets,
  onOpenJobDetails,
  onOpenJobHistory,
  onOpenPrivilegeUnlock,
  onOpenTunnelPlans,
  privilegeMaterial,
  setPrivilegeMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
  error: string | null;
  loading: boolean;
  networkTrends: NetworkObservationTrendRecord[];
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onLoadNetworkTrends: () => Promise<void>;
  onLoadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onLoadTargets: (jobId: string) => Promise<JobTargetRecord[]>;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenJobHistory?: () => void;
  onOpenPrivilegeUnlock: () => void;
  onOpenTunnelPlans: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => Promise<void>;
  tunnelPlans: TunnelPlanRecord[];
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const formatBytes = useByteCountFormatter();
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
  const [probeCount, setProbeCount] = useState(5);
  const [probeIntervalMs, setProbeIntervalMs] = useState(500);
  const [speedDurationSecs, setSpeedDurationSecs] = useState(10);
  const [speedDirection, setSpeedDirection] =
    useState<NetworkSpeedSelection>("both");
  const [speedMaxBytesMiB, setSpeedMaxBytesMiB] = useState("");
  const [speedRateLimitMbps, setSpeedRateLimitMbps] = useState("");
  const [speedPort, setSpeedPort] = useState(5201);
  const [speedConnectTimeoutMs, setSpeedConnectTimeoutMs] = useState(5000);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastAction, setLastAction] = useState<NetworkAction>("status");
  const [networkSnapshot, setNetworkSnapshot] =
    useState<NetworkActionSnapshot | null>(null);
  const [jobProgress, setJobProgress] = useState<BulkJobProgress | null>(null);
  const [lastJobProgress, setLastJobProgress] =
    useState<BulkJobProgress | null>(null);
  const [lastOutputs, setLastOutputs] = useState<JobOutputRecord[]>([]);
  const [executionPhase, setExecutionPhase] =
    useState<NetworkExecutionPhase | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const [reviewAction, setReviewAction] = useState<NetworkAction | null>(null);
  const feedbackRef = useRef<HTMLDivElement | null>(null);
  const resultRef = useRef<HTMLDivElement | null>(null);
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
  const networkHeaderStatus =
    selectedPlan && !selectedPlan.enabled
      ? "Plan disabled; inspect only"
      : privilegeMaterial
        ? "Dispatch ready"
        : "Inspect available; unlock for probe/speed";
  const executionHasStarted =
    executionPhase !== null ||
    visibleJobProgress !== null ||
    lastOutputs.length > 0;
  const topActionError =
    actionError && networkSnapshot === null && !executionHasStarted
      ? actionError
      : null;
  const executionActionError =
    actionError && executionHasStarted ? actionError : null;
  const networkTestFeedbackMessage =
    error ??
    topActionError ??
    (reviewPending && reviewAction
      ? `Preparing ${actionLabel(reviewAction).toLowerCase()} review`
      : null);

  useEffect(() => {
    if (!topActionError) return;
    const frame = window.requestAnimationFrame(() => {
      if (feedbackRef.current) {
        scrollIntoViewWithMotion(feedbackRef.current, { block: "nearest" });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [topActionError]);

  useEffect(() => {
    if (!visibleJobProgress || networkSnapshot) return;
    const frame = window.requestAnimationFrame(() => {
      if (resultRef.current) {
        scrollIntoViewWithMotion(resultRef.current, { block: "nearest" });
      }
    });
    return () => window.cancelAnimationFrame(frame);
  }, [networkSnapshot, visibleJobProgress]);

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
    setActionError(null);
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
    const probeLimits =
      mode === "probe"
        ? {
            count: requiredInteger(probeCount, "Probe count", 1, 20),
            intervalMs: requiredInteger(
              probeIntervalMs,
              "Probe interval",
              200,
              10_000,
            ),
          }
        : null;
    const speedLimits =
      mode === "speed_test"
        ? {
            durationSecs: requiredInteger(
              speedDurationSecs,
              "Speed-test duration",
              1,
              30,
            ),
            maxBytes:
              optionalWholeNumberLimit(
                speedMaxBytesMiB,
                "Maximum data",
                1,
                256,
              ) *
              1024 *
              1024,
            rateLimitKbps: optionalMbpsLimitKbps(
              speedRateLimitMbps,
              "Rate limit",
              0.064,
              1_000,
            ),
            port: requiredInteger(
              speedPort,
              "Speed-test TCP port",
              1024,
              65_535,
            ),
            connectTimeoutMs: requiredInteger(
              speedConnectTimeoutMs,
              "Speed-test connection timeout",
              100,
              30_000,
            ),
          }
        : null;
    const boundedMaxTimeoutSecs = requiredInteger(
      maxTimeoutSecs,
      "Maximum job timeout",
      1,
      MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
    );
    const buildSubmission = async (
      planRecord: TunnelPlanRecord,
      planSide: TunnelEndpointSide,
      direction: NetworkSpeedDirection | null,
    ): Promise<NetworkJobSubmission> => {
      const builtOperation =
        mode === "status"
          ? buildNetworkStatusOperation(
              planRecord.id,
              planRecord.plan,
              planSide,
            )
          : mode === "probe"
            ? buildNetworkProbeOperation(
                planRecord.id,
                planRecord.plan,
                planSide,
                probeLimits!.count,
                probeLimits!.intervalMs,
              )
            : buildNetworkSpeedTestOperation(
                planRecord.id,
                planRecord.plan,
                direction!,
                speedLimits!.durationSecs,
                speedLimits!.maxBytes,
                speedLimits!.rateLimitKbps,
                speedLimits!.port,
                speedLimits!.connectTimeoutMs,
              );
      const targetClientIds =
        mode === "speed_test"
          ? planClientIds(planRecord)
          : [builtOperation.endpoint.localClientId];
      const selectorExpression =
        selectorExpressionForClientIds(targetClientIds);
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
        speedDirectionLabel:
          direction === null
            ? null
            : speedDirectionLabel(direction, planRecord, clientLabel),
        targetClientIds,
      };
    };
    const speedDirections =
      mode === "speed_test" ? selectedSpeedDirections(speedDirection) : [null];
    const submissions = await Promise.all(
      speedDirections.map((direction) =>
        buildSubmission(selectedPlan, side, direction),
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
    const planLabel = submissions[0]?.planName ?? "unknown";
    const directionSummary =
      mode === "speed_test"
        ? speedDirectionSelectionLabel(speedDirection)
        : null;
    return {
      action: mode,
      detail: `${actionLabel(mode)} ${submissions[0]?.planName ?? "selected plan"} across ${vpsCountLabel(snapshotTargets.length)}.`,
      items:
        mode === "speed_test"
          ? [
              { label: "Plan", value: planLabel },
              {
                label: "Targets",
                value: formatTargetAvailabilitySummary(snapshotTargets),
              },
              { label: "Direction", value: directionSummary! },
              {
                label: "Transfer limit",
                value: `${speedLimits!.durationSecs}s per direction · ${formatDataLimit(speedLimits!.maxBytes, formatBytes)} · ${formatRateLimit(speedLimits!.rateLimitKbps)}`,
              },
              {
                label: "Listener",
                value: `TCP ${speedLimits!.port} · ${speedLimits!.connectTimeoutMs} ms connect timeout`,
              },
              {
                label: "Job deadline",
                value: `${boundedMaxTimeoutSecs}s per direction`,
              },
            ]
          : [
              { label: "Plan", value: planLabel },
              {
                label: "Targets",
                value: formatTargetAvailabilitySummary(snapshotTargets),
              },
              { label: "Endpoint", value: side },
              ...(mode === "probe"
                ? [
                    {
                      label: "Probe cadence",
                      value: `${probeLimits!.count} packets · ${probeLimits!.intervalMs} ms interval`,
                    },
                  ]
                : []),
              { label: "Job deadline", value: `${boundedMaxTimeoutSecs}s` },
            ],
      submissions,
    };
  }

  function clearExecutionResults() {
    setJobProgress(null);
    setLastJobProgress(null);
    setLastOutputs([]);
    setExecutionPhase(null);
  }

  async function submitNetworkChange(snapshot: NetworkActionSnapshot) {
    await runPanelAction(setPending, setActionError, async () => {
      await executeNetworkSnapshot(snapshot);
    });
  }

  async function executeNetworkSnapshot(snapshot: NetworkActionSnapshot) {
    const jobs: NetworkCreatedJob[] = [];
    clearExecutionResults();
    const lastSubmission =
      snapshot.submissions[snapshot.submissions.length - 1] ?? null;
    setLastPayloadHash(lastSubmission?.payloadHashHex ?? null);
    setLastAction(snapshot.action);
    for (const [index, submission] of snapshot.submissions.entries()) {
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
      if (index === 0) {
        setNetworkSnapshot(null);
      }
      jobs.push({ job, submission });
      setExecutionPhase({
        current: index + 1,
        label: submission.speedDirectionLabel,
        total: snapshot.submissions.length,
      });
      setLastOutputs(await trackNetworkProgress(jobs));
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
    jobs: NetworkCreatedJob[],
  ): Promise<JobOutputRecord[]> {
    const jobIds = jobs.map(({ job }) => job.job_id);
    const operationId = jobIds[0];
    const targetCount = jobs.reduce(
      (count, { job }) => count + createJobTargetCount(job),
      0,
    );
    const targets = resolveAgentsById(
      agents,
      uniqueClientIds(
        jobs.flatMap(({ submission }) => submission.targetClientIds),
      ),
    );
    const maxTimeoutSecsForSnapshot = Math.max(
      ...jobs.map(({ submission }) => submission.maxTimeoutSecs),
    );
    setLastJobProgress(null);
    setJobProgress(
      buildBulkJobProgress({
        jobId: operationId,
        jobIds,
        targetCount,
        targetRecords: [],
        targets,
        maxTimeoutSecs: maxTimeoutSecsForSnapshot,
      }),
    );
    try {
      const result = await waitForBulkJobSet(jobIds, onLoadTargets, {
        operationId,
        onLoadOutputs,
        onProgress: setJobProgress,
        targetCount,
        targets,
        maxTimeoutSecs: maxTimeoutSecsForSnapshot,
      });
      setLastJobProgress(result.progress);
      const outputs = await Promise.all(
        jobIds.map((jobId) =>
          loadCompletedNetworkOutputs(jobId, onLoadOutputs),
        ),
      );
      return outputs.flat();
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
            <span>
              {error
                ? "Tunnel plans unavailable"
                : loading
                  ? "Loading tunnel plans"
                  : "No saved tunnel plans"}
            </span>
          </div>
          <ShieldCheck aria-hidden="true" size={20} />
        </div>
        {error ? (
          <ActionFeedback
            className="localActionFeedback"
            message={error}
            tone="danger"
          />
        ) : loading ? (
          <ActionFeedback
            className="localActionFeedback"
            message="Loading tunnel plans"
            tone="progress"
          />
        ) : (
          <div className="emptyState compactEmptyState">
            <strong>Create a declared tunnel before inspecting it</strong>
            <span>
              Status can verify enabled state or disabled-plan cleanup. Probe
              and speed jobs require an enabled plan.
            </span>
            <button
              className="primaryAction compactAction"
              onClick={onOpenTunnelPlans}
              type="button"
            >
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
          ref={feedbackRef}
          tone={error || topActionError ? "danger" : "progress"}
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
              <label title="Maximum wall-clock runtime included in each network job.">
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
              unavailableLabel="-"
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
                title={
                  pending
                    ? "Wait for the current network test operation to finish"
                    : networkSnapshot !== null
                      ? "Finish or cancel the current reviewed network test before inspecting status"
                      : !selectedPlan
                        ? "Select a tunnel plan before inspecting endpoint status"
                        : !endpoint
                          ? "Select a tunnel endpoint before inspecting status"
                          : "Run read-only status inspection for the selected endpoint; no local privilege unlock is required"
                }
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
                  pending
                    ? "Wait for the current network test operation to finish"
                    : networkSnapshot !== null
                      ? "Finish or cancel the current reviewed network test before running a probe"
                      : !selectedPlan
                        ? "Select a tunnel plan before running a latency probe"
                        : !endpoint
                          ? "Select a tunnel endpoint before running a latency probe"
                          : !selectedPlan.enabled
                            ? "Enable this plan before running a latency probe"
                            : privilegeMaterial
                              ? "Run a latency probe with bounded count and interval"
                              : "Unlock privilege before running a latency probe"
                }
                type="button"
              >
                <Activity size={17} />
                Run probe
              </button>
            </div>
          </section>

          <section
            className="topologyNetworkTestGroup topologyNetworkSpeedTestGroup"
            title="Speed tests coordinate both selected plan endpoints. Duration always bounds a run; byte and rate limits are optional."
          >
            <div className="topologyNetworkTestGroupHeader">
              <strong>Speed test</strong>
              <small>Paired endpoints</small>
            </div>
            <div className="dispatchControls">
              <label title="Traffic direction. Both runs the two directions sequentially so they do not compete for the same tunnel or TCP port.">
                <span>Direction</span>
                <select
                  aria-label="Network speed test direction"
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedDirection(
                      event.target.value as NetworkSpeedSelection,
                    );
                  }}
                  value={speedDirection}
                >
                  <option value="both">Both directions</option>
                  <option value="left_to_right">Left → Right</option>
                  <option value="right_to_left">Right → Left</option>
                </select>
              </label>
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
              <label title="Optional per-run byte cap. Leave empty for unlimited data within the selected duration.">
                <span>Max data MiB</span>
                <input
                  aria-label="Network speed test max mebibytes"
                  max={256}
                  min={1}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedMaxBytesMiB(event.target.value);
                  }}
                  placeholder="Unlimited"
                  type="number"
                  value={speedMaxBytesMiB}
                />
              </label>
              <label title="Optional bandwidth cap. Leave empty to use available tunnel bandwidth; the job contract remains exact Kbps internally.">
                <span>Rate limit Mbps</span>
                <input
                  aria-label="Network speed test rate limit Mbps"
                  max={1_000}
                  min={0.064}
                  onChange={(event) => {
                    clearNetworkReview();
                    setSpeedRateLimitMbps(event.target.value);
                  }}
                  placeholder="Unlimited"
                  step="0.001"
                  type="number"
                  value={speedRateLimitMbps}
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
                  pending
                    ? "Wait for the current network test operation to finish"
                    : networkSnapshot !== null
                      ? "Finish or cancel the current reviewed network test before reviewing a speed test"
                      : !selectedPlan
                        ? "Select a tunnel plan before reviewing a speed test"
                        : !endpoint
                          ? "Select a tunnel endpoint before reviewing a speed test"
                          : !selectedPlan.enabled
                            ? "Enable this plan before running a speed test"
                            : privilegeMaterial
                              ? "Review the selected directional speed test against both plan endpoints"
                              : "Unlock privilege before reviewing the speed test"
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
          error={actionError}
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
          <div ref={resultRef} tabIndex={-1}>
            <ActionFeedback
              className="localActionFeedback"
              message={executionActionError}
              tone="danger"
            />
            <ExecutionResultPanel
              context={networkExecutionContext(lastAction, executionPhase)}
              loading={jobProgress !== null}
              onClearResults={clearExecutionResults}
              onOpenJobDetails={onOpenJobDetails}
              onOpenJobHistory={onOpenJobHistory}
              progress={visibleJobProgress}
            >
              <NetworkExecutionEvidence
                clientLabel={clientLabel}
                loading={jobProgress !== null}
                outputs={lastOutputs}
              />
            </ExecutionResultPanel>
          </div>
        )}
        <NetworkTestTrendCharts
          expectedBandwidthMbps={selectedPlan?.plan.bandwidth_mbps ?? null}
          trends={selectedPlanTrends}
        />
      </form>
      {!privilegeMaterial && (
        <PrivilegeVaultBox
          lastPayloadHash={lastPayloadHash}
          onOpenUnlock={onOpenPrivilegeUnlock}
          onPrivilegeMaterialChange={async (material) => {
            clearNetworkReview();
            await setPrivilegeMaterial(material);
          }}
          privilegeMaterial={privilegeMaterial}
        />
      )}
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
  loading,
  outputs,
}: {
  clientLabel: (clientId: string) => string;
  loading: boolean;
  outputs: JobOutputRecord[];
}) {
  const formatBytes = useByteCountFormatter();
  const rows = networkExecutionRows(outputs, clientLabel);
  const speedCards = networkSpeedEvidenceCards(
    outputs,
    clientLabel,
    formatBytes,
  );
  const retainedResultCount = rows.length + speedCards.length;
  return (
    <div
      className="topologyNetworkResultEvidence"
      aria-label="Per-endpoint network test evidence"
    >
      <div className="topologyNetworkResultHeader">
        <strong>Endpoint evidence</strong>
        <span>
          {retainedResultCount > 0
            ? `${retainedResultCount} retained result${retainedResultCount === 1 ? "" : " groups"}`
            : "No retained status output"}
        </span>
      </div>
      {speedCards.length > 0 && (
        <div className="topologyNetworkSpeedEvidence">
          {speedCards.map((card) => (
            <NetworkSpeedEvidenceCard card={card} key={card.direction} />
          ))}
        </div>
      )}
      {rows.length > 0 &&
        rows.map((row) => (
          <div className="topologyNetworkResultRow" key={row.id}>
            <span className="historyPrimary">
              <strong title={row.target}>{row.target}</strong>
              <small title={row.detail}>{row.detail}</small>
            </span>
            <span className="historyPrimary topologyNetworkResultMetric">
              <strong title={row.metric}>{row.metric}</strong>
              <small>{row.kind}</small>
            </span>
            <ConsoleStatusBadge tone={row.tone}>
              {row.status}
            </ConsoleStatusBadge>
          </div>
        ))}
      {retainedResultCount === 0 && (
        <span className="topologyNetworkResultEmpty">
          {loading
            ? "Waiting for structured endpoint evidence while the accepted job runs."
            : "The job is terminal, but no structured status output is retained. Open job details for target state and raw output evidence."}
        </span>
      )}
    </div>
  );
}

function NetworkSpeedEvidenceCard({ card }: { card: NetworkSpeedEvidence }) {
  const chartTimes = speedIntervalTimes(card);
  const chartLines: TimeSeriesChartLine[] = [
    {
      color:
        card.direction === "left_to_right"
          ? consolePalette.chart.blue
          : consolePalette.chart.purple,
      label:
        card.measurementRole === "receiver"
          ? "Receiver throughput"
          : "Sender throughput",
      values: card.intervals.map((interval) => interval.throughputMbps),
    },
  ];
  return (
    <article className="topologyNetworkSpeedCard">
      <div className="topologyNetworkSpeedHeader">
        <span className="historyPrimary">
          <strong title={card.label}>{card.label}</strong>
          <small title={card.detail}>{card.detail}</small>
        </span>
        <ConsoleStatusBadge tone={card.tone}>{card.status}</ConsoleStatusBadge>
      </div>
      <dl className="topologyNetworkSpeedMetrics">
        <div>
          <dt>Min</dt>
          <dd title={formatThroughput(card.minimumMbps)}>
            {formatThroughput(card.minimumMbps)}
          </dd>
        </div>
        <div>
          <dt>Avg</dt>
          <dd title={formatThroughput(card.averageMbps)}>
            {formatThroughput(card.averageMbps)}
          </dd>
        </div>
        <div>
          <dt>Max</dt>
          <dd title={formatThroughput(card.maximumMbps)}>
            {formatThroughput(card.maximumMbps)}
          </dd>
        </div>
      </dl>
      {card.intervals.length > 0 && (
        <div className="topologyNetworkSpeedCurve">
          <TimeSeriesChart
            ariaLabel={`${card.label} per-second ${card.measurementRole} throughput`}
            emptyLabel="No interval throughput evidence"
            height={118}
            lines={chartLines}
            times={chartTimes}
            valueFormatter={(value) =>
              value === null ? "-" : `${formatMetric(value)} Mbps`
            }
          />
        </div>
      )}
    </article>
  );
}

type NetworkSpeedInterval = {
  bytes: number;
  endMs: number;
  startMs: number;
  throughputMbps: number;
};

type NetworkSpeedEvidence = {
  averageMbps: number | null;
  bytes: number | null;
  detail: string;
  direction: NetworkSpeedDirection;
  elapsedMs: number | null;
  intervals: NetworkSpeedInterval[];
  label: string;
  maximumMbps: number | null;
  measurementRole: "receiver" | "sender";
  minimumMbps: number | null;
  observedAt: string;
  status: string;
  tone: "ok" | "warning" | "critical";
};

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
  return parsedNetworkStatusOutputs(outputs).flatMap<NetworkExecutionRow>(
    ({ index, output, parsed }) => {
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
        return [
          {
            detail:
              reasons.length > 0
                ? reasons.map(readableNetworkToken).join(", ")
                : `Interface ${stringValue(parsed.interface) ?? "unknown"}; no runtime drift reason reported`,
            id: `${output.client_id}:${type}:${index}`,
            kind: "Runtime status",
            metric: `${stringValue(parsed.interface) ?? "interface"} · ${readableNetworkToken(status)}`,
            status: healthy ? "Healthy" : readableNetworkToken(status),
            target,
            tone: healthy ? "ok" : "warning",
          },
        ];
      }
      if (type === "tunnel_reachability") {
        const probe = recordValue(parsed.parsed);
        const latencyMin = numberValue(probe.latency_min_ms);
        const latencyAvg = numberValue(probe.latency_avg_ms);
        const latencyMax = numberValue(probe.latency_max_ms);
        const received = numberValue(probe.received);
        const transmitted = numberValue(probe.transmitted);
        const loss = numberValue(probe.packet_loss_ratio);
        const healthy = probe.healthy === true || parsed.success === true;
        const packets =
          received === null || transmitted === null
            ? "Packet count unavailable"
            : `${formatMetric(received)}/${formatMetric(transmitted)} received`;
        const lossLabel =
          loss === null
            ? "loss unavailable"
            : `${formatMetric(loss * 100)}% loss`;
        return [
          {
            detail: `${packets}; ${lossLabel}; target ${stringValue(parsed.target) ?? "peer tunnel address"}`,
            id: `${output.client_id}:${type}:${index}`,
            kind: "Probe · min / avg / max",
            metric:
              latencyMin === null || latencyAvg === null || latencyMax === null
                ? "Latency unavailable"
                : `${formatMetric(latencyMin)} / ${formatMetric(latencyAvg)} / ${formatMetric(latencyMax)} ms`,
            status: healthy ? "Healthy" : "Probe failed",
            target,
            tone: healthy ? "ok" : "critical",
          },
        ];
      }
      if (type === "network_speed_test") return [];
      return [
        {
          detail:
            stringValue(parsed.message) ??
            "Structured network status output retained",
          id: `${output.client_id}:${type}:${index}`,
          kind: readableNetworkToken(type),
          metric: "Result retained",
          status: output.exit_code === 0 ? "Completed" : "Needs review",
          target,
          tone: output.exit_code === 0 ? "ok" : "warning",
        },
      ];
    },
  );
}

type ParsedNetworkStatusOutput = {
  index: number;
  output: JobOutputRecord;
  parsed: Record<string, unknown>;
};

function parsedNetworkStatusOutputs(
  outputs: JobOutputRecord[],
): ParsedNetworkStatusOutput[] {
  return outputs.flatMap<ParsedNetworkStatusOutput>((output, index) => {
    if (output.stream !== "status") return [];
    try {
      const value = JSON.parse(decodeOutputPreview(output.data_base64));
      return value && typeof value === "object" && !Array.isArray(value)
        ? [{ index, output, parsed: value as Record<string, unknown> }]
        : [];
    } catch {
      return [];
    }
  });
}

function networkSpeedEvidenceCards(
  outputs: JobOutputRecord[],
  clientLabel: (clientId: string) => string,
  formatBytes: ByteCountFormatter,
): NetworkSpeedEvidence[] {
  const groups = new Map<NetworkSpeedDirection, ParsedNetworkStatusOutput[]>();
  for (const entry of parsedNetworkStatusOutputs(outputs)) {
    if (stringValue(entry.parsed.type) !== "network_speed_test") continue;
    const direction = networkSpeedDirection(entry.parsed);
    if (!direction) continue;
    const entries = groups.get(direction) ?? [];
    entries.push(entry);
    groups.set(direction, entries);
  }
  return (["left_to_right", "right_to_left"] as const).flatMap((direction) => {
    const entries = groups.get(direction);
    if (!entries?.length) return [];
    const receiver = entries.find(
      (entry) => stringValue(entry.parsed.role) === "server",
    );
    const sender = entries.find(
      (entry) => stringValue(entry.parsed.role) === "client",
    );
    const headline = receiver ?? sender ?? entries[0];
    const senderClientId =
      stringValue(headline.parsed.sender_client_id) ??
      (sender
        ? (stringValue(sender.parsed.client_id) ?? sender.output.client_id)
        : stringValue(receiver?.parsed.peer_client_id));
    const receiverClientId =
      stringValue(headline.parsed.receiver_client_id) ??
      (receiver
        ? (stringValue(receiver.parsed.client_id) ?? receiver.output.client_id)
        : stringValue(sender?.parsed.peer_client_id));
    const succeeded = entries.filter(
      (entry) => entry.parsed.success === true,
    ).length;
    const completed =
      Boolean(receiver) && Boolean(sender) && succeeded === entries.length;
    const headlineStats = headline.parsed;
    const measurementRole = receiver ? "receiver" : "sender";
    const averageMbps = numberValue(headlineStats.throughput_mbps);
    const minimumMbps = numberValue(headlineStats.throughput_min_mbps);
    const maximumMbps = numberValue(headlineStats.throughput_max_mbps);
    const bytes = numberValue(headlineStats.bytes);
    const elapsedMs = numberValue(headlineStats.elapsed_ms);
    const intervals = speedIntervals(headlineStats.throughput_intervals);
    const messages = Array.from(
      new Set(
        entries.flatMap((entry) => {
          const message = stringValue(entry.parsed.message);
          return message ? [message] : [];
        }),
      ),
    );
    const senderLabel = senderClientId
      ? clientLabel(senderClientId)
      : "Unknown sender";
    const receiverLabel = receiverClientId
      ? clientLabel(receiverClientId)
      : "Unknown receiver";
    const detailParts = [
      bytes === null
        ? "Transferred bytes unavailable"
        : `${formatBytes(bytes)} ${measurementRole === "receiver" ? "received" : "sent"}`,
      elapsedMs === null
        ? "elapsed time unavailable"
        : `${formatMetric(elapsedMs / 1000)}s measured`,
    ];
    const senderAverage = sender
      ? numberValue(sender.parsed.throughput_mbps)
      : null;
    if (senderAverage !== null && receiver) {
      detailParts.push(`${formatMetric(senderAverage)} Mbps sender evidence`);
    }
    if (!receiver) {
      detailParts.push("Receiver evidence unavailable");
    }
    detailParts.push(...messages);
    return [
      {
        averageMbps,
        bytes,
        detail: detailParts.join(" · "),
        direction,
        elapsedMs,
        intervals,
        label: `${senderLabel} → ${receiverLabel}`,
        maximumMbps,
        measurementRole,
        minimumMbps,
        observedAt: headline.output.received_at ?? headline.output.created_at,
        status: completed ? "Completed" : succeeded > 0 ? "Partial" : "Failed",
        tone: completed ? "ok" : succeeded > 0 ? "warning" : "critical",
      },
    ];
  });
}

function networkSpeedDirection(
  parsed: Record<string, unknown>,
): NetworkSpeedDirection | null {
  const direction = stringValue(parsed.direction);
  if (direction === "left_to_right" || direction === "right_to_left") {
    return direction;
  }
  const serverSide = stringValue(parsed.server_side);
  if (serverSide === "right") return "left_to_right";
  if (serverSide === "left") return "right_to_left";
  return null;
}

function speedIntervals(value: unknown): NetworkSpeedInterval[] {
  if (!Array.isArray(value)) return [];
  return value.slice(0, 30).flatMap<NetworkSpeedInterval>((candidate) => {
    const interval = recordValue(candidate);
    const startMs = numberValue(interval.start_ms);
    const endMs = numberValue(interval.end_ms);
    const bytes = numberValue(interval.bytes);
    const throughputMbps = numberValue(interval.throughput_mbps);
    return startMs !== null &&
      endMs !== null &&
      endMs > startMs &&
      bytes !== null &&
      bytes >= 0 &&
      throughputMbps !== null &&
      throughputMbps >= 0
      ? [{ bytes, endMs, startMs, throughputMbps }]
      : [];
  });
}

function speedIntervalTimes(card: NetworkSpeedEvidence): string[] {
  const observedAt = Date.parse(card.observedAt);
  const base = Number.isFinite(observedAt)
    ? observedAt - (card.elapsedMs ?? card.intervals.length * 1000)
    : Date.UTC(2000, 0, 1);
  return card.intervals.map((interval, index) =>
    new Date(base + Math.max(interval.endMs, (index + 1) * 1000)).toISOString(),
  );
}

function formatThroughput(value: number | null): string {
  return value === null ? "Unavailable" : `${formatMetric(value)} Mbps`;
}

function recordValue(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function stringArrayValue(value: unknown): string[] {
  return Array.isArray(value)
    ? value.filter(
        (item): item is string => typeof item === "string" && item.length > 0,
      )
    : [];
}

function numberValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function readableNetworkToken(value: string): string {
  return value
    .replace(/_/g, " ")
    .replace(/^./, (letter: string) => letter.toUpperCase());
}

function NetworkTestTrendCharts({
  expectedBandwidthMbps,
  trends,
}: {
  expectedBandwidthMbps?: number | null;
  trends: NetworkObservationTrendRecord[];
}) {
  const probeTrends = sortedTrends(
    trends.filter((trend) => trend.kind === "tunnel_reachability"),
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
  const retainedResolution = trends.reduce(
    (coarsest, trend) =>
      trend.retained && typeof trend.effective_resolution_secs === "number"
        ? Math.max(coarsest, trend.effective_resolution_secs)
        : coarsest,
    0,
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
            {retainedResolution > 0
              ? `Retained tiered history · ${formatTrendResolution(retainedResolution)} coarsest source resolution. Recent exact evidence remains separate.`
              : "Persisted probe and throughput-test ranges for the selected plan."}
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
            ? `${times.length} evidence point${times.length === 1 ? "" : "s"}`
            : "No evidence points"}
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
          <span>
            No trend line yet; capture another run to compare movement.
          </span>
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
type NetworkSpeedSelection = NetworkSpeedDirection | "both";

type NetworkExecutionPhase = {
  current: number;
  label: string | null;
  total: number;
};

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
  speedDirectionLabel: string | null;
  targetClientIds: string[];
};

type NetworkCreatedJob = {
  job: CreateJobResponse;
  submission: NetworkJobSubmission;
};

type NetworkActionSnapshot = {
  action: NetworkAction;
  detail: string;
  items: Array<{ label: string; value: string }>;
  submissions: NetworkJobSubmission[];
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

function selectedSpeedDirections(
  selection: NetworkSpeedSelection,
): NetworkSpeedDirection[] {
  return selection === "both"
    ? ["left_to_right", "right_to_left"]
    : [selection];
}

function speedDirectionSelectionLabel(
  selection: NetworkSpeedSelection,
): string {
  if (selection === "both") {
    return "Left → right; then right → left (sequential)";
  }
  return selection === "left_to_right" ? "Left → right" : "Right → left";
}

function speedDirectionLabel(
  direction: NetworkSpeedDirection,
  plan: TunnelPlanRecord,
  clientLabel: (clientId: string) => string,
): string {
  const left = clientLabel(plan.left_client_id);
  const right = clientLabel(plan.right_client_id);
  return direction === "left_to_right"
    ? `${left} → ${right}`
    : `${right} → ${left}`;
}

function networkExecutionContext(
  action: NetworkAction,
  phase: NetworkExecutionPhase | null,
): string {
  const base = `Network ${actionLabel(action).toLowerCase()}`;
  if (action !== "speed_test" || !phase) return base;
  const direction = phase.label ? ` · ${phase.label}` : "";
  return `${base} · phase ${phase.current}/${phase.total}${direction}`;
}

function runtimeOwnershipHint(plan: TunnelPlanRecord | null): string {
  if (!plan) {
    return "No tunnel plan selected";
  }
  const manager = plan.plan.runtime_control?.manager ?? "agent_builtin";
  return `Runtime ownership: ${runtimeManagerLabel(manager)}`;
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
      timestampMillis(trendTime(left)) - timestampMillis(trendTime(right)),
  );
}

function trendTimes(trends: NetworkObservationTrendRecord[]): string[] {
  return trends.map(trendTime);
}

function trendTime(trend: NetworkObservationTrendRecord): string {
  return trend.bucket_start ?? trend.latest_observed_at;
}

function formatTrendResolution(seconds: number): string {
  if (seconds % 86_400 === 0) return `${seconds / 86_400}d`;
  if (seconds % 3_600 === 0) return `${seconds / 3_600}h`;
  if (seconds % 60 === 0) return `${seconds / 60}m`;
  return `${seconds}s`;
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

function formatDataLimit(
  maxBytes: number,
  formatBytes: ByteCountFormatter,
): string {
  return maxBytes === 0 ? "unlimited data" : `${formatBytes(maxBytes)} cap`;
}

function formatBandwidthMbps(value: number): string {
  return `${Math.round(value)} Mbps`;
}

function formatRateLimit(kbps: number): string {
  if (kbps === 0) return "unlimited rate";
  if (kbps >= 1000) {
    const wholeMbps = Math.floor(kbps / 1000);
    const fractionalMbps = String(kbps % 1000)
      .padStart(3, "0")
      .replace(/0+$/, "");
    return `${wholeMbps}${fractionalMbps ? `.${fractionalMbps}` : ""} Mbps cap`;
  }
  return `${formatMetric(kbps)} Kbps cap`;
}

function optionalWholeNumberLimit(
  value: string,
  label: string,
  min: number,
  max: number,
): number {
  if (value.trim() === "") return 0;
  const parsed = Number(value);
  if (
    !Number.isFinite(parsed) ||
    !Number.isInteger(parsed) ||
    parsed < min ||
    parsed > max
  ) {
    throw new Error(
      `${label} must be a whole number from ${min} to ${max}, or left empty for unlimited`,
    );
  }
  return parsed;
}

function optionalMbpsLimitKbps(
  value: string,
  label: string,
  min: number,
  max: number,
): number {
  if (value.trim() === "") return 0;
  const parsed = Number(value);
  if (
    !Number.isFinite(parsed) ||
    parsed < min ||
    parsed > max ||
    Number(parsed.toFixed(3)) !== parsed
  ) {
    throw new Error(
      `${label} must be from ${min} to ${max} in 0.001 Mbps increments, or left empty for unlimited`,
    );
  }
  return Math.round(parsed * 1_000);
}

function requiredInteger(
  value: number,
  label: string,
  min: number,
  max: number,
): number {
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${label} must be a whole number from ${min} to ${max}`);
  }
  return value;
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
