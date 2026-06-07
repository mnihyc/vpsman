use std::collections::BTreeMap;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AgentMetrics, CommandEnvelope, FilePushChunk, ProtocolError, TunnelConfigBackend,
    TunnelEndpointSide, TunnelPlan,
};

pub const NETWORK_SPEED_TEST_MIN_DURATION_SECS: u8 = 1;
pub const NETWORK_SPEED_TEST_MAX_DURATION_SECS: u8 = 30;
pub const NETWORK_SPEED_TEST_MIN_MAX_BYTES: u64 = 16 * 1024;
pub const NETWORK_SPEED_TEST_MAX_MAX_BYTES: u64 = 256 * 1024 * 1024;
pub const NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS: u32 = 64;
pub const NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS: u32 = 1_000_000;
pub const NETWORK_SPEED_TEST_MIN_PORT: u16 = 1024;
pub const NETWORK_SPEED_TEST_MAX_PORT: u16 = 65_535;
pub const NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS: u16 = 100;
pub const NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS: u16 = 30_000;
pub const MAX_SHELL_SCRIPT_BYTES: usize = 16 * 1024;
pub const MAX_TERMINAL_INPUT_BYTES: usize = 8 * 1024;
pub const MAX_TERMINAL_REASON_BYTES: usize = 240;
pub const MIN_TERMINAL_COLS: u16 = 20;
pub const MAX_TERMINAL_COLS: u16 = 240;
pub const MIN_TERMINAL_ROWS: u16 = 5;
pub const MAX_TERMINAL_ROWS: u16 = 120;
pub const MIN_TERMINAL_IDLE_TIMEOUT_SECS: u32 = 10;
pub const MAX_TERMINAL_IDLE_TIMEOUT_SECS: u32 = 86_400;
pub const MIN_TERMINAL_FLOW_WINDOW_BYTES: u32 = 4 * 1024;
pub const MAX_TERMINAL_FLOW_WINDOW_BYTES: u32 = 1024 * 1024;
pub const CURRENT_COMMAND_PROTOCOL_VERSION: u16 = 1;
pub const MIN_COMMAND_PROTOCOL_VERSION: u16 = 1;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRestartPolicy {
    #[default]
    Never,
    OnFailure,
    Always,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessRunPolicy {
    #[serde(default)]
    pub restart: ProcessRestartPolicy,
    #[serde(default)]
    pub restart_max_retries: u16,
    #[serde(default = "default_process_restart_backoff_secs")]
    pub restart_backoff_secs: u64,
    #[serde(default = "default_process_graceful_stop_secs")]
    pub graceful_stop_secs: u64,
}

impl Default for ProcessRunPolicy {
    fn default() -> Self {
        Self {
            restart: ProcessRestartPolicy::Never,
            restart_max_retries: 0,
            restart_backoff_secs: default_process_restart_backoff_secs(),
            graceful_stop_secs: default_process_graceful_stop_secs(),
        }
    }
}

impl ProcessRunPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessResourceLimits {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_max_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_max: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_files_max: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_shares: Option<u32>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_new_privileges: bool,
}

impl ProcessResourceLimits {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

pub fn default_process_restart_backoff_secs() -> u64 {
    5
}

pub fn default_process_graceful_stop_secs() -> u64 {
    5
}

pub fn default_terminal_idle_timeout_secs() -> u32 {
    1800
}

pub fn default_terminal_flow_window_bytes() -> u32 {
    64 * 1024
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RestoreRollbackFile {
    pub archive_path: String,
    pub destination_path: String,
    pub rollback_path: Option<String>,
    pub restored_size_bytes: u64,
    pub restored_sha256_hex: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentUpdateHeartbeat {
    pub activation_job_id: Uuid,
    pub sha256_hex: String,
    pub marker_unix: u64,
    pub observed_unix: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPrivilegeMode {
    #[default]
    Unknown,
    Root,
    Unprivileged,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentCapabilitySnapshot {
    #[serde(default)]
    pub privilege_mode: AgentPrivilegeMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_uid: Option<u32>,
    #[serde(default)]
    pub can_attempt_privileged_ops: bool,
    #[serde(default)]
    pub can_manage_runtime_tunnels: bool,
    #[serde(default)]
    pub can_apply_process_limits: bool,
    #[serde(default = "default_command_protocol_version")]
    pub command_protocol_version: u16,
    #[serde(default)]
    pub unprivileged_hint: Option<String>,
}

impl Default for AgentCapabilitySnapshot {
    fn default() -> Self {
        Self {
            privilege_mode: AgentPrivilegeMode::Unknown,
            effective_uid: None,
            can_attempt_privileged_ops: false,
            can_manage_runtime_tunnels: false,
            can_apply_process_limits: false,
            command_protocol_version: CURRENT_COMMAND_PROTOCOL_VERSION,
            unprivileged_hint: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentHello {
    pub client_id: String,
    pub agent_version: String,
    pub os_release: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_heartbeat: Option<AgentUpdateHeartbeat>,
    #[serde(default)]
    pub capabilities: AgentCapabilitySnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ServerHello {
    pub server_id: String,
    pub accepted: bool,
    pub message: String,
    pub telemetry_light_secs: u64,
    pub telemetry_full_secs: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TelemetryEnvelope {
    pub client_id: String,
    pub metrics: AgentMetrics,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayAgentHelloIngest {
    pub gateway_id: String,
    pub noise_public_key_hex: Option<String>,
    pub hello: AgentHello,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTelemetryIngest {
    pub gateway_id: String,
    pub telemetry: TelemetryEnvelope,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandOutputIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub job_id: Uuid,
    pub seq: i32,
    pub output: CommandOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayTerminalOutputIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub output: TerminalStreamOutput,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewaySessionLifecycleIngest {
    pub gateway_id: String,
    pub client_id: String,
    pub session_id: Uuid,
    pub noise_public_key_hex: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandDispatch {
    pub client_id: String,
    pub request: JobRequest,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandCancel {
    pub client_id: String,
    pub job_id: Uuid,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandDispatchResult {
    pub client_id: String,
    pub job_id: Uuid,
    pub accepted: bool,
    pub message: String,
    pub outputs: Vec<CommandOutput>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GatewayCommandCancelResult {
    pub client_id: String,
    pub job_id: Uuid,
    pub canceled: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRequest {
    pub job_id: Uuid,
    #[serde(default = "default_command_protocol_version")]
    pub command_version: u16,
    pub command: JobCommand,
    pub envelope: CommandEnvelope,
    pub timeout_secs: u64,
}

pub fn default_command_protocol_version() -> u16 {
    CURRENT_COMMAND_PROTOCOL_VERSION
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobCancelRequest {
    pub job_id: Uuid,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobCommand {
    Shell {
        argv: Vec<String>,
        pty: bool,
    },
    ShellScript {
        script: String,
    },
    TerminalOpen {
        session_id: Uuid,
        argv: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay_from_seq: Option<u64>,
        #[serde(default = "default_terminal_idle_timeout_secs")]
        idle_timeout_secs: u32,
        #[serde(default = "default_terminal_flow_window_bytes")]
        flow_window_bytes: u32,
    },
    TerminalInput {
        session_id: Uuid,
        input_seq: u64,
        data_base64: String,
    },
    TerminalPoll {
        session_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        replay_from_seq: Option<u64>,
    },
    TerminalResize {
        session_id: Uuid,
        cols: u16,
        rows: u16,
    },
    TerminalClose {
        session_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    HotConfig {
        toml: String,
    },
    DataSourceConfigPatch {
        toml: String,
    },
    AuthProofKeyRotate {
        new_proof_key_hex: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rotation_generation: Option<String>,
    },
    #[serde(rename = "agent_update", alias = "update_agent")]
    UpdateAgent {
        artifact_url: String,
        sha256_hex: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_signature_hex: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artifact_signing_key_hex: Option<String>,
    },
    AgentUpdateActivate {
        staged_sha256_hex: String,
        #[serde(default, skip_serializing_if = "is_false")]
        restart_agent: bool,
    },
    AgentUpdateRollback {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rollback_sha256_hex: Option<String>,
    },
    AgentUpdateCheck {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version_url: Option<String>,
        #[serde(default = "default_agent_update_check_activate")]
        activate: bool,
        #[serde(default = "default_agent_update_check_restart_agent")]
        restart_agent: bool,
    },
    FilePull {
        path: String,
    },
    FilePush {
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        data_base64: String,
        #[serde(default, skip_serializing_if = "FileExistingPolicy::is_default")]
        existing_policy: FileExistingPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gid: Option<u32>,
        #[serde(default, skip_serializing_if = "FileOwnershipPolicy::is_default")]
        ownership_policy: FileOwnershipPolicy,
    },
    FilePushChunked {
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        chunks: Vec<FilePushChunk>,
        #[serde(default, skip_serializing_if = "FileExistingPolicy::is_default")]
        existing_policy: FileExistingPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gid: Option<u32>,
        #[serde(default, skip_serializing_if = "FileOwnershipPolicy::is_default")]
        ownership_policy: FileOwnershipPolicy,
    },
    FileTransferStart {
        session_id: Uuid,
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        chunk_size_bytes: u32,
        #[serde(default)]
        rate_limit_kbps: u32,
        #[serde(default, skip_serializing_if = "FileExistingPolicy::is_default")]
        existing_policy: FileExistingPolicy,
        resume_token_hash: String,
    },
    FileTransferChunk {
        session_id: Uuid,
        offset: u64,
        chunk: FilePushChunk,
        resume_token_hash: String,
    },
    FileTransferCommit {
        session_id: Uuid,
        resume_token_hash: String,
    },
    FileTransferAbort {
        session_id: Uuid,
        resume_token_hash: String,
    },
    FileTransferDownloadStart {
        session_id: Uuid,
        path: String,
        chunk_size_bytes: u32,
        rate_limit_kbps: u32,
        resume_token_hash: String,
    },
    FileTransferDownloadChunk {
        session_id: Uuid,
        offset: u64,
        max_bytes: u32,
        resume_token_hash: String,
    },
    FileStat {
        path: String,
    },
    FileListDir {
        path: String,
        #[serde(default)]
        offset: u32,
        #[serde(default = "default_file_list_limit")]
        limit: u32,
        #[serde(default)]
        show_hidden: bool,
    },
    FileReadText {
        path: String,
        #[serde(default = "default_file_read_max_bytes")]
        max_bytes: u64,
    },
    FileWriteText {
        path: String,
        mode: u32,
        size_bytes: u64,
        sha256_hex: String,
        content_base64: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sha256_hex: Option<String>,
        #[serde(default)]
        create: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileMkdir {
        path: String,
        mode: u32,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileRename {
        path: String,
        new_path: String,
        #[serde(default)]
        overwrite: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileDelete {
        path: String,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileChmod {
        path: String,
        mode: u32,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileChown {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        uid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gid: Option<u32>,
        #[serde(default)]
        recursive: bool,
        #[serde(default, skip_serializing_if = "FileOwnershipPolicy::is_default")]
        ownership_policy: FileOwnershipPolicy,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileCopy {
        path: String,
        new_path: String,
        #[serde(default)]
        overwrite: bool,
        #[serde(default)]
        recursive: bool,
        #[serde(default)]
        policy: FileActionPolicy,
    },
    FileDownload {
        path: String,
        #[serde(default = "default_file_download_max_bytes")]
        max_bytes: u64,
    },
    FileArchiveTar {
        path: String,
        #[serde(default = "default_file_archive_max_bytes")]
        max_bytes: u64,
    },
    UserSessions,
    ProcessList {
        limit: u16,
    },
    ProcessStart {
        name: String,
        argv: Vec<String>,
        cwd: Option<String>,
        env: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "ProcessRunPolicy::is_default")]
        policy: ProcessRunPolicy,
        #[serde(default, skip_serializing_if = "ProcessResourceLimits::is_default")]
        limits: ProcessResourceLimits,
    },
    ProcessStop {
        name: String,
    },
    ProcessRestart {
        name: String,
    },
    ProcessStatus {
        name: Option<String>,
    },
    ProcessLogs {
        name: String,
        max_bytes: u32,
    },
    Backup {
        paths: Vec<String>,
        include_config: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recipient_public_key_hex: Option<String>,
    },
    Restore {
        source_backup_request_id: Uuid,
        paths: Vec<String>,
        include_config: bool,
        destination_root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive_path: Option<String>,
        archive_base64: Option<String>,
        archive_size_bytes: Option<u64>,
        archive_sha256_hex: Option<String>,
        #[serde(default, skip_serializing_if = "is_false")]
        dry_run: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        post_restore_argv: Vec<String>,
    },
    RestoreRollback {
        source_restore_job_id: Uuid,
        restored_files: Vec<RestoreRollbackFile>,
    },
    NetworkApply {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        #[serde(default)]
        config_backend: TunnelConfigBackend,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config_sha256_hex: Option<String>,
        ifupdown_sha256_hex: String,
        bird2_sha256_hex: String,
    },
    NetworkOspfCostUpdate {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        current_ospf_cost: u16,
        recommended_ospf_cost: u16,
        bird2_sha256_hex: String,
    },
    NetworkRollback {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
    },
    NetworkStatus {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
    },
    NetworkProbe {
        plan: Box<TunnelPlan>,
        side: TunnelEndpointSide,
        count: u8,
        interval_ms: u16,
    },
    NetworkSpeedTest {
        plan: Box<TunnelPlan>,
        server_side: TunnelEndpointSide,
        duration_secs: u8,
        max_bytes: u64,
        rate_limit_kbps: u32,
        port: u16,
        connect_timeout_ms: u16,
    },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileActionPolicy {
    #[default]
    Fail,
    Ensure,
    Ignore,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileExistingPolicy {
    #[default]
    Skip,
    Replace,
}

impl FileExistingPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOwnershipPolicy {
    #[default]
    Fail,
    Ignore,
}

impl FileOwnershipPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_file_list_limit() -> u32 {
    250
}

fn default_file_read_max_bytes() -> u64 {
    1024 * 1024
}

fn default_file_archive_max_bytes() -> u64 {
    64 * 1024 * 1024
}

fn default_file_download_max_bytes() -> u64 {
    64 * 1024 * 1024
}

fn default_agent_update_check_activate() -> bool {
    true
}

fn default_agent_update_check_restart_agent() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobAck {
    pub job_id: Uuid,
    pub accepted: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CommandOutput {
    pub job_id: Uuid,
    pub stream: OutputStream,
    pub data: Vec<u8>,
    pub exit_code: Option<i32>,
    pub done: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TerminalStreamOutput {
    pub job_id: Uuid,
    pub session_id: Uuid,
    pub terminal_seq: Option<u64>,
    pub output_first_seq: Option<u64>,
    pub output_next_seq: u64,
    pub output_retained_first_seq: Option<u64>,
    pub output_retained_bytes: u64,
    pub output_dropped_bytes: u64,
    pub output_dropped_chunks: u64,
    pub output_replay_truncated: bool,
    pub output: CommandOutput,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
    Pty,
    Status,
}

pub fn encode_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    Ok(serde_json::to_vec(value)?)
}

pub fn decode_json<T: DeserializeOwned>(payload: &[u8]) -> Result<T, ProtocolError> {
    Ok(serde_json::from_slice(payload)?)
}

#[cfg(test)]
mod tests {
    use super::JobCommand;

    #[test]
    fn serializes_agent_update_with_canonical_name_and_accepts_legacy_alias() {
        let command = JobCommand::UpdateAgent {
            artifact_url: "https://updates.example/vpsman-agent".to_string(),
            sha256_hex: "ab".repeat(32),
            artifact_signature_hex: None,
            artifact_signing_key_hex: None,
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["type"], "agent_update");
        assert!(encoded.get("artifact_signature_hex").is_none());

        let legacy = serde_json::json!({
            "type": "update_agent",
            "artifact_url": "https://updates.example/vpsman-agent",
            "sha256_hex": "ab".repeat(32),
        });
        assert!(matches!(
            serde_json::from_value::<JobCommand>(legacy).unwrap(),
            JobCommand::UpdateAgent { .. }
        ));
    }

    #[test]
    fn omits_false_agent_update_restart_flag_from_payload_hash_shape() {
        let command = JobCommand::AgentUpdateActivate {
            staged_sha256_hex: "ab".repeat(32),
            restart_agent: false,
        };
        let encoded = serde_json::to_value(&command).unwrap();
        assert_eq!(encoded["type"], "agent_update_activate");
        assert!(encoded.get("restart_agent").is_none());

        let restart = JobCommand::AgentUpdateActivate {
            staged_sha256_hex: "ab".repeat(32),
            restart_agent: true,
        };
        let encoded = serde_json::to_value(&restart).unwrap();
        assert_eq!(encoded["restart_agent"], true);
    }
}
