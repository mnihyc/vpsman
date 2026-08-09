use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
    time::Duration,
};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;
use futures_util::{stream, StreamExt};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, warn};
use vpsman_common::{
    CommandOutput, JobCommand, NetworkTrafficImportBatch, NetworkTrafficImportBucket,
    NetworkTrafficImportResult, OutputStream, NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE, NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES,
};

use crate::{state::AppState, TargetDispatchOutcome};

const NETWORK_TRAFFIC_IMPORT_FINALIZER_INTERVAL_SECS: u64 = 5;
const NETWORK_TRAFFIC_IMPORT_FINALIZER_BATCH: i64 = 128;
const NETWORK_TRAFFIC_IMPORT_FINALIZER_IN_FLIGHT: usize = 4;
const NETWORK_TRAFFIC_IMPORT_FINALIZER_RETRY_AFTER_SECS: u64 = 30;

struct NetworkTrafficImportFinalizerState {
    notify: Notify,
    started: AtomicBool,
    sweep_lock: Mutex<()>,
}

impl Default for NetworkTrafficImportFinalizerState {
    fn default() -> Self {
        Self {
            notify: Notify::new(),
            started: AtomicBool::new(false),
            sweep_lock: Mutex::new(()),
        }
    }
}

static NETWORK_TRAFFIC_IMPORT_FINALIZER_STATE: OnceLock<NetworkTrafficImportFinalizerState> =
    OnceLock::new();

fn finalizer_state() -> &'static NetworkTrafficImportFinalizerState {
    NETWORK_TRAFFIC_IMPORT_FINALIZER_STATE.get_or_init(NetworkTrafficImportFinalizerState::default)
}

pub(crate) fn spawn_network_traffic_import_finalizer(state: AppState) {
    let finalizer = finalizer_state();
    if finalizer.started.swap(true, Ordering::AcqRel) {
        return;
    }
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(
            NETWORK_TRAFFIC_IMPORT_FINALIZER_INTERVAL_SECS,
        ));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = finalizer.notify.notified() => {}
            }
            if let Err(error) = finalize_pending_network_traffic_imports(&state).await {
                warn!(%error, "durable vnStat import finalizer sweep failed");
            }
        }
    });
}

pub(crate) fn wake_network_traffic_import_finalizer(state: AppState) {
    finalizer_state().notify.notify_one();
    if !finalizer_state().started.load(Ordering::Acquire) {
        tokio::spawn(async move {
            if let Err(error) = finalize_pending_network_traffic_imports(&state).await {
                warn!(%error, "durable vnStat import finalizer wake failed");
            }
        });
    }
}

pub(crate) async fn finalize_pending_network_traffic_imports(state: &AppState) -> Result<usize> {
    let _guard = finalizer_state().sweep_lock.lock().await;
    let pending = state
        .repo
        .list_pending_network_traffic_import_finalizations(NETWORK_TRAFFIC_IMPORT_FINALIZER_BATCH)
        .await?;
    let results = stream::iter(pending)
        .map(|item| async move {
            let result =
                finalize_network_traffic_import_target(state, item.job_id, &item.client_id).await;
            (item, result)
        })
        .buffer_unordered(NETWORK_TRAFFIC_IMPORT_FINALIZER_IN_FLIGHT)
        .collect::<Vec<_>>()
        .await;
    let mut finalized = 0_usize;
    for (item, result) in results {
        match result {
            Ok(true) => finalized += 1,
            Ok(false) => {}
            Err(error) => {
                warn!(
                    %error,
                    job_id = %item.job_id,
                    client_id = %item.client_id,
                    "vnStat import finalization failed and remains queued for retry"
                );
                if let Err(message_error) = state
                    .repo
                    .defer_network_traffic_import_finalization(
                        item.job_id,
                        &item.client_id,
                        "vnStat server import retry pending",
                        NETWORK_TRAFFIC_IMPORT_FINALIZER_RETRY_AFTER_SECS,
                    )
                    .await
                {
                    warn!(
                        %message_error,
                        job_id = %item.job_id,
                        client_id = %item.client_id,
                        "vnStat import retry state could not be recorded"
                    );
                }
            }
        }
    }
    if finalized > 0 {
        debug!(finalized, "durable vnStat import finalizer sweep completed");
    }
    Ok(finalized)
}

