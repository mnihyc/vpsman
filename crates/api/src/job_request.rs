use anyhow::Result;
use vpsman_common::{
    backend_config_signature_payload,
    job_command_min_supported_protocol_version as common_job_command_min_supported_protocol_version,
    job_command_protocol_version as common_job_command_protocol_version, payload_hash,
    render_tunnel_endpoint_backend_config, render_tunnel_endpoint_config,
    validate_agent_config_shape, validate_data_source_config_patch_section,
    validate_runtime_topology_intent, validate_runtime_tunnel_control,
    verify_update_artifact_signature, AgentConfig, JobCommand, ProcessResourceLimits,
    ProcessRunPolicy, RestoreRollbackFile, TunnelConfigBackend, MAX_AGENT_HOT_CONFIG_BYTES,
    MAX_SHELL_SCRIPT_BYTES, NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS,
    NETWORK_SPEED_TEST_MAX_DURATION_SECS, NETWORK_SPEED_TEST_MAX_MAX_BYTES,
    NETWORK_SPEED_TEST_MAX_PORT, NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS,
    NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS, NETWORK_SPEED_TEST_MIN_DURATION_SECS,
    NETWORK_SPEED_TEST_MIN_MAX_BYTES, NETWORK_SPEED_TEST_MIN_PORT,
    NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS,
};

use crate::{
    job_files::{validate_file_command, validate_inline_file_payload},
    job_terminal::{
        validate_terminal_close, validate_terminal_input, validate_terminal_open,
        validate_terminal_poll, validate_terminal_resize, TerminalOpenValidation,
    },
    model::{BulkResolveRequest, CreateJobRequest},
    ApiError,
};

pub(crate) use crate::job_files::validate_file_path;

const MAX_BACKUP_PATHS: usize = 64;
const MAX_RESTORE_PATHS: usize = 64;
const MAX_RESTORE_ROLLBACK_FILE_BYTES: u64 = 16 * 1024 * 1024;

impl CreateJobRequest {
    pub(crate) fn fixed_target_ids(&self) -> Result<Vec<String>, ApiError> {
        normalized_target_client_ids(&self.target_client_ids)
    }

    pub(crate) fn target_selection(&self) -> Result<BulkResolveRequest, ApiError> {
        fixed_target_selection(&self.target_client_ids)
    }

    pub(crate) fn job_command(&self) -> Result<JobCommand, ApiError> {
        if let Some(command) = &self.operation {
            validate_job_command(command)?;
            return Ok(command.clone());
        }
        let argv = if self.argv.is_empty() {
            if self.command.trim().is_empty() {
                return Err(ApiError::bad_request("command_required"));
            }
            vec![self.command.clone()]
        } else {
            self.argv.clone()
        };
        if argv.iter().any(|part| part.is_empty()) {
            return Err(ApiError::bad_request("argv_contains_empty_part"));
        }
        let command = JobCommand::Shell { argv, pty: false };
        validate_job_command(&command)?;
        Ok(command)
    }

    pub(crate) fn command_type_label(&self) -> &'static str {
        match &self.operation {
            Some(command) => job_command_type_label(command),
            None => "shell_argv",
        }
    }
}

pub(crate) fn normalized_target_client_ids(raw: &[String]) -> Result<Vec<String>, ApiError> {
    let mut ids = Vec::new();
    for value in raw {
        let id = value.trim();
        if id.is_empty() {
            continue;
        }
        if id.len() > 200 || id.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
            return Err(ApiError::bad_request("target_client_id_invalid"));
        }
        if !ids.iter().any(|stored| stored == id) {
            ids.push(id.to_string());
        }
    }
    if ids.is_empty() {
        return Err(ApiError::bad_request("fixed_targets_required"));
    }
    if ids.len() > 500 {
        return Err(ApiError::bad_request("too_many_fixed_targets"));
    }
    Ok(ids)
}

pub(crate) fn fixed_target_selection(raw: &[String]) -> Result<BulkResolveRequest, ApiError> {
    let ids = normalized_target_client_ids(raw)?;
    Ok(BulkResolveRequest {
        selector_expression: ids
            .iter()
            .map(|id| vpsman_common::id_selector_expression(id))
            .collect::<Vec<_>>()
            .join(" || "),
    })
}

pub(crate) fn job_command_type_label(command: &JobCommand) -> &'static str {
    vpsman_server_core::job_command_type_label(command)
}

