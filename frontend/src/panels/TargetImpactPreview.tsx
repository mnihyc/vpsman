import { ShieldAlert, ShieldCheck, ShieldQuestion } from "lucide-react";
import { useState } from "react";
import { targetPreflightUnavailable } from "../bulkJobProgress";
import { usePanelDisplaySettings } from "../panelDisplay";
import type { AgentView } from "../types";
import { formatVpsName, type VpsNameDisplayMode } from "../utils";

export type TargetImpactMode =
  | "agent_update"
  | "generic"
  | "process_limits"
  | "restore"
  | "root_network_mutation";

type TargetImpactGroup = {
  key: "ready" | "needs_review" | "unavailable";
  label: string;
  agents: AgentView[];
};

type TargetImpactClassification =
  | "ready"
  | "stale"
  | "degraded"
  | "forced"
  | "observation_only"
  | "unavailable"
  | "unsupported";

export function TargetImpactPreview({
  emptyText = "Review or select targets to classify capability impact",
  forceUnprivileged = false,
  mode,
  targets,
  title = "Target impact",
}: {
  emptyText?: string;
  forceUnprivileged?: boolean;
  mode: TargetImpactMode;
  targets: AgentView[];
  title?: string;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const groups = buildTargetImpactGroups(targets, mode);
  const attentionCount = groups
    .filter((group) => group.key !== "ready")
    .reduce((count, group) => count + group.agents.length, 0);

  return (
    <section className="targetImpactPreview" aria-label={title}>
      <div className="targetImpactHeader">
        <strong>{title}</strong>
        <span>
          {targets.length === 0
            ? emptyText
            : `${targets.length} target${targets.length === 1 ? "" : "s"} / ${operationLabel(mode)}`}
        </span>
      </div>
      {targets.length > 0 && (
        <div className="targetImpactGrid">
          {groups.map((group) => (
            <div className={`targetImpactGroup ${group.key}`} key={group.key}>
              <div className="targetImpactGroupHeader">
                {impactIcon(group.key)}
                <strong>{group.agents.length}</strong>
                <span>{group.label}</span>
              </div>
              <TargetImpactChips agents={group.agents} mode={vpsNameDisplayMode} />
            </div>
          ))}
        </div>
      )}
      {attentionCount > 0 && (
        <p className="targetImpactHint">
          {forceUnprivileged
            ? "Forced targets will be dispatched as privilege-unlocked best effort."
            : "Non-ready targets selected."}
        </p>
      )}
    </section>
  );
}

export function targetImpactModeForDispatch(mode: string): TargetImpactMode {
  if (
    mode === "agent_update" ||
    mode === "agent_update_check" ||
    mode === "agent_update_activate" ||
    mode === "agent_update_rollback"
  ) {
    return "agent_update";
  }
  if (mode === "backup") {
    return "agent_update";
  }
  return "generic";
}

export function resolveAgentsById(agents: AgentView[], clientIds: string[]): AgentView[] {
  const byId = new Map(agents.map((agent) => [agent.id, agent]));
  return clientIds.map((clientId) => byId.get(clientId)).filter((agent): agent is AgentView => Boolean(agent));
}

function buildTargetImpactGroups(
  targets: AgentView[],
  mode: TargetImpactMode,
): TargetImpactGroup[] {
  const groups: Record<TargetImpactGroup["key"], AgentView[]> = {
    needs_review: [],
    ready: [],
    unavailable: [],
  };
  for (const target of targets) {
    const capability = classifyTarget(target, mode);
    if (capability === "ready") {
      groups.ready.push(target);
    } else if (capability === "unavailable" || capability === "unsupported") {
      groups.unavailable.push(target);
    } else {
      groups.needs_review.push(target);
    }
  }
  return [
    { key: "ready", label: "Ready", agents: groups.ready },
    { key: "needs_review", label: "Needs review", agents: groups.needs_review },
    { key: "unavailable", label: "Unavailable", agents: groups.unavailable },
  ];
}

function classifyTarget(
  target: AgentView,
  mode: TargetImpactMode,
): TargetImpactClassification {
  if (targetPreflightUnavailable(target)) {
    return "unavailable";
  }
  if (target.status === "stale") {
    return "stale";
  }
  if (mode === "generic") {
    return target.capabilities.privilege_mode === "unknown" ? "observation_only" : "ready";
  }
  if (target.capabilities.privilege_mode === "unknown") {
    return "observation_only";
  }
  if (mode === "root_network_mutation") {
    return target.capabilities.privilege_mode === "root" && target.capabilities.can_manage_runtime_tunnels
      ? "ready"
      : target.capabilities.can_attempt_privileged_ops
        ? "degraded"
        : "unsupported";
  }
  if (mode === "process_limits") {
    return target.capabilities.privilege_mode === "root" && target.capabilities.can_apply_process_limits
      ? "ready"
      : target.capabilities.can_attempt_privileged_ops
        ? "degraded"
        : "unsupported";
  }
  return target.capabilities.privilege_mode === "root" && target.capabilities.can_attempt_privileged_ops
    ? "ready"
    : target.capabilities.can_attempt_privileged_ops
      ? "degraded"
      : "unsupported";
}

function TargetImpactChips({ agents, mode }: { agents: AgentView[]; mode: VpsNameDisplayMode }) {
  const [expanded, setExpanded] = useState(false);
  if (agents.length === 0) {
    return <small>No targets</small>;
  }
  const visible = expanded ? agents : agents.slice(0, 20);
  const remaining = agents.length - visible.length;
  return (
    <div className="targetChipList impactTargetChips">
      {visible.map((agent) => (
        <span className="targetChip" key={agent.id} title={agent.id}>
          {formatVpsName(agent, mode)}
        </span>
      ))}
      {remaining > 0 && (
        <button
          className="targetChip mutedChip showMoreChip"
          onClick={() => setExpanded(true)}
          title={agents
            .slice(visible.length)
            .map((agent) => agent.id)
            .join("\n")}
          type="button"
        >
          Show {remaining} more
        </button>
      )}
    </div>
  );
}

function impactIcon(key: TargetImpactGroup["key"]) {
  if (key === "ready") {
    return <ShieldCheck size={16} />;
  }
  if (key === "unavailable") {
    return <ShieldQuestion size={16} />;
  }
  return <ShieldAlert size={16} />;
}

function operationLabel(mode: TargetImpactMode): string {
  if (mode === "agent_update") {
    return "agent update";
  }
  if (mode === "root_network_mutation") {
    return "network mutation";
  }
  if (mode === "process_limits") {
    return "process limits";
  }
  if (mode === "restore") {
    return "restore mutation";
  }
  return "standard dispatch";
}
