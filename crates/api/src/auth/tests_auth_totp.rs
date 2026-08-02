use super::*;

#[test]
fn totp_matches_rfc6238_sha1_vector() {
    let secret = b"12345678901234567890";

    assert_eq!(totp_code_for_step(secret, 59 / 30), "287082");
    assert_eq!(totp_code_for_step(secret, 1_111_111_109 / 30), "081804");
}

#[test]
fn encrypted_totp_secret_requires_password_and_preserves_secret() {
    let secret = b"local totp secret bytes";
    let encrypted = encrypt_totp_secret("operator-password-123", secret).unwrap();

    assert!(!encrypted.ciphertext_hex.contains("local"));
    assert_eq!(
        decrypt_totp_secret("operator-password-123", &encrypted).unwrap(),
        secret
    );
    assert!(decrypt_totp_secret("wrong-password-123", &encrypted).is_err());
}

#[test]
fn totp_verifier_accepts_one_step_clock_skew_and_rejects_bad_shape() {
    let secret = b"12345678901234567890";
    let code = totp_code_for_step(secret, 1_111_111_109 / 30);

    assert!(verify_totp_code(secret, &code, 1_111_111_109));
    assert!(verify_totp_code(secret, &code, 1_111_111_109 + 30));
    assert!(!verify_totp_code(secret, "12345", 1_111_111_109));
    assert!(!verify_totp_code(secret, "abcdef", 1_111_111_109));
}
