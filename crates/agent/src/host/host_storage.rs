use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::{process::Command, time};
use vpsman_common::{
    CommandOutput, HostBlockDeviceRecord, HostMountRecord, HostStorageCapability,
    HostStorageCapabilityStatus, HostStorageProvider, HostStorageSnapshot, OutputStream,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::CommandCancelToken,
};

const MAX_STORAGE_COMMAND_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_STORAGE_OUTPUT_CHUNK_BYTES: usize = 64 * 1024;
const MAX_MOUNTINFO_BYTES: u64 = 4 * 1024 * 1024;
const STORAGE_COLUMNS: &[&str] = &[
    "NAME",
    "KNAME",
    "PKNAME",
    "TYPE",
    "SIZE",
    "FSTYPE",
    "FSVER",
    "LABEL",
    "UUID",
    "MOUNTPOINT",
    "FSAVAIL",
    "FSUSE%",
    "RO",
    "RM",
    "MODEL",
    "SERIAL",
    "TRAN",
    "MAJ:MIN",
];
const REQUIRED_STORAGE_COLUMNS: &[&str] = &["NAME", "TYPE", "SIZE", "RO"];

#[derive(Clone, Debug)]
struct StorageEnvironment {
    lsblk: Option<PathBuf>,
    mountinfo: PathBuf,
}

impl StorageEnvironment {
    fn discover() -> Self {
        Self {
            lsblk: resolve_executable(&[
                "/usr/bin/lsblk",
                "/bin/lsblk",
                "/usr/sbin/lsblk",
                "/sbin/lsblk",
            ]),
            mountinfo: PathBuf::from("/proc/self/mountinfo"),
        }
    }
}

