use std::{collections::BTreeSet, path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result};
use serde_json::Value;
use tokio::{process::Command, sync::mpsc, time};
use vpsman_common::{
    CommandOutput, NetworkTrafficImportBatch, NetworkTrafficImportBucket,
    NetworkTrafficImportResult, NetworkTrafficImportSource, OutputStream,
    NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
    NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES, NETWORK_TRAFFIC_IMPORT_MAX_LOOKBACK_SECS,
};

use crate::{
    child_process::{
        run_child_with_bounded_output_cancelable, ChildCleanupPolicy, ChildRunResult,
    },
    command_worker::{run_cancelable, CommandCancelToken},
};

const VNSTAT_OUTPUT_LIMIT_BYTES: usize = 4 * 1024 * 1024;
const VNSTAT_STATUS_OUTPUT_LIMIT_BYTES: usize = 30 * 1024;
const VNSTAT_COMMAND_TIMEOUT_SECS: u64 = 30;
const VNSTAT_EXECUTABLE_CANDIDATES: [&str; 5] = [
    "/usr/bin/vnstat",
    "/usr/local/bin/vnstat",
    "/usr/sbin/vnstat",
    "/usr/local/sbin/vnstat",
    "/bin/vnstat",
];

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
    let mut buckets = Vec::new();
    let mut sources = Vec::new();

    for interface in input.interfaces {
        input
            .cancel_token
            .check("network_traffic_import_vnstat")?;
        let payload = run_vnstat_query(executable, interface, input.cancel_token.clone()).await?;
        let (source, mut interface_buckets) =
            parse_vnstat_payload(&payload, interface, input.start_unix)?;
        anyhow::ensure!(
            interface_buckets.len() <= NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
            "vnstat history for {interface} exceeds the import bucket limit"
        );
        anyhow::ensure!(
            source
                .database_created_unix
                .is_some_and(|created| created <= input.start_unix),
            "vnstat database for {interface} was created after the requested start"
        );
        anyhow::ensure!(
            source
                .source_updated_unix
                .is_some_and(|updated| updated > input.start_unix),
            "vnstat database for {interface} has no updates after the requested start"
        );
        buckets.append(&mut interface_buckets);
        sources.push(source);
    }

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
        input
            .cancel_token
            .check("network_traffic_import_vnstat")?;
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
        interfaces: input.interfaces.to_vec(),
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
        !interfaces.is_empty() && interfaces.len() <= NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
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
    anyhow::ensure!(
        current_minute.saturating_sub(start_unix) <= NETWORK_TRAFFIC_IMPORT_MAX_LOOKBACK_SECS,
        "network traffic import start exceeds the lookback limit"
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

fn vnstat_executable() -> Result<&'static str> {
    VNSTAT_EXECUTABLE_CANDIDATES
        .iter()
        .copied()
        .find(|candidate| Path::new(candidate).is_file())
        .context("vnstat executable not found in standard system paths")
}

