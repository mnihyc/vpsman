use super::{changed_json_paths, redacted_toml_json, toml_json};

#[test]
fn changed_paths_are_computed_before_redaction() {
    let old = toml_json(
        r#"
version = 1

[database]
postgres_url = "postgres://vpsman:old@postgres:5432/vpsman"

[gateway]
expect_client_public_key_hex = "old"
"#,
    )
    .unwrap();
    let new = toml_json(
        r#"
version = 1

[database]
postgres_url = "postgres://vpsman:new@postgres:5432/vpsman"

[gateway]
expect_client_public_key_hex = "new"
"#,
    )
    .unwrap();

    assert_eq!(
        changed_json_paths(&old, &new),
        vec![
            "database.postgres_url".to_string(),
            "gateway.expect_client_public_key_hex".to_string(),
        ]
    );
}

#[test]
fn added_and_removed_tables_report_leaf_keys() {
    let base = toml_json("version = 1\n").unwrap();
    let with_api = toml_json(
        r#"
version = 1

[api]
job_output_artifact_min_bytes = 4096
gateway_control_read_timeout_ms = 2500
"#,
    )
    .unwrap();

    let expected = vec![
        "api.gateway_control_read_timeout_ms".to_string(),
        "api.job_output_artifact_min_bytes".to_string(),
    ];
    assert_eq!(changed_json_paths(&base, &with_api), expected);
    assert_eq!(changed_json_paths(&with_api, &base), expected);
}

#[test]
fn changed_paths_ignore_equivalent_integer_and_float_toml_numbers() {
    let old = toml_json(
        r#"
version = 1

[api]
artifact_max_bytes = 2.0
"#,
    )
    .unwrap();
    let reformatted = toml_json(
        r#"
version = 1

[api]
artifact_max_bytes = 2
"#,
    )
    .unwrap();
    assert!(changed_json_paths(&old, &reformatted).is_empty());

    let changed = toml_json(
        r#"
version = 1

[api]
artifact_max_bytes = 3
"#,
    )
    .unwrap();
    assert_eq!(
        changed_json_paths(&old, &changed),
        vec!["api.artifact_max_bytes".to_string()]
    );
}

#[test]
fn redacted_toml_json_hides_postgres_url_but_keeps_secret_file_refs() {
    let redacted = redacted_toml_json(
        r#"
version = 1

[database]
postgres_url = "postgres://vpsman:secret@postgres:5432/vpsman"

[api]
gateway_control_url = "http://gateway:9444"

[secrets]
internal_token_file = "/run/secrets/vpsman_internal_token"
"#,
    )
    .unwrap();

    assert_eq!(redacted["database"]["postgres_url"], "<redacted>");
    assert_eq!(
        redacted["api"]["gateway_control_url"],
        "http://gateway:9444"
    );
    assert_eq!(
        redacted["secrets"]["internal_token_file"],
        "/run/secrets/vpsman_internal_token"
    );
}