pub(crate) fn job_command_protocol_version(_command: &JobCommand) -> u16 {
    common_job_command_protocol_version(_command)
}

pub(crate) fn job_command_min_supported_protocol_version(_command: &JobCommand) -> u16 {
    common_job_command_min_supported_protocol_version(_command)
}

pub(crate) fn validate_job_command(command: &JobCommand) -> Result<(), ApiError> {
    if let Some(result) = validate_file_command(command) {
        return result;
    }
    match command {
        JobCommand::Shell { argv, .. } => {
            if argv.is_empty() {
                return Err(ApiError::bad_request("argv_command_is_empty"));
            }
            if argv.iter().any(|part| part.is_empty()) {
                return Err(ApiError::bad_request("argv_contains_empty_part"));
            }
            Ok(())
        }
        JobCommand::ShellScript { script } => validate_shell_script(script),
        JobCommand::TerminalOpen {
            session_id,
            argv,
            cwd,
            user,
            user_policy,
            cols,
            rows,
            idle_timeout_secs,
            flow_window_bytes,
            ..
        } => validate_terminal_open(TerminalOpenValidation {
            session_id: *session_id,
            argv,
            cwd: cwd.as_deref(),
            user: user.as_deref(),
            user_policy: *user_policy,
            cols: *cols,
            rows: *rows,
            idle_timeout_secs: *idle_timeout_secs,
            flow_window_bytes: *flow_window_bytes,
        }),
        JobCommand::TerminalInput {
            session_id,
            input_seq: _,
            data_base64,
        } => validate_terminal_input(*session_id, data_base64),
        JobCommand::TerminalPoll { session_id, .. } => validate_terminal_poll(*session_id),
        JobCommand::TerminalResize {
            session_id,
            cols,
            rows,
        } => validate_terminal_resize(*session_id, *cols, *rows),
        JobCommand::TerminalClose { session_id, reason } => {
            validate_terminal_close(*session_id, reason.as_deref())
        }
        JobCommand::FilePull { .. }
        | JobCommand::FilePush { .. }
        | JobCommand::FilePushChunked { .. }
        | JobCommand::FileTransferStart { .. }
        | JobCommand::FileTransferChunk { .. }
        | JobCommand::FileTransferCommit { .. }
        | JobCommand::FileTransferAbort { .. }
        | JobCommand::FileTransferDownloadStart { .. }
        | JobCommand::FileTransferDownloadChunk { .. }
        | JobCommand::FileStat { .. }
        | JobCommand::FileListDir { .. }
        | JobCommand::FileReadText { .. }
        | JobCommand::FileWriteText { .. }
        | JobCommand::FileMkdir { .. }
        | JobCommand::FileRename { .. }
        | JobCommand::FileDelete { .. }
        | JobCommand::FileChmod { .. }
        | JobCommand::FileChown { .. }
        | JobCommand::FileCopy { .. }
        | JobCommand::FileDownload { .. }
        | JobCommand::FileArchiveTar { .. } => {
            unreachable!("file commands are validated by job_files")
        }
        JobCommand::UserSessions => Ok(()),
        JobCommand::ProcessList { limit } => {
            if *limit == 0 || *limit > 512 {
                return Err(ApiError::bad_request("process_list_limit_out_of_range"));
            }
            Ok(())
        }
        JobCommand::ProcessStart {
            name,
            argv,
            cwd,
            env,
            policy,
            limits,
        } => validate_process_start(name, argv, cwd.as_deref(), env, policy, limits),
        JobCommand::ProcessStop { name } | JobCommand::ProcessRestart { name } => {
            validate_process_name(name)
        }
        JobCommand::ProcessStatus { name } => {
            if let Some(name) = name {
                validate_process_name(name)?;
            }
            Ok(())
        }
        JobCommand::ProcessLogs { name, max_bytes } => {
            validate_process_name(name)?;
            if *max_bytes == 0 || *max_bytes > 512 * 1024 {
                return Err(ApiError::bad_request("process_logs_max_bytes_out_of_range"));
            }
            Ok(())
        }
        JobCommand::ConfigRead => Ok(()),
        JobCommand::HotConfig {
            toml,
            preserve_redacted: _,
            base_config_sha256_hex,
        } => {
            validate_hot_config_document(toml)?;
            if let Some(base_config_sha256_hex) = base_config_sha256_hex {
                validate_sha256_hex(
                    base_config_sha256_hex,
                    "hot_config_base_config_sha256_invalid",
                )?;
            }
            Ok(())
        }
        JobCommand::DataSourceConfigPatch { toml } => {
            validate_data_source_config_patch_document(toml)
        }
        JobCommand::UpdateAgent {
            artifact_url,
            sha256_hex,
            artifact_signature_hex,
            artifact_signing_key_hex,
        } => validate_update_agent(
            artifact_url,
            sha256_hex,
            artifact_signature_hex.as_deref(),
            artifact_signing_key_hex.as_deref(),
        ),
        JobCommand::AgentUpdateActivate {
            staged_sha256_hex, ..
        } => validate_sha256_hex(staged_sha256_hex, "agent_update_activate_sha256_invalid"),
        JobCommand::AgentUpdateRollback {
            rollback_sha256_hex,
        } => {
            if let Some(rollback_sha256_hex) = rollback_sha256_hex {
                validate_sha256_hex(rollback_sha256_hex, "agent_update_rollback_sha256_invalid")?;
            }
            Ok(())
        }
        JobCommand::AgentUpdateCheck { version_url, .. } => {
            if let Some(version_url) = version_url {
                validate_update_manifest_url(version_url)?;
            }
            Ok(())
        }
        JobCommand::Backup {
            paths,
            include_config,
            recipient_public_key_hex,
        } => validate_backup_operation(paths, *include_config, recipient_public_key_hex.as_deref()),
        JobCommand::Restore {
            paths,
            include_config,
            destination_root,
            archive_base64,
            archive_path,
            archive_size_bytes,
            archive_sha256_hex,
            dry_run: _,
            post_restore_argv,
            ..
        } => validate_restore_operation(RestoreOperationValidation {
            paths,
            include_config: *include_config,
            destination_root: destination_root.as_deref(),
            archive_path: archive_path.as_deref(),
            archive_base64: archive_base64.as_deref(),
            archive_size_bytes: *archive_size_bytes,
            archive_sha256_hex: archive_sha256_hex.as_deref(),
            post_restore_argv,
        }),
        JobCommand::RestoreRollback { restored_files, .. } => {
            validate_restore_rollback_operation(restored_files)
        }
        JobCommand::NetworkApply {
            plan,
            side,
            config_backend,
            config_sha256_hex,
            ifupdown_sha256_hex,
            bird2_sha256_hex,
        } => validate_network_apply_operation(
            plan,
            *side,
            *config_backend,
            config_sha256_hex.as_deref(),
            ifupdown_sha256_hex,
            bird2_sha256_hex,
        ),
        JobCommand::NetworkOspfCostUpdate {
            plan,
            side,
            current_ospf_cost,
            recommended_ospf_cost,
            bird2_sha256_hex,
        } => validate_network_ospf_cost_update_operation(
            plan,
            *side,
            *current_ospf_cost,
            *recommended_ospf_cost,
            bird2_sha256_hex,
        ),
        JobCommand::NetworkRollback { plan, side } => validate_network_plan_side(
            plan,
            *side,
            "network_rollback_plan_must_be_observe_plan",
            "network_rollback_plan_invalid",
        ),
        JobCommand::NetworkStatus { plan, side } => validate_network_plan_side(
            plan,
            *side,
            "network_status_plan_must_be_observe_plan",
            "network_status_plan_invalid",
        ),
        JobCommand::NetworkInterfaces => Ok(()),
        JobCommand::NetworkProbe {
            plan,
            side,
            count,
            interval_ms,
        } => validate_network_probe_operation(plan, *side, *count, *interval_ms),
        JobCommand::NetworkSpeedTest {
            plan,
            server_side,
            duration_secs,
            max_bytes,
            rate_limit_kbps,
            port,
            connect_timeout_ms,
        } => validate_network_speed_test_operation(
            plan,
            *server_side,
            *duration_secs,
            *max_bytes,
            *rate_limit_kbps,
            *port,
            *connect_timeout_ms,
        ),
    }
}

