use super::*;

const HELP_JSON: &str = "Usage: lsblk [options]\n  -J, --json use JSON output format\n  -P, --pairs use key=\"value\" output format\n  -p, --paths print complete device path\nAvailable output columns:\n  NAME device name\n  KNAME kernel name\n  PKNAME parent name\n  TYPE device type\n  SIZE size\n  FSTYPE filesystem type\n  FSVER filesystem version\n  LABEL filesystem label\n  UUID filesystem UUID\n  MOUNTPOINT mount point\n  FSAVAIL filesystem available bytes\n  FSUSE% filesystem use percentage\n  RO read-only\n  RM removable\n  MODEL model\n  SERIAL serial\n  TRAN transport\n  MAJ:MIN major:minor\n";

const HELP_PAIRS: &str = "Usage: lsblk [options]\n  -P, --pairs output pairs\n  -p, --paths print paths\nAvailable output columns:\n  NAME name\n  KNAME kernel name\n  PKNAME parent\n  TYPE type\n  SIZE size\n  FSTYPE fs\n  LABEL label\n  UUID uuid\n  MOUNTPOINT mount\n  RO read only\n  RM removable\n  MODEL model\n  SERIAL serial\n  MAJ:MIN major minor\n";

#[test]
fn selects_one_advertised_machine_provider_and_keeps_usage_support_explicit() {
    let json = capability_from_help(Some("lsblk 2.39".to_string()), HELP_JSON);
    assert_eq!(json.status, HostStorageCapabilityStatus::Supported);
    assert_eq!(json.provider, Some(HostStorageProvider::LsblkJson));
    assert!(json.can_report_filesystem_usage);

    let pairs = capability_from_help(Some("lsblk 2.23".to_string()), HELP_PAIRS);
    assert_eq!(pairs.status, HostStorageCapabilityStatus::Supported);
    assert_eq!(pairs.provider, Some(HostStorageProvider::LsblkPairs));
    assert!(!pairs.can_report_filesystem_usage);
    assert!(pairs
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("does not report")));
}

#[test]
fn refuses_unstructured_or_incomplete_lsblk_instead_of_falling_back() {
    let unstructured = capability_from_help(
        Some("lsblk 2.19".to_string()),
        "--paths\nNAME name\nTYPE type\nSIZE size\nRO read only\n",
    );
    assert_eq!(
        unstructured.status,
        HostStorageCapabilityStatus::Unsupported
    );
    assert_eq!(unstructured.provider, None);

    let incomplete = capability_from_help(
        Some("lsblk 2.20".to_string()),
        "--pairs\n--paths\nNAME name\nTYPE type\nRO read only\n",
    );
    assert_eq!(incomplete.status, HostStorageCapabilityStatus::Unsupported);
    assert!(incomplete
        .reason
        .as_deref()
        .is_some_and(|reason| reason.contains("SIZE")));
}

#[test]
fn parses_nested_json_devices_without_conflating_parent_and_mount_addresses() {
    let rows = parse_json_devices(
        r#"{"blockdevices":[{"name":"/dev/vda","kname":"vda","type":"disk","size":21474836480,"ro":false,"rm":false,"model":"Virtual Disk","maj:min":"252:0","children":[{"name":"/dev/vda1","kname":"vda1","pkname":"/dev/vda","type":"part","size":"21473787904","fstype":"ext4","fsver":"1.0","uuid":"root-id","mountpoint":"/","fsavail":"8589934592","fsuse%":"60%","ro":"0","rm":"0","maj:min":"252:1"}]}]}"#,
    )
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].parent_path.as_deref(), Some("/dev/vda"));
    assert_eq!(rows[1].mount_points, vec!["/"]);
    assert_eq!(rows[1].filesystem_used_percent, Some(60));
    assert_eq!(rows[1].filesystem_available_bytes, Some(8_589_934_592));
}

#[test]
fn parses_legacy_pairs_and_decodes_escaped_values() {
    let rows = parse_pairs_devices(
        "NAME=\"/dev/sda\" KNAME=\"sda\" PKNAME=\"\" TYPE=\"disk\" SIZE=\"1000204886016\" FSTYPE=\"\" LABEL=\"Cloud\\x20disk\" UUID=\"\" MOUNTPOINT=\"\" RO=\"0\" RM=\"0\" MODEL=\"Virtual\\x20Disk\" SERIAL=\"abc\" MAJ:MIN=\"8:0\"\n",
    )
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label.as_deref(), Some("Cloud disk"));
    assert_eq!(rows[0].model.as_deref(), Some("Virtual Disk"));
    assert_eq!(rows[0].filesystem_used_percent, None);
}

#[test]
fn parses_mountinfo_escapes_and_filters_only_explicit_pseudo_filesystems() {
    let mounts = parse_mountinfo(
        "36 25 252:1 / / rw,relatime - ext4 /dev/vda1 rw,errors=remount-ro\n37 25 0:31 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw\n38 25 0:44 / /srv/app\\040data ro,relatime - tmpfs tmpfs ro,size=1024k\n",
    )
    .unwrap();
    assert_eq!(mounts.len(), 3);
    assert_eq!(mounts[1].target, "/proc");
    assert!(mounts[1].pseudo);
    assert_eq!(mounts[2].target, "/srv/app data");
    assert!(!mounts[2].pseudo);
    assert!(mounts[2].read_only);
}

#[test]
fn malformed_machine_output_is_rejected_without_parser_fallback() {
    assert!(parse_json_devices("NAME=\"/dev/sda\"").is_err());
    assert!(parse_pairs_devices("NAME=/dev/sda").is_err());
}
