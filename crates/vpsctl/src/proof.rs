use std::collections::HashMap;

use anyhow::{Context, Result};
use uuid::Uuid;
use vpsman_common::{
    derive_super_key, encode_json, payload_hash, random_nonce, sign_privilege_proof,
    CommandEnvelope, JobCommand,
};

use crate::unix_now;

pub(crate) fn build_command_envelopes_for_clients(
    client_ids: &[String],
    argv: &[String],
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
) -> Result<(String, HashMap<String, CommandEnvelope>)> {
    let command = JobCommand::Shell {
        argv: argv.to_vec(),
        pty: false,
    };
    build_envelopes_for_job_command(client_ids, &command, password, salt_hex, ttl_secs)
}

pub(crate) fn build_envelopes_for_job_command(
    client_ids: &[String],
    command: &JobCommand,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
) -> Result<(String, HashMap<String, CommandEnvelope>)> {
    let payload_hash_hex = payload_hash(&encode_json(&command)?);
    let envelopes = build_envelopes_for_payload_hash(
        client_ids,
        &payload_hash_hex,
        password,
        salt_hex,
        ttl_secs,
    )?;
    Ok((payload_hash_hex, envelopes))
}

pub(crate) fn build_envelopes_for_payload_hash(
    client_ids: &[String],
    payload_hash_hex: &str,
    password: &str,
    salt_hex: &str,
    ttl_secs: u64,
) -> Result<HashMap<String, CommandEnvelope>> {
    let salt = decode_super_salt(salt_hex)?;
    let super_key = derive_super_key(password, &salt);
    let expires_unix = unix_now().saturating_add(ttl_secs.max(1));
    let mut envelopes = HashMap::new();
    for client_id in client_ids {
        let command_id = Uuid::new_v4();
        let scope = format!("client:{client_id}");
        let nonce = random_nonce();
        let proof = sign_privilege_proof(
            &super_key,
            command_id,
            &scope,
            payload_hash_hex,
            &nonce,
            expires_unix,
        );
        envelopes.insert(
            client_id.clone(),
            CommandEnvelope {
                command_id,
                scope,
                payload_hash_hex: payload_hash_hex.to_string(),
                proof: Some(proof),
                server_signature: Vec::new(),
            },
        );
    }
    Ok(envelopes)
}

pub(crate) fn decode_super_salt(salt_hex: &str) -> Result<Vec<u8>> {
    let salt = hex::decode(salt_hex.trim()).context("super-password salt is not valid hex")?;
    anyhow::ensure!(
        !salt.is_empty(),
        "super-password salt decodes to empty salt"
    );
    Ok(salt)
}

pub(crate) fn load_super_password(password_env: &str) -> Result<String> {
    let password = std::env::var(password_env)
        .with_context(|| format!("environment variable {password_env} is not set"))?;
    anyhow::ensure!(
        !password.is_empty(),
        "environment variable {password_env} is empty"
    );
    Ok(password)
}

