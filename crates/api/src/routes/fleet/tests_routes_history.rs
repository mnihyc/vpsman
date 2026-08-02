use super::*;

#[test]
fn history_retention_prune_preview_hash_ignores_moving_cutoff() {
    let plan = vec![HistoryRetentionPruneDomainPlan {
        policy: HistoryRetentionPolicyView {
            domain: "job_outputs".to_string(),
            retention_days: 30,
            prune_limit: 1000,
            enabled: true,
            metadata_only: false,
            export_enabled: true,
            notes: None,
            updated_by: None,
            updated_at: "0".to_string(),
            built_in_default: false,
        },
        prune_plan: HistoryRetentionPrunePlan {
            domain: HistoryDomain::JobOutputs,
            prune_limit: 1000,
            enabled: true,
        },
        cutoff_unix: 10_000,
        metadata_only: false,
        object_candidates: Some(vec![HistoryRetentionObjectCandidate::JobOutput {
            job_id: uuid::Uuid::new_v4(),
            client_id: "edge-a".to_string(),
            seq: 1,
            object_key: Some("job-output/a".to_string()),
        }]),
    }];
    let mut domain = HistoryRetentionPruneDomainView {
        domain: "job_outputs".to_string(),
        enabled: true,
        retention_days: 30,
        cutoff_unix: 10_000,
        matched_rows: 2,
        pruned_rows: 0,
        object_keys: vec!["job-output/a".to_string(), "job-output/b".to_string()],
        object_delete_attempted: false,
        object_delete_errors: Vec::new(),
        metadata_only: false,
        status: "dry_run".to_string(),
    };

    let first = history_retention_prune_preview_hash(
        Some("job_outputs"),
        Some(false),
        &plan,
        &[domain.clone()],
    )
    .expect("first history prune preview hash");
    domain.cutoff_unix += 60;
    let same_candidates = history_retention_prune_preview_hash(
        Some("job_outputs"),
        Some(false),
        &plan,
        &[domain.clone()],
    )
    .expect("second history prune preview hash");
    assert_eq!(first, same_candidates);

    domain.cutoff_unix += RETENTION_DAY_SECS;
    let next_day = history_retention_prune_preview_hash(
        Some("job_outputs"),
        Some(false),
        &plan,
        &[domain.clone()],
    )
    .expect("next-day history prune preview hash");
    assert_ne!(first, next_day);

    domain.cutoff_unix -= RETENTION_DAY_SECS;
    domain.object_keys.push("job-output/c".to_string());
    domain.matched_rows += 1;
    let changed_candidates =
        history_retention_prune_preview_hash(Some("job_outputs"), Some(false), &plan, &[domain])
            .expect("changed history prune preview hash");
    assert_ne!(first, changed_candidates);
}
