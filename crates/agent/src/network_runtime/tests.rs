use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use vpsman_common::{
    plan_tunnel, AgentConfig, AgentNetworkConfig, AgentRuntimeUnprivilegedMutationPolicy,
    BandwidthTier, OspfCostPolicy, RuntimeTunnelCommand, RuntimeTunnelControl,
    RuntimeTunnelFouOptions, RuntimeTunnelManager, RuntimeTunnelRoute, RuntimeTunnelTrafficLimit,
    TunnelEndpointSide, TunnelKind, TunnelPlan, TunnelPlanInput,
};

use super::*;

#[tokio::test]
async fn iproute2_reconciler_builds_create_addr_up_and_tc_steps() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("create", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let mut plan = test_plan();
    plan.runtime_control.traffic_limit = RuntimeTunnelTrafficLimit {
        ingress_kbps: Some(50_000),
        egress_kbps: Some(100_000),
        burst_kb: Some(64),
    };
    let config = test_config(&root);

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "converged");
    assert_eq!(report["link_existed_before"], false);
    let labels = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_tunnel_add"));
    assert!(labels.contains(&"runtime_addr_replace"));
    assert!(labels.contains(&"runtime_link_up"));
    assert!(labels.contains(&"runtime_traffic_egress_limit"));
    assert!(labels.contains(&"runtime_traffic_ingress_filter"));
    assert!(report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .all(|command| command["success"].as_bool() == Some(true)));
}

#[tokio::test]
async fn iproute2_reconciler_keeps_matching_existing_link_instead_of_changing_identity() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("existing", job_id);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    let plan = test_plan();
    let mut config = test_config(&root);
    configure_inspected_existing_link(&mut config, &root, &plan, TunnelEndpointSide::Left, None)
        .await;

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["link_existed_before"], true);
    assert_eq!(report["existing_link_validation"]["status"], "matched");
    let labels = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_tunnel_inspect"));
    assert!(labels.contains(&"runtime_addr_replace"));
    assert!(labels.contains(&"runtime_link_up"));
    assert!(!labels.contains(&"runtime_tunnel_change"));
    assert!(!labels.contains(&"runtime_tunnel_add"));
}

#[tokio::test]
async fn iproute2_reconciler_rejects_conflicting_existing_link() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("existing-conflict", job_id);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    let plan = test_plan();
    let mut config = test_config(&root);
    configure_inspected_existing_link(
        &mut config,
        &root,
        &plan,
        TunnelEndpointSide::Left,
        Some(("remote", "203.0.113.99")),
    )
    .await;

    let error = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("existing runtime tunnel tunlr does not match saved plan"));
    assert!(message.contains("remote_underlay expected 203.0.113.20 got 203.0.113.99"));
}

#[tokio::test]
async fn iproute2_reconciler_rejects_conflicting_existing_tunnel_address() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("existing-address-conflict", job_id);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    let plan = test_plan();
    let mut config = test_config(&root);
    configure_inspected_existing_link(
        &mut config,
        &root,
        &plan,
        TunnelEndpointSide::Left,
        Some(("address_peer", "10.255.0.2")),
    )
    .await;

    let error = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("existing runtime tunnel tunlr does not match saved plan"));
    assert!(message.contains("tunnel_address expected 10.255.0.0/31 peer 10.255.0.1"));
}

#[tokio::test]
async fn unprivileged_agent_reports_degraded_without_mutating() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("unprivileged", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let plan = test_plan();
    let config = test_config(&root);

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(1000),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "degraded_unprivileged");
    let commands = report["commands"].as_array().unwrap();
    assert_eq!(commands[0]["label"], "runtime_link_show");
    assert_eq!(commands[0]["skipped"], false);
    assert!(commands
        .iter()
        .skip(1)
        .all(|command| command["skipped"].as_bool() == Some(true)
            && command["reason"] == "agent_unprivileged"));
}

