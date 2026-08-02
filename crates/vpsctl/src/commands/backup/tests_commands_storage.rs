use super::*;

#[test]
fn storage_inventory_is_read_only_and_preserves_explicit_mount_scope() {
    match storage_inventory_operation(2048, true).unwrap() {
        JobCommand::StorageInventory {
            include_pseudo_mounts: true,
            limit: 2048,
        } => {}
        other => panic!("unexpected operation: {other:?}"),
    }
    assert!(storage_inventory_operation(0, false).is_err());
}