fn validate_restore_rollback_operation(
    restored_files: &[RestoreRollbackFile],
) -> Result<(), ApiError> {
    if restored_files.is_empty() {
        return Err(ApiError::bad_request("restore_rollback_files_required"));
    }
    if restored_files.len() > MAX_RESTORE_PATHS {
        return Err(ApiError::bad_request(
            "restore_rollback_file_limit_exceeded",
        ));
    }
    for file in restored_files {
        if file.archive_path.trim().is_empty() || file.archive_path.len() > 4096 {
            return Err(ApiError::bad_request(
                "restore_rollback_archive_path_invalid",
            ));
        }
        if path_contains_dot_segment(&file.destination_path) {
            return Err(ApiError::bad_request(
                "restore_rollback_destination_path_invalid",
            ));
        }
        validate_file_path(&file.destination_path)?;
        if let Some(rollback_path) = &file.rollback_path {
            if path_contains_dot_segment(rollback_path) {
                return Err(ApiError::bad_request(
                    "restore_rollback_snapshot_path_invalid",
                ));
            }
            validate_file_path(rollback_path)?;
        }
        if file.restored_size_bytes > MAX_RESTORE_ROLLBACK_FILE_BYTES {
            return Err(ApiError::bad_request("restore_rollback_size_invalid"));
        }
        if !is_sha256_hex(&file.restored_sha256_hex) {
            return Err(ApiError::bad_request("restore_rollback_sha256_invalid"));
        }
    }
    Ok(())
}

