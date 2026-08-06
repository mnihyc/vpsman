import { useCallback, useEffect, useRef, useState } from "react";
import { MAX_TERMINAL_INPUT_BYTES } from "../generated/protocolContracts";
import type {
  TerminalControlAck,
  TerminalControlAction,
  TerminalSessionRecord,
} from "../typesTerminal";

const MAX_PENDING_INPUT_BYTES = 64 * 1024;
const MAX_PENDING_INPUT_FRAMES = 32;
const MAX_RECONNECT_DELAY_MS = 15_000;

export type TerminalSocketState =
  | "idle"
  | "connecting"
  | "ready"
  | "reconnecting"
  | "closed";

export type TerminalStreamSnapshot = {
  availableFirstSeq: number | null;
  byteCount: number;
  chunkCount: number;
  firstSeq: number;
  nextSeq: number;
  sessionKey: string;
  text: string;
  truncated: boolean;
};

type PendingControl = {
  action: TerminalControlAction;
  inputBytes: number;
  reject?: (reason: Error) => void;
  resolve?: (ack: TerminalControlAck) => void;
};

type TerminalServerFrame =
  | {
      available_first_seq?: number | null;
      from_seq?: number;
      next_seq?: number;
      replay_truncated?: boolean;
      session: TerminalSessionRecord;
      type: "ready";
    }
  | {
      ack: TerminalControlAck;
      type: "control_ack";
    }
  | {
      data_base64: string;
      terminal_seq: number;
      type: "output";
    }
  | {
      session: TerminalSessionRecord;
      type: "session_state";
    }
  | {
      message: string;
      code?: string;
      recoverable?: boolean;
      request_id?: string;
      type: "error";
    };

