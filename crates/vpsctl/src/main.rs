mod backup_artifact_crypto;
mod build_info;
mod cli;
mod cli_access;
mod cli_update;
mod commands;
mod commands_auth;
mod commands_backups;
mod commands_config;
mod commands_dispatch_access;
mod commands_dispatch_backups;
mod commands_dispatch_jobs;
mod commands_enrollment;
mod commands_file_transfer;
mod commands_file_transfer_download;
mod commands_file_transfers;
mod commands_files;
mod commands_inventory;
mod commands_jobs;
mod commands_keys;
mod commands_migrations;
mod commands_network;
mod commands_process;
mod commands_schedules;
mod commands_terminal;
mod commands_terminal_sessions;
mod http;
mod jobs;
mod network_runtime_args;
mod output;
mod privilege;
mod util;
mod vty;
mod vty_agent_update;
mod vty_auth;
mod vty_backup_artifacts;
mod vty_backups;
#[cfg(test)]
mod vty_backups_tests;
mod vty_config;
mod vty_direct;
mod vty_enrollment;
mod vty_file_transfer;
mod vty_file_transfers;
mod vty_files;
mod vty_inventory;
mod vty_job_outputs;
mod vty_jobs;
mod vty_migrations;
mod vty_network;
mod vty_network_adapter;
mod vty_network_dispatch;
mod vty_network_observations;
mod vty_network_ospf;
mod vty_network_probe;
mod vty_network_speed;
mod vty_privilege;
mod vty_process;
mod vty_schedules;
mod vty_terminal;
mod vty_terminal_sessions;
mod vty_tunnel_plan;
mod vty_update_releases;
mod vty_update_rollouts;

use anyhow::Result;
use clap::Parser;
use cli::Args;

pub(crate) use util::unix_now;

fn main() -> Result<()> {
    commands::run(Args::parse())
}
