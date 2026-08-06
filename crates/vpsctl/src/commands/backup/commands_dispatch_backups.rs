use anyhow::Result;

use crate::{
    cli::Command, commands::CommandContext, commands_backups, commands_keys, commands_migrations,
    commands_network, commands_port_forwarding, vty::run_vty,
};

pub(crate) fn dispatch(ctx: &CommandContext, command: Command) -> Result<Option<Command>> {
    let api_url = &ctx.api_url;
    let token = ctx.token();
    match command {
        Command::Backups { limit } => {
            commands_backups::backups(api_url, token, limit)?;
            Ok(None)
        }
        Command::BackupArtifacts { limit } => {
            commands_backups::backup_artifacts(api_url, token, limit)?;
            Ok(None)
        }
        Command::BackupPolicies { limit, offset } => {
            commands_backups::backup_policies(api_url, token, limit, offset)?;
            Ok(None)
        }
        Command::BackupPolicyUpsert {
            schedule_id,
            name,
            paths,
            include_config,
            follow_symlinks,
            skip_missing_paths,
            clients,
            tags,
            cron_expr,
            disabled,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            retention_days,
            keep_last,
            rotation_generation,
            clear_rotation_generation,
            confirmed,
        } => {
            commands_backups::backup_policy_upsert(
                api_url,
                token,
                commands_backups::BackupPolicyUpsertOptions {
                    schedule_id,
                    name,
                    paths,
                    include_config,
                    follow_symlinks,
                    skip_missing_paths,
                    clients,
                    tags,
                    cron_expr,
                    enabled: !disabled,
                    catch_up_policy,
                    catch_up_limit,
                    retry_delay_secs,
                    max_failures,
                    retention_days,
                    keep_last,
                    rotation_generation,
                    clear_rotation_generation,
                    confirmed,
                },
            )?;
            Ok(None)
        }
        Command::BackupPolicyPrune {
            schedule_id,
            dry_run,
            metadata_only,
            preview_hash,
            confirmed,
        } => {
            commands_backups::backup_policy_prune(
                api_url,
                token,
                schedule_id,
                dry_run,
                metadata_only,
                preview_hash,
                confirmed,
            )?;
            Ok(None)
        }
        Command::RestorePlans { limit } => {
            commands_backups::restore_plans(api_url, token, limit)?;
            Ok(None)
        }
        Command::MigrationLinks { limit } => {
            commands_migrations::migration_links(api_url, token, limit)?;
            Ok(None)
        }
        Command::BackupRequest {
            client_id,
            paths,
            include_config,
            follow_symlinks,
            skip_missing_paths,
            note,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            confirmed,
        } => {
            commands_backups::backup_request(
                api_url,
                token,
                client_id,
                paths,
                include_config,
                follow_symlinks,
                skip_missing_paths,
                note,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::BackupRun {
            paths,
            include_config,
            follow_symlinks,
            skip_missing_paths,
            clients,
            tags,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
        } => {
            commands_backups::backup_run(
                api_url,
                token,
                commands_backups::BackupRunOptions {
                    paths,
                    include_config,
                    follow_symlinks,
                    skip_missing_paths,
                    clients,
                    tags,
                    password_env,
                    super_salt_hex,
                    privilege_ttl_secs,
                    max_timeout_secs,
                    confirmed,
                },
            )?;
            Ok(None)
        }
        Command::BackupArtifactRecord {
            backup_request_id,
            object_key,
            sha256_hex,
            size_bytes,
            confirmed,
        } => {
            commands_backups::backup_artifact_record(
                api_url,
                token,
                backup_request_id,
                object_key,
                sha256_hex,
                size_bytes,
                confirmed,
            )?;
            Ok(None)
        }
        Command::BackupArtifactUpload {
            backup_request_id,
            object_key,
            artifact_file,
            confirmed,
        } => {
            commands_backups::backup_artifact_upload(
                api_url,
                token,
                backup_request_id,
                object_key,
                artifact_file,
                confirmed,
            )?;
            Ok(None)
        }
        Command::BackupArtifactUploadChunked {
            backup_request_id,
            object_key,
            artifact_file,
            chunk_size_bytes,
            confirmed,
        } => {
            commands_backups::backup_artifact_upload_chunked(
                api_url,
                token,
                backup_request_id,
                object_key,
                artifact_file,
                chunk_size_bytes,
                confirmed,
            )?;
            Ok(None)
        }
        Command::BackupArtifactHandoff {
            backup_request_id,
            job_id,
            confirmed,
        } => {
            commands_backups::backup_artifact_handoff(
                api_url,
                token,
                backup_request_id,
                job_id,
                confirmed,
            )?;
            Ok(None)
        }
        Command::RestorePlan {
            source_backup_request_id,
            target_client_id,
            note,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            confirmed,
        } => {
            commands_backups::restore_plan(
                api_url,
                token,
                source_backup_request_id,
                target_client_id,
                note,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::RestoreRun {
            source_backup_request_id,
            target_client_id,
            archive_transfer_session_id,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
            dry_run,
            force_unprivileged,
        } => {
            commands_backups::restore_run(
                api_url,
                token,
                commands_backups::RestoreRunOptions {
                    source_backup_request_id,
                    target_client_id,
                    archive_transfer_session_id,
                    password_env,
                    super_salt_hex,
                    privilege_ttl_secs,
                    max_timeout_secs,
                    confirmed,
                    dry_run,
                    force_unprivileged,
                },
            )?;
            Ok(None)
        }
        Command::RestoreRollback {
            restore_job_id,
            target_client_id,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_backups::restore_rollback(
                api_url,
                token,
                restore_job_id,
                target_client_id,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                max_timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::MigrationLink {
            restore_plan_id,
            note,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            confirmed,
        } => {
            commands_migrations::migration_link(
                api_url,
                token,
                restore_plan_id,
                note,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::MigrationRun {
            restore_plan_id,
            archive_transfer_session_id,
            note,
            password_env,
            super_salt_hex,
            privilege_ttl_secs,
            max_timeout_secs,
            confirmed,
            dry_run,
            force_unprivileged,
        } => {
            commands_migrations::migration_run(
                api_url,
                token,
                restore_plan_id,
                archive_transfer_session_id,
                note,
                password_env,
                super_salt_hex,
                privilege_ttl_secs,
                max_timeout_secs,
                confirmed,
                dry_run,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::TunnelPlans => {
            commands_network::tunnel_plans(api_url, token)?;
            Ok(None)
        }
        Command::PortForwards => {
            commands_port_forwarding::list(api_url, token)?;
            Ok(None)
        }
        Command::PortForwardCreate(request) => {
            commands_port_forwarding::create(api_url, token, request)?;
            Ok(None)
        }
        Command::PortForwardUpdate(request) => {
            commands_port_forwarding::update(api_url, token, request)?;
            Ok(None)
        }
        Command::PortForwardEnable(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "enable")?;
            Ok(None)
        }
        Command::PortForwardDisable(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "disable")?;
            Ok(None)
        }
        Command::PortForwardDelete(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "delete")?;
            Ok(None)
        }
        Command::PortForwardForget(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "forget")?;
            Ok(None)
        }
        Command::PortForwardReapply(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "reapply")?;
            Ok(None)
        }
        Command::PortForwardResolve(request) => {
            commands_port_forwarding::resolve(api_url, token, request)?;
            Ok(None)
        }
        Command::PortForwardBulk(request) => {
            commands_port_forwarding::bulk(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelAllocate(request) => {
            commands_network::tunnel_allocate(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelPlan(request) => {
            commands_network::tunnel_plan(api_url, token, *request)?;
            Ok(None)
        }
        Command::TunnelPlanExport(request) => {
            commands_network::tunnel_plan_export(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelPlanEnable(request) => {
            commands_network::set_tunnel_plan_enabled(api_url, token, request, true)?;
            Ok(None)
        }
        Command::TunnelPlanDisable(request) => {
            commands_network::set_tunnel_plan_enabled(api_url, token, request, false)?;
            Ok(None)
        }
        Command::TunnelPlanRotateCredentials(request) => {
            commands_network::rotate_tunnel_plan_credentials(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelPlanDelete(request) => {
            commands_network::delete_tunnel_plan(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelOspfStatusRefresh(request) => {
            commands_network::refresh_tunnel_ospf_status(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelOspfCostUpdate(request) => {
            commands_network::tunnel_ospf_cost_update(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelStatus(request) => {
            commands_network::tunnel_status(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelProbe(request) => {
            commands_network::tunnel_probe(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelSpeedTest(request) => {
            commands_network::tunnel_speed_test(api_url, token, request)?;
            Ok(None)
        }
        Command::NoiseKeygen => {
            commands_keys::noise_keygen()?;
            Ok(None)
        }
        Command::Vty => {
            run_vty(api_url)?;
            Ok(None)
        }
        other => Ok(Some(other)),
    }
}
