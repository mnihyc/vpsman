#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
use anyhow::Context;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountIdentity {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NameIdResolution {
    pub(crate) id: u32,
    pub(crate) name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwnershipTokens {
    pub(crate) owner: Option<String>,
    pub(crate) group: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PlatformAccounts {
    users: NameIdEntries,
    groups: NameIdEntries,
}

impl PlatformAccounts {
    #[cfg(unix)]
    pub(crate) fn load() -> anyhow::Result<Self> {
        Self::load_from_paths(Path::new("/etc/passwd"), Path::new("/etc/group"))
    }

    #[cfg(not(unix))]
    pub(crate) fn load() -> anyhow::Result<Self> {
        Ok(Self::default())
    }

    #[cfg(unix)]
    fn load_from_paths(passwd_path: &Path, group_path: &Path) -> anyhow::Result<Self> {
        let passwd = std::fs::read_to_string(passwd_path).with_context(|| {
            format!(
                "failed to read platform user database {}",
                passwd_path.display()
            )
        })?;
        let users = parse_unix_passwd(&passwd).with_context(|| {
            format!(
                "failed to parse platform user database {}",
                passwd_path.display()
            )
        })?;
        let group = std::fs::read_to_string(group_path).with_context(|| {
            format!(
                "failed to read platform group database {}",
                group_path.display()
            )
        })?;
        let groups = parse_unix_group(&group).with_context(|| {
            format!(
                "failed to parse platform group database {}",
                group_path.display()
            )
        })?;
        Ok(Self { users, groups })
    }

    pub(crate) fn find_user_identity(&self, user: &str) -> Option<AccountIdentity> {
        self.users.identity_for_name(user)
    }

    pub(crate) fn resolve_user(&self, value: &str) -> Option<NameIdResolution> {
        resolve_name_or_id(value, &self.users)
    }

    pub(crate) fn resolve_group(&self, value: &str) -> Option<NameIdResolution> {
        resolve_name_or_id(value, &self.groups)
    }

    pub(crate) fn user_name_for_id(&self, id: u32) -> Option<String> {
        self.users.name_for_id(id)
    }

    pub(crate) fn group_name_for_id(&self, id: u32) -> Option<String> {
        self.groups.name_for_id(id)
    }
}

fn resolve_name_or_id(value: &str, entries: &NameIdEntries) -> Option<NameIdResolution> {
    if let Ok(id) = value.parse::<u32>() {
        return Some(NameIdResolution {
            id,
            name: entries.name_for_id(id),
        });
    }
    entries.id_for_name(value).map(|id| NameIdResolution {
        id,
        name: Some(value.to_string()),
    })
}

pub(crate) fn normalize_ownership_tokens(
    owner: Option<&str>,
    group: Option<&str>,
    uid: Option<u32>,
    gid: Option<u32>,
) -> anyhow::Result<OwnershipTokens> {
    let owner = trimmed_nonempty(owner);
    let group = trimmed_nonempty(group);
    if let Some(owner_value) = owner {
        if let Some((owner_part, group_part)) = owner_value.split_once(':') {
            if group.is_some() || uid.is_some() || gid.is_some() {
                anyhow::bail!("owner:group must not be combined with separate owner/group ids");
            }
            let owner_part = owner_part.trim();
            let group_part = group_part.trim();
            if owner_part.is_empty() || group_part.is_empty() || group_part.contains(':') {
                anyhow::bail!("owner:group requires exactly one owner and one group");
            }
            return Ok(OwnershipTokens {
                owner: Some(owner_part.to_string()),
                group: Some(group_part.to_string()),
            });
        }
    }
    Ok(OwnershipTokens {
        owner: owner.map(str::to_string),
        group: group.map(str::to_string),
    })
}

fn trimmed_nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[derive(Clone, Debug, Default)]
struct NameIdEntries {
    entries: Vec<NameIdEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NameIdEntry {
    name: String,
    id: u32,
    primary_group_id: Option<u32>,
}

impl NameIdEntries {
    fn identity_for_name(&self, name: &str) -> Option<AccountIdentity> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| AccountIdentity {
                uid: entry.id,
                gid: entry.primary_group_id.unwrap_or(entry.id),
            })
    }

    fn id_for_name(&self, name: &str) -> Option<u32> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.id)
    }

    fn name_for_id(&self, id: u32) -> Option<String> {
        self.entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.name.clone())
    }
}

