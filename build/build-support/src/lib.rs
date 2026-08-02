use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseIdentity {
    pub version: String,
    pub tag: Option<String>,
}

pub fn emit_release_identity() {
    println!("cargo:rerun-if-env-changed=VPSMAN_RELEASE_VERSION");
    println!("cargo:rerun-if-env-changed=VPSMAN_RELEASE_TAG");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_TYPE");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");

    let identity = resolve_release_identity(
        |name| std::env::var(name).ok(),
        &std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION is set by Cargo"),
    );
    println!(
        "cargo:rustc-env=VPSMAN_RELEASE_VERSION={}",
        identity.version
    );
    if let Some(tag) = identity.tag {
        println!("cargo:rustc-env=VPSMAN_RELEASE_TAG={tag}");
    }
}

pub fn emit_component_build_number_for(component_env: &str, component_name: &str) {
    emit_release_identity();

    let counter_path = counter_path(component_name);
    println!("cargo:rerun-if-env-changed=VPSMAN_BUILD_NUMBER_DIR");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-changed={}", counter_path.display());

    let build_number = if is_github_actions() {
        read_counter(&counter_path).unwrap_or_else(|error| panic!("{error}"))
    } else {
        increment_counter(&counter_path)
    };

    println!("cargo:rustc-env={component_env}={build_number}");
}

fn resolve_release_identity(
    get_env: impl Fn(&str) -> Option<String>,
    cargo_package_version: &str,
) -> ReleaseIdentity {
    let explicit_tag = clean_env(get_env("VPSMAN_RELEASE_TAG"));
    let github_tag = match (
        clean_env(get_env("GITHUB_REF_TYPE")),
        clean_env(get_env("GITHUB_REF_NAME")),
    ) {
        (Some(kind), Some(name)) if kind == "tag" => Some(name),
        _ => None,
    };
    let tag = explicit_tag.or(github_tag);
    let explicit_version = clean_env(get_env("VPSMAN_RELEASE_VERSION"));
    let version = explicit_version
        .clone()
        .or_else(|| tag.as_deref().map(version_from_tag))
        .unwrap_or_else(|| cargo_package_version.trim().to_string());

    assert!(
        !version.is_empty() && !version.chars().any(char::is_whitespace),
        "VPSMAN_RELEASE_VERSION must be non-empty and contain no whitespace"
    );
    if let Some(tag) = tag.as_deref() {
        assert!(
            !tag.chars().any(char::is_whitespace),
            "VPSMAN_RELEASE_TAG must contain no whitespace"
        );
        if explicit_version.is_some() {
            let tag_version = version_from_tag(tag);
            assert_eq!(
                tag_version, version,
                "VPSMAN_RELEASE_VERSION must match VPSMAN_RELEASE_TAG without a leading v"
            );
        }
    }

    ReleaseIdentity { version, tag }
}

fn clean_env(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn version_from_tag(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

fn counter_path(component_name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("VPSMAN_BUILD_NUMBER_DIR") {
        return PathBuf::from(dir).join(format!("{component_name}.txt"));
    }
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by Cargo"),
    );
    manifest_dir
        .join("../..")
        .join("build")
        .join("build-numbers")
        .join(format!("{component_name}.txt"))
}

fn increment_counter(path: &Path) -> u64 {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("failed to create build-number directory");
    }

    let _lock = CounterLock::acquire(path.with_extension("lock"));
    let current = match read_counter(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("{error}"),
    };
    let next = current
        .checked_add(1)
        .expect("build-number counter exhausted u64");
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .expect("failed to write build-number counter");
    writeln!(file, "{next}").expect("failed to persist build-number counter");
    next
}

fn read_counter(path: &Path) -> std::io::Result<u64> {
    let mut value = String::new();
    let mut file = OpenOptions::new().read(true).open(path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "failed to open build-number counter {}: {error}",
                path.display()
            ),
        )
    })?;
    file.read_to_string(&mut value).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "failed to read build-number counter {}: {error}",
                path.display()
            ),
        )
    })?;
    parse_counter(path, &value)
}

fn parse_counter(path: &Path, value: &str) -> std::io::Result<u64> {
    let trimmed = value.trim();
    let parsed = trimmed.parse::<u64>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "build-number counter {} must contain one positive integer",
                path.display()
            ),
        )
    })?;
    if parsed == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "build-number counter {} must be greater than zero",
                path.display()
            ),
        ));
    }
    Ok(parsed)
}

fn is_github_actions() -> bool {
    std::env::var("GITHUB_ACTIONS")
        .map(|value| value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

struct CounterLock {
    path: PathBuf,
}

impl CounterLock {
    fn acquire(path: PathBuf) -> Self {
        for _ in 0..400 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(_) => return Self { path },
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("failed to acquire build-number lock: {error}"),
            }
        }
        panic!("timed out waiting for build-number lock {}", path.display());
    }
}

impl Drop for CounterLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
#[path = "tests_lib.rs"]
mod tests;
