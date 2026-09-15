use super::*;
use vpsman_common::{
    plan_tunnel, RuntimeTunnelControl, RuntimeTunnelWireguardEndpointMode,
    RuntimeTunnelWireguardOptions, TunnelAddressFamily, TunnelAddressPair, TunnelPlanInput,
};

const LOCAL_PRIVATE: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
const LOCAL_PUBLIC: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=";
const PEER_PUBLIC: &str = "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC=";
const OLD_PEER_PUBLIC: &str = "DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD=";

fn wireguard_plan(endpoint_mode: RuntimeTunnelWireguardEndpointMode) -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "wireguard-test".to_string(),
        interface_name: "wg-test".to_string(),
        kind: vpsman_common::TunnelKind::Wireguard,
        runtime_control: RuntimeTunnelControl {
            wireguard: RuntimeTunnelWireguardOptions {
                endpoint_mode,
                left_listen_port: 51820,
                right_listen_port: 51821,
                left_keepalive_secs: 25,
                right_keepalive_secs: 0,
            },
            ..RuntimeTunnelControl::default()
        },
        runtime_topology: Default::default(),
        left_client_id: "v-1".to_string(),
        right_client_id: "v-2".to_string(),
        left_remote_underlay: "2001:db8::2".to_string(),
        left_local_underlay: None,
        right_remote_underlay: "2001:db8::1".to_string(),
        right_local_underlay: None,
        address_pool_cidr: "10.0.0.0/31".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(TunnelAddressPair {
            left: "10.0.0.0".to_string(),
            right: "10.0.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: Some("fd00::/127".to_string()),
        ipv6_tunnel: Some(TunnelAddressPair {
            left: "fd00::".to_string(),
            right: "fd00::1".to_string(),
            prefix_len: 127,
        }),
        latency_primary_family: TunnelAddressFamily::Ipv4,
        additional_addresses: Default::default(),
        manage_link_local: true,
        bandwidth_mbps: 100,
        dynamic_bandwidth: false,
        left_mtu: Some(1420),
        right_mtu: Some(1420),
        ospf: None,
    })
    .unwrap()
}

fn credentials() -> TunnelEndpointBuiltinCredentials {
    TunnelEndpointBuiltinCredentials::Wireguard {
        generation: 1,
        local_private_key_base64: LOCAL_PRIVATE.to_string(),
        local_public_key_base64: LOCAL_PUBLIC.to_string(),
        peer_public_key_base64: PEER_PUBLIC.to_string(),
    }
}

fn prepared(previous_applied: Option<AppliedWireguardState>) -> PreparedWireguardState {
    PreparedWireguardState {
        plan_id: Uuid::nil(),
        private_key_path: PathBuf::from("/state/wireguard.key"),
        applied_state_path: PathBuf::from("/state/wireguard.applied.json"),
        pending_state_path: PathBuf::from("/state/wireguard.pending.json"),
        previous_applied,
        pending: None,
    }
}

#[tokio::test]
async fn preparation_does_not_write_wireguard_keys_before_start_hooks_accept() {
    let plan_id = Uuid::new_v4().to_string();
    let prepared = prepare_wireguard_state(
        Some(&plan_id),
        TunnelEndpointSide::Left,
        Some(&credentials()),
    )
    .await
    .unwrap();
    assert!(!prepared.private_key_path.exists());
    assert!(!prepared.private_key_path.parent().unwrap().exists());
}

fn prepared_with_pending(
    previous_applied: Option<AppliedWireguardState>,
    pending: Option<AppliedWireguardState>,
) -> PreparedWireguardState {
    PreparedWireguardState {
        pending,
        ..prepared(previous_applied)
    }
}

fn steps(
    mode: RuntimeTunnelWireguardEndpointMode,
    side: TunnelEndpointSide,
    existing_peers: &[String],
    previous_applied: Option<AppliedWireguardState>,
) -> Vec<RuntimeCommandSpec> {
    let plan = wireguard_plan(mode);
    let endpoint = vpsman_common::render_tunnel_endpoint_config(&plan, side).unwrap();
    build_wireguard_reconcile_steps(
        &AgentConfig::default(),
        &plan,
        &endpoint,
        Some(&credentials()),
        &prepared(previous_applied),
        true,
        existing_peers,
    )
    .unwrap()
}

