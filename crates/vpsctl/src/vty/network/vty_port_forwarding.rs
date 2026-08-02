use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::commands_port_forwarding::{
    self, PortForwardBulkCommand, PortForwardCreateCommand, PortForwardMutationCommand,
    PortForwardResolveCommand, PortForwardUpdateCommand,
};

#[derive(Debug, Parser)]
#[command(no_binary_name = false)]
struct VtyPortForwardArgs {
    #[command(subcommand)]
    command: VtyPortForwardCommand,
}

#[derive(Debug, Subcommand)]
enum VtyPortForwardCommand {
    #[command(name = "port-forward-create")]
    Create(PortForwardCreateCommand),
    #[command(name = "port-forward-update")]
    Update(PortForwardUpdateCommand),
    #[command(name = "port-forward-enable")]
    Enable(PortForwardMutationCommand),
    #[command(name = "port-forward-disable")]
    Disable(PortForwardMutationCommand),
    #[command(name = "port-forward-delete")]
    Delete(PortForwardMutationCommand),
    #[command(name = "port-forward-forget")]
    Forget(PortForwardMutationCommand),
    #[command(name = "port-forward-reapply")]
    Reapply(PortForwardMutationCommand),
    #[command(name = "port-forward-resolve")]
    Resolve(PortForwardResolveCommand),
    #[command(name = "port-forward-bulk")]
    Bulk(PortForwardBulkCommand),
}

pub(crate) fn is_vty_port_forward_command(command: &str) -> bool {
    [
        "port-forward-create ",
        "port-forward-update ",
        "port-forward-enable ",
        "port-forward-disable ",
        "port-forward-delete ",
        "port-forward-forget ",
        "port-forward-reapply ",
        "port-forward-resolve ",
        "port-forward-bulk ",
    ]
    .iter()
    .any(|prefix| command.starts_with(prefix))
}

pub(crate) fn submit_vty_port_forward_command(
    api_url: &str,
    token: Option<&str>,
    command: &str,
) -> Result<()> {
    let args = VtyPortForwardArgs::try_parse_from(command.split_whitespace())?;
    match args.command {
        VtyPortForwardCommand::Create(request) => {
            commands_port_forwarding::create(api_url, token, request)
        }
        VtyPortForwardCommand::Update(request) => {
            commands_port_forwarding::update(api_url, token, request)
        }
        VtyPortForwardCommand::Enable(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "enable")
        }
        VtyPortForwardCommand::Disable(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "disable")
        }
        VtyPortForwardCommand::Delete(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "delete")
        }
        VtyPortForwardCommand::Forget(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "forget")
        }
        VtyPortForwardCommand::Reapply(request) => {
            commands_port_forwarding::mutate(api_url, token, request, "reapply")
        }
        VtyPortForwardCommand::Resolve(request) => {
            commands_port_forwarding::resolve(api_url, token, request)
        }
        VtyPortForwardCommand::Bulk(request) => {
            commands_port_forwarding::bulk(api_url, token, request)
        }
    }
}

#[cfg(test)]
#[path = "tests_vty_port_forwarding.rs"]
mod tests;