export function useTerminalSessionSocket({
  accessToken,
  clientId,
  enabled,
  fromSeq,
  sessionId,
}: {
  accessToken: string;
  clientId: string | null;
  enabled: boolean;
  fromSeq: number;
  sessionId: string | null;
}) {
  const sessionKey = clientId && sessionId ? `${clientId}:${sessionId}` : null;
  const [connectionState, setConnectionState] =
    useState<TerminalSocketState>("idle");
  const [feedback, setFeedback] = useState<string | null>(null);
  const [sessionState, setSessionState] = useState<string | null>(null);
  const [sessionRecord, setSessionRecord] =
    useState<TerminalSessionRecord | null>(null);
  const [snapshot, setSnapshot] = useState<TerminalStreamSnapshot | null>(null);
  const socketRef = useRef<WebSocket | null>(null);
  const readyRef = useRef(false);
  const closeQueuedRef = useRef(false);
  const inputBufferRef = useRef(new Uint8Array());
  const inputFrameRef = useRef<number | null>(null);
  const pendingInputBytesRef = useRef(0);
  const pendingInputFramesRef = useRef(0);
  const pendingControlsRef = useRef(new Map<string, PendingControl>());
  const lastResizeRef = useRef<string | null>(null);
  const nextSeqRef = useRef(Math.max(1, Math.trunc(fromSeq)));
  const outputDecoderRef = useRef(new TextDecoder());
  const streamSessionKeyRef = useRef<string | null>(null);

  const rejectPendingControls = useCallback((message: string) => {
    const error = new Error(message);
    for (const pending of pendingControlsRef.current.values()) {
      pending.reject?.(error);
    }
    pendingControlsRef.current.clear();
    pendingInputBytesRef.current = 0;
    pendingInputFramesRef.current = 0;
    inputBufferRef.current = new Uint8Array();
    if (inputFrameRef.current !== null) {
      window.cancelAnimationFrame(inputFrameRef.current);
      inputFrameRef.current = null;
    }
  }, []);

  const sendControl = useCallback(
    (
      action: TerminalControlAction,
      resolve?: (ack: TerminalControlAck) => void,
      reject?: (reason: Error) => void,
    ): boolean => {
      const socket = socketRef.current;
      if (!readyRef.current || !socket || socket.readyState !== WebSocket.OPEN) {
        reject?.(
          new Error(
            "The terminal stream is disconnected. This control was not sent and will not be retried.",
          ),
        );
        return false;
      }
      const requestId = crypto.randomUUID();
      const inputBytes =
        action.type === "input"
          ? base64DecodedLength(action.data_base64)
          : 0;
      pendingControlsRef.current.set(requestId, {
        action,
        inputBytes,
        reject,
        resolve,
      });
      if (action.type === "input") {
        pendingInputFramesRef.current += 1;
      }
      try {
        socket.send(JSON.stringify({ request_id: requestId, ...action }));
        return true;
      } catch {
        pendingControlsRef.current.delete(requestId);
        if (action.type === "input") {
          pendingInputFramesRef.current = Math.max(
            0,
            pendingInputFramesRef.current - 1,
          );
        }
        const error = new Error(
          `Terminal ${action.type} was not sent because the stream disconnected. It was not retried.`,
        );
        setFeedback(error.message);
        reject?.(error);
        return false;
      }
    },
    [],
  );

  const flushInput = useCallback(() => {
    if (inputFrameRef.current !== null) {
      window.cancelAnimationFrame(inputFrameRef.current);
      inputFrameRef.current = null;
    }
    const bytes = inputBufferRef.current;
    inputBufferRef.current = new Uint8Array();
    if (bytes.length === 0) {
      return;
    }
    if (!readyRef.current) {
      pendingInputBytesRef.current = Math.max(
        0,
        pendingInputBytesRef.current - bytes.length,
      );
      setFeedback(
        "Terminal input was not sent because the stream disconnected. It was not retried.",
      );
      return;
    }
    for (
      let offset = 0;
      offset < bytes.length;
      offset += MAX_TERMINAL_INPUT_BYTES
    ) {
      const chunk = bytes.slice(offset, offset + MAX_TERMINAL_INPUT_BYTES);
      const sent = sendControl({
        type: "input",
        data_base64: bytesToBase64(chunk),
      });
      if (!sent) {
        pendingInputBytesRef.current = Math.max(
          0,
          pendingInputBytesRef.current - (bytes.length - offset),
        );
        setFeedback(
          "Terminal input was not sent because the stream disconnected. It was not retried.",
        );
        break;
      }
    }
  }, [sendControl]);

  const queueInput = useCallback(
    (data: string) => {
      if (!data || closeQueuedRef.current) {
        if (closeQueuedRef.current && data) {
          setFeedback("Terminal input is disabled because session close is pending.");
        }
        return;
      }
      if (!readyRef.current) {
        setFeedback(
          "Terminal input is disabled while the stream reconnects. Nothing was sent.",
        );
        return;
      }
      const bytes = new TextEncoder().encode(data);
      const buffered = inputBufferRef.current;
      const nextByteCount = pendingInputBytesRef.current + bytes.length;
      const nextFrameCount =
        pendingInputFramesRef.current +
        Math.ceil((buffered.length + bytes.length) / MAX_TERMINAL_INPUT_BYTES);
      if (
        nextByteCount > MAX_PENDING_INPUT_BYTES ||
        nextFrameCount > MAX_PENDING_INPUT_FRAMES
      ) {
        setFeedback(
          "Terminal input backpressure limit reached. New input was not sent; wait for the session to catch up.",
        );
        return;
      }
      const combined = new Uint8Array(buffered.length + bytes.length);
      combined.set(buffered);
      combined.set(bytes, buffered.length);
      inputBufferRef.current = combined;
      pendingInputBytesRef.current = nextByteCount;
      setFeedback(null);
      if (inputFrameRef.current === null) {
        inputFrameRef.current = window.requestAnimationFrame(flushInput);
      }
    },
    [flushInput],
  );

  const queueResize = useCallback(
    (cols: number, rows: number) => {
      if (!readyRef.current || closeQueuedRef.current) {
        return;
      }
      const dimensions = `${cols}:${rows}`;
      if (lastResizeRef.current === dimensions) {
        return;
      }
      lastResizeRef.current = dimensions;
      if (!sendControl({ type: "resize", cols, rows })) {
        lastResizeRef.current = null;
      }
    },
    [sendControl],
  );

  const closeSession = useCallback(
    (reason: string | null = "operator_closed") =>
      new Promise<TerminalControlAck>((resolve, reject) => {
        if (closeQueuedRef.current) {
          reject(new Error("Terminal close is already pending."));
          return;
        }
        flushInput();
        closeQueuedRef.current = true;
        const sent = sendControl(
          { type: "close", reason },
          resolve,
          (error) => {
            closeQueuedRef.current = false;
            reject(error);
          },
        );
        if (!sent) {
          closeQueuedRef.current = false;
        }
      }),
    [flushInput, sendControl],
  );

  useEffect(() => {
    if (!enabled || !accessToken || !clientId || !sessionId || !sessionKey) {
      setConnectionState("idle");
      setFeedback(null);
      setSessionState(null);
      setSessionRecord(null);
      readyRef.current = false;
      return undefined;
    }

    let disposed = false;
    let reconnectAttempt = 0;
    let reconnectTimer: number | null = null;
    let socket: WebSocket | null = null;
    const continuingSession = streamSessionKeyRef.current === sessionKey;
    const initialFromSeq = continuingSession
      ? nextSeqRef.current
      : Math.max(1, Math.trunc(fromSeq));
    streamSessionKeyRef.current = sessionKey;
    if (!continuingSession) {
      nextSeqRef.current = initialFromSeq;
      outputDecoderRef.current = new TextDecoder();
    }
    closeQueuedRef.current = false;
    lastResizeRef.current = null;
    setFeedback(null);
    setSessionState(null);
    setSessionRecord(null);
    if (!continuingSession) {
      setSnapshot({
        availableFirstSeq: null,
        byteCount: 0,
        chunkCount: 0,
        firstSeq: initialFromSeq,
        nextSeq: initialFromSeq,
        sessionKey,
        text: "",
        truncated: false,
      });
    }

    const scheduleReconnect = () => {
      if (disposed || closeQueuedRef.current) {
        return;
      }
      const delay = Math.min(
        1_000 * 2 ** reconnectAttempt,
        MAX_RECONNECT_DELAY_MS,
      );
      reconnectAttempt += 1;
      setConnectionState("reconnecting");
      reconnectTimer = window.setTimeout(connect, delay);
    };

    const recoverOutputGap = (receivedSeq: number) => {
      setFeedback(
        `Terminal output sequence ${receivedSeq} arrived before expected sequence ${nextSeqRef.current}. Reconnecting from the last contiguous output; no gap is displayed as complete.`,
      );
      readyRef.current = false;
      socket?.close();
    };

    const handleFrame = (frame: TerminalServerFrame) => {
      if (frame.type === "ready") {
        const availableFirstSeq = finiteSequence(frame.available_first_seq);
        if (
          availableFirstSeq !== null &&
          availableFirstSeq > nextSeqRef.current
        ) {
          nextSeqRef.current = availableFirstSeq;
          outputDecoderRef.current = new TextDecoder();
          setFeedback(
            `Earlier terminal output is no longer retained. Streaming resumed at sequence ${availableFirstSeq}.`,
          );
        }
        reconnectAttempt = 0;
        readyRef.current = true;
        setSessionState(frame.session.state);
        setSessionRecord(frame.session);
        setConnectionState("ready");
        setSnapshot((current) =>
          current && current.sessionKey === sessionKey
            ? {
                ...current,
                availableFirstSeq,
                nextSeq: nextSeqRef.current,
                truncated:
                  current.truncated ||
                  Boolean(frame.replay_truncated) ||
                  (availableFirstSeq !== null &&
                    availableFirstSeq > initialFromSeq),
              }
            : current,
        );
        return;
      }
      if (frame.type === "output") {
        const terminalSeq = finiteSequence(frame.terminal_seq);
        if (terminalSeq === null) {
          setFeedback("Terminal stream sent an invalid output sequence.");
          readyRef.current = false;
          socket?.close();
          return;
        }
        if (terminalSeq < nextSeqRef.current) {
          return;
        }
        if (terminalSeq > nextSeqRef.current) {
          recoverOutputGap(terminalSeq);
          return;
        }
        let bytes: Uint8Array;
        try {
          bytes = base64ToBytes(frame.data_base64);
        } catch {
          setFeedback(
            `Terminal output sequence ${terminalSeq} was not valid base64. Reconnecting before displaying further output.`,
          );
          readyRef.current = false;
          socket?.close();
          return;
        }
        const text = outputDecoderRef.current.decode(bytes, { stream: true });
        nextSeqRef.current = terminalSeq + 1;
        setSnapshot((current) => {
          if (!current || current.sessionKey !== sessionKey) {
            return current;
          }
          return {
            ...current,
            byteCount: current.byteCount + bytes.length,
            chunkCount: current.chunkCount + 1,
            nextSeq: nextSeqRef.current,
            text: current.text + text,
          };
        });
        return;
      }
      if (frame.type === "control_ack") {
        const ack = frame.ack;
        const pending = pendingControlsRef.current.get(ack.request_id);
        if (!pending) {
          return;
        }
        pendingControlsRef.current.delete(ack.request_id);
        if (pending.action.type === "input") {
          pendingInputBytesRef.current = Math.max(
            0,
            pendingInputBytesRef.current - pending.inputBytes,
          );
          pendingInputFramesRef.current = Math.max(
            0,
            pendingInputFramesRef.current - 1,
          );
        }
        if (!ack.accepted) {
          const error = new Error(
            ack.message || `Terminal ${pending.action.type} was rejected.`,
          );
          pending.reject?.(error);
          setFeedback(error.message);
          if (pending.action.type === "close") {
            closeQueuedRef.current = false;
          }
          return;
        }
        setSessionRecord((current) =>
          current
            ? applyAcceptedControlAck(current, pending.action, ack)
            : current,
        );
        pending.resolve?.(ack);
        if (pending.action.type === "close") {
          setSessionState("closed");
          setConnectionState("closed");
        }
        return;
      }
      if (frame.type === "session_state") {
        setSessionState(frame.session.state);
        setSessionRecord(frame.session);
        if (!isOpenSessionState(frame.session.state)) {
          closeQueuedRef.current = true;
          readyRef.current = false;
          setConnectionState("closed");
          socket?.close();
        }
        return;
      }
      setFeedback(frame.message || "Terminal stream reported an error.");
      if (frame.request_id) {
        const pending = pendingControlsRef.current.get(frame.request_id);
        if (pending) {
          pendingControlsRef.current.delete(frame.request_id);
          if (pending.action.type === "input") {
            pendingInputBytesRef.current = Math.max(
              0,
              pendingInputBytesRef.current - pending.inputBytes,
            );
            pendingInputFramesRef.current = Math.max(
              0,
              pendingInputFramesRef.current - 1,
            );
          }
          if (pending.action.type === "close") {
            closeQueuedRef.current = false;
          }
          pending.reject?.(
            new Error(
              frame.message || `Terminal ${pending.action.type} failed.`,
            ),
          );
        }
      }
      if (!frame.request_id && frame.recoverable === false) {
        closeQueuedRef.current = true;
        readyRef.current = false;
        setConnectionState("closed");
        socket?.close();
      }
    };

    const connect = () => {
      if (disposed) {
        return;
      }
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
        reconnectTimer = null;
      }
      readyRef.current = false;
      setConnectionState(reconnectAttempt === 0 ? "connecting" : "reconnecting");
      const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(
        `${protocol}//${window.location.host}/ws/terminal/${encodeURIComponent(clientId)}/${encodeURIComponent(sessionId)}`,
      );
      socketRef.current = socket;
      socket.addEventListener("open", () => {
        if (disposed || !socket) {
          socket?.close();
          return;
        }
        try {
          socket.send(
            JSON.stringify({
              access_token: accessToken,
              from_seq: nextSeqRef.current,
              type: "auth",
            }),
          );
        } catch {
          setFeedback(
            "Terminal stream authentication could not be sent. Input remains disabled while it reconnects.",
          );
          socket.close();
        }
      });
      socket.addEventListener("message", (event) => {
        if (disposed) {
          return;
        }
        const frame = parseServerFrame(event.data);
        if (!frame) {
          setFeedback("Terminal stream returned an unreadable protocol frame.");
          readyRef.current = false;
          socket?.close();
          return;
        }
        handleFrame(frame);
      });
      socket.addEventListener("error", () => {
        if (!disposed) {
          setFeedback(
            "Terminal stream connection failed. Input is disabled while it reconnects.",
          );
        }
      });
      socket.addEventListener("close", () => {
        if (disposed) {
          return;
        }
        readyRef.current = false;
        socketRef.current = null;
        rejectPendingControls(
          "The terminal stream disconnected before acknowledging this control. It was not retried.",
        );
        if (closeQueuedRef.current) {
          setConnectionState("closed");
          return;
        }
        scheduleReconnect();
      });
    };

    connect();
    return () => {
      disposed = true;
      readyRef.current = false;
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
      }
      rejectPendingControls(
        "The selected terminal changed before this control completed. It was not retried.",
      );
      if (
        socket?.readyState === WebSocket.CONNECTING ||
        socket?.readyState === WebSocket.OPEN
      ) {
        socket.close();
      }
      if (socketRef.current === socket) {
        socketRef.current = null;
      }
    };
  }, [
    accessToken,
    clientId,
    enabled,
    fromSeq,
    rejectPendingControls,
    sessionId,
    sessionKey,
  ]);

  return {
    closeSession,
    connectionState,
    feedback,
    queueInput,
    queueResize,
    sessionState,
    sessionRecord,
    snapshot,
  };
}

