pub const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const RELEASE_VERSION: &str = env!("VPSMAN_RELEASE_VERSION");

pub fn package_version() -> &'static str {
    PACKAGE_VERSION
}

pub fn release_version() -> &'static str {
    RELEASE_VERSION
}

pub fn release_tag() -> Option<&'static str> {
    option_env!("VPSMAN_RELEASE_TAG").filter(|tag| !tag.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{package_version, release_version};

    #[test]
    fn release_identity_is_available_at_runtime() {
        assert!(!package_version().is_empty());
        assert!(!release_version().is_empty());
    }
}