fn validate_shell_script(script: &str) -> Result<(), ApiError> {
    if script.trim().is_empty() {
        return Err(ApiError::bad_request("shell_script_is_empty"));
    }
    if script.len() > MAX_SHELL_SCRIPT_BYTES {
        return Err(ApiError::bad_request("shell_script_too_large"));
    }
    if script
        .chars()
        .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
    {
        return Err(ApiError::bad_request(
            "shell_script_contains_control_character",
        ));
    }
    Ok(())
}

fn validate_backup_operation(
    paths: &[String],
    include_config: bool,
    recipient_public_key_hex: Option<&str>,
) -> Result<(), ApiError> {
    if !include_config && paths.is_empty() {
        return Err(ApiError::bad_request("backup_scope_required"));
    }
    if paths.len() > MAX_BACKUP_PATHS {
        return Err(ApiError::bad_request("backup_path_limit_exceeded"));
    }
    for path in paths {
        validate_file_path(path)?;
    }
    if let Some(recipient_public_key_hex) = recipient_public_key_hex {
        validate_hex32(
            recipient_public_key_hex,
            "backup_recipient_public_key_hex_invalid",
        )?;
    }
    Ok(())
}

struct RestoreOperationValidation<'a> {
    paths: &'a [String],
    include_config: bool,
    destination_root: Option<&'a str>,
    archive_path: Option<&'a str>,
    archive_base64: Option<&'a str>,
    archive_size_bytes: Option<u64>,
    archive_sha256_hex: Option<&'a str>,
    post_restore_argv: &'a [String],
}

fn validate_restore_operation(input: RestoreOperationValidation<'_>) -> Result<(), ApiError> {
    let RestoreOperationValidation {
        paths,
        include_config,
        destination_root,
        archive_path,
        archive_base64,
        archive_size_bytes,
        archive_sha256_hex,
        post_restore_argv,
    } = input;
    if !include_config && paths.is_empty() {
        return Err(ApiError::bad_request("restore_scope_required"));
    }
    if paths.len() > MAX_RESTORE_PATHS {
        return Err(ApiError::bad_request("restore_path_limit_exceeded"));
    }
    for path in paths {
        if path_contains_dot_segment(path) {
            return Err(ApiError::bad_request("restore_path_invalid"));
        }
        validate_file_path(path)?;
    }
    if let Some(destination_root) = destination_root {
        if path_contains_dot_segment(destination_root) {
            return Err(ApiError::bad_request("restore_destination_root_invalid"));
        }
        validate_file_path(destination_root)?;
    }
    if include_config && destination_root.is_none() {
        return Err(ApiError::bad_request(
            "restore_config_requires_destination_root",
        ));
    }
    let archive_path = archive_path
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if archive_path.is_some() && archive_base64.is_some() {
        return Err(ApiError::bad_request("restore_archive_source_ambiguous"));
    }
    if let Some(archive_path) = archive_path {
        if path_contains_dot_segment(archive_path) {
            return Err(ApiError::bad_request("restore_archive_path_invalid"));
        }
        validate_file_path(archive_path)?;
        if let Some(archive_size_bytes) = archive_size_bytes {
            if archive_size_bytes == 0 {
                return Err(ApiError::bad_request("restore_archive_size_invalid"));
            }
        }
        if let Some(archive_sha256_hex) = archive_sha256_hex {
            validate_sha256_hex(archive_sha256_hex, "restore_archive_sha256_invalid")?;
        }
    } else {
        let archive_base64 =
            archive_base64.ok_or_else(|| ApiError::bad_request("restore_archive_required"))?;
        let archive_size_bytes = archive_size_bytes
            .ok_or_else(|| ApiError::bad_request("restore_archive_size_required"))?;
        let archive_sha256_hex = archive_sha256_hex
            .ok_or_else(|| ApiError::bad_request("restore_archive_sha256_required"))?;
        validate_inline_file_payload(archive_base64, archive_size_bytes, archive_sha256_hex)?;
    }
    validate_post_restore_argv(post_restore_argv)?;
    Ok(())
}

