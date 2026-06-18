import { useMemo, useState, type FormEvent } from "react";
import { Activity, Play, RotateCcw, Search, ShieldCheck } from "lucide-react";
import {
  buildBulkJobProgress,
  bulkProgressTimeoutMs,
  createJobTargetCount,
  formatTargetAvailabilitySummary,
  waitForBulkJobTargets,
  type BulkJobProgress,
} from "../../bulkJobProgress";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { ExecutionResultPanel } from "../../components/ExecutionResultPanel";
import { PrivilegeVaultBox } from "../../components/PrivilegeVaultBox";
import { usePanelDisplaySettings } from "../../panelDisplay";
import { buildPrivilegeForJobOperation, type PrivilegeAssertion, type PrivilegeMaterial } from "../../privilege";
import { selectorExpressionForClientIds } from "../../searchExpression";
import { networkBackendPresetLabel } from "../../presets/networkBackendPresets";
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
  TunnelConfigBackend,
  TunnelEndpointSide,
  TunnelPlanRecord,
} from "../../types";
import { clientDisplayNameFromMap, clientDisplayNameMap, runPanelAction, shortId } from "../../utils";
import { clampInteger } from "../jobDispatchModel";
import { resolveAgentsById, TargetImpactPreview } from "../TargetImpactPreview";

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
  const [selectedPlanId, setSelectedPlanId] = useState(() => tunnelPlans[0]?.id ?? "");
  const [side, setSide] = useState<TunnelEndpointSide>("left");
  const [backend, setBackend] = useState<TunnelConfigBackend>("ifupdown");
  const [timeoutSecs, setTimeoutSecs] = useState(60);
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
  const selectedPlan = tunnelPlans.find((plan) => plan.id === selectedPlanId) ?? tunnelPlans[0] ?? null;
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
  const endpoint = useMemo(
    () => (selectedPlan ? renderTunnelEndpointConfig(selectedPlan.plan, side) : null),
    [selectedPlan, side],
  );
  const mutationTargets = resolveAgentsById(agents, endpoint ? [endpoint.localClientId] : []);
  const visibleJobProgress = jobProgress ?? lastJobProgress;
  const status =
    actionError ??
    (visibleJobProgress
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

  async function openNetworkPrompt(mode: NetworkAction) {
    setActionError(null);
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedPlan || !endpoint) {
        throw new Error("Select a tunnel plan");
      }
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      if (!selectedPlan.enabled && !disabledPlanAllowsAction(mode)) {
        throw new Error("Tunnel plan is disabled");
      }
      const boundedProbeCount = clampInteger(probeCount, 1, 20);
      const boundedProbeIntervalMs = clampInteger(probeIntervalMs, 200, 10_000);
      const boundedSpeedDurationSecs = clampInteger(speedDurationSecs, 1, 30);
      const boundedSpeedMaxBytes = clampInteger(speedMaxBytesMiB, 1, 256) * 1024 * 1024;
      const boundedSpeedRateLimitKbps = clampInteger(speedRateLimitKbps, 64, 1_000_000);
      const boundedSpeedPort = clampInteger(speedPort, 1024, 65_535);
      const boundedSpeedConnectTimeoutMs = clampInteger(speedConnectTimeoutMs, 100, 30_000);
      const builtOperation =
        mode === "apply"
          ? await buildNetworkApplyOperation(selectedPlan.plan, side, backend)
          : mode === "rollback"
            ? buildNetworkRollbackOperation(selectedPlan.plan, side)
            : mode === "status"
              ? buildNetworkStatusOperation(selectedPlan.plan, side)
              : mode === "probe"
                ? buildNetworkProbeOperation(selectedPlan.plan, side, boundedProbeCount, boundedProbeIntervalMs)
                : buildNetworkSpeedTestOperation(
                    selectedPlan.plan,
                    side,
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
      const targets = resolveAgentsById(agents, targetClientIds);
      const selectorExpression = selectorExpressionForClientIds(targetClientIds);
      const boundedTimeoutSecs = clampInteger(timeoutSecs, 1, 3600);
      const boundedForceUnprivileged = isMutation(mode) ? forceUnprivileged : false;
      const builtPrivilege = await buildPrivilegeForJobOperation({
        clientIds: targetClientIds,
        commandType: commandName(mode),
        forceUnprivileged: boundedForceUnprivileged,
        operation: builtOperation.operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: boundedTimeoutSecs,
      });
      setNetworkSnapshot({
        action: mode,
        command: commandName(mode),
        confirmed: isMutation(mode),
        destructive: isMutation(mode),
        detail: `${actionLabel(mode)} ${selectedPlan.name} on ${vpsCountLabel(targets.length)}.`,
        forceUnprivileged: boundedForceUnprivileged,
        items: [
          { label: "Operation", value: actionLabel(mode) },
          { label: "Selector", value: selectorExpression },
          { label: "Targets", value: formatTargetAvailabilitySummary(targets) },
          { label: "Plan", value: selectedPlan.name },
          { label: "Endpoint", value: side },
          ...(mode === "apply" ? [{ label: "Backend", value: backendLabel(backend) }] : []),
          { label: "Timeout", value: `${boundedTimeoutSecs}s` },
          { label: "Privilege unlock", value: "Unlocked locally" },
          ...(isMutation(mode)
            ? [{ label: "Privilege", value: boundedForceUnprivileged ? "Forced best effort" : "Root required" }]
            : []),
        ],
        operation: builtOperation.operation,
        payloadHashHex: builtPrivilege.payloadHashHex,
        privilegeAssertion: builtPrivilege.privilegeAssertion,
        selectorExpression,
        targetClientIds,
        targets,
        timeoutSecs: boundedTimeoutSecs,
      });
    });
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
      const job = await onCreateJob({
        argv: [],
        selector_expression: snapshot.selectorExpression,
        target_client_ids: snapshot.targetClientIds,
        command: snapshot.command,
        confirmed: snapshot.confirmed,
        destructive: snapshot.destructive,
        operation: snapshot.operation,
        force_unprivileged: snapshot.forceUnprivileged,
        privileged: true,
        privilege_assertion: snapshot.privilegeAssertion,
        timeout_secs: snapshot.timeoutSecs,
      });
      setLastPayloadHash(snapshot.payloadHashHex);
      setLastJob(job);
      setLastAction(snapshot.action);
      await trackNetworkProgress(job, snapshot.targets, snapshot.timeoutSecs);
    });
  }

  async function trackNetworkProgress(job: CreateJobResponse, targets: AgentView[], timeoutSecsForSnapshot: number) {
    const targetCount = createJobTargetCount(job);
    setLastJobProgress(null);
    setJobProgress(buildBulkJobProgress({
      jobId: job.job_id,
      targetCount,
      targetRecords: [],
      targets,
    }));
    try {
      const result = await waitForBulkJobTargets(job.job_id, onLoadTargets, {
        onProgress: setJobProgress,
        targetCount,
        targets,
        timeoutMs: bulkProgressTimeoutMs(timeoutSecsForSnapshot),
      });
      setLastJobProgress(result.progress);
      if (result.timedOut) {
        throw new Error("Timed out waiting for network apply targets");
      }
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
      <form className="dispatchForm" onSubmit={submitApply}>
        <div className="dispatchControls">
          <label>
            <span>Apply plan</span>
            <select
              aria-label="Network apply plan"
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSelectedPlanId(event.target.value);
              }}
              value={selectedPlan?.id ?? ""}
            >
              {tunnelPlans.map((plan) => (
                <option key={plan.id} value={plan.id}>
                  {plan.name}{plan.enabled ? "" : " (disabled)"}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Endpoint side</span>
            <select
              aria-label="Network apply endpoint side"
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSide(event.target.value as TunnelEndpointSide);
              }}
              value={side}
            >
              <option value="left">Left endpoint</option>
              <option value="right">Right endpoint</option>
            </select>
          </label>
          <label>
            <span>Network backend</span>
            <select
              aria-label="Network apply backend"
              onChange={(event) => {
                setNetworkSnapshot(null);
                setBackend(event.target.value as TunnelConfigBackend);
              }}
              value={backend}
            >
              <option value="ifupdown">ifupdown</option>
              <option value="netplan">netplan</option>
              <option value="systemd_networkd">systemd-networkd</option>
            </select>
          </label>
        </div>
        <div className="dispatchControls">
          <label>
            <span>Timeout seconds</span>
            <input
              aria-label="Network apply timeout seconds"
              max={3600}
              min={1}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setTimeoutSecs(Number(event.target.value));
              }}
              type="number"
              value={timeoutSecs}
            />
          </label>
        </div>
        <div className="dispatchControls">
          <label>
            <span>Probe count</span>
            <input
              aria-label="Network probe count"
              max={20}
              min={1}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setProbeCount(Number(event.target.value));
              }}
              type="number"
              value={probeCount}
            />
          </label>
          <label>
            <span>Probe interval ms</span>
            <input
              aria-label="Network probe interval milliseconds"
              max={10_000}
              min={200}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setProbeIntervalMs(Number(event.target.value));
              }}
              type="number"
              value={probeIntervalMs}
            />
          </label>
        </div>
        <div className="dispatchControls">
          <label>
            <span>Speed duration seconds</span>
            <input
              aria-label="Network speed test duration seconds"
              max={30}
              min={1}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSpeedDurationSecs(Number(event.target.value));
              }}
              type="number"
              value={speedDurationSecs}
            />
          </label>
          <label>
            <span>Speed cap MiB</span>
            <input
              aria-label="Network speed test max mebibytes"
              max={256}
              min={1}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSpeedMaxBytesMiB(Number(event.target.value));
              }}
              type="number"
              value={speedMaxBytesMiB}
            />
          </label>
        </div>
        <div className="dispatchControls">
          <label>
            <span>Rate limit Kbps</span>
            <input
              aria-label="Network speed test rate limit Kbps"
              max={1_000_000}
              min={64}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSpeedRateLimitKbps(Number(event.target.value));
              }}
              type="number"
              value={speedRateLimitKbps}
            />
          </label>
          <label>
            <span>TCP port</span>
            <input
              aria-label="Network speed test TCP port"
              max={65_535}
              min={1024}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSpeedPort(Number(event.target.value));
              }}
              type="number"
              value={speedPort}
            />
          </label>
        </div>
        <div className="dispatchControls">
          <label>
            <span>Connect timeout ms</span>
            <input
              aria-label="Network speed test connect timeout milliseconds"
              max={30_000}
              min={100}
              onChange={(event) => {
                setNetworkSnapshot(null);
                setSpeedConnectTimeoutMs(Number(event.target.value));
              }}
              type="number"
              value={speedConnectTimeoutMs}
            />
          </label>
        </div>
        {endpoint && (
          <div className="operationNote">
            <strong>{clientLabel(endpoint.localClientId)}</strong>
            <span>
              {backendLabel(backend)} / {selectedPlan?.plan.bird2_file}
            </span>
          </div>
        )}
        <TargetImpactPreview
          forceUnprivileged={forceUnprivileged}
          mode="root_network_mutation"
          targets={mutationTargets}
          title="Network mutation impact"
        />
        <label className="checkLine">
          <input
            aria-label="Force unprivileged network best effort"
            checked={forceUnprivileged}
            onChange={(event) => {
              setNetworkSnapshot(null);
              setForceUnprivileged(event.target.checked);
            }}
            type="checkbox"
          />
          <span>Force unprivileged best effort</span>
        </label>
        <ConfirmationPrompt
          confirmLabel={networkSnapshot ? actionConfirmLabel(networkSnapshot.action) : "Run"}
          detail={networkSnapshot?.detail ?? ""}
          expiresAtUnix={networkSnapshot?.privilegeAssertion.expires_unix}
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
        <div className="dispatchActions">
          <button
            className="primaryAction"
            disabled={pending || networkSnapshot !== null || !selectedPlan || !endpoint || !privilegeMaterial || !selectedPlan.enabled}
            type="submit"
          >
            <Play size={17} />
            Review apply
          </button>
          <button
            className="secondaryAction"
            disabled={pending || networkSnapshot !== null || !selectedPlan || !endpoint || !privilegeMaterial}
            onClick={submitRollback}
            type="button"
          >
            <RotateCcw size={17} />
            Review rollback
          </button>
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
            disabled={pending || networkSnapshot !== null || !selectedPlan || !endpoint || !privilegeMaterial || !selectedPlan.enabled}
            onClick={submitProbe}
            type="button"
          >
            <Activity size={17} />
            Review probe
          </button>
          <button
            className="secondaryAction"
            disabled={pending || networkSnapshot !== null || !selectedPlan || !endpoint || !privilegeMaterial || !selectedPlan.enabled}
            onClick={submitSpeedTest}
            type="button"
          >
            <Activity size={17} />
            Review speed test
          </button>
        </div>
      </form>
      <PrivilegeVaultBox
        lastPayloadHash={lastPayloadHash}
        onOpenUnlock={onOpenPrivilegeUnlock}
        onPrivilegeMaterialChange={(material) => {
          setNetworkSnapshot(null);
          setPrivilegeMaterial(material);
        }}
        privilegeMaterial={privilegeMaterial}
      />
    </section>
  );
}

type NetworkAction = "apply" | "rollback" | "status" | "probe" | "speed_test";

type NetworkActionSnapshot = {
  action: NetworkAction;
  command: string;
  confirmed: boolean;
  destructive: boolean;
  detail: string;
  forceUnprivileged: boolean;
  items: Array<{ label: string; value: string }>;
  operation: JobOperation;
  payloadHashHex: string;
  privilegeAssertion: PrivilegeAssertion;
  selectorExpression: string;
  targetClientIds: string[];
  targets: AgentView[];
  timeoutSecs: number;
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
    return "Apply side";
  }
  if (mode === "rollback") {
    return "Rollback side";
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

function backendLabel(backend: TunnelConfigBackend) {
  return networkBackendPresetLabel(backend);
}

function vpsCountLabel(count: number): string {
  return `${count} VPS${count === 1 ? "" : "s"}`;
}
