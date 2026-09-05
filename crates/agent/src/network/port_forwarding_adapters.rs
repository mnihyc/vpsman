use super::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vpsman_common::{
    ensure_private_dir_async, write_private_file_atomically_async, PortForwardCleanupRule,
    RuntimeTunnelCommand,
};

/// Last cleanup owner, saved before Apply so interrupted/partial applies remain removable.
/// Remove commands are required to address all resources by the stable rule ID.
#[derive(Clone, Serialize, Deserialize)]
struct OwnedRule {
    client_id: String,
    rule: PortForwardRule,
}

#[derive(Default, Serialize, Deserialize)]
struct InventoryRecord {
    owned: BTreeMap<Uuid, OwnedRule>,
    removed_rules: BTreeMap<Uuid, i64>,
}

pub(super) struct AdapterInventory {
    client_id: String,
    root: Option<PathBuf>,
    loaded: bool,
    owned: BTreeMap<Uuid, OwnedRule>,
    removed_rules: BTreeMap<Uuid, i64>,
    cleanup_requests: BTreeMap<Uuid, i64>,
}

impl AdapterInventory {
    pub(super) fn new(client_id: String) -> Self {
        Self {
            client_id,
            root: None,
            loaded: false,
            owned: BTreeMap::new(),
            removed_rules: BTreeMap::new(),
            cleanup_requests: BTreeMap::new(),
        }
    }

