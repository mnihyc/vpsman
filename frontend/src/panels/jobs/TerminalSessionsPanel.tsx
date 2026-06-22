import { History, Keyboard, LogIn, Maximize2, Radio, RefreshCw, TerminalSquare, XCircle } from "lucide-react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import { terminalSessionStateBadgeClass } from "../../jobStatusPresentation";
import type { TerminalAction } from "../jobDispatchModel";
import type { WsTerminalOutputEvent } from "../../types";
import type { TerminalReplayRecord, TerminalSessionRecord } from "../../typesTerminal";
import { formatTime, shortId } from "../../utils";

export function TerminalSessionsPanel({
  clientLabel,
  sessions,
  lastTerminalOutputEvent,
  loading,
  onPrepareAction,
  onReplay,
  onRefresh,
}: {
  clientLabel: (clientId: string) => string;
  sessions: TerminalSessionRecord[];
  lastTerminalOutputEvent: WsTerminalOutputEvent | null;
  loading: boolean;
  onPrepareAction: (session: TerminalSessionRecord, action: TerminalAction) => void;
  onReplay: (clientId: string, sessionId: string, fromSeq?: number) => Promise<TerminalReplayRecord>;
  onRefresh: () => void;
}) {
  const [replayPreview, setReplayPreview] = useState<TerminalReplayPreview | null>(null);
  const [replayPendingKey, setReplayPendingKey] = useState<string | null>(null);
  const [replayError, setReplayError] = useState<string | null>(null);
  const [followKey, setFollowKey] = useState<string | null>(null);
  const [activeKey, setActiveKey] = useState<string | null>(null);
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
            <small>{shortId(session.session_id)}</small>
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
          <small>{session.last_status}</small>
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
          <small>{session.cwd ?? session.last_event}</small>
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
          <span className="rowActions compactRowActions">
            <button
              aria-label={`${following ? "Stop following" : "Follow"} terminal session ${shortId(session.session_id)}`}
              className={`iconButton ${following ? "activeAction" : ""}`}
              disabled={session.output_next_seq === null}
              onClick={(event) => {
                event.stopPropagation();
                toggleFollow(session);
              }}
              title="Live follow persisted terminal output"
              type="button"
            >
              <Radio size={13} />
            </button>
            <button
              aria-label={`Durable replay terminal session ${shortId(session.session_id)}`}
              className="iconButton"
              disabled={session.output_next_seq === null || replayPendingKey === key}
              onClick={(event) => {
                event.stopPropagation();
                void loadDurableReplay(session);
              }}
              title="Load persisted replay from server job output history"
              type="button"
            >
              <History size={13} />
            </button>
            <button
              aria-label={`Attach terminal session ${shortId(session.session_id)}`}
              className="iconButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "open");
              }}
              title="Prepare attach/replay"
              type="button"
            >
              <LogIn size={13} />
            </button>
            <button
              aria-label={`Poll terminal session ${shortId(session.session_id)}`}
              className="iconButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "poll");
              }}
              title="Prepare output poll"
              type="button"
            >
              <RefreshCw size={13} />
            </button>
            <button
              aria-label={`Input terminal session ${shortId(session.session_id)}`}
              className="iconButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "input");
              }}
              title="Prepare input"
              type="button"
            >
              <Keyboard size={13} />
            </button>
            <button
              aria-label={`Resize terminal session ${shortId(session.session_id)}`}
              className="iconButton"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "resize");
              }}
              title="Prepare resize"
              type="button"
            >
              <Maximize2 size={13} />
            </button>
            <button
              aria-label={`Close terminal session ${shortId(session.session_id)}`}
              className="iconButton dangerAction"
              disabled={!active}
              onClick={(event) => {
                event.stopPropagation();
                onPrepareAction(session, "close");
              }}
              title="Prepare close"
              type="button"
            >
              <XCircle size={13} />
            </button>
          </span>
        );
      },
      enableHiding: false,
      header: "Actions",
      id: "actions",
      minSize: 240,
      size: 260,
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

  return (
    <div className="fleetPanel terminalSessionsPanel">
      <div className="sectionHeader">
        <div>
          <h2>Terminal sessions</h2>
          <span>{replayError ?? `${sessions.length} retained terminal states`}</span>
        </div>
        <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
          <RefreshCw size={14} />
          <span>Refresh</span>
        </button>
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
          <strong>{followKey ? "Live" : "Idle"}</strong>
          <small>Follow state</small>
        </span>
      </div>
      <div className="terminalWorkspace">
        <div className="terminalActiveHeader">
          <div>
            <strong>{activeSession ? clientLabel(activeSession.client_id) : "No active terminal"}</strong>
            <span>
              {activeSession
                ? `${shortId(activeSession.session_id)} · ${formatArgv(activeSession.argv) || activeSession.last_command_type}`
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
              disabled={!activeSession || Boolean(activeSession.session_exited) || activeSession.state === "closed"}
              onClick={() => activeSession && onPrepareAction(activeSession, "input")}
              type="button"
            >
              <Keyboard size={13} />
              <span>Input</span>
            </button>
          </div>
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
            <span>Last event</span>
            <strong>{session.last_event}</strong>
          </div>
        )}
        rows={sessions}
        searchPlaceholder="Search terminal sessions"
        selectable={false}
        storageKey="vpsman.jobs.terminalSessions"
        title="Terminal records"
      />
      {replayPreview && (
        <div className="terminalReplayPreview" aria-label="Durable terminal replay preview">
          <div>
            <strong>
              Durable replay {shortId(replayPreview.sessionId)}: {replayPreview.chunkCount} chunks,{" "}
              {formatBytes(replayPreview.byteCount)}
            </strong>
            <span>
              seq {replayPreview.availableFirstSeq ?? replayPreview.fromSeq} -&gt; {replayPreview.nextSeq}
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
      fontFamily: '"Roboto Mono", "SFMono-Regular", Consolas, monospace',
      fontSize: 12,
      rows: 18,
      theme: {
        background: "#111827",
        foreground: "#e5e7eb",
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

function formatArgv(argv: string[]): string {
  return argv.join(" ");
}

function formatWindow(session: TerminalSessionRecord): string {
  if (!session.cols || !session.rows) {
    return "Size not reported";
  }
  return `${session.cols} x ${session.rows}`;
}

function formatLimits(session: TerminalSessionRecord): string {
  const idle = session.idle_timeout_secs ? `${session.idle_timeout_secs}s idle` : "idle n/a";
  const flow = session.flow_window_bytes ? `${formatBytes(session.flow_window_bytes)} flow` : "flow n/a";
  return `${idle}; ${flow}`;
}

function formatOutputRange(session: TerminalSessionRecord): string {
  if (session.output_next_seq === null) {
    return "No output retained";
  }
  const first = session.output_first_seq ?? session.output_next_seq;
  return `${first} -> ${session.output_next_seq}`;
}

function formatOutputRetention(session: TerminalSessionRecord): string {
  const input = session.last_input_seq === null ? "no input" : `input ${session.last_input_seq}`;
  const retained =
    session.output_retained_bytes === null ? "retained n/a" : `${formatBytes(session.output_retained_bytes)} kept`;
  if (!session.output_dropped_bytes) {
    return `${input}; ${retained}`;
  }
  const chunks = session.output_dropped_chunks ? `, ${session.output_dropped_chunks} chunks` : "";
  const replay = session.output_replay_truncated ? "; replay truncated" : "";
  return `${input}; ${formatBytes(session.output_dropped_bytes)} dropped${chunks}${replay}`;
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
