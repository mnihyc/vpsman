use std::io::{self, BufRead, Write};

use crate::http::http_get;
use crate::vty_agent_update::{is_vty_agent_update_command, submit_vty_agent_update_command};
use crate::vty_auth::{is_vty_totp_command, submit_vty_totp_command};
use crate::vty_backup_artifacts::{
    parse_vty_backup_artifact_handoff, parse_vty_backup_artifact_record,
    parse_vty_backup_artifact_upload, parse_vty_backup_artifact_upload_chunked,
    submit_vty_backup_artifact_handoff, submit_vty_backup_artifact_record,
    submit_vty_backup_artifact_upload, submit_vty_backup_artifact_upload_chunked,
};
use crate::vty_backups::{
    parse_vty_backup_policy_upsert, parse_vty_backup_request, parse_vty_backup_run,
    parse_vty_restore_plan, parse_vty_restore_rollback, parse_vty_restore_run,
    submit_vty_backup_policy_upsert, submit_vty_backup_request, submit_vty_backup_run,
    submit_vty_restore_plan, submit_vty_restore_rollback, submit_vty_restore_run,
};
use crate::vty_config::{
    parse_vty_data_source_hot_config_apply, parse_vty_hot_config,
    submit_vty_data_source_hot_config_apply, submit_vty_hot_config,
};
use crate::vty_direct::submit_vty_direct_command;
use crate::vty_file_transfer::{
    parse_vty_file_transfer_download, parse_vty_file_transfer_upload,
    submit_vty_file_transfer_download, submit_vty_file_transfer_upload,
};
use crate::vty_file_transfers::{is_vty_file_transfers_command, submit_vty_file_transfers_command};
use crate::vty_files::{parse_vty_file_pull, parse_vty_file_push};
use crate::vty_inventory::{
    gateway_sessions_path, is_vty_gateway_sessions_command, is_vty_inventory_command,
    submit_vty_inventory_command,
};
use crate::vty_job_outputs::{is_vty_job_output_command, submit_vty_job_output_command};
use crate::vty_jobs::{
    vty_create_job, vty_create_shell_script, vty_submit_operation, vty_submit_operation_with_force,
    VtyJobSelection, VtyPrivilegeContext,
};
use crate::vty_migrations::{
    parse_vty_migration_link, parse_vty_migration_run, submit_vty_migration_link,
    submit_vty_migration_run,
};
use crate::vty_network_dispatch::{
    is_vty_network_dispatch_command, submit_vty_network_dispatch_command,
};
use crate::vty_network_observations::{
    is_vty_network_evidence_command, submit_vty_network_evidence_command,
};
use crate::vty_privilege::{
    render_vty_capabilities, render_vty_degraded_policy, render_vty_privilege_status,
    vty_privilege_help,
};
use crate::vty_process::{
    is_vty_process_supervisor_inventory_command, parse_vty_process_list,
    parse_vty_process_supervisor, parse_vty_user_sessions, process_supervisor_inventory_path,
    process_supervisor_usage,
};
use crate::vty_schedules::{
    parse_vty_schedule_create_options, submit_vty_schedule_create, VtyScheduleCreateRequest,
};
use crate::vty_terminal::{is_vty_terminal_command, submit_vty_terminal_command};
use crate::vty_terminal_sessions::{
    is_vty_terminal_sessions_command, submit_vty_terminal_sessions_command,
};
use crate::vty_update_releases::{
    is_vty_agent_update_releases_command, parse_vty_agent_update_release_record,
    submit_vty_agent_update_release_record, submit_vty_agent_update_releases,
};
use anyhow::Result;

const VTY_COMMAND_HELP: &str = r#"Commands

Core:
  health | summary | agents
  gateway-sessions [--limit <1-200>]
  telemetry-rollups | telemetry-network-rates | telemetry-tunnels
  enable | disable | show privilege | show capabilities | show degraded-policy

Access:
  operators | operator-create | operator-update | operator-disable | operator-enable
  operator-delete | operator-password-reset | operator-totp-clear
  operator-sessions | operator-session-revoke | operator-auth-events
  operator mutations require enable and --confirmed
  totp-setup | totp-confirm | totp-disable
  agent-identity-upsert | client-key-revoke (privileged mode required)
  client-key-revocations | key-lifecycle-report

