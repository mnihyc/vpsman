use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    fmt::Write as _,
    net::IpAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::{
    process::{Child, ChildStdout, Command},
    sync::{mpsc, oneshot, watch},
};
use vpsman_common::{
    payload_hash, port_forwarding_desired_hash, validate_port_forwarding_config,
    AgentPortForwardingConfig, PortForwardAddressFamily, PortForwardCapability,
    PortForwardCapabilityStatus, PortForwardMode, PortForwardRule, PortForwardRuleRuntimeStat,
    PortForwardRuntimeSnapshot, PortForwardRuntimeStatus, MAX_PORT_FORWARD_NFT_SCRIPT_BYTES,
};

use crate::{
    child_process::{
        run_child_with_bounded_output, run_child_with_input_bounded_output_cancelable,
        ChildCleanupPolicy, ChildRunResult,
    },
    command_worker::CommandCancelToken,
    telemetry::unix_now,
};

pub(crate) const OWNED_TABLE_FAMILY: &str = "inet";
pub(crate) const OWNED_TABLE_NAME: &str = "vpsman_port_forward";
const OWNED_TABLE_COMMENT_PREFIX: &str = "vpsman-owned desired=";
const OWNERSHIP_SET_NAME: &str = "vpsman_ownership_v1";
const OWNERSHIP_MARK: u64 = 0x5650_534d;
const OWNED_FLOW_SET_NAME: &str = "owned_flows";
const NFT_OUTPUT_LIMIT: usize = MAX_PORT_FORWARD_NFT_SCRIPT_BYTES * 4;
const NFT_TIMEOUT_SECS: u64 = 15;

#[path = "port_forwarding_adapters.rs"]
mod adapters;
use adapters::AdapterInventory;

#[derive(Clone, Debug)]
struct AppliedBaseline {
    desired_hash: String,
    observed_hash: String,
    // Compared with terse listings only while the nft event stream proves that
    // no static table element has changed since the exact full inspection.
    structure_hash: String,
    event_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TableListMode {
    Full,
    Terse,
}

enum PortForwardingWork {
    Probe {
        reply: oneshot::Sender<PortForwardCapability>,
    },
    Reconcile {
        config: AgentPortForwardingConfig,
        require_table_access: bool,
        previous_native_ids: Vec<uuid::Uuid>,
        cancel_token: CommandCancelToken,
        reply: oneshot::Sender<Result<PortForwardRuntimeSnapshot>>,
    },
    Inspect {
        config: AgentPortForwardingConfig,
        reply: oneshot::Sender<PortForwardRuntimeSnapshot>,
    },
    Observe {
        config: AgentPortForwardingConfig,
        custom_interval_secs: u64,
        cancel_token: CommandCancelToken,
    },
}

#[derive(Clone)]
pub(crate) struct PortForwardingConsumerHandle {
    work_tx: mpsc::UnboundedSender<PortForwardingWork>,
    published: watch::Receiver<PortForwardRuntimeSnapshot>,
    observation_queued: Arc<AtomicBool>,
    observation_cancel: Arc<Mutex<CommandCancelToken>>,
}

impl PortForwardingConsumerHandle {
    /// Reading telemetry never waits behind an external management command.
    pub(crate) fn snapshot(
        &self,
        config: &AgentPortForwardingConfig,
        interval_secs: u64,
    ) -> PortForwardRuntimeSnapshot {
        let snapshot = self.published.borrow().clone();
        if !self.observation_queued.swap(true, Ordering::AcqRel) {
            let token = CommandCancelToken::default();
            *self
                .observation_cancel
                .lock()
                .expect("observation token lock") = token.clone();
            if self
                .work_tx
                .send(PortForwardingWork::Observe {
                    config: config.clone(),
                    custom_interval_secs: interval_secs,
                    cancel_token: token,
                })
                .is_err()
            {
                self.observation_queued.store(false, Ordering::Release);
            }
        }
        snapshot
    }
    pub(crate) async fn probe(&self) -> Result<PortForwardCapability> {
        self.cancel_background_observation();
        let (reply, response) = oneshot::channel();
        self.work_tx
            .send(PortForwardingWork::Probe { reply })
            .map_err(|_| anyhow::anyhow!("port-forwarding consumer is unavailable"))?;
        response
            .await
            .context("port-forwarding consumer stopped before capability response")
    }

    pub(crate) async fn reconcile(
        &self,
        config: &AgentPortForwardingConfig,
        require_table_access: bool,
        previous_native_ids: Vec<uuid::Uuid>,
        cancel_token: CommandCancelToken,
    ) -> Result<PortForwardRuntimeSnapshot> {
        self.cancel_background_observation();
        let (reply, response) = oneshot::channel();
        self.work_tx
            .send(PortForwardingWork::Reconcile {
                config: config.clone(),
                require_table_access,
                previous_native_ids,
                cancel_token,
                reply,
            })
            .map_err(|_| anyhow::anyhow!("port-forwarding consumer is unavailable"))?;
        response
            .await
            .context("port-forwarding consumer stopped before reconcile response")?
    }

    pub(crate) async fn inspect(
        &self,
        config: &AgentPortForwardingConfig,
    ) -> Result<PortForwardRuntimeSnapshot> {
        self.cancel_background_observation();
        let (reply, response) = oneshot::channel();
        self.work_tx
            .send(PortForwardingWork::Inspect {
                config: config.clone(),
                reply,
            })
            .map_err(|_| anyhow::anyhow!("port-forwarding consumer is unavailable"))?;
        response
            .await
            .context("port-forwarding consumer stopped before inspection response")
    }

    fn cancel_background_observation(&self) {
        self.observation_cancel
            .lock()
            .expect("observation token lock")
            .cancel("superseded by requested port-forwarding work".to_string());
    }
}

pub(crate) struct PortForwardingConsumer {
    work_rx: mpsc::UnboundedReceiver<PortForwardingWork>,
    capability: PortForwardCapability,
    baseline: Option<AppliedBaseline>,
    owned_table_event_generation: u64,
    monitor: Option<NftMonitorConsumer>,
    published: watch::Sender<PortForwardRuntimeSnapshot>,
    observation_queued: Arc<AtomicBool>,
    inventory: AdapterInventory,
    native_owners: BTreeSet<uuid::Uuid>,
    active_config: Option<AgentPortForwardingConfig>,
}

impl PortForwardingConsumer {
    pub(crate) fn channel(client_id: String) -> (PortForwardingConsumerHandle, Self) {
        let (work_tx, work_rx) = mpsc::unbounded_channel();
        let (published, snapshots) = watch::channel(PortForwardRuntimeSnapshot::default());
        let observation_queued = Arc::new(AtomicBool::new(false));
        (
            PortForwardingConsumerHandle {
                work_tx,
                published: snapshots,
                observation_queued: observation_queued.clone(),
                observation_cancel: Arc::new(Mutex::new(CommandCancelToken::default())),
            },
            Self {
                work_rx,
                capability: PortForwardCapability::default(),
                baseline: None,
                owned_table_event_generation: 0,
                monitor: None,
                published,
                observation_queued,
                inventory: AdapterInventory::new(client_id),
                native_owners: BTreeSet::new(),
                active_config: None,
            },
        )
    }