#[tokio::test]
async fn iproute2_reconciler_uses_custom_fou_runtime_options() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("custom-fou", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let plan = test_fou_plan();
    let config = test_config(&root);

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    let commands = report["commands"].as_array().unwrap();
    let fou_add = command_argv(commands, "runtime_fou_add");
    assert!(fou_add.contains(&"6655"));
    assert!(fou_add.contains(&"47"));
    let tunnel_add = command_argv(commands, "runtime_tunnel_add");
    assert!(tunnel_add.contains(&"encap-dport"));
    assert!(tunnel_add.contains(&"7755"));

    let report = execute_runtime_tunnel_remove_report(NetworkRuntimeRemoveInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();
    let commands = report["commands"].as_array().unwrap();
    let fou_delete = command_argv(commands, "runtime_fou_delete");
    assert!(fou_delete.contains(&"6655"));
}

#[tokio::test]
async fn external_adapter_expands_placeholders_and_runs_status() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("adapter", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let mut plan = test_plan();
    let adapter = root.join("adapter.sh");
    write_executable(&adapter, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").await;
    plan.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "start".to_string(),
                "{interface}".to_string(),
                "{local_address}".to_string(),
                "{remote_address}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        status: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "status".to_string(),
                "{plan}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    let config = test_config(&root);

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "converged");
    let commands = report["commands"].as_array().unwrap();
    assert_eq!(commands[0]["label"], "runtime_adapter_startup");
    assert!(commands[0]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("tunlr"));
    assert_eq!(commands[1]["label"], "runtime_adapter_status");
    assert!(commands[1]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("left-right"));
}

#[tokio::test]
async fn external_adapter_can_run_unprivileged_when_policy_allows_it() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("adapter-unprivileged-try", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let marker = root.join("adapter.log");
    let adapter = root.join("adapter.sh");
    write_executable(
        &adapter,
        "#!/bin/sh\nlog_file=\"$1\"\nshift\nprintf '%s\\n' \"$*\" >>\"$log_file\"\n",
    )
    .await;
    let mut plan = test_plan();
    plan.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                marker.to_string_lossy().to_string(),
                "start".to_string(),
                "{interface}".to_string(),
            ],
            ..RuntimeTunnelCommand::default()
        }),
        status: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                marker.to_string_lossy().to_string(),
                "status".to_string(),
                "{plan}".to_string(),
            ],
            ..RuntimeTunnelCommand::default()
        }),
        ..RuntimeTunnelControl::default()
    };
    let mut config = test_config(&root);
    config.network.runtime_unprivileged_mutation_policy =
        AgentRuntimeUnprivilegedMutationPolicy::TryExternalAdapters;

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(1000),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "converged");
    assert_eq!(
        report["unprivileged_mutation_policy"],
        "try_external_adapters"
    );
    assert!(report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .all(|command| command["skipped"].as_bool() != Some(true)));
    let adapter_log = tokio::fs::read_to_string(&marker).await.unwrap();
    assert!(adapter_log.contains("start tunlr"));
    assert!(adapter_log.contains("status left-right"));
}

#[tokio::test]
async fn reconciler_removes_explicit_stale_links_and_replaces_routes() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("cleanup-routes", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let mut plan = test_plan();
    plan.runtime_topology.version = Some("desired-42".to_string());
    plan.runtime_topology.desired_interfaces = vec!["tunlr".to_string()];
    plan.runtime_topology.stale_interfaces = vec!["oldtun0".to_string()];
    plan.runtime_topology.stale_routes = vec![RuntimeTunnelRoute {
        destination_cidr: "10.44.0.0/24".to_string(),
        interface_name: Some("oldtun0".to_string()),
        ..RuntimeTunnelRoute::default()
    }];
    plan.runtime_topology.routes = vec![RuntimeTunnelRoute {
        destination_cidr: "10.55.0.0/24".to_string(),
        via: Some("10.255.0.1".to_string()),
        metric: Some(42),
        ..RuntimeTunnelRoute::default()
    }];
    let config = test_config(&root);

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "converged");
    assert_eq!(report["topology_version"], "desired-42");
    let commands = report["commands"].as_array().unwrap();
    let labels = commands
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_route_delete"));
    assert!(labels.contains(&"runtime_stale_link_delete"));
    assert!(labels.contains(&"runtime_route_replace"));
    let route = commands
        .iter()
        .find(|command| command["label"] == "runtime_route_replace")
        .unwrap();
    let argv = route["argv"].as_array().unwrap();
    assert!(argv.iter().any(|arg| arg == "10.55.0.0/24"));
    assert!(argv.iter().any(|arg| arg == "10.255.0.1"));
    assert!(argv.iter().any(|arg| arg == "42"));
}

