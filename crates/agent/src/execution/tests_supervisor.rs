use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn tail_file_reads_only_requested_suffix() {
    let path = std::env::temp_dir().join(format!(
        "vpsman-supervisor-tail-{}.log",
        uuid::Uuid::new_v4()
    ));
    let data = (0..4096)
        .map(|value| b'a' + (value % 26) as u8)
        .collect::<Vec<_>>();
    fs::write(&path, &data).unwrap();

    let tail = tail_file(&path, 37).unwrap();

    assert_eq!(tail, data[data.len() - 37..]);
    assert_eq!(mode(&path), 0o600);
    let _ = fs::remove_file(path);
}

#[test]
fn process_records_are_written_private() {
    let root = temp_supervisor_root("record");
    let record = test_record(&root, "private-record", 12345);

    save_record(&root, &record).unwrap();

    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&records_dir(&root)), 0o700);
    assert_eq!(mode(&record_path(&root, "private-record")), 0o600);
    let loaded = load_record(&root, "private-record").unwrap().unwrap();
    assert_eq!(loaded.name, "private-record");
    assert_eq!(mode(&record_path(&root, "private-record")), 0o600);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn process_logs_are_created_private() {
    let root = temp_supervisor_root("logs");
    let argv = vec!["/bin/true".to_string()];
    let record = start_process(
        &root,
        "private-logs",
        &argv,
        &None,
        &BTreeMap::new(),
        &ProcessRunPolicy::default(),
        &ProcessResourceLimits::default(),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let _ = collect_child_exit_code(record.pid);

    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&logs_dir(&root)), 0o700);
    assert_eq!(mode(Path::new(&record.stdout_log)), 0o600);
    assert_eq!(mode(Path::new(&record.stderr_log)), 0o600);
    let _ = fs::remove_dir_all(root);
}

fn test_record(root: &Path, name: &str, pid: u32) -> ProcessRecord {
    ProcessRecord {
        name: name.to_string(),
        argv: vec!["/bin/true".to_string()],
        cwd: None,
        env: BTreeMap::new(),
        policy: ProcessRunPolicy::default(),
        limits: ProcessResourceLimits::default(),
        pid,
        process_group_id: Some(pid),
        process_identity: None,
        started_unix: 1,
        stdout_log: logs_dir(root)
            .join(format!("{name}.stdout.log"))
            .to_string_lossy()
            .to_string(),
        stderr_log: logs_dir(root)
            .join(format!("{name}.stderr.log"))
            .to_string_lossy()
            .to_string(),
        status: "running".to_string(),
        exit_code: None,
        restart_attempts: 0,
        last_exit_code: None,
        last_exit_unix: None,
        last_restart_unix: None,
        cgroup_path: None,
        limit_evidence: ProcessLimitEvidence::default(),
    }
}

fn temp_supervisor_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "vpsman-supervisor-{label}-{}",
        uuid::Uuid::new_v4()
    ))
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}
