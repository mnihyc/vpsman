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
