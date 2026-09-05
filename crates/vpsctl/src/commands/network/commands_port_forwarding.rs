use std::net::IpAddr;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use uuid::Uuid;
use vpsman_common::{
    pair_port_expressions, PortForwardAddressFamily, PortForwardMode, PortForwardProtocol,
};

use crate::http::{http_get, http_post_json, http_put_json};

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum PortForwardProtocolArg {
    Tcp,
    Udp,
    Both,
}

impl From<PortForwardProtocolArg> for PortForwardProtocol {
    fn from(value: PortForwardProtocolArg) -> Self {
        match value {
            PortForwardProtocolArg::Tcp => Self::Tcp,
            PortForwardProtocolArg::Udp => Self::Udp,
            PortForwardProtocolArg::Both => Self::Both,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum PortForwardModeArg {
    #[default]
    Dnat,
    Redirect,
    CustomAdapter,
}

impl From<PortForwardModeArg> for PortForwardMode {
    fn from(value: PortForwardModeArg) -> Self {
        match value {
            PortForwardModeArg::Dnat => Self::Dnat,
            PortForwardModeArg::Redirect => Self::Redirect,
            PortForwardModeArg::CustomAdapter => Self::CustomAdapter,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum PortForwardFamilyArg {
    Ipv4,
    Ipv6,
    Both,
}

impl From<PortForwardFamilyArg> for PortForwardAddressFamily {
    fn from(value: PortForwardFamilyArg) -> Self {
        match value {
            PortForwardFamilyArg::Ipv4 => Self::Ipv4,
            PortForwardFamilyArg::Ipv6 => Self::Ipv6,
            PortForwardFamilyArg::Both => Self::Both,
        }
    }
}

#[derive(Debug, Default, Args)]
pub(crate) struct PortForwardModeArgs {
    #[arg(long, value_enum, default_value = "dnat")]
    pub(crate) mode: PortForwardModeArg,
    #[arg(
        long,
        value_enum,
        help = "REDIRECT family (default ipv4); custom adapters own family selection"
    )]
    pub(crate) address_family: Option<PortForwardFamilyArg>,
    #[arg(
        long,
        help = "Reusable port-forward adapter definition UUID; required for custom_adapter"
    )]
    pub(crate) adapter_definition_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum PortForwardBulkActionArg {
    Enable,
    Disable,
    Reapply,
    Delete,
}

impl PortForwardBulkActionArg {
    fn name(self) -> &'static str {
        match self {
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Reapply => "reapply",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct PortForwardCreateCommand {
    #[command(flatten)]
    pub(crate) forwarding: PortForwardModeArgs,
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, value_enum, default_value = "tcp")]
    pub(crate) protocol: PortForwardProtocolArg,
    #[arg(long, value_name = "IP")]
    pub(crate) target_ip: Option<IpAddr>,
    #[arg(
        long,
        value_name = "HOSTNAME",
        help = "Retain the resolved hostname alongside the selected target IP"
    )]
    pub(crate) target_hostname: Option<String>,
    #[arg(
        long,
        value_name = "PORTS",
        help = "Incoming PORT or START-END items, comma separated"
    )]
    pub(crate) incoming: String,
    #[arg(
        long,
        value_name = "PORTS",
        help = "One target port, or one item per incoming item"
    )]
    pub(crate) target: String,
    #[arg(
        long,
        default_value_t = false,
        help = "Preserve original source addresses instead of targeted masquerade"
    )]
    pub(crate) preserve_source: bool,
    #[arg(
        long,
        default_value_t = false,
        help = "Save without applying to the VPS"
    )]
    pub(crate) disabled: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PortForwardUpdateCommand {
    #[command(flatten)]
    pub(crate) forwarding: PortForwardModeArgs,
    #[arg(long)]
    pub(crate) rule_id: Uuid,
    #[arg(long)]
    pub(crate) expected_revision: i64,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, value_enum)]
    pub(crate) protocol: PortForwardProtocolArg,
    #[arg(long, value_name = "IP")]
    pub(crate) target_ip: Option<IpAddr>,
    #[arg(
        long,
        value_name = "HOSTNAME",
        conflicts_with = "clear_target_hostname",
        help = "Replace the resolved hostname retained alongside the target IP"
    )]
    pub(crate) target_hostname: Option<String>,
    #[arg(
        long,
        default_value_t = false,
        conflicts_with = "target_hostname",
        help = "Clear the retained target hostname"
    )]
    pub(crate) clear_target_hostname: bool,
    #[arg(long, value_name = "PORTS")]
    pub(crate) incoming: String,
    #[arg(long, value_name = "PORTS")]
    pub(crate) target: String,
    #[arg(long, default_value_t = false)]
    pub(crate) preserve_source: bool,
    #[arg(long, num_args = 0..=1, default_missing_value = "true", default_value = "true")]
    pub(crate) enabled: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct PortForwardMutationCommand {
    #[arg(long)]
    pub(crate) rule_id: Uuid,
    #[arg(long)]
    pub(crate) expected_revision: i64,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long)]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct PortForwardResolveCommand {
    #[arg(long)]
    pub(crate) hostname: String,
    #[arg(long, value_enum, default_value = "dnat")]
    pub(crate) mode: PortForwardModeArg,
}