#[tokio::test]
async fn reconciler_only_deletes_interfaces_explicitly_declared_stale() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("safe-stale-delete", job_id);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    tokio::fs::create_dir_all(root.join("sys/class/net/ovpn42"))
        .await
        .unwrap();
    let plan = test_plan();
    let mut config = test_config(&root);
    configure_inspected_existing_link(&mut config, &root, &plan, TunnelEndpointSide::Left, None)
        .await;

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    let command_args = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|command| command["argv"].as_array().unwrap().iter())
        .map(|arg| arg.as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(!command_args.contains(&"ovpn42"));

    let mut promoted = test_plan();
    promoted.runtime_topology.stale_interfaces = vec!["ovpn42".to_string()];
    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &promoted,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    let stale_delete = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["label"] == "runtime_stale_link_delete")
        .unwrap();
    let argv = stale_delete["argv"].as_array().unwrap();
    assert!(argv.iter().any(|arg| arg == "ovpn42"));
}

#[tokio::test]
async fn iproute2_reconciler_compensates_new_link_after_required_failure() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("compensate-new-link", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let ip = root.join("ip.sh");
    write_executable(
        &ip,
        "#!/bin/sh\nif [ \"$1\" = \"addr\" ]; then exit 42; fi\nexit 0\n",
    )
    .await;
    let plan = test_plan();
    let mut config = test_config(&root);
    config.network.runtime_ip_argv = vec![ip.to_string_lossy().to_string()];

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "failed");
    let labels = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_tunnel_add"));
    assert!(labels.contains(&"runtime_addr_replace"));
    assert!(!labels.contains(&"runtime_link_up"));
    assert_eq!(report["compensation"]["status"], "completed");
    assert_eq!(
        report["compensation"]["triggered_by"],
        "runtime_addr_replace"
    );
    let compensation_labels = report["compensation"]["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(compensation_labels, vec!["runtime_compensate_link_delete"]);
}

#[tokio::test]
async fn external_adapter_reconciler_compensates_with_stop_after_required_failure() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("adapter-compensate", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let adapter = root.join("adapter-compensate.sh");
    write_executable(
        &adapter,
        "#!/bin/sh\nif [ \"$1\" = \"start\" ]; then exit 7; fi\nprintf '%s\\n' \"$@\"\n",
    )
    .await;
    let mut plan = test_plan();
    plan.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        startup: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "start".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        stop: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "stop".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        cleanup: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "cleanup".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        status: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "status".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    let config = test_config(&root);

    let report = execute_runtime_tunnel_reconcile_report(NetworkRuntimeReconcileInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "failed");
    let labels = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(labels, vec!["runtime_adapter_startup"]);
    assert_eq!(report["compensation"]["status"], "completed");
    let compensation = report["compensation"]["commands"].as_array().unwrap();
    assert_eq!(compensation[0]["label"], "runtime_adapter_compensate_stop");
    assert!(compensation[0]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("stop"));
    assert_eq!(
        compensation[1]["label"],
        "runtime_adapter_compensate_cleanup"
    );
    assert!(compensation[1]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("cleanup"));
}

#[tokio::test]
async fn iproute2_remove_deletes_existing_link_and_declared_routes() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("remove", job_id);
    tokio::fs::create_dir_all(root.join("sys/class/net/tunlr"))
        .await
        .unwrap();
    let mut plan = test_plan();
    plan.runtime_topology.routes = vec![RuntimeTunnelRoute {
        destination_cidr: "10.55.0.0/24".to_string(),
        metric: Some(20),
        ..RuntimeTunnelRoute::default()
    }];
    let config = test_config(&root);

    let report = execute_runtime_tunnel_remove_report(NetworkRuntimeRemoveInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "removed");
    let labels = report["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["label"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(labels.contains(&"runtime_route_delete"));
    assert!(labels.contains(&"runtime_link_delete"));
}

