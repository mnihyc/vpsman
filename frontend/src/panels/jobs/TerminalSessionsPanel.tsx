import {
  Copy,
  Download,
  History,
  Keyboard,
  LockKeyhole,
  LogIn,
  Maximize2,
  Play,
  Radio,
  RefreshCw,
  ShieldCheck,
  TerminalSquare,
  XCircle,
} from "lucide-react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import {
  ConsoleDataGrid,
  type ConsoleDataGridAction,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  ActionFeedback,
  type ActionFeedbackTone,
} from "../../components/ActionFeedback";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { VpsCombobox } from "../../components/VpsCombobox";
import { consolePalette } from "../../colorPalette";
import { formatLowerBoundCount } from "../../constants";
import {
  useTerminalSessionSocket,
  type TerminalStreamSnapshot,
} from "../../hooks/useTerminalSessionSocket";
import { terminalSessionStateBadgeClass } from "../../jobStatusPresentation";
import { scrollIntoViewWithMotion } from "../../motion";
import type { AgentView } from "../../types";
import type {
  TerminalReplayRecord,
  TerminalSessionRecord,
} from "../../typesTerminal";
import { formatTime, shortId } from "../../utils";
import type { PrivilegeMaterial } from "../../privilege";

type TerminalLaunchProfile = "posix-login" | "bash-login" | "plain-sh";

const TERMINAL_LAUNCH_PROFILES: Array<{
  argv: string[];
  description: string;
  label: string;
  value: TerminalLaunchProfile;
}> = [
  {
    argv: ["/bin/sh", "-l"],
    description: "Portable login shell",
    label: "POSIX login",
    value: "posix-login",
  },
  {
    argv: ["/bin/bash", "-l"],
    description: "Bash login shell",
    label: "Bash login",
    value: "bash-login",
  },
  {
    argv: ["/bin/sh"],
    description: "Plain non-login shell",
    label: "Plain sh",
    value: "plain-sh",
  },
];

type TerminalLaunchUser = "agent" | "root" | "root-fallback";

const TERMINAL_UNLOCK_REQUIRED_STATUS =
  "Unlock privilege, then open the terminal from this launcher.";
type ModalSiblingState = {
  ariaHidden: string | null;
  element: HTMLElement;
  inert: boolean;
};