#[derive(Clone, Debug)]
struct CommandResult {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

pub(crate) async fn execute_storage_inventory(
    job_id: uuid::Uuid,
    include_pseudo_mounts: bool,
    limit: u16,
    max_timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<Vec<CommandOutput>> {
    let snapshot = time::timeout(
        Duration::from_secs(max_timeout_secs.max(1)),
        collect_storage_snapshot(
            StorageEnvironment::discover(),
            include_pseudo_mounts,
            limit.clamp(1, 2048),
            cancel_token,
        ),
    )
    .await
    .context("storage inventory timed out")??;
    let payload = serde_json::to_vec(&snapshot)?;
    let mut outputs = chunked_output(job_id, OutputStream::Stdout, &payload);
    outputs.push(CommandOutput {
        job_id,
        stream: OutputStream::Status,
        data: serde_json::to_vec(&serde_json::json!({
            "type": "storage_inventory",
            "capability_status": snapshot.capability.status,
            "provider": snapshot.capability.provider,
            "device_count": snapshot.devices.len(),
            "mount_count": snapshot.mounts.len(),
            "devices_truncated": snapshot.devices_truncated,
            "mounts_truncated": snapshot.mounts_truncated,
        }))?,
        exit_code: Some(0),
        done: true,
    });
    Ok(outputs)
}

async fn collect_storage_snapshot(
    environment: StorageEnvironment,
    include_pseudo_mounts: bool,
    limit: u16,
    cancel_token: CommandCancelToken,
) -> Result<HostStorageSnapshot> {
    cancel_token.check("storage_inventory")?;
    let capability = probe_storage_capability(&environment, cancel_token.clone()).await;
    if cancel_token.is_canceled() {
        cancel_token.check("storage_inventory")?;
    }
    if !capability.supported() {
        return Ok(empty_snapshot(capability, include_pseudo_mounts));
    }

    let collection = collect_supported_storage(
        &environment,
        &capability,
        include_pseudo_mounts,
        limit,
        cancel_token.clone(),
    )
    .await;
    cancel_token.check("storage_inventory")?;
    match collection {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => Ok(empty_snapshot(
            HostStorageCapability {
                status: HostStorageCapabilityStatus::ProbeFailed,
                reason: Some(format!("storage inventory failed: {error:#}")),
                ..capability
            },
            include_pseudo_mounts,
        )),
    }
}

async fn probe_storage_capability(
    environment: &StorageEnvironment,
    cancel_token: CommandCancelToken,
) -> HostStorageCapability {
    let Some(lsblk) = environment.lsblk.as_deref() else {
        return HostStorageCapability {
            status: HostStorageCapabilityStatus::Unsupported,
            reason: Some(
                "lsblk is not installed in a standard executable path or PATH".to_string(),
            ),
            ..HostStorageCapability::default()
        };
    };
    let version = match run_command(lsblk, &["--version"], 5, cancel_token.clone()).await {
        Ok(result) => match command_text("lsblk --version", &result) {
            Ok(value) => first_nonempty_line(&value),
            Err(error) => {
                return HostStorageCapability {
                    status: HostStorageCapabilityStatus::ProbeFailed,
                    reason: Some(error.to_string()),
                    ..HostStorageCapability::default()
                };
            }
        },
        Err(error) => {
            return HostStorageCapability {
                status: HostStorageCapabilityStatus::ProbeFailed,
                reason: Some(format!("cannot execute lsblk --version: {error:#}")),
                ..HostStorageCapability::default()
            };
        }
    };
    let help = match run_command(lsblk, &["--help"], 5, cancel_token).await {
        Ok(result) => match command_text("lsblk --help", &result) {
            Ok(value) => value,
            Err(error) => {
                return HostStorageCapability {
                    status: HostStorageCapabilityStatus::ProbeFailed,
                    provider_version: version,
                    reason: Some(error.to_string()),
                    ..HostStorageCapability::default()
                };
            }
        },
        Err(error) => {
            return HostStorageCapability {
                status: HostStorageCapabilityStatus::ProbeFailed,
                provider_version: version,
                reason: Some(format!("cannot execute lsblk --help: {error:#}")),
                ..HostStorageCapability::default()
            };
        }
    };
    capability_from_help(version, &help)
}

fn capability_from_help(version: Option<String>, help: &str) -> HostStorageCapability {
    let available = available_storage_columns(help);
    let missing = REQUIRED_STORAGE_COLUMNS
        .iter()
        .filter(|column| !available.contains(**column))
        .copied()
        .collect::<Vec<_>>();
    let provider = if help.contains("--json") {
        Some(HostStorageProvider::LsblkJson)
    } else if help.contains("--pairs") {
        Some(HostStorageProvider::LsblkPairs)
    } else {
        None
    };
    let mut capability = HostStorageCapability {
        status: HostStorageCapabilityStatus::Unsupported,
        provider,
        provider_version: version,
        available_columns: available.iter().cloned().collect(),
        can_report_filesystem_usage: available.contains("FSAVAIL") && available.contains("FSUSE%"),
        reason: None,
    };
    if !help.contains("--paths") {
        capability.reason = Some(
            "installed lsblk does not advertise --paths; stable device paths cannot be requested"
                .to_string(),
        );
    } else if provider.is_none() {
        capability.reason =
            Some("installed lsblk advertises neither JSON nor key/value pairs output".to_string());
    } else if !missing.is_empty() {
        capability.reason = Some(format!(
            "installed lsblk is missing required output columns: {}",
            missing.join(", ")
        ));
    } else {
        capability.status = HostStorageCapabilityStatus::Supported;
        if !capability.can_report_filesystem_usage {
            capability.reason = Some(
                "device inventory is supported; this lsblk version does not report FSAVAIL and FSUSE%"
                    .to_string(),
            );
        }
    }
    capability
}

async fn collect_supported_storage(
    environment: &StorageEnvironment,
    capability: &HostStorageCapability,
    include_pseudo_mounts: bool,
    limit: u16,
    cancel_token: CommandCancelToken,
) -> Result<HostStorageSnapshot> {
    let binary = environment
        .lsblk
        .as_deref()
        .context("lsblk disappeared after capability probe")?;
    let provider = capability
        .provider
        .context("supported storage provider is missing")?;
    let selected_columns = STORAGE_COLUMNS
        .iter()
        .filter(|column| {
            capability
                .available_columns
                .iter()
                .any(|item| item == **column)
        })
        .copied()
        .collect::<Vec<_>>();
    let output_format = match provider {
        HostStorageProvider::LsblkJson => "-J",
        HostStorageProvider::LsblkPairs => "-P",
    };
    let columns = selected_columns.join(",");
    let result = run_command(
        binary,
        &["-b", "-p", output_format, "-o", columns.as_str()],
        20,
        cancel_token.clone(),
    )
    .await?;
    let output = command_text("lsblk inventory", &result)?;
    let mut devices = match provider {
        HostStorageProvider::LsblkJson => parse_json_devices(&output)?,
        HostStorageProvider::LsblkPairs => parse_pairs_devices(&output)?,
    };

    let mountinfo_metadata = std::fs::metadata(&environment.mountinfo)
        .with_context(|| format!("cannot stat {}", environment.mountinfo.display()))?;
    if mountinfo_metadata.len() > MAX_MOUNTINFO_BYTES {
        anyhow::bail!(
            "mountinfo exceeds the {} byte safety limit",
            MAX_MOUNTINFO_BYTES
        );
    }
    let mountinfo = std::fs::read_to_string(&environment.mountinfo)
        .with_context(|| format!("cannot read {}", environment.mountinfo.display()))?;
    let mut mounts = parse_mountinfo(&mountinfo)?;
    if !include_pseudo_mounts {
        mounts.retain(|mount| !mount.pseudo);
    }
    merge_device_mounts(&mut devices, &mounts);
    devices.sort_by(|left, right| left.path.cmp(&right.path));
    mounts.sort_by(|left, right| left.target.cmp(&right.target));

    let limit = limit as usize;
    let devices_truncated = devices.len() > limit;
    let mounts_truncated = mounts.len() > limit;
    devices.truncate(limit);
    mounts.truncate(limit);
    cancel_token.check("storage_inventory")?;
    Ok(HostStorageSnapshot {
        r#type: "storage_inventory".to_string(),
        capability: capability.clone(),
        include_pseudo_mounts,
        devices_truncated,
        mounts_truncated,
        devices,
        mounts,
    })
}

fn empty_snapshot(
    capability: HostStorageCapability,
    include_pseudo_mounts: bool,
) -> HostStorageSnapshot {
    HostStorageSnapshot {
        r#type: "storage_inventory".to_string(),
        capability,
        include_pseudo_mounts,
        devices_truncated: false,
        mounts_truncated: false,
        devices: Vec::new(),
        mounts: Vec::new(),
    }
}

fn available_storage_columns(help: &str) -> BTreeSet<String> {
    let known = STORAGE_COLUMNS.iter().copied().collect::<BTreeSet<_>>();
    help.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(|token| token.trim_matches(','))
        .filter(|token| known.contains(token))
        .map(str::to_string)
        .collect()
}

fn parse_json_devices(output: &str) -> Result<Vec<HostBlockDeviceRecord>> {
    let document: Value = serde_json::from_str(output).context("lsblk JSON is malformed")?;
    let rows = document
        .get("blockdevices")
        .and_then(Value::as_array)
        .context("lsblk JSON is missing blockdevices")?;
    let mut devices = Vec::new();
    for row in rows {
        append_json_device(row, None, &mut devices)?;
    }
    Ok(devices)
}

fn append_json_device(
    row: &Value,
    nested_parent: Option<&str>,
    devices: &mut Vec<HostBlockDeviceRecord>,
) -> Result<()> {
    let object = row.as_object().context("lsblk JSON row is not an object")?;
    let path = required_json_string(object.get("name"), "NAME")?;
    let parent_path = optional_json_string(object.get("pkname"))
        .map(normalize_device_path)
        .or_else(|| nested_parent.map(str::to_string));
    let mount_points = optional_json_string(object.get("mountpoint"))
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect();
    let device = HostBlockDeviceRecord {
        name: device_name(&path),
        path: path.clone(),
        kernel_name: optional_json_string(object.get("kname")),
        parent_path,
        device_type: required_json_string(object.get("type"), "TYPE")?,
        size_bytes: required_json_u64(object.get("size"), "SIZE")?,
        filesystem_type: optional_json_string(object.get("fstype")),
        filesystem_version: optional_json_string(object.get("fsver")),
        label: optional_json_string(object.get("label")),
        uuid: optional_json_string(object.get("uuid")),
        mount_points,
        filesystem_available_bytes: optional_json_u64(object.get("fsavail"), "FSAVAIL")?,
        filesystem_used_percent: optional_json_percent(object.get("fsuse%"), "FSUSE%")?,
        read_only: required_json_bool(object.get("ro"), "RO")?,
        removable: optional_json_bool(object.get("rm"), "RM")?.unwrap_or(false),
        model: optional_json_string(object.get("model")),
        serial: optional_json_string(object.get("serial")),
        transport: optional_json_string(object.get("tran")),
        major_minor: optional_json_string(object.get("maj:min")),
    };
    devices.push(device);
    if let Some(children) = object.get("children").and_then(Value::as_array) {
        for child in children {
            append_json_device(child, Some(&path), devices)?;
        }
    }
    Ok(())
}

fn parse_pairs_devices(output: &str) -> Result<Vec<HostBlockDeviceRecord>> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let values = parse_pairs_line(line)?;
            let path = required_pair(&values, "NAME")?.to_string();
            let mount_points = values
                .get("MOUNTPOINT")
                .filter(|value| !value.is_empty())
                .cloned()
                .into_iter()
                .collect();
            Ok(HostBlockDeviceRecord {
                name: device_name(&path),
                path,
                kernel_name: optional_pair(&values, "KNAME"),
                parent_path: optional_pair(&values, "PKNAME").map(normalize_device_path),
                device_type: required_pair(&values, "TYPE")?.to_string(),
                size_bytes: parse_required_u64(required_pair(&values, "SIZE")?, "SIZE")?,
                filesystem_type: optional_pair(&values, "FSTYPE"),
                filesystem_version: optional_pair(&values, "FSVER"),
                label: optional_pair(&values, "LABEL"),
                uuid: optional_pair(&values, "UUID"),
                mount_points,
                filesystem_available_bytes: parse_optional_u64(
                    values.get("FSAVAIL").map(String::as_str),
                    "FSAVAIL",
                )?,
                filesystem_used_percent: parse_optional_percent(
                    values.get("FSUSE%").map(String::as_str),
                    "FSUSE%",
                )?,
                read_only: parse_required_bool(required_pair(&values, "RO")?, "RO")?,
                removable: values
                    .get("RM")
                    .filter(|value| !value.is_empty())
                    .map(|value| parse_required_bool(value, "RM"))
                    .transpose()?
                    .unwrap_or(false),
                model: optional_pair(&values, "MODEL"),
                serial: optional_pair(&values, "SERIAL"),
                transport: optional_pair(&values, "TRAN"),
                major_minor: optional_pair(&values, "MAJ:MIN"),
            })
        })
        .collect()
}

