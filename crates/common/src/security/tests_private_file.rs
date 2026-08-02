use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn private_atomic_write_clamps_default_readable_modes() {
    let path = std::env::temp_dir().join(format!(
        "vpsman-private-write-{}.toml",
        uuid::Uuid::new_v4()
    ));
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    write_private_file_atomically(&path, b"new").unwrap();

    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let _ = fs::remove_file(path);
}

#[test]
fn private_atomic_write_clamps_group_readable_and_preserves_owner_only_modes() {
    let path = std::env::temp_dir().join(format!(
        "vpsman-private-preserve-{}.toml",
        uuid::Uuid::new_v4()
    ));
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    write_private_file_atomically(&path, b"new").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    write_private_file_atomically(&path, b"newer").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let _ = fs::remove_file(path);
}

#[test]
fn private_dir_tree_clamps_intermediate_dirs() {
    let root = std::env::temp_dir().join(format!("vpsman-private-dir-{}", uuid::Uuid::new_v4()));
    let nested = root.join("a").join("b");

    ensure_private_dir_tree(&root, &nested).unwrap();

    assert_eq!(mode(&root), PRIVATE_DIR_MODE);
    assert_eq!(mode(&root.join("a")), PRIVATE_DIR_MODE);
    assert_eq!(mode(&nested), PRIVATE_DIR_MODE);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn private_file_open_clamps_existing_file() {
    let path = std::env::temp_dir().join(format!("vpsman-private-open-{}", uuid::Uuid::new_v4()));
    fs::write(&path, b"secret").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

    let _file = open_private_file_read(&path).unwrap();

    assert_eq!(mode(&path), PRIVATE_FILE_MODE);
    let _ = fs::remove_file(path);
}

fn mode(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}
