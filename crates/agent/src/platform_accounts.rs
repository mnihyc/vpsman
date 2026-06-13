use anyhow::{Context, Result};

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

#[derive(Clone, Debug, Default)]
pub(crate) struct PlatformAccounts {
    users: NameIdEntries,
    groups: NameIdEntries,
}

impl PlatformAccounts {
    pub(crate) fn load() -> Self {
        Self {
            users: load_platform_users(),
            groups: load_platform_groups(),
        }
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
fn load_platform_users() -> NameIdEntries {
    parse_unix_passwd(&std::fs::read_to_string("/etc/passwd").unwrap_or_default())
}

#[cfg(not(unix))]
fn load_platform_users() -> NameIdEntries {
    NameIdEntries::default()
}

#[cfg(unix)]
fn load_platform_groups() -> NameIdEntries {
    parse_unix_group(&std::fs::read_to_string("/etc/group").unwrap_or_default())
}

#[cfg(not(unix))]
fn load_platform_groups() -> NameIdEntries {
    NameIdEntries::default()
}

#[cfg(unix)]
fn parse_unix_passwd(data: &str) -> NameIdEntries {
    let entries = data
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 4 {
                return None;
            }
            Some(NameIdEntry {
                name: parts[0].to_string(),
                id: parts[2].parse().ok()?,
                primary_group_id: Some(parts[3].parse().ok()?),
            })
        })
        .collect();
    NameIdEntries { entries }
}

#[cfg(unix)]
fn parse_unix_group(data: &str) -> NameIdEntries {
    let entries = data
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split(':').collect();
            if parts.len() < 3 {
                return None;
            }
            Some(NameIdEntry {
                name: parts[0].to_string(),
                id: parts[2].parse().ok()?,
                primary_group_id: None,
            })
        })
        .collect();
    NameIdEntries { entries }
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

pub(crate) fn chown_path(path: &std::path::Path, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    chown_path_impl(path, uid, gid)
}

#[cfg(unix)]
fn chown_path_impl(path: &std::path::Path, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path =
        CString::new(path.as_os_str().as_bytes()).context("path contains an interior nul byte")?;
    let uid = uid
        .map(|value| value as libc::uid_t)
        .unwrap_or(!0 as libc::uid_t);
    let gid = gid
        .map(|value| value as libc::gid_t)
        .unwrap_or(!0 as libc::gid_t);
    let result = unsafe { libc::chown(path.as_ptr(), uid, gid) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("failed to change file ownership");
    }
    Ok(())
}

#[cfg(not(unix))]
fn chown_path_impl(_path: &std::path::Path, _uid: Option<u32>, _gid: Option<u32>) -> Result<()> {
    anyhow::bail!("file ownership changes are not supported on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unix_passwd_with_primary_group() {
        let users = parse_unix_passwd(
            "root:x:0:0:root:/root:/bin/sh\nalice:x:1000:1001::/home/alice:/bin/sh\n",
        );
        assert_eq!(
            users.identity_for_name("alice"),
            Some(AccountIdentity {
                uid: 1000,
                gid: 1001
            })
        );
        assert_eq!(users.name_for_id(0), Some("root".to_string()));
    }

    #[test]
    fn parses_unix_group_entries() {
        let groups = parse_unix_group("root:x:0:\noperators:x:1001:alice\n");
        assert_eq!(groups.id_for_name("operators"), Some(1001));
        assert_eq!(groups.name_for_id(0), Some("root".to_string()));
    }
}
