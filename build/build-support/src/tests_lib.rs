use std::path::Path;

use super::{parse_counter, resolve_release_identity};

fn env_from<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
    |name| {
        pairs
            .iter()
            .find_map(|(key, value)| (*key == name).then(|| (*value).to_string()))
    }
}

#[test]
fn release_identity_prefers_explicit_release_env() {
    let identity = resolve_release_identity(
        env_from(&[
            ("VPSMAN_RELEASE_TAG", "v1.2.3"),
            ("VPSMAN_RELEASE_VERSION", "1.2.3"),
        ]),
        "0.1.0",
    );

    assert_eq!(identity.version, "1.2.3");
    assert_eq!(identity.tag.as_deref(), Some("v1.2.3"));
}

#[test]
fn release_identity_derives_version_from_github_tag() {
    let identity = resolve_release_identity(
        env_from(&[("GITHUB_REF_TYPE", "tag"), ("GITHUB_REF_NAME", "v2.0.1")]),
        "0.1.0",
    );

    assert_eq!(identity.version, "2.0.1");
    assert_eq!(identity.tag.as_deref(), Some("v2.0.1"));
}

#[test]
fn release_identity_falls_back_to_cargo_package_version() {
    let identity = resolve_release_identity(env_from(&[]), "0.1.0");

    assert_eq!(identity.version, "0.1.0");
    assert_eq!(identity.tag, None);
}

#[test]
#[should_panic(expected = "VPSMAN_RELEASE_VERSION must match VPSMAN_RELEASE_TAG")]
fn release_identity_rejects_tag_version_mismatch() {
    let _ = resolve_release_identity(
        env_from(&[
            ("VPSMAN_RELEASE_TAG", "v1.2.3"),
            ("VPSMAN_RELEASE_VERSION", "1.2.4"),
        ]),
        "0.1.0",
    );
}

#[test]
fn build_counter_requires_one_positive_integer() {
    let path = Path::new("build/build-numbers/agent.txt");
    assert_eq!(parse_counter(path, "42\n").unwrap(), 42);
    for invalid in ["", "0", "-1", "1.5", "not-a-number", "1\n2"] {
        assert!(
            parse_counter(path, invalid).is_err(),
            "accepted invalid build counter {invalid:?}"
        );
    }
}