#[derive(Debug, Args)]
pub(crate) struct PortForwardBulkCommand {
    #[arg(long, value_enum)]
    pub(crate) action: PortForwardBulkActionArg,
    #[arg(long = "item", value_name = "UUID:REVISION", required = true)]
    pub(crate) items: Vec<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
    #[arg(long)]
    pub(crate) reason: Option<String>,
}

pub(crate) fn list(api_url: &str, token: Option<&str>) -> Result<()> {
    println!(
        "{}",
        http_get(api_url, "/api/v1/port-forward-rules", token)?
    );
    Ok(())
}

pub(crate) fn create(
    api_url: &str,
    token: Option<&str>,
    request: PortForwardCreateCommand,
) -> Result<()> {
    if !request.disabled {
        anyhow::ensure!(
            request.confirmed,
            "enabled port-forward creation requires --confirmed"
        );
    }
    let mappings = pair_port_expressions(&request.incoming, &request.target)
        .context("invalid port mapping")?;
    let mut payload = serde_json::json!({
        "client_id": request.client_id,
        "name": request.name,
        "protocol": PortForwardProtocol::from(request.protocol),
        "target_ip": request.target_ip,
        "mappings": mappings,
        "masquerade": !request.preserve_source,
        "enabled": !request.disabled,
        "confirmed": request.confirmed,
    });
    insert_target_hostname(&mut payload, request.target_hostname.as_deref(), false);
    insert_mode_fields(
        &mut payload,
        &request.forwarding,
        request.target_ip,
        request.target_hostname.as_deref(),
        request.preserve_source,
    )?;
    println!(
        "{}",
        http_post_json(api_url, "/api/v1/port-forward-rules", token, &payload,)?
    );
    Ok(())
}

pub(crate) fn update(
    api_url: &str,
    token: Option<&str>,
    request: PortForwardUpdateCommand,
) -> Result<()> {
    anyhow::ensure!(
        request.expected_revision > 0,
        "--expected-revision must be positive"
    );
    if request.enabled {
        anyhow::ensure!(
            request.confirmed,
            "enabled port-forward update requires --confirmed"
        );
    }
    let mappings = pair_port_expressions(&request.incoming, &request.target)
        .context("invalid port mapping")?;
    let mut payload = serde_json::json!({
        "expected_revision": request.expected_revision,
        "name": request.name,
        "protocol": PortForwardProtocol::from(request.protocol),
        "target_ip": request.target_ip,
        "mappings": mappings,
        "masquerade": !request.preserve_source,
        "enabled": request.enabled,
        "confirmed": request.confirmed,
    });
    insert_target_hostname(
        &mut payload,
        request.target_hostname.as_deref(),
        request.clear_target_hostname,
    );
    insert_mode_fields(
        &mut payload,
        &request.forwarding,
        request.target_ip,
        request.target_hostname.as_deref(),
        request.preserve_source,
    )?;
    println!(
        "{}",
        http_put_json(
            api_url,
            &format!("/api/v1/port-forward-rules/{}", request.rule_id),
            token,
            &payload,
        )?
    );
    Ok(())
}

pub(crate) fn mutate(
    api_url: &str,
    token: Option<&str>,
    request: PortForwardMutationCommand,
    operation: &str,
) -> Result<()> {
    anyhow::ensure!(
        request.expected_revision > 0,
        "--expected-revision must be positive"
    );
    anyhow::ensure!(
        request.confirmed,
        "port-forward {operation} requires --confirmed"
    );
    println!(
        "{}",
        http_post_json(
            api_url,
            &format!(
                "/api/v1/port-forward-rules/{}/{}",
                request.rule_id, operation
            ),
            token,
            &serde_json::json!({
                "expected_revision": request.expected_revision,
                "confirmed": true,
                "reason": request.reason,
            })
        )?
    );
    Ok(())
}