Fleet and integrations:
  tags | tag-create <name> --confirmed | agent-tag <client_id> <tag> --confirmed
  fleet-alerts | fleet-alert-export | fleet-alert-states | fleet-alert-state-update
  fleet-alert-policies | fleet-alert-policy-upsert
  fleet-alert-notification-channels | fleet-alert-notification-channel-upsert
  fleet-alert-notifications | fleet-alert-notification-dispatch | fleet-alert-notification-process
  data-source-presets | data-source-preset-create | data-source-preset-clone
  data-source-preset-diff | data-source-preset-test | data-source-preset-update
  data-source-status | data-source-assignments | data-source-preset-assign
  data-source-hot-config | data-source-hot-config-apply

Jobs and schedules:
  jobs | job-create | job-shell | job-targets | job-target-status-download
  job-outputs | job-follow | job-output-download
  schedules | schedule-create
  server-jobs | artifact-cleanup-preview | artifact-cleanup-create | server-job-cancel

Files, terminals, and processes:
  file-pull | file-push
  file-transfer-upload (--source <file>|--source-artifact-id <uuid>)
    --path <remote-abs> <target ...> --confirmed
  file-transfer-download --path <remote-abs> --destination <local> <target ...>
    --confirmed
  file-transfers | file-transfer-handoff | file-transfer-sources
  file-transfer-source-upload | file-transfer-source-download
  terminal-open | terminal-input | terminal-poll | terminal-resize | terminal-close
  terminal-sessions | terminal-replay | terminal-follow
  user-sessions | process-list | process-start | process-stop | process-restart
  process-status | process-logs | process-supervisor-inventory

Agent updates:
  agent-update-check [--version-url <https-url>] <target ...> [--no-activate]
    [--no-restart-agent] [--timeout <secs>] [--privilege-ttl <15-300>]
    [--force-unprivileged] --confirmed
  agent-update --artifact-url <https-url> --sha256-hex <sha256> <target ...>
    [--timeout <secs>] [--privilege-ttl <15-300>] [--force-unprivileged]
    --confirmed
  agent-update-activate --staged-sha256-hex <sha256> <target ...>
    [--restart-agent] [--timeout <secs>] [--privilege-ttl <15-300>]
    [--force-unprivileged] --confirmed
  agent-update-rollback [--rollback-sha256-hex <sha256>] <target ...>
    [--timeout <secs>] [--privilege-ttl <15-300>] [--force-unprivileged]
    --confirmed
  agent-update-releases | agent-update-release-latest | agent-update-release-record

Backups, restores, and migrations:
  backups | backup-policies | backup-artifacts | backup-policy-upsert
  backup-policy-prune | backup-request | backup-run
  backup-artifact-record | backup-artifact-upload | backup-artifact-upload-chunked
  backup-artifact-handoff
  restore-plans | restore-plan | restore-run | restore-rollback
  migration-links | migration-link | migration-run

Network and topology:
  tunnel-plans | tunnel-plan | tunnel-allocate | tunnel-promote-telemetry
  tunnel-promote-adapter | tunnel-apply | tunnel-ospf-cost-update
  tunnel-rollback | tunnel-status | tunnel-probe | tunnel-speed-test
  network-observations | network-trends | network-ospf-recommendations
  network-ospf-update-plans | topology-graph

Audit and history:
  audit | history-retention | history-retention-upsert
  history-retention-prune | history-export

Targets:
  bulk-resolve <target ...>
  id:<client-id> | name:<display-name> | tag:<name>
  Bare target tokens are treated as tags.
"#;

const PRIVILEGE_UNLOCK_REQUIRED: &str = concat!(
    "privilege unlock is required; run enable after setting ",
    "VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
);

const TARGET_SELECTION_USAGE: &str =
    "targets: id:<client-id> name:<display-name> tag:<name>; bare targets are tags";

const FILE_TRANSFERS_USAGE: &str = concat!(
    "usage: file-transfers [--limit <1-200>] [--client-id <id>] ",
    "[--session-id <uuid>]\n",
    "       file-transfers reports handoff_evidence_status and ",
    "handoff_unavailable_reason\n",
    "       file-transfer-handoff --client-id <id> --session-id <uuid> ",
    "[--output-file <file>] --confirmed\n",
    "       file-transfer-sources [--limit <1-200>]\n",
    "       file-transfer-source-upload --source <file> [--name <name>] --confirmed\n",
    "       file-transfer-source-download --artifact-id <uuid> --output-file <file>"
);