function parseServerFrame(value: unknown): TerminalServerFrame | null {
  if (typeof value !== "string") {
    return null;
  }
  try {
    const frame = JSON.parse(value) as Record<string, unknown>;
    if (!frame || typeof frame.type !== "string") {
      return null;
    }
    if (
      frame.type === "ready" &&
      typeof (frame.session as Record<string, unknown> | undefined)?.state ===
        "string"
    ) {
      return frame as TerminalServerFrame;
    }
    if (
      frame.type === "output" &&
      typeof frame.data_base64 === "string" &&
      finiteSequence(frame.terminal_seq) !== null
    ) {
      return frame as TerminalServerFrame;
    }
    if (
      frame.type === "control_ack" &&
      typeof (frame.ack as Record<string, unknown> | undefined)?.request_id ===
        "string" &&
      typeof (frame.ack as Record<string, unknown> | undefined)?.action ===
        "string" &&
      typeof (frame.ack as Record<string, unknown> | undefined)?.accepted ===
        "boolean"
    ) {
      return frame as TerminalServerFrame;
    }
    if (
      frame.type === "session_state" &&
      typeof (frame.session as Record<string, unknown> | undefined)?.state ===
        "string"
    ) {
      return frame as TerminalServerFrame;
    }
    if (
      frame.type === "error" &&
      typeof frame.message === "string" &&
      (frame.request_id === undefined || typeof frame.request_id === "string")
    ) {
      return frame as TerminalServerFrame;
    }
    return null;
  } catch {
    return null;
  }
}

