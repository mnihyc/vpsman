use super::{parse_vty_migration_link, parse_vty_migration_run};

#[test]
fn parses_vty_migration_link() {
    let request = parse_vty_migration_link(&[
        "49c7c3ea-0da8-40b6-b380-5543b1eb3adb",
        "--note",
        "rebuilt",
        "--confirmed",
    ])
    .unwrap();
    assert_eq!(
        request.restore_plan_id.to_string(),
        "49c7c3ea-0da8-40b6-b380-5543b1eb3adb"
    );
    assert_eq!(request.note.as_deref(), Some("rebuilt"));
    assert!(request.confirmed);
}

#[test]
fn rejects_unconfirmed_vty_migration_link() {
    assert!(parse_vty_migration_link(&["49c7c3ea-0da8-40b6-b380-5543b1eb3adb"]).is_err());
}

#[test]
fn parses_vty_migration_run() {
    let archive_transfer_session_id = uuid::Uuid::new_v4();
    let request = parse_vty_migration_run(&[
        "49c7c3ea-0da8-40b6-b380-5543b1eb3adb",
        "--archive-transfer-session-id",
        &archive_transfer_session_id.to_string(),
        "--note",
        "cutover",
        "--max-timeout",
        "120",
        "--dry-run",
        "--force-unprivileged",
        "--confirmed",
    ])
    .unwrap();
    assert_eq!(
        request.restore_plan_id.to_string(),
        "49c7c3ea-0da8-40b6-b380-5543b1eb3adb"
    );
    assert_eq!(
        request.archive_transfer_session_id,
        archive_transfer_session_id
    );
    assert_eq!(request.note.as_deref(), Some("cutover"));
    assert_eq!(request.max_timeout_secs, 120);
    assert!(request.dry_run);
    assert!(request.force_unprivileged);
    assert!(request.confirmed);
}

#[test]
fn rejects_unconfirmed_vty_migration_run() {
    assert!(parse_vty_migration_run(&["49c7c3ea-0da8-40b6-b380-5543b1eb3adb"]).is_err());
    assert!(parse_vty_migration_run(&[
        "49c7c3ea-0da8-40b6-b380-5543b1eb3adb",
        "--archive-transfer-session-id",
        &uuid::Uuid::new_v4().to_string(),
        "--dry-run",
    ])
    .is_err());
}
