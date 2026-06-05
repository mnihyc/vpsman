use anyhow::Result;

use crate::{
    cli::Command, commands::CommandContext, commands_backups, commands_keys, commands_migrations,
    commands_network, vty::run_vty,
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
        Command::BackupPolicies => {
            commands_backups::backup_policies(api_url, token)?;
            Ok(None)
        }
        Command::BackupPolicyUpsert {
            name,
            paths,
            include_config,
            recipient_public_key_hex,
            clients,
            tags,
            interval_secs,
            start_at_unix,
            disabled,
            catch_up_policy,
            catch_up_limit,
            retry_delay_secs,
            max_failures,
            retention_days,
            keep_last,
            rotation_generation,
            confirmed,
        } => {
            commands_backups::backup_policy_upsert(
                api_url,
                token,
                name,
                paths,
                include_config,
                recipient_public_key_hex,
                clients,
                tags,
                interval_secs,
                start_at_unix,
                !disabled,
                catch_up_policy,
                catch_up_limit,
                retry_delay_secs,
                max_failures,
                retention_days,
                keep_last,
                rotation_generation,
                confirmed,
            )?;
            Ok(None)
        }
        Command::BackupPolicyPrune {
            schedule_id,
            dry_run,
            metadata_only,
            confirmed,
        } => {
            commands_backups::backup_policy_prune(
                api_url,
                token,
                schedule_id,
                dry_run,
                metadata_only,
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
            recipient_public_key_hex,
            note,
            password_env,
            super_salt_hex,
            proof_ttl_secs,
            confirmed,
        } => {
            commands_backups::backup_request(
                api_url,
                token,
                client_id,
                paths,
                include_config,
                recipient_public_key_hex,
                note,
                password_env,
                super_salt_hex,
                proof_ttl_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::BackupRun {
            paths,
            include_config,
            recipient_public_key_hex,
            clients,
            tags,
            password_env,
            super_salt_hex,
            proof_ttl_secs,
            timeout_secs,
            confirmed,
        } => {
            commands_backups::backup_run(
                api_url,
                token,
                paths,
                include_config,
                recipient_public_key_hex,
                clients,
                tags,
                password_env,
                super_salt_hex,
                proof_ttl_secs,
                timeout_secs,
                confirmed,
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
            paths,
            include_config,
            destination_root,
            note,
            password_env,
            super_salt_hex,
            proof_ttl_secs,
            confirmed,
        } => {
            commands_backups::restore_plan(
                api_url,
                token,
                source_backup_request_id,
                target_client_id,
                paths,
                include_config,
                destination_root,
                note,
                password_env,
                super_salt_hex,
                proof_ttl_secs,
                confirmed,
            )?;
            Ok(None)
        }
        Command::RestoreRun {
            source_backup_request_id,
            target_client_id,
            artifact_file,
            private_key_env,
            paths,
            include_config,
            destination_root,
            password_env,
            super_salt_hex,
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_backups::restore_run(
                api_url,
                token,
                source_backup_request_id,
                target_client_id,
                artifact_file,
                private_key_env,
                paths,
                include_config,
                destination_root,
                password_env,
                super_salt_hex,
                proof_ttl_secs,
                timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::RestoreRollback {
            restore_job_id,
            target_client_id,
            password_env,
            super_salt_hex,
            proof_ttl_secs,
            timeout_secs,
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
                proof_ttl_secs,
                timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::MigrationLink {
            restore_plan_id,
            note,
            confirmed,
        } => {
            commands_migrations::migration_link(api_url, token, restore_plan_id, note, confirmed)?;
            Ok(None)
        }
        Command::MigrationRun {
            restore_plan_id,
            artifact_file,
            private_key_env,
            note,
            password_env,
            super_salt_hex,
            proof_ttl_secs,
            timeout_secs,
            confirmed,
            force_unprivileged,
        } => {
            commands_migrations::migration_run(
                api_url,
                token,
                restore_plan_id,
                artifact_file,
                private_key_env,
                note,
                password_env,
                super_salt_hex,
                proof_ttl_secs,
                timeout_secs,
                confirmed,
                force_unprivileged,
            )?;
            Ok(None)
        }
        Command::TunnelPlans => {
            commands_network::tunnel_plans(api_url, token)?;
            Ok(None)
        }
        Command::TunnelPlan(request) => {
            commands_network::tunnel_plan(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelPromoteTelemetry(request) => {
            commands_network::tunnel_promote_telemetry(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelPromoteAdapter(request) => {
            commands_network::tunnel_promote_adapter(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelApply(request) => {
            commands_network::tunnel_apply(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelOspfCostUpdate(request) => {
            commands_network::tunnel_ospf_cost_update(api_url, token, request)?;
            Ok(None)
        }
        Command::TunnelRollback(request) => {
            commands_network::tunnel_rollback(api_url, token, request)?;
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
        Command::SigningKeygen => {
            commands_keys::signing_keygen()?;
            Ok(None)
        }
        Command::Proof {
            scope,
            salt_hex,
            payload_hash_hex,
            password_env,
            command_id,
            ttl_secs,
        } => {
            commands_keys::print_proof(
                &scope,
                &salt_hex,
                &payload_hash_hex,
                &password_env,
                command_id.as_deref(),
                ttl_secs,
            )?;
            Ok(None)
        }
        Command::Vty => {
            run_vty(api_url)?;
            Ok(None)
        }
        other => Ok(Some(other)),
    }
}