function finiteSequence(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 1
    ? value
    : null;
}

function isOpenSessionState(state: string): boolean {
  return state === "opening" || state === "open";
}

function applyAcceptedControlAck(
  session: TerminalSessionRecord,
  action: TerminalControlAction,
  ack: TerminalControlAck,
): TerminalSessionRecord {
  if (action.type === "input") {
    const acknowledgedInputSeq = nonNegativeSequence(ack.input_seq);
    return {
      ...session,
      last_event: "terminal_input",
      last_input_seq:
        acknowledgedInputSeq === null
          ? session.last_input_seq
          : Math.max(session.last_input_seq, acknowledgedInputSeq),
      last_status: "accepted",
    };
  }
  if (action.type === "resize") {
    return {
      ...session,
      cols: positiveInteger(ack.cols) ?? action.cols,
      last_event: "terminal_resize",
      last_status: "resized",
      rows: positiveInteger(ack.rows) ?? action.rows,
    };
  }
  return {
    ...session,
    close_reason: action.reason ?? session.close_reason,
    last_event: "terminal_close",
    last_status: "closed",
    state: "closed",
  };
}

function nonNegativeSequence(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 0
    ? value
    : null;
}

function positiveInteger(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value > 0
    ? value
    : null;
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  }
  return window.btoa(binary);
}

function base64ToBytes(value: string): Uint8Array {
  const binary = window.atob(value);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}

function base64DecodedLength(value: string): number {
  if (!value) {
    return 0;
  }
  const padding = value.endsWith("==") ? 2 : value.endsWith("=") ? 1 : 0;
  return Math.floor((value.length * 3) / 4) - padding;
}
