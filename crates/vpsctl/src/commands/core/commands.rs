use anyhow::{bail, Result};

use crate::{
    cli::Args,
    commands_dispatch_access, commands_dispatch_backups, commands_dispatch_jobs,
    output::{self, OutputMode},
};

pub(crate) struct CommandContext {
    pub(crate) api_url: String,
    api_token: Option<String>,
}

impl CommandContext {
    pub(crate) fn token(&self) -> Option<&str> {
        self.api_token.as_deref()
    }
}

pub(crate) fn run(args: Args) -> Result<()> {
    let output_mode = args.output;
    if output_mode != OutputMode::Raw && !args.command.supports_output_mode() {
        bail!("--output is not supported for the interactive vty shell");
    }
    let ctx = CommandContext {
        api_url: args.api_url,
        api_token: args.api_token,
    };

    output::run_with_output_mode(output_mode, || dispatch_command(&ctx, args.command))
}

fn dispatch_command(ctx: &CommandContext, command: crate::cli::Command) -> Result<()> {
    let command = commands_dispatch_access::dispatch(ctx, command)?;
    let command = if let Some(command) = command {
        commands_dispatch_jobs::dispatch(ctx, command)?
    } else {
        None
    };
    let command = if let Some(command) = command {
        commands_dispatch_backups::dispatch(ctx, command)?
    } else {
        None
    };
    if let Some(command) = command {
        bail!("unhandled vpsctl command: {command:?}");
    }
    Ok(())
}