#[cfg(unix)]
fn parse_unix_passwd(data: &str) -> anyhow::Result<NameIdEntries> {
    let mut entries = Vec::new();
    for (index, line) in data.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_number = index + 1;
        let parts = line.split(':').collect::<Vec<_>>();
        anyhow::ensure!(
            parts.len() >= 4,
            "passwd line {line_number} has fewer than four fields"
        );
        anyhow::ensure!(
            !parts[0].is_empty(),
            "passwd line {line_number} has an empty account name"
        );
        let uid = parts[2]
            .parse::<u32>()
            .with_context(|| format!("passwd line {line_number} has invalid uid `{}`", parts[2]))?;
        let gid = parts[3].parse::<u32>().with_context(|| {
            format!(
                "passwd line {line_number} has invalid primary gid `{}`",
                parts[3]
            )
        })?;
        entries.push(NameIdEntry {
            name: parts[0].to_string(),
            id: uid,
            primary_group_id: Some(gid),
        });
    }
    Ok(NameIdEntries { entries })
}

#[cfg(unix)]
fn parse_unix_group(data: &str) -> anyhow::Result<NameIdEntries> {
    let mut entries = Vec::new();
    for (index, line) in data.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_number = index + 1;
        let parts = line.split(':').collect::<Vec<_>>();
        anyhow::ensure!(
            parts.len() >= 3,
            "group line {line_number} has fewer than three fields"
        );
        anyhow::ensure!(
            !parts[0].is_empty(),
            "group line {line_number} has an empty group name"
        );
        let gid = parts[2]
            .parse::<u32>()
            .with_context(|| format!("group line {line_number} has invalid gid `{}`", parts[2]))?;
        entries.push(NameIdEntry {
            name: parts[0].to_string(),
            id: gid,
            primary_group_id: None,
        });
    }
    Ok(NameIdEntries { entries })
}

pub(crate) fn current_effective_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() as u32 }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

pub(crate) fn metadata_uid(metadata: &std::fs::Metadata) -> Option<u32> {
    metadata_uid_impl(metadata)
}

#[cfg(unix)]
fn metadata_uid_impl(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.uid())
}

#[cfg(not(unix))]
fn metadata_uid_impl(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

pub(crate) fn metadata_gid(metadata: &std::fs::Metadata) -> Option<u32> {
    metadata_gid_impl(metadata)
}

#[cfg(unix)]
fn metadata_gid_impl(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.gid())
}

#[cfg(not(unix))]
fn metadata_gid_impl(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

pub(crate) fn metadata_mode(metadata: &std::fs::Metadata) -> Option<u32> {
    metadata_mode_impl(metadata)
}

#[cfg(unix)]
fn metadata_mode_impl(metadata: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.mode())
}

#[cfg(not(unix))]
fn metadata_mode_impl(_metadata: &std::fs::Metadata) -> Option<u32> {
    None
}

pub(crate) fn metadata_mtime_unix(metadata: &std::fs::Metadata) -> Option<i64> {
    metadata_mtime_unix_impl(metadata)
}

#[cfg(unix)]
fn metadata_mtime_unix_impl(metadata: &std::fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;

    Some(metadata.mtime())
}

#[cfg(not(unix))]
fn metadata_mtime_unix_impl(_metadata: &std::fs::Metadata) -> Option<i64> {
    None
}

#[cfg(test)]
#[path = "tests_platform_accounts.rs"]
mod tests;