fn parse_pairs_line(line: &str) -> Result<BTreeMap<String, String>> {
    let bytes = line.as_bytes();
    let mut index = 0;
    let mut values = BTreeMap::new();
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        let key_start = index;
        while index < bytes.len() && bytes[index] != b'=' {
            index += 1;
        }
        if index == bytes.len() || key_start == index {
            anyhow::bail!("lsblk pairs row has an invalid key/value boundary");
        }
        let key = std::str::from_utf8(&bytes[key_start..index])
            .context("lsblk pairs key is not UTF-8")?
            .to_string();
        index += 1;
        if bytes.get(index) != Some(&b'\"') {
            anyhow::bail!("lsblk pairs value for {key} is not quoted");
        }
        index += 1;
        let mut decoded = Vec::new();
        let mut closed = false;
        while index < bytes.len() {
            match bytes[index] {
                b'\"' => {
                    index += 1;
                    closed = true;
                    break;
                }
                b'\\' if bytes.get(index + 1) == Some(&b'x') && index + 3 < bytes.len() => {
                    let high = hex_nibble(bytes[index + 2])
                        .context("lsblk pairs value has an invalid hex escape")?;
                    let low = hex_nibble(bytes[index + 3])
                        .context("lsblk pairs value has an invalid hex escape")?;
                    decoded.push((high << 4) | low);
                    index += 4;
                }
                b'\\' if index + 1 < bytes.len() => {
                    decoded.push(bytes[index + 1]);
                    index += 2;
                }
                byte => {
                    decoded.push(byte);
                    index += 1;
                }
            }
        }
        if !closed {
            anyhow::bail!("lsblk pairs value for {key} is not terminated");
        }
        values.insert(key, String::from_utf8_lossy(&decoded).into_owned());
    }
    Ok(values)
}

