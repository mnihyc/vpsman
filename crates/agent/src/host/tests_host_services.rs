use super::*;

fn test_environment(pid1: &str) -> (PathBuf, ServiceEnvironment) {
    let root = std::env::temp_dir().join(format!("vpsman-host-services-{}", uuid::Uuid::new_v4()));
    let proc_root = root.join("proc");
    let run_root = root.join("run");
    let etc_root = root.join("etc");
    std::fs::create_dir_all(proc_root.join("1")).unwrap();
    std::fs::create_dir_all(&run_root).unwrap();
    std::fs::create_dir_all(&etc_root).unwrap();
    std::fs::write(proc_root.join("1/comm"), format!("{pid1}\n")).unwrap();
    (
        root,
        ServiceEnvironment {
            proc_root,
            run_root,
            etc_root,
            effective_uid: 0,
            systemctl: None,
            journalctl: None,
            rc_service: None,
            rc_status: None,
            rc_update: None,
            service: None,
            update_rc_d: None,
            chkconfig: None,
        },
    )
}

fn write_executable(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[tokio::test]
async fn detects_systemd_only_when_pid1_marker_and_binary_agree() {
    let (root, mut environment) = test_environment("systemd");
    std::fs::create_dir_all(environment.run_root.join("systemd/system")).unwrap();
    environment.systemctl = Some(root.join("bin/systemctl"));
    environment.journalctl = Some(root.join("bin/journalctl"));

    let capability = probe_service_capability(&environment).await;
    assert_eq!(capability.status, HostServiceCapabilityStatus::Supported);
    assert_eq!(capability.provider, Some(HostServiceProvider::Systemd));
    assert!(capability.can_inventory);
    assert!(capability.can_start_stop_restart);
    assert!(capability.can_enable_disable);
    assert!(capability.can_read_logs);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn reports_containers_without_a_confirmed_init_provider_as_unsupported() {
    let (root, mut environment) = test_environment("tini");
    environment.systemctl = Some(root.join("bin/systemctl"));
    environment.service = Some(root.join("sbin/service"));

    let capability = probe_service_capability(&environment).await;
    assert_eq!(capability.status, HostServiceCapabilityStatus::Unsupported);
    assert_eq!(capability.provider, None);
    assert!(capability
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("PID 1 is \"tini\"")));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn refuses_conflicting_systemd_and_openrc_markers() {
    let (root, mut environment) = test_environment("init");
    std::fs::create_dir_all(environment.run_root.join("systemd/system")).unwrap();
    std::fs::create_dir_all(environment.run_root.join("openrc")).unwrap();
    environment.systemctl = Some(root.join("bin/systemctl"));
    environment.rc_service = Some(root.join("bin/rc-service"));
    environment.rc_status = Some(root.join("bin/rc-status"));

    let capability = probe_service_capability(&environment).await;
    assert_eq!(capability.status, HostServiceCapabilityStatus::Ambiguous);
    assert_eq!(capability.provider, None);
    assert!(!capability.can_start_stop_restart);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn detects_openrc_and_reports_unprivileged_mutations_explicitly() {
    let (root, mut environment) = test_environment("openrc-init");
    std::fs::create_dir_all(environment.run_root.join("openrc")).unwrap();
    environment.effective_uid = 1000;
    environment.rc_service = Some(root.join("bin/rc-service"));
    environment.rc_status = Some(root.join("bin/rc-status"));
    environment.rc_update = Some(root.join("bin/rc-update"));

    let capability = probe_service_capability(&environment).await;
    assert_eq!(capability.status, HostServiceCapabilityStatus::Supported);
    assert_eq!(capability.provider, Some(HostServiceProvider::Openrc));
    assert!(capability.can_inventory);
    assert!(!capability.can_start_stop_restart);
    assert!(!capability.can_enable_disable);
    assert!(capability
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("effective UID 1000")));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn keeps_sysv_enablement_unsupported_when_backends_are_ambiguous() {
    let (root, mut environment) = test_environment("init");
    std::fs::create_dir_all(environment.init_dir()).unwrap();
    environment.service = Some(root.join("sbin/service"));
    environment.update_rc_d = Some(root.join("sbin/update-rc.d"));
    environment.chkconfig = Some(root.join("sbin/chkconfig"));

    let capability = probe_service_capability(&environment).await;
    assert_eq!(capability.status, HostServiceCapabilityStatus::Supported);
    assert_eq!(capability.provider, Some(HostServiceProvider::Sysv));
    assert!(capability.can_start_stop_restart);
    assert!(!capability.can_enable_disable);
    assert_eq!(capability.enable_backend.as_deref(), Some("ambiguous"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn rejects_a_stale_systemd_confirmation_before_mutation() {
    let (root, mut environment) = test_environment("systemd");
    std::fs::create_dir_all(environment.run_root.join("systemd/system")).unwrap();
    let mutation_marker = root.join("mutation-ran");
    let systemctl = root.join("bin/systemctl");
    write_executable(
        &systemctl,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"show\" ]; then\n  printf '%s\\n' 'Id=sshd.service' 'Description=OpenSSH server' 'LoadState=loaded' 'ActiveState=active' 'SubState=running' 'UnitFileState=enabled'\n  exit 0\nfi\nprintf mutation > '{}'\n",
            mutation_marker.display()
        ),
    );
    environment.systemctl = Some(systemctl);

    let error = apply_service_action(
        &environment,
        HostServiceProvider::Systemd,
        "sshd.service",
        HostServiceAction::Start,
        "inactive",
        "enabled",
        10,
        CommandCancelToken::default(),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("host_service_confirmation_stale"));
    assert!(!mutation_marker.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_systemd_inventory_across_old_plain_text_shape() {
    let rows = parse_systemd_inventory(
        b"sshd.service loaded active running OpenSSH server\ncron.service loaded inactive dead Regular background program processing daemon\n",
        b"cron.service enabled\nsshd.service enabled-runtime\nmissing.service disabled\n",
    );
    assert_eq!(rows.len(), 3);
    let sshd = rows.iter().find(|row| row.name == "sshd.service").unwrap();
    assert_eq!(sshd.active_state, "active");
    assert_eq!(sshd.enabled_state, "enabled-runtime");
    assert_eq!(sshd.description, "OpenSSH server");
    let missing = rows
        .iter()
        .find(|row| row.name == "missing.service")
        .unwrap();
    assert_eq!(missing.load_state, "not-loaded");
    assert_eq!(missing.enabled_state, "disabled");
}

#[test]
fn parses_centos_chkconfig_runlevels_without_guessing_status() {
    let enabled = parse_chkconfig_enabled(
        b"network 0:off 1:off 2:on 3:on 4:on 5:on 6:off\nsshd 0:off 1:off 2:off 3:off 4:off 5:off 6:off\n",
    );
    assert!(enabled.contains("network"));
    assert!(!enabled.contains("sshd"));
}

#[test]
fn script_status_uses_lsb_exit_codes_and_keeps_unknown_explicit() {
    let inactive = CommandResult {
        stdout: b"service is not running\n".to_vec(),
        stderr: Vec::new(),
        exit_code: Some(3),
        stdout_truncated: false,
        stderr_truncated: false,
    };
    assert_eq!(
        script_service_state(&inactive),
        ("inactive".to_string(), "stopped".to_string())
    );
    let unknown = CommandResult {
        stdout: b"status unavailable\n".to_vec(),
        exit_code: Some(4),
        ..inactive
    };
    assert_eq!(script_service_state(&unknown).0, "unknown");
}
