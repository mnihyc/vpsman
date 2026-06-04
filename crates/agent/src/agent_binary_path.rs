use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) fn current_agent_binary_path() -> Result<PathBuf> {
    Ok(normalize_deleted_exe_path(
        std::env::current_exe().context("failed to locate current agent binary")?,
    ))
}

pub(crate) fn normalize_deleted_exe_path(path: PathBuf) -> PathBuf {
    const DELETED_SUFFIX: &str = " (deleted)";
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return path;
    };
    let Some(active_name) = file_name.strip_suffix(DELETED_SUFFIX) else {
        return path;
    };
    path.with_file_name(active_name)
}

pub(crate) fn staged_path(current_exe: &Path) -> Result<PathBuf> {
    let file_name = current_exe
        .file_name()
        .context("current executable path has no file name")?
        .to_string_lossy();
    Ok(current_exe.with_file_name(format!("{file_name}.next")))
}

pub(crate) fn rollback_path(current_exe: &Path) -> Result<PathBuf> {
    let file_name = current_exe
        .file_name()
        .context("current executable path has no file name")?
        .to_string_lossy();
    Ok(current_exe.with_file_name(format!("{file_name}.rollback")))
}

pub(crate) fn activation_marker_path(current_exe: &Path) -> Result<PathBuf> {
    let file_name = current_exe
        .file_name()
        .context("current executable path has no file name")?
        .to_string_lossy();
    Ok(current_exe.with_file_name(format!("{file_name}.activated.json")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::normalize_deleted_exe_path;

    #[test]
    fn strips_linux_deleted_exe_suffix_for_update_paths() {
        let normalized = normalize_deleted_exe_path(PathBuf::from("/tmp/vpsman-agent (deleted)"));
        assert_eq!(normalized, PathBuf::from("/tmp/vpsman-agent"));
    }
}