fn parse_mountinfo(output: &str) -> Result<Vec<HostMountRecord>> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_mountinfo_line)
        .collect()
}

fn parse_mountinfo_line(line: &str) -> Result<HostMountRecord> {
    let (left, right) = line
        .split_once(" - ")
        .context("mountinfo row is missing the field separator")?;
    let left = left.split_whitespace().collect::<Vec<_>>();
    let right = right.split_whitespace().collect::<Vec<_>>();
    if left.len() < 6 || right.len() < 3 {
        anyhow::bail!("mountinfo row is missing required fields");
    }
    let mut options = left[5]
        .split(',')
        .chain(right[2].split(','))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    options.sort();
    options.dedup();
    let filesystem_type = decode_mountinfo_field(right[0])?;
    Ok(HostMountRecord {
        mount_id: left[0].parse().context("mountinfo mount ID is invalid")?,
        parent_id: left[1].parse().context("mountinfo parent ID is invalid")?,
        major_minor: left[2].to_string(),
        root: decode_mountinfo_field(left[3])?,
        target: decode_mountinfo_field(left[4])?,
        source: decode_mountinfo_field(right[1])?,
        read_only: options.iter().any(|option| option == "ro"),
        pseudo: is_pseudo_filesystem(&filesystem_type),
        filesystem_type,
        options,
    })
}

