import { useMemo, useState, type FormEvent } from "react";
import { Activity, Play, RotateCcw, Search, ShieldCheck } from "lucide-react";
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
import { useReviewGenerationGuard, waitForReviewRender } from "../../hooks/useReviewGenerationGuard";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeAssertion, type PrivilegeMaterial } from "../../privilege";
import { selectorExpressionForClientIds } from "../../searchExpression";
import {
  buildNetworkApplyOperation,
  buildNetworkProbeOperation,
  buildNetworkRollbackOperation,
  buildNetworkSpeedTestOperation,
  buildNetworkStatusOperation,
  renderTunnelEndpointConfig,
} from "../../topologyApply";
import type {
  AgentView,
  CreateJobRequest,
  CreateJobResponse,
  JobOperation,
  JobTargetRecord,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import { clientDisplayNameFromMap, clientDisplayNameMap, runPanelAction, shortId } from "../../utils";
import {
  clampJobMaxTimeoutSecs,
  clampInteger,
  MAX_CONFIGURABLE_JOB_TIMEOUT_SECS,
} from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

const ALL_PLANNED_SCOPE = "__all_planned__";

export function TopologyApplyControls({
  agents,
  onCreateJob,
  onLoadTargets,
  onOpenJobDetails,
  onOpenPrivilegeUnlock,
  privilegeMaterial,
  setPrivilegeMaterial,
  tunnelPlans,
}: {
  agents: AgentView[];
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
  const [selectedPlanId, setSelectedPlanId] = useState(() => tunnelPlans[0]?.id ?? "");
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [maxTimeoutSecs, setMaxTimeoutSecs] = useState(60);
  const [probeCount, setProbeCount] = useState(3);
  const [probeIntervalMs, setProbeIntervalMs] = useState(500);
  const [speedDurationSecs, setSpeedDurationSecs] = useState(3);
  const [speedMaxBytesMiB, setSpeedMaxBytesMiB] = useState(16);
  const [speedRateLimitKbps, setSpeedRateLimitKbps] = useState(100_000);
  const [speedPort, setSpeedPort] = useState(5201);
  const [speedConnectTimeoutMs, setSpeedConnectTimeoutMs] = useState(5000);
  const [forceUnprivileged, setForceUnprivileged] = useState(false);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [lastAction, setLastAction] = useState<NetworkAction>("apply");
  const [networkSnapshot, setNetworkSnapshot] = useState<NetworkActionSnapshot | null>(null);
  const [jobProgress, setJobProgress] = useState<BulkJobProgress | null>(null);
  const [lastJobProgress, setLastJobProgress] = useState<BulkJobProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const plannedApplyPlans = useMemo(() => buildPlannedApplyPlans(tunnelPlans), [tunnelPlans]);
  const bulkApplySelected = selectedPlanId === ALL_PLANNED_SCOPE;
  const selectedPlan = bulkApplySelected
    ? null
    : (tunnelPlans.find((plan) => plan.id === selectedPlanId) ?? tunnelPlans[0] ?? null);
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
  const endpoint = useMemo(
    () => (selectedPlan ? renderTunnelEndpointConfig(selectedPlan.plan, side) : null),
    [selectedPlan, side],
  );
  const mutationTargets = resolveAgentsById(
    agents,
    bulkApplySelected
      ? uniqueClientIds(plannedApplyPlans.flatMap((candidate) => planClientIds(candidate.plan)))
      : selectedPlan
        ? planClientIds(selectedPlan)
        : [],
  );
  const visibleJobProgress = jobProgress ?? lastJobProgress;
  const status =
    actionError ??
    (reviewPending
      ? `Preparing ${actionLabel(lastAction).toLowerCase()} review`
      : visibleJobProgress
      ? `${actionLabel(lastAction)} result for job ${shortId(visibleJobProgress.jobId)}`
      : lastJob
        ? `${actionLabel(lastAction)} job ${shortId(lastJob.job_id)} ${lastJob.status}; ${lastJob.target_count} targets`
      : selectedPlan && !selectedPlan.enabled
        ? "Plan disabled; inspect and rollback only"
      : privilegeMaterial
        ? "Ready"
        : "Locked");

  function submitApply(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    void openNetworkPrompt("apply");
  }

  function submitRollback() {
    void openNetworkPrompt("rollback");
  }

  function submitStatus() {
    void openNetworkPrompt("status");
  }

  function submitProbe() {
    void openNetworkPrompt("probe");
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
        if (!privilegeMaterial) {
          throw new Error("Privilege unlock is locked");
        }
        if (bulkApplySelected && mode !== "apply") {
          throw new Error("All Unplanned is only available for apply");
        }
        if (!bulkApplySelected && (!selectedPlan || !endpoint)) {
          throw new Error("Select a tunnel plan");
        }
        if (!bulkApplySelected && selectedPlan && !selectedPlan.enabled && !disabledPlanAllowsAction(mode)) {
          throw new Error("Tunnel plan is disabled");
        }
        const boundedProbeCount = clampInteger(probeCount, 1, 20);
        const boundedProbeIntervalMs = clampInteger(probeIntervalMs, 200, 10_000);
        const boundedSpeedDurationSecs = clampInteger(speedDurationSecs, 1, 30);
        const boundedSpeedMaxBytes = clampInteger(speedMaxBytesMiB, 1, 256) * 1024 * 1024;
        const boundedSpeedRateLimitKbps = clampInteger(speedRateLimitKbps, 64, 1_000_000);
        const boundedSpeedPort = clampInteger(speedPort, 1024, 65_535);
        const boundedSpeedConnectTimeoutMs = clampInteger(speedConnectTimeoutMs, 100, 30_000);
        const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(maxTimeoutSecs);
        const boundedForceUnprivileged = isMutation(mode) ? forceUnprivileged : false;
        const buildSubmission = async (
          planRecord: TunnelPlanRecord,
          planSide: TunnelEndpointSide,
        ): Promise<NetworkJobSubmission> => {
          const builtOperation =
            mode === "apply"
              ? await buildNetworkApplyOperation(planRecord.plan, planSide)
              : mode === "rollback"
                ? buildNetworkRollbackOperation(planRecord.plan, planSide)
                : mode === "status"
                  ? buildNetworkStatusOperation(planRecord.plan, planSide)
                  : mode === "probe"
                    ? buildNetworkProbeOperation(planRecord.plan, planSide, boundedProbeCount, boundedProbeIntervalMs)
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
              ? [builtOperation.endpoint.localClientId, builtOperation.endpoint.peerClientId]
              : [builtOperation.endpoint.localClientId];
          const selectorExpression = selectorExpressionForClientIds(targetClientIds);
          const builtPrivilege = await buildPrivilegeForJobOperation({
            clientIds: targetClientIds,
            commandType: commandName(mode),
            forceUnprivileged: boundedForceUnprivileged,
            operation: builtOperation.operation,
            privilegeMaterial,
            selectorExpression,
            maxTimeoutSecs: boundedMaxTimeoutSecs,
          });
          return {
            command: commandName(mode),
            confirmed: requiresConfirmation(mode),
            destructive: isMutation(mode),
            forceUnprivileged: boundedForceUnprivileged,
            jobId: crypto.randomUUID(),
            maxTimeoutSecs: boundedMaxTimeoutSecs,
            operation: builtOperation.operation,
            payloadHashHex: builtPrivilege.payloadHashHex,
            planName: planRecord.name,
            privilegeAssertion: builtPrivilege.privilegeAssertion,
            selectorExpression,
            side: planSide,
            targetClientIds,
            targets: resolveAgentsById(agents, targetClientIds),
          };
        };
        const submissionCandidates = bulkApplySelected
          ? plannedApplyPlans.flatMap((candidate) => planEndpointSides(candidate.plan).map((planSide) => ({
              plan: candidate.plan,
              side: planSide,
            })))
          : selectedPlan && (mode === "apply" || mode === "rollback")
            ? planEndpointSides(selectedPlan).map((planSide) => ({ plan: selectedPlan, side: planSide }))
            : selectedPlan
              ? [{ plan: selectedPlan, side }]
              : [];
        const submissions = await Promise.all(
          submissionCandidates.map((candidate) => buildSubmission(candidate.plan, candidate.side)),
        );
        if (!submissions.length) {
          throw new Error("No unplanned tunnel plans are ready to apply");
        }
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const snapshotTargets = resolveAgentsById(
          agents,
          uniqueClientIds(submissions.flatMap((submission) => submission.targetClientIds)),
        );
        const scopeLabel = bulkApplySelected
          ? `All unplanned (${plannedApplyPlans.length})`
          : mode === "apply" || mode === "rollback"
            ? "Selected plan"
            : "Selected endpoint";
        const planLabel = bulkApplySelected
          ? `${uniquePlanCount(submissions)} plans`
          : submissions[0]?.planName ?? "unknown";
        setNetworkSnapshot({
          action: mode,
          bulk: bulkApplySelected,
          detail: bulkApplySelected
            ? `Apply ${plannedApplyPlans.length} unplanned plans with ${submissions.length} endpoint jobs.`
            : submissions.length > 1
              ? `${actionLabel(mode)} ${submissions[0]?.planName ?? "selected plan"} on both endpoints.`
            : `${actionLabel(mode)} ${submissions[0]?.planName ?? "selected plan"} on ${vpsCountLabel(snapshotTargets.length)}.`,
          forceUnprivileged: boundedForceUnprivileged,
          items: [
            { label: "Operation", value: actionLabel(mode) },
            { label: "Scope", value: scopeLabel },
            { label: "Targets", value: formatTargetAvailabilitySummary(snapshotTargets) },
            { label: "Plans", value: planLabel },
            { label: "Endpoint", value: submissions.length > 1 ? "Both endpoints" : side },
            { label: "Max timeout", value: `${boundedMaxTimeoutSecs}s` },
            { label: "Privilege unlock", value: "Unlocked locally" },
            ...(isMutation(mode)
              ? [{ label: "Privilege", value: boundedForceUnprivileged ? "Forced best effort" : "Root required" }]
              : []),
          ],
          submissions,
          targets: snapshotTargets,
        });
      });
    } finally {
      setReviewPending(false);
    }
  }

  function clearExecutionResults() {
    setJobProgress(null);
    setLastJobProgress(null);
    setLastJob(null);
  }

  async function submitNetworkChange(snapshot: NetworkActionSnapshot) {
    setNetworkSnapshot(null);
    clearExecutionResults();
    await runPanelAction(setPending, setActionError, async () => {
      const jobs: Array<{ job: CreateJobResponse; submission: NetworkJobSubmission }> = [];
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
          privileged: true,
          privilege_assertion: submission.privilegeAssertion,
          max_timeout_secs: submission.maxTimeoutSecs,
        });
        jobs.push({ job, submission });
      }
      const lastSubmission = snapshot.submissions[snapshot.submissions.length - 1] ?? null;
      setLastPayloadHash(lastSubmission?.payloadHashHex ?? null);
      setLastAction(snapshot.action);
      for (const { job, submission } of jobs) {
        setLastJob(job);
        await trackNetworkProgress(job, submission.targets, submission.maxTimeoutSecs);
      }
    });
  }

  async function trackNetworkProgress(job: CreateJobResponse, targets: AgentView[], maxTimeoutSecsForSnapshot: number) {
    const targetCount = createJobTargetCount(job);
    setLastJobProgress(null);
    setJobProgress(buildBulkJobProgress({
      jobId: job.job_id,
      targetCount,
      targetRecords: [],
      targets,
      maxTimeoutSecs: maxTimeoutSecsForSnapshot,
    }));
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
          <h2>Network apply</h2>
          <span>{status}</span>
        </div>
        <ShieldCheck size={20} />
      </div>
      <form className="dispatchForm topologyApplyForm" onSubmit={submitApply}>
        <div className="topologyApplyGroups">
          <section
            className="topologyApplyGroup"
            title="Required for apply. Select one saved plan, or All Unplanned to apply every enabled plan with at least one endpoint still in planned status."
          >
            <div className="topologyApplyGroupHeader">
              <strong>Apply target</strong>
              <small>Required</small>
            </div>
            <div className="dispatchControls">
              <label>
                <span>Plan</span>
                <select
                  aria-label="Network apply plan"
                  onChange={(event) => {
                    clearNetworkReview();
                    setSelectedPlanId(event.target.value);
                  }}
                  value={selectedPlanId}
                >
                  <option value={ALL_PLANNED_SCOPE}>All Unplanned ({plannedApplyPlans.length})</option>
                  {tunnelPlans.map((plan) => (
                    <option key={plan.id} value={plan.id}>
                      {plan.name}{plan.enabled ? "" : " (disabled)"}
                    </option>
                  ))}
                </select>
              </label>
              <label title="Maximum wall-clock job runtime sent to the backend for each reviewed network job.">
                <span>Max timeout</span>
                <input
                  aria-label="Network apply max timeout seconds"
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
            {bulkApplySelected ? (
              <div className="operationNote compactTopologyNote">
                <strong>All Unplanned ({plannedApplyPlans.length})</strong>
                <span title="Agents apply with their configured network backend.">
                  {plannedApplyPlans.length} plans / {vpsCountLabel(mutationTargets.length)}
                </span>
              </div>
            ) : endpoint ? (
              <div className="operationNote compactTopologyNote">
                <strong>{selectedPlan?.name ?? "Selected plan"}</strong>
                <span title={agentBackendHint(agents, selectedPlan)}>
                  {clientLabel(selectedPlan?.left_client_id ?? "")} / {clientLabel(selectedPlan?.right_client_id ?? "")}
                </span>
              </div>
            ) : null}
            <TargetImpactPreview
              forceUnprivileged={forceUnprivileged}
              mode="root_network_mutation"
              targets={mutationTargets}
              title="Network mutation impact"
            />
            <label
              className="checkLine"
              title="Try the apply or rollback as the agent user instead of requiring root privilege. Host changes may fail depending on local permissions."
            >
              <input
                aria-label="Force unprivileged network best effort"
                checked={forceUnprivileged}
                onChange={(event) => {
                  clearNetworkReview();
                  setForceUnprivileged(event.target.checked);
                }}
                type="checkbox"
              />
              <span>Unprivileged</span>
            </label>
          </section>

          <section className="topologyApplyGroup" title="Mutating network actions. All Unplanned is apply-only.">
            <div className="topologyApplyGroupHeader">
              <strong>Mutations</strong>
              <small>Mutating</small>
            </div>
            <div className="topologyApplyActionRow">
              <button
                className="primaryAction"
                disabled={
                  pending ||
                  networkSnapshot !== null ||
                  !privilegeMaterial ||
                  (bulkApplySelected
                    ? plannedApplyPlans.length === 0
                    : !selectedPlan || !endpoint || !selectedPlan.enabled)
                }
                type="submit"
              >
                <Play size={17} />
                Review apply
              </button>
              <button
                className="secondaryAction"
                disabled={pending || networkSnapshot !== null || bulkApplySelected || !selectedPlan || !endpoint || !privilegeMaterial}
                onClick={submitRollback}
                type="button"
              >
                <RotateCcw size={17} />
                Review rollback
              </button>
            </div>
          </section>

          <section className="topologyApplyGroup" title="Read-only checks for one selected endpoint side.">
            <div className="topologyApplyGroupHeader">
              <strong>Checks</strong>
              <small>Single endpoint</small>
            </div>
            <div className="dispatchControls">
              <label title="Single-endpoint checks run from this side of the selected plan.">
                <span>Endpoint</span>
                <select
                  aria-label="Network apply endpoint side"
                  disabled={bulkApplySelected}
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
            <div className="topologyApplyActionRow">
              <button
                className="secondaryAction"
                disabled={pending || networkSnapshot !== null || bulkApplySelected || !selectedPlan || !endpoint || !privilegeMaterial}
                onClick={submitStatus}
                type="button"
              >
                <Search size={17} />
                Review inspect
              </button>
              <button
                className="secondaryAction"
                disabled={
                  pending ||
                  networkSnapshot !== null ||
                  bulkApplySelected ||
                  !selectedPlan ||
                  !endpoint ||
                  !privilegeMaterial ||
                  !selectedPlan.enabled
                }
                onClick={submitProbe}
                type="button"
              >
                <Activity size={17} />
                Review probe
              </button>
            </div>
          </section>

          <section
            className="topologyApplyGroup"
            title="Speed tests are single-endpoint jobs and always require byte and rate safety caps."
          >
            <div className="topologyApplyGroupHeader">
              <strong>Speed test</strong>
              <small>Safety capped</small>
            </div>
            <div className="dispatchControls">
              <label title="Maximum speed-test duration.">
                <span>Duration</span>
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
                <span>Max MiB</span>
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
                <span>Rate Kbps</span>
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
                <span>Connect ms</span>
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
            <div className="topologyApplyActionRow">
              <button
                className="secondaryAction"
                disabled={
                  pending ||
                  networkSnapshot !== null ||
                  bulkApplySelected ||
                  !selectedPlan ||
                  !endpoint ||
                  !privilegeMaterial ||
                  !selectedPlan.enabled
                }
                onClick={submitSpeedTest}
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
              ? networkSnapshot.bulk
                ? "Apply all"
                : actionConfirmLabel(networkSnapshot.action)
              : "Run"
          }
          detail={networkSnapshot?.detail ?? ""}
          expiresAtUnix={networkSnapshot ? minSubmissionExpiry(networkSnapshot.submissions) : undefined}
          items={networkSnapshot?.items ?? []}
          onCancel={() => setNetworkSnapshot(null)}
          onConfirm={() => networkSnapshot && void submitNetworkChange(networkSnapshot)}
          open={networkSnapshot !== null}
          pending={pending}
          title={networkSnapshot ? `Confirm ${actionLabel(networkSnapshot.action).toLowerCase()}` : "Confirm network action"}
          tone={networkSnapshot && isMutation(networkSnapshot.action) ? "danger" : "normal"}
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

