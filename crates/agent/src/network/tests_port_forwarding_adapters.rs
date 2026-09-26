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
        .await
        .unwrap();
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
        .unwrap()
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
        handle.reconcile(
            &cleanup,
            false,
            true,
            true,
            vec![],
            CommandCancelToken::default(),
        ),
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
async fn failed_removal_retires_old_commands_without_claiming_absence() {
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
        .await
        .unwrap();
    assert!(removed.is_empty());
    assert_eq!(failed.len(), 1);
    assert!(!inventory.owned.contains_key(&rule.id));
    assert_eq!(inventory.cleanup_failures.len(), 1);
    assert!(inventory.removed_rules().is_empty());
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
        .await
        .unwrap();
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
        .await
        .unwrap();
    assert_eq!(removed, vec![rule.id]);
    assert!(failed.is_empty());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn failed_disable_keeps_diagnostics_without_replaying_old_commands_after_restart() {
    let (mut inventory, mut rule, root) = fixture().await;
    let new_adapter = rule.adapter.clone();
    let cleanup_calls = root.join("cleanup-calls");
    rule.adapter.as_mut().unwrap().remove = command(&[
        "/bin/sh",
        "-c",
        "printf x >> \"$1\"; exit 1",
        "fixture",
        cleanup_calls.to_str().unwrap(),
    ]);
    inventory.apply(&rule, CommandCancelToken::default()).await;
    let disabled = AgentPortForwardingConfig {
        schema_version: 2,
        cleanup_rules: vec![PortForwardCleanupRule {
            rule_id: rule.id,
            revision: 2,
        }],
        ..AgentPortForwardingConfig::default()
    };
    let (_, mut consumer) = PortForwardingConsumer::channel("client with spaces".into());
    consumer.inventory = inventory;
    let result = consumer
        .reconcile(&disabled, false, true, true, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(result.status, PortForwardRuntimeStatus::Failed);
    assert!(result.removed_rules.is_empty());
    assert!(consumer.inventory.owned.is_empty());
    let saved: InventoryRecord = serde_json::from_slice(
        &tokio::fs::read(
            consumer
                .inventory
                .root
                .as_ref()
                .unwrap()
                .join("ownership.json"),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert!(
        saved.owned.is_empty(),
        "failed cleanup must not persist executable old ownership"
    );
    assert_eq!(saved.cleanup_failures.len(), 1);

    let (_, mut restarted) = PortForwardingConsumer::channel("client with spaces".into());
    restarted.inventory.root = consumer.inventory.root.clone();
    let inspected = restarted
        .inspect(&disabled, 0, CommandCancelToken::default())
        .await;
    assert_eq!(inspected.status, PortForwardRuntimeStatus::Failed);
    assert!(inspected.removed_rules.is_empty());
    let retried = restarted
        .reconcile(&disabled, false, true, true, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(retried.status, PortForwardRuntimeStatus::Failed);
    assert!(retried.removed_rules.is_empty());
    assert_eq!(tokio::fs::read(&cleanup_calls).await.unwrap(), b"x");

    rule.revision = 3;
    rule.adapter = new_adapter;
    rule.adapter.as_mut().unwrap().definition_hash = "latest-definition".into();
    let enabled = AgentPortForwardingConfig {
        schema_version: 2,
        desired_hash: port_forwarding_desired_hash(&[rule.clone()]),
        rules: vec![rule.clone()],
        ..AgentPortForwardingConfig::default()
    };
    let applied = restarted
        .reconcile(&enabled, false, true, true, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(applied.status, PortForwardRuntimeStatus::Applied);
    assert!(restarted.inventory.cleanup_failures.is_empty());
    assert_eq!(restarted.inventory.owned[&rule.id].rule, rule);
    assert_eq!(tokio::fs::read(&cleanup_calls).await.unwrap(), b"x");
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn replacement_applies_new_definition_even_when_old_cleanup_fails() {
    for new_apply_fails in [false, true] {
        let (mut inventory, mut old, root) = fixture().await;
        let mut replacement = old.clone();
        old.adapter.as_mut().unwrap().remove = command(&["/bin/false"]);
        inventory.apply(&old, CommandCancelToken::default()).await;
        replacement.revision = 2;
        replacement.adapter.as_mut().unwrap().definition_hash = "replacement".into();
        let new_apply = root.join("new-apply");
        replacement.adapter.as_mut().unwrap().apply = command(&[
            "/bin/sh",
            "-c",
            if new_apply_fails {
                "printf x > \"$1\"; exit 1"
            } else {
                "printf x > \"$1\""
            },
            "fixture",
            new_apply.to_str().unwrap(),
        ]);
        let config = AgentPortForwardingConfig {
            schema_version: 2,
            desired_hash: port_forwarding_desired_hash(&[replacement.clone()]),
            rules: vec![replacement.clone()],
            ..AgentPortForwardingConfig::default()
        };
        let (_, mut consumer) = PortForwardingConsumer::channel("client with spaces".into());
        consumer.inventory = inventory;
        let result = consumer
            .reconcile(&config, false, true, true, CommandCancelToken::default())
            .await
            .unwrap();
        assert_eq!(tokio::fs::read(new_apply).await.unwrap(), b"x");
        assert_eq!(
            result.status,
            PortForwardRuntimeStatus::Failed,
            "new Apply success cannot hide cleanup failure"
        );
        assert_eq!(consumer.inventory.owned[&old.id].rule, replacement);
        assert_eq!(consumer.inventory.cleanup_failures.len(), 1);
        if new_apply_fails {
            assert_eq!(
                result.rules[0].error_code.as_deref(),
                Some("adapter_cleanup_and_apply_failed")
            );
            assert!(result.rules[0]
                .error_message
                .as_deref()
                .unwrap()
                .contains("newest adapter apply failed"));
        }
        let inspected = consumer
            .inspect(&config, 0, CommandCancelToken::default())
            .await;
        assert_eq!(inspected.status, PortForwardRuntimeStatus::Failed);
        assert!(inspected.removed_rules.is_empty());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}

#[tokio::test]
async fn canceled_or_unstarted_work_adopts_new_owner_without_executing_commands() {
    for canceled_reconcile in [true, false] {
        let (mut inventory, mut old, root) = fixture().await;
        let old_status = root.join("old-status");
        let old_remove = root.join("old-remove");
        old.adapter.as_mut().unwrap().status = command(&[
            "/bin/sh",
            "-c",
            "printf x >> \"$1\"; printf '%s' '{\"state\":\"applied\"}'",
            "fixture",
            old_status.to_str().unwrap(),
        ]);
        old.adapter.as_mut().unwrap().remove = command(&[
            "/bin/sh",
            "-c",
            "printf x >> \"$1\"",
            "fixture",
            old_remove.to_str().unwrap(),
        ]);
        inventory.apply(&old, CommandCancelToken::default()).await;
        let old_config = AgentPortForwardingConfig {
            schema_version: 2,
            desired_hash: port_forwarding_desired_hash(&[old.clone()]),
            rules: vec![old.clone()],
            ..AgentPortForwardingConfig::default()
        };
        let mut newest = old.clone();
        newest.revision = 2;
        newest.adapter.as_mut().unwrap().definition_hash = "newest".into();
        let new_apply = root.join("new-apply");
        let new_status = root.join("new-status");
        newest.adapter.as_mut().unwrap().apply = command(&[
            "/bin/sh",
            "-c",
            "printf x >> \"$1\"",
            "fixture",
            new_apply.to_str().unwrap(),
        ]);
        newest.adapter.as_mut().unwrap().status = command(&[
            "/bin/sh",
            "-c",
            "printf x >> \"$1\"; printf '%s' '{\"state\":\"applied\"}'",
            "fixture",
            new_status.to_str().unwrap(),
        ]);
        let newest_config = AgentPortForwardingConfig {
            schema_version: 2,
            desired_hash: port_forwarding_desired_hash(&[newest.clone()]),
            rules: vec![newest.clone()],
            ..AgentPortForwardingConfig::default()
        };
        let inventory_root = inventory.root.clone();
        let (handle, mut consumer) = PortForwardingConsumer::channel("client with spaces".into());
        consumer.inventory = inventory;
        consumer.active_config = Some(old_config.clone());
        let task = tokio::spawn(consumer.run());
        if canceled_reconcile {
            let canceled = CommandCancelToken::default();
            canceled.cancel("fixture canceled before polling".into());
            assert!(handle
                .reconcile(&newest_config, false, true, true, vec![], canceled)
                .await
                .is_err());
        }
        // Also covers the outer cancellation path where reconcile was never polled.
        handle.adopt_desired(&newest_config).await.unwrap();
        assert!(!tokio::fs::try_exists(&old_remove).await.unwrap());
        assert!(!tokio::fs::try_exists(&new_apply).await.unwrap());
        assert!(!tokio::fs::try_exists(&new_status).await.unwrap());
        assert_eq!(tokio::fs::read(&old_status).await.unwrap(), b"x");
        let mut restarted = AdapterInventory::new("client with spaces".into());
        restarted.root = inventory_root.clone();
        restarted.load().await.unwrap();
        assert_eq!(restarted.owned[&old.id].rule, newest);
        assert!(restarted.removed_rules().is_empty());
        assert_eq!(restarted.cleanup_failures.len(), 1);

        let inspected = handle.inspect(&old_config).await.unwrap();
        assert_eq!(inspected.status, PortForwardRuntimeStatus::Failed);
        assert_eq!(inspected.desired_hash, Some(newest_config.desired_hash));
        assert_eq!(tokio::fs::read(&new_status).await.unwrap(), b"x");
        assert_eq!(tokio::fs::read(&old_status).await.unwrap(), b"x");

        let disabled = AgentPortForwardingConfig {
            schema_version: 2,
            cleanup_rules: vec![PortForwardCleanupRule {
                rule_id: old.id,
                revision: 3,
            }],
            ..AgentPortForwardingConfig::default()
        };
        handle.adopt_desired(&disabled).await.unwrap();
        let inspected = handle.inspect(&old_config).await.unwrap();
        assert_eq!(inspected.status, PortForwardRuntimeStatus::Failed);
        assert!(inspected.removed_rules.is_empty());
        assert!(!tokio::fs::try_exists(&old_remove).await.unwrap());
        assert_eq!(tokio::fs::read(&new_status).await.unwrap(), b"x");
        drop(handle);
        task.await.unwrap().unwrap();
        let mut restarted = AdapterInventory::new("client with spaces".into());
        restarted.root = inventory_root;
        restarted.load().await.unwrap();
        assert!(restarted.owned.is_empty());
        assert!(restarted.removed_rules().is_empty());
        assert_eq!(restarted.cleanup_failures.len(), 1);
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}

#[tokio::test]
async fn unrelated_update_preserves_cleanup_failure_until_explicit_successful_reapply() {
    let (mut inventory, mut old, root) = fixture().await;
    let apply_calls = root.join("apply-calls");
    old.adapter.as_mut().unwrap().apply.argv[2].push_str("; printf x >> \"$2\"");
    old.adapter
        .as_mut()
        .unwrap()
        .apply
        .argv
        .push(apply_calls.to_str().unwrap().into());
    old.adapter.as_mut().unwrap().remove = command(&["/bin/false"]);
    inventory.apply(&old, CommandCancelToken::default()).await;
    let mut current = old.clone();
    current.revision += 1;
    current.adapter.as_mut().unwrap().definition_hash = "current".into();
    let mut config = AgentPortForwardingConfig {
        schema_version: 2,
        desired_hash: port_forwarding_desired_hash(&[current.clone()]),
        rules: vec![current.clone()],
        ..AgentPortForwardingConfig::default()
    };
    let (_, mut consumer) = PortForwardingConsumer::channel("client with spaces".into());
    consumer.inventory = inventory;
    let initial = consumer
        .reconcile(&config, false, false, false, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(initial.status, PortForwardRuntimeStatus::Failed);
    assert_eq!(tokio::fs::read(&apply_calls).await.unwrap(), b"xx");

    // Changing an unrelated adapter (and refreshing a display label) must not
    // re-run this owner's Apply or erase its old cleanup diagnostics.
    config.rules[0].adapter.as_mut().unwrap().definition_name = "renamed label".into();
    let mut unrelated = current.clone();
    unrelated.id = Uuid::new_v4();
    unrelated.adapter.as_mut().unwrap().definition_id = Uuid::new_v4();
    unrelated.adapter.as_mut().unwrap().apply = command(&["/bin/true"]);
    config.rules.push(unrelated);
    config.desired_hash = port_forwarding_desired_hash(&config.rules);
    let updated = consumer
        .reconcile(&config, false, false, false, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(updated.status, PortForwardRuntimeStatus::Failed);
    assert_eq!(tokio::fs::read(&apply_calls).await.unwrap(), b"xx");
    assert_eq!(consumer.inventory.cleanup_failures.len(), 1);

    // Startup/authoritative recovery still runs Apply, but it is not proof
    // that the operator repaired residue left by an earlier cleanup failure.
    let recovered = consumer
        .reconcile(&config, false, true, false, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(recovered.status, PortForwardRuntimeStatus::Failed);
    assert_eq!(tokio::fs::read(&apply_calls).await.unwrap(), b"xxx");
    assert_eq!(consumer.inventory.cleanup_failures.len(), 1);

    let reapplied = consumer
        .reconcile(&config, false, true, true, CommandCancelToken::default())
        .await
        .unwrap();
    assert_eq!(reapplied.status, PortForwardRuntimeStatus::Applied);
    assert_eq!(tokio::fs::read(&apply_calls).await.unwrap(), b"xxxx");
    assert!(consumer.inventory.cleanup_failures.is_empty());
    tokio::fs::remove_dir_all(root).await.unwrap();
}

#[tokio::test]
async fn retired_custom_cleanup_can_be_repaired_after_switch_to_native() {
    for mode in [PortForwardMode::Dnat, PortForwardMode::Redirect] {
        for explicit_reapply in [false, true] {
            let (mut inventory, mut old, root) = fixture().await;
            let cleanup_calls = root.join("cleanup-calls");
            old.adapter.as_mut().unwrap().remove = command(&[
                "/bin/sh",
                "-c",
                "printf x >> \"$1\"; exit 1",
                "fixture",
                cleanup_calls.to_str().unwrap(),
            ]);
            assert_eq!(
                inventory
                    .apply(&old, CommandCancelToken::default())
                    .await
                    .status,
                Some(PortForwardRuntimeStatus::Applied)
            );
            let mut replacement = old.clone();
            replacement.revision = 2;
            replacement.mode = mode;
            replacement.adapter = None;
            replacement.target_ip =
                (mode == PortForwardMode::Dnat).then(|| "192.0.2.8".parse().unwrap());
            replacement.address_family =
                (mode == PortForwardMode::Redirect).then_some(PortForwardAddressFamily::Ipv4);
            let desired = AgentPortForwardingConfig {
                schema_version: 2,
                desired_hash: port_forwarding_desired_hash(&[replacement.clone()]),
                rules: vec![replacement],
                ..AgentPortForwardingConfig::default()
            };
            let (_, failed) = inventory
                .remove_replaced(&desired, CommandCancelToken::default())
                .await
                .unwrap();
            assert_eq!(failed.len(), 1);
            assert!(inventory.owned.is_empty());
            let native = native_config(&desired);
            // Kernel execution is unchanged. Exercise its verified-success
            // result boundary against the real persisted adapter inventory.
            let applied = PortForwardRuntimeSnapshot {
                status: PortForwardRuntimeStatus::Applied,
                owned_table_present: Some(true),
                ..PortForwardRuntimeSnapshot::default()
            };
            let all_native = native_repair_candidates(None, &desired, true);
            inventory
                .confirm_native_repairs(
                    &applied,
                    &all_native,
                    &failed.iter().map(|stat| stat.rule_id).collect(),
                )
                .await
                .unwrap();
            assert_eq!(inventory.cleanup_failures.len(), 1);
            assert_eq!(
                merge_snapshot(
                    &desired,
                    &native,
                    applied.clone(),
                    inventory.cleanup_failures(&desired),
                )
                .status,
                PortForwardRuntimeStatus::Failed,
                "initial native success cannot hide cleanup failure"
            );

            let mut restarted = AdapterInventory::new("client with spaces".into());
            restarted.root = inventory.root.clone();
            restarted.load().await.unwrap();
            for previous in [None, Some(&desired)] {
                let recovery = native_repair_candidates(previous, &desired, false);
                assert!(recovery.is_empty());
                restarted
                    .confirm_native_repairs(&applied, &recovery, &BTreeSet::new())
                    .await
                    .unwrap();
                assert_eq!(restarted.cleanup_failures.len(), 1);
            }

            let disabled = AgentPortForwardingConfig::default();
            let repairs = native_repair_candidates(
                Some(if explicit_reapply {
                    &desired
                } else {
                    &disabled
                }),
                &desired,
                explicit_reapply,
            );
            assert_eq!(repairs, BTreeSet::from([old.id]));
            restarted
                .confirm_native_repairs(
                    &PortForwardRuntimeSnapshot {
                        status: PortForwardRuntimeStatus::Failed,
                        ..applied.clone()
                    },
                    &repairs,
                    &BTreeSet::new(),
                )
                .await
                .unwrap();
            assert_eq!(restarted.cleanup_failures.len(), 1);
            restarted
                .confirm_native_repairs(&applied, &repairs, &BTreeSet::new())
                .await
                .unwrap();
            assert!(restarted.cleanup_failures.is_empty());
            assert_eq!(
                merge_snapshot(
                    &desired,
                    &native,
                    applied,
                    restarted.cleanup_failures(&desired),
                )
                .status,
                PortForwardRuntimeStatus::Applied
            );
            let mut persisted = AdapterInventory::new("client with spaces".into());
            persisted.root = inventory.root;
            persisted.load().await.unwrap();
            assert!(persisted.cleanup_failures.is_empty());
            assert_eq!(tokio::fs::read(&cleanup_calls).await.unwrap(), b"x");
            tokio::fs::remove_dir_all(root).await.unwrap();
        }
    }
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
