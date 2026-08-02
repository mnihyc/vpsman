use super::*;

const TEST_FILE_PULL_PATH: &str = "/etc/hostname";

#[test]
fn parses_vty_file_pull() {
    let request = parse_vty_file_pull(&["--path", TEST_FILE_PULL_PATH, "id:edge-a"]).unwrap();
    assert_eq!(request.command_label, "file_pull");
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["id:edge-a"]);
    match request.operation {
        JobCommand::FilePull {
            path,
            follow_symlinks,
        } => {
            assert_eq!(path, TEST_FILE_PULL_PATH);
            assert!(!follow_symlinks);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_vty_file_pull_follow_symlinks() {
    let request = parse_vty_file_pull(&[
        "--path",
        TEST_FILE_PULL_PATH,
        "--follow-symlinks",
        "id:edge-a",
    ])
    .unwrap();
    match request.operation {
        JobCommand::FilePull {
            path,
            follow_symlinks,
        } => {
            assert_eq!(path, TEST_FILE_PULL_PATH);
            assert!(follow_symlinks);
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn parses_vty_file_push() {
    let source = std::env::temp_dir().join(format!("vpsman-vty-push-{}", uuid::Uuid::new_v4()));
    fs::write(&source, b"payload").unwrap();
    let request = parse_vty_file_push(&[
        "--source",
        source.to_str().unwrap(),
        "--path",
        "/tmp/payload",
        "--mode",
        "0600",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.command_label, "file_push");
    assert!(request.selection.clients.is_empty());
    assert_eq!(request.selection.tags, vec!["id:edge-a"]);
    match request.operation {
        JobCommand::FilePush {
            mode, size_bytes, ..
        } => {
            assert_eq!(mode, 0o600);
            assert_eq!(size_bytes, 7);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let _ = fs::remove_file(source);
}

#[test]
fn parses_vty_chunked_file_push_for_large_source() {
    let source = std::env::temp_dir().join(format!("vpsman-vty-push-{}", uuid::Uuid::new_v4()));
    fs::write(&source, vec![9_u8; MAX_INLINE_FILE_PUSH_BYTES + 1]).unwrap();
    let request = parse_vty_file_push(&[
        "--source",
        source.to_str().unwrap(),
        "--path",
        "/tmp/payload",
        "id:edge-a",
        "--confirmed",
    ])
    .unwrap();

    assert_eq!(request.command_label, "file_push_chunked");
    match request.operation {
        JobCommand::FilePushChunked {
            size_bytes, chunks, ..
        } => {
            assert_eq!(size_bytes, (MAX_INLINE_FILE_PUSH_BYTES + 1) as u64);
            assert!(chunks.len() > 1);
        }
        other => panic!("unexpected command: {other:?}"),
    }
    let _ = fs::remove_file(source);
}

#[test]
fn rejects_vty_file_push_without_confirmation_or_absolute_path() {
    let source = std::env::temp_dir().join(format!("vpsman-vty-push-{}", uuid::Uuid::new_v4()));
    fs::write(&source, b"payload").unwrap();
    assert!(parse_vty_file_push(&[
        "--source",
        source.to_str().unwrap(),
        "--path",
        "/tmp/payload",
        "id:edge-a",
    ])
    .is_err());
    assert!(parse_vty_file_push(&[
        "--source",
        source.to_str().unwrap(),
        "--path",
        "relative",
        "id:edge-a",
        "--confirmed",
    ])
    .is_err());
    let _ = fs::remove_file(source);
}
