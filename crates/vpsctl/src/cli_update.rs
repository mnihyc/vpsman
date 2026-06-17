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
    pub(crate) artifact_url: String,
    #[arg(long)]
    pub(crate) sha256_hex: String,
    #[arg(long)]
    pub(crate) rollback_artifact_url: Option<String>,
    #[arg(long)]
    pub(crate) rollback_sha256_hex: Option<String>,
    #[arg(long)]
    pub(crate) size_bytes: Option<i64>,
    #[arg(long)]
    pub(crate) rollback_size_bytes: Option<i64>,
    #[arg(long)]
    pub(crate) notes: Option<String>,
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