    pub(crate) async fn run(mut self) -> Result<()> {
        loop {
            tokio::select! {
                work = self.work_rx.recv() => {
                    let Some(work) = work else {
                        return Ok(());
                    };
                    self.process(work).await;
                }
                event = next_monitor_event(&mut self.monitor), if self.monitor.is_some() => {
                    match event {
                        NftMonitorEvent::Line(line) => {
                            if nft_event_invalidates_owned_table(&line) {
                                self.owned_table_event_generation =
                                    self.owned_table_event_generation.wrapping_add(1);
                            }
                        }
                        NftMonitorEvent::Stopped => {
                            self.monitor = None;
                        }
                    }
                }
            }
        }
    }

    async fn process(&mut self, work: PortForwardingWork) {
        match work {
            PortForwardingWork::Probe { reply } => {
                let capability = self.probe().await;
                let _ = reply.send(capability);
            }
            PortForwardingWork::Reconcile {
                config,
                require_table_access,
                previous_native_ids,
                cancel_token,
                reply,
            } => {
                self.native_owners.extend(previous_native_ids);
                let result = self
                    .reconcile(&config, require_table_access, cancel_token)
                    .await;
                if let Ok(snapshot) = &result {
                    self.published.send_replace(snapshot.clone());
                }
                let _ = reply.send(result);
            }
            PortForwardingWork::Inspect { config, reply } => {
                let config = self.active_config.clone().unwrap_or(config);
                let snapshot = self
                    .inspect(&config, 0, CommandCancelToken::default())
                    .await;
                self.published.send_replace(snapshot.clone());
                let _ = reply.send(snapshot);
            }
            PortForwardingWork::Observe {
                config,
                custom_interval_secs,
                cancel_token,
            } => {
                let config = self.active_config.clone().unwrap_or(config);
                if !cancel_token.is_canceled() {
                    let snapshot = self
                        .inspect(&config, custom_interval_secs, cancel_token.clone())
                        .await;
                    if !cancel_token.is_canceled() {
                        self.published.send_replace(snapshot);
                    }
                }
                self.observation_queued.store(false, Ordering::Release);
            }
        }
    }

    async fn probe(&mut self) -> PortForwardCapability {
        let mut capability = probe_port_forwarding_capability_inner().await;
        capability.schema_version = 2;
        capability.supported_modes = vec![PortForwardMode::CustomAdapter];
        if capability.supported() {
            capability
                .supported_modes
                .extend([PortForwardMode::Dnat, PortForwardMode::Redirect]);
        }
        if capability.supported() && self.monitor.is_none() {
            if let Some(nft) = resolve_nft_binary() {
                self.monitor = start_nft_monitor(&nft);
            }
        }
        self.capability = capability.clone();
        capability
    }

    async fn reconcile(
        &mut self,
        config: &AgentPortForwardingConfig,
        require_table_access: bool,
        cancel_token: CommandCancelToken,
    ) -> Result<PortForwardRuntimeSnapshot> {
        validate_port_forwarding_config(config)
            .map_err(|error| anyhow::anyhow!("invalid port-forwarding desired state: {error}"))?;
        cancel_token.check("port_forwarding")?;
        self.inventory.load().await?;
        self.active_config = Some(config.clone());
        self.inventory
            .synchronize_cleanup_requests(&config.cleanup_rules)
            .await?;
        let (_, mut custom_stats) = self
            .inventory
            .remove_replaced(config, cancel_token.clone())
            .await;
        cancel_token.check("port_forwarding")?;
        let blocked = custom_stats
            .iter()
            .map(|stat| stat.rule_id)
            .collect::<BTreeSet<_>>();
        let mut native = native_config(config);
        native.rules.retain(|rule| !blocked.contains(&rule.id));
        native.desired_hash = native_hash(&native.rules);
        let native_result = if custom_ownership_context(config)
            && !require_table_access
            && !self.has_native_ownership(&native)
        {
            Ok(PortForwardRuntimeSnapshot {
                status: PortForwardRuntimeStatus::Absent,
                ..PortForwardRuntimeSnapshot::default()
            })
        } else {
            self.reconcile_native(&native, require_table_access, cancel_token.clone())
                .await
        };
        let native_failed = native_result.is_err();
        let mut snapshot = match native_result {
            Ok(snapshot) => snapshot,
            Err(error) => failed_snapshot(
                &native,
                &self.capability,
                "native_reconcile_failed",
                &error.to_string(),
            ),
        };
        for rule in config
            .rules
            .iter()
            .filter(|rule| rule.mode == PortForwardMode::CustomAdapter)
        {
            cancel_token.check("port_forwarding")?;
            if blocked.contains(&rule.id) {
                continue;
            }
            if native_failed && self.native_owners.contains(&rule.id) {
                custom_stats.push(adapters::error_stat(
                    rule,
                    "previous_native_cleanup_failed",
                    "the previous native forwarding rule could not be removed",
                ));
            } else {
                custom_stats.push(self.inventory.apply(rule, cancel_token.clone()).await);
            }
        }
        attach_native_stats(&native, &mut snapshot);
        snapshot.removed_rules = self.inventory.removed_rules();
        Ok(merge_snapshot(config, &native, snapshot, custom_stats))
    }

    async fn inspect(
        &mut self,
        config: &AgentPortForwardingConfig,
        custom_interval_secs: u64,
        cancel_token: CommandCancelToken,
    ) -> PortForwardRuntimeSnapshot {
        if let Err(error) = self.inventory.load().await {
            return failed_snapshot(
                config,
                &self.capability,
                "adapter_inventory_failed",
                &error.to_string(),
            );
        }
        let mut native = native_config(config);
        native
            .rules
            .retain(|rule| !self.inventory.retains_owner(rule.id));
        native.desired_hash = native_hash(&native.rules);
        let mut snapshot =
            if self.has_native_ownership(&native) || !custom_ownership_context(config) {
                self.inspect_native(&native, cancel_token.clone()).await
            } else {
                PortForwardRuntimeSnapshot {
                    status: PortForwardRuntimeStatus::Absent,
                    ..PortForwardRuntimeSnapshot::default()
                }
            };
        attach_native_stats(&native, &mut snapshot);
        snapshot.removed_rules = self.inventory.removed_rules();
        let previous = self.published.borrow().clone();
        let mut custom_stats = if previous.desired_hash.as_deref()
            == (!config.desired_hash.is_empty()).then_some(config.desired_hash.as_str())
            && config
                .rules
                .iter()
                .filter(|rule| rule.mode == PortForwardMode::CustomAdapter)
                .all(|rule| {
                    previous
                        .rules
                        .iter()
                        .find(|stat| stat.rule_id == rule.id && stat.revision == rule.revision)
                        .and_then(|stat| stat.observed_unix)
                        .is_some_and(|observed| {
                            unix_now().saturating_sub(observed) < custom_interval_secs
                        })
                }) {
            previous
                .rules
                .into_iter()
                .filter(|stat| {
                    stat.mode == PortForwardMode::CustomAdapter
                        && config.rules.iter().any(|rule| {
                            rule.id == stat.rule_id && rule.mode == PortForwardMode::CustomAdapter
                        })
                })
                .collect()
        } else {
            self.inventory.inspect(config, cancel_token).await
        };
        custom_stats.extend(self.inventory.cleanup_failures(config));
        merge_snapshot(config, &native, snapshot, custom_stats)
    }

