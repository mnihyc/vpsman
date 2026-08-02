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
#[path = "tests_release_identity.rs"]
mod tests;