type NetworkAction = "apply" | "rollback" | "status" | "probe" | "speed_test";

type PlannedApplyPlan = {
  plan: TunnelPlanRecord;
};

type NetworkJobSubmission = {
  command: string;
  confirmed: boolean;
  destructive: boolean;
  forceUnprivileged: boolean;
  jobId: string;
  maxTimeoutSecs: number;
  operation: JobOperation;
  payloadHashHex: string;
  planName: string;
  privilegeAssertion: PrivilegeAssertion;
  selectorExpression: string;
  side: TunnelEndpointSide;
  targetClientIds: string[];
  targets: AgentView[];
};

type NetworkActionSnapshot = {
  action: NetworkAction;
  bulk: boolean;
  detail: string;
  forceUnprivileged: boolean;
  items: Array<{ label: string; value: string }>;
  submissions: NetworkJobSubmission[];
  targets: AgentView[];
};

function disabledPlanAllowsAction(mode: NetworkAction): boolean {
  return mode === "rollback" || mode === "status";
}

function commandName(mode: NetworkAction) {
  if (mode === "apply") {
    return "network_apply";
  }
  if (mode === "rollback") {
    return "network_rollback";
  }
  if (mode === "probe") {
    return "network_probe";
  }
  if (mode === "speed_test") {
    return "network_speed_test";
  }
  return "network_status";
}