    async fn reconcile_native(
        &mut self,
        config: &AgentPortForwardingConfig,
        require_table_access: bool,
        cancel_token: CommandCancelToken,
    ) -> Result<PortForwardRuntimeSnapshot> {
        validate_port_forwarding_config(config)
            .map_err(|error| anyhow::anyhow!("invalid port-forwarding desired state: {error}"))?;

        let capability = self.probe().await;
        if !capability.supported() {
            if !require_table_access && config.rules.is_empty() {
                return Ok(PortForwardRuntimeSnapshot {
                    status: PortForwardRuntimeStatus::Absent,
                    nft_version: capability.nft_version.clone(),
                    ..PortForwardRuntimeSnapshot::default()
                });
            }
            anyhow::bail!(
                "port forwarding unavailable ({:?}): {}",
                capability.status,
                capability
                    .reason
                    .as_deref()
                    .unwrap_or("capability probe did not provide a reason")
            );
        }
        let nft = resolve_nft_binary().context("nft binary disappeared after capability probe")?;
        let before = list_owned_table(&nft, cancel_token.clone(), TableListMode::Full).await?;
        if before.as_ref().is_some_and(|table| !table_is_owned(table)) {
            anyhow::bail!(
                "port_forward_table_ownership_conflict: table {OWNED_TABLE_FAMILY} {OWNED_TABLE_NAME} exists without the vpsman ownership marker"
            );
        }
        if let Some(table) = before.as_ref() {
            self.native_owners = extract_rule_counters(table)
                .into_iter()
                .map(|rule| rule.rule_id)
                .collect();
        } else {
            self.native_owners.clear();
        }
        if config.rules.is_empty() && before.is_none() {
            self.baseline = None;
            return Ok(runtime_snapshot_from_table(
                config,
                &capability,
                None,
                self.baseline.as_ref(),
            ));
        }

        if let Some(table) = before.as_ref() {
            let observed = runtime_snapshot_from_table(
                config,
                &capability,
                Some(table),
                self.baseline.as_ref(),
            );
            if observed.status == PortForwardRuntimeStatus::Applied {
                return Ok(observed);
            }
        }

        let script = render_apply_script(config, before.is_some())?;
        run_nft_script(&nft, true, script.as_bytes().to_vec(), cancel_token.clone())
            .await
            .context("nft rejected port-forwarding desired state")?;
        run_nft_script(&nft, false, script.into_bytes(), cancel_token.clone())
            .await
            .context("failed to atomically apply port-forwarding desired state")?;

        let event_generation = self.owned_table_event_generation;
        let after = list_owned_table(&nft, cancel_token, TableListMode::Full).await?;
        if config.rules.is_empty() {
            anyhow::ensure!(
                after.is_none(),
                "owned nftables table still exists after removal"
            );
            self.baseline = None;
            self.native_owners.clear();
        } else {
            let observed = after
                .as_ref()
                .context("owned nftables table missing immediately after apply")?;
            self.baseline = Some(AppliedBaseline {
                desired_hash: config.desired_hash.clone(),
                observed_hash: normalized_table_hash(observed),
                structure_hash: normalized_table_structure_hash(observed),
                event_generation,
            });
            self.native_owners = config.rules.iter().map(|rule| rule.id).collect();
        }
        Ok(runtime_snapshot_from_table(
            config,
            &capability,
            after.as_ref(),
            self.baseline.as_ref(),
        ))
    }

    fn has_native_ownership(&self, config: &AgentPortForwardingConfig) -> bool {
        !config.rules.is_empty() || !self.native_owners.is_empty() || self.baseline.is_some()
    }

