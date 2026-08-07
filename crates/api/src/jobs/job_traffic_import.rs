use std::collections::BTreeMap;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use vpsman_common::{
    CommandOutput, JobCommand, NetworkTrafficImportBatch, NetworkTrafficImportBucket,
    NetworkTrafficImportResult, OutputStream, NETWORK_TRAFFIC_IMPORT_BUCKETS_PER_OUTPUT,
    NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE,
};

use crate::state::AppState;

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
    if !final_output.done
        || final_output.stream != OutputStream::Status
        || final_output.exit_code != Some(0)
    {
        return Ok(NetworkTrafficImportApply::NotApplicable);
    }
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
    let max_buckets = interfaces
        .len()
        .saturating_mul(NETWORK_TRAFFIC_IMPORT_MAX_BUCKETS_PER_INTERFACE);
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
        if view.client_id != client_id || view.done || view.stream != "status" {
            continue;
        }
        let data = BASE64
            .decode(&view.data_base64)
            .context("stored network traffic import output is not valid base64")?;
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
        if *seq >= final_seq || output.done || output.stream != OutputStream::Status {
            continue;
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
    if output_by_seq.len() != expected_batch_count
        || output_by_seq.keys().copied().ne(0..final_seq)
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
        Err(error) if error.to_string().contains("network_traffic_import_invalid:") => {
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

