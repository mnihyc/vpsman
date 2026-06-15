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
    is_vty_agent_update_releases_command, parse_vty_agent_update_artifact_upload,
    parse_vty_agent_update_release_record, submit_vty_agent_update_artifact_upload,
    submit_vty_agent_update_release_record, submit_vty_agent_update_releases,
};
use anyhow::Result;

pub(crate) fn run_vty(api_url: &str) -> Result<()> {
    println!("vpsman VTY connected to {api_url}");
    println!(
        "Commands: health, summary, agents, fleet-alerts, fleet-alert-export, fleet-alert-states, fleet-alert-state-update, fleet-alert-policies, fleet-alert-policy-upsert, fleet-alert-notification-channels, fleet-alert-notification-channel-upsert, fleet-alert-notifications, fleet-alert-notification-dispatch, fleet-alert-notification-process, operators, operator-create, operator-sessions, operator-session-revoke, totp-setup, totp-confirm, totp-disable, agent-identity-upsert, client-key-revocations, client-key-revoke, key-lifecycle-report, gateway-sessions, telemetry-rollups, telemetry-network-rates, telemetry-tunnels, tags, tag-create, agent-tag, data-source-presets, data-source-preset-create, data-source-preset-clone, data-source-preset-diff, data-source-preset-test, data-source-preset-update, data-source-status, data-source-assignments, data-source-hot-config, data-source-hot-config-apply, data-source-preset-assign, jobs, schedules, schedule-create, job-create, job-shell, terminal-open, terminal-input, terminal-poll, terminal-resize, terminal-close, terminal-sessions, terminal-replay, terminal-follow, file-pull, file-push, file-transfer-upload, file-transfer-download, file-transfers, file-transfer-handoff, file-transfer-sources, file-transfer-source-upload, file-transfer-source-download, user-sessions, hot-config, agent-update, agent-update-activate, agent-update-rollback, agent-update-releases, agent-update-release-record, agent-update-artifact-upload, process-list, process-start, process-stop, process-restart, process-status, process-logs, process-supervisor-inventory, job-targets, job-target-status-download, job-outputs, job-follow, job-output-download, backups, backup-policies, backup-artifacts, backup-policy-upsert, backup-policy-prune, backup-request, backup-run, backup-artifact-record, backup-artifact-upload, backup-artifact-upload-chunked, backup-artifact-handoff, restore-plans, restore-plan, restore-run, restore-rollback, migration-links, migration-link, migration-run, tunnel-plans, tunnel-plan, tunnel-allocate, tunnel-promote-telemetry, tunnel-promote-adapter, tunnel-apply, tunnel-ospf-cost-update, tunnel-rollback, tunnel-status, tunnel-probe, tunnel-speed-test, network-observations, network-trends, network-ospf-recommendations, network-ospf-update-plans, topology-graph, audit, history-retention, history-retention-upsert, history-retention-prune, history-export, bulk-resolve, enable, exit"
    );
    println!("{}", vty_privilege_help());
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
        if let Some(output) = submit_vty_direct_command(api_url, token.as_deref(), command)? {
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
                        println!(
                            "usage: file-transfers [--limit <1-200>] [--client-id <id>] [--session-id <uuid>] | file-transfer-handoff --client-id <id> --session-id <uuid> [--output-file <file>] --confirmed | file-transfer-sources [--limit <1-200>] | file-transfer-source-upload --source <file> [--name <name>] --confirmed | file-transfer-source-download --artifact-id <uuid> --output-file <file>"
                        );
                    }
                }
            }
            command if is_vty_terminal_sessions_command(command) => {
                match submit_vty_terminal_sessions_command(api_url, token.as_deref(), command) {
                    Ok(output) => println!("{output}"),
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: terminal-sessions [--limit <1-200>] [--client-id <id>] [--session-id <uuid>] | terminal-replay --client-id <id> --session-id <uuid> [--from-seq <n>] [--limit <1-1000>] [--max-bytes <1-4194304>] [--output-file <file>] [--metadata-only] | terminal-follow --client-id <id> --session-id <uuid> [--from-seq <n>] [--interval-ms <250-10000>] [--max-polls <1-1000>] [--json]"
                        );
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
            command if command.starts_with("agent-update-artifact-upload ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                match parse_vty_agent_update_artifact_upload(&parts[1..]).and_then(|request| {
                    submit_vty_agent_update_artifact_upload(api_url, token.as_deref(), request)
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
                    println!("usage: schedule-create <name> <cron_min> <cron_hour> <cron_dom> <cron_mon> <cron_dow> <command> [schedule policy flags] <target ...>");
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
                    println!("targets: id:<client-id> name:<display-name> tag:<name>; bare targets are tags");
                    continue;
                }
                if !privilege_context.enabled {
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!("targets: id:<client-id> name:<display-name> tag:<name>; bare targets are tags");
                    continue;
                };
                if !privilege_context.enabled {
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                        println!(
                            "usage: terminal-open --argv </abs/bin,arg> <target ...> [--session-id <uuid>] [--cols <20-240>] [--rows <5-120>] [--confirmed]"
                        );
                        println!(
                            "usage: terminal-input --session-id <uuid> --input-seq <n> (--text <text>|--data-base64 <b64>) <target ...>"
                        );
                        println!(
                            "usage: terminal-poll --session-id <uuid> [--replay-from-seq <n>] <target ...>"
                        );
                        println!(
                            "usage: terminal-resize --session-id <uuid> --cols <20-240> --rows <5-120> <target ...>"
                        );
                        println!("usage: terminal-close --session-id <uuid> <target ...> [--reason <text>]");
                    }
                }
            }
            command if command.starts_with("file-pull ") || command.starts_with("file-push ") => {
                let parts = command.split_whitespace().collect::<Vec<_>>();
                if !privilege_context.enabled {
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = if parts[0] == "file-pull" {
                    match parse_vty_file_pull(&parts[1..]) {
                        Ok(request) => request,
                        Err(error) => {
                            println!("usage error: {error}");
                            println!(
                                "usage: file-pull --path <remote-abs> <target ...> [--timeout <1-3600>] [--confirmed]"
                            );
                            continue;
                        }
                    }
                } else {
                    match parse_vty_file_push(&parts[1..]) {
                        Ok(request) => request,
                        Err(error) => {
                            println!("usage error: {error}");
                            println!(
                                "usage: file-push --source <local-file> --path <remote-abs> <target ...> [--mode <0644>] [--timeout <1-3600>] --confirmed"
                            );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_file_transfer_upload(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: file-transfer-upload (--source <local-file>|--source-artifact-id <uuid>) --path <remote-abs> <target ...> [--mode <0644>] [--session-id <uuid>] [--resume-token <token>] [--chunk-size-bytes <1-65536>] [--rate-limit-kbps <0-1000000>] [--multi-target-policy same-offset|independent-offsets] [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed"
                        );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_file_transfer_download(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: file-transfer-download --path <remote-abs> --destination <local-file-or-dir> <target ...> [--session-id <uuid>] [--resume-token <token>] [--chunk-size-bytes <1-65536>] [--rate-limit-kbps <0-1000000>] [--multi-target-policy single-target|per-target-files] [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed"
                        );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_hot_config(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: hot-config --config-file <path> <target ...> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed"
                        );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_data_source_hot_config_apply(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: data-source-hot-config-apply --client-id <id> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed"
                        );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_user_sessions(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: user-sessions <target ...> [--timeout <1-3600>] [--confirmed]"
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_restore_run(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: restore-run <backup_uuid> <target_client_id> [--artifact-file <path>] [--private-key-env <env>] [--path <abs>] [--include-config] [--destination-root <abs>] [--timeout <1-3600>] [--force-unprivileged] --confirmed"
                        );
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
                    println!(
                        "privilege unlock is required; run enable after setting VPSMAN_SUPER_PASSWORD and VPSMAN_SUPER_SALT_HEX"
                    );
                    continue;
                }
                let request = match parse_vty_restore_rollback(&parts[1..]) {
                    Ok(request) => request,
                    Err(error) => {
                        println!("usage error: {error}");
                        println!(
                            "usage: restore-rollback <restore_job_uuid> <target_client_id> [--timeout <1-3600>] [--force-unprivileged] --confirmed"
                        );
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
                    submit_vty_migration_link(api_url, token.as_deref(), request)?
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
                        println!(
                            "usage: migration-run <restore_plan_uuid> [--artifact-file <path>] [--private-key-env <env>] [--note <text>] [--timeout <1-3600>] [--force-unprivileged] --confirmed"
                        );
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
                        "privileged mode enabled locally; privilege assertions will be generated without sending the super password"
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
                        "privileged mode disabled; local privilege unlock material cleared for this VTY session"
                    );
            }
            "show privilege" => println!("{}", render_vty_privilege_status(&privilege_context)),
            "show capabilities" => println!("{}", render_vty_capabilities()),
            "show degraded-policy" => println!("{}", render_vty_degraded_policy()),
            "exit" | "quit" => break,
            "help" | "?" => {
                println!("{}", vty_privilege_help());
                println!(
                    "health | summary | agents | fleet-alerts [filters] | fleet-alert-export [filters] | fleet-alert-states [--state open|acknowledged|muted|escalated] | fleet-alert-state-update --alert-id <id> --action acknowledge|mute|escalate|clear [--muted-for-secs <secs>] [--reason <text>] --confirmed | fleet-alert-policies [--limit <1-1000>] [--enabled true|false] [--scope-kind global|provider|tag|client] [--scope-value <value>] | fleet-alert-policy-upsert --name <name> --scope-kind global|provider|tag|client [--scope-value <value>] [resource threshold flags] [--priority <n>] [--enabled true|false] [--notes <text>] --confirmed | fleet-alert-notification-channels [--limit <1-1000>] [--enabled true|false] [--scope-kind global|provider|tag|client] [--scope-value <value>] [--delivery-kind <kind>] | fleet-alert-notification-channel-upsert --name <name> --scope-kind global|provider|tag|client [--scope-value <value>] [--min-severity critical|warning|info] [--categories a,b] [--operator-states open,escalated] --delivery-kind <kind> --target <target> [--cooldown-secs <secs>] [--enabled true|false] [--notes <text>] --confirmed | fleet-alert-notifications [--limit <1-1000>] [--channel-id <uuid>] [--alert-id <id>] [--status <status>] | fleet-alert-notification-dispatch [filters] [--include-muted] [--dry-run|--confirmed] | fleet-alert-notification-process [--limit <1-200>] [--status queued|failed] [--delivery-kind <kind>] [--dry-run|--confirmed] | operators | operator-create <username> <admin|operator|viewer> <password_env> [scope,scope] | operator-sessions [--limit <1-200>] | operator-session-revoke <session_uuid> | totp-setup [password_env] | totp-confirm [password_env] [code_env] | totp-disable [password_env] [code_env] | gateway-sessions [--limit <1-200>] | telemetry-rollups [--limit <1-200>] [--client-id <id>] [--bucket-secs <60-86400>] | telemetry-network-rates [--limit <1-5000>] [--client-id <id>] [--interface <name>] [--bucket-secs <60-86400>] | telemetry-tunnels [--limit <1-200>] [--client-id <id>] [--interface <name>] | tags | tag-create <name> | agent-tag <client_id> <tag> | data-source-presets [--domain <domain>] | data-source-preset-create --domain <domain> --name <name> [--scope shared|vps_local] [--owner-client-id <client>] [--description <text>] [--definition-json <json>] | data-source-preset-clone --source-preset-id <uuid> --name <name> [--description <text>] | data-source-preset-diff --preset-id <uuid> [--description <text>|--clear-description] [--definition-json <json>] | data-source-preset-test --preset-id <uuid> [--definition-json <json>] | data-source-preset-update --preset-id <uuid> [--description <text>|--clear-description] [--definition-json <json>] [--confirmed] | data-source-status [--client-id <id>] [--domain <domain>] | data-source-assignments [--client-id <id>] [--domain <domain>] | data-source-hot-config --client-id <id> [--format toml|json] | data-source-hot-config-apply --client-id <id> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed | data-source-preset-assign --domain <domain> --preset-id <uuid> [--client <id>] [--tag <tag>] [--confirmed] | jobs | schedules | schedule-create <name> <cron_min> <cron_hour> <cron_dom> <cron_mon> <cron_dow> <command> [--catch-up-policy skip_missed|run_once|run_all_limited] [--catch-up-limit <1-25>] [--retry-delay-secs <1-86400>] [--max-failures <1-100>] <target ...> | job-create <command> <id:<id>|name:<display>|tag:<name>|tag ...> [--destructive --confirmed] | job-shell <shell-script> -- <id:<id>|name:<display>|tag:<name>|tag ...> | file-pull --path <remote-abs> <target ...> [--timeout <1-3600>] [--confirmed] | file-push --source <local-file> --path <remote-abs> <target ...> [--mode <0644>] [--timeout <1-3600>] --confirmed | file-transfer-upload (--source <local-file>|--source-artifact-id <uuid>) --path <remote-abs> <target ...> [--mode <0644>] [--session-id <uuid>] [--resume-token <token>] [--chunk-size-bytes <1-65536>] [--rate-limit-kbps <0-1000000>] [--multi-target-policy same-offset|independent-offsets] [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed | file-transfer-download --path <remote-abs> --destination <local-file-or-dir> <target ...> [--session-id <uuid>] [--resume-token <token>] [--chunk-size-bytes <1-65536>] [--rate-limit-kbps <0-1000000>] [--multi-target-policy single-target|per-target-files] [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed | file-transfers [--limit <1-200>] [--client-id <id>] [--session-id <uuid>] | file-transfer-handoff --client-id <id> --session-id <uuid> [--output-file <file>] --confirmed | file-transfer-sources [--limit <1-200>] | file-transfer-source-upload --source <file> [--name <name>] --confirmed | file-transfer-source-download --artifact-id <uuid> --output-file <file> | user-sessions <target ...> [--timeout <1-3600>] [--confirmed] | hot-config --config-file <path> <target ...> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed | agent-update --artifact-url <https-url> --sha256-hex <sha256> [--artifact-signature-hex <sig>] [--artifact-signing-key-hex <key>] <target ...> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed | agent-update-activate --staged-sha256-hex <sha256> <target ...> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed | agent-update-rollback [--rollback-sha256-hex <sha256>] <target ...> [--timeout <1-3600>] [--privilege-ttl <15-300>] [--force-unprivileged] --confirmed | agent-update-releases [--limit <1-200>] | agent-update-release-latest [--name <name>] [--channel stable] | agent-update-release-record --name <name> --version <version> --artifact-url <https-url> --sha256-hex <sha256> --artifact-signature-hex <sig> --artifact-signing-key-hex <key> [--rollback-artifact-file <path> --rollback-artifact-url <https-url> --rollback-signing-seed-hex <seed>] [--channel stable] [--size-bytes <n>] [--note <text>] --confirmed | agent-update-artifact-upload --name <name> --version <version> --artifact-file <path> --signing-seed-hex <seed> [--rollback-artifact-file <path>] [--rollback-signing-seed-hex <seed>] [--stream] [--channel stable] [--note <text>] --confirmed | process-list <target ...> [--limit <1-512>] [--confirmed] | process-start <name> --argv <abs> [--argv arg ...] <target ...> [--cwd <abs>] [--env KEY=VALUE] [--confirmed] | process-stop <name> <target ...> [--confirmed] | process-restart <name> <target ...> [--confirmed] | process-status [--name <name>] <target ...> [--confirmed] | process-logs <name> <target ...> [--max-bytes <n>] [--confirmed] | process-supervisor-inventory [--limit <1-200>] | job-targets <job_uuid> | job-target-status-download <job_uuid> <output_file> | job-outputs <job_uuid> | job-follow <job_uuid> [--interval-ms <100-10000>] [--max-polls <1-10000>] [--json] | job-output-download <job_uuid> <client_id> --seq <seq> <output_file> | backups | backup-policies | backup-artifacts | backup-policy-upsert <name> [--path <abs>] [--include-config] [--recipient-public-key-hex <hex>] [--cron <min> <hour> <dom> <mon> <dow>] [--retention-days <n>] [--keep-last <n>] [--rotation-generation <id>] [--disabled] <target ...> --confirmed | backup-policy-prune [--schedule-id <uuid>] [--dry-run] [--metadata-only true|false] [--confirmed] | backup-request <client_id> [--path <abs>] [--include-config] [--confirmed] | backup-run [--path <abs>] [--include-config] <target ...> [--timeout <1-3600>] --confirmed | backup-artifact-record <backup_uuid> --object-key <key> --sha256-hex <sha256> --size-bytes <n> --confirmed | backup-artifact-upload <backup_uuid> --object-key <key> --artifact-file <path> --confirmed | backup-artifact-upload-chunked <backup_uuid> --object-key <key> --artifact-file <path> [--chunk-size-bytes <1-4194304>] --confirmed | backup-artifact-handoff <backup_uuid> [--job-id <job_uuid>] --confirmed | restore-plans | restore-plan <backup_uuid> <target_client_id> [--path <abs>] [--include-config] [--destination-root <abs>] [--confirmed] | restore-run <backup_uuid> <target_client_id> [--artifact-file <path>] [--private-key-env <env>] [--path <abs>] [--include-config] [--destination-root <abs>] [--timeout <1-3600>] [--force-unprivileged] --confirmed | restore-rollback <restore_job_uuid> <target_client_id> [--timeout <1-3600>] [--force-unprivileged] --confirmed | migration-links | migration-link <restore_plan_uuid> [--note <text>] --confirmed | migration-run <restore_plan_uuid> [--artifact-file <path>] [--private-key-env <env>] [--note <text>] [--timeout <1-3600>] [--force-unprivileged] --confirmed | tunnel-plans | tunnel-plan --name <name> --interface-name <ifname> --kind <gre|ipip|sit|fou|openvpn|wireguard|tun_tap|custom> --left-client-id <id> --right-client-id <id> --left-underlay <ip> --right-underlay <ip> (--left-tunnel-ipv4 <ip> --right-tunnel-ipv4 <ip> and/or --left-tunnel-ipv6 <ip> --right-tunnel-ipv6 <ip>) [--address-pool-cidr <cidr>] [--ipv6-address-pool-cidr <cidr>] [--latency-primary-family <ipv4|ipv6>] --bandwidth <10m|100m|1000m> --latency-ms <ms> [--runtime-manager <agent|observed|adapter>] [--runtime-cleanup-argv <abs,arg>] [--fou-port <1-65535>] [--fou-peer-port <1-65535>] [--fou-ipproto <1-255>] [--save] | tunnel-allocate [--ipv4-pool-cidr <cidr>] [--ipv6-pool-cidr <cidr>] [--reserved-address <ip>] [--include-ipv4=true|false|--no-ipv4] [--include-ipv6|--include-ipv6=true|false] | tunnel-promote-telemetry --client-id <id> --interface <ifname> --peer-client-id <id> --local-underlay <ip> --peer-underlay <ip> (--left-tunnel-ipv4 <ip> --right-tunnel-ipv4 <ip> and/or --left-tunnel-ipv6 <ip> --right-tunnel-ipv6 <ip>) [--address-pool-cidr <cidr>] [--ipv6-address-pool-cidr <cidr>] [--latency-primary-family <ipv4|ipv6>] [--side <left|right>] [--bandwidth <10m|100m|1000m>] | tunnel-promote-adapter --plan-id <uuid> --runtime-status-argv <abs,arg> [--runtime-startup-argv <abs,arg>] [--runtime-stop-argv <abs,arg>] [--runtime-cleanup-argv <abs,arg>] [--fou-port <1-65535>] [--fou-peer-port <1-65535>] [--fou-ipproto <1-255>] --confirmed | tunnel-apply --plan-file <plan.json> --side <left|right> [--backend <ifupdown|netplan|systemd-networkd>] [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed | tunnel-ospf-cost-update --plan-file <plan.json> --side <left|right> --current-ospf-cost <1-65535> --recommended-ospf-cost <1-65535> [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed | tunnel-rollback --plan-file <plan.json> --side <left|right> [--timeout <1-3600>] [--privilege-ttl <15-300>] --confirmed | tunnel-status --plan-file <plan.json> --side <left|right> [--timeout <1-3600>] [--privilege-ttl <15-300>] | tunnel-probe --plan-file <plan.json> --side <left|right> [--count <1-20>] [--interval-ms <200-10000>] [--timeout <1-3600>] [--privilege-ttl <15-300>] | tunnel-speed-test --plan-file <plan.json> --server-side <left|right> [--duration-secs <1-30>] [--max-bytes <16384-268435456>] [--rate-limit-kbps <64-1000000>] [--port <1024-65535>] [--connect-timeout-ms <100-30000>] [--timeout <1-3600>] [--privilege-ttl <15-300>] | network-observations [--limit <1-200>] | network-trends [--limit <1-200>] | network-ospf-recommendations [--limit <1-200>] | network-ospf-update-plans [--limit <1-200>] | topology-graph [--limit <1-200>] | audit | history-retention | history-retention-upsert --domain <domain> [--retention-days <1-3650>] [--prune-limit <1-100000>] [--metadata-only true|false] --confirmed | history-retention-prune [--domain <domain>] [--dry-run] [--metadata-only true|false] [--confirmed] | history-export [--domains audit_logs,job_outputs] [--limit <1-200>] | bulk-resolve [tag ...] | enable | exit"
                );
                println!(
                    "server-jobs [--limit <1-200>] | artifact-cleanup-preview --expression <expr> | artifact-cleanup-create --expression <expr> --preview-hash <sha256> --confirmed | server-job-cancel --job-id <uuid>"
                )
            }
            other => println!("unknown command: {other}"),
        }
    }

    Ok(())
}
