use super::is_vty_file_transfers_command;

#[test]
fn recognizes_file_transfers_commands() {
    assert!(is_vty_file_transfers_command("file-transfers"));
    assert!(is_vty_file_transfers_command("file-transfers --limit 10"));
    assert!(is_vty_file_transfers_command(
        "file-transfer-handoff --client-id edge-a --session-id 11111111-2222-4333-8444-555555555555 --confirmed"
    ));
    assert!(is_vty_file_transfers_command("file-transfer-sources"));
    assert!(is_vty_file_transfers_command(
        "file-transfer-source-upload --source ./payload.bin --confirmed"
    ));
    assert!(is_vty_file_transfers_command(
        "file-transfer-source-download --artifact-id 11111111-2222-4333-8444-555555555555 --output-file ./payload.bin"
    ));
    assert!(!is_vty_file_transfers_command(
        "file-transfer-upload --path /tmp/a"
    ));
}