fn validate_post_restore_argv(argv: &[String]) -> Result<(), ApiError> {
    if argv.is_empty() {
        return Ok(());
    }
    if argv.len() > 32 {
        return Err(ApiError::bad_request("restore_post_restore_argv_too_long"));
    }
    if argv[0].trim().is_empty() || !argv[0].starts_with('/') {
        return Err(ApiError::bad_request(
            "restore_post_restore_executable_invalid",
        ));
    }
    if argv
        .iter()
        .any(|part| part.is_empty() || part.len() > 4096 || part.contains('\0'))
    {
        return Err(ApiError::bad_request("restore_post_restore_argv_invalid"));
    }
    Ok(())
}

fn validate_network_apply_operation(
    plan: &vpsman_common::TunnelPlan,
    side: vpsman_common::TunnelEndpointSide,
    backend: TunnelConfigBackend,
    config_sha256_hex: Option<&str>,
    ifupdown_sha256_hex: &str,
    bird2_sha256_hex: &str,
) -> Result<(), ApiError> {
    if plan.mutates_host {
        return Err(ApiError::bad_request(
            "network_apply_plan_must_be_observe_plan",
        ));
    }
    validate_runtime_tunnel_control(&plan.runtime_control)
        .map_err(|_| ApiError::bad_request("network_runtime_control_invalid"))?;
    validate_runtime_topology_intent(&plan.runtime_topology, &plan.interface_name).map_err(
        |error| match error {
            vpsman_common::NetworkPlanError::InvalidRuntimeTunnelRoute => {
                ApiError::bad_request("network_runtime_route_invalid")
            }
            _ => ApiError::bad_request("network_runtime_topology_invalid"),
        },
    )?;
    let endpoint = render_tunnel_endpoint_config(plan, side)
        .map_err(|_| ApiError::bad_request("network_apply_plan_invalid"))?;
    let backend_config = render_tunnel_endpoint_backend_config(plan, side, backend)
        .map_err(|_| ApiError::bad_request("network_apply_backend_invalid"))?;
    if backend == TunnelConfigBackend::Ifupdown
        && payload_hash(endpoint.ifupdown_snippet.as_bytes())
            != normalize_sha256(ifupdown_sha256_hex)?
    {
        return Err(ApiError::bad_request(
            "network_apply_ifupdown_hash_mismatch",
        ));
    }
    if backend != TunnelConfigBackend::Ifupdown && config_sha256_hex.is_none() {
        return Err(ApiError::bad_request("network_apply_config_hash_required"));
    }
    if let Some(config_sha256_hex) = config_sha256_hex {
        let expected = payload_hash(&backend_config_signature_payload(&backend_config));
        if expected != normalize_sha256(config_sha256_hex)? {
            return Err(ApiError::bad_request("network_apply_config_hash_mismatch"));
        }
    }
    if payload_hash(endpoint.bird2_interface_snippet.as_bytes())
        != normalize_sha256(bird2_sha256_hex)?
    {
        return Err(ApiError::bad_request("network_apply_bird2_hash_mismatch"));
    }
    Ok(())
}

