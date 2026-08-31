use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn temp_download_file_uses_private_spool_directory() {
    let temp = TempDownloadFile::new("vpsman-test-download", "bin").unwrap();
    let parent = temp.path().parent().unwrap();

    assert_eq!(
        std::fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn process_inventory_scan_limit_is_an_explicit_incomplete_signal() {
    let error = map_process_supervisor_inventory_error(anyhow::anyhow!(
        PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR
    ));

    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(error.code, PROCESS_SUPERVISOR_INVENTORY_SCAN_LIMIT_ERROR);
    assert!(error
        .public_message
        .as_deref()
        .is_some_and(|message| message.contains("Exact process inventory")));
}

#[test]
fn exact_job_target_status_batch_validates_pair_identity_and_bound() {
    let first_job = Uuid::new_v4();
    let second_job = Uuid::new_v4();
    assert!(validate_job_target_status_batch(&[
        JobTargetStatusBatchItem {
            job_id: first_job,
            client_id: "v-1".to_string(),
        },
        JobTargetStatusBatchItem {
            job_id: first_job,
            client_id: "v-2".to_string(),
        },
        JobTargetStatusBatchItem {
            job_id: second_job,
            client_id: "v-1".to_string(),
        },
    ])
    .is_ok());
    assert_eq!(
        validate_job_target_status_batch(&[])
            .expect_err("empty exact-pair request must fail")
            .code,
        "job_target_status_pairs_invalid"
    );
    assert_eq!(
        validate_job_target_status_batch(&[
            JobTargetStatusBatchItem {
                job_id: first_job,
                client_id: "v-1".to_string(),
            },
            JobTargetStatusBatchItem {
                job_id: first_job,
                client_id: "v-1".to_string(),
            },
        ])
        .expect_err("duplicate exact pair must fail")
        .code,
        "job_target_status_pairs_duplicate"
    );
    let oversized = (0..=JOB_TARGET_STATUS_BATCH_MAX_ITEMS)
        .map(|index| JobTargetStatusBatchItem {
            job_id: Uuid::new_v4(),
            client_id: format!("v-{index}"),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        validate_job_target_status_batch(&oversized)
            .expect_err("oversized exact-pair request must fail")
            .code,
        "job_target_status_pairs_invalid"
    );
}

#[test]
fn exact_job_target_status_query_is_pairwise_and_primary_key_addressable() {
    let query = crate::repository_jobs::JOB_TARGET_STATUS_BATCH_SQL;
    assert!(query.contains("unnest($1::uuid[], $2::text[]) WITH ORDINALITY"));
    assert!(query.contains("exact.job_id = requested.job_id"));
    assert!(query.contains("exact.client_id = requested.client_id"));
    assert!(query.contains("ORDER BY requested.input_ordinal"));
    assert!(!query.contains("job_id = ANY"));
    assert!(!query.contains("client_id = ANY"));
}
