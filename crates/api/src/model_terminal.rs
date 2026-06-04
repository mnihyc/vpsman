use serde::Serialize;
use uuid::Uuid;

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
    pub(crate) job_output_seq: i32,
    pub(crate) data_base64: Option<String>,
    pub(crate) size_bytes: i64,
    pub(crate) sha256_hex: Option<String>,
    pub(crate) storage: String,
    pub(crate) artifact_object_key: Option<String>,
    pub(crate) created_at: String,
}