    async fn inspect_native(
        &mut self,
        config: &AgentPortForwardingConfig,
        cancel_token: CommandCancelToken,
    ) -> PortForwardRuntimeSnapshot {
        let capability = self.capability.clone();
        if !capability.supported() {
            return unsupported_snapshot(config, &capability);
        }
        let Some(nft) = resolve_nft_binary() else {
            return failed_snapshot(
                config,
                &capability,
                "nft_missing",
                "nft binary is no longer available",
            );
        };
        let event_generation_before = self.owned_table_event_generation;
        let mode = if self.monitor.is_some()
            && self.baseline.as_ref().is_some_and(|baseline| {
                baseline.event_generation == event_generation_before
                    && baseline.desired_hash == config.desired_hash
            }) {
            TableListMode::Terse
        } else {
            TableListMode::Full
        };
        match list_owned_table(&nft, cancel_token, mode).await {
            Ok(Some(table))
                if match mode {
                    TableListMode::Full => !table_is_owned(&table),
                    TableListMode::Terse => !table_has_ownership_declaration(&table),
                } =>
            {
                ownership_conflict_snapshot(config, &capability)
            }
            Ok(table) => {
                let snapshot = match mode {
                    TableListMode::Full => runtime_snapshot_from_table(
                        config,
                        &capability,
                        table.as_ref(),
                        self.baseline.as_ref(),
                    ),
                    TableListMode::Terse => runtime_snapshot_from_terse_table(
                        config,
                        &capability,
                        table.as_ref(),
                        self.baseline.as_ref(),
                    ),
                };
                self.native_owners = snapshot.rules.iter().map(|rule| rule.rule_id).collect();
                let event_generation_after = self.owned_table_event_generation;
                if mode == TableListMode::Full
                    && snapshot.status == PortForwardRuntimeStatus::Applied
                    && event_generation_before == event_generation_after
                {
                    if let Some(baseline) = self.baseline.as_mut() {
                        baseline.event_generation = event_generation_after;
                    }
                }
                snapshot
            }
            Err(error) => {
                failed_snapshot(config, &capability, "inspection_failed", &error.to_string())
            }
        }
    }
}

fn native_config(config: &AgentPortForwardingConfig) -> AgentPortForwardingConfig {
    let rules = config
        .rules
        .iter()
        .filter(|rule| rule.mode != PortForwardMode::CustomAdapter)
        .cloned()
        .collect::<Vec<_>>();
    AgentPortForwardingConfig {
        schema_version: if rules.iter().all(|rule| rule.mode == PortForwardMode::Dnat) {
            1
        } else {
            2
        },
        desired_hash: native_hash(&rules),
        rules,
        ..AgentPortForwardingConfig::default()
    }
}

fn custom_ownership_context(config: &AgentPortForwardingConfig) -> bool {
    !config.cleanup_rules.is_empty()
        || config
            .rules
            .iter()
            .any(|rule| rule.mode == PortForwardMode::CustomAdapter)
}

fn native_hash(rules: &[PortForwardRule]) -> String {
    if rules.is_empty() {
        String::new()
    } else {
        port_forwarding_desired_hash(rules)
    }
}

fn attach_native_stats(
    config: &AgentPortForwardingConfig,
    snapshot: &mut PortForwardRuntimeSnapshot,
) {
    for rule in &config.rules {
        let stat = if let Some(index) = snapshot
            .rules
            .iter()
            .position(|stat| stat.rule_id == rule.id)
        {
            &mut snapshot.rules[index]
        } else {
            snapshot
                .rules
                .push(adapters::runtime_stat(rule, snapshot.status));
            snapshot
                .rules
                .last_mut()
                .expect("inserted native runtime stat")
        };
        stat.mode = rule.mode;
        stat.status = Some(snapshot.status);
        stat.observed_unix = Some(snapshot.observed_unix);
        stat.error_code = snapshot.error_code.clone();
        stat.error_message = snapshot.error_message.clone();
    }
}

fn merge_snapshot(
    config: &AgentPortForwardingConfig,
    native: &AgentPortForwardingConfig,
    mut snapshot: PortForwardRuntimeSnapshot,
    custom: Vec<PortForwardRuleRuntimeStat>,
) -> PortForwardRuntimeSnapshot {
    if snapshot.owned_table_present.is_some()
        && matches!(
            snapshot.status,
            PortForwardRuntimeStatus::Applied | PortForwardRuntimeStatus::Absent
        )
    {
        snapshot.native_desired_hash = Some(native.desired_hash.clone());
    }
    // Empty desired state has always used None on the runtime wire. Keep the
    // separate native hash present when observed absence proves cleanup.
    snapshot.desired_hash = (!config.desired_hash.is_empty()).then(|| config.desired_hash.clone());
    for stat in custom {
        snapshot
            .rules
            .retain(|native| native.rule_id != stat.rule_id);
        snapshot.rules.push(stat);
    }
    snapshot.rules.sort_by_key(|rule| rule.rule_id);
    if snapshot.status == PortForwardRuntimeStatus::Failed
        || snapshot
            .rules
            .iter()
            .any(|rule| rule.status == Some(PortForwardRuntimeStatus::Failed))
    {
        snapshot.status = PortForwardRuntimeStatus::Failed;
    } else if snapshot.status == PortForwardRuntimeStatus::Drifted
        || snapshot
            .rules
            .iter()
            .any(|rule| rule.status == Some(PortForwardRuntimeStatus::Drifted))
    {
        snapshot.status = PortForwardRuntimeStatus::Drifted;
    } else if matches!(
        snapshot.status,
        PortForwardRuntimeStatus::Applied | PortForwardRuntimeStatus::Absent
    ) && !config.rules.is_empty()
        && snapshot
            .rules
            .iter()
            .all(|rule| rule.status == Some(PortForwardRuntimeStatus::Applied))
    {
        snapshot.status = PortForwardRuntimeStatus::Applied;
    }
    snapshot.observed_unix = unix_now();
    snapshot
}

pub(crate) fn render_apply_script(
    config: &AgentPortForwardingConfig,
    table_exists: bool,
) -> Result<String> {
    validate_port_forwarding_config(config)
        .map_err(|error| anyhow::anyhow!("invalid port-forwarding desired state: {error}"))?;
    let native = native_config(config);
    let config = &native;
    let mut script = String::new();
    if table_exists {
        script.push_str("delete table inet vpsman_port_forward\n");
    }
    if config.rules.is_empty() {
        return Ok(script);
    }

    script.push_str(&format!(
        "table inet vpsman_port_forward {{\n  comment \"{OWNED_TABLE_COMMENT_PREFIX}{}\"\n",
        config.desired_hash
    ));
    script.push_str(
        "  set vpsman_ownership_v1 {\n    type mark\n    elements = { 0x5650534d }\n  }\n",
    );
    if config.rules.iter().any(|rule| rule.masquerade) {
        script.push_str(
            "  set owned_flows {\n    typeof ct id\n    flags dynamic,timeout\n    timeout 2m\n    size 262144\n  }\n",
        );
    }

    render_dispatch_maps(&mut script, &config.rules);
    render_translation_maps(&mut script, &config.rules);

    for chain in ["prerouting", "output"] {
        script.push_str(&format!(
            "  chain {chain} {{\n    type nat hook {chain} priority -110; policy accept;\n    fib daddr type local jump pf_dispatch\n  }}\n"
        ));
    }
    render_dispatch_chain(&mut script, &config.rules);
    render_rule_chains(&mut script, &config.rules);
    if config.rules.iter().any(|rule| rule.masquerade) {
        script.push_str(
            "  chain postrouting {\n    type nat hook postrouting priority 90; policy accept;\n    ct id @owned_flows counter masquerade comment \"vpsman-owned-return\"\n  }\n",
        );
    }
    script.push_str("}\n");
    anyhow::ensure!(
        script.len() <= MAX_PORT_FORWARD_NFT_SCRIPT_BYTES,
        "rendered nftables program exceeds {} bytes",
        MAX_PORT_FORWARD_NFT_SCRIPT_BYTES
    );
    Ok(script)
}

fn render_dispatch_maps(script: &mut String, rules: &[PortForwardRule]) {
    for (nfproto, transport) in populated_dispatches(rules) {
        let map_name = dispatch_map_name(nfproto, transport);
        let _ = write!(
            script,
            "  map {map_name} {{\n    type inet_service : verdict\n    flags interval\n    elements = {{ "
        );
        let mut first = true;
        for (rule_index, rule) in rules.iter().enumerate() {
            if !rule_has_nfproto(rule, nfproto) || !rule.protocol.transports().contains(&transport)
            {
                continue;
            }
            for mapping in &rule.mappings {
                push_element_separator(script, &mut first);
                let incoming = render_port_range(mapping.incoming.start, mapping.incoming.end);
                let _ = write!(
                    script,
                    "{incoming} : jump {}",
                    rule_chain_name(rule_index, transport)
                );
            }
        }
        script.push_str(" }\n  }\n");
    }
}

fn render_translation_maps(script: &mut String, rules: &[PortForwardRule]) {
    for (rule_index, rule) in rules.iter().enumerate() {
        for transport in rule.protocol.transports() {
            if rule.mappings.iter().any(mapping_is_fixed) {
                let name = fixed_map_name(rule_index, transport);
                let _ = write!(
                    script,
                    "  map {name} {{\n    type inet_service : inet_service\n    flags interval\n    elements = {{ "
                );
                let mut first = true;
                for mapping in rule
                    .mappings
                    .iter()
                    .filter(|mapping| mapping_is_fixed(mapping))
                {
                    push_element_separator(script, &mut first);
                    let incoming = render_port_range(mapping.incoming.start, mapping.incoming.end);
                    let _ = write!(script, "{incoming} : {}", mapping.target.start);
                }
                script.push_str(" }\n  }\n");
            }

            if rule.mappings.iter().any(mapping_is_shifted) {
                let name = shifted_map_name(rule_index, transport);
                let _ = write!(
                    script,
                    "  map {name} {{\n    type inet_service : inet_service\n    elements = {{ "
                );
                let mut first = true;
                for mapping in rule
                    .mappings
                    .iter()
                    .filter(|mapping| mapping_is_shifted(mapping))
                {
                    for offset in 0..mapping.incoming.cardinality() {
                        push_element_separator(script, &mut first);
                        let incoming = u32::from(mapping.incoming.start) + offset;
                        let target = u32::from(mapping.target.start) + offset;
                        let _ = write!(script, "{incoming} : {target}");
                    }
                }
                script.push_str(" }\n  }\n");
            }
        }
    }
}

fn render_dispatch_chain(script: &mut String, rules: &[PortForwardRule]) {
    script.push_str("  chain pf_dispatch {\n");
    for (nfproto, transport) in populated_dispatches(rules) {
        let map_name = dispatch_map_name(nfproto, transport);
        let _ = writeln!(
            script,
            "    meta nfproto {nfproto} {transport} dport vmap @{map_name}"
        );
    }
    script.push_str("  }\n");
}

fn render_rule_chains(script: &mut String, rules: &[PortForwardRule]) {
    for (rule_index, rule) in rules.iter().enumerate() {
        for transport in rule.protocol.transports() {
            let chain_name = rule_chain_name(rule_index, transport);
            let track = if rule.masquerade {
                " add @owned_flows { ct id timeout 2m }"
            } else {
                ""
            };
            let _ = writeln!(
                script,
                "  chain {chain_name} {{\n    counter{track} comment \"vpsman-rule:{}:{}\"",
                rule.id, rule.revision
            );
            let (translation, destination) = match rule.mode {
                PortForwardMode::Dnat => {
                    let (family, ip) =
                        render_target_ip(rule.target_ip.expect("validated DNAT destination"));
                    (format!("dnat {family} to {ip}"), " :")
                }
                PortForwardMode::Redirect => ("redirect".to_string(), " to :"),
                PortForwardMode::CustomAdapter => {
                    unreachable!("custom rules are excluded from native rendering")
                }
            };

            let identity = rule
                .mappings
                .iter()
                .filter(|mapping| mapping_is_identity(mapping))
                .collect::<Vec<_>>();
            if !identity.is_empty() {
                script.push_str("    ");
                script.push_str(transport);
                script.push_str(" dport { ");
                for (index, mapping) in identity.iter().enumerate() {
                    if index != 0 {
                        script.push_str(", ");
                    }
                    script.push_str(&render_port_range(
                        mapping.incoming.start,
                        mapping.incoming.end,
                    ));
                }
                let _ = writeln!(script, " }} {translation}");
            }
            // Older nft userspace needs the transport match in the NAT
            // statement itself; it does not infer it from the dispatch jump.
            if rule.mappings.iter().any(mapping_is_fixed) {
                let name = fixed_map_name(rule_index, transport);
                let _ = writeln!(
                    script,
                    "    meta l4proto {transport} {translation}{destination} {transport} dport map @{name}"
                );
            }
            if rule.mappings.iter().any(mapping_is_shifted) {
                let name = shifted_map_name(rule_index, transport);
                let _ = writeln!(
                    script,
                    "    meta l4proto {transport} {translation}{destination} {transport} dport map @{name}"
                );
            }
            script.push_str("  }\n");
        }
    }
}

fn populated_dispatches(rules: &[PortForwardRule]) -> Vec<(&'static str, &'static str)> {
    let mut dispatches = Vec::new();
    for nfproto in ["ipv4", "ipv6"] {
        for transport in ["tcp", "udp"] {
            if rules.iter().any(|rule| {
                rule_has_nfproto(rule, nfproto) && rule.protocol.transports().contains(&transport)
            }) {
                dispatches.push((nfproto, transport));
            }
        }
    }
    dispatches
}

fn mapping_is_identity(mapping: &vpsman_common::PortForwardMapping) -> bool {
    // Omitting the port from DNAT preserves it exactly and avoids one map
    // element for every port in a same-to-same range.
    mapping.incoming == mapping.target
}

fn mapping_is_fixed(mapping: &vpsman_common::PortForwardMapping) -> bool {
    mapping.target.is_single() && !mapping_is_identity(mapping)
}

fn mapping_is_shifted(mapping: &vpsman_common::PortForwardMapping) -> bool {
    !mapping.target.is_single() && !mapping_is_identity(mapping)
}

fn rule_has_nfproto(rule: &PortForwardRule, nfproto: &str) -> bool {
    rule.native_families().contains(&if nfproto == "ipv4" {
        PortForwardAddressFamily::Ipv4
    } else {
        PortForwardAddressFamily::Ipv6
    })
}

fn render_target_ip(ip: IpAddr) -> (&'static str, String) {
    match ip {
        IpAddr::V4(ip) => ("ip", ip.to_string()),
        IpAddr::V6(ip) => ("ip6", format!("[{ip}]")),
    }
}

fn push_element_separator(script: &mut String, first: &mut bool) {
    if !*first {
        script.push_str(", ");
    }
    *first = false;
}

fn render_port_range(start: u16, end: u16) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    }
}