async fn finalize_network_traffic_import_target(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
) -> Result<bool> {
    let Some(candidate) = state
        .repo
        .contiguous_final_job_output_candidate(job_id, client_id)
        .await?
    else {
        return Ok(false);
    };
    let received_at = candidate
        .received_at
        .clone()
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let mut outcome = target_outcome_from_done_output(job_id, &candidate.output, received_at);
    if outcome.status == vpsman_server_core::TARGET_STATUS_COMPLETED {
        match apply_network_traffic_import_if_ready(
            state,
            job_id,
            client_id,
            candidate.seq,
            &candidate.output,
            &[],
        )
        .await?
        {
            NetworkTrafficImportApply::Pending => return Ok(false),
            NetworkTrafficImportApply::Applied(message) => outcome.message = message,
            NetworkTrafficImportApply::Invalid(message) => {
                outcome.status = vpsman_server_core::TARGET_STATUS_FAILED.to_string();
                outcome.exit_code = Some(1);
                outcome.message = message;
            }
            NetworkTrafficImportApply::NotApplicable => {
                outcome.status = vpsman_server_core::TARGET_STATUS_FAILED.to_string();
                outcome.exit_code = Some(1);
                outcome.message = "network_traffic_import_invalid:job_context_invalid".to_string();
            }
        }
    }
    if !state
        .repo
        .update_job_target_result(job_id, client_id, &outcome)
        .await?
    {
        return Ok(false);
    }
    let refreshed = state.repo.refresh_job_status_from_targets(job_id).await?;
    state
        .process_job_terminal_events_or_publish_refresh(500, job_id, refreshed)
        .await?;
    Ok(true)
}

