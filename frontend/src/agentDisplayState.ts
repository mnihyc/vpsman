import type { AgentView } from "./types";
import { formatTime } from "./utils";

export type AgentDisplayState = {
  detail: string;
  label: string;
  tone: AgentStatusTone;
};

export type AgentStatusTone =
  | "critical"
  | "info"
  | "neutral"
  | "ok"
  | "warning";

export type AgentStatusPresentation = {
  label: string;
  tone: AgentStatusTone;
};

export const ACCESS_REVOKED_RECOVERY_DETAIL =
  "The current agent key is permanently blocked. Assign a new key to recover this VPS ID.";

export function agentStatusPresentation(
  rawStatus: string | null | undefined,
): AgentStatusPresentation {
  const trimmed = rawStatus?.trim() ?? "";
  switch (trimmed.toLowerCase()) {
    case "online":
      return { label: "Online", tone: "ok" };
    case "stale":
      return { label: "Stale", tone: "warning" };
    case "offline":
    case "disconnected":
      return { label: "Offline", tone: "neutral" };
    case "suspended":
      return { label: "Suspended", tone: "neutral" };
    case "never":
      return { label: "Never connected", tone: "warning" };
    case "revoked":
      return { label: "Access revoked", tone: "warning" };
    case "deleted":
      return { label: "Deleted", tone: "critical" };
    case "":
      return { label: "Unknown", tone: "warning" };
    default:
      return {
        label: capitalizeWords(trimmed.replace(/[_-]+/g, " ")),
        tone: "warning",
      };
  }
}

export function agentDisplayState(agent: AgentView): AgentDisplayState {
  const rawStatus = agent.status.trim();
  const status = rawStatus.toLowerCase();
  const presentation = agentStatusPresentation(rawStatus);
  const lastSeen = normalizeAgentTimestamp(agent.last_seen_at);
  if (status === "online") {
    if (!lastSeen) {
      return {
        detail:
          "Registered as online, but no last contact has been reported by the gateway.",
        label: "Contact unknown",
        tone: "warning",
      };
    }
    return {
      detail: `Last contact ${formatTime(lastSeen)}`,
      ...presentation,
    };
  }
  if (status === "stale") {
    return {
      detail: agent.stale_reason ?? "Last contact is stale.",
      ...presentation,
    };
  }
  if (status === "offline" || status === "disconnected") {
    return {
      detail: lastSeen
        ? status === "disconnected"
          ? `Gateway session disconnected; last contact ${formatTime(lastSeen)}`
          : `Last contact ${formatTime(lastSeen)}`
        : status === "disconnected"
          ? "The last gateway session ended and no current connection is active."
          : "No current agent connection.",
      ...presentation,
    };
  }
  if (status === "never") {
    return {
      detail:
        "Registered, but the agent has never established a gateway session.",
      ...presentation,
    };
  }
  if (status === "suspended") {
    return {
      detail:
        "This offline period is expected. Monitoring, alerts, and new dispatches are paused until manual Unsuspend or authenticated reconnect.",
      ...presentation,
    };
  }
  if (status === "revoked") {
    return {
      detail: ACCESS_REVOKED_RECOVERY_DETAIL,
      ...presentation,
    };
  }
  return {
    detail: lastSeen
      ? `Last contact ${formatTime(lastSeen)}`
      : "Contact evidence is not reported.",
    ...presentation,
  };
}

function capitalizeWords(value: string): string {
  return value.replace(/(^|\s)\S/g, (character) => character.toUpperCase());
}

function normalizeAgentTimestamp(
  value: string | null | undefined,
): string | null {
  const trimmed = value?.trim();
  if (!trimmed) {
    return null;
  }
  if (/^\d{10}$/.test(trimmed)) {
    return new Date(Number(trimmed) * 1000).toISOString();
  }
  if (/^\d{13}$/.test(trimmed)) {
    return new Date(Number(trimmed)).toISOString();
  }
  return trimmed;
}