pub(crate) fn resolve(
    api_url: &str,
    token: Option<&str>,
    request: PortForwardResolveCommand,
) -> Result<()> {
    anyhow::ensure!(
        !matches!(request.mode, PortForwardModeArg::Redirect),
        "REDIRECT has no target hostname"
    );
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/network/resolve-hostname",
            token,
            &serde_json::json!({ "hostname": request.hostname, "mode": PortForwardMode::from(request.mode) })
        )?
    );
    Ok(())
}

fn insert_mode_fields(
    payload: &mut serde_json::Value,
    options: &PortForwardModeArgs,
    target_ip: Option<IpAddr>,
    target_hostname: Option<&str>,
    preserve_source: bool,
) -> Result<()> {
    let mode = PortForwardMode::from(options.mode);
    match mode {
        PortForwardMode::Dnat => {
            anyhow::ensure!(target_ip.is_some(), "DNAT requires --target-ip");
            anyhow::ensure!(
                options.address_family.is_none() && options.adapter_definition_id.is_none(),
                "DNAT derives family from --target-ip and does not use an adapter"
            );
        }
        PortForwardMode::Redirect => {
            anyhow::ensure!(
                target_ip.is_none()
                    && target_hostname.is_none()
                    && options.adapter_definition_id.is_none()
                    && !preserve_source,
                "REDIRECT does not accept a target IP/hostname, adapter, or source-NAT option"
            );
        }
        PortForwardMode::CustomAdapter => {
            anyhow::ensure!(
                options.adapter_definition_id.is_some(),
                "custom_adapter requires --adapter-definition-id"
            );
            anyhow::ensure!(
                options.address_family.is_none() && !preserve_source,
                "custom adapters own address family and source behavior"
            );
            anyhow::ensure!(
                target_hostname.is_none() || target_ip.is_some(),
                "resolve the hostname and select --target-ip before saving it"
            );
        }
    }
    payload["mode"] = serde_json::to_value(mode)?;
    payload["address_family"] = if mode == PortForwardMode::Redirect {
        serde_json::to_value(PortForwardAddressFamily::from(
            options.address_family.unwrap_or(PortForwardFamilyArg::Ipv4),
        ))?
    } else {
        serde_json::Value::Null
    };
    payload["adapter_definition_id"] = serde_json::to_value(options.adapter_definition_id)?;
    payload["masquerade"] = serde_json::json!(mode == PortForwardMode::Dnat && !preserve_source);
    if mode == PortForwardMode::Redirect
        || (mode == PortForwardMode::CustomAdapter && target_ip.is_none())
    {
        payload["target_hostname"] = serde_json::Value::Null;
    }
    Ok(())
}

pub(crate) fn bulk(
    api_url: &str,
    token: Option<&str>,
    request: PortForwardBulkCommand,
) -> Result<()> {
    anyhow::ensure!(
        request.confirmed,
        "port-forward bulk mutation requires --confirmed"
    );
    let items = request.items.iter().map(|item| {
        let (id, revision) = item.rsplit_once(':').with_context(|| format!("invalid --item {item:?}; expected UUID:REVISION"))?;
        Ok(serde_json::json!({
            "id": Uuid::parse_str(id).with_context(|| format!("invalid rule UUID in {item:?}"))?,
            "expected_revision": revision.parse::<i64>().with_context(|| format!("invalid revision in {item:?}"))?,
        }))
    }).collect::<Result<Vec<_>>>()?;
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/port-forward-rules/bulk",
            token,
            &serde_json::json!({
                "action": request.action.name(),
                "items": items,
                "confirmed": true,
                "reason": request.reason,
            })
        )?
    );
    Ok(())
}

fn insert_target_hostname(
    payload: &mut serde_json::Value,
    target_hostname: Option<&str>,
    clear_target_hostname: bool,
) {
    let fields = payload
        .as_object_mut()
        .expect("port-forward request payload must be an object");
    if clear_target_hostname {
        fields.insert("target_hostname".to_string(), serde_json::Value::Null);
    } else if let Some(target_hostname) = target_hostname {
        fields.insert(
            "target_hostname".to_string(),
            serde_json::Value::String(target_hostname.to_string()),
        );
    }
}

#[cfg(test)]
#[path = "tests_commands_port_forwarding.rs"]
mod tests;
