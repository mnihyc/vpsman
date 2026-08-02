use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn file_transfer_handoff_temp_path_uses_private_directory() {
    let path = file_transfer_handoff_temp_path(Uuid::new_v4()).unwrap();
    let parent = path.parent().unwrap();

    assert_eq!(
        std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