function actionLabel(mode: NetworkAction) {
  if (mode === "apply") {
    return "Apply";
  }
  if (mode === "rollback") {
    return "Rollback";
  }
  if (mode === "probe") {
    return "Probe";
  }
  if (mode === "speed_test") {
    return "Speed test";
  }
  return "Status";
}

function actionConfirmLabel(mode: NetworkAction): string {
  if (mode === "apply") {
    return "Apply plan";
  }
  if (mode === "rollback") {
    return "Rollback plan";
  }
  if (mode === "probe") {
    return "Probe latency";
  }
  if (mode === "speed_test") {
    return "Run speed test";
  }
  return "Inspect side";
}

function isMutation(mode: NetworkAction) {
  return mode === "apply" || mode === "rollback";
}

function requiresConfirmation(mode: NetworkAction) {
  return isMutation(mode) || mode === "speed_test";
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function buildPlannedApplyPlans(tunnelPlans: TunnelPlanRecord[]): PlannedApplyPlan[] {
  return tunnelPlans
    .filter((plan) => plan.enabled && (plan.left_status === "planned" || plan.right_status === "planned"))
    .map((plan) => ({ plan }));
}

function uniqueClientIds(clientIds: string[]): string[] {
  return Array.from(new Set(clientIds));
}

function uniquePlanCount(submissions: NetworkJobSubmission[]): number {
  return new Set(submissions.map((submission) => submission.planName)).size;
}

function minSubmissionExpiry(submissions: NetworkJobSubmission[]): number | undefined {
  const expiries = submissions
    .map((submission) => submission.privilegeAssertion.expires_unix)
    .filter((value): value is number => typeof value === "number" && Number.isFinite(value));
  return expiries.length ? Math.min(...expiries) : undefined;
}

function planClientIds(plan: TunnelPlanRecord): string[] {
  return [plan.left_client_id, plan.right_client_id];
}

function planEndpointSides(_plan: TunnelPlanRecord): TunnelEndpointSide[] {
  return ["left", "right"];
}

function agentBackendHint(agents: AgentView[], plan: TunnelPlanRecord | null): string {
  if (!plan) {
    return "agent defaults";
  }
  const backendForClient = (clientId: string) =>
    agents.find((candidate) => candidate.id === clientId)?.capabilities.network_backend ?? "ifupdown";
  return `backend L ${backendForClient(plan.left_client_id)} / R ${backendForClient(plan.right_client_id)}`;
}
