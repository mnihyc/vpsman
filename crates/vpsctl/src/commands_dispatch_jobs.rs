use anyhow::Result;
use vpsman_common::FileExistingPolicy;

use crate::{
    cli::Command, commands::CommandContext, commands_config, commands_file_transfer,
    commands_file_transfer_download, commands_file_transfers, commands_files, commands_jobs,
    commands_process, commands_terminal, commands_terminal_sessions,
};

pub(crate) fn dispatch(ctx: &CommandContext, command: Command) -> Result<Option<Command>> {
    let api_url = &ctx.api_url;
    let token = ctx.token();
    match command {
        Command::Jobs { limit } => {
            commands_jobs::jobs(api_url, token, limit)?;
            Ok(None)
        }
        Command::JobCreate {
            command,
            argv,
            pty,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            privileged,
            destructive,
            confirmed,
            force_unprivileged,
        } => {
            commands_jobs::job_create(
                api_url,
                token,
                commands_jobs::JobCreateOptions {
                    command,
                    argv,
                    pty,
                    clients,
                    tags,
                    password_env,
                    super_salt_hex,
                    privilege_ttl_secs,
                    timeout_secs,
                    privileged,
                    destructive,
                    confirmed,
                    force_unprivileged,
                },
            )?;
            Ok(None)
        }
        Command::JobShell {
            script,
            script_file,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_jobs::job_shell(
                api_url,
                token,
                script,
                script_file,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::TerminalOpen(command) => {
            commands_terminal::terminal_open(api_url, token, command)?;
            Ok(None)
        }
        Command::TerminalInput(command) => {
            commands_terminal::terminal_input(api_url, token, command)?;
            Ok(None)
        }
        Command::TerminalPoll(command) => {
            commands_terminal::terminal_poll(api_url, token, command)?;
            Ok(None)
        }
        Command::TerminalResize(command) => {
            commands_terminal::terminal_resize(api_url, token, command)?;
            Ok(None)
        }
        Command::TerminalClose(command) => {
            commands_terminal::terminal_close(api_url, token, command)?;
            Ok(None)
        }
        Command::TerminalSessions {
            limit,
            client_id,
            session_id,
        } => {
            commands_terminal_sessions::terminal_sessions(
                api_url, token, limit, client_id, session_id,
            )?;
            Ok(None)
        }
        Command::TerminalReplay {
            client_id,
            session_id,
            from_seq,
            limit,
            max_bytes,
            output_file,
            metadata_only,
        } => {
            commands_terminal_sessions::terminal_replay(
                api_url,
                token,
                commands_terminal_sessions::TerminalReplayRequest {
                    client_id,
                    session_id,
                    from_seq,
                    limit,
                    max_bytes,
                    output_file,
                    metadata_only,
                },
            )?;
            Ok(None)
        }
        Command::TerminalFollow {
            client_id,
            session_id,
            from_seq,
            interval_ms,
            max_polls,
            json,
        } => {
            commands_terminal_sessions::terminal_follow(
                api_url,
                token,
                commands_terminal_sessions::TerminalFollowRequest {
                    client_id,
                    session_id,
                    from_seq,
                    interval_ms,
                    max_polls,
                    json,
                },
            )?;
            Ok(None)
        }
        Command::JobTargets { job_id } => {
            commands_jobs::job_targets(api_url, token, job_id)?;
            Ok(None)
        }
        Command::JobTargetStatusDownload {
            job_id,
            output_file,
        } => {
            commands_jobs::job_target_status_download(api_url, token, job_id, output_file)?;
            Ok(None)
        }
        Command::JobOutputs { job_id } => {
            commands_jobs::job_outputs(api_url, token, job_id)?;
            Ok(None)
        }
        Command::JobFollow {
            job_id,
            interval_ms,
            max_polls,
            json,
        } => {
            commands_jobs::job_follow(api_url, token, job_id, interval_ms, max_polls, json)?;
            Ok(None)
        }
        Command::JobOutputDownload {
            job_id,
            client_id,
            seq,
            output_file,
        } => {
            commands_jobs::job_output_download(
                api_url,
                token,
                job_id,
                client_id,
                seq,
                output_file,
            )?;
            Ok(None)
        }
        Command::ServerJobs { limit } => {
            commands_jobs::server_jobs(api_url, token, limit)?;
            Ok(None)
        }
        Command::ArtifactCleanupPreview { expression } => {
            commands_jobs::artifact_cleanup_preview(api_url, token, expression)?;
            Ok(None)
        }
        Command::ArtifactCleanupCreate {
            expression,
            preview_hash,
            confirmed,
        } => {
            commands_jobs::artifact_cleanup_create(
                api_url,
                token,
                expression,
                preview_hash,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ServerJobCancel { job_id, confirmed } => {
            commands_jobs::server_job_cancel(api_url, token, job_id, confirmed)?;
            Ok(None)
        }
        Command::Audit { limit } => {
            commands_jobs::audit(api_url, token, limit)?;
            Ok(None)
        }
        Command::HistoryRetention => {
            commands_jobs::history_retention(api_url, token)?;
            Ok(None)
        }
        Command::HistoryRetentionUpsert {
            domain,
            retention_days,
            prune_limit,
            enabled,
            metadata_only,
            export_enabled,
            notes,
            clear_notes,
            confirmed,
        } => {
            commands_jobs::history_retention_upsert(
                api_url,
                token,
                commands_jobs::HistoryRetentionUpsertOptions {
                    domain,
                    retention_days,
                    prune_limit,
                    enabled,
                    metadata_only,
                    export_enabled,
                    notes,
                    clear_notes,
                    confirmed,
                },
            )?;
            Ok(None)
        }
        Command::HistoryRetentionPrune {
            domain,
            dry_run,
            metadata_only,
            confirmed,
        } => {
            commands_jobs::history_retention_prune(
                api_url,
                token,
                commands_jobs::HistoryRetentionPruneOptions {
                    domain,
                    dry_run,
                    metadata_only,
                    confirmed,
                },
            )?;
            Ok(None)
        }
        Command::HistoryExport {
            domains,
            limit,
            client_id,
            job_id,
        } => {
            commands_jobs::history_export(api_url, token, domains, limit, client_id, job_id)?;
            Ok(None)
        }
        Command::NetworkObservations { limit } => {
            commands_jobs::network_observations(api_url, token, limit)?;
            Ok(None)
        }
        Command::NetworkTrends { limit } => {
            commands_jobs::network_trends(api_url, token, limit)?;
            Ok(None)
        }
        Command::NetworkOspfRecommendations { limit } => {
            commands_jobs::network_ospf_recommendations(api_url, token, limit)?;
            Ok(None)
        }
        Command::NetworkOspfUpdatePlans { limit } => {
            commands_jobs::network_ospf_update_plans(api_url, token, limit)?;
            Ok(None)
        }
        Command::TopologyGraph { limit } => {
            commands_jobs::topology_graph(api_url, token, limit)?;
            Ok(None)
        }
        Command::FilePull {
            path,
            follow_symlinks,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_files::file_pull(
                api_url,
                token,
                path,
                follow_symlinks,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::FilePush {
            source,
            path,
            mode,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_files::file_push(
                api_url,
                token,
                source,
                path,
                mode,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::FileTransferUpload {
            source,
            source_artifact_id,
            path,
            mode,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            session_id,
            resume_token,
            chunk_size_bytes,
            rate_limit_kbps,
            existing_policy,
            poll_interval_ms,
            max_polls,
            multi_target_policy,
        } => {
            let mode = commands_files::parse_file_mode(&mode)?;
            let existing_policy = parse_file_existing_policy(&existing_policy)?;
            let multi_target_policy =
                commands_file_transfer::FileTransferMultiTargetPolicy::parse(&multi_target_policy)?;
            let source = match (source, source_artifact_id) {
                (Some(path), None) => {
                    commands_file_transfer::FileTransferUploadSource::LocalFile(path)
                }
                (None, Some(artifact_id)) => {
                    commands_file_transfer::FileTransferUploadSource::SourceArtifact { artifact_id }
                }
                _ => anyhow::bail!(
                    "file-transfer-upload requires exactly one of --source or --source-artifact-id"
                ),
            };
            commands_file_transfer::file_transfer_upload(
                api_url,
                token,
                commands_file_transfer::FileTransferUploadOptions {
                    plan: commands_file_transfer::FileTransferUploadPlan {
                        source,
                        path,
                        mode,
                        clients,
                        tags,
                        privilege_ttl_secs,
                        timeout_secs,
                        confirmed,
                        session_id,
                        resume_token,
                        chunk_size_bytes,
                        rate_limit_kbps,
                        existing_policy,
                        poll_interval_ms,
                        max_polls,
                        multi_target_policy,
                    },
                    password_env,
                    super_salt_hex,
                },
            )?;
            Ok(None)
        }
        Command::FileTransferDownload {
            path,
            follow_symlinks,
            destination,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            session_id,
            resume_token,
            chunk_size_bytes,
            rate_limit_kbps,
            poll_interval_ms,
            max_polls,
            multi_target_policy,
        } => {
            let multi_target_policy =
                commands_file_transfer_download::FileTransferDownloadMultiTargetPolicy::parse(
                    &multi_target_policy,
                )?;
            commands_file_transfer_download::file_transfer_download(
                api_url,
                token,
                commands_file_transfer_download::FileTransferDownloadOptions {
                    plan: commands_file_transfer_download::FileTransferDownloadPlan {
                        destination,
                        path,
                        follow_symlinks,
                        clients,
                        tags,
                        privilege_ttl_secs,
                        timeout_secs,
                        confirmed,
                        session_id,
                        resume_token,
                        chunk_size_bytes,
                        rate_limit_kbps,
                        poll_interval_ms,
                        max_polls,
                        multi_target_policy,
                    },
                    password_env,
                    super_salt_hex,
                },
            )?;
            Ok(None)
        }
        Command::FileTransfers {
            limit,
            client_id,
            session_id,
        } => {
            commands_file_transfers::file_transfers(api_url, token, limit, client_id, session_id)?;
            Ok(None)
        }
        Command::FileTransferHandoff {
            client_id,
            session_id,
            output_file,
            confirmed,
        } => {
            commands_file_transfers::file_transfer_handoff(
                api_url,
                token,
                client_id,
                session_id,
                output_file,
                confirmed,
            )?;
            Ok(None)
        }
        Command::FileTransferSources { limit } => {
            commands_file_transfers::file_transfer_sources(api_url, token, limit)?;
            Ok(None)
        }
        Command::FileTransferSourceUpload {
            source,
            name,
            confirmed,
        } => {
            commands_file_transfers::file_transfer_source_upload(
                api_url, token, source, name, confirmed,
            )?;
            Ok(None)
        }
        Command::FileTransferSourceDownload {
            artifact_id,
            output_file,
        } => {
            commands_file_transfers::file_transfer_source_download(
                api_url,
                token,
                artifact_id,
                output_file,
            )?;
            Ok(None)
        }
        Command::HotConfig {
            config_file,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_config::hot_config(
                api_url,
                token,
                config_file,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::ConfigPatch {
            config_file,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_config::config_patch(
                api_url,
                token,
                config_file,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::AgentUpdate {
            artifact_url,
            sha256_hex,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_config::agent_update(
                api_url,
                token,
                commands_config::AgentUpdateOptions {
                    artifact_url,
                    sha256_hex,
                    clients,
                    tags,
                    password_env,
                    super_salt_hex,
                    privilege_ttl_secs,
                    timeout_secs,
                    confirmed,
                    force_unprivileged,
                },
            )?;
            Ok(None)
        }
        Command::AgentUpdateCheck {
            version_url,
            activate,
            restart_agent,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_config::agent_update_check(
                api_url,
                token,
                commands_config::AgentUpdateCheckOptions {
                    version_url,
                    activate,
                    restart_agent,
                    clients,
                    tags,
                    password_env,
                    super_salt_hex,
                    privilege_ttl_secs,
                    timeout_secs,
                    confirmed,
                    force_unprivileged,
                },
            )?;
            Ok(None)
        }
        Command::AgentUpdateActivate {
            staged_sha256_hex,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            restart_agent,
            confirmed,
            force_unprivileged,
        } => {
            commands_config::agent_update_activate(
                api_url,
                token,
                staged_sha256_hex,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                restart_agent,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::AgentUpdateRollback {
            rollback_sha256_hex,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_config::agent_update_rollback(
                api_url,
                token,
                rollback_sha256_hex,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::AgentUpdateReleaseRecord(command) => {
            commands_config::agent_update_release_record(
                api_url,
                token,
                commands_config::AgentUpdateReleaseRecordOptions {
                    name: command.name,
                    version: command.version,
                    channel: command.channel,
                    artifact_url: command.artifact_url,
                    sha256_hex: command.sha256_hex,
                    rollback_artifact_url: command.rollback_artifact_url,
                    rollback_sha256_hex: command.rollback_sha256_hex,
                    size_bytes: command.size_bytes,
                    rollback_size_bytes: command.rollback_size_bytes,
                    notes: command.notes,
                    confirmed: command.confirmed,
                },
            )?;
            Ok(None)
        }
        Command::AgentUpdateReleaseLatest(command) => {
            commands_config::agent_update_release_latest(
                api_url,
                token,
                command.name,
                command.channel,
            )?;
            Ok(None)
        }
        Command::AgentUpdateReleases { limit } => {
            commands_config::agent_update_releases(api_url, token, limit)?;
            Ok(None)
        }
        Command::UserSessions {
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_process::user_sessions(
                api_url,
                token,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ProcessList {
            limit,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_process::process_list(
                api_url,
                token,
                limit,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ProcessStart {
            name,
            argv,
            cwd,
            env,
            restart_policy,
            restart_max_retries,
            restart_backoff_secs,
            graceful_stop_secs,
            memory_max_bytes,
            pids_max,
            open_files_max,
            cpu_shares,
            no_new_privileges,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_process::process_start(
                api_url,
                token,
                commands_process::ProcessStartOptions {
                    name,
                    argv,
                    cwd,
                    env,
                    restart_policy,
                    restart_max_retries,
                    restart_backoff_secs,
                    graceful_stop_secs,
                    memory_max_bytes,
                    pids_max,
                    open_files_max,
                    cpu_shares,
                    no_new_privileges,
                    clients,
                    tags,
                    password_env,
                    super_salt_hex,
                    privilege_ttl_secs,
                    timeout_secs,
                    confirmed,
                    force_unprivileged,
                },
            )?;
            Ok(None)
        }
        Command::ProcessStop {
            name,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_process::process_stop(
                api_url,
                token,
                name,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ProcessRestart {
            name,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_process::process_restart(
                api_url,
                token,
                name,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ProcessStatus {
            name,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_process::process_status(
                api_url,
                token,
                name,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ProcessLogs {
            name,
            max_bytes,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_process::process_logs(
                api_url,
                token,
                name,
                max_bytes,
                clients,
                tags,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                timeout_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::ProcessSupervisorInventory { limit } => {
            commands_process::process_supervisor_inventory(api_url, token, limit)?;
            Ok(None)
        }
        other => Ok(Some(other)),
    }
}

fn parse_file_existing_policy(value: &str) -> Result<FileExistingPolicy> {
    match value.trim() {
        "skip" => Ok(FileExistingPolicy::Skip),
        "replace" => Ok(FileExistingPolicy::Replace),
        other => anyhow::bail!("file existing policy must be skip or replace, got {other}"),
    }
}
