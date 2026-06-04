use std::path::PathBuf;

use clap::Args;

#[derive(Debug, Args)]
pub(crate) struct AgentUpdateReleasePublishArgs {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) version: String,
    #[arg(long, default_value = "stable")]
    pub(crate) channel: String,
    #[arg(long)]
    pub(crate) artifact_file: PathBuf,
    #[arg(long)]
    pub(crate) artifact_url: String,
    #[arg(long)]
    pub(crate) signing_seed_hex: String,
    #[arg(long)]
    pub(crate) rollback_artifact_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) rollback_artifact_url: Option<String>,
    #[arg(long)]
    pub(crate) rollback_signing_seed_hex: Option<String>,
    #[arg(long)]
    pub(crate) notes: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentUpdateArtifactUploadArgs {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) version: String,
    #[arg(long, default_value = "stable")]
    pub(crate) channel: String,
    #[arg(long)]
    pub(crate) artifact_file: PathBuf,
    #[arg(long)]
    pub(crate) signing_seed_hex: String,
    #[arg(long)]
    pub(crate) rollback_artifact_file: Option<PathBuf>,
    #[arg(long)]
    pub(crate) rollback_signing_seed_hex: Option<String>,
    #[arg(long)]
    pub(crate) notes: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) stream: bool,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AgentUpdateReleaseLatestArgs {
    #[arg(long, default_value = "vpsman-agent")]
    pub(crate) name: String,
    #[arg(long, default_value = "stable")]
    pub(crate) channel: String,
}

#[derive(Debug, Args)]
pub(crate) struct AgentUpdateRolloutPoliciesArgs {
    #[arg(long, default_value_t = 25)]
    pub(crate) limit: u16,
    #[arg(long)]
    pub(crate) enabled: Option<bool>,
    #[arg(long)]
    pub(crate) channel: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct AgentUpdateRolloutPolicyCreateArgs {
    #[arg(long)]
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) scope_kind: String,
    #[arg(long)]
    pub(crate) scope_value: Option<String>,
    #[arg(long)]
    pub(crate) channel: Option<String>,
    #[arg(long)]
    pub(crate) canary_count: Option<i32>,
    #[arg(long)]
    pub(crate) health_gate: Option<String>,
    #[arg(long, default_value_t = 0)]
    pub(crate) priority: i32,
    #[arg(long, default_value_t = true)]
    pub(crate) enabled: bool,
    #[arg(long)]
    pub(crate) notes: Option<String>,
    #[arg(long, default_value_t = false)]
    pub(crate) confirmed: bool,
}
