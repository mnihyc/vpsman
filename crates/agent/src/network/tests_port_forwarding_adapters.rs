use super::*;
use vpsman_common::{pair_port_expressions, PortForwardAdapterCommands, PortForwardProtocol};

fn command(argv: &[&str]) -> RuntimeTunnelCommand {
    RuntimeTunnelCommand {
        argv: argv.iter().map(|arg| arg.to_string()).collect(),
        max_timeout_secs: 5,
        max_output_bytes: 16 * 1024,
    }
}

async fn fixture() -> (AdapterInventory, PortForwardRule, PathBuf) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp")
        .join(format!("pf-adapter-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&root).await.unwrap();
    let state = root.join("listener-state.json");
    let state = state.to_str().unwrap();
    let rule = PortForwardRule {
        id: Uuid::new_v4(),
        revision: 1,
        name: "fixture".to_string(),
        protocol: PortForwardProtocol::Both,
        target_ip: None,
        mappings: pair_port_expressions("80,1000-1002", "8080,2000-2002").unwrap(),
        masquerade: false,
        mode: PortForwardMode::CustomAdapter,
        address_family: None,
        adapter: Some(PortForwardAdapterCommands {
            definition_id: Uuid::new_v4(),
            definition_name: "fixture service".to_string(),
            definition_hash: "fixture".to_string(),
            apply: command(&[
                "/bin/sh",
                "-c",
                "printf '%s' '{\"state\":\"applied\"}' > \"$1\"",
                "fixture",
                state,
            ]),
            remove: command(&[
                "/bin/sh",
                "-c",
                "printf '%s' '{\"state\":\"absent\"}' > \"$1\"",
                "fixture",
                state,
            ]),
            status: command(&["/bin/cat", state]),
        }),
    };
    let mut inventory = AdapterInventory::new("client with spaces".to_string());
    inventory.root = Some(root.join("owners"));
    inventory.load().await.unwrap();
    (inventory, rule, root)
}

#[tokio::test]
async fn exact_arguments_preserve_empty_destination_and_mapping_correspondence() {
    let (_, rule, root) = fixture().await;
    let entry = OwnedRule {
        client_id: "client with {protocol}".to_string(),
        rule,
    };
    let rendered = render(
        &command(&[
            "/usr/bin/helper",
            "{client_id}",
            "{protocol}",
            "{incoming_ports}",
            "{target_ports}",
            "{target_ip}",
        ]),
        &entry,
    )
    .unwrap();
    assert_eq!(
        &rendered[1..],
        &[
            "client with {protocol}",
            "both",
            "80,1000-1002",
            "8080,2000-2002",
            ""
        ]
    );
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn interrupted_apply_retains_cleanup_owner_across_restart() {
    let (mut inventory, mut rule, root) = fixture().await;
    rule.adapter.as_mut().unwrap().apply.argv[2].push_str("; exit 1");
    let failed = inventory.apply(&rule, CommandCancelToken::default()).await;
    assert_eq!(failed.status, Some(PortForwardRuntimeStatus::Failed));
    let mut restarted = AdapterInventory::new("client with spaces".to_string());
    restarted.root = inventory.root.clone();
    restarted.load().await.unwrap();
    assert!(restarted.owned.contains_key(&rule.id));
    let (removed, failed) = restarted
        .remove_replaced(
            &AgentPortForwardingConfig::default(),
            CommandCancelToken::default(),
        )
        .await;
    assert!(failed.is_empty());
    assert_eq!(removed, vec![rule.id]);
    assert!(restarted.owned.is_empty());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn cleanup_receipts_survive_restart_and_are_invalidated_by_new_apply() {
    let (mut inventory, mut rule, root) = fixture().await;
    inventory.apply(&rule, CommandCancelToken::default()).await;
    let request = PortForwardCleanupRule {
        rule_id: rule.id,
        revision: 2,
    };
    let cleanup = AgentPortForwardingConfig {
        schema_version: 2,
        cleanup_rules: vec![request.clone()],
        ..AgentPortForwardingConfig::default()
    };
    inventory
        .synchronize_cleanup_requests(&cleanup.cleanup_rules)
        .await
        .unwrap();
    assert!(inventory.removed_rules().is_empty());
    assert!(inventory
        .remove_replaced(&cleanup, CommandCancelToken::default())
        .await
        .1
        .is_empty());
    let mut restarted = AdapterInventory::new("client with spaces".to_string());
    restarted.root = inventory.root.clone();
    restarted.load().await.unwrap();
    assert_eq!(restarted.removed_rules(), vec![request]);
    rule.revision = 3;
    assert_eq!(
        restarted
            .apply(&rule, CommandCancelToken::default())
            .await
            .status,
        Some(PortForwardRuntimeStatus::Applied)
    );
    assert!(restarted.removed_rules().is_empty());
    restarted
        .synchronize_cleanup_requests(&[PortForwardCleanupRule {
            rule_id: rule.id,
            revision: 4,
        }])
        .await
        .unwrap();
    assert!(restarted.removed_rules().is_empty());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn requested_cleanup_preempts_slow_background_status() {
    let (mut inventory, mut rule, root) = fixture().await;
    inventory.apply(&rule, CommandCancelToken::default()).await;
    let started = root.join("started");
    rule.adapter.as_mut().unwrap().status = command(&[
        "/bin/sh",
        "-c",
        "printf x > \"$1\"; sleep 5; printf '%s' '{\"state\":\"applied\"}'",
        "fixture",
        started.to_str().unwrap(),
    ]);
    let config = AgentPortForwardingConfig {
        schema_version: 2,
        desired_hash: port_forwarding_desired_hash(&[rule.clone()]),
        rules: vec![rule.clone()],
        ..AgentPortForwardingConfig::default()
    };
    let (handle, mut consumer) = PortForwardingConsumer::channel("client with spaces".to_string());
    consumer.inventory = inventory;
    let task = tokio::spawn(consumer.run());
    handle.snapshot(&config, 0);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !tokio::fs::try_exists(&started).await.unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let request = PortForwardCleanupRule {
        rule_id: rule.id,
        revision: 2,
    };
    let cleanup = AgentPortForwardingConfig {
        schema_version: 2,
        cleanup_rules: vec![request.clone()],
        ..AgentPortForwardingConfig::default()
    };
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        handle.reconcile(&cleanup, false, vec![], CommandCancelToken::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(snapshot.removed_rules, vec![request]);
    assert_eq!(snapshot.status, PortForwardRuntimeStatus::Absent);
    drop(handle);
    task.await.unwrap().unwrap();
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn status_must_confirm_removal_before_ownership_is_released() {
    let (mut inventory, mut rule, root) = fixture().await;
    rule.adapter.as_mut().unwrap().remove = command(&["/bin/true"]);
    assert_eq!(
        inventory
            .apply(&rule, CommandCancelToken::default())
            .await
            .status,
        Some(PortForwardRuntimeStatus::Applied)
    );
    let (removed, failed) = inventory
        .remove_replaced(
            &AgentPortForwardingConfig::default(),
            CommandCancelToken::default(),
        )
        .await;
    assert!(removed.is_empty());
    assert_eq!(failed.len(), 1);
    assert!(inventory.owned.contains_key(&rule.id));
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn same_adapter_update_retains_owner_but_changed_definition_hash_requires_cleanup() {
    let (mut inventory, mut rule, root) = fixture().await;
    inventory.apply(&rule, CommandCancelToken::default()).await;
    rule.revision += 1;
    let mut config = AgentPortForwardingConfig {
        schema_version: 2,
        desired_hash: port_forwarding_desired_hash(&[rule.clone()]),
        rules: vec![rule.clone()],
        ..AgentPortForwardingConfig::default()
    };
    let (removed, failed) = inventory
        .remove_replaced(&config, CommandCancelToken::default())
        .await;
    assert!(removed.is_empty() && failed.is_empty());
    assert_eq!(
        inventory
            .apply(&rule, CommandCancelToken::default())
            .await
            .status,
        Some(PortForwardRuntimeStatus::Applied)
    );
    assert_eq!(inventory.owned[&rule.id].rule.revision, 2);
    config.rules[0].adapter.as_mut().unwrap().definition_hash = "updated-definition".to_string();
    let (removed, failed) = inventory
        .remove_replaced(&config, CommandCancelToken::default())
        .await;
    assert_eq!(removed, vec![rule.id]);
    assert!(failed.is_empty());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn native_replacement_keeps_current_revision_cleanup_failure_during_observation() {
    let (mut inventory, rule, root) = fixture().await;
    inventory.apply(&rule, CommandCancelToken::default()).await;
    let mut replacement = rule.clone();
    replacement.revision = 2;
    replacement.mode = PortForwardMode::Dnat;
    replacement.target_ip = Some("192.0.2.8".parse().unwrap());
    replacement.adapter = None;
    let config = AgentPortForwardingConfig {
        schema_version: 2,
        rules: vec![replacement],
        cleanup_rules: vec![PortForwardCleanupRule {
            rule_id: rule.id,
            revision: 2,
        }],
        ..AgentPortForwardingConfig::default()
    };
    let failures = inventory.cleanup_failures(&config);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].revision, 2);
    assert_eq!(failures[0].mode, PortForwardMode::Dnat);
    assert_eq!(failures[0].status, Some(PortForwardRuntimeStatus::Failed));
    assert!(inventory.retains_owner(rule.id));
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn slow_adapter_observation_does_not_block_telemetry_or_queue_duplicate_checks() {
    let (mut inventory, mut rule, root) = fixture().await;
    inventory.apply(&rule, CommandCancelToken::default()).await;
    let started = root.join("started");
    let started_arg = started.to_str().unwrap();
    rule.adapter.as_mut().unwrap().status = command(&[
        "/bin/sh",
        "-c",
        "printf x > \"$1\"; sleep 0.3; printf '%s' '{\"state\":\"applied\"}'",
        "fixture",
        started_arg,
    ]);
    let config = AgentPortForwardingConfig {
        schema_version: 2,
        desired_hash: port_forwarding_desired_hash(&[rule.clone()]),
        rules: vec![rule],
        ..AgentPortForwardingConfig::default()
    };
    let (handle, mut consumer) = PortForwardingConsumer::channel("client with spaces".to_string());
    consumer.inventory = inventory;
    let task = tokio::spawn(consumer.run());
    handle.snapshot(&config, 0);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !tokio::fs::try_exists(&started).await.unwrap() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let start = std::time::Instant::now();
    for _ in 0..100 {
        handle.snapshot(&config, 0);
    }
    assert!(start.elapsed() < std::time::Duration::from_millis(100));
    assert!(handle.observation_queued.load(Ordering::Acquire));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while handle.observation_queued.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        handle.published.borrow().status,
        PortForwardRuntimeStatus::Applied
    );
    drop(handle);
    task.await.unwrap().unwrap();
    tokio::fs::remove_dir_all(root).await.unwrap();
}
