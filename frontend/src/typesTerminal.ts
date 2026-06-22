import type {
  GeneratedTerminalCommandType,
  GeneratedTerminalSessionEvent,
  GeneratedTerminalSessionState,
  GeneratedTerminalSessionStatus,
} from "./generated/protocolContracts";
import type { CreateJobResponse, PrivilegeAssertion } from "./types";

export type TerminalSessionRecord = {
  session_id: string;
  client_id: string;
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
  last_input_seq: number | null;
  session_exited: boolean;
  close_reason: string | null;
  last_event: GeneratedTerminalSessionEvent;
  last_job_id: string;
  last_command_type: GeneratedTerminalCommandType;
  last_seq: number;
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

export type TerminalInputSubmitRequest = {
  job_id: string;
  text?: string | null;
  data_base64?: string | null;
  max_timeout_secs?: number;
  confirmed: boolean;
  privilege_assertion?: PrivilegeAssertion | null;
};

export type TerminalInputSubmitResponse = {
  job: CreateJobResponse;
  input_seq: number;
  request_status: string;
};