const TERMINAL_SESSIONS_USAGE: &str = concat!(
    "usage: terminal-sessions [--limit <1-200>] [--client-id <id>] ",
    "[--session-id <uuid>]\n",
    "       terminal-replay --client-id <id> --session-id <uuid> ",
    "[--from-seq <n>] [--limit <1-1000>] [--max-bytes <1-4194304>] ",
    "[--output-file <file>] [--metadata-only]\n",
    "       terminal-follow --client-id <id> --session-id <uuid> [--from-seq <n>] ",
    "[--interval-ms <250-10000>] [--max-polls <1-1000>] [--json]"
);

const SCHEDULE_CREATE_USAGE: &str = concat!(
    "usage: schedule-create <name> <cron_min> <cron_hour> <cron_dom> ",
    "<cron_mon> <cron_dow> <command> [schedule policy flags] <target ...> --confirmed"
);

const TERMINAL_COMMAND_USAGE: &str = concat!(
    "usage: terminal-open --argv </abs/bin,arg> <target ...> [--session-id <uuid>] ",
    "[--cols <20-240>] [--rows <5-120>] [--confirmed]\n",
    "usage: terminal-input --client-id <id> --session-id <uuid> ",
    "(--text <text>|--data-base64 <b64>) --confirmed\n",
    "usage: terminal-poll --session-id <uuid> [--replay-from-seq <n>] <target ...>\n",
    "usage: terminal-resize --session-id <uuid> --cols <20-240> ",
    "--rows <5-120> <target ...>\n",
    "usage: terminal-close --session-id <uuid> <target ...> [--reason <text>]"
);

const FILE_PUSH_USAGE: &str = concat!(
    "usage: file-push --source <local-file> --path <remote-abs> <target ...> ",
    "[--mode <0644>] [--timeout <secs>] --confirmed"
);

const FILE_TRANSFER_UPLOAD_USAGE: &str = concat!(
    "usage: file-transfer-upload (--source <local-file>|--source-artifact-id <uuid>) ",
    "--path <remote-abs> <target ...> [--mode <0644>] [--session-id <uuid>] ",
    "[--resume-token <token>] [--chunk-size-bytes <1-65536>] ",
    "[--rate-limit-kbps <0-1000000>] ",
    "[--multi-target-policy same-offset|independent-offsets] ",
    "[--timeout <secs>] [--privilege-ttl <15-300>] --confirmed"
);

const FILE_TRANSFER_DOWNLOAD_USAGE: &str = concat!(
    "usage: file-transfer-download --path <remote-abs> ",
    "--destination <local-file-or-dir> <target ...> [--session-id <uuid>] ",
    "[--resume-token <token>] [--chunk-size-bytes <1-65536>] ",
    "[--rate-limit-kbps <0-1000000>] ",
    "[--multi-target-policy single-target|per-target-files] ",
    "[--timeout <secs>] [--privilege-ttl <15-300>] --confirmed"
);

const HOT_CONFIG_USAGE: &str = concat!(
    "usage: hot-config --config-file <path> <target ...> ",
    "[--timeout <secs>] [--privilege-ttl <15-300>] ",
    "[--force-unprivileged] --confirmed"
);

const DATA_SOURCE_HOT_CONFIG_USAGE: &str = concat!(
    "usage: data-source-hot-config-apply --client-id <id> ",
    "[--timeout <secs>] [--privilege-ttl <15-300>] ",
    "[--force-unprivileged] --confirmed"
);

const RESTORE_RUN_USAGE: &str = concat!(
    "usage: restore-run <backup_uuid> <target_client_id> ",
    "--archive-transfer-session-id <uuid> [--timeout <secs>] ",
    "[--force-unprivileged] --confirmed"
);

const RESTORE_ROLLBACK_USAGE: &str = concat!(
    "usage: restore-rollback <restore_job_uuid> <target_client_id> ",
    "[--timeout <secs>] [--force-unprivileged] --confirmed"
);

const MIGRATION_RUN_USAGE: &str = concat!(
    "usage: migration-run <restore_plan_uuid> --archive-transfer-session-id <uuid> ",
    "[--note <text>] ",
    "[--timeout <secs>] [--force-unprivileged] --confirmed"
);

fn render_vty_help() -> String {
    format!("{VTY_COMMAND_HELP}\n{}", vty_privilege_help())
}