pub(crate) fn target_outcome_from_done_output(
    job_id: uuid::Uuid,
    output: &CommandOutput,
    received_at: String,
) -> TargetDispatchOutcome {
    let outputs = vec![CommandOutput {
        job_id,
        stream: output.stream,
        data: output.data.clone(),
        exit_code: output.exit_code,
        done: output.done,
    }];
    let final_output = outputs.last();
    let (status, exit_code) = crate::routes_jobs::target_status_from_final_output(final_output);
    let message =
        crate::routes_jobs::target_message_for_status(&outputs, status, status, final_output);
    TargetDispatchOutcome {
        status: status.to_string(),
        exit_code,
        #[cfg(test)]
        command_version: None,
        accepted: true,
        message,
        received_at: Some(received_at),
        outputs,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum NetworkTrafficImportApply {
    NotApplicable,
    Pending,
    Applied(String),
    Invalid(String),
}

pub(crate) async fn apply_network_traffic_import_if_ready(
    state: &AppState,
    job_id: uuid::Uuid,
    client_id: &str,
    final_seq: i32,
    final_output: &CommandOutput,
    inline_outputs: &[(i32, CommandOutput)],
) -> Result<NetworkTrafficImportApply> {
    let Some(context) = state.repo.get_job_completion_context(job_id).await? else {
        return Ok(NetworkTrafficImportApply::NotApplicable);
    };
    let JobCommand::NetworkTrafficImportVnstat {
        interfaces,
        start_unix,
    } = context.operation
    else {
        return Ok(NetworkTrafficImportApply::NotApplicable);
    };
    if !final_output.done || final_output.exit_code != Some(0) {
        return Ok(NetworkTrafficImportApply::NotApplicable);
    }
    if final_output.stream != OutputStream::Status {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:final_output_invalid".to_string(),
        ));
    }

    let result = match serde_json::from_slice::<NetworkTrafficImportResult>(&final_output.data) {
        Ok(result) => result,
        Err(error) => {
            return Ok(NetworkTrafficImportApply::Invalid(format!(
                "network_traffic_import_invalid:agent_result_json_invalid:{error}"
            )));
        }
    };
    if result.r#type != "network_traffic_import_vnstat" || result.status != "collected" {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:agent_result_type_invalid".to_string(),
        ));
    }
    let resolved_interface_count = result.interfaces.len();
    if resolved_interface_count == 0
        || resolved_interface_count > NETWORK_TRAFFIC_IMPORT_MAX_INTERFACES
    {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:agent_result_interface_count_out_of_range".to_string(),
        ));
    }
    let max_buckets =
        resolved_interface_count.saturating_mul(NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE);
    let max_batches = max_buckets.div_ceil(NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT);
    if usize::try_from(result.batch_count).map_or(true, |count| count > max_batches)
        || usize::try_from(result.bucket_count).map_or(true, |count| count > max_buckets)
    {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:agent_result_count_exceeds_limit".to_string(),
        ));
    }
    if final_seq < 0 {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:final_sequence_invalid".to_string(),
        ));
    }
    if u32::try_from(final_seq).ok() != Some(result.batch_count) {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:output_sequence_count_mismatch".to_string(),
        ));
    }

    let mut output_by_seq = BTreeMap::<i32, CommandOutput>::new();
    for view in state.repo.list_job_outputs(job_id).await? {
        if view.client_id != client_id || view.seq >= final_seq {
            continue;
        }
        if view.done || view.stream != "status" {
            return Ok(NetworkTrafficImportApply::Invalid(
                "network_traffic_import_invalid:batch_output_invalid".to_string(),
            ));
        }
        let data = match BASE64.decode(&view.data_base64) {
            Ok(data) => data,
            Err(error) => {
                return Ok(NetworkTrafficImportApply::Invalid(format!(
                    "network_traffic_import_invalid:batch_output_base64_invalid:{error}"
                )));
            }
        };
        output_by_seq.insert(
            view.seq,
            CommandOutput {
                job_id,
                stream: OutputStream::Status,
                data,
                exit_code: view.exit_code,
                done: view.done,
            },
        );
    }
    for (seq, output) in inline_outputs {
        if *seq >= final_seq {
            continue;
        }
        if output.done || output.stream != OutputStream::Status {
            return Ok(NetworkTrafficImportApply::Invalid(
                "network_traffic_import_invalid:batch_output_invalid".to_string(),
            ));
        }
        match output_by_seq.get(seq) {
            Some(existing) if !command_output_matches(existing, output) => {
                return Ok(NetworkTrafficImportApply::Invalid(
                    "network_traffic_import_invalid:output_sequence_conflict".to_string(),
                ));
            }
            Some(_) => {}
            None => {
                output_by_seq.insert(*seq, output.clone());
            }
        }
    }

    let expected_batch_count = usize::try_from(result.batch_count).unwrap_or(usize::MAX);
    if output_by_seq.len() < expected_batch_count {
        return Ok(NetworkTrafficImportApply::Pending);
    }
    if output_by_seq.len() != expected_batch_count || output_by_seq.keys().copied().ne(0..final_seq)
    {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:batch_output_sequence_invalid".to_string(),
        ));
    }

    let mut batches = BTreeMap::<u32, NetworkTrafficImportBatch>::new();
    for output in output_by_seq.into_values() {
        let batch = match serde_json::from_slice::<NetworkTrafficImportBatch>(&output.data) {
            Ok(batch) => batch,
            Err(error) => {
                return Ok(NetworkTrafficImportApply::Invalid(format!(
                    "network_traffic_import_invalid:batch_json_invalid:{error}"
                )));
            }
        };
        if batch.r#type != "network_traffic_import_vnstat_batch" {
            return Ok(NetworkTrafficImportApply::Invalid(
                "network_traffic_import_invalid:batch_type_invalid".to_string(),
            ));
        }
        if batches.insert(batch.batch_index, batch).is_some() {
            return Ok(NetworkTrafficImportApply::Invalid(
                "network_traffic_import_invalid:duplicate_batch_index".to_string(),
            ));
        }
    }
    if batches.keys().copied().ne(0..result.batch_count) {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:batch_index_sequence_invalid".to_string(),
        ));
    }
    let buckets = batches
        .into_values()
        .flat_map(|batch| batch.buckets)
        .collect::<Vec<NetworkTrafficImportBucket>>();
    if usize::try_from(result.bucket_count).ok() != Some(buckets.len()) {
        return Ok(NetworkTrafficImportApply::Invalid(
            "network_traffic_import_invalid:bucket_count_mismatch".to_string(),
        ));
    }

    match state
        .repo
        .import_vnstat_traffic_history(
            job_id,
            client_id,
            &interfaces,
            start_unix,
            &result,
            &buckets,
            crate::unix_now(),
        )
        .await
    {
        Ok(summary) => Ok(NetworkTrafficImportApply::Applied(summary.message)),
        Err(error)
            if error
                .to_string()
                .contains("network_traffic_import_invalid:") =>
        {
            Ok(NetworkTrafficImportApply::Invalid(error.to_string()))
        }
        Err(error) => Err(error),
    }
}

fn command_output_matches(left: &CommandOutput, right: &CommandOutput) -> bool {
    left.job_id == right.job_id
        && left.stream == right.stream
        && left.data == right.data
        && left.exit_code == right.exit_code
        && left.done == right.done
}

#[cfg(test)]
#[path = "tests_job_traffic_import.rs"]
mod tests;
