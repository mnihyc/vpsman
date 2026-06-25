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
import { useReviewGenerationGuard, waitForReviewRender } from "../../hooks/useReviewGenerationGuard";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeAssertion, type PrivilegeMaterial } from "../../privilege";
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

export function TopologyNetworkTestControls({
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
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [lastJob, setLastJob] = useState<CreateJobResponse | null>(null);
  const [lastAction, setLastAction] = useState<NetworkAction>("status");
  const [networkSnapshot, setNetworkSnapshot] = useState<NetworkActionSnapshot | null>(null);
  const [jobProgress, setJobProgress] = useState<BulkJobProgress | null>(null);
  const [lastJobProgress, setLastJobProgress] = useState<BulkJobProgress | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewPending, setReviewPending] = useState(false);
  const selectedPlan = tunnelPlans.find((plan) => plan.id === selectedPlanId) ?? tunnelPlans[0] ?? null;
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
  const endpoint = useMemo(
    () => (selectedPlan ? renderTunnelEndpointConfig(selectedPlan.plan, side) : null),
    [selectedPlan, side],
  );
  const planTargets = resolveAgentsById(agents, selectedPlan ? planClientIds(selectedPlan) : []);
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
        ? "Plan disabled; inspect only"
      : privilegeMaterial
        ? "Ready"
        : "Locked");

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
        if (!selectedPlan || !endpoint) {
          throw new Error("Select a tunnel plan");
        }
        if (!selectedPlan.enabled && mode !== "status") {
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
        const buildSubmission = async (
          planRecord: TunnelPlanRecord,
          planSide: TunnelEndpointSide,
        ): Promise<NetworkJobSubmission> => {
          const builtOperation =
            mode === "status"
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
            forceUnprivileged: false,
            operation: builtOperation.operation,
            privilegeMaterial,
            selectorExpression,
            maxTimeoutSecs: boundedMaxTimeoutSecs,
          });
          return {
            command: commandName(mode),
            confirmed: requiresConfirmation(mode),
            destructive: false,
            forceUnprivileged: false,
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
        const submissionCandidates = selectedPlan ? [{ plan: selectedPlan, side }] : [];
        const submissions = await Promise.all(
          submissionCandidates.map((candidate) => buildSubmission(candidate.plan, candidate.side)),
        );
        if (!submissions.length) {
          throw new Error("No tunnel plan is ready for testing");
        }
        if (!isReviewGenerationCurrent(reviewGeneration)) {
          return;
        }
        const snapshotTargets = resolveAgentsById(
          agents,
          uniqueClientIds(submissions.flatMap((submission) => submission.targetClientIds)),
        );
        const scopeLabel = mode === "speed_test" ? "Selected plan endpoints" : "Selected endpoint";
        const planLabel = submissions[0]?.planName ?? "unknown";
        setNetworkSnapshot({
          action: mode,
          detail: `${actionLabel(mode)} ${submissions[0]?.planName ?? "selected plan"} on ${vpsCountLabel(snapshotTargets.length)}.`,
          items: [
            { label: "Operation", value: actionLabel(mode) },
            { label: "Scope", value: scopeLabel },
            { label: "Targets", value: formatTargetAvailabilitySummary(snapshotTargets) },
            { label: "Plans", value: planLabel },
            { label: "Endpoint", value: submissions.length > 1 ? "Both endpoints" : side },
            { label: "Max timeout", value: `${boundedMaxTimeoutSecs}s` },
            { label: "Privilege unlock", value: "Unlocked locally" },
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
          <h2>Network tests</h2>
          <span>{status}</span>
        </div>
        <ShieldCheck size={20} />
      </div>
      <form className="dispatchForm topologyNetworkTestForm" onSubmit={(event) => event.preventDefault()}>
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
                      {plan.name}{plan.enabled ? "" : " (disabled)"}
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
                  {clientLabel(selectedPlan?.left_client_id ?? "")} / {clientLabel(selectedPlan?.right_client_id ?? "")}
                </span>
              </div>
            ) : null}
            <TargetImpactPreview
              mode="generic"
              targets={planTargets}
              title="Plan endpoint visibility"
            />
          </section>

          <section className="topologyNetworkTestGroup" title="Read-only checks for one selected endpoint side.">
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
                disabled={pending || networkSnapshot !== null || !selectedPlan || !endpoint || !privilegeMaterial}
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
            className="topologyNetworkTestGroup"
            title="Speed tests are single-endpoint jobs and always require byte and rate safety caps."
          >
            <div className="topologyNetworkTestGroupHeader">
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
                type="button"
              >
                <Activity size={17} />
                Review speed test
              </button>
            </div>
          </section>
        </div>
        <ConfirmationPrompt
          confirmLabel={networkSnapshot ? actionConfirmLabel(networkSnapshot.action) : "Run network test"}
          detail={networkSnapshot?.detail ?? ""}
          expiresAtUnix={networkSnapshot ? minSubmissionExpiry(networkSnapshot.submissions) : undefined}
          items={networkSnapshot?.items ?? []}
          onCancel={() => setNetworkSnapshot(null)}
          onConfirm={() => networkSnapshot && void submitNetworkChange(networkSnapshot)}
          open={networkSnapshot !== null}
          pending={pending}
          title={networkSnapshot ? `Confirm ${actionLabel(networkSnapshot.action).toLowerCase()}` : "Confirm network action"}
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

type NetworkAction = "status" | "probe" | "speed_test";

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

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}

function uniqueClientIds(clientIds: string[]): string[] {
  return Array.from(new Set(clientIds));
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

function agentBackendHint(agents: AgentView[], plan: TunnelPlanRecord | null): string {
  if (!plan) {
    return "agent defaults";
  }
  const backendForClient = (clientId: string) =>
    agents.find((candidate) => candidate.id === clientId)?.capabilities.network_backend ?? "ifupdown";
  return `backend L ${backendForClient(plan.left_client_id)} / R ${backendForClient(plan.right_client_id)}`;
}