fn decode_mountinfo_field(value: &str) -> Result<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let octal = &bytes[index + 1..index + 4];
            if octal.iter().all(|byte| matches!(byte, b'0'..=b'7')) {
                output.push((octal[0] - b'0') * 64 + (octal[1] - b'0') * 8 + octal[2] - b'0');
                index += 4;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(output).context("decoded mountinfo field is not UTF-8")
}

fn is_pseudo_filesystem(filesystem_type: &str) -> bool {
    matches!(
        filesystem_type,
        "proc"
            | "sysfs"
            | "devtmpfs"
            | "devpts"
            | "cgroup"
            | "cgroup2"
            | "pstore"
            | "securityfs"
            | "debugfs"
            | "tracefs"
            | "configfs"
            | "mqueue"
            | "hugetlbfs"
            | "fusectl"
            | "autofs"
            | "binfmt_misc"
            | "nsfs"
            | "rpc_pipefs"
    )
}

fn merge_device_mounts(devices: &mut [HostBlockDeviceRecord], mounts: &[HostMountRecord]) {
    for device in devices {
        for mount in mounts {
            let same_device = device.major_minor.as_deref() == Some(mount.major_minor.as_str())
                || mount.source == device.path;
            if same_device && !device.mount_points.contains(&mount.target) {
                device.mount_points.push(mount.target.clone());
            }
        }
        device.mount_points.sort();
        device.mount_points.dedup();
    }
}

fn required_json_string(value: Option<&Value>, column: &str) -> Result<String> {
    optional_json_string(value)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("lsblk JSON row is missing {column}"))
}

fn optional_json_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Null => None,
        Value::String(value) => (!value.is_empty()).then(|| value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_string()),
        _ => None,
    }
}

fn required_json_u64(value: Option<&Value>, column: &str) -> Result<u64> {
    optional_json_u64(value, column)?.with_context(|| format!("lsblk JSON row is missing {column}"))
}

fn optional_json_u64(value: Option<&Value>, column: &str) -> Result<Option<u64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Number(value) => value
            .as_u64()
            .map(Some)
            .with_context(|| format!("lsblk JSON {column} is not an unsigned integer")),
        Value::String(value) => parse_optional_u64(Some(value), column),
        _ => anyhow::bail!("lsblk JSON {column} has an invalid type"),
    }
}

fn required_json_bool(value: Option<&Value>, column: &str) -> Result<bool> {
    optional_json_bool(value, column)?
        .with_context(|| format!("lsblk JSON row is missing {column}"))
}

fn optional_json_bool(value: Option<&Value>, column: &str) -> Result<Option<bool>> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value {
        Value::Null => Ok(None),
        Value::Bool(value) => Ok(Some(*value)),
        Value::Number(value) => parse_required_bool(&value.to_string(), column).map(Some),
        Value::String(value) if value.is_empty() => Ok(None),
        Value::String(value) => parse_required_bool(value, column).map(Some),
        _ => anyhow::bail!("lsblk JSON {column} has an invalid type"),
    }
}

fn optional_json_percent(value: Option<&Value>, column: &str) -> Result<Option<u8>> {
    match optional_json_string(value) {
        Some(value) => parse_optional_percent(Some(&value), column),
        None => Ok(None),
    }
}

fn required_pair<'a>(values: &'a BTreeMap<String, String>, column: &str) -> Result<&'a str> {
    values
        .get(column)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .with_context(|| format!("lsblk pairs row is missing {column}"))
}

