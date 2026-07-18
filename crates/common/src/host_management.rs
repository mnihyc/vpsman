use serde::{Deserialize, Serialize};

fn default_host_process_snapshot_type() -> String {
    "process_list".to_string()
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostProcessView {
    pub pid: u32,
    pub ppid: u32,
    pub uid: u32,
    pub state: String,
    pub name: String,
    pub command: String,
    pub rss_kib: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HostProcessSnapshot {
    #[serde(default = "default_host_process_snapshot_type")]
    pub r#type: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub processes: Vec<HostProcessView>,
}
