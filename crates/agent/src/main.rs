mod agent_binary_path;
mod backup;
mod build_info;
mod child_process;
mod cli;
mod command_ledger;
mod command_worker;
mod config_update;
mod executor;
#[cfg(test)]
mod executor_tests;
mod file_browser;
mod file_download;
mod file_pull;
mod file_push;
mod host_packages;
mod host_services;
mod host_storage;
mod network_interfaces;
mod network_probe;
mod network_routing_adapter;
mod network_runtime;
mod network_speed;
mod network_status;
mod platform_accounts;
mod port_forwarding;
mod process;
mod process_cleanup;
mod restore;
mod restore_rollback;
mod runtime;
mod runtime_config_cache;
mod safe_file;
mod safe_fs;
mod state_dir;
mod supervisor;
mod supervisor_cgroup;
#[cfg(test)]
mod supervisor_tests;
mod supervisor_validation;
mod telemetry;
mod telemetry_custom;
mod telemetry_traffic;
mod terminal;
mod update;
mod update_activation;

use anyhow::Result;
use clap::Parser;
use cli::{load_config, Args, Command};
use runtime::run_agent;
use telemetry::{collect_metrics_for_config, TelemetryRuntimeState};
use tracing_subscriber::fmt::writer::MakeWriterExt;

#[tokio::main]
async fn main() -> Result<()> {
    let log_writer = std::io::stderr
        .with_max_level(tracing::Level::WARN)
        .or_else(std::io::stdout.with_min_level(tracing::Level::INFO));
    tracing_subscriber::fmt()
        .with_writer(log_writer)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,vpsman_agent=info".into()),
        )
        .init();

    let args = Args::parse();
    let config = load_config(&args.config)?;

    match args.command {
        Command::Run { endpoint } => run_agent(config, args.config, endpoint).await,
        Command::Once => {
            let mut runtime_state = TelemetryRuntimeState::default();
            let metrics = collect_metrics_for_config(&config, &mut runtime_state).await?;
            println!("{}", serde_json::to_string_pretty(&metrics)?);
            Ok(())
        }
    }
}
