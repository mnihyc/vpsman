use super::*;

#[test]
fn backup_policy_prune_preview_hash_ignores_moving_cutoff() {
    let schedule_id = uuid::Uuid::new_v4();
    let plan = vec![BackupPolicyPrunePlan {
        policy: BackupPolicyView {
            schedule_id,
            name: "nightly".to_string(),
            enabled: true,
            selector_expression: "id:edge-a".to_string(),
            target_client_ids: vec!["edge-a".to_string()],
            paths: vec!["/etc".to_string()],
            include_config: true,
            follow_symlinks: false,
            missing_path_policy: vpsman_common::BackupMissingPathPolicy::Fail,
            retention_days: 7,
            keep_last: 2,
            rotation_generation: None,
            cron_expr: "0 3 * * *".to_string(),
            timezone: "UTC".to_string(),
            next_runs: Vec::new(),
            cadence_error: None,
            catch_up_policy: "skip_missed".to_string(),
            catch_up_limit: 1,
            retry_delay_secs: 120,
            max_failures: 3,
            failure_count: 0,
            last_error: None,
            next_run_at: String::new(),
            last_run_at: None,
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        },
        cutoff_unix: 1_000,
        candidates: vec![
            BackupPolicyPruneCandidate::for_test(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "edge-a".to_string(),
                "backups/a.tar".to_string(),
                "0".to_string(),
            ),
            BackupPolicyPruneCandidate::for_test(
                uuid::Uuid::new_v4(),
                uuid::Uuid::new_v4(),
                "edge-a".to_string(),
                "backups/b.tar".to_string(),
                "1".to_string(),
            ),
        ],
    }];
    let mut policy = crate::model::BackupPolicyPrunePolicyView {
        schedule_id,
        name: "nightly".to_string(),
        enabled: true,
        retention_days: 7,
        keep_last: 2,
        cutoff_unix: 1_000,
        matched_rows: 2,
        pruned_rows: 0,
        object_keys: vec!["backups/a.tar".to_string(), "backups/b.tar".to_string()],
        object_delete_attempted: false,
        object_delete_errors: Vec::new(),
        metadata_only: true,
        status: "dry_run".to_string(),
    };

    let first =
        backup_policy_prune_preview_hash(Some(schedule_id), Some(true), &plan, &[policy.clone()])
            .expect("first prune preview hash");
    policy.cutoff_unix += 60;
    let same_candidates =
        backup_policy_prune_preview_hash(Some(schedule_id), Some(true), &plan, &[policy.clone()])
            .expect("second prune preview hash");
    assert_eq!(first, same_candidates);

    policy.cutoff_unix += RETENTION_DAY_SECS;
    let next_day =
        backup_policy_prune_preview_hash(Some(schedule_id), Some(true), &plan, &[policy.clone()])
            .expect("next-day prune preview hash");
    assert_ne!(first, next_day);

    policy.cutoff_unix -= RETENTION_DAY_SECS;
    policy.object_keys.push("backups/c.tar".to_string());
    policy.matched_rows += 1;
    let changed_candidates =
        backup_policy_prune_preview_hash(Some(schedule_id), Some(true), &plan, &[policy])
            .expect("changed prune preview hash");
    assert_ne!(first, changed_candidates);
}