fn dispatch_map_name(nfproto: &str, transport: &str) -> String {
    format!("pf_dispatch_{nfproto}_{transport}")
}

fn rule_chain_name(rule_index: usize, transport: &str) -> String {
    format!("pf_rule_{rule_index}_{transport}")
}

fn fixed_map_name(rule_index: usize, transport: &str) -> String {
    format!("pf_{rule_index}_{transport}_fixed")
}

fn shifted_map_name(rule_index: usize, transport: &str) -> String {
    format!("pf_{rule_index}_{transport}_shift")
}

fn render_capability_probe_script() -> Result<String> {
    use vpsman_common::{pair_port_expressions, PortForwardProtocol};

    // Exercise the production grammar, including every mapping shape and both
    // families/transports. This is passed only to nft --check, never installed.
    let modes = [
        (PortForwardMode::Dnat, Some("192.0.2.1"), None),
        (PortForwardMode::Dnat, Some("2001:db8::1"), None),
        (
            PortForwardMode::Redirect,
            None,
            Some(PortForwardAddressFamily::Both),
        ),
    ];
    let mut rules = Vec::new();
    for (index, (mode, target, address_family)) in modes.into_iter().enumerate() {
        let port = 65000 + index * 10;
        rules.push(PortForwardRule {
            id: uuid::Uuid::from_u128(index as u128 + 1),
            revision: 1,
            name: format!("capability-{index}"),
            protocol: PortForwardProtocol::Both,
            target_ip: target.map(str::parse).transpose()?,
            mode,
            address_family,
            adapter: None,
            mappings: pair_port_expressions(
                &format!("{port},{},{}-{}", port + 2, port + 3, port + 4),
                &format!("{},{},{}-{}", port + 1, port + 2, port + 5, port + 6),
            )?,
            masquerade: mode == PortForwardMode::Dnat,
        });
    }
    let config = AgentPortForwardingConfig {
        schema_version: vpsman_common::PORT_FORWARDING_MODES_SCHEMA_VERSION,
        desired_hash: port_forwarding_desired_hash(&rules),
        rules,
        ..AgentPortForwardingConfig::default()
    };
    Ok(render_apply_script(&config, false)?.replace(
        OWNED_TABLE_NAME,
        &format!("vpsman_pf_probe_{}", std::process::id()),
    ))
}