#[tokio::test]
async fn external_adapter_remove_runs_stop_then_status() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("adapter-remove", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let adapter = root.join("adapter-stop.sh");
    write_executable(&adapter, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").await;
    let mut plan = test_plan();
    plan.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        stop: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "stop".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        cleanup: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "cleanup".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        status: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "status".to_string(),
                "{plan}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    let config = test_config(&root);

    let report = execute_runtime_tunnel_remove_report(NetworkRuntimeRemoveInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "removed");
    let commands = report["commands"].as_array().unwrap();
    assert_eq!(commands[0]["label"], "runtime_adapter_stop");
    assert!(commands[0]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("stop"));
    assert_eq!(commands[1]["label"], "runtime_adapter_cleanup");
    assert!(commands[1]["stdout"]["text"]
        .as_str()
        .unwrap()
        .contains("cleanup"));
    assert_eq!(commands[2]["label"], "runtime_adapter_status");
}

#[tokio::test]
async fn external_adapter_remove_reports_unavailable_without_remove_lifecycle() {
    let job_id = uuid::Uuid::new_v4();
    let root = test_root("adapter-remove-unavailable", job_id);
    tokio::fs::create_dir_all(&root).await.unwrap();
    let adapter = root.join("adapter-status.sh");
    write_executable(&adapter, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").await;
    let mut plan = test_plan();
    plan.runtime_control = RuntimeTunnelControl {
        manager: RuntimeTunnelManager::ExternalManagedAdapter,
        status: Some(RuntimeTunnelCommand {
            argv: vec![
                adapter.to_string_lossy().to_string(),
                "status".to_string(),
                "{interface}".to_string(),
            ],
            timeout_secs: 5,
            max_output_bytes: 4096,
        }),
        ..RuntimeTunnelControl::default()
    };
    let config = test_config(&root);

    let report = execute_runtime_tunnel_remove_report(NetworkRuntimeRemoveInput {
        config: &config,
        plan: &plan,
        side: TunnelEndpointSide::Left,
        timeout_secs: 5,
        effective_uid_override: Some(0),
    })
    .await
    .unwrap();

    assert_eq!(report["status"], "remove_unavailable");
    let commands = report["commands"].as_array().unwrap();
    assert_eq!(commands[0]["label"], "runtime_adapter_status");
}

fn test_plan() -> TunnelPlan {
    plan_tunnel(&TunnelPlanInput {
        name: "left-right".to_string(),
        interface_name: "tunlr".to_string(),
        kind: TunnelKind::Gre,
        runtime_control: Default::default(),
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.0.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.0.0".to_string(),
            right: "10.255.0.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 15.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}

fn test_fou_plan() -> TunnelPlan {
    let runtime_control = RuntimeTunnelControl {
        fou: RuntimeTunnelFouOptions {
            port: 6655,
            peer_port: 7755,
            ipproto: 47,
        },
        ..RuntimeTunnelControl::default()
    };
    plan_tunnel(&TunnelPlanInput {
        name: "left-right-fou".to_string(),
        interface_name: "foulr".to_string(),
        kind: TunnelKind::Fou,
        runtime_control,
        runtime_topology: Default::default(),
        left_client_id: "left-a".to_string(),
        right_client_id: "right-b".to_string(),
        left_underlay: "198.51.100.10".to_string(),
        right_underlay: "203.0.113.20".to_string(),
        address_pool_cidr: "10.255.1.0/30".to_string(),
        reserved_addresses: Vec::new(),
        ipv4_tunnel: Some(vpsman_common::TunnelAddressPair {
            left: "10.255.1.0".to_string(),
            right: "10.255.1.1".to_string(),
            prefix_len: 31,
        }),
        ipv6_address_pool_cidr: None,
        ipv6_tunnel: None,
        latency_primary_family: Default::default(),
        bandwidth: BandwidthTier::M100,
        latency_ms: 15.0,
        packet_loss_ratio: 0.0,
        preference: 1.0,
        ospf_policy: OspfCostPolicy::default(),
    })
    .unwrap()
}

fn command_argv<'a>(commands: &'a [serde_json::Value], label: &str) -> Vec<&'a str> {
    commands
        .iter()
        .find(|command| command["label"] == label)
        .unwrap_or_else(|| panic!("missing command {label}"))["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect()
}

async fn configure_inspected_existing_link(
    config: &mut AgentConfig,
    root: &Path,
    plan: &TunnelPlan,
    side: TunnelEndpointSide,
    override_field: Option<(&str, &str)>,
) {
    let endpoint = render_tunnel_endpoint_config(plan, side).unwrap();
    let mut local = local_underlay(plan, &endpoint).to_string();
    let mut remote = remote_underlay(plan, &endpoint).to_string();
    let mut mode = linux_tunnel_mode(plan.kind).unwrap().to_string();
    let mut ttl = "255".to_string();
    let mut encap = (plan.kind == TunnelKind::Fou).then(|| "fou".to_string());
    let mut encap_dport =
        (plan.kind == TunnelKind::Fou).then(|| plan.runtime_control.fou.peer_port.to_string());
    let mut address_local = local_address(plan, &endpoint).to_string();
    let mut address_peer = remote_address(plan, &endpoint).to_string();
    let mut address_prefix_len = plan.tunnel_prefix_len;
    if let Some((field, value)) = override_field {
        match field {
            "local" => local = value.to_string(),
            "remote" => remote = value.to_string(),
            "mode" => mode = value.to_string(),
            "ttl" => ttl = value.to_string(),
            "encap" => encap = Some(value.to_string()),
            "encap_dport" => encap_dport = Some(value.to_string()),
            "address_local" => address_local = value.to_string(),
            "address_peer" => address_peer = value.to_string(),
            "address_prefix_len" => address_prefix_len = value.parse().unwrap(),
            other => panic!("unsupported inspect override {other}"),
        }
    }
    let mut info_data = serde_json::json!({
        "local": local,
        "remote": remote,
        "ttl": ttl,
    });
    if let Some(encap) = encap {
        info_data["encap"] = serde_json::Value::String(encap);
    }
    if let Some(encap_dport) = encap_dport {
        info_data["encap_dport"] = serde_json::Value::String(encap_dport);
    }
    let inspect_json = serde_json::json!([{
        "ifname": plan.interface_name,
        "linkinfo": {
            "info_kind": mode,
            "info_data": info_data,
        },
    }])
    .to_string();
    let address_json = serde_json::json!([{
        "ifname": plan.interface_name,
        "addr_info": [{
            "family": if address_local.contains(':') { "inet6" } else { "inet" },
            "local": address_local,
            "prefixlen": address_prefix_len,
            "peer": address_peer,
        }],
    }])
    .to_string();
    let ip = root.join("ip-inspect.sh");
    write_executable(
        &ip,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"-details\" ] && [ \"$2\" = \"-json\" ] && [ \"$3\" = \"link\" ]; then\ncat <<'JSON'\n{inspect_json}\nJSON\nexit 0\nfi\nif [ \"$1\" = \"-details\" ] && [ \"$2\" = \"-json\" ] && [ \"$3\" = \"addr\" ]; then\ncat <<'JSON'\n{address_json}\nJSON\nexit 0\nfi\nexit 0\n"
        ),
    )
    .await;
    config.network.runtime_ip_argv = vec![ip.to_string_lossy().to_string()];
}

fn test_config(root: &Path) -> AgentConfig {
    AgentConfig {
        client_id: "left-a".to_string(),
        network: AgentNetworkConfig {
            apply_enabled: true,
            runtime_reconcile_enabled: true,
            root_dir: root.to_string_lossy().to_string(),
            runtime_ip_argv: vec!["/bin/echo".to_string()],
            runtime_tc_argv: vec!["/bin/echo".to_string()],
            ..AgentNetworkConfig::default()
        },
        ..AgentConfig::default()
    }
}

fn test_root(name: &str, job_id: uuid::Uuid) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("vpsman-runtime-{name}-{job_id}"))
}

async fn write_executable(path: &Path, contents: &str) {
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .await
        .unwrap();
}
