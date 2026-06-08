use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

pub fn emit_component_build_number_for(component_env: &str, component_name: &str) {
    let counter_path = counter_path(component_name);
    println!("cargo:rerun-if-env-changed=VPSMAN_BUILD_NUMBER_DIR");
    println!("cargo:rerun-if-env-changed=GITHUB_ACTIONS");
    println!("cargo:rerun-if-changed={}", counter_path.display());

    let build_number = if is_github_actions() {
        read_counter(&counter_path).max(1)
    } else {
        increment_counter(&counter_path)
    };

    println!("cargo:rustc-env={component_env}={build_number}");
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
    let current = read_counter(path);
    let next = current.saturating_add(1).max(1);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .expect("failed to write build-number counter");
    writeln!(file, "{next}").expect("failed to persist build-number counter");
    next
}

fn read_counter(path: &Path) -> u64 {
    let mut value = String::new();
    let Ok(mut file) = OpenOptions::new().read(true).open(path) else {
        return 0;
    };
    if file.read_to_string(&mut value).is_err() {
        return 0;
    }
    value.trim().parse::<u64>().unwrap_or(0)
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