async fn probe_port_forwarding_capability_inner() -> PortForwardCapability {
    let Some(nft) = resolve_nft_binary() else {
        return capability(
            PortForwardCapabilityStatus::NftMissing,
            None,
            "nft was not found in standard system paths or PATH",
        );
    };
    let version = nft_version(&nft).await;
    if !has_net_admin_capability() {
        return capability(
            PortForwardCapabilityStatus::InsufficientPrivilege,
            version,
            "agent requires root or CAP_NET_ADMIN in the host network namespace",
        );
    }
    let probe_script = match render_capability_probe_script() {
        Ok(script) => script,
        Err(error) => {
            return capability(
                PortForwardCapabilityStatus::ProbeFailed,
                version,
                &error.to_string(),
            );
        }
    };
    match run_nft_script(
        &nft,
        true,
        probe_script.into_bytes(),
        CommandCancelToken::default(),
    )
    .await
    {
        Ok(()) => PortForwardCapability {
            status: PortForwardCapabilityStatus::Supported,
            nft_version: version,
            reason: None,
            ..PortForwardCapability::default()
        },
        Err(error) => {
            let message = error.to_string();
            let status = if message.contains("Operation not permitted")
                || message.contains("Permission denied")
            {
                PortForwardCapabilityStatus::InsufficientPrivilege
            } else if message.contains("not supported")
                || message.contains("No such file or directory")
                || message.contains("unknown keyword")
            {
                PortForwardCapabilityStatus::InetNatUnsupported
            } else {
                PortForwardCapabilityStatus::ProbeFailed
            };
            capability(status, version, &message)
        }
    }
}

struct NftMonitorConsumer {
    child: Child,
    lines: tokio::io::Lines<BufReader<ChildStdout>>,
}

enum NftMonitorEvent {
    Line(String),
    Stopped,
}

fn start_nft_monitor(path: &Path) -> Option<NftMonitorConsumer> {
    let mut command = Command::new(path);
    command
        .arg("monitor")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return None,
    };
    let stdout = child.stdout.take()?;
    Some(NftMonitorConsumer {
        child,
        lines: BufReader::new(stdout).lines(),
    })
}

async fn next_monitor_event(monitor: &mut Option<NftMonitorConsumer>) -> NftMonitorEvent {
    let Some(monitor) = monitor.as_mut() else {
        std::future::pending::<()>().await;
        unreachable!("pending monitor future returned")
    };
    match monitor.lines.next_line().await {
        Ok(Some(line)) => NftMonitorEvent::Line(line),
        Ok(None) | Err(_) => {
            let _ = monitor.child.wait().await;
            NftMonitorEvent::Stopped
        }
    }
}

fn nft_event_invalidates_owned_table(line: &str) -> bool {
    line.contains(OWNED_TABLE_NAME) && !line.contains(OWNED_FLOW_SET_NAME)
}

async fn nft_version(path: &Path) -> Option<String> {
    let mut command = Command::new(path);
    command.arg("--version").stdin(Stdio::null());
    match run_child_with_bounded_output(command, 5, 4096, ChildCleanupPolicy::DirectChild).await {
        Ok(ChildRunResult::Completed(output)) if output.exit_code == Some(0) => {
            String::from_utf8(output.stdout)
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        }
        _ => None,
    }
}

async fn run_nft_script(
    path: &Path,
    check: bool,
    script: Vec<u8>,
    cancel_token: CommandCancelToken,
) -> Result<()> {
    anyhow::ensure!(
        script.len() <= MAX_PORT_FORWARD_NFT_SCRIPT_BYTES,
        "nftables program exceeds safe input limit"
    );
    let mut command = Command::new(path);
    if check {
        command.arg("--check");
    }
    command.args(["--file", "-"]);
    match run_child_with_input_bounded_output_cancelable(
        command,
        script,
        NFT_TIMEOUT_SECS,
        NFT_OUTPUT_LIMIT,
        ChildCleanupPolicy::DirectChild,
        cancel_token,
    )
    .await?
    {
        ChildRunResult::Completed(output) if output.exit_code == Some(0) => Ok(()),
        ChildRunResult::Completed(output) => anyhow::bail!(
            "nft exited with {:?}: {}",
            output.exit_code,
            String::from_utf8_lossy(&output.stderr).trim()
        ),
        ChildRunResult::TimedOut(_) => anyhow::bail!("nft command timed out"),
        ChildRunResult::Canceled { reason, .. } => anyhow::bail!("nft command canceled: {reason}"),
    }
}

async fn list_owned_table(
    path: &Path,
    cancel_token: CommandCancelToken,
    mode: TableListMode,
) -> Result<Option<Value>> {
    let mut command = Command::new(path);
    if mode == TableListMode::Terse {
        command.arg("--terse");
    }
    command
        .args([
            "--json",
            "--numeric",
            "list",
            "table",
            OWNED_TABLE_FAMILY,
            OWNED_TABLE_NAME,
        ])
        .stdin(Stdio::null());
    match crate::child_process::run_child_with_bounded_output_cancelable(
        command,
        NFT_TIMEOUT_SECS,
        NFT_OUTPUT_LIMIT,
        ChildCleanupPolicy::DirectChild,
        cancel_token,
    )
    .await?
    {
        ChildRunResult::Completed(output) if output.exit_code == Some(0) => {
            let value = serde_json::from_slice(&output.stdout)
                .context("nft returned malformed JSON for owned table")?;
            Ok(Some(value))
        }
        ChildRunResult::Completed(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("No such file or directory")
                || stderr.contains("does not exist")
                || stderr.contains("No such file")
            {
                Ok(None)
            } else {
                anyhow::bail!(
                    "failed to inspect owned nftables table (exit {:?}): {}",
                    output.exit_code,
                    stderr.trim()
                )
            }
        }
        ChildRunResult::TimedOut(_) => anyhow::bail!("owned nftables table inspection timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("owned nftables table inspection canceled: {reason}")
        }
    }
}