fn validate_network_ospf_cost_update_operation(
    plan: &vpsman_common::TunnelPlan,
    side: vpsman_common::TunnelEndpointSide,
    current_ospf_cost: u16,
    recommended_ospf_cost: u16,
    bird2_sha256_hex: &str,
) -> Result<(), ApiError> {
    if plan.mutates_host {
        return Err(ApiError::bad_request(
            "network_ospf_cost_update_plan_must_be_observe_plan",
        ));
    }
    validate_runtime_tunnel_control(&plan.runtime_control)
        .map_err(|_| ApiError::bad_request("network_runtime_control_invalid"))?;
    validate_runtime_topology_intent(&plan.runtime_topology, &plan.interface_name)
        .map_err(|_| ApiError::bad_request("network_runtime_topology_invalid"))?;
    if current_ospf_cost == recommended_ospf_cost {
        return Err(ApiError::bad_request("network_ospf_cost_update_noop"));
    }
    if plan.recommended_ospf_cost != recommended_ospf_cost {
        return Err(ApiError::bad_request(
            "network_ospf_cost_update_plan_cost_mismatch",
        ));
    }
    let endpoint = render_tunnel_endpoint_config(plan, side)
        .map_err(|_| ApiError::bad_request("network_ospf_cost_update_plan_invalid"))?;
    if payload_hash(endpoint.bird2_interface_snippet.as_bytes())
        != normalize_sha256(bird2_sha256_hex)?
    {
        return Err(ApiError::bad_request(
            "network_ospf_cost_update_bird2_hash_mismatch",
        ));
    }
    Ok(())
}

fn validate_network_plan_side(
    plan: &vpsman_common::TunnelPlan,
    side: vpsman_common::TunnelEndpointSide,
    mutating_error: &'static str,
    invalid_error: &'static str,
) -> Result<(), ApiError> {
    if plan.mutates_host {
        return Err(ApiError::bad_request(mutating_error));
    }
    validate_runtime_tunnel_control(&plan.runtime_control)
        .map_err(|_| ApiError::bad_request("network_runtime_control_invalid"))?;
    validate_runtime_topology_intent(&plan.runtime_topology, &plan.interface_name)
        .map_err(|_| ApiError::bad_request("network_runtime_topology_invalid"))?;
    render_tunnel_endpoint_config(plan, side).map_err(|_| ApiError::bad_request(invalid_error))?;
    Ok(())
}

fn validate_network_probe_operation(
    plan: &vpsman_common::TunnelPlan,
    side: vpsman_common::TunnelEndpointSide,
    count: u8,
    interval_ms: u16,
) -> Result<(), ApiError> {
    validate_network_plan_side(
        plan,
        side,
        "network_probe_plan_must_be_observe_plan",
        "network_probe_plan_invalid",
    )?;
    if !(1..=20).contains(&count) {
        return Err(ApiError::bad_request("network_probe_count_out_of_range"));
    }
    if !(200..=10_000).contains(&interval_ms) {
        return Err(ApiError::bad_request(
            "network_probe_interval_ms_out_of_range",
        ));
    }
    Ok(())
}

fn validate_network_speed_test_operation(
    plan: &vpsman_common::TunnelPlan,
    server_side: vpsman_common::TunnelEndpointSide,
    duration_secs: u8,
    max_bytes: u64,
    rate_limit_kbps: u32,
    port: u16,
    connect_timeout_ms: u16,
) -> Result<(), ApiError> {
    validate_network_plan_side(
        plan,
        server_side,
        "network_speed_test_plan_must_be_observe_plan",
        "network_speed_test_plan_invalid",
    )?;
    if !(NETWORK_SPEED_TEST_MIN_DURATION_SECS..=NETWORK_SPEED_TEST_MAX_DURATION_SECS)
        .contains(&duration_secs)
    {
        return Err(ApiError::bad_request(
            "network_speed_test_duration_secs_out_of_range",
        ));
    }
    if !(NETWORK_SPEED_TEST_MIN_MAX_BYTES..=NETWORK_SPEED_TEST_MAX_MAX_BYTES).contains(&max_bytes) {
        return Err(ApiError::bad_request(
            "network_speed_test_max_bytes_out_of_range",
        ));
    }
    if !(NETWORK_SPEED_TEST_MIN_RATE_LIMIT_KBPS..=NETWORK_SPEED_TEST_MAX_RATE_LIMIT_KBPS)
        .contains(&rate_limit_kbps)
    {
        return Err(ApiError::bad_request(
            "network_speed_test_rate_limit_kbps_out_of_range",
        ));
    }
    if !(NETWORK_SPEED_TEST_MIN_PORT..=NETWORK_SPEED_TEST_MAX_PORT).contains(&port) {
        return Err(ApiError::bad_request(
            "network_speed_test_port_out_of_range",
        ));
    }
    if !(NETWORK_SPEED_TEST_MIN_CONNECT_TIMEOUT_MS..=NETWORK_SPEED_TEST_MAX_CONNECT_TIMEOUT_MS)
        .contains(&connect_timeout_ms)
    {
        return Err(ApiError::bad_request(
            "network_speed_test_connect_timeout_ms_out_of_range",
        ));
    }
    Ok(())
}