#[test]
fn both_mode_sets_bracketed_ipv6_endpoint_and_both_allowed_families() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Both,
        TunnelEndpointSide::Left,
        &[],
        None,
    );
    let configure = steps
        .iter()
        .find(|step| step.label == "runtime_wireguard_configure")
        .unwrap();
    assert!(configure
        .argv
        .windows(2)
        .any(|pair| { pair == ["endpoint", "[2001:db8::2]:51821"] }));
    assert!(configure
        .argv
        .windows(2)
        .any(|pair| pair == ["allowed-ips", "0.0.0.0/0,::/0"]));
    let generated = vpsman_common::tunnel_generated_link_local(Uuid::nil(), "v-1");
    assert!(steps.iter().any(|step| step.label == "runtime_addr_replace"
        && step
            .argv
            .windows(2)
            .any(|pair| pair == ["replace", generated.as_str()])));
}

#[test]
fn one_sided_endpoint_mode_points_the_roaming_side_at_the_fixed_vps() {
    let left = steps(
        RuntimeTunnelWireguardEndpointMode::Right,
        TunnelEndpointSide::Left,
        &[],
        None,
    );
    let configure = left
        .iter()
        .find(|step| step.label == "runtime_wireguard_configure")
        .unwrap();
    assert!(configure.argv.iter().any(|value| value == "endpoint"));
    assert!(configure
        .argv
        .windows(2)
        .any(|pair| pair == ["persistent-keepalive", "25"]));

    let right = steps(
        RuntimeTunnelWireguardEndpointMode::Right,
        TunnelEndpointSide::Right,
        &[],
        None,
    );
    let configure = right
        .iter()
        .find(|step| step.label == "runtime_wireguard_configure")
        .unwrap();
    assert!(!configure.argv.iter().any(|value| value == "endpoint"));
}

#[test]
fn idempotent_reconcile_does_not_remove_the_desired_peer() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Both,
        TunnelEndpointSide::Left,
        &[PEER_PUBLIC.to_string()],
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
    );
    assert!(!steps
        .iter()
        .any(|step| step.label == "runtime_wireguard_peer_remove"));
}

#[test]
fn peer_rotation_configures_and_verifies_before_removing_the_previous_peer() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Both,
        TunnelEndpointSide::Left,
        &[OLD_PEER_PUBLIC.to_string()],
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: OLD_PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
    );
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    let configure = labels
        .iter()
        .position(|label| *label == "runtime_wireguard_configure")
        .unwrap();
    let verify = labels
        .iter()
        .position(|label| *label == "runtime_wireguard_public_key_verify")
        .unwrap();
    let remove = labels
        .iter()
        .position(|label| *label == "runtime_wireguard_peer_remove")
        .unwrap();
    assert!(configure < verify && verify < remove);
}

#[test]
fn changing_from_fixed_to_roaming_explicitly_resets_the_desired_peer() {
    let steps = steps(
        RuntimeTunnelWireguardEndpointMode::Left,
        TunnelEndpointSide::Left,
        &[PEER_PUBLIC.to_string()],
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
    );
    let labels = steps.iter().map(|step| step.label).collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_wireguard_roaming_peer_reset"));
    assert!(labels.contains(&"runtime_wireguard_configure_roaming"));
}

#[test]
fn interrupted_fixed_to_roaming_transition_still_clears_the_old_endpoint() {
    let plan = wireguard_plan(RuntimeTunnelWireguardEndpointMode::Left);
    let endpoint =
        vpsman_common::render_tunnel_endpoint_config(&plan, TunnelEndpointSide::Left).unwrap();
    let prepared = prepared_with_pending(
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: true,
        }),
        Some(AppliedWireguardState {
            local_public_key_base64: LOCAL_PUBLIC.to_string(),
            peer_public_key_base64: PEER_PUBLIC.to_string(),
            peer_endpoint_configured: false,
        }),
    );
    let steps = build_wireguard_reconcile_steps(
        &AgentConfig::default(),
        &plan,
        &endpoint,
        Some(&credentials()),
        &prepared,
        true,
        &[PEER_PUBLIC.to_string()],
    )
    .unwrap();
    assert!(steps
        .iter()
        .any(|step| step.label == "runtime_wireguard_roaming_peer_reset"));
}

