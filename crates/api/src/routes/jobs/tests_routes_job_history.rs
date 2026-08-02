use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn temp_download_file_uses_private_spool_directory() {
    let temp = TempDownloadFile::new("vpsman-test-download", "bin").unwrap();
    let parent = temp.path().parent().unwrap();

    assert_eq!(
        std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn process_inventory_scan_limit_is_an_explicit_incomplete_signal() {
    let error = map_process_supervisor_inventory_error(anyhow::anyhow!(
        PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR
    ));

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR);
    assert!(error
        .public_message
        .as_deref()
        .is_some_and(|message| message.contains("Exact process inventory")));
}