fn path_contains_dot_segment(path: &str) -> bool {
    path.split('/')
        .any(|segment| segment == "." || segment == "..")
}

fn validate_process_start(
    name: &str,
    argv: &[String],
    cwd: Option<&str>,
    env: &std::collections::BTreeMap<String, String>,
    policy: &ProcessRunPolicy,
    limits: &ProcessResourceLimits,
) -> Result<(), ApiError> {
    validate_process_name(name)?;
    if argv.is_empty() {
        return Err(ApiError::bad_request("process_argv_required"));
    }
    if argv.len() > 64 {
        return Err(ApiError::bad_request("process_argv_too_large"));
    }
    if argv
        .iter()
        .any(|part| part.is_empty() || part.len() > 4096 || part.as_bytes().contains(&0))
    {
        return Err(ApiError::bad_request("invalid_process_argv"));
    }
    if !argv[0].starts_with('/') {
        return Err(ApiError::bad_request("process_executable_must_be_absolute"));
    }
    if let Some(cwd) = cwd {
        if cwd.len() > 4096 || !cwd.starts_with('/') || cwd.as_bytes().contains(&0) {
            return Err(ApiError::bad_request("invalid_process_cwd"));
        }
    }
    if env.len() > 32 {
        return Err(ApiError::bad_request("process_env_too_large"));
    }
    for (key, value) in env {
        if key.is_empty()
            || key.len() > 128
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(ApiError::bad_request("invalid_process_env_key"));
        }
        if value.len() > 4096 || value.as_bytes().contains(&0) {
            return Err(ApiError::bad_request("invalid_process_env_value"));
        }
    }
    validate_process_policy(policy)?;
    validate_process_limits(limits)?;
    Ok(())
}

fn validate_process_policy(policy: &ProcessRunPolicy) -> Result<(), ApiError> {
    if policy.restart_max_retries > 100 {
        return Err(ApiError::bad_request(
            "process_restart_retries_out_of_range",
        ));
    }
    if policy.restart_backoff_secs > 3600 {
        return Err(ApiError::bad_request(
            "process_restart_backoff_secs_out_of_range",
        ));
    }
    if !(1..=300).contains(&policy.graceful_stop_secs) {
        return Err(ApiError::bad_request(
            "process_graceful_stop_secs_out_of_range",
        ));
    }
    Ok(())
}

fn validate_process_limits(limits: &ProcessResourceLimits) -> Result<(), ApiError> {
    if let Some(value) = limits.memory_max_bytes {
        if !(1024 * 1024..=1024_u64.pow(4)).contains(&value) {
            return Err(ApiError::bad_request("process_memory_limit_out_of_range"));
        }
    }
    if let Some(value) = limits.pids_max {
        if !(1..=65_535).contains(&value) {
            return Err(ApiError::bad_request("process_pids_limit_out_of_range"));
        }
    }
    if let Some(value) = limits.open_files_max {
        if !(16..=1_048_576).contains(&value) {
            return Err(ApiError::bad_request(
                "process_open_files_limit_out_of_range",
            ));
        }
    }
    if let Some(value) = limits.cpu_shares {
        if !(2..=262_144).contains(&value) {
            return Err(ApiError::bad_request(
                "process_cpu_shares_limit_out_of_range",
            ));
        }
    }
    Ok(())
}

fn validate_process_name(name: &str) -> Result<(), ApiError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ApiError::bad_request("invalid_process_name"));
    }
    Ok(())
}

fn validate_hot_config_document(toml_document: &str) -> Result<(), ApiError> {
    if toml_document.is_empty() {
        return Err(ApiError::bad_request("hot_config_required"));
    }
    if toml_document.len() > MAX_AGENT_HOT_CONFIG_BYTES {
        return Err(ApiError::bad_request("hot_config_too_large"));
    }
    let config = toml::from_str::<AgentConfig>(toml_document)
        .map_err(|_| ApiError::bad_request("hot_config_invalid_toml"))?;
    validate_agent_config_shape(&config)
        .map_err(|_| ApiError::bad_request("hot_config_invalid"))?;
    Ok(())
}