#[cfg(target_os = "linux")]
mod linux_integration {
    use super::super::super::{
        execute_runtime_tunnel_reconcile_report, execute_runtime_tunnel_remove_report_cancelable,
        NetworkRuntimeReconcileInput, NetworkRuntimeRemoveInput,
    };
    use super::*;
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    async fn native_tool(program: &str, args: &[&str], input: Option<&str>) -> String {
        let mut child = tokio::process::Command::new(program)
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .await
                .unwrap();
        }
        let output = child.wait_with_output().await.unwrap();
        assert!(
            output.status.success(),
            "{program} {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    struct RealWireguard {
        config: AgentConfig,
        plan: TunnelPlan,
        plan_id: String,
        credentials: TunnelEndpointBuiltinCredentials,
    }

    impl RealWireguard {
        async fn new() -> Self {
            // The caller supplies the isolated namespace. Never opt in automatically
            // merely because this process has privileges or Docker is installed.
            assert_eq!(
                std::env::var("VPSMAN_TEST_ISOLATED_NETWORK").as_deref(),
                Ok("1"),
                "run only inside an isolated Docker network namespace with NET_ADMIN and IPv6 enabled"
            );
            let mut config = AgentConfig {
                client_id: "v-1".to_string(),
                ..AgentConfig::default()
            };
            config.network.apply_enabled = true;
            config.network.runtime_reconcile_enabled = true;
            config.network.root_dir = "/".to_string();
            let mut plan = wireguard_plan(RuntimeTunnelWireguardEndpointMode::Both);
            let id = Uuid::new_v4();
            // Linux IFNAMSIZ permits 15 visible characters; UUID-derived names also
            // keep parallel manual runs from sharing an interface.
            plan.interface_name = format!("wgr{}", &id.simple().to_string()[..12]);
            assert!(!std::path::Path::new("/sys/class/net")
                .join(&plan.interface_name)
                .exists());
            let wg = &config.network.runtime_wg_argv[0];
            let private = native_tool(wg, &["genkey"], None).await;
            let public = native_tool(wg, &["pubkey"], Some(&private)).await;
            let peer_private = native_tool(wg, &["genkey"], None).await;
            let peer_public = native_tool(wg, &["pubkey"], Some(&peer_private)).await;
            Self {
                config,
                plan,
                plan_id: id.to_string(),
                credentials: TunnelEndpointBuiltinCredentials::Wireguard {
                    generation: 1,
                    local_private_key_base64: private.trim().to_string(),
                    local_public_key_base64: public.trim().to_string(),
                    peer_public_key_base64: peer_public.trim().to_string(),
                },
            }
        }