pub(crate) fn load_super_salt_hex(explicit_salt_hex: Option<&str>) -> Result<String> {
    let salt_hex = match explicit_salt_hex {
        Some(value) => value.to_string(),
        None => std::env::var("VPSMAN_SUPER_SALT_HEX")
            .context("set --super-salt-hex or VPSMAN_SUPER_SALT_HEX for local proof generation")?,
    };
    decode_super_salt(&salt_hex)?;
    Ok(salt_hex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use vpsman_common::{
        plan_tunnel, render_tunnel_endpoint_config, verify_privilege_proof, BandwidthTier,
        OspfCostPolicy, TunnelConfigBackend, TunnelEndpointSide, TunnelKind, TunnelPlanInput,
    };
    use vpsman_common::{FileExistingPolicy, FileOwnershipPolicy};

    const TEST_TRUE_ARGV: &str = "/bin/true";
    const TEST_TERMINAL_SHELL: &str = "/bin/sh";
    const TEST_FILE_PULL_PATH: &str = "/etc/hostname";
    const TEST_PROCESS_SLEEP_ARGV: &str = "/bin/sleep";
    const TEST_RESTORE_DESTINATION_PATH: &str = "/restore/etc/hostname";

    #[test]
    fn builds_per_client_proof_envelopes_without_server_signature() {
        let clients = vec!["client-a".to_string(), "client-b".to_string()];
        let argv = vec![TEST_TRUE_ARGV.to_string()];
        let (payload_hash_hex, envelopes) =
            build_command_envelopes_for_clients(&clients, &argv, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(
            &encode_json(&JobCommand::Shell {
                argv: argv.clone(),
                pty: false,
            })
            .unwrap(),
        );
        let super_key = derive_super_key("correct horse", &[1, 2, 3, 4]);

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(envelopes.len(), 2);
        for client_id in clients {
            let envelope = envelopes.get(&client_id).unwrap();
            let scope = format!("client:{client_id}");
            let proof = envelope.proof.as_ref().unwrap();

            assert_eq!(envelope.scope, scope);
            assert_eq!(envelope.payload_hash_hex, payload_hash_hex);
            assert!(envelope.server_signature.is_empty());
            assert!(verify_privilege_proof(
                &super_key,
                envelope.command_id,
                &envelope.scope,
                &payload_hash_hex,
                proof,
                unix_now()
            ));
        }
    }

    #[test]
    fn builds_shell_pty_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::Shell {
            argv: vec!["/usr/bin/tty".to_string()],
            pty: true,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes["client-a"].payload_hash_hex,
            expected_payload_hash
        );
        assert!(envelopes["client-a"].server_signature.is_empty());
    }

    #[test]
    fn builds_terminal_open_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::TerminalOpen {
            session_id: uuid::Uuid::parse_str("11111111-1111-4111-8111-111111111111").unwrap(),
            argv: vec![TEST_TERMINAL_SHELL.to_string(), "-l".to_string()],
            cwd: Some("/root".to_string()),
            cols: 120,
            rows: 40,
            replay_from_seq: Some(3),
            idle_timeout_secs: 1800,
            flow_window_bytes: 65_536,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes["client-a"].payload_hash_hex,
            expected_payload_hash
        );
        assert!(envelopes["client-a"].server_signature.is_empty());
    }

    #[test]
    fn builds_file_pull_proof_with_file_pull_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::FilePull {
            path: TEST_FILE_PULL_PATH.to_string(),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_shell_script_proof_with_shell_script_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::ShellScript {
            script: "echo vpsman".to_string(),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_user_sessions_proof_with_user_sessions_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::UserSessions;
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_file_push_proof_with_file_push_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let data = b"file contents";
        let command = JobCommand::FilePush {
            path: "/tmp/upload.txt".to_string(),
            mode: 0o640,
            size_bytes: data.len() as u64,
            sha256_hex: payload_hash(data),
            data_base64: vpsman_common::encode_inline_file_payload(data).unwrap(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_chunked_file_push_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let data = vec![13_u8; vpsman_common::MAX_INLINE_FILE_PUSH_BYTES + 9];
        let command = JobCommand::FilePushChunked {
            path: "/tmp/upload.bin".to_string(),
            mode: 0o600,
            size_bytes: data.len() as u64,
            sha256_hex: payload_hash(&data),
            chunks: vpsman_common::encode_chunked_file_payload(&data).unwrap(),
            existing_policy: FileExistingPolicy::Replace,
            owner: None,
            group: None,
            uid: None,
            gid: None,
            ownership_policy: FileOwnershipPolicy::Fail,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_process_list_proof_with_process_list_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::ProcessList { limit: 25 };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_process_supervisor_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::ProcessStart {
            name: "demo".to_string(),
            argv: vec![TEST_PROCESS_SLEEP_ARGV.to_string(), "60".to_string()],
            cwd: Some("/tmp".to_string()),
            env: BTreeMap::from([("VPSMAN_TEST".to_string(), "1".to_string())]),
            policy: vpsman_common::ProcessRunPolicy::default(),
            limits: vpsman_common::ProcessResourceLimits::default(),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_hot_config_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::HotConfig {
            toml: "client_id = \"client-a\"".to_string(),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_agent_update_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::UpdateAgent {
            artifact_url: "https://updates.example/vpsman-agent".to_string(),
            sha256_hex: "ab".repeat(32),
            artifact_signature_hex: None,
            artifact_signing_key_hex: None,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_agent_update_activation_and_rollback_proofs_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        for command in [
            JobCommand::AgentUpdateActivate {
                staged_sha256_hex: "ab".repeat(32),
                restart_agent: false,
            },
            JobCommand::AgentUpdateRollback {
                rollback_sha256_hex: Some("cd".repeat(32)),
            },
        ] {
            let (payload_hash_hex, envelopes) = build_envelopes_for_job_command(
                &clients,
                &command,
                "correct horse",
                "01020304",
                60,
            )
            .unwrap();
            let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());
            assert_eq!(payload_hash_hex, expected_payload_hash);
            assert_eq!(
                envelopes.get("client-a").unwrap().payload_hash_hex,
                expected_payload_hash
            );
        }
    }

    #[test]
    fn builds_auth_proof_rotation_proof_with_operation_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::AuthProofKeyRotate {
            new_proof_key_hex: "ef".repeat(32),
            rotation_generation: Some("2026-q2".to_string()),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_backup_proof_with_backup_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let command = JobCommand::Backup {
            paths: vec![TEST_FILE_PULL_PATH.to_string()],
            include_config: true,
            recipient_public_key_hex: None,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_restore_proof_with_restore_payload_hash() {
        let clients = vec!["client-b".to_string()];
        let command = JobCommand::Restore {
            source_backup_request_id: uuid::Uuid::new_v4(),
            paths: vec![TEST_FILE_PULL_PATH.to_string()],
            include_config: false,
            destination_root: Some("/restore".to_string()),
            archive_path: None,
            archive_base64: None,
            archive_size_bytes: None,
            archive_sha256_hex: None,
            dry_run: false,
            post_restore_argv: Vec::new(),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-b").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_restore_rollback_proof_with_operation_payload_hash() {
        let clients = vec!["client-b".to_string()];
        let command = JobCommand::RestoreRollback {
            source_restore_job_id: uuid::Uuid::new_v4(),
            restored_files: vec![vpsman_common::RestoreRollbackFile {
                archive_path: TEST_FILE_PULL_PATH.to_string(),
                destination_path: TEST_RESTORE_DESTINATION_PATH.to_string(),
                rollback_path: None,
                restored_size_bytes: 12,
                restored_sha256_hex:
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            }],
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-b").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_network_apply_proof_with_operation_payload_hash() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
        let clients = vec![endpoint.local_client_id.clone()];
        let command = JobCommand::NetworkApply {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
            config_backend: TunnelConfigBackend::Ifupdown,
            config_sha256_hex: None,
            ifupdown_sha256_hex: payload_hash(endpoint.ifupdown_snippet.as_bytes()),
            bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_network_ospf_cost_update_proof_with_operation_payload_hash() {
        let mut plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let current_ospf_cost = plan.recommended_ospf_cost;
        let recommended_ospf_cost = current_ospf_cost + 10;
        plan.recommended_ospf_cost = recommended_ospf_cost;
        let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
        let clients = vec![endpoint.local_client_id.clone()];
        let command = JobCommand::NetworkOspfCostUpdate {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
            current_ospf_cost,
            recommended_ospf_cost,
            bird2_sha256_hex: payload_hash(endpoint.bird2_interface_snippet.as_bytes()),
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_network_rollback_proof_with_operation_payload_hash() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
        let clients = vec![endpoint.local_client_id.clone()];
        let command = JobCommand::NetworkRollback {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_network_status_proof_with_operation_payload_hash() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
        let clients = vec![endpoint.local_client_id.clone()];
        let command = JobCommand::NetworkStatus {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_network_probe_proof_with_operation_payload_hash() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let endpoint = render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
        let clients = vec![endpoint.local_client_id.clone()];
        let command = JobCommand::NetworkProbe {
            plan: Box::new(plan),
            side: TunnelEndpointSide::Left,
            count: 3,
            interval_ms: 500,
        };
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_network_speed_test_proofs_for_both_endpoints() {
        let plan = plan_tunnel(&TunnelPlanInput {
            name: "edge-a-edge-b".to_string(),
            interface_name: "tunab".to_string(),
            kind: TunnelKind::Gre,
            runtime_control: Default::default(),
            runtime_topology: Default::default(),
            left_client_id: "client-a".to_string(),
            right_client_id: "client-b".to_string(),
            left_underlay: "198.51.100.10".to_string(),
            right_underlay: "203.0.113.20".to_string(),
            address_pool_cidr: "10.255.0.0/30".to_string(),
            reserved_addresses: Vec::new(),
            bandwidth: BandwidthTier::M100,
            latency_ms: 18.0,
            packet_loss_ratio: 0.0,
            preference: 1.0,
            ospf_policy: OspfCostPolicy::default(),
        })
        .unwrap();
        let command = JobCommand::NetworkSpeedTest {
            plan: Box::new(plan),
            server_side: TunnelEndpointSide::Left,
            duration_secs: 3,
            max_bytes: 16 * 1024 * 1024,
            rate_limit_kbps: 100_000,
            port: 5201,
            connect_timeout_ms: 5000,
        };
        let clients = vec!["client-a".to_string(), "client-b".to_string()];
        let (payload_hash_hex, envelopes) =
            build_envelopes_for_job_command(&clients, &command, "correct horse", "01020304", 60)
                .unwrap();
        let expected_payload_hash = payload_hash(&encode_json(&command).unwrap());

        assert_eq!(payload_hash_hex, expected_payload_hash);
        assert_eq!(envelopes.len(), 2);
        assert_eq!(
            envelopes.get("client-a").unwrap().payload_hash_hex,
            expected_payload_hash
        );
        assert_eq!(
            envelopes.get("client-b").unwrap().payload_hash_hex,
            expected_payload_hash
        );
    }

    #[test]
    fn builds_scheduled_dispatch_envelopes_from_existing_payload_hash() {
        let clients = vec!["client-a".to_string()];
        let payload_hash_hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let envelopes = build_envelopes_for_payload_hash(
            &clients,
            payload_hash_hex,
            "correct horse",
            "01020304",
            60,
        )
        .unwrap();
        let envelope = envelopes.get("client-a").unwrap();

        assert_eq!(envelope.scope, "client:client-a");
        assert_eq!(envelope.payload_hash_hex, payload_hash_hex);
        assert!(envelope.proof.is_some());
        assert!(envelope.server_signature.is_empty());
    }

    #[test]
    fn rejects_empty_or_invalid_super_salt() {
        assert!(decode_super_salt("").is_err());
        assert!(decode_super_salt("not-hex").is_err());
        assert!(decode_super_salt("00").is_ok());
    }
}