export function TerminalSessionsPanel({
  agents,
  accessToken,
  clientLabel,
  initialTargetClientId,
  initialTargetRequestId,
  sessions,
  sessionsTruncated,
  loading,
  onOpenSessionEvidence,
  onOpenPrivilegeUnlock,
  onInitialTargetConsumed,
  onOpenTerminal,
  onReplay,
  onRefresh,
  privilegeMaterial,
}: {
  agents: AgentView[];
  accessToken: string;
  clientLabel: (clientId: string) => string;
  initialTargetClientId?: string | null;
  initialTargetRequestId?: string | null;
  sessions: TerminalSessionRecord[];
  sessionsTruncated: boolean;
  loading: boolean;
  onOpenSessionEvidence?: () => void;
  onOpenPrivilegeUnlock: () => void;
  onInitialTargetConsumed?: (requestId: string) => void;
  onOpenTerminal: (request: {
    maxTimeoutSecs: number;
    session: TerminalSessionRecord;
    terminalReplayFromSeq?: string;
    terminalUser: string;
    terminalUserPolicy: "fail" | "fallback";
  }) => Promise<void>;
  onReplay: (
    clientId: string,
    sessionId: string,
    fromSeq?: number,
  ) => Promise<TerminalReplayRecord>;
  onRefresh: () => void;
  privilegeMaterial: PrivilegeMaterial | null;
}) {
  const [launchTargetId, setLaunchTargetId] = useState("");
  const [launchProfile, setLaunchProfile] =
    useState<TerminalLaunchProfile>("posix-login");
  const [launchCwd, setLaunchCwd] = useState("");
  const [launchUser, setLaunchUser] = useState<TerminalLaunchUser>("agent");
  const [launchIdleTimeoutSecs, setLaunchIdleTimeoutSecs] = useState(3600);
  const [launchCols, setLaunchCols] = useState(120);
  const [launchRows, setLaunchRows] = useState(40);
  const [launchStatus, setLaunchStatus] = useState<string | null>(null);
  const [launchStatusTone, setLaunchStatusTone] =
    useState<ActionFeedbackTone>("info");
  const [launchPending, setLaunchPending] = useState(false);
  const [replayPreview, setReplayPreview] =
    useState<TerminalReplayPreview | null>(null);
  const [replayPendingKey, setReplayPendingKey] = useState<string | null>(null);
  const [replayError, setReplayError] = useState<string | null>(null);
  const [followKey, setFollowKey] = useState<string | null>(null);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const [terminalFocusOpen, setTerminalFocusOpen] = useState(false);
  const [closeSession, setCloseSession] =
    useState<TerminalSessionRecord | null>(null);
  const [closePending, setClosePending] = useState(false);
  const [closeStatus, setCloseStatus] = useState<string | null>(null);
  const [closeStatusTone, setCloseStatusTone] =
    useState<ActionFeedbackTone>("info");
  const terminalFocusRef = useRef<HTMLDivElement | null>(null);
  const terminalReplayFeedbackRef = useRef<HTMLDivElement | null>(null);
  const terminalFocusReplayFeedbackRef = useRef<HTMLDivElement | null>(null);
  const terminalCloseFeedbackRef = useRef<HTMLDivElement | null>(null);
  const appliedInitialTargetRequestRef = useRef<string | null>(null);
  const autoFollowedSessionKeyRef = useRef<string | null>(null);
  const launchTarget =
    agents.find((agent) => agent.id === launchTargetId) ?? null;
  const launchProfileRecord =
    TERMINAL_LAUNCH_PROFILES.find(
      (profile) => profile.value === launchProfile,
    ) ?? TERMINAL_LAUNCH_PROFILES[0];
  const privilegeReady = Boolean(privilegeMaterial);
  const launchFeedbackMessage =
    privilegeReady && launchStatus === TERMINAL_UNLOCK_REQUIRED_STATUS
      ? "Privilege unlocked. Terminal controls are ready."
      : launchStatus;
  const launchFeedbackTone =
    privilegeReady && launchStatus === TERMINAL_UNLOCK_REQUIRED_STATUS
      ? "success"
      : launchStatusTone;
  const launchPrivilegeTitle = privilegeReady
    ? "Local privilege is unlocked in this browser. Open submits one audited terminal_open job for the selected VPS."
    : "Terminal open requires local privilege material. Unlock once, then open the terminal from this launcher.";
  const launchPrimaryTitle = !launchTarget
    ? "Select an online VPS target before opening a terminal."
    : launchPending
      ? "Terminal open request is being submitted."
      : privilegeReady
        ? `Open ${launchProfileRecord.label} on ${clientLabel(launchTarget.id)} with an audited terminal_open job.`
        : "Unlock privilege; after unlock, this same launcher opens the terminal directly.";
  const activeSession = useMemo(
    () =>
      sessions.find(
        (session) => `${session.client_id}:${session.session_id}` === activeKey,
      ) ??
      sessions.find(isTerminalActive) ??
      sessions[0] ??
      null,
    [activeKey, sessions],
  );
  const openSessions = sessions.filter(isTerminalActive).length;
  const replayableSessions = sessions.filter(
    (session) => session.output_next_seq !== null,
  ).length;
  const retainedBytes = sessions.reduce(
    (total, session) => total + (session.output_retained_bytes ?? 0),
    0,
  );
  const followedSession = followKey
    ? (sessions.find(
        (session) => `${session.client_id}:${session.session_id}` === followKey,
      ) ?? null)
    : null;
  const followingLive = Boolean(
    followedSession && isTerminalActive(followedSession),
  );
  const streamFromSeq = followedSession
    ? (followedSession.output_retained_first_seq ??
      followedSession.output_first_seq ??
      1)
    : 1;
  const terminalSocket = useTerminalSessionSocket({
    accessToken,
    clientId: followedSession?.client_id ?? null,
    enabled: followingLive,
    fromSeq: streamFromSeq,
    sessionId: followedSession?.session_id ?? null,
  });
  const terminalSummary = sessionsTruncated
    ? `${formatLowerBoundCount(openSessions, true)} open, ${formatLowerBoundCount(replayableSessions, true)} replayable, ${formatBytes(retainedBytes)} retained in loaded sessions`
    : `${openSessions} open, ${replayableSessions} replayable, ${formatBytes(retainedBytes)} retained`;
  const terminalReplayFeedbackMessage = replayError;
  const activeSessionKey = activeSession
    ? `${activeSession.client_id}:${activeSession.session_id}`
    : null;
  const activeSocketReplay =
    terminalSocket.snapshot?.sessionKey === activeSessionKey
      ? terminalSocket.snapshot
      : null;
  const activeStream =
    activeSocketReplay?.sessionKey === followKey ? activeSocketReplay : null;
  const presentedActiveSession =
    activeSession && activeStream
      ? {
          ...activeSession,
          ...(terminalSocket.sessionRecord ?? {}),
          output_next_seq: activeStream.nextSeq,
          output_retained_first_seq:
            activeStream.availableFirstSeq ??
            terminalSocket.sessionRecord?.output_retained_first_seq ??
            activeSession.output_retained_first_seq,
        }
      : activeSession;
  const activeReplay = activeSocketReplay
    ? toStreamReplayPreview(activeSocketReplay, activeSession?.session_id ?? "")
    : replayPreview && activeSession?.session_id === replayPreview.sessionId
      ? replayPreview
      : null;
  const transcriptUnavailableReason = activeSession
    ? activeReplay?.text
      ? null
      : activeStream && terminalSocket.connectionState !== "ready"
        ? "Terminal replay is reconnecting; retained output already shown remains available."
        : "Load Replay first; transcript export uses the retained replay loaded in this browser."
    : "Select a terminal session before copying or downloading transcript text.";
  const terminalControlStatus = terminalSocket.feedback;
  const terminalControlStatusTone: ActionFeedbackTone =
    terminalSocket.feedback === null ? "info" : "danger";
  const terminalInputEnabled = Boolean(
    activeSession &&
    isTerminalActive(activeSession) &&
    activeStream &&
    terminalSocket.connectionState === "ready" &&
    (!terminalSocket.sessionState ||
      isTerminalSocketStateOpen(terminalSocket.sessionState)),
  );
  const closeSessionKey = closeSession
    ? `${closeSession.client_id}:${closeSession.session_id}`
    : null;
  const closeSocketReady = Boolean(
    closeSessionKey &&
    terminalSocket.snapshot?.sessionKey === closeSessionKey &&
    terminalSocket.connectionState === "ready",
  );
  const terminalTranscriptState = activeStream
    ? terminalSocket.connectionState === "ready"
      ? "Live terminal connected; retained replay and new output stream over this session socket."
      : terminalSocket.connectionState === "closed"
        ? "Terminal stream closed; retained output already received remains available."
        : "Terminal stream reconnecting; input is disabled and retained output already received remains available."
    : (transcriptUnavailableReason ??
      "Loaded retained replay can be copied or downloaded from this browser.");
  const terminalRowActions: ConsoleDataGridAction<TerminalSessionRecord>[] = [
    {
      description: ([session]) =>
        session
          ? "Follow persisted output as new terminal chunks arrive."
          : "Follow terminal output.",
      disabled: ([session]) =>
        !session ||
        !isTerminalActive(session) ||
        session.output_next_seq === null,
      hidden: ([session]) =>
        Boolean(
          session &&
          followingLive &&
          followKey === `${session.client_id}:${session.session_id}`,
        ),
      icon: <Radio size={13} />,
      label: "Follow",
      onSelect: ([session]) => session && toggleFollow(session),
    },
    {
      description: () => "Stop following live terminal output.",
      hidden: ([session]) =>
        !session ||
        !followingLive ||
        followKey !== `${session.client_id}:${session.session_id}`,
      icon: <Radio size={13} />,
      label: "Stop follow",
      onSelect: ([session]) => session && toggleFollow(session),
    },
    {
      description: () => "Load durable replay from retained terminal output.",
      disabled: ([session]) =>
        !session ||
        session.output_next_seq === null ||
        replayPendingKey === `${session.client_id}:${session.session_id}`,
      icon: <History size={13} />,
      label: "Replay",
      onSelect: ([session]) => {
        if (session) {
          void loadDurableReplay(session);
        }
      },
    },
    {
      description: () => "Attach to this active terminal session.",
      disabled: ([session]) => !session || !isTerminalActive(session),
      icon: <LogIn size={13} />,
      label: "Attach",
      onSelect: ([session]) => session && attachTerminalSession(session),
    },
    {
      description: () => "Focus the interactive terminal for this session.",
      disabled: ([session]) => !session || !isTerminalActive(session),
      icon: <Keyboard size={13} />,
      label: "Input",
      onSelect: ([session]) => session && focusTerminalInput(session),
    },
    {
      description: () => "Close this terminal session after review.",
      disabled: ([session]) => !session || !isTerminalActive(session),
      icon: <XCircle size={13} />,
      label: "Close",
      onSelect: ([session]) => session && requestTerminalClose(session),
      tone: "danger",
    },
  ];
  const terminalColumns: ConsoleDataGridColumn<TerminalSessionRecord>[] = [
    {
      cell: (session) => {
        const key = `${session.client_id}:${session.session_id}`;
        const selected =
          activeSession?.client_id === session.client_id &&
          activeSession.session_id === session.session_id;
        return (
          <span className="historyPrimary">
            <button
              className={`linkLikeButton ${selected ? "activeAction" : ""}`}
              onClick={(event) => {
                event.stopPropagation();
                selectTerminalSession(key);
              }}
              type="button"
            >
              {clientLabel(session.client_id)}
            </button>
            <small>Session {shortId(session.session_id)}</small>
          </span>
        );
      },
      header: "Session",
      id: "session",
      searchValue: (session) =>
        `${clientLabel(session.client_id)} ${session.client_id} ${session.session_id}`,
      sortValue: (session) =>
        `${clientLabel(session.client_id)}:${session.session_id}`,
    },
    {
      cell: (session) => (
        <span className="historyPrimary">
          <span
            className={`status ${terminalSessionStateBadgeClass(session.state)}`}
            title={`${session.state}; ${formatSessionLifecycle(session)}`}
          >
            {session.state}
          </span>
          <small>{formatSessionLifecycle(session)}</small>
        </span>
      ),
      header: "State",
      id: "state",
      searchValue: (session) => `${session.state} ${session.last_status}`,
      sortValue: (session) => session.state,
    },
    {
      cell: (session) => (
        <span className="historyPrimary">
          <strong>{formatArgv(session.argv) || "Terminal"}</strong>
          <small>{formatShellContext(session)}</small>
        </span>
      ),
      header: "Command",
      id: "command",
      searchValue: (session) =>
        `${formatArgv(session.argv)} terminal_open ${session.cwd ?? ""}`,
      sortValue: (session) => formatArgv(session.argv) || "terminal_open",
    },
    {
      cell: (session) => (
        <span className="historyPrimary">
          <strong>{formatWindow(session)}</strong>
          <small>{formatLimits(session)}</small>
        </span>
      ),
      header: "Window",
      id: "window",
      searchValue: (session) =>
        `${formatWindow(session)} ${formatLimits(session)}`,
      sortValue: (session) => formatWindow(session),
    },
    {
      cell: (session) => (
        <span className="historyPrimary">
          <strong>{formatOutputRange(session)}</strong>
          <small
            className={
              session.output_dropped_bytes || session.output_replay_truncated
                ? "terminalWarning"
                : undefined
            }
          >
            {formatOutputRetention(session)}
          </small>
        </span>
      ),
      header: "Output",
      id: "output",
      searchValue: (session) =>
        `${formatOutputRange(session)} ${formatOutputRetention(session)}`,
      sortValue: (session) => session.output_next_seq ?? 0,
    },
    {
      cell: (session) => formatTime(session.observed_at),
      header: "Observed",
      id: "observed",
      searchValue: (session) => formatTime(session.observed_at),
      sortValue: (session) => session.observed_at,
    },
  ];

  useEffect(() => {
    if (agents.length === 0) {
      setLaunchTargetId("");
      return;
    }
    if (
      launchTargetId &&
      !agents.some((agent) => agent.id === launchTargetId)
    ) {
      setLaunchTargetId("");
    }
  }, [agents, launchTargetId]);

  useEffect(() => {
    if (
      !initialTargetClientId ||
      !initialTargetRequestId ||
      appliedInitialTargetRequestRef.current === initialTargetRequestId ||
      !agents.some((agent) => agent.id === initialTargetClientId)
    ) {
      return;
    }
    appliedInitialTargetRequestRef.current = initialTargetRequestId;
    setLaunchTargetId(initialTargetClientId);
    onInitialTargetConsumed?.(initialTargetRequestId);
  }, [
    agents,
    initialTargetClientId,
    initialTargetRequestId,
    onInitialTargetConsumed,
  ]);

  useEffect(() => {
    if (!followKey) {
      return;
    }
    const followed = sessions.find(
      (session) => `${session.client_id}:${session.session_id}` === followKey,
    );
    if (!followed || !isTerminalActive(followed)) {
      setFollowKey(null);
    }
  }, [followKey, sessions]);

  useEffect(() => {
    if (!closeSession || loading) {
      return;
    }
    const currentSession = sessions.find(
      (session) =>
        session.client_id === closeSession.client_id &&
        session.session_id === closeSession.session_id,
    );
    if (currentSession && isTerminalActive(currentSession)) {
      return;
    }
    setCloseSession(null);
    setCloseStatusTone("warning");
    setCloseStatus(
      `Terminal ${shortId(closeSession.session_id)} is already closed; no close action was sent.`,
    );
  }, [closeSession, loading, sessions]);

  useEffect(() => {
    if (
      !activeSession ||
      !isTerminalActive(activeSession) ||
      activeSession.output_next_seq === null
    ) {
      return;
    }
    const key = `${activeSession.client_id}:${activeSession.session_id}`;
    if (followKey || autoFollowedSessionKeyRef.current === key) {
      return;
    }
    autoFollowedSessionKeyRef.current = key;
    setFollowKey(key);
  }, [
    activeSession?.client_id,
    activeSession?.session_id,
    activeSession?.output_next_seq,
    activeSession?.state,
    followKey,
  ]);

  useEffect(() => {
    if (!terminalFocusOpen || !terminalFocusRef.current) {
      return undefined;
    }
    const overlay = terminalFocusRef.current;
    const previousFocus =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    const siblings: ModalSiblingState[] = Array.from(document.body.children)
      .filter(
        (element): element is HTMLElement =>
          element instanceof HTMLElement && element !== overlay,
      )
      .map((element) => ({
        ariaHidden: element.getAttribute("aria-hidden"),
        element,
        inert: element.inert,
      }));
    for (const sibling of siblings) {
      sibling.element.inert = true;
      sibling.element.setAttribute("aria-hidden", "true");
    }
    window.requestAnimationFrame(() => {
      const terminalInput = overlay.querySelector<HTMLElement>(
        ".xterm-helper-textarea",
      );
      terminalInput?.focus({ preventScroll: true });
    });
    function handleFocusedTerminalKeyDown(event: KeyboardEvent) {
      if (overlay.inert) {
        return;
      }
      if (event.key !== "Tab") {
        return;
      }
      if (
        event.target instanceof HTMLElement &&
        event.target.classList.contains("xterm-helper-textarea")
      ) {
        return;
      }
      const focusable = getFocusableElements(overlay);
      if (focusable.length === 0) {
        event.preventDefault();
        overlay.focus({ preventScroll: true });
        return;
      }
      const first = focusable[0]!;
      const last = focusable[focusable.length - 1]!;
      const active = document.activeElement;
      if (event.shiftKey && (active === first || !overlay.contains(active))) {
        event.preventDefault();
        last.focus({ preventScroll: true });
      } else if (
        !event.shiftKey &&
        (active === last || !overlay.contains(active))
      ) {
        event.preventDefault();
        first.focus({ preventScroll: true });
      }
    }
    function handleFocusedTerminalFocus(event: FocusEvent) {
      if (overlay.inert) {
        return;
      }
      if (event.target instanceof Node && overlay.contains(event.target)) {
        return;
      }
      overlay.focus({ preventScroll: true });
    }
    document.addEventListener("keydown", handleFocusedTerminalKeyDown, true);
    document.addEventListener("focusin", handleFocusedTerminalFocus);
    return () => {
      document.removeEventListener(
        "keydown",
        handleFocusedTerminalKeyDown,
        true,
      );
      document.removeEventListener("focusin", handleFocusedTerminalFocus);
      for (const sibling of siblings) {
        sibling.element.inert = sibling.inert;
        if (sibling.ariaHidden === null) {
          sibling.element.removeAttribute("aria-hidden");
        } else {
          sibling.element.setAttribute("aria-hidden", sibling.ariaHidden);
        }
      }
      if (previousFocus?.isConnected) {
        previousFocus.focus({ preventScroll: true });
      }
    };
  }, [terminalFocusOpen]);

  useEffect(() => {
    if (!replayError) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const outcome = terminalFocusOpen
        ? terminalFocusReplayFeedbackRef.current
        : terminalReplayFeedbackRef.current;
      if (!outcome) {
        return;
      }
      outcome.tabIndex = -1;
      scrollIntoViewWithMotion(outcome, { block: "nearest" });
      outcome.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [replayError, terminalFocusOpen]);

  useEffect(() => {
    if (!closeStatus || closeSession !== null) {
      return undefined;
    }
    const frame = window.requestAnimationFrame(() => {
      const outcome = terminalCloseFeedbackRef.current;
      if (!outcome) {
        return;
      }
      outcome.tabIndex = -1;
      scrollIntoViewWithMotion(outcome, { block: "nearest" });
      outcome.focus({ preventScroll: true });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [closeSession, closeStatus, closeStatusTone]);

  async function loadDurableReplay(session: TerminalSessionRecord) {
    const key = `${session.client_id}:${session.session_id}`;
    selectTerminalSession(key);
    setReplayPendingKey(key);
    setReplayError(null);
    try {
      const fromSeq =
        session.output_retained_first_seq ?? session.output_first_seq ?? 1;
      const replay = await onReplay(
        session.client_id,
        session.session_id,
        fromSeq,
      );
      setReplayPreview(toReplayPreview(replay));
    } catch (error) {
      setReplayError(
        error instanceof Error ? error.message : "Terminal replay unavailable",
      );
    } finally {
      setReplayPendingKey(null);
    }
  }

  async function copyTranscript() {
    if (!activeReplay?.text) {
      return;
    }
    await navigator.clipboard.writeText(activeReplay.text);
    setReplayError(null);
  }

  function downloadTranscript() {
    if (!activeReplay?.text) {
      return;
    }
    const blob = new Blob([activeReplay.text], {
      type: "text/plain;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `terminal-${shortId(activeReplay.sessionId)}-replay.txt`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
  }

  function toggleFollow(session: TerminalSessionRecord) {
    const key = `${session.client_id}:${session.session_id}`;
    selectTerminalSession(key);
    if (followKey === key) {
      autoFollowedSessionKeyRef.current = key;
      setFollowKey(null);
      return;
    }
    if (!isTerminalActive(session) || session.output_next_seq === null) {
      return;
    }
    setFollowKey(key);
  }

  function focusTerminalInput(session: TerminalSessionRecord) {
    selectTerminalSession(`${session.client_id}:${session.session_id}`);
    setFollowKey(`${session.client_id}:${session.session_id}`);
    setTerminalFocusOpen(true);
  }

  function attachTerminalSession(session: TerminalSessionRecord) {
    focusTerminalInput(session);
  }

  async function confirmTerminalClose() {
    const session = closeSession;
    if (!session || closePending) return;
    if (!closeSocketReady) {
      setCloseStatusTone("warning");
      setCloseStatus(
        "Terminal session socket is still connecting. No close action was sent.",
      );
      return;
    }
    setClosePending(true);
    setCloseStatusTone("progress");
    setCloseStatus(
      `Closing terminal ${shortId(session.session_id)} on ${clientLabel(session.client_id)}...`,
    );
    try {
      await terminalSocket.closeSession("operator_closed");
      setCloseStatusTone("success");
      setCloseStatus(`Terminal ${shortId(session.session_id)} closed.`);
      setCloseSession(null);
      onRefresh();
    } catch (error) {
      setCloseStatusTone("danger");
      setCloseStatus(
        error instanceof Error ? error.message : "Terminal close failed.",
      );
    } finally {
      setClosePending(false);
    }
  }

  function requestTerminalClose(session: TerminalSessionRecord) {
    setCloseStatus(null);
    selectTerminalSession(`${session.client_id}:${session.session_id}`);
    setFollowKey(`${session.client_id}:${session.session_id}`);
    setCloseSession(session);
  }

  async function openNewTerminal() {
    if (!launchTarget) {
      setLaunchStatusTone("warning");
      setLaunchStatus("Select a VPS before opening a terminal.");
      return;
    }
    if (!privilegeMaterial) {
      setLaunchStatusTone("warning");
      setLaunchStatus(TERMINAL_UNLOCK_REQUIRED_STATUS);
      onOpenPrivilegeUnlock();
      return;
    }
    const now = new Date().toISOString();
    const session: TerminalSessionRecord = {
      session_id: crypto.randomUUID(),
      client_id: launchTarget.id,
      job_id: "pending-review",
      state: "opening",
      last_status: "opening",
      argv: launchProfileRecord.argv,
      cwd: launchCwd.trim() || null,
      cols: clampNumber(launchCols, 20, 240),
      rows: clampNumber(launchRows, 5, 120),
      idle_timeout_secs: clampNumber(launchIdleTimeoutSecs, 10, 86400),
      flow_window_bytes: 65536,
      output_first_seq: null,
      output_next_seq: null,
      output_retained_first_seq: null,
      output_retained_bytes: 0,
      output_dropped_bytes: 0,
      output_dropped_chunks: 0,
      output_replay_truncated: false,
      last_input_seq: 0,
      close_reason: null,
      last_event: "terminal_open",
      opened_at: now,
      observed_at: now,
    };
    setLaunchPending(true);
    setLaunchStatusTone("progress");
    setLaunchStatus(`Opening terminal on ${clientLabel(launchTarget.id)}...`);
    try {
      await onOpenTerminal({
        maxTimeoutSecs: clampNumber(launchIdleTimeoutSecs, 10, 86400),
        session,
        terminalReplayFromSeq: "",
        terminalUser: launchUser === "agent" ? "" : "root",
        terminalUserPolicy:
          launchUser === "root-fallback" ? "fallback" : "fail",
      });
      selectTerminalSession(`${session.client_id}:${session.session_id}`);
      setFollowKey(null);
      setReplayPreview(null);
      setLaunchStatusTone("success");
      setLaunchStatus(
        `${clientLabel(launchTarget.id)} terminal open job submitted.`,
      );
    } catch (error) {
      setLaunchStatusTone("danger");
      setLaunchStatus(
        error instanceof Error ? error.message : "Terminal open failed.",
      );
    } finally {
      setLaunchPending(false);
    }
  }

  function selectTerminalSession(key: string) {
    setReplayError(null);
    setActiveKey(key);
  }

  return (
    <div className="fleetPanel terminalSessionsPanel">
      <div className="sectionHeader">
        <div>
          <h2>Terminal sessions</h2>
          <span>{terminalSummary}</span>
        </div>
        <div className="headerActionStack">
          <div className="rowActions compactRowActions">
            {onOpenSessionEvidence && (
              <button
                className="secondaryAction compactAction"
                onClick={onOpenSessionEvidence}
                title="Open Audit / Sessions evidence for terminal ownership, replay state, and retained proof."
                type="button"
              >
                <History size={14} />
                <span>Evidence</span>
              </button>
            )}
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                loading
                  ? "Terminal session inventory is already refreshing."
                  : undefined
              }
              disabled={loading}
              onClick={onRefresh}
              title={
                loading
                  ? "Terminal session inventory is refreshing."
                  : "Refresh terminal session inventory and retained state."
              }
              type="button"
            >
              <RefreshCw size={14} />
              <span>Refresh</span>
            </button>
          </div>
        </div>
      </div>
      <div className="terminalLaunchPanel" aria-label="New terminal composer">
        <div className="terminalLaunchIntro">
          <div>
            <h3>New terminal</h3>
            <span>Open one browser terminal without leaving Remote.</span>
          </div>
          <div className="terminalLaunchBadges">
            <strong title="Submitted as an audited terminal_open job with durable job and terminal evidence.">
              Audited terminal_open
            </strong>
            <span
              className={`consoleStatusBadge ${privilegeReady ? "ok" : "warning"}`}
              title={launchPrivilegeTitle}
            >
              {privilegeReady ? (
                <ShieldCheck size={13} />
              ) : (
                <LockKeyhole size={13} />
              )}
              <span>
                {privilegeReady ? "Privilege ready" : "Privilege locked"}
              </span>
            </span>
          </div>
        </div>
        <div className="terminalLaunchGrid">
          <label
            className="wideField"
            title="The terminal opens on exactly one selected VPS; use Fleet scope or search to narrow the target list."
          >
            <span>Target</span>
            <VpsCombobox
              agents={agents}
              ariaLabel="New terminal target"
              disabled={agents.length === 0}
              onChange={setLaunchTargetId}
              placeholder="Select VPS"
              value={launchTarget?.id ?? ""}
            />
          </label>
          <label title="POSIX login is portable; Bash login is richer when bash exists; plain sh avoids login profile side effects.">
            <span>Shell profile</span>
            <select
              aria-label="Terminal shell profile"
              onChange={(event) =>
                setLaunchProfile(event.target.value as TerminalLaunchProfile)
              }
              title={
                TERMINAL_LAUNCH_PROFILES.find(
                  (profile) => profile.value === launchProfile,
                )?.description
              }
              value={launchProfile}
            >
              {TERMINAL_LAUNCH_PROFILES.map((profile) => (
                <option key={profile.value} value={profile.value}>
                  {profile.label} - {profile.description}
                </option>
              ))}
            </select>
          </label>
          <label title="Blank uses the agent's reported default working directory.">
            <span>Working directory</span>
            <input
              aria-label="New terminal working directory"
              onChange={(event) => setLaunchCwd(event.target.value)}
              placeholder="Agent default"
              value={launchCwd}
            />
          </label>
          <label title="Agent user is least surprising; root modes are useful for production repair when agent capability permits it.">
            <span>Run as</span>
            <select
              aria-label="New terminal user policy"
              onChange={(event) =>
                setLaunchUser(event.target.value as TerminalLaunchUser)
              }
              value={launchUser}
            >
              <option value="agent">Agent user</option>
              <option value="root">root, fail if unavailable</option>
              <option value="root-fallback">
                root, fallback to agent user
              </option>
            </select>
          </label>
        </div>
        <details className="terminalAdvancedOptions">
          <summary>Advanced terminal options</summary>
          <div className="terminalLaunchGrid">
            <label title="Server-side idle timeout for the terminal session, clamped between 10 seconds and 24 hours.">
              <span>Idle timeout</span>
              <input
                aria-label="New terminal idle timeout seconds"
                max={86400}
                min={10}
                onChange={(event) =>
                  setLaunchIdleTimeoutSecs(Number(event.target.value))
                }
                type="number"
                value={launchIdleTimeoutSecs}
              />
            </label>
            <label title="Initial terminal width in columns, clamped between 20 and 240. Resize later from session controls.">
              <span>Columns</span>
              <input
                aria-label="New terminal columns"
                max={240}
                min={20}
                onChange={(event) => setLaunchCols(Number(event.target.value))}
                type="number"
                value={launchCols}
              />
            </label>
            <label title="Initial terminal height in rows, clamped between 5 and 120. Resize later from session controls.">
              <span>Rows</span>
              <input
                aria-label="New terminal rows"
                max={120}
                min={5}
                onChange={(event) => setLaunchRows(Number(event.target.value))}
                type="number"
                value={launchRows}
              />
            </label>
          </div>
        </details>
        <div className="terminalLaunchFooter">
          <span>
            {privilegeReady
              ? "Open submits one privileged terminal_open job; the live terminal then carries keyboard and resize controls."
              : "Unlock privilege once to enable terminal_open; replay, copy, and audit evidence stay available while locked."}
          </span>
          <button
            className="primaryAction compactAction"
            data-tooltip-disabled-reason={
              launchPending
                ? "A terminal-open request is already running."
                : !launchTarget
                  ? "Choose one VPS before opening a terminal."
                  : undefined
            }
            disabled={!launchTarget || launchPending}
            onClick={() => void openNewTerminal()}
            title={launchPrimaryTitle}
            type="button"
          >
            {privilegeReady ? <Play size={15} /> : <ShieldCheck size={15} />}
            <span>{privilegeReady ? "Open terminal" : "Unlock privilege"}</span>
          </button>
        </div>
        <ActionFeedback
          className="localActionFeedback terminalLaunchActionFeedback"
          message={launchFeedbackMessage}
          tone={launchFeedbackTone}
        />
      </div>
      <div className="terminalSummaryStrip">
        <span
          title={`${formatLowerBoundCount(openSessions, sessionsTruncated)} terminal sessions are open${sessionsTruncated ? " in the loaded page" : ""}.`}
        >
          <strong>
            {formatLowerBoundCount(openSessions, sessionsTruncated)}
          </strong>
          <small>
            {sessionsTruncated ? "Open in loaded sessions" : "Open"}
          </small>
        </span>
        <span
          title={`${formatLowerBoundCount(replayableSessions, sessionsTruncated)} terminal sessions retain replayable output${sessionsTruncated ? " in the loaded page" : ""}.`}
        >
          <strong>
            {formatLowerBoundCount(replayableSessions, sessionsTruncated)}
          </strong>
          <small>
            {sessionsTruncated ? "Replayable in loaded sessions" : "Replayable"}
          </small>
        </span>
        <span
          title={`${formatBytes(retainedBytes)} terminal output is retained${sessionsTruncated ? " across the loaded page" : ""}.`}
        >
          <strong>{formatBytes(retainedBytes)}</strong>
          <small>
            {sessionsTruncated
              ? "Retained in loaded sessions"
              : "Retained output"}
          </small>
        </span>
        <span
          title={
            followingLive
              ? "The selected terminal is following live output."
              : "The selected terminal is not following live output."
          }
        >
          <strong>{followingLive ? "Following" : "Not following"}</strong>
          <small>Live follow</small>
        </span>
      </div>
      <div className="terminalWorkspace">
        <div className="terminalActiveHeader">
          <div>
            <strong>
              {presentedActiveSession
                ? clientLabel(presentedActiveSession.client_id)
                : "No active terminal"}
            </strong>
            <span
              title={
                presentedActiveSession
                  ? `Terminal session: ${presentedActiveSession.session_id}; launch command: ${formatArgv(presentedActiveSession.argv) || "terminal"}`
                  : "Open a terminal session to attach retained output"
              }
            >
              {presentedActiveSession
                ? `${shortId(presentedActiveSession.session_id)} - ${formatArgv(presentedActiveSession.argv) || "terminal"}`
                : "Open a terminal session to attach retained output"}
            </span>
          </div>
          <div className="rowActions compactRowActions">
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                activeSession
                  ? undefined
                  : "Select a terminal session before loading replay."
              }
              disabled={!activeSession}
              onClick={() =>
                activeSession && void loadDurableReplay(activeSession)
              }
              title="Load retained replay for the active terminal session."
              type="button"
            >
              <History size={13} />
              <span>Replay</span>
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={!activeReplay?.text}
              onClick={() => void copyTranscript()}
              title={
                transcriptUnavailableReason ??
                "Copy the loaded retained replay text"
              }
              type="button"
            >
              <Copy size={13} />
              <span>Copy transcript</span>
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={!activeReplay?.text}
              onClick={downloadTranscript}
              title={
                transcriptUnavailableReason ??
                "Download the loaded retained replay text"
              }
              type="button"
            >
              <Download size={13} />
              <span>Download transcript</span>
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={!activeSession || !isTerminalActive(activeSession)}
              onClick={() => activeSession && focusTerminalInput(activeSession)}
              title={
                activeSession && isTerminalActive(activeSession)
                  ? "Focus the interactive terminal for this exact session."
                  : "Select an open terminal session before sending input."
              }
              type="button"
            >
              <Keyboard size={13} />
              <span>Type</span>
            </button>
            <button
              className="secondaryAction compactAction"
              data-tooltip-disabled-reason={
                activeSession
                  ? undefined
                  : "Select a terminal session before opening focused replay."
              }
              disabled={!activeSession}
              onClick={() => setTerminalFocusOpen(true)}
              title="Open the active terminal replay in a full-screen focused workspace."
              type="button"
            >
              <Maximize2 size={13} />
              <span>Focus terminal</span>
            </button>
          </div>
        </div>
        <div
          className="terminalTranscriptState"
          aria-label="Terminal transcript availability"
        >
          {terminalTranscriptState}
        </div>
        <div
          className="terminalSessionContext"
          aria-label="Active terminal session context"
        >
          <span>
            <strong>
              {presentedActiveSession
                ? formatSessionLifecycle(presentedActiveSession)
                : "No session selected"}
            </strong>
            <small>Lifecycle</small>
          </span>
          <span>
            <strong
              data-tooltip-empty-reason={
                presentedActiveSession
                  ? undefined
                  : "No terminal session is selected, so no working directory is available."
              }
            >
              {presentedActiveSession
                ? (presentedActiveSession.cwd ??
                  "Working directory not reported")
                : "-"}
            </strong>
            <small>Working directory</small>
          </span>
          <span>
            <strong
              data-tooltip-empty-reason={
                presentedActiveSession
                  ? undefined
                  : "No terminal session is selected, so no replay range is available."
              }
            >
              {presentedActiveSession
                ? formatOutputRange(presentedActiveSession)
                : "-"}
            </strong>
            <small>Replay range</small>
          </span>
          <span>
            <strong
              data-tooltip-empty-reason={
                presentedActiveSession
                  ? undefined
                  : "No terminal session is selected, so no input state is available."
              }
            >
              {presentedActiveSession
                ? formatLastInput(presentedActiveSession)
                : "-"}
            </strong>
            <small>Input state</small>
          </span>
        </div>
        <XtermReplay
          inputEnabled={terminalInputEnabled && !terminalFocusOpen}
          label="Active terminal emulator"
          onData={(data) => {
            if (activeSession) {
              terminalSocket.queueInput(data);
            }
          }}
          onResize={(cols, rows) => {
            if (activeSession) {
              terminalSocket.queueResize(cols, rows);
            }
          }}
          resetKey={activeSessionKey ?? "none"}
          text={
            activeReplay?.text ??
            (activeSession
              ? "Select Replay or Follow to load retained output for this session.\r\n"
              : "No terminal session selected.\r\n")
          }
        />
        <ActionFeedback
          className="localActionFeedback terminalInputFeedback"
          message={terminalControlStatus}
          tone={terminalControlStatusTone}
        />
      </div>
      {terminalFocusOpen &&
        activeSession &&
        createPortal(
          <div
            aria-label="Focused terminal workspace"
            aria-modal="true"
            className="terminalFocusOverlay"
            ref={terminalFocusRef}
            role="dialog"
            tabIndex={-1}
          >
            <header>
              <div>
                <strong>
                  {clientLabel(
                    presentedActiveSession?.client_id ??
                      activeSession.client_id,
                  )}
                </strong>
                <span>
                  {shortId(
                    presentedActiveSession?.session_id ??
                      activeSession.session_id,
                  )}{" "}
                  -{" "}
                  {formatArgv(
                    presentedActiveSession?.argv ?? activeSession.argv,
                  ) || "terminal"}
                </span>
              </div>
              <div className="rowActions compactRowActions">
                <button
                  className="secondaryAction compactAction"
                  onClick={() => void loadDurableReplay(activeSession)}
                  title="Reload retained replay for the focused terminal session."
                  type="button"
                >
                  <History size={13} />
                  <span>Replay</span>
                </button>
                <button
                  className="secondaryAction compactAction"
                  disabled={!terminalInputEnabled}
                  onClick={() => focusTerminalInput(activeSession)}
                  title={
                    terminalInputEnabled
                      ? "Focus the interactive terminal."
                      : "Terminal input is available after the active session socket connects."
                  }
                  type="button"
                >
                  <Keyboard size={13} />
                  <span>Input</span>
                </button>
                <button
                  aria-label="Exit focused terminal view"
                  className="secondaryAction compactAction"
                  onClick={() => setTerminalFocusOpen(false)}
                  title="Close focused terminal view and return to the session workspace."
                  type="button"
                >
                  <XCircle size={13} />
                  <span>Exit view</span>
                </button>
              </div>
            </header>
            <ActionFeedback
              className="localActionFeedback terminalReplayActionFeedback"
              message={terminalReplayFeedbackMessage}
              ref={terminalFocusReplayFeedbackRef}
              tone="danger"
            />
            <XtermReplay
              autoFocus
              inputEnabled={terminalInputEnabled}
              label="Focused terminal emulator"
              onData={(data) => terminalSocket.queueInput(data)}
              onResize={(cols, rows) => terminalSocket.queueResize(cols, rows)}
              resetKey={activeSessionKey ?? "none"}
              text={
                activeReplay?.text ??
                "Select Replay or Follow to load retained output for this session.\r\n"
              }
            />
            <ActionFeedback
              className="localActionFeedback terminalInputFeedback"
              message={terminalControlStatus}
              tone={terminalControlStatusTone}
            />
          </div>,
          document.body,
        )}
      {!terminalFocusOpen && (
        <ActionFeedback
          className="localActionFeedback terminalReplayActionFeedback"
          message={terminalReplayFeedbackMessage}
          ref={terminalReplayFeedbackRef}
          tone="danger"
        />
      )}
      <ActionFeedback
        className="localActionFeedback terminalCloseActionFeedback"
        message={closeStatus}
        ref={terminalCloseFeedbackRef}
        tone={closeStatusTone}
      />
      <ConsoleDataGrid
        columns={terminalColumns}
        defaultPageSize={8}
        expandOnRowClick
        getRowId={(session) => `${session.client_id}:${session.session_id}`}
        itemLabel="sessions"
        empty={
          <div className="emptyState">
            <TerminalSquare size={22} />
            <strong>No terminal sessions</strong>
            <span>
              Opening a terminal creates its authorized session and durable
              replay record.
            </span>
          </div>
        }
        renderExpandedRow={(session) => (
          <div className="consoleInlineDetailGrid">
            <span>Session ID</span>
            <strong>{session.session_id}</strong>
            <span>VPS</span>
            <strong>{clientLabel(session.client_id)}</strong>
            <span>Command</span>
            <strong>{formatArgv(session.argv) || "terminal"}</strong>
            <span>Working directory</span>
            <strong>{session.cwd ?? "Not reported"}</strong>
            <span>Output range</span>
            <strong>{formatOutputRange(session)}</strong>
            <span>Retention</span>
            <strong>{formatOutputRetention(session)}</strong>
            <span>Window</span>
            <strong>{formatWindow(session)}</strong>
            <span>Limits</span>
            <strong>{formatLimits(session)}</strong>
            <span>Session lifecycle</span>
            <strong>{formatSessionLifecycle(session)}</strong>
            <span>Last input</span>
            <strong>{formatLastInput(session)}</strong>
            <span>Close reason</span>
            <strong>
              {session.close_reason ??
                (isTerminalActive(session) ? "Open session" : "Not reported")}
            </strong>
            <span>Authorization job</span>
            <strong>{session.job_id}</strong>
            <span>Last input sequence</span>
            <strong>{session.last_input_seq}</strong>
            <span>Observed</span>
            <strong>{formatTime(session.observed_at)}</strong>
            <span>Opened by</span>
            <strong>Not reported by terminal API</strong>
            <span>Privilege scope</span>
            <strong>Not reported by terminal API</strong>
            <span>Retention expiry</span>
            <strong>Not reported by terminal API</strong>
            <span>Last event</span>
            <strong>{session.last_event}</strong>
          </div>
        )}
        rowActions={terminalRowActions}
        rows={sessions}
        rowsTruncated={sessionsTruncated}
        searchPlaceholder="Search terminal sessions"
        showMobileRowActions={false}
        storageKey="vpsman.jobs.terminalSessions"
        title="Session inventory and controls"
      />
      {activeReplay && (
        <div
          className="terminalReplayPreview"
          aria-label="Durable terminal replay status"
        >
          <div>
            <strong>
              Durable replay {shortId(activeReplay.sessionId)}:{" "}
              {activeReplay.chunkCount} chunks,{" "}
              {formatBytes(activeReplay.byteCount)}
            </strong>
            <span>
              {formatReplaySequence(
                activeReplay.availableFirstSeq ?? activeReplay.fromSeq,
                activeReplay.nextSeq,
              )}
              {activeReplay.truncated ? "; truncated" : ""}
              {followingLive &&
              activeSession?.session_id === activeReplay.sessionId &&
              followKey ===
                `${activeSession.client_id}:${activeSession.session_id}`
                ? "; following live output"
                : "; retained replay"}
            </span>
          </div>
        </div>
      )}
      <ConfirmationPrompt
        confirmDisabled={!closeSocketReady}
        confirmLabel="Close terminal"
        detail={
          closeSocketReady
            ? "Ends this exact authorized terminal session. Retained replay remains available after it closes."
            : "Connecting this exact authorized terminal session before close can be sent. No action is queued or retried while disconnected."
        }
        error={closeStatusTone === "danger" ? closeStatus : null}
        items={[
          {
            label: "VPS",
            value: closeSession ? (
              clientLabel(closeSession.client_id)
            ) : (
              <span data-tooltip-empty-reason="No terminal close review is active.">
                -
              </span>
            ),
          },
          {
            label: "Session",
            value: closeSession?.session_id ?? (
              <span data-tooltip-empty-reason="No terminal close review is active.">
                -
              </span>
            ),
          },
          {
            label: "Command",
            value: closeSession ? (
              <span>{formatArgv(closeSession.argv) || "terminal"}</span>
            ) : (
              <span data-tooltip-empty-reason="No terminal close review is active.">
                -
              </span>
            ),
          },
        ]}
        onCancel={() => setCloseSession(null)}
        onConfirm={() => void confirmTerminalClose()}
        open={closeSession !== null}
        pending={closePending}
        title="Confirm terminal close"
        tone="danger"
      />
    </div>
  );
}

function XtermReplay({
  autoFocus = false,
  inputEnabled,
  label,
  onData,
  onResize,
  resetKey,
  text,
}: {
  autoFocus?: boolean;
  inputEnabled: boolean;
  label: string;
  onData: (data: string) => void;
  onResize: (cols: number, rows: number) => void;
  resetKey: string;
  text: string;
}) {
  const shellRef = useRef<HTMLDivElement | null>(null);
  const fitHostRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const onDataRef = useRef(onData);
  const onResizeRef = useRef(onResize);
  const inputEnabledRef = useRef(inputEnabled);
  const renderedKeyRef = useRef<string | null>(null);
  const renderedTextRef = useRef("");
  onDataRef.current = onData;
  onResizeRef.current = onResize;
  inputEnabledRef.current = inputEnabled;

  useEffect(() => {
    if (!fitHostRef.current) {
      return;
    }
    const terminal = new Terminal({
      convertEol: true,
      cursorBlink: false,
      disableStdin: true,
      fontFamily:
        'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      rows: 18,
      theme: {
        background: consolePalette.neutral.text,
        foreground: consolePalette.neutral.terminalForeground,
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(fitHostRef.current);
    const dataSubscription = terminal.onData((data) => onDataRef.current(data));
    let resizeTimer: number | null = null;
    const terminalResizeSubscription = terminal.onResize(({ cols, rows }) => {
      if (resizeTimer !== null) {
        window.clearTimeout(resizeTimer);
      }
      resizeTimer = window.setTimeout(() => {
        resizeTimer = null;
        if (inputEnabledRef.current) {
          onResizeRef.current(cols, rows);
        }
      }, 120);
    });
    terminalRef.current = terminal;
    fitRef.current = fit;
    window.setTimeout(() => fit.fit(), 0);
    const resize = () => fit.fit();
    const resizeObserver = new ResizeObserver(() => fit.fit());
    resizeObserver.observe(fitHostRef.current);
    const blurWhenPointerLeavesTerminal = (event: PointerEvent) => {
      const container = shellRef.current;
      if (
        !container ||
        !(event.target instanceof Node) ||
        container.contains(event.target)
      ) {
        return;
      }
      const input = container.querySelector<HTMLElement>(
        ".xterm-helper-textarea",
      );
      if (input && document.activeElement === input) {
        input.blur();
      }
    };
    document.addEventListener(
      "pointerdown",
      blurWhenPointerLeavesTerminal,
      true,
    );
    window.addEventListener("resize", resize);
    return () => {
      resizeObserver.disconnect();
      document.removeEventListener(
        "pointerdown",
        blurWhenPointerLeavesTerminal,
        true,
      );
      window.removeEventListener("resize", resize);
      dataSubscription.dispose();
      terminalResizeSubscription.dispose();
      if (resizeTimer !== null) {
        window.clearTimeout(resizeTimer);
      }
      terminal.dispose();
      terminalRef.current = null;
      fitRef.current = null;
    };
  }, []);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) {
      return;
    }
    terminal.options.disableStdin = !inputEnabled;
    terminal.options.cursorBlink = inputEnabled;
    if (inputEnabled && autoFocus) {
      window.requestAnimationFrame(() => terminal.focus());
    }
    if (inputEnabled) {
      window.requestAnimationFrame(() => fitRef.current?.fit());
    }
  }, [autoFocus, inputEnabled]);

  useEffect(() => {
    const terminal = terminalRef.current;
    if (!terminal) {
      return;
    }
    const previousText = renderedTextRef.current;
    if (renderedKeyRef.current !== resetKey || !text.startsWith(previousText)) {
      terminal.reset();
      terminal.write(text);
    } else if (text.length > previousText.length) {
      terminal.write(text.slice(previousText.length));
    }
    renderedKeyRef.current = resetKey;
    renderedTextRef.current = text;
    window.setTimeout(() => fitRef.current?.fit(), 0);
  }, [resetKey, text]);

  return (
    <div aria-label={label} className="xtermReplay" ref={shellRef}>
      <div className="xtermFitHost" ref={fitHostRef} />
    </div>
  );
}

type TerminalReplayPreview = {
  sessionId: string;
  fromSeq: number;
  availableFirstSeq: number | null;
  nextSeq: number;
  chunkCount: number;
  byteCount: number;
  truncated: boolean;
  text: string;
  chunks: TerminalReplayPreviewChunk[];
};

type TerminalReplayPreviewChunk = {
  byteCount: number;
  terminalSeq: number;
  text: string;
};

function toReplayPreview(replay: TerminalReplayRecord): TerminalReplayPreview {
  const decoder = new TextDecoder();
  const chunks = [...replay.chunks]
    .sort((left, right) => left.terminal_seq - right.terminal_seq)
    .map((chunk) => ({
      byteCount: chunk.size_bytes,
      terminalSeq: chunk.terminal_seq,
      text: chunk.data_base64
        ? decoder.decode(base64ToBytes(chunk.data_base64), { stream: true })
        : "",
    }));
  const trailingText = decoder.decode();
  if (trailingText && chunks.length > 0) {
    chunks[chunks.length - 1]!.text += trailingText;
  }
  return {
    sessionId: replay.session_id,
    fromSeq: replay.from_seq,
    availableFirstSeq: replay.available_first_seq,
    nextSeq: replay.next_seq,
    chunkCount: replay.chunk_count,
    byteCount: replay.byte_count,
    truncated: replay.truncated,
    text: chunks.map((chunk) => chunk.text).join(""),
    chunks,
  };
}

function toStreamReplayPreview(
  stream: TerminalStreamSnapshot,
  sessionId: string,
): TerminalReplayPreview {
  return {
    sessionId,
    fromSeq: stream.firstSeq,
    availableFirstSeq: stream.availableFirstSeq,
    nextSeq: stream.nextSeq,
    chunkCount: stream.chunkCount,
    byteCount: stream.byteCount,
    truncated: stream.truncated,
    text: stream.text,
    chunks: [],
  };
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function clampNumber(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
}

function formatArgv(argv: string[]): string {
  return argv.join(" ");
}

function formatWindow(session: TerminalSessionRecord): string {
  if (!session.cols || !session.rows) {
    return "Size not reported";
  }
  return `${session.cols} cols x ${session.rows} rows`;
}

function formatLimits(session: TerminalSessionRecord): string {
  const idle = session.idle_timeout_secs
    ? `Idle timeout ${formatDuration(session.idle_timeout_secs)}`
    : "Idle timeout -";
  const flow = session.flow_window_bytes
    ? `${formatBytes(session.flow_window_bytes)} flow window`
    : "Flow window -";
  return `${idle}; ${flow}`;
}

function formatOutputRange(session: TerminalSessionRecord): string {
  if (session.output_next_seq === null) {
    return "No output retained";
  }
  const first =
    session.output_retained_first_seq ??
    session.output_first_seq ??
    session.output_next_seq;
  return formatReplaySequence(first, session.output_next_seq);
}

function formatReplaySequence(first: number, next: number): string {
  const last = next - 1;
  if (last < first) {
    return `Next seq ${next}; no retained chunks`;
  }
  if (last === first) {
    return `Seq ${first} retained`;
  }
  return `Seq ${first}-${last} retained, next ${next}`;
}

function formatOutputRetention(session: TerminalSessionRecord): string {
  const input = formatLastInput(session);
  const retained =
    session.output_retained_bytes === null
      ? "retained -"
      : `${formatBytes(session.output_retained_bytes)} kept`;
  if (!session.output_dropped_bytes) {
    return `${input}; ${retained}`;
  }
  const chunks = session.output_dropped_chunks
    ? `, ${session.output_dropped_chunks} chunks`
    : "";
  const replay = session.output_replay_truncated ? "; replay truncated" : "";
  return `${input}; ${formatBytes(session.output_dropped_bytes)} dropped${chunks}${replay}`;
}

function isTerminalActive(session: TerminalSessionRecord): boolean {
  return session.state === "open";
}

function isTerminalSocketStateOpen(state: string): boolean {
  return state === "opening" || state === "open";
}

function formatSessionLifecycle(session: TerminalSessionRecord): string {
  switch (session.state) {
    case "opening":
      return `Opening session - ${session.last_status}`;
    case "open":
      return `Active session - ${session.last_status}`;
    case "closed":
    case "exited":
      return `Closed session${session.close_reason ? ` - ${session.close_reason}` : ""}`;
    case "missing":
      return `Session missing${session.close_reason ? ` - ${session.close_reason}` : ""}`;
    case "rejected":
      return `Session rejected${session.close_reason ? ` - ${session.close_reason}` : ""}`;
    case "failed":
      return `Session failed${session.close_reason ? ` - ${session.close_reason}` : ""}`;
    default:
      return `${session.state} - ${session.last_status}`;
  }
}

function formatShellContext(session: TerminalSessionRecord): string {
  const cwd = session.cwd ?? "cwd not reported";
  return `${cwd} - ${formatWindow(session)}`;
}

function formatLastInput(session: TerminalSessionRecord): string {
  return session.last_input_seq <= 0
    ? "No input recorded"
    : `Last input seq ${session.last_input_seq}`;
}

function formatDuration(value: number): string {
  if (value >= 3600) {
    const hours = value / 3600;
    return `${Number.isInteger(hours) ? hours : hours.toFixed(1)}h`;
  }
  if (value >= 60) {
    const minutes = value / 60;
    return `${Number.isInteger(minutes) ? minutes : minutes.toFixed(1)}m`;
  }
  return `${value}s`;
}

function formatBytes(value: number): string {
  if (value >= 1024 * 1024) {
    return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
  }
  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KiB`;
  }
  return `${value} B`;
}

function getFocusableElements(container: HTMLElement): HTMLElement[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    ),
  ).filter(
    (element) =>
      !element.hidden &&
      element.getAttribute("aria-hidden") !== "true" &&
      element.getClientRects().length > 0,
  );
}
