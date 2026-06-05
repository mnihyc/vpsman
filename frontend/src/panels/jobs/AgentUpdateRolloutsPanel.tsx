import { useState } from "react";
import { KeyRound, PackageCheck, PauseCircle, PlayCircle, ShieldCheck } from "lucide-react";
import { CrudPager } from "../../components/CrudPager";
import type {
  AgentUpdateRolloutControlRequest,
  AgentUpdateRolloutPolicyRecord,
  AgentUpdateRolloutRecord,
  CreateAgentUpdateRolloutPolicyRequest,
} from "../../types";
import type { ProofMaterial } from "../../proof";
import { formatTime, shortHash, shortId, statusClass } from "../../utils";

type DelegationSummary = {
  target_count: number;
  ready_count: number;
  dispatching_count: number;
  dispatched_count: number;
  expired_count: number;
  failed_count: number;
  proof_expires_unix_min: number | null;
  proof_expires_unix_max: number | null;
  force_unprivileged?: boolean;
  updated_at: string;
};

function latestDelegation<T extends DelegationSummary>(summaries: T[] | undefined): T | null {
  if (!summaries || summaries.length === 0) {
    return null;
  }
  return [...summaries].sort((left, right) => right.updated_at.localeCompare(left.updated_at))[0];
}

function delegationStatus(summary: DelegationSummary | null): string {
  if (!summary) {
    return "none";
  }
  if (summary.ready_count > 0) {
    return `${summary.ready_count}/${summary.target_count} ready`;
  }
  if (summary.dispatching_count > 0) {
    return `${summary.dispatching_count}/${summary.target_count} dispatching`;
  }
  if (summary.dispatched_count > 0) {
    return `${summary.dispatched_count}/${summary.target_count} dispatched`;
  }
  if (summary.expired_count > 0) {
    return `${summary.expired_count}/${summary.target_count} expired`;
  }
  if (summary.failed_count > 0) {
    return `${summary.failed_count}/${summary.target_count} failed`;
  }
  return `${summary.target_count} recorded`;
}

function delegationTitle(summary: DelegationSummary | null): string | undefined {
  if (!summary) {
    return undefined;
  }
  const expiry =
    summary.proof_expires_unix_min === null
      ? "no expiry"
      : new Date(summary.proof_expires_unix_min * 1000).toLocaleString();
  const policy = summary.force_unprivileged ? "; forced unprivileged attempt" : "";
  return `Ready ${summary.ready_count}, dispatching ${summary.dispatching_count}, dispatched ${summary.dispatched_count}, expired ${summary.expired_count}, failed ${summary.failed_count}; earliest expiry ${expiry}${policy}`;
}

function delegationActionLabel(summary: DelegationSummary | null, delegateLabel: string, renewLabel: string): string {
  if (summary && summary.ready_count === 0 && (summary.expired_count > 0 || summary.failed_count > 0)) {
    return renewLabel;
  }
  return delegateLabel;
}

function canOneClickAdvance(
  rollout: AgentUpdateRolloutRecord,
  proofMaterial: ProofMaterial | null,
): boolean {
  const targets = rollout.targets ?? [];
  const hasCompletedTargets = targets.some((target) => target.status === "completed");
  return hasCompletedTargets && Boolean(proofMaterial);
}

