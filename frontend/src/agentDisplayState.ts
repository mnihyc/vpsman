import type { AgentView } from "./types";
import { formatTime } from "./utils";

export type AgentDisplayState = {
  detail: string;
  label: string;
  tone: "critical" | "info" | "neutral" | "ok" | "warning";
};

export function agentDisplayState(agent: AgentView): AgentDisplayState {
  const rawStatus = agent.status.trim();
  const status = rawStatus.toLowerCase();
  const lastSeen = normalizeAgentTimestamp(agent.last_seen_at);
  if (status === "online") {
    if (!lastSeen) {
      return {
        detail: "Registered as online, but no last contact has been reported by the gateway.",
        label: "Contact unknown",
        tone: "warning",
      };
    }
    return {
      detail: `Last contact ${formatTime(lastSeen)}`,
      label: "Online",
      tone: "ok",
    };
  }
  if (status === "stale") {
    return {
      detail: agent.stale_reason ?? "Last contact is stale.",
      label: "Stale",
      tone: "warning",
    };
  }
  if (status === "offline") {
    return {
      detail: lastSeen ? `Last contact ${formatTime(lastSeen)}` : "No current agent connection.",
      label: "Offline",
      tone: "neutral",
    };
  }
  return {
    detail: lastSeen ? `Last contact ${formatTime(lastSeen)}` : "Contact evidence is not reported.",
    label: rawStatus || "Unknown",
    tone: "warning",
  };
}

function normalizeAgentTimestamp(value: string | null | undefined): string | null {
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