fn optional_pair(values: &BTreeMap<String, String>, column: &str) -> Option<String> {
    values
        .get(column)
        .filter(|value| !value.is_empty())
        .cloned()
}

fn parse_required_u64(value: &str, column: &str) -> Result<u64> {
    value
        .parse()
        .with_context(|| format!("lsblk {column} is not an unsigned integer"))
}

fn parse_optional_u64(value: Option<&str>, column: &str) -> Result<Option<u64>> {
    match value.filter(|value| !value.is_empty()) {
        Some(value) => parse_required_u64(value, column).map(Some),
        None => Ok(None),
    }
}

fn parse_optional_percent(value: Option<&str>, column: &str) -> Result<Option<u8>> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let percent = value
        .trim_end_matches('%')
        .parse::<u8>()
        .with_context(|| format!("lsblk {column} is not a percentage"))?;
    if percent > 100 {
        anyhow::bail!("lsblk {column} exceeds 100 percent");
    }
    Ok(Some(percent))
}

fn parse_required_bool(value: &str, column: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "0" | "false" => Ok(false),
        "1" | "true" => Ok(true),
        _ => anyhow::bail!("lsblk {column} is not a boolean"),
    }
}

fn normalize_device_path(value: String) -> String {
    if value.starts_with('/') {
        value
    } else {
        format!("/dev/{value}")
    }
}

fn device_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_string()
}

fn first_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
}

async fn run_command(
    binary: &Path,
    args: &[&str],
    timeout_secs: u64,
    cancel_token: CommandCancelToken,
) -> Result<CommandResult> {
    let mut command = Command::new(binary);
    command.args(args);
    command.env("LC_ALL", "C");
    command.env("LANG", "C");
    command.stdin(Stdio::null());
    match run_child_with_bounded_output_cancelable(
        command,
        timeout_secs.max(1),
        MAX_STORAGE_COMMAND_OUTPUT_BYTES,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await
    .with_context(|| format!("failed to execute {}", binary.display()))?
    {
        ChildRunResult::Completed(output) => Ok(CommandResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            stdout_truncated: output.stdout_truncated,
            stderr_truncated: output.stderr_truncated,
        }),
        ChildRunResult::TimedOut(_) => {
            anyhow::bail!("host_storage_command_timed_out: {}", binary.display())
        }
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("host_storage_command_canceled: {reason}")
        }
    }
}

fn command_text(label: &str, result: &CommandResult) -> Result<String> {
    if result.exit_code != Some(0) || result.stdout_truncated || result.stderr_truncated {
        let diagnostic = first_diagnostic_line(result)
            .unwrap_or_else(|| format!("exit code {:?}", result.exit_code));
        anyhow::bail!("host_storage_command_failed: {label}: {diagnostic}");
    }
    let bytes = if result.stdout.is_empty() {
        &result.stderr
    } else {
        &result.stdout
    };
    String::from_utf8(bytes.clone()).with_context(|| format!("{label} output is not UTF-8"))
}

fn first_diagnostic_line(result: &CommandResult) -> Option<String> {
    result
        .stderr
        .split(|byte| *byte == b'\n')
        .chain(result.stdout.split(|byte| *byte == b'\n'))
        .find(|line| !line.is_empty())
        .map(|line| {
            let mut value = String::from_utf8_lossy(line).trim().to_string();
            if value.len() > 240 {
                value.truncate(237);
                value.push_str("...");
            }
            value
        })
}

fn resolve_executable(candidates: &[&str]) -> Option<PathBuf> {
    for candidate in candidates {
        let path = PathBuf::from(candidate);
        if is_executable(&path) {
            return Some(path);
        }
    }
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        for candidate in candidates {
            let Some(name) = Path::new(candidate).file_name() else {
                continue;
            };
            let path = directory.join(name);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn chunked_output(job_id: uuid::Uuid, stream: OutputStream, data: &[u8]) -> Vec<CommandOutput> {
    data.chunks(MAX_STORAGE_OUTPUT_CHUNK_BYTES)
        .map(|chunk| CommandOutput {
            job_id,
            stream,
            data: chunk.to_vec(),
            exit_code: None,
            done: false,
        })
        .collect()
}

#[cfg(test)]
#[path = "tests_host_storage.rs"]
mod tests;
