use super::*;

fn test_environment() -> (PathBuf, PackageEnvironment) {
    let root = std::env::temp_dir().join(format!("vpsman-host-packages-{}", uuid::Uuid::new_v4()));
    let etc_root = root.join("etc");
    let var_root = root.join("var");
    let usr_root = root.join("usr");
    let run_root = root.join("run");
    for directory in [&etc_root, &var_root, &usr_root, &run_root] {
        std::fs::create_dir_all(directory).unwrap();
    }
    (
        root,
        PackageEnvironment {
            etc_root,
            var_root,
            usr_root,
            run_root,
            effective_uid: 0,
            apt_get: None,
            dnf: None,
            yum: None,
            pacman: None,
            rpm: None,
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

fn write_os_release(environment: &PackageEnvironment, id: &str, version: Option<&str>) {
    let mut contents = format!("ID={id}\n");
    if let Some(version) = version {
        contents.push_str(&format!("VERSION_ID=\"{version}\"\n"));
    }
    std::fs::write(environment.etc_root.join("os-release"), contents).unwrap();
}

fn install_provider_fixture(
    root: &Path,
    environment: &mut PackageEnvironment,
    provider: HostPackageProvider,
) {
    match provider {
        HostPackageProvider::Apt => {
            let status = environment.var_root.join("lib/dpkg/status");
            std::fs::create_dir_all(status.parent().unwrap()).unwrap();
            std::fs::write(status, "Package: base-files\n").unwrap();
            environment.apt_get = Some(root.join("bin/apt-get"));
            write_executable(
                environment.apt_get.as_deref().unwrap(),
                "#!/bin/sh\nexit 0\n",
            );
        }
        HostPackageProvider::Dnf | HostPackageProvider::Yum => {
            std::fs::create_dir_all(environment.var_root.join("lib/rpm")).unwrap();
            environment.rpm = Some(root.join("bin/rpm"));
            write_executable(environment.rpm.as_deref().unwrap(), "#!/bin/sh\nexit 0\n");
            if provider == HostPackageProvider::Dnf {
                environment.dnf = Some(root.join("bin/dnf"));
                write_executable(environment.dnf.as_deref().unwrap(), "#!/bin/sh\nexit 0\n");
            } else {
                environment.yum = Some(root.join("bin/yum"));
                write_executable(environment.yum.as_deref().unwrap(), "#!/bin/sh\nexit 0\n");
            }
        }
        HostPackageProvider::Pacman => {
            std::fs::create_dir_all(environment.var_root.join("lib/pacman/local")).unwrap();
            environment.pacman = Some(root.join("bin/pacman"));
            write_executable(
                environment.pacman.as_deref().unwrap(),
                "#!/bin/sh\nexit 0\n",
            );
        }
    }
}

#[test]
fn selects_native_providers_for_supported_distro_generations() {
    let cases = [
        ("debian", Some("8"), HostPackageProvider::Apt),
        ("debian", Some("12"), HostPackageProvider::Apt),
        ("ubuntu", Some("14.04"), HostPackageProvider::Apt),
        ("ubuntu", Some("24.04"), HostPackageProvider::Apt),
        ("arch", None, HostPackageProvider::Pacman),
        ("centos", Some("7"), HostPackageProvider::Yum),
        ("centos", Some("8"), HostPackageProvider::Dnf),
        ("rocky", Some("9.4"), HostPackageProvider::Dnf),
    ];
    for (id, version, provider) in cases {
        let (root, mut environment) = test_environment();
        write_os_release(&environment, id, version);
        install_provider_fixture(&root, &mut environment, provider);
        let capability = probe_package_capability(&environment);
        assert_eq!(
            capability.status,
            HostPackageCapabilityStatus::Supported,
            "{id} {version:?}"
        );
        assert_eq!(capability.provider, Some(provider), "{id} {version:?}");
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn selects_yum_for_legacy_centos_without_os_release() {
    let (root, mut environment) = test_environment();
    std::fs::write(
        environment.etc_root.join("centos-release"),
        "CentOS release 6.10 (Final)\n",
    )
    .unwrap();
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Yum);
    let capability = probe_package_capability(&environment);
    assert_eq!(capability.status, HostPackageCapabilityStatus::Supported);
    assert_eq!(capability.provider, Some(HostPackageProvider::Yum));
    assert_eq!(capability.distro_version.as_deref(), Some("6.10"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn uses_explicit_distro_identity_and_refuses_unknown_rhel_versions_or_fallbacks() {
    let (root, mut mixed) = test_environment();
    write_os_release(&mixed, "ubuntu", Some("22.04"));
    install_provider_fixture(&root, &mut mixed, HostPackageProvider::Apt);
    std::fs::create_dir_all(mixed.var_root.join("lib/rpm")).unwrap();
    let capability = probe_package_capability(&mixed);
    assert_eq!(capability.status, HostPackageCapabilityStatus::Supported);
    assert_eq!(capability.provider, Some(HostPackageProvider::Apt));
    std::fs::remove_dir_all(root).unwrap();

    let (root, mut unknown) = test_environment();
    write_os_release(&unknown, "centos", None);
    install_provider_fixture(&root, &mut unknown, HostPackageProvider::Dnf);
    let capability = probe_package_capability(&unknown);
    assert_eq!(capability.status, HostPackageCapabilityStatus::Unsupported);
    assert!(capability
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("major version")));
    std::fs::remove_dir_all(root).unwrap();

    let (root, mut no_fallback) = test_environment();
    write_os_release(&no_fallback, "centos", Some("7"));
    install_provider_fixture(&root, &mut no_fallback, HostPackageProvider::Dnf);
    let capability = probe_package_capability(&no_fallback);
    assert_eq!(capability.status, HostPackageCapabilityStatus::Unsupported);
    assert_eq!(capability.provider, Some(HostPackageProvider::Yum));
    assert!(capability
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("yum")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cached_planning_remains_visible_when_mutation_requires_root() {
    let (root, mut environment) = test_environment();
    write_os_release(&environment, "debian", Some("12"));
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
    environment.effective_uid = 1000;
    let capability = probe_package_capability(&environment);
    assert!(capability.can_plan_cached);
    assert!(!capability.can_refresh_metadata);
    assert!(!capability.can_apply);
    assert!(capability
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("effective UID 1000")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pacman_split_metadata_refresh_is_explicitly_unsupported() {
    let (root, mut environment) = test_environment();
    write_os_release(&environment, "arch", None);
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Pacman);
    let capability = probe_package_capability(&environment);
    assert_eq!(capability.status, HostPackageCapabilityStatus::Supported);
    assert!(capability.can_plan_cached);
    assert!(capability.can_apply);
    assert!(!capability.can_refresh_metadata);
    assert!(capability
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("full system upgrade")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reads_usr_lib_os_release_and_legacy_arch_marker() {
    let (root, mut environment) = test_environment();
    std::fs::create_dir_all(environment.usr_root.join("lib")).unwrap();
    std::fs::write(
        environment.usr_root.join("lib/os-release"),
        "ID=debian\nVERSION_ID=8\n",
    )
    .unwrap();
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
    let capability = probe_package_capability(&environment);
    assert_eq!(capability.provider, Some(HostPackageProvider::Apt));
    std::fs::remove_dir_all(root).unwrap();

    let (root, mut environment) = test_environment();
    std::fs::write(environment.etc_root.join("arch-release"), "").unwrap();
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Pacman);
    let capability = probe_package_capability(&environment);
    assert_eq!(capability.provider, Some(HostPackageProvider::Pacman));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stale_apt_plan_is_rejected_before_package_mutation() {
    let (root, mut environment) = test_environment();
    write_os_release(&environment, "debian", Some("12"));
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
    let apt_get = environment.apt_get.clone().unwrap();
    let mutation_marker = root.join("mutation-ran");
    write_executable(
        &apt_get,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"-s\" ]; then\n  echo 'Inst bash [5.1-2] (5.2-1 Debian:stable [amd64])'\n  exit 0\nfi\nprintf mutation > '{}'\n",
            mutation_marker.display()
        ),
    );
    let error = apply_package_update_plan(
        &environment,
        HostPackageProvider::Apt,
        &"00".repeat(32),
        30,
        CommandCancelToken::default(),
    )
    .await
    .unwrap_err();
    assert!(error
        .to_string()
        .contains("package_update_confirmation_stale"));
    assert!(!mutation_marker.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn accepted_apt_plan_applies_and_rechecks_to_empty() {
    let (root, mut environment) = test_environment();
    write_os_release(&environment, "ubuntu", Some("20.04"));
    install_provider_fixture(&root, &mut environment, HostPackageProvider::Apt);
    let apt_get = environment.apt_get.clone().unwrap();
    let applied_marker = root.join("applied");
    write_executable(
        &apt_get,
        &format!(
            "#!/bin/sh\nif [ \"$1\" = \"-s\" ]; then\n  if [ ! -f '{}' ]; then echo 'Inst openssl [1.1.1f] (1.1.1f-1ubuntu2.22 Ubuntu:focal-updates [amd64])'; fi\n  exit 0\nfi\nif [ \"$1\" = \"-y\" ]; then\n  : > '{}'\n  exit 0\nfi\nexit 2\n",
            applied_marker.display(),
            applied_marker.display()
        ),
    );
    let capability = probe_package_capability(&environment);
    let mut packages = query_package_updates(
        &environment,
        HostPackageProvider::Apt,
        30,
        CommandCancelToken::default(),
    )
    .await
    .unwrap();
    packages.sort();
    let hash = package_plan_hash(&capability, HostPackageProvider::Apt, &packages).unwrap();
    let result = apply_package_update_plan(
        &environment,
        HostPackageProvider::Apt,
        &hash,
        30,
        CommandCancelToken::default(),
    )
    .await
    .unwrap();
    assert!(result.completed);
    assert_eq!(result.applied_package_count, 1);
    assert!(result.remaining_packages.is_empty());
    assert!(applied_marker.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn parses_apt_simulation_across_debian_and_ubuntu_shapes() {
    let updates = parse_apt_updates(
        b"Inst bash [5.1-2] (5.2-1 Debian:stable [amd64])\nInst openssl [3.0.2] (3.0.3 Ubuntu:22.04/jammy-updates [amd64])\nConf bash (5.2-1 Debian:stable [amd64])\n",
    );
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].name, "bash");
    assert_eq!(updates[0].current_version.as_deref(), Some("5.1-2"));
    assert_eq!(updates[0].candidate_version, "5.2-1");
    assert_eq!(updates[0].architecture.as_deref(), Some("amd64"));
    assert_eq!(updates[0].repository.as_deref(), Some("Debian:stable"));
}

#[test]
fn parses_dnf_and_yum_check_update_lines() {
    let updates = parse_rpm_provider_updates(
        b"bash.x86_64 5.1.8-9.el9 baseos\npython3-libs.x86_64 3.9.18-3.el9 appstream\n",
    );
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].name, "bash");
    assert_eq!(updates[0].architecture.as_deref(), Some("x86_64"));
    assert_eq!(updates[0].candidate_version, "5.1.8-9.el9");
    assert_eq!(updates[0].repository.as_deref(), Some("baseos"));
}

#[test]
fn parses_pacman_full_upgrade_plan() {
    let updates =
        parse_pacman_updates(b"linux 6.8.1.arch1-1 -> 6.8.2.arch1-1\nsystemd 255.4-2 -> 255.5-1\n");
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].name, "linux");
    assert_eq!(updates[0].current_version.as_deref(), Some("6.8.1.arch1-1"));
    assert_eq!(updates[0].candidate_version, "6.8.2.arch1-1");
}

#[test]
fn parses_legacy_centos_release_for_explicit_yum_selection() {
    assert_eq!(
        parse_legacy_centos_version("CentOS release 6.10 (Final)\n").as_deref(),
        Some("6.10")
    );
}

#[test]
fn plan_hash_is_order_sensitive_until_callers_sort() {
    let capability = HostPackageCapability {
        status: HostPackageCapabilityStatus::Supported,
        provider: Some(HostPackageProvider::Apt),
        distro_id: "debian".to_string(),
        distro_version: Some("12".to_string()),
        ..HostPackageCapability::default()
    };
    let left = HostPackageUpdateRecord {
        name: "a".to_string(),
        architecture: Some("amd64".to_string()),
        current_version: Some("1".to_string()),
        candidate_version: "2".to_string(),
        repository: Some("stable".to_string()),
    };
    let right = HostPackageUpdateRecord {
        name: "b".to_string(),
        ..left.clone()
    };
    assert_ne!(
        package_plan_hash(
            &capability,
            HostPackageProvider::Apt,
            &[left.clone(), right.clone()]
        )
        .unwrap(),
        package_plan_hash(&capability, HostPackageProvider::Apt, &[right, left]).unwrap()
    );
}