fn runtime_snapshot_from_table(
    config: &AgentPortForwardingConfig,
    capability: &PortForwardCapability,
    table: Option<&Value>,
    baseline: Option<&AppliedBaseline>,
) -> PortForwardRuntimeSnapshot {
    let expected_rules = !config.rules.is_empty();
    let (status, observed_hash) = match table {
        None if expected_rules => (PortForwardRuntimeStatus::Drifted, None),
        None => (PortForwardRuntimeStatus::Absent, None),
        Some(value) if !expected_rules => (
            PortForwardRuntimeStatus::Drifted,
            Some(normalized_table_hash(value)),
        ),
        Some(value) => {
            let hash = normalized_table_hash(value);
            let applied = baseline.is_some_and(|baseline| {
                baseline.desired_hash == config.desired_hash && baseline.observed_hash == hash
            });
            (
                if applied {
                    PortForwardRuntimeStatus::Applied
                } else {
                    PortForwardRuntimeStatus::Drifted
                },
                Some(hash),
            )
        }
    };
    PortForwardRuntimeSnapshot {
        status,
        owned_table_present: Some(table.is_some()),
        desired_hash: (!config.desired_hash.is_empty()).then(|| config.desired_hash.clone()),
        observed_hash,
        nft_version: capability.nft_version.clone(),
        ipv4_forwarding_enabled: read_forwarding_flag("/proc/sys/net/ipv4/ip_forward"),
        ipv6_forwarding_enabled: read_forwarding_flag("/proc/sys/net/ipv6/conf/all/forwarding"),
        rules: table.map(extract_rule_counters).unwrap_or_default(),
        error_code: None,
        error_message: None,
        observed_unix: unix_now(),
        ..PortForwardRuntimeSnapshot::default()
    }
}

fn runtime_snapshot_from_terse_table(
    config: &AgentPortForwardingConfig,
    capability: &PortForwardCapability,
    table: Option<&Value>,
    baseline: Option<&AppliedBaseline>,
) -> PortForwardRuntimeSnapshot {
    let expected_rules = !config.rules.is_empty();
    let (status, observed_hash) = match table {
        None if expected_rules => (PortForwardRuntimeStatus::Drifted, None),
        None => (PortForwardRuntimeStatus::Absent, None),
        Some(value) if !expected_rules => (
            PortForwardRuntimeStatus::Drifted,
            Some(normalized_table_structure_hash(value)),
        ),
        Some(value) => {
            let structure_hash = normalized_table_structure_hash(value);
            let applied = baseline.is_some_and(|baseline| {
                baseline.desired_hash == config.desired_hash
                    && baseline.structure_hash == structure_hash
            });
            (
                if applied {
                    PortForwardRuntimeStatus::Applied
                } else {
                    PortForwardRuntimeStatus::Drifted
                },
                baseline
                    .filter(|_| applied)
                    .map(|baseline| baseline.observed_hash.clone())
                    .or(Some(structure_hash)),
            )
        }
    };
    PortForwardRuntimeSnapshot {
        status,
        owned_table_present: Some(table.is_some()),
        desired_hash: (!config.desired_hash.is_empty()).then(|| config.desired_hash.clone()),
        observed_hash,
        nft_version: capability.nft_version.clone(),
        ipv4_forwarding_enabled: read_forwarding_flag("/proc/sys/net/ipv4/ip_forward"),
        ipv6_forwarding_enabled: read_forwarding_flag("/proc/sys/net/ipv6/conf/all/forwarding"),
        rules: table.map(extract_rule_counters).unwrap_or_default(),
        error_code: None,
        error_message: None,
        observed_unix: unix_now(),
        ..PortForwardRuntimeSnapshot::default()
    }
}

fn unsupported_snapshot(
    config: &AgentPortForwardingConfig,
    capability: &PortForwardCapability,
) -> PortForwardRuntimeSnapshot {
    PortForwardRuntimeSnapshot {
        status: PortForwardRuntimeStatus::Unsupported,
        desired_hash: (!config.desired_hash.is_empty()).then(|| config.desired_hash.clone()),
        nft_version: capability.nft_version.clone(),
        error_code: Some(capability_status_code(capability.status).to_string()),
        error_message: capability.reason.clone(),
        observed_unix: unix_now(),
        ..PortForwardRuntimeSnapshot::default()
    }
}

fn failed_snapshot(
    config: &AgentPortForwardingConfig,
    capability: &PortForwardCapability,
    code: &str,
    message: &str,
) -> PortForwardRuntimeSnapshot {
    PortForwardRuntimeSnapshot {
        status: PortForwardRuntimeStatus::Failed,
        desired_hash: (!config.desired_hash.is_empty()).then(|| config.desired_hash.clone()),
        nft_version: capability.nft_version.clone(),
        error_code: Some(code.to_string()),
        error_message: Some(message.chars().take(1024).collect()),
        observed_unix: unix_now(),
        ..PortForwardRuntimeSnapshot::default()
    }
}

fn ownership_conflict_snapshot(
    config: &AgentPortForwardingConfig,
    capability: &PortForwardCapability,
) -> PortForwardRuntimeSnapshot {
    PortForwardRuntimeSnapshot {
        status: PortForwardRuntimeStatus::Failed,
        owned_table_present: Some(false),
        desired_hash: (!config.desired_hash.is_empty()).then(|| config.desired_hash.clone()),
        nft_version: capability.nft_version.clone(),
        ipv4_forwarding_enabled: read_forwarding_flag("/proc/sys/net/ipv4/ip_forward"),
        ipv6_forwarding_enabled: read_forwarding_flag("/proc/sys/net/ipv6/conf/all/forwarding"),
        error_code: Some("table_ownership_conflict".to_string()),
        error_message: Some(format!(
            "table {OWNED_TABLE_FAMILY} {OWNED_TABLE_NAME} exists without the vpsman ownership marker; it was left unchanged"
        )),
        observed_unix: unix_now(),
        ..PortForwardRuntimeSnapshot::default()
    }
}

fn table_is_owned(value: &Value) -> bool {
    let mut table_present = false;
    let mut marker_present = false;
    visit_json(value, &mut |object| {
        if let Some(table) = object.get("table").and_then(Value::as_object) {
            table_present |= table.get("family").and_then(Value::as_str)
                == Some(OWNED_TABLE_FAMILY)
                && table.get("name").and_then(Value::as_str) == Some(OWNED_TABLE_NAME);
        }
        if let Some(set) = object.get("set").and_then(Value::as_object) {
            marker_present |= ownership_set_matches(set);
        }
    });
    table_present && marker_present
}

fn table_has_ownership_declaration(value: &Value) -> bool {
    let mut table_present = false;
    let mut marker_present = false;
    visit_json(value, &mut |object| {
        if let Some(table) = object.get("table").and_then(Value::as_object) {
            table_present |= table.get("family").and_then(Value::as_str)
                == Some(OWNED_TABLE_FAMILY)
                && table.get("name").and_then(Value::as_str) == Some(OWNED_TABLE_NAME);
        }
        if let Some(set) = object.get("set").and_then(Value::as_object) {
            marker_present |= set.get("family").and_then(Value::as_str) == Some(OWNED_TABLE_FAMILY)
                && set.get("table").and_then(Value::as_str) == Some(OWNED_TABLE_NAME)
                && set.get("name").and_then(Value::as_str) == Some(OWNERSHIP_SET_NAME)
                && set.get("type").and_then(Value::as_str) == Some("mark");
        }
    });
    table_present && marker_present
}

