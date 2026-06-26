import {
  Copy,
  Download,
  History,
  Keyboard,
  LogIn,
  Maximize2,
  Play,
  Radio,
  RefreshCw,
  TerminalSquare,
  XCircle,
} from "lucide-react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { VpsCombobox } from "../../components/VpsCombobox";
import { consolePalette } from "../../colorPalette";
import { terminalSessionStateBadgeClass } from "../../jobStatusPresentation";
import type { TerminalAction } from "../jobDispatchModel";
import type { AgentView, WsTerminalOutputEvent } from "../../types";
import type { TerminalReplayRecord, TerminalSessionRecord } from "../../typesTerminal";
import { formatTime, shortId } from "../../utils";
import type { TerminalComposerAction } from "../JobDispatchPanel";

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

export function TerminalSessionsPanel({
  agents,
  clientLabel,
  sessions,
  lastTerminalOutputEvent,
  loading,
  onOpenSessionEvidence,
  onPrepareAction,
  onReplay,
  onRefresh,
}: {
  agents: AgentView[];
  clientLabel: (clientId: string) => string;
  sessions: TerminalSessionRecord[];
  lastTerminalOutputEvent: WsTerminalOutputEvent | null;
  loading: boolean;
  onOpenSessionEvidence?: () => void;
  onPrepareAction: (
    session: TerminalSessionRecord,
    action: TerminalAction,
    options?: Omit<TerminalComposerAction, "action" | "requestId" | "session">,
  ) => void;
  onReplay: (clientId: string, sessionId: string, fromSeq?: number) => Promise<TerminalReplayRecord>;
  onRefresh: () => void;
}) {
  const [launchTargetId, setLaunchTargetId] = useState("");
  const [launchProfile, setLaunchProfile] = useState<TerminalLaunchProfile>("posix-login");
  const [launchCwd, setLaunchCwd] = useState("");
  const [launchUser, setLaunchUser] = useState<TerminalLaunchUser>("agent");
  const [launchIdleTimeoutSecs, setLaunchIdleTimeoutSecs] = useState(3600);
  const [launchCols, setLaunchCols] = useState(120);
  const [launchRows, setLaunchRows] = useState(40);
  const [launchStatus, setLaunchStatus] = useState<string | null>(null);
  const [replayPreview, setReplayPreview] = useState<TerminalReplayPreview | null>(null);
  const [replayPendingKey, setReplayPendingKey] = useState<string | null>(null);
  const [replayError, setReplayError] = useState<string | null>(null);
  const [followKey, setFollowKey] = useState<string | null>(null);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const launchTarget = agents.find((agent) => agent.id === launchTargetId) ?? agents[0] ?? null;
  const launchProfileRecord =
    TERMINAL_LAUNCH_PROFILES.find((profile) => profile.value === launchProfile) ?? TERMINAL_LAUNCH_PROFILES[0];
  const activeSession = useMemo(
    () =>
      sessions.find((session) => `${session.client_id}:${session.session_id}` === activeKey) ??
      sessions.find((session) => !session.session_exited && session.state !== "closed") ??
      sessions[0] ??
      null,
    [activeKey, sessions],
  );
  const openSessions = sessions.filter((session) => !session.session_exited && session.state !== "closed").length;
  const replayableSessions = sessions.filter((session) => session.output_next_seq !== null).length;
  const retainedBytes = sessions.reduce((total, session) => total + (session.output_retained_bytes ?? 0), 0);
  const terminalSummary =
    replayError ?? `${openSessions} open, ${replayableSessions} replayable, ${formatBytes(retainedBytes)} retained`;
  const activeReplay =
    replayPreview && activeSession?.session_id === replayPreview.sessionId
      ? replayPreview
      : null;
  const transcriptUnavailableReason = activeSession
    ? activeReplay?.text
      ? null
      : "Load Replay first; full transcript export endpoint is not exposed by the terminal API."
    : "Select a terminal session before copying or downloading transcript text.";
  const terminalColumns: ConsoleDataGridColumn<TerminalSessionRecord>[] = [
    {
      cell: (session) => {
        const key = `${session.client_id}:${session.session_id}`;
        const selected = activeSession?.client_id === session.client_id && activeSession.session_id === session.session_id;
        return (
          <span className="historyPrimary">
            <button
              className={`linkLikeButton ${selected ? "activeAction" : ""}`}
              onClick={(event) => {
                event.stopPropagation();
                setActiveKey(key);
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
      searchValue: (session) => `${clientLabel(session.client_id)} ${session.client_id} ${session.session_id}`,
      sortValue: (session) => `${clientLabel(session.client_id)}:${session.session_id}`,
    },
    {
      cell: (session) => (
        <span className="historyPrimary">
          <span className={`status ${terminalSessionStateBadgeClass(session.state)}`}>{session.state}</span>
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
          <strong title={formatArgv(session.argv)}>{formatArgv(session.argv) || session.last_command_type}</strong>
          <small>{formatShellContext(session)}</small>
        </span>
      ),
      header: "Command",
      id: "command",
      searchValue: (session) => `${formatArgv(session.argv)} ${session.last_command_type} ${session.cwd ?? ""}`,
      sortValue: (session) => formatArgv(session.argv) || session.last_command_type,
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
      searchValue: (session) => `${formatWindow(session)} ${formatLimits(session)}`,
      sortValue: (session) => formatWindow(session),
    },
    {
      cell: (session) => (
        <span className="historyPrimary">
          <strong>{formatOutputRange(session)}</strong>
          <small className={session.output_dropped_bytes || session.output_replay_truncated ? "terminalWarning" : undefined}>
            {formatOutputRetention(session)}
          </small>
        </span>
      ),
      header: "Output",
      id: "output",
      searchValue: (session) => `${formatOutputRange(session)} ${formatOutputRetention(session)}`,
      sortValue: (session) => session.output_next_seq ?? 0,
    },
    {
      cell: (session) => {
        const active = !session.session_exited && session.state !== "closed";
        const key = `${session.client_id}:${session.session_id}`;
        const following = followKey === key;
        return (
          <span className="terminalRowActions" aria-label={`Terminal session ${shortId(session.session_id)} controls`}>
            <button
              aria-label={`${following ? "Stop following" : "Follow"} terminal session ${shortId(session.session_id)}`}
              className={`terminalActionButton ${following ? "activeAction" : ""}`}
              disabled={session.output_next_seq === null}
              onClick={(event) => {
                event.stopPropagation();
                toggleFollow(session);
              }}
              title="Follow persisted output as new terminal chunks arrive"
              type="button"
            >
              <Radio size={13} />
              <span>{following ? "Stop follow" : "Follow"}</span>
            </button>
            <button
              aria-label={`Durable replay terminal session ${shortId(session.session_id)}`}
              className="terminalActionButton"
              disabled={session.output_next_seq === null || replayPendingKey === key}
              onClick={(event) => {
                event.stopPropagation();
                void loadDurableReplay(session);
              }}
              title="Load durable replay from retained terminal output"
              type="button"
            >
              <History size={13} />
              <span>Replay</span>
            </button>
            <button
              aria-label={`Attach terminal session ${shortId(session.session_id)}`}
              className="terminalActionButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "open");
              }}
              title="Attach to this active terminal session"
              type="button"
            >
              <LogIn size={13} />
              <span>Attach</span>
            </button>
            <button
              aria-label={`Poll terminal session ${shortId(session.session_id)}`}
              className="terminalActionButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "poll");
              }}
              title="Poll retained terminal output"
              type="button"
            >
              <RefreshCw size={13} />
              <span>Poll</span>
            </button>
            <button
              aria-label={`Input terminal session ${shortId(session.session_id)}`}
              className="terminalActionButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "input");
              }}
              title="Send input to this terminal session"
              type="button"
            >
              <Keyboard size={13} />
              <span>Input</span>
            </button>
            <button
              aria-label={`Resize terminal session ${shortId(session.session_id)}`}
              className="terminalActionButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "resize");
              }}
              title="Resize this terminal session"
              type="button"
            >
              <Maximize2 size={13} />
              <span>Resize</span>
            </button>
            <button
              aria-label={`Close terminal session ${shortId(session.session_id)}`}
              className="terminalActionButton dangerAction"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "close");
              }}
              title="Close this terminal session after review"
              type="button"
            >
              <XCircle size={13} />
              <span>Close</span>
            </button>
          </span>
        );
      },
      enableHiding: false,
      header: "Session controls",
      id: "actions",
      minSize: 420,
      size: 460,
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
    if (!agents.some((agent) => agent.id === launchTargetId)) {
      setLaunchTargetId(agents[0].id);
    }
  }, [agents, launchTargetId]);

  useEffect(() => {
    if (!lastTerminalOutputEvent || !followKey) {
      return;
    }
    const eventKey = `${lastTerminalOutputEvent.client_id}:${lastTerminalOutputEvent.session_id}`;
    if (eventKey !== followKey) {
      return;
    }
    void loadLiveReplay(lastTerminalOutputEvent.client_id, lastTerminalOutputEvent.session_id);
  }, [lastTerminalOutputEvent, followKey]);

  async function loadDurableReplay(session: TerminalSessionRecord) {
    const key = `${session.client_id}:${session.session_id}`;
    setActiveKey(key);
    setReplayPendingKey(key);
    setReplayError(null);
    try {
      const fromSeq = session.output_first_seq ?? session.output_retained_first_seq ?? 1;
      const replay = await onReplay(session.client_id, session.session_id, fromSeq);
      setReplayPreview(toReplayPreview(replay));
    } catch (error) {
      setReplayError(error instanceof Error ? error.message : "Terminal replay unavailable");
    } finally {
      setReplayPendingKey(null);
    }
  }

  async function loadLiveReplay(clientId: string, sessionId: string) {
    const key = `${clientId}:${sessionId}`;
    setReplayError(null);
    try {
      const fromSeq = replayPreview?.sessionId === sessionId ? replayPreview.nextSeq : 1;
      const replay = await onReplay(clientId, sessionId, fromSeq);
      setReplayPreview((current) => mergeReplayPreview(current, toReplayPreview(replay)));
    } catch (error) {
      setReplayError(error instanceof Error ? error.message : "Terminal live replay unavailable");
      setFollowKey((current) => (current === key ? null : current));
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
    const blob = new Blob([activeReplay.text], { type: "text/plain;charset=utf-8" });
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
    setActiveKey(key);
    if (followKey === key) {
      setFollowKey(null);
      return;
    }
    setFollowKey(key);
    void loadDurableReplay(session);
  }

  function prepareNewTerminal() {
    if (!launchTarget) {
      setLaunchStatus("Select a VPS before preparing a terminal review.");
      return;
    }
    const now = new Date().toISOString();
    const session: TerminalSessionRecord = {
      session_id: crypto.randomUUID(),
      client_id: launchTarget.id,
      state: "open",
      last_status: "opened",
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
      last_input_seq: null,
      session_exited: false,
      close_reason: null,
      last_event: "terminal_open",
      last_job_id: "pending-review",
      last_command_type: "terminal_open",
      last_seq: 0,
      observed_at: now,
    };
    onPrepareAction(session, "open", {
      maxTimeoutSecs: clampNumber(launchIdleTimeoutSecs, 10, 86400),
      terminalReplayFromSeq: "",
      terminalUser: launchUser === "agent" ? "" : "root",
      terminalUserPolicy: launchUser === "root-fallback" ? "fallback" : "fail",
    });
    setLaunchStatus(`${clientLabel(launchTarget.id)} terminal review prepared below.`);
  }

  return (
    <div className="fleetPanel terminalSessionsPanel">
      <div className="sectionHeader">
        <div>
          <h2>Terminal sessions</h2>
          <span>{terminalSummary}</span>
        </div>
        <div className="rowActions compactRowActions">
          {onOpenSessionEvidence && (
            <button className="secondaryAction compactAction" onClick={onOpenSessionEvidence} type="button">
              <History size={14} />
              <span>Audit evidence</span>
            </button>
          )}
          <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
            <RefreshCw size={14} />
            <span>Refresh</span>
          </button>
        </div>
      </div>
      <div className="terminalLaunchPanel" aria-label="New terminal composer">
        <div className="terminalLaunchIntro">
          <div>
            <h3>New terminal</h3>
            <span>Prepare one reviewed browser terminal without leaving Remote Operations.</span>
          </div>
          <strong>Audited terminal_open</strong>
        </div>
        <div className="terminalLaunchGrid">
          <label className="wideField">
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
          <label>
            <span>Shell profile</span>
            <select
              aria-label="Terminal shell profile"
              onChange={(event) => setLaunchProfile(event.target.value as TerminalLaunchProfile)}
              value={launchProfile}
            >
              {TERMINAL_LAUNCH_PROFILES.map((profile) => (
                <option key={profile.value} value={profile.value}>
                  {profile.label} - {profile.description}
                </option>
              ))}
            </select>
          </label>
          <label>
            <span>Working directory</span>
            <input
              aria-label="New terminal working directory"
              onChange={(event) => setLaunchCwd(event.target.value)}
              placeholder="Agent default"
              value={launchCwd}
            />
          </label>
          <label>
            <span>Run as</span>
            <select
              aria-label="New terminal user policy"
              onChange={(event) => setLaunchUser(event.target.value as TerminalLaunchUser)}
              value={launchUser}
            >
              <option value="agent">Agent user</option>
              <option value="root">root, fail if unavailable</option>
              <option value="root-fallback">root, fallback to agent user</option>
            </select>
          </label>
          <label>
            <span>Idle timeout</span>
            <input
              aria-label="New terminal idle timeout seconds"
              max={86400}
              min={10}
              onChange={(event) => setLaunchIdleTimeoutSecs(Number(event.target.value))}
              type="number"
              value={launchIdleTimeoutSecs}
            />
          </label>
          <label>
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
          <label>
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
        <div className="terminalLaunchFooter">
          <span>
            {launchStatus ??
              "Review below freezes target, session id, argv, user policy, window, timeout, and privilege intent."}
          </span>
          <button
            className="primaryAction compactAction"
            disabled={!launchTarget}
            onClick={prepareNewTerminal}
            type="button"
          >
            <Play size={15} />
            <span>Prepare terminal review</span>
          </button>
        </div>
      </div>
      <div className="terminalSummaryStrip">
        <span>
          <strong>{openSessions}</strong>
          <small>Open</small>
        </span>
        <span>
          <strong>{replayableSessions}</strong>
          <small>Replayable</small>
        </span>
        <span>
          <strong>{formatBytes(retainedBytes)}</strong>
          <small>Retained output</small>
        </span>
        <span>
          <strong>{followKey ? "Following" : "Not following"}</strong>
          <small>Live follow</small>
        </span>
      </div>
      <div className="terminalWorkspace">
        <div className="terminalActiveHeader">
          <div>
            <strong>{activeSession ? clientLabel(activeSession.client_id) : "No active terminal"}</strong>
            <span>
              {activeSession
                ? `${shortId(activeSession.session_id)} - ${formatArgv(activeSession.argv) || activeSession.last_command_type}`
                : "Open a terminal session to attach retained output"}
            </span>
          </div>
          <div className="rowActions compactRowActions">
            <button
              className="secondaryAction compactAction"
              disabled={!activeSession}
              onClick={() => activeSession && void loadDurableReplay(activeSession)}
              type="button"
            >
              <History size={13} />
              <span>Replay</span>
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={!activeReplay?.text}
              onClick={() => void copyTranscript()}
              title={transcriptUnavailableReason ?? "Copy the loaded retained replay text"}
              type="button"
            >
              <Copy size={13} />
              <span>Copy transcript</span>
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={!activeReplay?.text}
              onClick={downloadTranscript}
              title={transcriptUnavailableReason ?? "Download the loaded retained replay text"}
              type="button"
            >
              <Download size={13} />
              <span>Download transcript</span>
            </button>
            <button
              className="secondaryAction compactAction"
              disabled={!activeSession || Boolean(activeSession.session_exited) || activeSession.state === "closed"}
              onClick={() => activeSession && onPrepareAction(activeSession, "input")}
              type="button"
            >
              <Keyboard size={13} />
              <span>Input</span>
            </button>
          </div>
        </div>
        <div className="terminalTranscriptState" aria-label="Terminal transcript availability">
          {transcriptUnavailableReason ?? "Loaded retained replay can be copied or downloaded from this browser."}
        </div>
        <div className="terminalSessionContext" aria-label="Active terminal session context">
          <span>
            <strong>{activeSession ? formatSessionLifecycle(activeSession) : "No session selected"}</strong>
            <small>Lifecycle</small>
          </span>
          <span>
            <strong>{activeSession ? activeSession.cwd ?? "Working directory not reported" : "-"}</strong>
            <small>Working directory</small>
          </span>
          <span>
            <strong>{activeSession ? formatOutputRange(activeSession) : "-"}</strong>
            <small>Replay range</small>
          </span>
          <span>
            <strong>{activeSession ? formatLastInput(activeSession) : "-"}</strong>
            <small>Input state</small>
          </span>
        </div>
        <XtermReplay
          label="Active terminal emulator"
          text={
            replayPreview && replayPreview.sessionId === activeSession?.session_id
              ? replayPreview.text
              : activeSession
                ? "Select Replay or Follow to load retained output for this session.\r\n"
                : "No terminal session selected.\r\n"
          }
        />
      </div>
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
            <span>Terminal open, input, poll, resize, and close jobs populate this inventory.</span>
          </div>
        }
        renderExpandedRow={(session) => (
          <div className="consoleInlineDetailGrid">
            <span>Session ID</span>
            <strong>{session.session_id}</strong>
            <span>VPS</span>
            <strong>{clientLabel(session.client_id)}</strong>
            <span>Command</span>
            <strong>{formatArgv(session.argv) || session.last_command_type}</strong>
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
            <strong>{session.close_reason ?? (isTerminalActive(session) ? "Open session" : "Not reported")}</strong>
            <span>Last job</span>
            <strong>{session.last_job_id}</strong>
            <span>Last sequence</span>
            <strong>{session.last_seq}</strong>
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
        rows={sessions}
        searchPlaceholder="Search terminal sessions"
        selectable={false}
        storageKey="vpsman.jobs.terminalSessions"
        title="Session inventory and controls"
      />
      {replayPreview && (
        <div className="terminalReplayPreview" aria-label="Durable terminal replay preview">
          <div>
            <strong>
              Durable replay {shortId(replayPreview.sessionId)}: {replayPreview.chunkCount} chunks,{" "}
              {formatBytes(replayPreview.byteCount)}
            </strong>
            <span>
              {formatReplaySequence(replayPreview.availableFirstSeq ?? replayPreview.fromSeq, replayPreview.nextSeq)}
              {replayPreview.truncated ? "; truncated" : ""}
              {followKey?.endsWith(replayPreview.sessionId) ? "; following live output" : ""}
            </span>
          </div>
          <pre>{replayPreview.text || "(no replay text)"}</pre>
        </div>
      )}
    </div>
  );
}

function XtermReplay({ label, text }: { label: string; text: string }) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);

  useEffect(() => {
    if (!containerRef.current) {
      return;
    }
    const terminal = new Terminal({
      convertEol: true,
      cursorBlink: false,
      disableStdin: true,
      fontFamily: 'ui-monospace, "SFMono-Regular", Menlo, Monaco, Consolas, "Liberation Mono", monospace',
      fontSize: 12,
      rows: 18,
      theme: {
        background: consolePalette.neutral.text,
        foreground: consolePalette.neutral.terminalForeground,
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(containerRef.current);
    terminalRef.current = terminal;
    fitRef.current = fit;
    window.setTimeout(() => fit.fit(), 0);
    const resize = () => fit.fit();
    const resizeObserver = new ResizeObserver(() => fit.fit());
    resizeObserver.observe(containerRef.current);
    window.addEventListener("resize", resize);
    return () => {
      resizeObserver.disconnect();
      window.removeEventListener("resize", resize);
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
    terminal.reset();
    terminal.write(text);
    window.setTimeout(() => fitRef.current?.fit(), 0);
  }, [text]);

  return <div aria-label={label} className="xtermReplay" ref={containerRef} />;
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
};

function toReplayPreview(replay: TerminalReplayRecord): TerminalReplayPreview {
  const chunks = replay.chunks
    .map((chunk) => (chunk.data_base64 ? base64ToBytes(chunk.data_base64) : new Uint8Array()))
    .filter((chunk) => chunk.length > 0);
  const totalBytes = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const bytes = new Uint8Array(totalBytes);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return {
    sessionId: replay.session_id,
    fromSeq: replay.from_seq,
    availableFirstSeq: replay.available_first_seq,
    nextSeq: replay.next_seq,
    chunkCount: replay.chunk_count,
    byteCount: replay.byte_count,
    truncated: replay.truncated,
    text: new TextDecoder().decode(bytes).slice(0, 2000),
  };
}

function mergeReplayPreview(
  current: TerminalReplayPreview | null,
  next: TerminalReplayPreview,
): TerminalReplayPreview {
  if (!current || current.sessionId !== next.sessionId || next.fromSeq <= 1) {
    return next;
  }
  const text = `${current.text}${next.text}`.slice(-32_000);
  return {
    sessionId: next.sessionId,
    fromSeq: current.fromSeq,
    availableFirstSeq: current.availableFirstSeq ?? next.availableFirstSeq,
    nextSeq: Math.max(current.nextSeq, next.nextSeq),
    chunkCount: current.chunkCount + next.chunkCount,
    byteCount: current.byteCount + next.byteCount,
    truncated: current.truncated || next.truncated,
    text,
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
  const idle = session.idle_timeout_secs ? `Idle timeout ${formatDuration(session.idle_timeout_secs)}` : "Idle timeout n/a";
  const flow = session.flow_window_bytes ? `${formatBytes(session.flow_window_bytes)} flow window` : "Flow window n/a";
  return `${idle}; ${flow}`;
}

function formatOutputRange(session: TerminalSessionRecord): string {
  if (session.output_next_seq === null) {
    return "No output retained";
  }
  const first = session.output_first_seq ?? session.output_next_seq;
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
    session.output_retained_bytes === null ? "retained n/a" : `${formatBytes(session.output_retained_bytes)} kept`;
  if (!session.output_dropped_bytes) {
    return `${input}; ${retained}`;
  }
  const chunks = session.output_dropped_chunks ? `, ${session.output_dropped_chunks} chunks` : "";
  const replay = session.output_replay_truncated ? "; replay truncated" : "";
  return `${input}; ${formatBytes(session.output_dropped_bytes)} dropped${chunks}${replay}`;
}

function isTerminalActive(session: TerminalSessionRecord): boolean {
  return !session.session_exited && session.state !== "closed";
}

function formatSessionLifecycle(session: TerminalSessionRecord): string {
  if (isTerminalActive(session)) {
    return `Active session - ${session.last_status}`;
  }
  const reason = session.close_reason ? ` - ${session.close_reason}` : "";
  return `Closed session${reason}`;
}

function formatShellContext(session: TerminalSessionRecord): string {
  const cwd = session.cwd ?? "cwd not reported";
  return `${cwd} - ${formatWindow(session)}`;
}

function formatLastInput(session: TerminalSessionRecord): string {
  return session.last_input_seq === null ? "No input recorded" : `Last input seq ${session.last_input_seq}`;
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