        async fn reconcile(&self, config: &AgentConfig, plan: &TunnelPlan) -> serde_json::Value {
            execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
                config,
                plan_id: Some(&self.plan_id),
                plan,
                previous_plan: None,
                builtin_credentials: Some(&self.credentials),
                runtime_adapter: None,
                side: TunnelEndpointSide::Left,
                // Several native commands run serially; allow their normal timeout
                // budget without changing the production per-command limits.
                max_timeout_secs: 60,
                effective_uid_override: None,
            })
            .await
            .unwrap()
        }

        async fn disable(&self) {
            let report = execute_runtime_tunnel_remove_report_cancelable(
                NetworkRuntimeRemoveInput {
                    config: &self.config,
                    plan_id: Some(&self.plan_id),
                    plan: &self.plan,
                    builtin_credentials: Some(&self.credentials),
                    runtime_adapter: None,
                    side: TunnelEndpointSide::Left,
                    max_timeout_secs: 60,
                    effective_uid_override: None,
                },
                CommandCancelToken::default(),
            )
            .await
            .unwrap();
            assert_eq!(report["status"], "removed", "{report:#}");
            assert!(!self.link_exists());
        }

        fn link_exists(&self) -> bool {
            std::path::Path::new("/sys/class/net")
                .join(&self.plan.interface_name)
                .exists()
        }

        async fn identity(&self) -> (u64, String) {
            let link = native_tool(
                &self.config.network.runtime_ip_argv[0],
                &["-j", "link", "show", "dev", &self.plan.interface_name],
                None,
            )
            .await;
            let link: serde_json::Value = serde_json::from_str(&link).unwrap();
            let key = native_tool(
                &self.config.network.runtime_wg_argv[0],
                &["show", &self.plan.interface_name, "public-key"],
                None,
            )
            .await;
            (link[0]["ifindex"].as_u64().unwrap(), key.trim().to_string())
        }
    }

    impl Drop for RealWireguard {
        fn drop(&mut self) {
            // Only this randomly named, initially absent test interface is eligible
            // for fixture teardown, including after an assertion panic.
            if self.link_exists() {
                let _ = std::process::Command::new(&self.config.network.runtime_ip_argv[0])
                    .args(["link", "delete", "dev", &self.plan.interface_name])
                    .output();
            }
        }
    }

    fn command<'a>(report: &'a serde_json::Value, label: &str) -> &'a serde_json::Value {
        report["commands"]
            .as_array()
            .unwrap()
            .iter()
            .find(|command| command["label"] == label)
            .unwrap_or_else(|| panic!("missing {label}: {report:#}"))
    }

    #[tokio::test]
    #[ignore = "requires isolated Docker network namespace and NET_ADMIN"]
    async fn real_wireguard_reconcile_compensates_only_links_created_by_the_attempt() {
        let fixture = RealWireguard::new().await;
        let config = &fixture.config;
        let plan = &fixture.plan;
        let mut missing_wg = config.clone();
        let prepared = prepare_wireguard_state(
            Some(&fixture.plan_id),
            TunnelEndpointSide::Left,
            Some(&fixture.credentials),
        )
        .await
        .unwrap();
        let missing_program = prepared
            .private_key_path
            .with_file_name("missing-wg-executable");
        assert!(!missing_program.exists());
        missing_wg.network.runtime_wg_argv = vec![missing_program.to_string_lossy().into_owned()];

        // The actual Linux link-add succeeds before Command::spawn fails for wg.
        let failed = fixture.reconcile(&missing_wg, plan).await;
        assert_eq!(failed["status"], "failed", "{failed:#}");
        assert_eq!(failed["link_existed_before"], false);
        assert_eq!(
            command(&failed, "runtime_wireguard_link_add")["success"],
            true
        );
        let configure = command(&failed, "runtime_wireguard_configure");
        assert_eq!(configure["success"], false);
        assert!(configure["error"]
            .as_str()
            .is_some_and(|error| !error.is_empty()));
        assert_eq!(failed["compensation"]["status"], "completed", "{failed:#}");
        assert_eq!(
            failed["compensation"]["triggered_by"],
            "runtime_wireguard_configure"
        );
        assert!(
            !fixture.link_exists(),
            "successful compensation must remove the real Linux link"
        );

        // Retry uses the same plan ID, persisted private key and real binaries.
        let retry = fixture.reconcile(config, plan).await;
        assert_eq!(retry["status"], "converged", "{retry:#}");
        assert_eq!(retry["link_existed_before"], false);
        assert!(retry["commands"]
            .as_array()
            .unwrap()
            .iter()
            .any(|command| command["argv"][0] == config.network.runtime_tc_argv[0]));
        let identity = fixture.identity().await;
        let TunnelEndpointBuiltinCredentials::Wireguard {
            local_public_key_base64,
            ..
        } = &fixture.credentials
        else {
            unreachable!()
        };
        assert_eq!(&identity.1, local_public_key_base64);

        // Inspection is real wg; only the subsequent native configuration is made
        // invalid. A correctly owned, preexisting interface must not be deleted.
        let mut invalid_configure = config.clone();
        invalid_configure.network.runtime_wg_argv = vec!["/bin/sh".into(), "-c".into(),
            "if [ \"$1\" = set ]; then exec \"$0\" set \"$2\" invalid-option; fi; exec \"$0\" \"$@\"".into(),
            config.network.runtime_wg_argv[0].clone()];
        let failed = fixture.reconcile(&invalid_configure, plan).await;
        assert_eq!(failed["status"], "failed", "{failed:#}");
        assert_eq!(failed["link_existed_before"], true);
        assert_eq!(failed["existing_link_validation"]["status"], "matched");
        assert_eq!(
            command(&failed, "runtime_wireguard_configure")["success"],
            false
        );
        assert_eq!(
            failed["compensation"]["reason"],
            "no_plan_owned_link_created"
        );
        assert_eq!(fixture.identity().await, identity);
        let retry = fixture.reconcile(config, plan).await;
        assert_eq!(retry["status"], "converged", "{retry:#}");
        assert_eq!(fixture.identity().await, identity);

        // Explicit Disable is intentionally destructive; Enable may create a new
        // interface afterwards. Failure compensation must not emulate Disable.
        fixture.disable().await;
        let enabled = fixture.reconcile(config, plan).await;
        assert_eq!(enabled["status"], "converged", "{enabled:#}");
        assert_eq!(fixture.identity().await.1, identity.1);
        fixture.disable().await;

        // A link appearing after the initial sysfs read makes the actual add fail
        // with EEXIST. No successful add report means no compensation ownership.
        let mut raced_plan = plan.clone();
        raced_plan.runtime_control.hooks.left.pre_start =
            Some(vpsman_common::RuntimeTunnelCommand {
                argv: vec![
                    config.network.runtime_ip_argv[0].clone(),
                    "link".into(),
                    "add".into(),
                    "dev".into(),
                    "{interface}".into(),
                    "type".into(),
                    "wireguard".into(),
                ],
                ..Default::default()
            });
        let failed = fixture.reconcile(config, &raced_plan).await;
        assert_eq!(failed["status"], "failed", "{failed:#}");
        assert_eq!(command(&failed, "runtime_hook_pre_start")["success"], true);
        assert_eq!(
            command(&failed, "runtime_wireguard_link_add")["success"],
            false
        );
        assert_eq!(
            failed["compensation"]["reason"],
            "no_plan_owned_link_created"
        );
        assert!(fixture.link_exists());
        native_tool(
            &config.network.runtime_ip_argv[0],
            &["link", "delete", "dev", &plan.interface_name],
            None,
        )
        .await;

        // Failed best-effort cleanup is reported alongside the original wg launch
        // failure, without turning the entire reconciliation into an opaque Err.
        let mut failed_cleanup = missing_wg;
        failed_cleanup.network.runtime_ip_argv = vec!["/bin/sh".into(), "-c".into(),
            "if [ \"$1 $2\" = 'link delete' ]; then exec \"$0.missing-cleanup\" \"$@\"; fi; exec \"$0\" \"$@\"".into(),
            config.network.runtime_ip_argv[0].clone()];
        let failed = fixture.reconcile(&failed_cleanup, plan).await;
        assert_eq!(failed["status"], "failed", "{failed:#}");
        assert!(command(&failed, "runtime_wireguard_configure")["error"].is_string());
        assert_eq!(failed["compensation"]["status"], "attempted");
        assert_eq!(failed["compensation"]["all_steps_successful"], false);
        assert_eq!(
            failed["compensation"]["triggered_by"],
            "runtime_wireguard_configure"
        );
        assert_eq!(failed["compensation"]["commands"][0]["success"], false);
        assert_eq!(failed["compensation"]["commands"][0]["exit_code"], 127);
        assert!(fixture.link_exists());
        native_tool(
            &config.network.runtime_ip_argv[0],
            &["link", "delete", "dev", &plan.interface_name],
            None,
        )
        .await;
        cleanup_wireguard_state(Some(&fixture.plan_id), TunnelEndpointSide::Left)
            .await
            .unwrap();
    }
}