export function AgentUpdateRolloutsPanel({
  actionError,
  actionId,
  actionPending,
  batchSize,
  loading,
  onActivateBatch,
  onControlRollout,
  onCreatePolicy,
  onDelegateActivation,
  onDelegateRollback,
  onForceUnprivilegedChange,
  onRefresh,
  onRollbackTargets,
  onRestartAgentChange,
  onBatchSizeChange,
  onProofTtlSecsChange,
  proofMaterial,
  proofTtlSecs,
  forceUnprivileged,
  policies,
  restartAgent,
  rollouts,
}: {
  actionError: string | null;
  actionId: string | null;
  actionPending: boolean;
  batchSize: number;
  loading: boolean;
  onActivateBatch: (rollout: AgentUpdateRolloutRecord) => void;
  onControlRollout: (rollout: AgentUpdateRolloutRecord, request: AgentUpdateRolloutControlRequest) => void;
  onCreatePolicy: (request: CreateAgentUpdateRolloutPolicyRequest) => Promise<AgentUpdateRolloutPolicyRecord>;
  onDelegateActivation: (rollout: AgentUpdateRolloutRecord) => void;
  onDelegateRollback: (rollout: AgentUpdateRolloutRecord) => void;
  onForceUnprivilegedChange: (value: boolean) => void;
  onRefresh: () => void;
  onRollbackTargets: (rollout: AgentUpdateRolloutRecord) => void;
  onBatchSizeChange: (value: number) => void;
  onProofTtlSecsChange: (value: number) => void;
  onRestartAgentChange: (value: boolean) => void;
  proofMaterial: ProofMaterial | null;
  proofTtlSecs: number;
  forceUnprivileged: boolean;
  policies: AgentUpdateRolloutPolicyRecord[];
  restartAgent: boolean;
  rollouts: AgentUpdateRolloutRecord[];
}) {
  const [policyName, setPolicyName] = useState("stable-default");
  const [policyScopeKind, setPolicyScopeKind] = useState<CreateAgentUpdateRolloutPolicyRequest["scope_kind"]>("global");
  const [policyScopeValue, setPolicyScopeValue] = useState("");
  const [policyChannel, setPolicyChannel] = useState("stable");
  const [policyCanaryCount, setPolicyCanaryCount] = useState("1");
  const [policyHealthGate, setPolicyHealthGate] = useState("heartbeat_verified");
  const [policyPriority, setPolicyPriority] = useState("0");
  const [policyEnabled, setPolicyEnabled] = useState(true);
  const [policyPending, setPolicyPending] = useState(false);
  const [policyError, setPolicyError] = useState<string | null>(null);
  async function submitPolicy() {
    setPolicyPending(true);
    setPolicyError(null);
    try {
      const canary = policyCanaryCount.trim() === "" ? null : Number(policyCanaryCount);
      await onCreatePolicy({
        name: policyName.trim(),
        scope_kind: policyScopeKind,
        scope_value: policyScopeKind === "global" ? null : policyScopeValue.trim(),
        channel: policyChannel.trim() || null,
        canary_count: Number.isFinite(canary) ? canary : null,
        automation_health_gate: policyHealthGate || null,
        priority: Number(policyPriority) || 0,
        enabled: policyEnabled,
        confirmed: true,
      });
    } catch (error) {
      setPolicyError(error instanceof Error ? error.message : "Policy update failed");
    } finally {
      setPolicyPending(false);
    }
  }

  return (
    <div className="fleetPanel">
      <div className="sectionHeader">
        <div>
          <h2>Agent update rollouts</h2>
          <span>{actionError ?? `${rollouts.length} staged rollout records`}</span>
        </div>
        <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
          Refresh
        </button>
      </div>
      <div className="rolloutPolicyPanel">
        <div className="sectionSubheader">
          <h3>Rollout policy presets</h3>
          <span>{policyError ?? `${policies.length} active presets`}</span>
        </div>
        <div className="rolloutPolicyForm">
          <label>
            <span>Name</span>
            <input value={policyName} onChange={(event) => setPolicyName(event.target.value)} />
          </label>
          <label>
            <span>Scope</span>
            <select
              value={policyScopeKind}
              onChange={(event) => setPolicyScopeKind(event.target.value as CreateAgentUpdateRolloutPolicyRequest["scope_kind"])}
            >
              <option value="global">Global</option>
              <option value="tag">Tag</option>
              <option value="provider">Provider</option>
            </select>
          </label>
          <label>
            <span>Scope value</span>
            <input
              disabled={policyScopeKind === "global"}
              value={policyScopeKind === "global" ? "" : policyScopeValue}
              onChange={(event) => setPolicyScopeValue(event.target.value)}
            />
          </label>
          <label>
            <span>Channel</span>
            <input value={policyChannel} onChange={(event) => setPolicyChannel(event.target.value)} />
          </label>
          <label>
            <span>Canary</span>
            <input
              max={10000}
              min={0}
              type="number"
              value={policyCanaryCount}
              onChange={(event) => setPolicyCanaryCount(event.target.value)}
            />
          </label>
          <label>
            <span>Gate</span>
            <select value={policyHealthGate} onChange={(event) => setPolicyHealthGate(event.target.value)}>
              <option value="heartbeat_verified">Heartbeat</option>
              <option value="manual_after_canary">Manual after canary</option>
              <option value="manual_only">Manual only</option>
            </select>
          </label>
          <label>
            <span>Priority</span>
            <input value={policyPriority} onChange={(event) => setPolicyPriority(event.target.value)} type="number" />
          </label>
          <label className="checkLine inlineCheck">
            <input checked={policyEnabled} onChange={(event) => setPolicyEnabled(event.target.checked)} type="checkbox" />
            <span>Enabled</span>
          </label>
          <button className="primaryAction" disabled={policyPending || policyName.trim() === ""} onClick={() => void submitPolicy()} type="button">
            {policyPending ? "Saving" : "Save preset"}
          </button>
        </div>
        <div className="rolloutPolicyList">
          {policies.slice(0, 6).map((policy) => (
            <span className="policyChip" key={policy.id} title={policy.notes ?? undefined}>
              <strong>{policy.name}</strong>
              {policy.scope_kind}
              {policy.scope_value ? `:${policy.scope_value}` : ""}
              {policy.channel ? `/${policy.channel}` : ""}
              {policy.canary_count !== null ? ` c${policy.canary_count}` : ""}
              {policy.automation_health_gate ? ` ${policy.automation_health_gate}` : ""}
            </span>
          ))}
        </div>
      </div>
      <div className="approvalControls rolloutControls">
        <label>
          <span>Batch</span>
          <input
            aria-label="Rollout activation batch size"
            max={10000}
            min={1}
            onChange={(event) => onBatchSizeChange(Number(event.target.value))}
            type="number"
            value={batchSize}
          />
        </label>
        <label>
          <span>Proof TTL</span>
          <input
            aria-label="Rollout proof TTL seconds"
            max={3600}
            min={15}
            onChange={(event) => onProofTtlSecsChange(Number(event.target.value))}
            type="number"
            value={proofTtlSecs}
          />
        </label>
        <label className="checkLine inlineCheck">
          <input checked={restartAgent} onChange={(event) => onRestartAgentChange(event.target.checked)} type="checkbox" />
          <span>Restart agent after activation</span>
        </label>
        <label className="checkLine inlineCheck">
          <input checked={forceUnprivileged} onChange={(event) => onForceUnprivilegedChange(event.target.checked)} type="checkbox" />
          <span>Force unprivileged attempt</span>
        </label>
      </div>
      <CrudPager
        fields={[
          { label: "Rollout", value: (rollout) => `${rollout.id} ${rollout.job_id}` },
          { label: "Status", value: (rollout) => rollout.status },
          { label: "Policy", value: (rollout) => `${rollout.activation_policy} ${rollout.rollout_policy_name ?? ""}` },
          { label: "Next", value: (rollout) => `${rollout.automation_next_action ?? ""} ${rollout.automation_status ?? ""}` },
          { label: "Artifact", value: (rollout) => rollout.artifact_sha256_hex },
        ]}
        itemLabel="rollouts"
        items={rollouts}
        pageSize={6}
        title="Rollout records"
        empty={
          <div className="emptyState">
            <ShieldCheck size={22} />
            <strong>No rollout records</strong>
            <span>Proof-gated agent-update dispatches create staged rollout records here.</span>
          </div>
        }
      >
        {(rolloutRows) => (
          <div className="table historyTable">
            <div className="historyRow heading rolloutGrid">
              <span>Rollout</span>
              <span>Status</span>
              <span>Targets</span>
              <span>Policy / Next</span>
              <span>Artifact</span>
              <span>Updated</span>
              <span>Actions</span>
            </div>
            {rolloutRows.map((rollout) => {
              const automationTargets = rollout.automation_targets ?? [];
              const rolloutTargets = rollout.targets ?? [];
              const activationDelegation = latestDelegation(rollout.activation_delegations);
              const rollbackDelegation = latestDelegation(rollout.rollback_delegations);
              return (
                <div className="historyRow rolloutGrid" key={rollout.id}>
              <span className="historyPrimary">
                <strong>{shortId(rollout.job_id)}</strong>
                <small>{shortId(rollout.id)}</small>
              </span>
              <span className={`status ${statusClass(rollout.status)}`}>{rollout.status}</span>
              <span>
                {rollout.completed_count}/{rollout.target_count}
                {rollout.failed_count > 0 ? `, ${rollout.failed_count} failed` : ""}
                {rollout.pending_count > 0 ? `, ${rollout.pending_count} pending` : ""}
              </span>
              <span className="historyPrimary">
                <strong>{rollout.activation_policy}</strong>
                <small>
                  {rollout.canary_count > 0 ? `${rollout.canary_count} canary, ` : ""}
                  {rollout.heartbeat_timeout_secs ? `${rollout.heartbeat_timeout_secs}s heartbeat` : "default heartbeat"}
                </small>
                <small>{rollout.rollout_policy_name ? `policy ${rollout.rollout_policy_name}` : "no policy preset"}</small>
                <small title={rollout.automation_blocker ?? undefined}>
                  {rollout.automation_next_action ?? rollout.automation_status ?? "manual"}
                  {automationTargets.length > 0 ? `, ${automationTargets.length} target` : ""}
                </small>
                <small>
                  {rollout.automation_paused ? "paused, " : ""}
                  gate {rollout.automation_health_gate}
                  {rollout.automation_lease_owner ? `, leased by ${rollout.automation_lease_owner}` : ""}
                </small>
                <small title={delegationTitle(activationDelegation)}>
                  act proof {delegationStatus(activationDelegation)}
                  {activationDelegation?.force_unprivileged ? ", forced" : ""}
                </small>
                <small title={delegationTitle(rollbackDelegation)}>
                  roll proof {delegationStatus(rollbackDelegation)}
                  {rollbackDelegation?.force_unprivileged ? ", forced" : ""}
                </small>
              </span>
              <span className="monoValue">{shortHash(rollout.artifact_sha256_hex)}</span>
              <span>{formatTime(rollout.updated_at)}</span>
              <span className="rowActions">
                <button
                  className="secondaryAction compactAction"
                  disabled={actionPending || !proofMaterial || !rolloutTargets.some((target) => target.status === "completed")}
                  onClick={() => onActivateBatch(rollout)}
                  type="button"
                >
                  <PackageCheck size={14} />
                  <span>{actionId === rollout.id ? "Working" : "Activate"}</span>
                </button>
                <button
                  className="secondaryAction compactAction"
                  disabled={actionPending || !canOneClickAdvance(rollout, proofMaterial)}
                  onClick={() => {
                    if ((activationDelegation?.ready_count ?? 0) > 0 && proofMaterial) {
                      onActivateBatch(rollout);
                    } else {
                      onDelegateActivation(rollout);
                    }
                  }}
                  title="Advance this rollout using delegated proof when available, or record activation proof first"
                  type="button"
                >
                  <PackageCheck size={14} />
                  <span>Advance</span>
                </button>
                <button
                  className="secondaryAction compactAction"
                  disabled={
                    actionPending ||
                    !proofMaterial ||
                    !rolloutTargets.some(
                      (target) =>
                        target.status === "activation_pending_restart" ||
                        target.status === "activation_failed" ||
                        target.status === "heartbeat_timeout" ||
                        target.status === "heartbeat_verified",
                    )
                  }
                  onClick={() => onRollbackTargets(rollout)}
                  type="button"
                >
                  <ShieldCheck size={14} />
                  <span>Rollback</span>
                </button>
                <button
                  className="secondaryAction compactAction"
                  disabled={actionPending || !proofMaterial || rolloutTargets.length === 0}
                  onClick={() => onDelegateActivation(rollout)}
                  title="Delegate activation proof escrow"
                  type="button"
                >
                  <KeyRound size={14} />
                  <span>{delegationActionLabel(activationDelegation, "Delegate act.", "Renew act.")}</span>
                </button>
                <button
                  className="secondaryAction compactAction"
                  disabled={actionPending || !proofMaterial || rolloutTargets.length === 0}
                  onClick={() => onDelegateRollback(rollout)}
                  title="Delegate rollback proof escrow"
                  type="button"
                >
                  <KeyRound size={14} />
                  <span>{delegationActionLabel(rollbackDelegation, "Delegate roll.", "Renew roll.")}</span>
                </button>
                <select
                  aria-label={`Rollout ${shortId(rollout.id)} health gate`}
                  className="compactSelect"
                  disabled={actionPending}
                  onChange={(event) =>
                    onControlRollout(rollout, {
                      confirmed: true,
                      automation_health_gate: event.target.value,
                    })
                  }
                  value={rollout.automation_health_gate}
                >
                  <option value="heartbeat_verified">Heartbeat gate</option>
                  <option value="manual_after_canary">Manual after canary</option>
                  <option value="manual_only">Manual only</option>
                </select>
                <button
                  className="secondaryAction compactAction"
                  disabled={actionPending}
                  onClick={() =>
                    onControlRollout(rollout, {
                      confirmed: true,
                      paused: !rollout.automation_paused,
                      pause_reason: rollout.automation_paused ? null : "operator paused from panel",
                    })
                  }
                  type="button"
                >
                  {rollout.automation_paused ? <PlayCircle size={14} /> : <PauseCircle size={14} />}
                  <span>{rollout.automation_paused ? "Resume" : "Pause"}</span>
                </button>
              </span>
                </div>
              );
            })}
          </div>
        )}
      </CrudPager>
    </div>
  );
}
