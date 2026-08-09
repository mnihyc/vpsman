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

const COLLAPSED_TARGET_CHIP_LIMIT = 8;

export function TargetImpactPreview({
  emptyText = "Review or select targets to classify capability impact",
  forceUnprivileged = false,
  mode,
  targets,
  title = "Target impact",
  unavailableLabel = "Unavailable",
}: {
  emptyText?: string;
  forceUnprivileged?: boolean;
  mode: TargetImpactMode;
  targets: AgentView[];
  title?: string;
  unavailableLabel?: string;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const groups = buildTargetImpactGroups(targets, mode);
  const attentionCount = groups
    .filter((group) => group.key !== "ready")
    .reduce((count, group) => count + group.agents.length, 0);

  return (
    <section
      className="targetImpactPreview"
      aria-label={title}
      title={`${title}: ${
        targets.length === 0
          ? emptyText
          : `${targets.length} target${targets.length === 1 ? "" : "s"} for ${operationLabel(mode)}`
      }`}
    >
      <div
        className="targetImpactHeader"
        title="Capability preflight for the exact selected VPS set"
      >
        <strong title={title}>{title}</strong>
        <span
          title={
            targets.length === 0
              ? emptyText
              : `${targets.length} selected targets for ${operationLabel(mode)}`
          }
        >
          {targets.length === 0
            ? emptyText
            : `${targets.length} target${targets.length === 1 ? "" : "s"} / ${operationLabel(mode)}`}
        </span>
      </div>
      {targets.length > 0 && (
        <div className="targetImpactGrid">
          {groups.map((group) => (
            <div
              className={`targetImpactGroup ${group.key}`}
              key={group.key}
              title={impactGroupTitle(group, mode, unavailableLabel)}
            >
              <div
                className="targetImpactGroupHeader"
                title={impactGroupTitle(group, mode, unavailableLabel)}
              >
                {impactIcon(group.key)}
                <strong
                  title={`${group.agents.length} targets in this capability group`}
                >
                  {group.agents.length}
                </strong>
                <span title={impactGroupTitle(group, mode, unavailableLabel)}>
                  {group.key === "unavailable" ? unavailableLabel : group.label}
                </span>
              </div>
              <TargetImpactChips
                agents={group.agents}
                mode={vpsNameDisplayMode}
              />
            </div>
          ))}
        </div>
      )}
      {attentionCount > 0 && (
        <p
          className="targetImpactHint"
          title={
            forceUnprivileged
              ? "Targets that are not ready will still be dispatched as privilege-unlocked best effort"
              : "At least one selected target needs review or cannot run this operation"
          }
        >
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

export function resolveAgentsById(
  agents: AgentView[],
  clientIds: string[],
): AgentView[] {
  const byId = new Map(agents.map((agent) => [agent.id, agent]));
  return clientIds
    .map((clientId) => byId.get(clientId))
    .filter((agent): agent is AgentView => Boolean(agent));
}

export function LocalTargetPreview({
  agents,
  ariaLabel = "Local VPS match preview",
}: {
  agents: AgentView[];
  ariaLabel?: string;
}) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [expanded, setExpanded] = useState(false);
  const visibleAgents = expanded
    ? agents
    : agents.slice(0, COLLAPSED_TARGET_CHIP_LIMIT);
  const remaining = agents.length - visibleAgents.length;

  if (agents.length === 0) {
    return null;
  }
  return (
    <div
      aria-label={ariaLabel}
      className="targetChipList"
      title={`${agents.length} locally matched VPS${agents.length === 1 ? "" : "s"}`}
    >
      {visibleAgents.map((agent) => (
        <span className="targetChip" key={agent.id} title={agent.id}>
          {formatVpsName(agent, vpsNameDisplayMode)}
        </span>
      ))}
      {remaining > 0 ? (
        <button
          className="targetChip mutedChip showMoreChip"
          onClick={() => setExpanded(true)}
          title={`Show ${remaining} additional matched VPS${remaining === 1 ? "" : "s"}`}
          type="button"
        >
          Show {remaining} more
        </button>
      ) : expanded && agents.length > COLLAPSED_TARGET_CHIP_LIMIT ? (
        <button
          className="targetChip mutedChip showMoreChip"
          onClick={() => setExpanded(false)}
          title="Collapse the matched VPS list"
          type="button"
        >
          Show fewer
        </button>
      ) : null}
    </div>
  );
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
    return target.capabilities.privilege_mode === "unknown"
      ? "observation_only"
      : "ready";
  }
  if (target.capabilities.privilege_mode === "unknown") {
    return "observation_only";
  }
  if (mode === "root_network_mutation") {
    return target.capabilities.privilege_mode === "root" &&
      target.capabilities.can_manage_runtime_tunnels
      ? "ready"
      : target.capabilities.can_attempt_privileged_ops
        ? "degraded"
        : "unsupported";
  }
  if (mode === "process_limits") {
    return target.capabilities.privilege_mode === "root" &&
      target.capabilities.can_apply_process_limits
      ? "ready"
      : target.capabilities.can_attempt_privileged_ops
        ? "degraded"
        : "unsupported";
  }
  return target.capabilities.privilege_mode === "root" &&
    target.capabilities.can_attempt_privileged_ops
    ? "ready"
    : target.capabilities.can_attempt_privileged_ops
      ? "degraded"
      : "unsupported";
}

function TargetImpactChips({
  agents,
  mode,
}: {
  agents: AgentView[];
  mode: VpsNameDisplayMode;
}) {
  const [expanded, setExpanded] = useState(false);
  if (agents.length === 0) {
    return (
      <small title="No selected VPS belongs to this capability group">
        No targets
      </small>
    );
  }
  const visible = expanded
    ? agents
    : agents.slice(0, COLLAPSED_TARGET_CHIP_LIMIT);
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

function impactGroupTitle(
  group: TargetImpactGroup,
  mode: TargetImpactMode,
  unavailableLabel: string,
): string {
  const label = group.key === "unavailable" ? unavailableLabel : group.label;
  const detail =
    group.key === "ready"
      ? "can run the operation with the reported capabilities"
      : group.key === "needs_review"
        ? "has stale, degraded, forced, or observation-only capability evidence"
        : "has unavailable evidence or does not report the required capability";
  return `${group.agents.length} ${label.toLocaleLowerCase()} target${group.agents.length === 1 ? "" : "s"} for ${operationLabel(mode)}; ${detail}`;
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
