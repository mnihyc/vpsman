use std::{collections::BTreeSet, path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use chrono::{Datelike, Local, Months, NaiveDate, TimeZone, Timelike, Utc};
use serde_json::Value;
use tokio::{process::Command, sync::mpsc, time};
use vpsman_common::{
    CommandOutput, NetworkTrafficImportBatch, NetworkTrafficImportBucket,
    NetworkTrafficImportResult, NetworkTrafficImportSource, OutputStream,
    NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT, NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
    NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
};

use crate::{
    child_process::{run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult},
    command_worker::{run_cancelable, CommandCancelToken},
};

const VNSTAT_OUTPUT_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const VNSTAT_ALL_INTERFACES_OUTPUT_LIMIT_BYTES: usize =
    VNSTAT_OUTPUT_LIMIT_BYTES * NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES;
const VNSTAT_CONFIG_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const VNSTAT_STATUS_OUTPUT_LIMIT_BYTES: usize = 30 * 1024;
const VNSTAT_COMMAND_TIMEOUT_SECS: u64 = 30;
const VNSTAT_EXECUTABLE_CANDIDATES: [&str; 5] = [
    "/usr/bin/vnstat",
    "/usr/local/bin/vnstat",
    "/usr/sbin/vnstat",
    "/usr/local/sbin/vnstat",
    "/bin/vnstat",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VnstatCalendarConfig {
    month_rotate: u32,
    month_rotate_affects_years: bool,
    use_utc: bool,
    trafficless_entries: bool,
}

impl Default for VnstatCalendarConfig {
    fn default() -> Self {
        Self {
            month_rotate: 1,
            month_rotate_affects_years: false,
            use_utc: false,
            trafficless_entries: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CalendarResolution {
    Month,
    Year,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetainedResolution {
    FiveMinute,
    Hour,
    Day,
    Month,
    Year,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntervalCoverage {
    None,
    Partial,
    Full,
}

impl CalendarResolution {
    fn field(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

impl RetainedResolution {
    fn field(self) -> &'static str {
        match self {
            Self::FiveMinute => "fiveminute",
            Self::Hour => "hour",
            Self::Day => "day",
            Self::Month => "month",
            Self::Year => "year",
        }
    }
}

pub(crate) struct NetworkTrafficImportInput<'a> {
    pub(crate) job_id: uuid::Uuid,
    pub(crate) interfaces: &'a [String],
    pub(crate) start_unix: u64,
    pub(crate) max_timeout_secs: u64,
    pub(crate) cancel_token: CommandCancelToken,
    pub(crate) output_tx: Option<mpsc::Sender<CommandOutput>>,
}

pub(crate) async fn execute_network_traffic_import_command(
    input: NetworkTrafficImportInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let cancel_token = input.cancel_token.clone();
    run_cancelable("network_traffic_import_vnstat", cancel_token, async move {
        time::timeout(
            Duration::from_secs(input.max_timeout_secs.max(1)),
            collect_vnstat_history(input),
        )
        .await
        .context("network traffic import timed out")?
    })
    .await
}

async fn collect_vnstat_history(
    input: NetworkTrafficImportInput<'_>,
) -> Result<Vec<CommandOutput>> {
    let now_unix = unix_now();
    let collected_until_unix = floor_minute(now_unix);
    validate_request_at(input.interfaces, input.start_unix, now_unix)?;
    let executable = vnstat_executable()?;
    let calendar_config = run_vnstat_showconfig(executable, input.cancel_token.clone()).await?;
    tracing::debug!(
        month_rotate = calendar_config.month_rotate,
        month_rotate_affects_years = calendar_config.month_rotate_affects_years,
        use_utc = calendar_config.use_utc,
        trafficless_entries = calendar_config.trafficless_entries,
        "loaded effective vnStat calendar configuration"
    );
    let mut buckets = Vec::new();
    let mut sources = Vec::new();
    let resolved_interfaces = if input.interfaces.is_empty() {
        input.cancel_token.check("network_traffic_import_vnstat")?;
        let payload = run_vnstat_query(executable, None, input.cancel_token.clone()).await?;
        let (interfaces, discovered_sources, discovered_buckets) =
            parse_discovered_vnstat_payload(&payload, input.start_unix, &calendar_config)?;
        sources = discovered_sources;
        buckets = discovered_buckets;
        interfaces
    } else {
        for interface in input.interfaces {
            input.cancel_token.check("network_traffic_import_vnstat")?;
            let payload =
                run_vnstat_query(executable, Some(interface), input.cancel_token.clone()).await?;
            append_parsed_interface(
                &payload,
                interface,
                input.start_unix,
                &calendar_config,
                &mut buckets,
                &mut sources,
            )?;
        }
        input.interfaces.to_vec()
    };

    buckets.sort_by(|left, right| {
        left.interface
            .cmp(&right.interface)
            .then_with(|| left.start_unix.cmp(&right.start_unix))
            .then_with(|| left.duration_secs.cmp(&right.duration_secs))
    });

    let bucket_count = u32::try_from(buckets.len()).context("vnstat bucket count overflow")?;
    let batch_count = u32::try_from(
        buckets
            .len()
            .div_ceil(NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT),
    )
    .context("vnstat batch count overflow")?;
    let mut returned_outputs = Vec::new();
    for (batch_index, chunk) in buckets
        .chunks(NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT)
        .enumerate()
    {
        input.cancel_token.check("network_traffic_import_vnstat")?;
        let batch = NetworkTrafficImportBatch {
            r#type: "network_traffic_import_vnstat_batch".to_string(),
            batch_index: u32::try_from(batch_index).context("vnstat batch index overflow")?,
            buckets: chunk.to_vec(),
        };
        let data = serde_json::to_vec(&batch)?;
        anyhow::ensure!(
            data.len() <= VNSTAT_STATUS_OUTPUT_LIMIT_BYTES,
            "vnstat import batch exceeds {} bytes",
            VNSTAT_STATUS_OUTPUT_LIMIT_BYTES
        );
        emit_streamed_output(
            &input.output_tx,
            &mut returned_outputs,
            CommandOutput {
                job_id: input.job_id,
                stream: OutputStream::Status,
                data,
                exit_code: None,
                done: false,
            },
        )
        .await?;
    }

    let result = NetworkTrafficImportResult {
        r#type: "network_traffic_import_vnstat".to_string(),
        status: "collected".to_string(),
        requested_start_unix: input.start_unix,
        collected_until_unix,
        interfaces: resolved_interfaces,
        sources,
        batch_count,
        bucket_count,
        message: "vnStat history collected; API import is pending".to_string(),
    };
    let data = serde_json::to_vec(&result)?;
    anyhow::ensure!(
        data.len() <= VNSTAT_STATUS_OUTPUT_LIMIT_BYTES,
        "vnstat import result exceeds {} bytes",
        VNSTAT_STATUS_OUTPUT_LIMIT_BYTES
    );
    returned_outputs.push(CommandOutput {
        job_id: input.job_id,
        stream: OutputStream::Status,
        data,
        exit_code: Some(0),
        done: true,
    });
    Ok(returned_outputs)
}

async fn emit_streamed_output(
    output_tx: &Option<mpsc::Sender<CommandOutput>>,
    returned_outputs: &mut Vec<CommandOutput>,
    output: CommandOutput,
) -> Result<()> {
    if let Some(output_tx) = output_tx {
        output_tx
            .send(output)
            .await
            .context("network traffic import output receiver closed")?;
    } else {
        returned_outputs.push(output);
    }
    Ok(())
}

fn validate_request_at(interfaces: &[String], start_unix: u64, now_unix: u64) -> Result<()> {
    let current_minute = floor_minute(now_unix);
    anyhow::ensure!(
        interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
        "network traffic import interface count is out of range"
    );
    let mut normalized = interfaces
        .iter()
        .map(|interface| interface.trim())
        .collect::<Vec<_>>();
    anyhow::ensure!(
        normalized
            .iter()
            .all(|interface| valid_interface_name(interface)),
        "network traffic import contains an invalid interface"
    );
    normalized.sort_unstable();
    normalized.dedup();
    anyhow::ensure!(
        normalized.len() == interfaces.len(),
        "network traffic import interfaces must be unique"
    );
    anyhow::ensure!(
        start_unix >= 60 && start_unix.is_multiple_of(60),
        "network traffic import start must be UTC-minute aligned"
    );
    anyhow::ensure!(
        start_unix < current_minute,
        "network traffic import start must be before the current minute"
    );
    Ok(())
}

fn valid_interface_name(interface: &str) -> bool {
    !interface.is_empty()
        && interface.len() <= 64
        && interface
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn discover_vnstat_interfaces(payload: &Value) -> Result<Vec<String>> {
    anyhow::ensure!(
        json_version_is_two(payload),
        "vnstat JSON version 2 is required"
    );
    let entries = payload
        .get("interfaces")
        .and_then(Value::as_array)
        .context("vnstat JSON is missing interfaces")?;
    let mut interfaces = BTreeSet::new();
    for entry in entries {
        let interface = entry
            .get("name")
            .and_then(Value::as_str)
            .context("vnstat JSON interface is missing its name")?;
        anyhow::ensure!(
            valid_interface_name(interface),
            "vnstat JSON contains an invalid interface name"
        );
        interfaces.insert(interface.to_string());
    }
    anyhow::ensure!(
        !interfaces.is_empty() && interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
        "vnstat discovered interface count is out of range"
    );
    Ok(interfaces.into_iter().collect())
}

fn parse_discovered_vnstat_payload(
    payload: &Value,
    requested_start_unix: u64,
    calendar_config: &VnstatCalendarConfig,
) -> Result<(
    Vec<String>,
    Vec<NetworkTrafficImportSource>,
    Vec<NetworkTrafficImportBucket>,
)> {
    let interfaces = discover_vnstat_interfaces(payload)?;
    let mut sources = Vec::with_capacity(interfaces.len());
    let mut buckets = Vec::new();
    for interface in &interfaces {
        append_parsed_interface(
            payload,
            interface,
            requested_start_unix,
            calendar_config,
            &mut buckets,
            &mut sources,
        )?;
    }
    Ok((interfaces, sources, buckets))
}

fn append_parsed_interface(
    payload: &Value,
    interface: &str,
    requested_start_unix: u64,
    calendar_config: &VnstatCalendarConfig,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
    sources: &mut Vec<NetworkTrafficImportSource>,
) -> Result<()> {
    let (source, mut interface_buckets) =
        parse_vnstat_payload(payload, interface, requested_start_unix, calendar_config)?;
    anyhow::ensure!(
        interface_buckets.len() <= NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
        "vnstat history for {interface} exceeds the import bucket limit"
    );
    let effective_start_unix = requested_start_unix.max(source.retained_start_unix);
    anyhow::ensure!(
        source
            .source_updated_unix
            .is_some_and(|updated| floor_minute(updated) > effective_start_unix),
        "vnstat database for {interface} has no updates after the effective start"
    );
    buckets.append(&mut interface_buckets);
    sources.push(source);
    Ok(())
}

fn vnstat_executable() -> Result<&'static str> {
    VNSTAT_EXECUTABLE_CANDIDATES
        .iter()
        .copied()
        .find(|candidate| Path::new(candidate).is_file())
        .context("vnstat executable not found in standard system paths")
}

async fn run_vnstat_query(
    executable: &str,
    interface: Option<&str>,
    cancel_token: CommandCancelToken,
) -> Result<Value> {
    let command = vnstat_query_command(executable, interface);
    let output_limit = if interface.is_some() {
        VNSTAT_OUTPUT_LIMIT_BYTES
    } else {
        VNSTAT_ALL_INTERFACES_OUTPUT_LIMIT_BYTES
    };
    let output = run_vnstat_command(command, "query", output_limit, cancel_token).await?;
    serde_json::from_slice(&output).context("vnstat returned invalid JSON")
}

async fn run_vnstat_showconfig(
    executable: &str,
    cancel_token: CommandCancelToken,
) -> Result<VnstatCalendarConfig> {
    let command = vnstat_showconfig_command(executable);
    let output = run_vnstat_command(
        command,
        "configuration query",
        VNSTAT_CONFIG_OUTPUT_LIMIT_BYTES,
        cancel_token,
    )
    .await?;
    let output = std::str::from_utf8(&output).context("vnstat returned non-UTF-8 configuration")?;
    parse_vnstat_showconfig(output)
}

async fn run_vnstat_command(
    command: Command,
    operation: &str,
    output_limit_bytes: usize,
    cancel_token: CommandCancelToken,
) -> Result<Vec<u8>> {
    let result = run_child_with_bounded_output_cancelable(
        command,
        VNSTAT_COMMAND_TIMEOUT_SECS,
        output_limit_bytes,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await?;
    match result {
        ChildRunResult::Completed(output) => {
            anyhow::ensure!(
                !output.stdout_truncated && !output.stderr_truncated,
                "vnstat {operation} output exceeded {output_limit_bytes} bytes"
            );
            anyhow::ensure!(
                output.exit_code == Some(0),
                "vnstat {operation} exited with {:?}: {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            Ok(output.stdout)
        }
        ChildRunResult::TimedOut(_) => anyhow::bail!("vnstat {operation} timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("vnstat {operation} canceled: {reason}")
        }
    }
}

fn vnstat_query_command(executable: &str, interface: Option<&str>) -> Command {
    let mut command = Command::new(executable);
    command.arg("--json").arg("--limit").arg("0");
    if let Some(interface) = interface {
        command.arg("--iface").arg(interface);
    }
    command.stdin(Stdio::null());
    command
}

fn vnstat_showconfig_command(executable: &str) -> Command {
    let mut command = Command::new(executable);
    command.arg("--showconfig").stdin(Stdio::null());
    command
}

fn parse_vnstat_showconfig(output: &str) -> Result<VnstatCalendarConfig> {
    let month_rotate = parse_vnstat_config_u32(output, "MonthRotate")?;
    anyhow::ensure!(
        (1..=28).contains(&month_rotate),
        "vnstat MonthRotate is outside the supported range"
    );
    Ok(VnstatCalendarConfig {
        month_rotate,
        month_rotate_affects_years: parse_vnstat_config_bool(output, "MonthRotateAffectsYears")?,
        // UseUTC was added in vnStat 2.8. Earlier JSON-v2 databases always
        // used local time, so a missing setting has an unambiguous legacy
        // meaning while every older calendar field remains required.
        use_utc: parse_vnstat_config_optional_bool(output, "UseUTC")?.unwrap_or(false),
        trafficless_entries: parse_vnstat_config_bool(output, "TrafficlessEntries")?,
    })
}

fn parse_vnstat_config_bool(output: &str, key: &str) -> Result<bool> {
    parse_vnstat_config_optional_bool(output, key)?
        .with_context(|| format!("vnstat configuration is missing {key}"))
}

fn parse_vnstat_config_optional_bool(output: &str, key: &str) -> Result<Option<bool>> {
    let Some(value) = parse_vnstat_config_optional_u32(output, key)? else {
        return Ok(None);
    };
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => anyhow::bail!("vnstat {key} must be either 0 or 1"),
    }
    .map(Some)
}

fn parse_vnstat_config_u32(output: &str, key: &str) -> Result<u32> {
    parse_vnstat_config_optional_u32(output, key)?
        .with_context(|| format!("vnstat configuration is missing {key}"))
}

fn parse_vnstat_config_optional_u32(output: &str, key: &str) -> Result<Option<u32>> {
    let mut found = None;
    for line in output.lines() {
        let line = line.trim().strip_prefix(';').unwrap_or(line.trim()).trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        if fields.next() != Some(key) {
            continue;
        }
        anyhow::ensure!(found.is_none(), "vnstat configuration repeats {key}");
        let raw = fields
            .next()
            .with_context(|| format!("vnstat configuration is missing a value for {key}"))?;
        found = Some(
            raw.parse::<u32>()
                .with_context(|| format!("vnstat configuration has an invalid {key}"))?,
        );
    }
    Ok(found)
}

fn parse_vnstat_payload(
    payload: &Value,
    interface: &str,
    requested_start_unix: u64,
    calendar_config: &VnstatCalendarConfig,
) -> Result<(NetworkTrafficImportSource, Vec<NetworkTrafficImportBucket>)> {
    anyhow::ensure!(
        json_version_is_two(payload),
        "vnstat JSON version 2 is required"
    );
    let interface_payload = interface_payload(payload, interface)?;
    let database_created_unix = nested_timestamp(interface_payload, "created")
        .context("vnstat JSON is missing the interface creation timestamp")?;
    let database_available_unix = ceil_minute(database_created_unix)
        .context("vnstat interface creation timestamp is too large")?;
    let source_updated_unix = nested_timestamp(interface_payload, "updated")
        .context("vnstat JSON is missing the interface update timestamp")?;
    let source_cutoff_unix = floor_minute(source_updated_unix);
    let effective_start_unix = requested_start_unix.max(database_available_unix);
    anyhow::ensure!(
        source_cutoff_unix > effective_start_unix,
        "vnstat database has no completed minute after the effective start"
    );

    let mut buckets = Vec::new();
    parse_interval_rows(
        interface_payload,
        interface,
        "fiveminute",
        300,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    parse_interval_rows(
        interface_payload,
        interface,
        "hour",
        3_600,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    parse_day_rows(
        interface_payload,
        interface,
        calendar_config,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    parse_calendar_rows(
        interface_payload,
        interface,
        CalendarResolution::Month,
        calendar_config,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    parse_calendar_rows(
        interface_payload,
        interface,
        CalendarResolution::Year,
        calendar_config,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    if !calendar_config.trafficless_entries {
        synthesize_missing_trafficless_rows(
            interface_payload,
            interface,
            calendar_config,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
            &mut buckets,
        )?;
    }
    dedupe_equivalent_resolution_buckets(&mut buckets)?;
    let retained_start_unix = latest_continuous_coverage(&buckets, interface)?.0;

    Ok((
        NetworkTrafficImportSource {
            interface: interface.to_string(),
            database_created_unix: Some(database_created_unix),
            retained_start_unix,
            source_updated_unix: Some(source_updated_unix),
        },
        buckets,
    ))
}

fn latest_continuous_coverage(
    buckets: &[NetworkTrafficImportBucket],
    interface: &str,
) -> Result<(u64, u64)> {
    let mut intervals = buckets
        .iter()
        .filter(|bucket| bucket.interface == interface)
        .map(|bucket| {
            let end_unix = bucket
                .start_unix
                .checked_add(u64::from(bucket.duration_secs))
                .context("vnstat traffic interval end is too large")?;
            anyhow::ensure!(
                bucket.start_unix < end_unix
                    && bucket.start_unix.is_multiple_of(60)
                    && end_unix.is_multiple_of(60),
                "vnstat traffic interval is not minute aligned"
            );
            Ok((bucket.start_unix, end_unix))
        })
        .collect::<Result<Vec<_>>>()?;
    anyhow::ensure!(
        !intervals.is_empty(),
        "vnstat database for {interface} has no retained traffic coverage"
    );
    intervals.sort_unstable();

    let mut components = Vec::<(u64, u64)>::new();
    for (start_unix, end_unix) in intervals {
        if let Some(last) = components.last_mut() {
            if start_unix <= last.1 {
                last.1 = last.1.max(end_unix);
                continue;
            }
        }
        components.push((start_unix, end_unix));
    }
    components
        .into_iter()
        .max_by_key(|(start_unix, end_unix)| (*end_unix, std::cmp::Reverse(*start_unix)))
        .context("vnstat retained traffic coverage is empty")
}

fn dedupe_equivalent_resolution_buckets(
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    buckets.sort_by(|left, right| {
        left.start_unix
            .cmp(&right.start_unix)
            .then_with(|| left.duration_secs.cmp(&right.duration_secs))
    });
    let mut deduped = Vec::<NetworkTrafficImportBucket>::with_capacity(buckets.len());
    for bucket in buckets.drain(..) {
        if let Some(previous) = deduped.last() {
            if previous.start_unix == bucket.start_unix
                && previous.duration_secs == bucket.duration_secs
            {
                anyhow::ensure!(
                    previous.rx_bytes == bucket.rx_bytes && previous.tx_bytes == bucket.tx_bytes,
                    "vnstat overlapping resolution totals disagree for one interval"
                );
                continue;
            }
        }
        deduped.push(bucket);
    }
    *buckets = deduped;
    Ok(())
}

fn json_version_is_two(payload: &Value) -> bool {
    match payload.get("jsonversion") {
        Some(Value::String(value)) => value == "2",
        Some(Value::Number(value)) => value.as_u64() == Some(2),
        _ => false,
    }
}

fn interface_payload<'a>(payload: &'a Value, interface: &str) -> Result<&'a Value> {
    payload
        .get("interfaces")
        .and_then(Value::as_array)
        .context("vnstat JSON is missing interfaces")?
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(interface))
        .with_context(|| format!("vnstat JSON is missing interface {interface}"))
}

fn nested_timestamp(payload: &Value, field: &str) -> Option<u64> {
    payload
        .get(field)
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_u64)
}

fn parse_interval_rows(
    interface_payload: &Value,
    interface: &str,
    field: &str,
    nominal_duration_secs: u32,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let rows = traffic_rows(interface_payload, field)?;
    let mut seen = BTreeSet::new();
    for row in rows {
        let start_unix = traffic_row_timestamp(row)?;
        anyhow::ensure!(
            seen.insert(start_unix),
            "vnstat {field} rows contain a duplicate timestamp"
        );
        let end_unix = start_unix
            .checked_add(u64::from(nominal_duration_secs))
            .context("vnstat traffic interval end is too large")?;
        push_bucket_if_relevant(
            buckets,
            interface,
            row,
            start_unix,
            end_unix,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
        )?;
    }
    Ok(())
}

fn parse_day_rows(
    interface_payload: &Value,
    interface: &str,
    calendar_config: &VnstatCalendarConfig,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let mut rows = traffic_rows(interface_payload, "day")?
        .iter()
        .map(|row| traffic_row_timestamp(row).map(|timestamp| (timestamp, row)))
        .collect::<Result<Vec<_>>>()?;
    rows.sort_by_key(|(timestamp, _)| *timestamp);
    anyhow::ensure!(
        rows.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "vnstat day rows contain a duplicate timestamp"
    );
    for (start_unix, row) in rows {
        let end_unix = calendar_day_end_unix(start_unix, calendar_config.use_utc)?;
        push_bucket_if_relevant(
            buckets,
            interface,
            row,
            start_unix,
            end_unix,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn synthesize_missing_trafficless_rows(
    interface_payload: &Value,
    interface: &str,
    calendar_config: &VnstatCalendarConfig,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let resolution = [
        RetainedResolution::Year,
        RetainedResolution::Month,
        RetainedResolution::Day,
        RetainedResolution::Hour,
        RetainedResolution::FiveMinute,
    ]
    .into_iter()
    .find(|resolution| {
        traffic_rows(interface_payload, resolution.field()).is_ok_and(|rows| !rows.is_empty())
    });
    let Some(resolution) = resolution else {
        return Ok(());
    };

    match resolution {
        RetainedResolution::Year => synthesize_missing_calendar_rows(
            interface_payload,
            interface,
            CalendarResolution::Year,
            calendar_config,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
            buckets,
        ),
        RetainedResolution::Month => synthesize_missing_calendar_rows(
            interface_payload,
            interface,
            CalendarResolution::Month,
            calendar_config,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
            buckets,
        ),
        RetainedResolution::Day => synthesize_missing_day_rows(
            interface_payload,
            interface,
            calendar_config,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
            buckets,
        ),
        RetainedResolution::Hour => synthesize_missing_fixed_rows(
            interface_payload,
            interface,
            "hour",
            3_600,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
            buckets,
        ),
        RetainedResolution::FiveMinute => synthesize_missing_fixed_rows(
            interface_payload,
            interface,
            "fiveminute",
            300,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
            buckets,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn synthesize_missing_calendar_rows(
    interface_payload: &Value,
    interface: &str,
    resolution: CalendarResolution,
    calendar_config: &VnstatCalendarConfig,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let rows = traffic_rows(interface_payload, resolution.field())?;
    let present_period_starts = rows
        .iter()
        .map(|row| {
            calendar_period_bounds(traffic_row_timestamp(row)?, resolution, calendar_config)
                .map(|(start, _)| start)
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let first_label_unix = rows
        .iter()
        .map(traffic_row_timestamp)
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .min()
        .context("vnstat retained calendar resolution is empty")?;
    // The oldest retained row proves the start of known coverage. Absence
    // before it may be retention expiry and must not be fabricated as zero.
    let mut label_date =
        calendar_label_date(first_label_unix, resolution, calendar_config.use_utc)?;
    let step_months = Months::new(match resolution {
        CalendarResolution::Month => 1,
        CalendarResolution::Year => 12,
    });

    loop {
        let label_unix = calendar_midnight_unix(label_date, calendar_config.use_utc)?;
        let (period_start_unix, period_end_unix) =
            calendar_period_bounds(label_unix, resolution, calendar_config)?;
        if period_start_unix >= source_cutoff_unix {
            break;
        }
        if !present_period_starts.contains(&period_start_unix)
            && period_end_unix > database_available_unix
        {
            push_known_zero_bucket(
                buckets,
                interface,
                period_start_unix,
                period_end_unix,
                database_available_unix,
                requested_start_unix,
                source_cutoff_unix,
            )?;
        }
        label_date = label_date
            .checked_add_months(step_months)
            .context("vnstat calendar period is out of range")?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn synthesize_missing_day_rows(
    interface_payload: &Value,
    interface: &str,
    calendar_config: &VnstatCalendarConfig,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let present_starts = traffic_rows(interface_payload, "day")?
        .iter()
        .map(traffic_row_timestamp)
        .collect::<Result<BTreeSet<_>>>()?;
    let mut period_start_unix = *present_starts
        .first()
        .context("vnstat retained day resolution is empty")?;
    while period_start_unix < source_cutoff_unix {
        let period_end_unix = calendar_day_end_unix(period_start_unix, calendar_config.use_utc)?;
        if !present_starts.contains(&period_start_unix) {
            push_known_zero_bucket(
                buckets,
                interface,
                period_start_unix,
                period_end_unix,
                database_available_unix,
                requested_start_unix,
                source_cutoff_unix,
            )?;
        }
        period_start_unix = period_end_unix;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn synthesize_missing_fixed_rows(
    interface_payload: &Value,
    interface: &str,
    field: &str,
    duration_secs: u64,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let present_starts = traffic_rows(interface_payload, field)?
        .iter()
        .map(traffic_row_timestamp)
        .collect::<Result<BTreeSet<_>>>()?;
    let mut period_start_unix = *present_starts
        .first()
        .with_context(|| format!("vnstat retained {field} resolution is empty"))?;
    anyhow::ensure!(
        present_starts
            .iter()
            .all(|start| (start - period_start_unix).is_multiple_of(duration_secs)),
        "vnstat {field} rows are not on one interval grid"
    );
    while period_start_unix < source_cutoff_unix {
        let period_end_unix = period_start_unix
            .checked_add(duration_secs)
            .context("vnstat traffic interval end is too large")?;
        if !present_starts.contains(&period_start_unix) {
            push_known_zero_bucket(
                buckets,
                interface,
                period_start_unix,
                period_end_unix,
                database_available_unix,
                requested_start_unix,
                source_cutoff_unix,
            )?;
        }
        period_start_unix = period_end_unix;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn push_known_zero_bucket(
    buckets: &mut Vec<NetworkTrafficImportBucket>,
    interface: &str,
    period_start_unix: u64,
    period_end_unix: u64,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
) -> Result<()> {
    push_explicit_bucket_if_relevant(
        buckets,
        interface,
        period_start_unix,
        period_end_unix,
        0,
        0,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
    )?;
    anyhow::ensure!(
        buckets.len() <= NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
        "vnstat history for {interface} exceeds the import bucket limit"
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn parse_calendar_rows(
    interface_payload: &Value,
    interface: &str,
    resolution: CalendarResolution,
    calendar_config: &VnstatCalendarConfig,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let field = resolution.field();
    let mut rows = traffic_rows(interface_payload, field)?
        .iter()
        .map(|row| traffic_row_timestamp(row).map(|timestamp| (timestamp, row)))
        .collect::<Result<Vec<_>>>()?;
    rows.sort_by_key(|(timestamp, _)| *timestamp);
    anyhow::ensure!(
        rows.windows(2).all(|pair| pair[0].0 != pair[1].0),
        "vnstat {field} rows contain a duplicate timestamp"
    );

    for (label_unix, row) in rows {
        let (period_start_unix, period_end_unix) =
            calendar_period_bounds(label_unix, resolution, calendar_config)?;
        let effective_start_unix = period_start_unix.max(database_available_unix);
        let available_end_unix = period_end_unix.min(source_cutoff_unix);
        if effective_start_unix >= available_end_unix {
            continue;
        }
        let crosses_unrotated_year = resolution == CalendarResolution::Month
            && calendar_config.month_rotate > 1
            && !calendar_config.month_rotate_affects_years
            && interval_crosses_calendar_year(
                effective_start_unix,
                available_end_unix,
                calendar_config.use_utc,
            )?;
        if crosses_unrotated_year {
            let relevant_start_unix = effective_start_unix.max(requested_start_unix);
            match calendar_resolution_coverage(
                interface_payload,
                CalendarResolution::Year,
                calendar_config,
                database_available_unix,
                relevant_start_unix,
                available_end_unix,
                source_cutoff_unix,
            )? {
                IntervalCoverage::Full => {
                    // A rotated month crossing Jan 1 is not nested inside
                    // vnStat's unrotated year aggregate. When year rows cover
                    // the requested span, omitting this month keeps finer and
                    // coarser aggregate totals reconcilable.
                    continue;
                }
                IntervalCoverage::None => {}
                IntervalCoverage::Partial => anyhow::bail!(
                    "vnstat year rows only partially cover a rotated month crossing a year boundary"
                ),
            }
        }
        push_bucket_if_relevant(
            buckets,
            interface,
            row,
            period_start_unix,
            period_end_unix,
            database_available_unix,
            requested_start_unix,
            source_cutoff_unix,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn calendar_resolution_coverage(
    interface_payload: &Value,
    resolution: CalendarResolution,
    calendar_config: &VnstatCalendarConfig,
    database_available_unix: u64,
    range_start_unix: u64,
    range_end_unix: u64,
    source_cutoff_unix: u64,
) -> Result<IntervalCoverage> {
    if range_start_unix >= range_end_unix {
        return Ok(IntervalCoverage::Full);
    }
    let field = resolution.field();
    let mut labels = traffic_rows(interface_payload, field)?
        .iter()
        .map(traffic_row_timestamp)
        .collect::<Result<Vec<_>>>()?;
    labels.sort_unstable();
    anyhow::ensure!(
        labels.windows(2).all(|pair| pair[0] != pair[1]),
        "vnstat {field} rows contain a duplicate timestamp"
    );

    let mut cursor = range_start_unix;
    let mut saw_overlap = false;
    let mut saw_gap = false;
    let mut first_overlap_start_unix = None;
    for label_unix in labels {
        let (period_start_unix, period_end_unix) =
            calendar_period_bounds(label_unix, resolution, calendar_config)?;
        let interval_start_unix = period_start_unix.max(database_available_unix);
        let interval_end_unix = period_end_unix.min(source_cutoff_unix);
        let overlap_start_unix = interval_start_unix.max(range_start_unix);
        let overlap_end_unix = interval_end_unix.min(range_end_unix);
        if overlap_start_unix >= overlap_end_unix {
            continue;
        }
        first_overlap_start_unix.get_or_insert(overlap_start_unix);
        saw_overlap = true;
        if overlap_start_unix > cursor {
            saw_gap = true;
        }
        cursor = cursor.max(overlap_end_unix);
    }

    if !saw_overlap {
        Ok(IntervalCoverage::None)
    } else if resolution == CalendarResolution::Year && !calendar_config.trafficless_entries {
        // With TrafficlessEntries disabled, missing yearly rows denote known
        // zero periods after the first retained year. A missing prefix may be
        // retention, so it cannot safely displace the monthly fallback.
        if first_overlap_start_unix == Some(range_start_unix) {
            Ok(IntervalCoverage::Full)
        } else {
            Ok(IntervalCoverage::Partial)
        }
    } else if !saw_gap && cursor >= range_end_unix {
        Ok(IntervalCoverage::Full)
    } else {
        Ok(IntervalCoverage::Partial)
    }
}

fn traffic_rows<'a>(interface_payload: &'a Value, field: &str) -> Result<&'a [Value]> {
    Ok(interface_payload
        .get("traffic")
        .and_then(|traffic| traffic.get(field))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default())
}

fn traffic_row_timestamp(row: &Value) -> Result<u64> {
    let start_unix = row
        .get("timestamp")
        .and_then(Value::as_u64)
        .context("vnstat traffic row is missing timestamp")?;
    anyhow::ensure!(
        start_unix % 60 == 0,
        "vnstat traffic timestamp is not minute aligned"
    );
    Ok(start_unix)
}

fn calendar_period_bounds(
    label_unix: u64,
    resolution: CalendarResolution,
    config: &VnstatCalendarConfig,
) -> Result<(u64, u64)> {
    let label_date = calendar_label_date(label_unix, resolution, config.use_utc)?;
    let rotate_day = match resolution {
        CalendarResolution::Month => config.month_rotate,
        CalendarResolution::Year if config.month_rotate_affects_years => config.month_rotate,
        CalendarResolution::Year => 1,
    };
    let start_date = label_date
        .with_day(rotate_day)
        .context("vnstat calendar rotation day is invalid")?;
    let end_date = start_date
        .checked_add_months(Months::new(match resolution {
            CalendarResolution::Month => 1,
            CalendarResolution::Year => 12,
        }))
        .context("vnstat calendar period end is out of range")?;
    let start_unix = calendar_midnight_unix(start_date, config.use_utc)?;
    let end_unix = calendar_midnight_unix(end_date, config.use_utc)?;
    anyhow::ensure!(
        end_unix > start_unix,
        "vnstat calendar period has an invalid duration"
    );
    let duration_secs = end_unix - start_unix;
    let valid_duration = match resolution {
        CalendarResolution::Month => {
            (27 * 24 * 60 * 60..=32 * 24 * 60 * 60).contains(&duration_secs)
        }
        CalendarResolution::Year => {
            (364 * 24 * 60 * 60..=367 * 24 * 60 * 60).contains(&duration_secs)
        }
    };
    anyhow::ensure!(
        valid_duration && duration_secs.is_multiple_of(60),
        "vnstat {} row has an invalid calendar duration",
        resolution.field()
    );
    Ok((start_unix, end_unix))
}

fn calendar_label_date(
    label_unix: u64,
    resolution: CalendarResolution,
    use_utc: bool,
) -> Result<NaiveDate> {
    let label_unix = i64::try_from(label_unix).context("vnstat calendar timestamp is too large")?;
    let (date, hour, minute, second) = if use_utc {
        let value = Utc
            .timestamp_opt(label_unix, 0)
            .single()
            .context("vnstat UTC calendar timestamp is invalid")?;
        (
            value.date_naive(),
            value.hour(),
            value.minute(),
            value.second(),
        )
    } else {
        let value = Local
            .timestamp_opt(label_unix, 0)
            .single()
            .context("vnstat local calendar timestamp is invalid")?;
        (
            value.date_naive(),
            value.hour(),
            value.minute(),
            value.second(),
        )
    };
    anyhow::ensure!(
        hour == 0
            && minute == 0
            && second == 0
            && date.day() == 1
            && (resolution == CalendarResolution::Month || date.month() == 1),
        "vnstat {} row is not labeled at its calendar boundary",
        resolution.field()
    );
    Ok(date)
}

fn calendar_day_end_unix(start_unix: u64, use_utc: bool) -> Result<u64> {
    if use_utc {
        calendar_day_end_unix_in_timezone(start_unix, &Utc)
    } else {
        calendar_day_end_unix_in_timezone(start_unix, &Local)
    }
}

fn calendar_day_end_unix_in_timezone<Tz: TimeZone>(start_unix: u64, timezone: &Tz) -> Result<u64> {
    let start_unix_i64 = i64::try_from(start_unix).context("vnstat day timestamp is too large")?;
    let value = timezone
        .timestamp_opt(start_unix_i64, 0)
        .single()
        .context("vnstat day timestamp is invalid")?;
    anyhow::ensure!(
        value.hour() == 0 && value.minute() == 0 && value.second() == 0,
        "vnstat day row is not labeled at calendar midnight"
    );
    let end_date = value
        .date_naive()
        .succ_opt()
        .context("vnstat day interval end is out of range")?;
    let end_midnight = end_date
        .and_hms_opt(0, 0, 0)
        .context("vnstat day interval end is invalid")?;
    let end_unix = u64::try_from(
        timezone
            .from_local_datetime(&end_midnight)
            .single()
            .context("vnstat calendar midnight is ambiguous or unavailable")?
            .timestamp(),
    )
    .context("vnstat day interval end predates the Unix epoch")?;
    let duration_secs = end_unix
        .checked_sub(start_unix)
        .context("vnstat day interval has an invalid duration")?;
    anyhow::ensure!(
        (23 * 60 * 60..=25 * 60 * 60).contains(&duration_secs) && duration_secs.is_multiple_of(60),
        "vnstat day row has an invalid calendar duration"
    );
    Ok(end_unix)
}

fn calendar_midnight_unix(date: NaiveDate, use_utc: bool) -> Result<u64> {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .context("vnstat calendar midnight is invalid")?;
    let timestamp = if use_utc {
        Utc.from_utc_datetime(&midnight).timestamp()
    } else {
        Local
            .from_local_datetime(&midnight)
            .single()
            .context("vnstat local calendar midnight is ambiguous or unavailable")?
            .timestamp()
    };
    u64::try_from(timestamp).context("vnstat calendar timestamp predates the Unix epoch")
}

fn interval_crosses_calendar_year(start_unix: u64, end_unix: u64, use_utc: bool) -> Result<bool> {
    anyhow::ensure!(end_unix > start_unix, "vnstat calendar interval is empty");
    let last_minute_unix = end_unix.saturating_sub(60);
    Ok(calendar_year(start_unix, use_utc)? != calendar_year(last_minute_unix, use_utc)?)
}

fn calendar_year(unix: u64, use_utc: bool) -> Result<i32> {
    let unix = i64::try_from(unix).context("vnstat calendar timestamp is too large")?;
    if use_utc {
        Ok(Utc
            .timestamp_opt(unix, 0)
            .single()
            .context("vnstat UTC calendar timestamp is invalid")?
            .year())
    } else {
        Ok(Local
            .timestamp_opt(unix, 0)
            .single()
            .context("vnstat local calendar timestamp is invalid")?
            .year())
    }
}

#[allow(clippy::too_many_arguments)]
fn push_bucket_if_relevant(
    buckets: &mut Vec<NetworkTrafficImportBucket>,
    interface: &str,
    row: &Value,
    nominal_start_unix: u64,
    nominal_end_unix: u64,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
) -> Result<()> {
    let rx_bytes = row
        .get("rx")
        .and_then(Value::as_u64)
        .context("vnstat traffic row is missing rx")?;
    let tx_bytes = row
        .get("tx")
        .and_then(Value::as_u64)
        .context("vnstat traffic row is missing tx")?;
    push_explicit_bucket_if_relevant(
        buckets,
        interface,
        nominal_start_unix,
        nominal_end_unix,
        rx_bytes,
        tx_bytes,
        database_available_unix,
        requested_start_unix,
        source_cutoff_unix,
    )
}

#[allow(clippy::too_many_arguments)]
fn push_explicit_bucket_if_relevant(
    buckets: &mut Vec<NetworkTrafficImportBucket>,
    interface: &str,
    nominal_start_unix: u64,
    nominal_end_unix: u64,
    rx_bytes: u64,
    tx_bytes: u64,
    database_available_unix: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
) -> Result<()> {
    let start_unix = nominal_start_unix.max(database_available_unix);
    let available_end_unix = nominal_end_unix.min(source_cutoff_unix);
    if available_end_unix <= start_unix || available_end_unix <= requested_start_unix {
        return Ok(());
    }
    let duration_secs = available_end_unix - start_unix;
    anyhow::ensure!(
        duration_secs.is_multiple_of(60),
        "vnstat traffic interval is not minute aligned"
    );
    let duration_secs = u32::try_from(duration_secs).context("vnstat interval is too long")?;
    buckets.push(NetworkTrafficImportBucket {
        interface: interface.to_string(),
        start_unix,
        duration_secs,
        rx_bytes,
        tx_bytes,
    });
    Ok(())
}

fn floor_minute(unix: u64) -> u64 {
    unix - unix % 60
}

fn ceil_minute(unix: u64) -> Option<u64> {
    unix.checked_add(59).map(floor_minute)
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "tests_network_traffic_import.rs"]
mod tests;
