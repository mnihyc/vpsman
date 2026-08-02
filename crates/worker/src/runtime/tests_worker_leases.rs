use super::*;

#[test]
fn lease_duration_bounds_are_documented() {
    assert_eq!(0_i32.clamp(1, 3600), 1);
    assert_eq!(4_000_i32.clamp(1, 3600), 3600);
}

#[test]
fn worker_lease_uses_a_task_scoped_transaction_advisory_lock() {
    assert!(WORKER_ADVISORY_LOCK_QUERY.contains("pg_try_advisory_xact_lock"));
    assert!(WORKER_ADVISORY_LOCK_QUERY.contains("vpsman.worker."));
}
