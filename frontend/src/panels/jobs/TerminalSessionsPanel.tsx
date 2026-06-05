import { History, Keyboard, LogIn, Maximize2, Radio, RefreshCw, TerminalSquare, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import { CrudPager } from "../../components/CrudPager";
import type { TerminalAction } from "../jobDispatchModel";
import type { WsTerminalOutputEvent } from "../../types";
import type { TerminalReplayRecord, TerminalSessionRecord } from "../../typesTerminal";
import { formatTime, shortId, statusClass } from "../../utils";

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
    if (followKey === key) {
      setFollowKey(null);
      return;
    }
    setFollowKey(key);
    void loadDurableReplay(session);
  }

  return (
    <div className="fleetPanel">
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
      <CrudPager
        fields={[
          { label: "VPS", value: (session) => clientLabel(session.client_id) },
          { label: "Session", value: (session) => session.session_id },
          { label: "State", value: (session) => `${session.state} ${session.last_status}` },
          { label: "Command", value: (session) => `${formatArgv(session.argv)} ${session.last_command_type}` },
          { label: "Output", value: (session) => `${formatOutputRange(session)} ${formatOutputRetention(session)}` },
        ]}
        itemLabel="sessions"
        items={sessions}
        pageSize={8}
        title="Terminal records"
        empty={
          <div className="emptyState">
            <TerminalSquare size={22} />
            <strong>No terminal sessions</strong>
            <span>Terminal open, input, poll, resize, and close jobs populate this inventory.</span>
          </div>
        }
      >
        {(rows) => (
          <div className="table historyTable">
            <div className="historyRow heading terminalSessionGrid">
              <span>Session</span>
              <span>State</span>
              <span>Command</span>
              <span>Window</span>
              <span>Output</span>
              <span>Actions</span>
              <span>Observed</span>
            </div>
            {rows.map((session) => {
              const active = !session.session_exited && session.state !== "closed";
              const key = `${session.client_id}:${session.session_id}`;
              const following = followKey === key;
              return (
                <div className="historyRow terminalSessionGrid" key={`${session.client_id}:${session.session_id}`}>
                  <span className="historyPrimary">
                    <strong>{clientLabel(session.client_id)}</strong>
                    <small>{shortId(session.session_id)}</small>
                  </span>
                  <span className="historyPrimary">
                    <span className={`status ${statusClass(session.state)}`}>{session.state}</span>
                    <small>{session.last_status}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong title={formatArgv(session.argv)}>{formatArgv(session.argv) || session.last_command_type}</strong>
                    <small>{session.cwd ?? session.last_event}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{formatWindow(session)}</strong>
                    <small>{formatLimits(session)}</small>
                  </span>
                  <span className="historyPrimary">
                    <strong>{formatOutputRange(session)}</strong>
                    <small className={session.output_dropped_bytes || session.output_replay_truncated ? "terminalWarning" : undefined}>
                      {formatOutputRetention(session)}
                    </small>
                  </span>
                  <span className="rowActions compactRowActions">
                    <button
                      aria-label={`${following ? "Stop following" : "Follow"} terminal session ${shortId(session.session_id)}`}
                      className={`secondaryAction compactAction ${following ? "activeAction" : ""}`}
                      disabled={session.output_next_seq === null}
                      onClick={() => toggleFollow(session)}
                      title="Live follow persisted terminal output"
                      type="button"
                    >
                      <Radio size={13} />
                      <span>{following ? "Live" : "Follow"}</span>
                    </button>
                    <button
                      aria-label={`Durable replay terminal session ${shortId(session.session_id)}`}
                      className="secondaryAction compactAction"
                      disabled={session.output_next_seq === null || replayPendingKey === `${session.client_id}:${session.session_id}`}
                      onClick={() => void loadDurableReplay(session)}
                      title="Load persisted replay from server job output history"
                      type="button"
                    >
                      <History size={13} />
                      <span>Replay</span>
                    </button>
                    <button
                      aria-label={`Attach terminal session ${shortId(session.session_id)}`}
                      className="secondaryAction compactAction"
                      disabled={!active}
                      onClick={() => onPrepareAction(session, "open")}
                      title="Prepare attach/replay"
                      type="button"
                    >
                      <LogIn size={13} />
                      <span>Attach</span>
                    </button>
                    <button
                      aria-label={`Poll terminal session ${shortId(session.session_id)}`}
                      className="secondaryAction compactAction"
                      disabled={!active}
                      onClick={() => onPrepareAction(session, "poll")}
                      title="Prepare output poll"
                      type="button"
                    >
                      <RefreshCw size={13} />
                      <span>Poll</span>
                    </button>
                    <button
                      aria-label={`Input terminal session ${shortId(session.session_id)}`}
                      className="secondaryAction compactAction"
                      disabled={!active}
                      onClick={() => onPrepareAction(session, "input")}
                      title="Prepare input"
                      type="button"
                    >
                      <Keyboard size={13} />
                      <span>Input</span>
                    </button>
                    <button
                      aria-label={`Resize terminal session ${shortId(session.session_id)}`}
                      className="secondaryAction compactAction"
                      disabled={!active}
                      onClick={() => onPrepareAction(session, "resize")}
                      title="Prepare resize"
                      type="button"
                    >
                      <Maximize2 size={13} />
                      <span>Resize</span>
                    </button>
                    <button
                      aria-label={`Close terminal session ${shortId(session.session_id)}`}
                      className="secondaryAction compactAction dangerAction"
                      disabled={!active}
                      onClick={() => onPrepareAction(session, "close")}
                      title="Prepare close"
                      type="button"
                    >
                      <XCircle size={13} />
                      <span>Close</span>
                    </button>
                  </span>
                  <span>{formatTime(session.observed_at)}</span>
                </div>
              );
            })}
          </div>
        )}
      </CrudPager>
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
