use super::*;
use std::os::unix::fs::PermissionsExt;

#[tokio::test]
async fn filesystem_object_store_writes_private_files_and_dirs() {
    let root = std::env::temp_dir().join(format!("vpsman-object-store-private-{}", Uuid::new_v4()));
    let store = FilesystemBackupObjectStore::new(root.clone()).unwrap();

    store
        .put_new("backups/client-a/direct.tar", b"direct")
        .await
        .unwrap();

    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&root.join("backups")), 0o700);
    assert_eq!(mode(&root.join("backups/client-a")), 0o700);
    assert_eq!(mode(&root.join("backups/client-a/direct.tar")), 0o600);

    let source = std::env::temp_dir().join(format!("vpsman-object-source-{}", Uuid::new_v4()));
    tokio::fs::write(&source, b"from-file").await.unwrap();
    let expected_hash = sha256_hex(b"from-file");
    store
        .put_file_idempotent(
            "backups/client-a/from-file.tar",
            &source,
            &expected_hash,
            b"from-file".len() as u64,
        )
        .await
        .unwrap();

    assert_eq!(mode(&root.join("backups/client-a/from-file.tar")), 0o600);
    let _ = tokio::fs::remove_file(source).await;
    let _ = tokio::fs::remove_dir_all(root).await;
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}
