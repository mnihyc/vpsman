use super::{package_version, release_version};

#[test]
fn release_identity_is_available_at_runtime() {
    assert!(!package_version().is_empty());
    assert!(!release_version().is_empty());
}
