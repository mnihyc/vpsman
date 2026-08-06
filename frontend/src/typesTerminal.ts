import type {
  GeneratedTerminalSessionEvent,
  GeneratedTerminalSessionState,
  GeneratedTerminalSessionStatus,
} from "./generated/protocolContracts";

export type TerminalSessionRecord = {
  session_id: string;
  client_id: string;
  job_id: string;
  state: GeneratedTerminalSessionState;
  last_status: GeneratedTerminalSessionStatus;
  argv: string[];
  cwd: string | null;
  cols: number | null;
  rows: number | null;
  idle_timeout_secs: number | null;
  flow_window_bytes: number | null;
  output_first_seq: number | null;
  output_next_seq: number | null;
  output_retained_first_seq: number | null;
  output_retained_bytes: number | null;
  output_dropped_bytes: number | null;
  output_dropped_chunks: number | null;
  output_replay_truncated: boolean;
  last_input_seq: number;
  close_reason: string | null;
  last_event: GeneratedTerminalSessionEvent;
  opened_at: string | null;
  observed_at: string;
};

export type TerminalReplayChunkRecord = {
  terminal_seq: number;
  job_id: string;
  data_base64: string | null;
  size_bytes: number;
  sha256_hex: string;
  created_at: string;
};

export type TerminalReplayRecord = {
  session_id: string;
  client_id: string;
  from_seq: number;
  available_first_seq: number | null;
  next_seq: number;
  chunk_count: number;
  byte_count: number;
  truncated: boolean;
  source: string;
  chunks: TerminalReplayChunkRecord[];
};

export type TerminalControlAction =
  | { type: "input"; data_base64: string }
  | { type: "resize"; cols: number; rows: number }
  | { type: "close"; reason?: string | null };

export type TerminalControlAck = {
  request_id: string;
  session_id: string;
  action: TerminalControlAction["type"];
  accepted: boolean;
  status: string;
  message: string;
  input_seq?: number | null;
  written_bytes?: number | null;
  cols?: number | null;
  rows?: number | null;
};
