use super::BackupPolicyRetentionPruneConfig;

#[test]
fn backup_policy_prune_config_clamps_bounds() {
    let low = BackupPolicyRetentionPruneConfig::new(true, -5, false, false, false, None);
    assert_eq!(low.limit, 1);
    assert!(low.enabled);

    let high = BackupPolicyRetentionPruneConfig::new(true, 50_000, true, true, true, None);
    assert_eq!(high.limit, 1_000);
    assert!(high.dry_run);
    assert!(high.include_disabled);
    assert!(high.delete_objects);
}

#[test]
fn backup_policy_retention_candidate_query_returns_prune_identities() {
    let query = super::backup_policy_retention_candidate_query();
    assert!(query.contains("SELECT request_id, artifact_id, object_key"));
}

#[test]
fn backup_policy_scan_is_ordered_by_the_durable_fairness_cursor() {
    let query = super::backup_policy_retention_policies_query();
    assert!(query.contains("retention_scanned_at ASC NULLS FIRST"));
    assert!(query.contains("schedule.id ASC"));
    assert!(query.contains("schedule.deleted_at IS NULL"));
}
