use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn normalize_single_json_value() {
    let value = normalize_stdout("{\"ok\":true}\n").unwrap();
    assert_eq!(value["ok"], true);
}

#[test]
fn normalize_jsonl_events() {
    let value = normalize_stdout("{\"seq\":1}\n{\"seq\":2}\n").unwrap();
    assert_eq!(value["kind"], "jsonl");
    assert_eq!(value["items"].as_array().unwrap().len(), 2);
}

#[test]
fn normalize_text_output() {
    let value = normalize_stdout("agent config toml\nline two\n").unwrap();
    assert_eq!(value["kind"], "text");
    assert_eq!(value["stdout"], "agent config toml\nline two\n");
}

#[test]
fn normalize_empty_output() {
    let value = normalize_stdout("").unwrap();
    assert_eq!(value["kind"], "empty");
    assert!(value["stdout"].is_null());
}

#[test]
fn capture_file_path_uses_private_directory() {
    let path = capture_file_path().unwrap();
    let parent = path.parent().unwrap();

    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
