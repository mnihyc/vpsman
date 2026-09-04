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