fn validate_data_source_config_patch_document(toml_document: &str) -> Result<(), ApiError> {
    if toml_document.is_empty() {
        return Err(ApiError::bad_request("data_source_config_patch_required"));
    }
    if toml_document.len() > MAX_AGENT_HOT_CONFIG_BYTES {
        return Err(ApiError::bad_request("data_source_config_patch_too_large"));
    }
    let value = toml::from_str::<toml::Value>(toml_document)
        .map_err(|_| ApiError::bad_request("data_source_config_patch_invalid_toml"))?;
    let table = value
        .as_table()
        .ok_or_else(|| ApiError::bad_request("data_source_config_patch_invalid"))?;
    if table.is_empty() {
        return Err(ApiError::bad_request("data_source_config_patch_empty"));
    }
    for section in table.keys() {
        validate_data_source_config_patch_section(section)
            .map_err(|_| ApiError::bad_request("data_source_config_patch_section_not_allowed"))?;
    }
    Ok(())
}

fn validate_update_agent(
    artifact_url: &str,
    sha256_hex: &str,
    artifact_signature_hex: Option<&str>,
    artifact_signing_key_hex: Option<&str>,
) -> Result<(), ApiError> {
    if artifact_url.is_empty() || artifact_url.len() > 2048 || artifact_url.as_bytes().contains(&0)
    {
        return Err(ApiError::bad_request("invalid_update_artifact_url"));
    }
    if !artifact_url.starts_with("https://") {
        return Err(ApiError::bad_request("update_artifact_url_must_be_https"));
    }
    if !is_sha256_hex(sha256_hex) {
        return Err(ApiError::bad_request("invalid_update_sha256"));
    }
    match (artifact_signature_hex, artifact_signing_key_hex) {
        (Some(signature), Some(signing_key)) => {
            if !is_hex_len(signature, 128) {
                return Err(ApiError::bad_request("invalid_update_artifact_signature"));
            }
            if !is_hex_len(signing_key, 64) {
                return Err(ApiError::bad_request("invalid_update_artifact_signing_key"));
            }
            if !verify_update_artifact_signature(
                signing_key,
                signature,
                &sha256_hex.to_ascii_lowercase(),
            ) {
                return Err(ApiError::bad_request("invalid_update_artifact_signature"));
            }
        }
        (None, None) => {}
        _ => {
            return Err(ApiError::bad_request(
                "update_artifact_signature_and_key_required_together",
            ));
        }
    }
    Ok(())
}

fn validate_update_manifest_url(version_url: &str) -> Result<(), ApiError> {
    if version_url.is_empty() || version_url.len() > 2048 || version_url.as_bytes().contains(&0) {
        return Err(ApiError::bad_request("invalid_update_manifest_url"));
    }
    if version_url.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = version_url.strip_prefix("http://") {
        if is_localhost_http_authority(rest) {
            return Ok(());
        }
        return Err(ApiError::bad_request(
            "update_manifest_url_http_must_be_localhost",
        ));
    }
    if let Some(path) = version_url.strip_prefix("file://") {
        if path.starts_with('/') {
            return Ok(());
        }
        return Err(ApiError::bad_request(
            "update_manifest_url_file_must_be_absolute",
        ));
    }
    Err(ApiError::bad_request("update_manifest_url_must_be_https"))
}

fn is_localhost_http_authority(rest: &str) -> bool {
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, _suffix)) = rest.split_once(']') else {
            return false;
        };
        host
    } else {
        match authority.rsplit_once(':') {
            Some((host, _port)) if !host.contains(':') => host,
            _ => authority,
        }
    };
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn is_hex_len(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn is_sha256_hex(value: &str) -> bool {
    is_fixed_hex(value, 64)
}

fn validate_sha256_hex(value: &str, error_code: &'static str) -> Result<(), ApiError> {
    if is_sha256_hex(value) {
        Ok(())
    } else {
        Err(ApiError::bad_request(error_code))
    }
}

fn validate_hex32(value: &str, error_code: &'static str) -> Result<(), ApiError> {
    if is_fixed_hex(value, 64) {
        Ok(())
    } else {
        Err(ApiError::bad_request(error_code))
    }
}

fn normalize_sha256(value: &str) -> Result<String, ApiError> {
    let normalized = value.trim().to_ascii_lowercase();
    if !is_sha256_hex(&normalized) {
        return Err(ApiError::bad_request("invalid_sha256"));
    }
    Ok(normalized)
}

fn is_fixed_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.as_bytes().iter().all(u8::is_ascii_hexdigit)
}