fn ownership_set_matches(set: &serde_json::Map<String, Value>) -> bool {
    set.get("family").and_then(Value::as_str) == Some(OWNED_TABLE_FAMILY)
        && set.get("table").and_then(Value::as_str) == Some(OWNED_TABLE_NAME)
        && set.get("name").and_then(Value::as_str) == Some(OWNERSHIP_SET_NAME)
        && set.get("type").and_then(Value::as_str) == Some("mark")
        && set
            .get("elem")
            .and_then(Value::as_array)
            .is_some_and(|elements| elements.iter().any(ownership_mark_matches))
}

fn ownership_mark_matches(value: &Value) -> bool {
    value.as_u64() == Some(OWNERSHIP_MARK)
        || value.as_str().is_some_and(|value| {
            value == OWNERSHIP_MARK.to_string()
                || value.eq_ignore_ascii_case(&format!("0x{OWNERSHIP_MARK:x}"))
        })
}

fn normalized_table_hash(value: &Value) -> String {
    payload_hash(&serde_json::to_vec(&normalize_json(value)).unwrap_or_default())
}

fn normalized_table_structure_hash(value: &Value) -> String {
    payload_hash(&serde_json::to_vec(&normalize_json_structure(value)).unwrap_or_default())
}

fn normalize_json(value: &Value) -> Value {
    normalize_json_inner(value, false, false).unwrap_or(Value::Null)
}

fn normalize_json_structure(value: &Value) -> Value {
    normalize_json_inner(value, false, true).unwrap_or(Value::Null)
}

fn normalize_json_inner(
    value: &Value,
    in_owned_flow_set: bool,
    omit_static_elements: bool,
) -> Option<Value> {
    match value {
        Value::Object(object) => {
            if omit_static_elements && object.contains_key("element") {
                return None;
            }
            if object
                .get("element")
                .and_then(Value::as_object)
                .is_some_and(|element| {
                    element.get("family").and_then(Value::as_str) == Some(OWNED_TABLE_FAMILY)
                        && element.get("table").and_then(Value::as_str) == Some(OWNED_TABLE_NAME)
                        && element.get("name").and_then(Value::as_str) == Some(OWNED_FLOW_SET_NAME)
                })
            {
                return None;
            }
            let normalized = object
                .iter()
                .filter(|(key, _)| {
                    !matches!(key.as_str(), "handle" | "packets" | "bytes")
                        && !(in_owned_flow_set && key.as_str() == "elem")
                        && !(omit_static_elements && key.as_str() == "elem")
                })
                .filter_map(|(key, value)| {
                    let nested_owned_flow_set = key == "set"
                        && value
                            .as_object()
                            .and_then(|set| set.get("name"))
                            .and_then(Value::as_str)
                            == Some(OWNED_FLOW_SET_NAME);
                    normalize_json_inner(value, nested_owned_flow_set, omit_static_elements)
                        .map(|value| (key.clone(), value))
                })
                .collect::<BTreeMap<_, _>>();
            Some(serde_json::to_value(normalized).unwrap_or(Value::Null))
        }
        Value::Array(items) => Some(Value::Array(
            items
                .iter()
                .filter(|item| item.get("metainfo").is_none())
                .filter_map(|item| {
                    normalize_json_inner(item, in_owned_flow_set, omit_static_elements)
                })
                .collect(),
        )),
        _ => Some(value.clone()),
    }
}

fn extract_rule_counters(value: &Value) -> Vec<PortForwardRuleRuntimeStat> {
    let mut counters = HashMap::<(uuid::Uuid, i64), u64>::new();
    visit_json(value, &mut |object| {
        let Some(rule) = object.get("rule").and_then(Value::as_object) else {
            return;
        };
        let Some(comment) = rule.get("comment").and_then(Value::as_str) else {
            return;
        };
        let Some((id, revision)) = parse_rule_comment(comment) else {
            return;
        };
        let packets = rule
            .get("expr")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|expression| expression.get("counter"))
            .filter_map(|counter| counter.get("packets"))
            .filter_map(Value::as_u64)
            .sum::<u64>();
        *counters.entry((id, revision)).or_default() += packets;
    });
    let mut result = counters
        .into_iter()
        .map(
            |((rule_id, revision), nat_matches)| PortForwardRuleRuntimeStat {
                rule_id,
                revision,
                nat_matches: Some(nat_matches),
                mode: PortForwardMode::Dnat,
                status: None,
                observed_unix: None,
                error_code: None,
                error_message: None,
            },
        )
        .collect::<Vec<_>>();
    result.sort_by_key(|stat| stat.rule_id);
    result
}

fn visit_json(value: &Value, visitor: &mut impl FnMut(&serde_json::Map<String, Value>)) {
    match value {
        Value::Object(object) => {
            visitor(object);
            for nested in object.values() {
                visit_json(nested, visitor);
            }
        }
        Value::Array(items) => {
            for nested in items {
                visit_json(nested, visitor);
            }
        }
        _ => {}
    }
}

fn parse_rule_comment(comment: &str) -> Option<(uuid::Uuid, i64)> {
    let value = comment.strip_prefix("vpsman-rule:")?;
    let (id, revision) = value.rsplit_once(':')?;
    Some((id.parse().ok()?, revision.parse().ok()?))
}

fn resolve_nft_binary() -> Option<PathBuf> {
    ["/usr/sbin/nft", "/sbin/nft", "/usr/bin/nft"]
        .into_iter()
        .map(PathBuf::from)
        .chain(env::var_os("PATH").into_iter().flat_map(|paths| {
            env::split_paths(&paths)
                .map(|path| path.join("nft"))
                .collect::<Vec<_>>()
        }))
        .find(|path| {
            path.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn has_net_admin_capability() -> bool {
    if unsafe { libc::geteuid() } == 0 {
        return true;
    }
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:\t"))
                .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
        })
        .is_some_and(|capabilities| capabilities & (1_u64 << 12) != 0)
}

fn read_forwarding_flag(path: &str) -> Option<bool> {
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim() == "1")
}

fn capability(
    status: PortForwardCapabilityStatus,
    nft_version: Option<String>,
    reason: &str,
) -> PortForwardCapability {
    PortForwardCapability {
        status,
        nft_version,
        reason: Some(reason.chars().take(1024).collect()),
        ..PortForwardCapability::default()
    }
}

fn capability_status_code(status: PortForwardCapabilityStatus) -> &'static str {
    match status {
        PortForwardCapabilityStatus::Supported => "supported",
        PortForwardCapabilityStatus::NftMissing => "nft_missing",
        PortForwardCapabilityStatus::InsufficientPrivilege => "insufficient_privilege",
        PortForwardCapabilityStatus::InetNatUnsupported => "inet_nat_unsupported",
        PortForwardCapabilityStatus::ProbeFailed => "probe_failed",
        PortForwardCapabilityStatus::Unknown => "unknown",
    }
}

#[cfg(test)]
#[path = "tests_port_forwarding.rs"]
mod tests;
