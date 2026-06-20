use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::PrivilegeAssertion;

use crate::model::CreateJobResponse;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TerminalSessionView {
    pub(crate) session_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) state: String,
    pub(crate) last_status: String,
    pub(crate) argv: Vec<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) cols: Option<i64>,
    pub(crate) rows: Option<i64>,
    pub(crate) idle_timeout_secs: Option<i64>,
    pub(crate) flow_window_bytes: Option<i64>,
    pub(crate) output_first_seq: Option<i64>,
    pub(crate) output_next_seq: Option<i64>,
    pub(crate) output_retained_first_seq: Option<i64>,
    pub(crate) output_retained_bytes: Option<i64>,
    pub(crate) output_dropped_bytes: Option<i64>,
    pub(crate) output_dropped_chunks: Option<i64>,
    pub(crate) output_replay_truncated: bool,
    pub(crate) last_input_seq: Option<i64>,
    pub(crate) session_exited: bool,
    pub(crate) close_reason: Option<String>,
    pub(crate) last_event: String,
    pub(crate) last_job_id: Uuid,
    pub(crate) last_command_type: String,
    pub(crate) last_seq: i32,
    pub(crate) observed_at: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TerminalInputSubmitRequest {
    pub(crate) job_id: Uuid,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) data_base64: Option<String>,
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    #[serde(default)]
    pub(crate) confirmed: bool,
    #[serde(default)]
    pub(crate) privilege_assertion: Option<PrivilegeAssertion>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TerminalInputSubmitResponse {
    pub(crate) job: CreateJobResponse,
    pub(crate) input_seq: i64,
    pub(crate) request_status: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalInputRequestRecord {
    pub(crate) job_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) session_id: Uuid,
    pub(crate) input_seq: i64,
    pub(crate) payload_sha256_hex: String,
    pub(crate) status: String,
    pub(crate) updated_at: String,
    pub(crate) completed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TerminalReplayView {
    pub(crate) session_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) from_seq: i64,
    pub(crate) available_first_seq: Option<i64>,
    pub(crate) next_seq: i64,
    pub(crate) chunk_count: usize,
    pub(crate) byte_count: i64,
    pub(crate) truncated: bool,
    pub(crate) source: String,
    pub(crate) chunks: Vec<TerminalReplayChunkView>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TerminalReplayChunkView {
    pub(crate) terminal_seq: i64,
    pub(crate) job_id: Uuid,
    pub(crate) data_base64: Option<String>,
    pub(crate) size_bytes: i64,
    pub(crate) sha256_hex: String,
    pub(crate) created_at: String,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalOutputChunkRecord {
    pub(crate) client_id: String,
    pub(crate) session_id: Uuid,
    pub(crate) terminal_seq: i64,
    pub(crate) job_id: Uuid,
    pub(crate) data: Vec<u8>,
    pub(crate) size_bytes: i64,
    pub(crate) sha256_hex: String,
    pub(crate) created_at: String,
}
