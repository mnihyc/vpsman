use std::net::IpAddr;

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use uuid::Uuid;
use vpsman_common::{pair_port_expressions, PortForwardProtocol};

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
    #[arg(long)]
    pub(crate) client_id: String,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, value_enum, default_value = "tcp")]
    pub(crate) protocol: PortForwardProtocolArg,
    #[arg(long, value_name = "IP")]
    pub(crate) target_ip: IpAddr,
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
    #[arg(long)]
    pub(crate) rule_id: Uuid,
    #[arg(long)]
    pub(crate) expected_revision: i64,
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long, value_enum)]
    pub(crate) protocol: PortForwardProtocolArg,
    #[arg(long, value_name = "IP")]
    pub(crate) target_ip: IpAddr,
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
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/port-forward-rules",
            token,
            &serde_json::json!({
                "client_id": request.client_id,
                "name": request.name,
                "protocol": PortForwardProtocol::from(request.protocol),
                "target_ip": request.target_ip,
                "mappings": mappings,
                "masquerade": !request.preserve_source,
                "enabled": !request.disabled,
                "confirmed": request.confirmed,
            })
        )?
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
    println!(
        "{}",
        http_put_json(
            api_url,
            &format!("/api/v1/port-forward-rules/{}", request.rule_id),
            token,
            &serde_json::json!({
                "expected_revision": request.expected_revision,
                "name": request.name,
                "protocol": PortForwardProtocol::from(request.protocol),
                "target_ip": request.target_ip,
                "mappings": mappings,
                "masquerade": !request.preserve_source,
                "enabled": request.enabled,
                "confirmed": request.confirmed,
            })
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
    println!(
        "{}",
        http_post_json(
            api_url,
            "/api/v1/network/resolve-hostname",
            token,
            &serde_json::json!({ "hostname": request.hostname })
        )?
    );
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

#[cfg(test)]
#[path = "tests_commands_port_forwarding.rs"]
mod tests;