    pub(super) async fn load(&mut self) -> Result<()> {
        if self.loaded {
            return Ok(());
        }
        let root = match &self.root {
            Some(root) => root.clone(),
            None => crate::state_dir::agent_state_dir()?.join("port-forwarding"),
        };
        let record: InventoryRecord = match tokio::fs::read(root.join("ownership.json")).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .context("invalid port-forward adapter ownership inventory")?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                InventoryRecord::default()
            }
            Err(error) => {
                return Err(error).context("cannot read port-forward adapter ownership inventory")
            }
        };
        self.owned = record.owned;
        self.removed_rules = record.removed_rules;
        self.root = Some(root);
        self.loaded = true;
        Ok(())
    }

    async fn store(
        &mut self,
        owned: BTreeMap<Uuid, OwnedRule>,
        removed_rules: BTreeMap<Uuid, i64>,
    ) -> Result<()> {
        let root = self
            .root
            .as_ref()
            .context("adapter inventory is not loaded")?;
        ensure_private_dir_async(root).await?;
        let record = InventoryRecord {
            owned,
            removed_rules,
        };
        write_private_file_atomically_async(
            &root.join("ownership.json"),
            &serde_json::to_vec(&record)?,
        )
        .await?;
        self.owned = record.owned;
        self.removed_rules = record.removed_rules;
        Ok(())
    }

    pub(super) async fn synchronize_cleanup_requests(
        &mut self,
        rules: &[PortForwardCleanupRule],
    ) -> Result<()> {
        self.cleanup_requests = rules
            .iter()
            .map(|rule| (rule.rule_id, rule.revision))
            .collect();
        // Ownership is committed before every Apply and removed only after verified
        // absence. An absent owner therefore also covers a never-applied rule.
        let receipts = self
            .cleanup_requests
            .iter()
            .filter(|(id, _)| !self.owned.contains_key(id))
            .map(|(id, revision)| (*id, *revision))
            .collect::<BTreeMap<_, _>>();
        if receipts != self.removed_rules {
            self.store(self.owned.clone(), receipts).await?;
        }
        Ok(())
    }

    pub(super) fn removed_rules(&self) -> Vec<PortForwardCleanupRule> {
        self.removed_rules
            .iter()
            .map(|(rule_id, revision)| PortForwardCleanupRule {
                rule_id: *rule_id,
                revision: *revision,
            })
            .collect()
    }

    pub(super) fn retains_owner(&self, id: Uuid) -> bool {
        self.owned.contains_key(&id)
    }

    pub(super) async fn remove_replaced(
        &mut self,
        config: &AgentPortForwardingConfig,
        cancel: CommandCancelToken,
    ) -> (Vec<Uuid>, Vec<PortForwardRuleRuntimeStat>) {
        let stale = self
            .owned
            .values()
            .filter(|entry| {
                !config.rules.iter().any(|rule| {
                    rule.id == entry.rule.id
                        && rule.mode == PortForwardMode::CustomAdapter
                        && same_adapter(rule, &entry.rule)
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut removed = Vec::new();
        let mut failed = Vec::new();
        for entry in stale {
            if cancel.is_canceled() {
                break;
            }
            match self.remove(&entry, cancel.clone()).await {
                Ok(()) => removed.push(entry.rule.id),
                Err(error) => failed.push(cleanup_error_stat(
                    config,
                    &entry.rule,
                    "adapter_remove_failed",
                    &error.to_string(),
                )),
            }
        }
        (removed, failed)
    }

    async fn remove(&mut self, entry: &OwnedRule, cancel: CommandCancelToken) -> Result<()> {
        let adapter = entry
            .rule
            .adapter
            .as_ref()
            .context("saved custom owner has no adapter")?;
        run(&adapter.remove, entry, cancel.clone(), "remove").await?;
        let status = observe(entry, cancel).await?;
        anyhow::ensure!(
            status.state == AdapterState::Absent,
            "adapter removal verification: {}",
            status.description()
        );
        let mut remaining = self.owned.clone();
        remaining.remove(&entry.rule.id);
        let mut receipts = self.removed_rules.clone();
        if let Some(revision) = self.cleanup_requests.get(&entry.rule.id) {
            receipts.insert(entry.rule.id, *revision);
        }
        self.store(remaining, receipts).await
    }

    pub(super) async fn apply(
        &mut self,
        rule: &PortForwardRule,
        cancel: CommandCancelToken,
    ) -> PortForwardRuleRuntimeStat {
        let entry = OwnedRule {
            client_id: self.client_id.clone(),
            rule: rule.clone(),
        };
        let result = async {
            let adapter = rule
                .adapter
                .as_ref()
                .context("custom rule has no adapter")?;
            let mut owners = self.owned.clone();
            owners.insert(rule.id, entry.clone());
            let mut receipts = self.removed_rules.clone();
            receipts.remove(&rule.id);
            self.store(owners, receipts).await?;
            run(&adapter.apply, &entry, cancel.clone(), "apply").await?;
            let status = observe(&entry, cancel).await?;
            anyhow::ensure!(
                status.state == AdapterState::Applied,
                "adapter apply verification: {}",
                status.description()
            );
            Ok::<_, anyhow::Error>(())
        }
        .await;
        match result {
            Ok(()) => runtime_stat(rule, PortForwardRuntimeStatus::Applied),
            Err(error) => error_stat(rule, "adapter_apply_failed", &error.to_string()),
        }
    }

    pub(super) async fn inspect(
        &self,
        config: &AgentPortForwardingConfig,
        cancel_token: CommandCancelToken,
    ) -> Vec<PortForwardRuleRuntimeStat> {
        let mut stats = Vec::new();
        for rule in config
            .rules
            .iter()
            .filter(|rule| rule.mode == PortForwardMode::CustomAdapter)
        {
            if cancel_token.is_canceled() {
                break;
            }
            if self
                .owned
                .get(&rule.id)
                .is_some_and(|entry| !same_adapter(rule, &entry.rule))
            {
                continue;
            }
            // Inspect the requested inputs, while removal keeps its independently saved owner.
            let entry = OwnedRule {
                client_id: self.client_id.clone(),
                rule: rule.clone(),
            };
            match observe(&entry, cancel_token.clone()).await {
                Ok(status) => {
                    let mut stat = runtime_stat(
                        rule,
                        if status.state == AdapterState::Applied {
                            PortForwardRuntimeStatus::Applied
                        } else {
                            PortForwardRuntimeStatus::Drifted
                        },
                    );
                    if status.state != AdapterState::Applied {
                        stat.error_message = status.message;
                    }
                    stats.push(stat);
                }
                Err(error) => stats.push(error_stat(
                    rule,
                    "adapter_status_failed",
                    &error.to_string(),
                )),
            }
        }
        stats
    }

    pub(super) fn cleanup_failures(
        &self,
        config: &AgentPortForwardingConfig,
    ) -> Vec<PortForwardRuleRuntimeStat> {
        self.owned
            .values()
            .filter(|entry| {
                !config.rules.iter().any(|rule| {
                    rule.id == entry.rule.id
                        && rule.mode == PortForwardMode::CustomAdapter
                        && same_adapter(rule, &entry.rule)
                })
            })
            .map(|entry| {
                cleanup_error_stat(
                    config,
                    &entry.rule,
                    "adapter_cleanup_pending",
                    "saved adapter ownership still requires removal",
                )
            })
            .collect()
    }
}

fn same_adapter(left: &PortForwardRule, right: &PortForwardRule) -> bool {
    left.adapter
        .as_ref()
        .map(|adapter| (adapter.definition_id, &adapter.definition_hash))
        == right
            .adapter
            .as_ref()
            .map(|adapter| (adapter.definition_id, &adapter.definition_hash))
}

fn cleanup_error_stat(
    config: &AgentPortForwardingConfig,
    saved: &PortForwardRule,
    code: &str,
    message: &str,
) -> PortForwardRuleRuntimeStat {
    let mut rule = config
        .rules
        .iter()
        .find(|rule| rule.id == saved.id)
        .unwrap_or(saved)
        .clone();
    if let Some(request) = config
        .cleanup_rules
        .iter()
        .find(|request| request.rule_id == rule.id)
    {
        rule.revision = request.revision;
    }
    error_stat(&rule, code, message)
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum AdapterState {
    Applied,
    Absent,
    Drifted,
}

#[derive(Deserialize)]
struct AdapterObservation {
    state: AdapterState,
    #[serde(default)]
    message: Option<String>,
}

impl AdapterObservation {
    fn description(&self) -> String {
        format!(
            "{:?}{}",
            self.state,
            self.message
                .as_ref()
                .map(|m| format!(": {m}"))
                .unwrap_or_default()
        )
    }
}

async fn observe(entry: &OwnedRule, cancel: CommandCancelToken) -> Result<AdapterObservation> {
    let adapter = entry
        .rule
        .adapter
        .as_ref()
        .context("custom rule has no adapter")?;
    let bytes = run(&adapter.status, entry, cancel, "status").await?;
    serde_json::from_slice(&bytes)
        .context("adapter status must return JSON state applied, absent, or drifted")
}

fn render(command: &RuntimeTunnelCommand, entry: &OwnedRule) -> Result<Vec<String>> {
    anyhow::ensure!(
        !command.argv.is_empty() && !command.argv[0].trim().is_empty(),
        "adapter command requires an executable"
    );
    let rule = &entry.rule;
    let ports = |incoming| {
        rule.mappings
            .iter()
            .map(|mapping| {
                let range = if incoming {
                    mapping.incoming
                } else {
                    mapping.target
                };
                render_port_range(range.start, range.end)
            })
            .collect::<Vec<_>>()
            .join(",")
    };
    let placeholders = [
        ("{rule_id}", rule.id.to_string()),
        ("{client_id}", entry.client_id.clone()),
        ("{revision}", rule.revision.to_string()),
        (
            "{protocol}",
            match rule.protocol {
                vpsman_common::PortForwardProtocol::Tcp => "tcp",
                vpsman_common::PortForwardProtocol::Udp => "udp",
                vpsman_common::PortForwardProtocol::Both => "both",
            }
            .to_string(),
        ),
        ("{incoming_ports}", ports(true)),
        ("{target_ports}", ports(false)),
        (
            "{target_ip}",
            rule.target_ip.map(|ip| ip.to_string()).unwrap_or_default(),
        ),
    ];
    Ok(command
        .argv
        .iter()
        .map(|arg| {
            let mut rendered = String::with_capacity(arg.len());
            let mut remaining = arg.as_str();
            while !remaining.is_empty() {
                if let Some((key, value)) = placeholders
                    .iter()
                    .find(|(key, _)| remaining.starts_with(key))
                {
                    rendered.push_str(value);
                    remaining = &remaining[key.len()..];
                } else {
                    let character = remaining
                        .chars()
                        .next()
                        .expect("nonempty argument remainder");
                    rendered.push(character);
                    remaining = &remaining[character.len_utf8()..];
                }
            }
            rendered
        })
        .collect())
}

async fn run(
    command: &RuntimeTunnelCommand,
    entry: &OwnedRule,
    cancel: CommandCancelToken,
    phase: &str,
) -> Result<Vec<u8>> {
    let argv = render(command, entry)?;
    let mut child = Command::new(&argv[0]);
    child.args(&argv[1..]).stdin(Stdio::null());
    let result = crate::child_process::run_child_with_bounded_output_cancelable(
        child,
        command.max_timeout_secs,
        usize::try_from(command.max_output_bytes)?,
        ChildCleanupPolicy::ProcessGroup,
        cancel,
    )
    .await
    .with_context(|| format!("failed to execute adapter {phase}"))?;
    let output = match result {
        ChildRunResult::Completed(output) => output,
        ChildRunResult::TimedOut(_) => anyhow::bail!("adapter {phase} timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("adapter {phase} canceled: {reason}")
        }
    };
    anyhow::ensure!(
        !output.stdout_truncated && !output.stderr_truncated,
        "adapter {phase} exceeded its configured output budget"
    );
    anyhow::ensure!(
        output.exit_code == Some(0),
        "adapter {phase} exited {:?}: {} {}",
        output.exit_code,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(output.stdout)
}

pub(super) fn runtime_stat(
    rule: &PortForwardRule,
    status: PortForwardRuntimeStatus,
) -> PortForwardRuleRuntimeStat {
    PortForwardRuleRuntimeStat {
        rule_id: rule.id,
        revision: rule.revision,
        nat_matches: None,
        mode: rule.mode,
        status: Some(status),
        observed_unix: Some(unix_now()),
        error_code: None,
        error_message: None,
    }
}

pub(super) fn error_stat(
    rule: &PortForwardRule,
    code: &str,
    message: &str,
) -> PortForwardRuleRuntimeStat {
    let mut stat = runtime_stat(rule, PortForwardRuntimeStatus::Failed);
    stat.error_code = Some(code.to_string());
    stat.error_message = Some(message.chars().take(1024).collect());
    stat
}

#[cfg(test)]
#[path = "tests_port_forwarding_adapters.rs"]
mod tests;