pub(crate) fn run_vty(api_url: &str) -> Result<()> {
    println!("vpsman VTY connected to {api_url}");
    println!("{}", render_vty_help());
    let stdin = io::stdin();
    let mut privilege_context = VtyPrivilegeContext::default();
    let token = std::env::var("VPSMAN_API_TOKEN").ok();

    loop {
        print!(
            "{}",
            if privilege_context.enabled {
                "vpsman# "
            } else {
                "vpsman> "
            }
        );
        io::stdout().flush()?;

        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }

        let command = line.trim();
        if command.is_empty() {
            continue;
        }
        if let Some(output) =
            submit_vty_direct_command(api_url, token.as_deref(), command, &privilege_context)?
        {
            println!("{output}");
            continue;
        }

        match command {
            command if is_vty_totp_command(command) => {
                match submit_vty_totp_command(api_url, token.as_deref(), command) {
                    Ok(output) => println!("{output}"),
                    Err(error) => println!("usage error: {error}"),
                }
            }
            command if is_vty_file_transfers_command(command) => {
                match submit_vty_file_transfers_command(api_url, token.as_deref(), command) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{FILE_TRANSFERS_USAGE}");
                    }
                }
            }
            command if is_vty_terminal_sessions_command(command) => {
                match submit_vty_terminal_sessions_command(api_url, token.as_deref(), command) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{TERMINAL_SESSIONS_USAGE}");
                    }
                }
            }
            command if is_vty_gateway_sessions_command(command) => {
                let path = match gateway_sessions_path(command) {
                    Ok(path) => path,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("usage: gateway-sessions [--limit <1-200>]");
                        continue;
                    }
                };
                println!("{}", http_get(api_url, &path, token.as_deref())?);
            }
            command if is_vty_agent_update_releases_command(command) => println!(
                "{}",
                submit_vty_agent_update_releases(api_url, token.as_deref(), command)?
            ),
            command if command.starts_with("agent-update-release-record ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                match parse_vty_agent_update_release_record(&parts[1..]).and_then(|request| {
                    submit_vty_agent_update_release_record(api_url, token.as_deref(), request)
                }) {
                    Ok(output) => println!("{output}"),
                    Err(error) => println!("usage error: {error}"),
                }
            }
            command if is_vty_network_evidence_command(command) => println!(
                "{}",
                submit_vty_network_evidence_command(api_url, token.as_deref(), command)?
            ),
            command if command.starts_with("schedule-create ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if parts.len() < 8 {
                    println!("{SCHEDULE_CREATE_USAGE}");
                    continue;
                }
                let cron_expr = parts[2..7].join(" ");
                let schedule_options = match parse_vty_schedule_create_options(&parts[8..]) {
                    Ok(options) => options,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                let target_refs = schedule_options
                    .target_tokens
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                let selection = match VtyJobSelection::parse(&target_refs) {
                    Ok(selection) => selection,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                if selection.destructive {
                    println!("usage error: schedule-create does not accept --destructive");
                    continue;
                }
                if !privilege_context.enabled {
                    println!(
                        "usage error: schedule-create requires privilege unlock; run enable first"
                    );
                    continue;
                }
                println!(
                    "{}",
                    submit_vty_schedule_create(VtyScheduleCreateRequest {
                        api_url,
                        token: token.as_deref(),
                        name: parts[1],
                        cron_expr: &cron_expr,
                        command: parts[7],
                        selection,
                        options: &schedule_options,
                        privilege_context: &privilege_context,
                    })?
                );
            }
            command if command.starts_with("job-create ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if parts.len() < 3 {
                    println!(
                        "usage: job-create <command> <target ...> [--pty --destructive --confirmed]"
                    );
                    println!("{TARGET_SELECTION_USAGE}");
                    continue;
                }
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let pty = parts.contains(&"--pty");
                let target_parts = parts[2..]
                    .iter()
                    .copied()
                    .filter(|part| *part != "--pty")
                    .collect::<Vec<_>>();
                let selection = match VtyJobSelection::parse(&target_parts) {
                    Ok(selection) => selection,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    vty_create_job(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        parts[1],
                        pty,
                        selection
                    )?
                );
            }
            command if command.starts_with("job-shell ") => {
                let body = command.trim_start_matches("job-shell ").trim();
                let Some((script, target_text)) = body.split_once(" -- ") else {
                    println!("usage: job-shell <shell-script> -- <target ...>");
                    println!("{TARGET_SELECTION_USAGE}");
                    continue;
                };
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let target_parts = target_text.split_whitespace().collect::<Vec<_>>();
                let selection = match VtyJobSelection::parse(&target_parts) {
                    Ok(selection) => selection,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    vty_create_shell_script(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        script.trim(),
                        selection
                    )?
                );
            }
            command if is_vty_terminal_command(command) => {
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                match submit_vty_terminal_command(
                    api_url,
                    token.as_deref(),
                    &privilege_context,
                    command,
                ) {
                    Ok(response) => println!("{response}"),
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{TERMINAL_COMMAND_USAGE}");
                    }
                }
            }
            command if command.starts_with("file-pull ") || command.starts_with("file-push ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = if parts[0] == "file-pull" {
                    match parse_vty_file_pull(&parts[1..]) {
                        Ok(request) => request,
                        Err(error) => {
                            println!("usage error: {error}");
                            println!(
                                "usage: file-pull --path <remote-abs> <target ...> [--timeout <secs>] [--confirmed]"
                            );
                            continue;
                        }
                    }
                } else {
                    match parse_vty_file_push(&parts[1..]) {
                        Ok(request) => request,
                        Err(error) => {
                            println!("usage error: {error}");
                            println!("{FILE_PUSH_USAGE}");
                            continue;
                        }
                    }
                };
                println!(
                    "{}",
                    vty_submit_operation(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request.command_label,
                        &request.operation,
                        request.selection,
                        request.timeout_secs,
                    )?
                );
            }
            command if command.starts_with("file-transfer-upload ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_file_transfer_upload(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{FILE_TRANSFER_UPLOAD_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_file_transfer_upload(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request,
                    )?
                );
            }
            command if command.starts_with("file-transfer-download ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_file_transfer_download(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{FILE_TRANSFER_DOWNLOAD_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_file_transfer_download(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request,
                    )?
                );
            }
            command if command.starts_with("hot-config ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_hot_config(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{HOT_CONFIG_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_hot_config(
                        api_url,
                        token.as_deref(),
                        &privilege_context.password,
                        &privilege_context.salt_hex,
                        request,
                    )?
                );
            }
            command if command.starts_with("data-source-hot-config-apply ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_data_source_hot_config_apply(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{DATA_SOURCE_HOT_CONFIG_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_data_source_hot_config_apply(
                        api_url,
                        token.as_deref(),
                        &privilege_context.password,
                        &privilege_context.salt_hex,
                        request,
                    )?
                );
            }
            command if is_vty_agent_update_command(command) => {
                match submit_vty_agent_update_command(
                    api_url,
                    token.as_deref(),
                    &privilege_context,
                    command,
                ) {
                    Ok(response) => println!("{response}"),
                    Err(error) => println!("usage error: {error}"),
                }
            }
            command if command.starts_with("user-sessions ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_user_sessions(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: user-sessions <target ...> [--timeout <secs>] [--confirmed]"
                        );
                        continue;
                    }
                };
                println!(
                    "{}",
                    vty_submit_operation(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request.command_label,
                        &request.operation,
                        request.selection,
                        request.timeout_secs,
                    )?
                );
            }
            command if command.starts_with("process-list ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if parts.len() < 2 {
                    println!("usage: process-list <target ...> [--limit <1-512>] [--confirmed]");
                    continue;
                }
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_process_list(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    vty_submit_operation(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request.command_label,
                        &request.operation,
                        request.selection,
                        request.timeout_secs,
                    )?
                );
            }
            command if is_vty_process_supervisor_inventory_command(command) => {
                let path = match process_supervisor_inventory_path(command) {
                    Ok(path) => path,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("usage: process-supervisor-inventory [--limit <1-200>]");
                        continue;
                    }
                };
                println!("{}", http_get(api_url, &path, token.as_deref())?);
            }
            command
                if command.starts_with("process-start ")
                    || command.starts_with("process-stop ")
                    || command.starts_with("process-restart ")
                    || command.starts_with("process-status ")
                    || command.starts_with("process-logs ") =>
            {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_process_supervisor(parts[0], &parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{}", process_supervisor_usage(parts[0]));
                        continue;
                    }
                };
                println!(
                    "{}",
                    vty_submit_operation_with_force(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request.command_label,
                        &request.operation,
                        request.selection,
                        request.timeout_secs,
                        request.force_unprivileged,
                    )?
                );
            }
            command if is_vty_job_output_command(command) => {
                println!(
                    "{}",
                    submit_vty_job_output_command(api_url, token.as_deref(), command)?
                );
            }
            command if command.starts_with("backup-policy-upsert ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_backup_policy_upsert(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_policy_upsert(api_url, token.as_deref(), request)?
                );
            }
            command if command.starts_with("backup-request ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_backup_request(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_request(
                        api_url,
                        token.as_deref(),
                        &privilege_context.password,
                        &privilege_context.salt_hex,
                        request,
                    )?
                );
            }
            command if command.starts_with("backup-run ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_backup_run(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_run(api_url, token.as_deref(), &privilege_context, request,)?
                );
            }
            command if command.starts_with("backup-artifact-record ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_backup_artifact_record(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_artifact_record(api_url, token.as_deref(), request)?
                );
            }
            command if command.starts_with("backup-artifact-upload-chunked ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_backup_artifact_upload_chunked(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_artifact_upload_chunked(api_url, token.as_deref(), request)?
                );
            }
            command if command.starts_with("backup-artifact-upload ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_backup_artifact_upload(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_artifact_upload(api_url, token.as_deref(), request)?
                );
            }
            command if command.starts_with("backup-artifact-handoff ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_backup_artifact_handoff(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_backup_artifact_handoff(api_url, token.as_deref(), request)?
                );
            }
            command if command.starts_with("restore-plan ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_restore_plan(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_restore_plan(
                        api_url,
                        token.as_deref(),
                        &privilege_context.password,
                        &privilege_context.salt_hex,
                        request,
                    )?
                );
            }
            command if command.starts_with("restore-run ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_restore_run(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{RESTORE_RUN_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_restore_run(api_url, token.as_deref(), &privilege_context, request)?
                );
            }
            command if command.starts_with("restore-rollback ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!("{PRIVILEGE_UNLOCK_REQUIRED}");
                    continue;
                }
                let request = match parse_vty_restore_rollback(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{RESTORE_ROLLBACK_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_restore_rollback(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request,
                    )?
                );
            }
            command if command.starts_with("migration-link ") => {
                if !privilege_context.enabled {
                    println!("enter privileged mode first with: enable");
                    continue;
                }
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_migration_link(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: migration-link <restore_plan_uuid> [--note <text>] --confirmed"
                        );
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_migration_link(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request,
                    )?
                );
            }
            command if command.starts_with("migration-run ") => {
                if !privilege_context.enabled {
                    println!("enter privileged mode first with: enable");
                    continue;
                }
                let parts = command.split_whitespace().collect::<Vec<_>>();
                let request = match parse_vty_migration_run(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!("{MIGRATION_RUN_USAGE}");
                        continue;
                    }
                };
                println!(
                    "{}",
                    submit_vty_migration_run(
                        api_url,
                        token.as_deref(),
                        &privilege_context,
                        request,
                    )?
                );
            }
            command if is_vty_network_dispatch_command(command) => {
                submit_vty_network_dispatch_command(
                    api_url,
                    token.as_deref(),
                    &privilege_context,
                    command,
                )?;
            }
            command if is_vty_inventory_command(command) => {
                match submit_vty_inventory_command(api_url, token.as_deref(), command) {
                    Ok(output) => println!("{output}"),
                    Err(error) => println!("usage error: {error}"),
                }
            }
            "enable" => match VtyPrivilegeContext::from_env() {
                Ok(context) => {
                    privilege_context = context;
                    println!(
                        "privileged mode enabled locally; privilege assertions will be \
                         generated without sending the super password"
                    );
                }
                Err(error) => {
                    privilege_context = VtyPrivilegeContext::default();
                    println!("enable failed: {error}");
                }
            },
            "disable" => {
                privilege_context = VtyPrivilegeContext::default();
                println!(
                    "privileged mode disabled; local privilege unlock material cleared for this \
                     VTY session"
                );
            }
            "show privilege" => println!("{}", render_vty_privilege_status(&privilege_context)),
            "show capabilities" => println!("{}", render_vty_capabilities()),
            "show degraded-policy" => println!("{}", render_vty_degraded_policy()),
            "exit" | "quit" => break,
            "help" | "?" => println!("{}", render_vty_help()),
            other => println!("unknown command: {other}"),
        }
    }

    Ok(())
}