async fn run_vnstat_query(
    executable: &str,
    interface: &str,
    cancel_token: CommandCancelToken,
) -> Result<Value> {
    let mut command = Command::new(executable);
    command
        .arg("--json")
        .arg("--limit")
        .arg("0")
        .arg("--interface")
        .arg(interface)
        .stdin(Stdio::null());
    let result = run_child_with_bounded_output_cancelable(
        command,
        VNSTAT_COMMAND_TIMEOUT_SECS,
        VNSTAT_OUTPUT_LIMIT_BYTES,
        ChildCleanupPolicy::ProcessGroup,
        cancel_token,
    )
    .await?;
    let output = match result {
        ChildRunResult::Completed(output) => {
            anyhow::ensure!(
                !output.stdout_truncated && !output.stderr_truncated,
                "vnstat output exceeded {} bytes",
                VNSTAT_OUTPUT_LIMIT_BYTES
            );
            anyhow::ensure!(
                output.exit_code == Some(0),
                "vnstat exited with {:?}: {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            output.stdout
        }
        ChildRunResult::TimedOut(_) => anyhow::bail!("vnstat query timed out"),
        ChildRunResult::Canceled { reason, .. } => {
            anyhow::bail!("vnstat query canceled: {reason}")
        }
    };
    serde_json::from_slice(&output).context("vnstat returned invalid JSON")
}

fn parse_vnstat_payload(
    payload: &Value,
    interface: &str,
    requested_start_unix: u64,
) -> Result<(NetworkTrafficImportSource, Vec<NetworkTrafficImportBucket>)> {
    anyhow::ensure!(json_version_is_two(payload), "vnstat JSON version 2 is required");
    let interface_payload = interface_payload(payload, interface)?;
    let database_created_unix = nested_timestamp(interface_payload, "created");
    let source_updated_unix = nested_timestamp(interface_payload, "updated")
        .context("vnstat JSON is missing the interface update timestamp")?;
    let source_cutoff_unix = floor_minute(source_updated_unix);
    anyhow::ensure!(
        source_cutoff_unix > requested_start_unix,
        "vnstat database has no completed minute after the requested start"
    );

    let mut buckets = Vec::new();
    parse_interval_rows(
        interface_payload,
        interface,
        "fiveminute",
        300,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    parse_interval_rows(
        interface_payload,
        interface,
        "hour",
        3_600,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;
    parse_day_rows(
        interface_payload,
        interface,
        requested_start_unix,
        source_cutoff_unix,
        &mut buckets,
    )?;

    Ok((
        NetworkTrafficImportSource {
            interface: interface.to_string(),
            database_created_unix,
            source_updated_unix: Some(source_updated_unix),
        },
        buckets,
    ))
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
    requested_start_unix: u64,
    source_cutoff_unix: u64,
    buckets: &mut Vec<NetworkTrafficImportBucket>,
) -> Result<()> {
    let rows = traffic_rows(interface_payload, field)?;
    let mut seen = BTreeSet::new();
    for row in rows {
        let start_unix = traffic_row_timestamp(row)?;
        anyhow::ensure!(seen.insert(start_unix), "vnstat {field} rows contain a duplicate timestamp");
        push_bucket_if_relevant(
            buckets,
            interface,
            row,
            start_unix,
            u64::from(nominal_duration_secs),
            requested_start_unix,
            source_cutoff_unix,
        )?;
    }
    Ok(())
}

fn parse_day_rows(
    interface_payload: &Value,
    interface: &str,
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
    for (index, (start_unix, row)) in rows.iter().enumerate() {
        let next_delta = rows
            .get(index + 1)
            .map(|(next, _)| next.saturating_sub(*start_unix));
        let nominal_duration_secs = next_delta
            .filter(|duration| (23 * 60 * 60..=25 * 60 * 60).contains(duration))
            .unwrap_or(24 * 60 * 60);
        push_bucket_if_relevant(
            buckets,
            interface,
            row,
            *start_unix,
            nominal_duration_secs,
            requested_start_unix,
            source_cutoff_unix,
        )?;
    }
    Ok(())
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

#[allow(clippy::too_many_arguments)]
fn push_bucket_if_relevant(
    buckets: &mut Vec<NetworkTrafficImportBucket>,
    interface: &str,
    row: &Value,
    start_unix: u64,
    nominal_duration_secs: u64,
    requested_start_unix: u64,
    source_cutoff_unix: u64,
) -> Result<()> {
    let nominal_end_unix = start_unix.saturating_add(nominal_duration_secs);
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
        rx_bytes: row
            .get("rx")
            .and_then(Value::as_u64)
            .context("vnstat traffic row is missing rx")?,
        tx_bytes: row
            .get("tx")
            .and_then(Value::as_u64)
            .context("vnstat traffic row is missing tx")?,
    });
    Ok(())
}

fn floor_minute(unix: u64) -> u64 {
    unix - unix % 60
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
