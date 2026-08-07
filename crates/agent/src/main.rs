#[path = "runtime/agent_binary_path.rs"]
mod agent_binary_path;
#[path = "lifecycle/backup.rs"]
mod backup;
#[path = "runtime/build_info.rs"]
mod build_info;
#[path = "execution/child_process.rs"]
mod child_process;
#[path = "runtime/cli.rs"]
mod cli;
#[path = "execution/command_ledger.rs"]
mod command_ledger;
#[path = "execution/command_worker.rs"]
mod command_worker;
#[path = "runtime/config_update.rs"]
mod config_update;
#[path = "execution/executor.rs"]
mod executor;
#[cfg(test)]
#[path = "execution/tests_executor.rs"]
mod executor_tests;
#[path = "files/file_browser.rs"]
mod file_browser;
#[path = "files/file_download.rs"]
mod file_download;
#[path = "files/file_pull.rs"]
mod file_pull;
#[path = "files/file_push.rs"]
mod file_push;
#[path = "host/host_packages.rs"]
mod host_packages;
#[path = "host/host_services.rs"]
mod host_services;
#[path = "host/host_storage.rs"]
mod host_storage;
#[path = "network/network_interfaces.rs"]
mod network_interfaces;
#[path = "network/network_probe.rs"]
mod network_probe;
#[path = "network/network_routing_adapter.rs"]
mod network_routing_adapter;
#[path = "network/network_runtime.rs"]
mod network_runtime;
#[path = "network/network_speed.rs"]
mod network_speed;
#[path = "network/network_status.rs"]
mod network_status;
#[path = "network/network_traffic_import.rs"]
mod network_traffic_import;
#[path = "host/platform_accounts.rs"]
mod platform_accounts;
#[path = "network/port_forwarding.rs"]
mod port_forwarding;
#[path = "execution/process.rs"]
mod process;
#[path = "execution/process_cleanup.rs"]
mod process_cleanup;
#[path = "lifecycle/restore.rs"]
mod restore;
#[path = "lifecycle/restore_rollback.rs"]
mod restore_rollback;
#[path = "runtime/runtime.rs"]
mod runtime;
#[path = "runtime/runtime_config_cache.rs"]
mod runtime_config_cache;
#[path = "files/safe_file.rs"]
mod safe_file;
#[path = "files/safe_fs.rs"]
mod safe_fs;
#[path = "runtime/state_dir.rs"]
mod state_dir;
#[path = "execution/supervisor.rs"]
mod supervisor;
#[path = "execution/supervisor_cgroup.rs"]
mod supervisor_cgroup;
#[cfg(test)]
#[path = "execution/tests_supervisor_process.rs"]
mod supervisor_tests;
#[path = "execution/supervisor_validation.rs"]
mod supervisor_validation;
#[path = "telemetry/telemetry.rs"]
mod telemetry;
#[path = "telemetry/telemetry_custom.rs"]
mod telemetry_custom;
#[path = "telemetry/telemetry_traffic.rs"]
mod telemetry_traffic;
#[path = "execution/terminal.rs"]
mod terminal;
#[path = "lifecycle/update.rs"]
mod update;
#[path = "lifecycle/update_activation.rs"]
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
