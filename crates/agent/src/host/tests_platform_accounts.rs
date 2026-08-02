use super::*;

#[test]
fn parses_unix_passwd_with_primary_group() {
    let users = parse_unix_passwd(
        "root:x:0:0:root:/root:/bin/sh\nalice:x:1000:1001::/home/alice:/bin/sh\n",
    )
    .unwrap();
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
    let groups = parse_unix_group("root:x:0:\noperators:x:1001:alice\n").unwrap();
    assert_eq!(groups.id_for_name("operators"), Some(1001));
    assert_eq!(groups.name_for_id(0), Some("root".to_string()));
}

#[test]
fn rejects_invalid_unix_passwd_entry_with_line_context() {
    let error =
        parse_unix_passwd("root:x:0:0:root:/root:/bin/sh\nalice:x:not-a-uid:1001\n").unwrap_err();

    assert!(error
        .to_string()
        .contains("passwd line 2 has invalid uid `not-a-uid`"));
}

#[test]
fn rejects_invalid_unix_group_entry_with_line_context() {
    let error = parse_unix_group("root:x:0:\noperators:x:not-a-gid:alice\n").unwrap_err();

    assert!(error
        .to_string()
        .contains("group line 2 has invalid gid `not-a-gid`"));
}

#[test]
fn account_database_read_error_identifies_the_source() {
    let missing_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-fixtures/definitely-missing-platform-account-database");
    let error = PlatformAccounts::load_from_paths(&missing_path, &missing_path).unwrap_err();

    assert!(error
        .to_string()
        .contains("failed to read platform user database"));
    assert!(error
        .to_string()
        .contains(&missing_path.display().to_string()));
}
