use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn compose_secrets_write_required_files_without_exposing_password() {
    let root = std::env::temp_dir().join(format!(
        "vpsctl-compose-secrets-test-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let password_env = "VPSCTL_TEST_COMPOSE_SECRET_PASSWORD";
    std::env::set_var(password_env, "correct horse battery staple");
    compose_secrets(ComposeSecretsOptions {
        secrets_dir: root.clone(),
        password_env: password_env.to_string(),
        super_salt_hex: Some("01020304".to_string()),
        force: false,
    })
    .unwrap();

    for name in [
        "vpsman_internal_token",
        "vpsman_gateway_private_key_hex",
        "vpsman_privilege_verifier_key_hex",
        "vpsman_gateway_public_key_hex",
        "operator-privilege.env",
    ] {
        let path = root.join(name);
        let contents = fs::read_to_string(&path).unwrap();
        assert!(!contents.contains("correct horse battery staple"));
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    assert_eq!(
        fs::read_to_string(root.join("operator-privilege.env")).unwrap(),
        "export VPSMAN_SUPER_SALT_HEX=01020304\n"
    );
    let expected_verifier = hex::encode(derive_super_key(
        "correct horse battery staple",
        &[1, 2, 3, 4],
    ));
    assert_eq!(
        fs::read_to_string(root.join("vpsman_privilege_verifier_key_hex")).unwrap(),
        format!("{expected_verifier}\n")
    );
    let _ = fs::remove_dir_all(root);
    std::env::remove_var(password_env);
}
