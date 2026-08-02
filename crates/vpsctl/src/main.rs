#[path = "support/backup_artifact_validation.rs"]
mod backup_artifact_validation;
#[path = "support/build_info.rs"]
mod build_info;
#[path = "cli/cli.rs"]
mod cli;
#[path = "cli/cli_access.rs"]
mod cli_access;
#[path = "cli/cli_update.rs"]
mod cli_update;
#[path = "commands/core/commands.rs"]
mod commands;
#[path = "commands/access/commands_auth.rs"]
mod commands_auth;
#[path = "commands/backup/commands_backups.rs"]
mod commands_backups;
#[path = "commands/core/commands_config.rs"]
mod commands_config;
#[path = "commands/access/commands_dispatch_access.rs"]
mod commands_dispatch_access;
#[path = "commands/backup/commands_dispatch_backups.rs"]
mod commands_dispatch_backups;
#[path = "commands/jobs/commands_dispatch_jobs.rs"]
mod commands_dispatch_jobs;
#[path = "commands/files/commands_file_transfer.rs"]
mod commands_file_transfer;
#[path = "commands/files/commands_file_transfer_download.rs"]
mod commands_file_transfer_download;
#[path = "commands/files/commands_file_transfers.rs"]
mod commands_file_transfers;
#[path = "commands/files/commands_files.rs"]
mod commands_files;
#[path = "commands/core/commands_host_management.rs"]
mod commands_host_management;
#[path = "commands/jobs/commands_inventory.rs"]
mod commands_inventory;
#[path = "commands/jobs/commands_jobs.rs"]
mod commands_jobs;
#[path = "commands/access/commands_keys.rs"]
mod commands_keys;
#[path = "commands/jobs/commands_migrations.rs"]
mod commands_migrations;
#[path = "commands/network/commands_network.rs"]
mod commands_network;
#[path = "commands/network/commands_port_forwarding.rs"]
mod commands_port_forwarding;
#[path = "commands/jobs/commands_process.rs"]
mod commands_process;
#[path = "commands/jobs/commands_schedules.rs"]
mod commands_schedules;
#[path = "commands/backup/commands_storage.rs"]
mod commands_storage;
#[path = "commands/terminal/commands_terminal.rs"]
mod commands_terminal;
#[path = "commands/terminal/commands_terminal_sessions.rs"]
mod commands_terminal_sessions;
#[path = "support/http.rs"]
mod http;
#[path = "support/jobs.rs"]
mod jobs;
#[path = "support/network_runtime_args.rs"]
mod network_runtime_args;
#[path = "support/output.rs"]
mod output;
#[path = "support/privilege.rs"]
mod privilege;
#[path = "support/util.rs"]
mod util;
#[path = "vty/core/vty.rs"]
mod vty;
#[path = "vty/updates/vty_agent_update.rs"]
mod vty_agent_update;
#[path = "vty/access/vty_auth.rs"]
mod vty_auth;
#[path = "vty/backup/vty_backup_artifacts.rs"]
mod vty_backup_artifacts;
#[path = "vty/backup/vty_backups.rs"]
mod vty_backups;
#[cfg(test)]
#[path = "vty/backup/tests_vty_backups.rs"]
mod vty_backups_tests;
#[path = "vty/core/vty_config.rs"]
mod vty_config;
#[path = "vty/core/vty_direct.rs"]
mod vty_direct;
#[path = "vty/files/vty_file_transfer.rs"]
mod vty_file_transfer;
#[path = "vty/files/vty_file_transfers.rs"]
mod vty_file_transfers;
#[path = "vty/files/vty_files.rs"]
mod vty_files;
#[path = "vty/jobs/vty_inventory.rs"]
mod vty_inventory;
#[path = "vty/jobs/vty_job_outputs.rs"]
mod vty_job_outputs;
#[path = "vty/jobs/vty_jobs.rs"]
mod vty_jobs;
#[path = "vty/jobs/vty_migrations.rs"]
mod vty_migrations;
#[path = "vty/network/vty_network.rs"]
mod vty_network;
#[path = "vty/network/vty_network_dispatch.rs"]
mod vty_network_dispatch;
#[path = "vty/network/vty_network_observations.rs"]
mod vty_network_observations;
#[path = "vty/network/vty_network_ospf.rs"]
mod vty_network_ospf;
#[path = "vty/network/vty_network_probe.rs"]
mod vty_network_probe;
#[path = "vty/network/vty_network_speed.rs"]
mod vty_network_speed;
#[path = "vty/network/vty_port_forwarding.rs"]
mod vty_port_forwarding;
#[path = "vty/access/vty_privilege.rs"]
mod vty_privilege;
#[path = "vty/jobs/vty_process.rs"]
mod vty_process;
#[path = "vty/jobs/vty_schedules.rs"]
mod vty_schedules;
#[path = "vty/terminal/vty_terminal.rs"]
mod vty_terminal;
#[path = "vty/terminal/vty_terminal_sessions.rs"]
mod vty_terminal_sessions;
#[path = "vty/network/vty_tunnel_plan.rs"]
mod vty_tunnel_plan;
#[path = "vty/updates/vty_update_releases.rs"]
mod vty_update_releases;

use anyhow::Result;
use clap::Parser;
use cli::Args;

pub(crate) use util::unix_now;

fn main() -> Result<()> {
    commands::run(Args::parse())
}
