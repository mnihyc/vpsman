use std::path::PathBuf;

use super::normalize_deleted_exe_path;

#[test]
fn strips_linux_deleted_exe_suffix_for_update_paths() {
    let normalized = normalize_deleted_exe_path(PathBuf::from("/tmp/vpsman-agent (deleted)"));
    assert_eq!(normalized, PathBuf::from("/tmp/vpsman-agent"));
}
