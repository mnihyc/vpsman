use super::*;

#[test]
fn speed_test_nonce_is_job_and_payload_bound() {
    let job_id = uuid::Uuid::parse_str("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap();
    let first = speed_test_nonce_hex(job_id, "payload-a");
    let second = speed_test_nonce_hex(job_id, "payload-b");
    assert_eq!(first.len(), 64);
    assert_ne!(first, second);
    assert_eq!(first, speed_test_nonce_hex(job_id, "payload-a"));
}

#[test]
fn socket_address_requires_an_explicit_ip() {
    assert_eq!(
        socket_addr("10.255.0.1", 42000).unwrap(),
        "10.255.0.1:42000".parse().unwrap()
    );
    assert!(socket_addr("example.invalid", 42000).is_err());
}

#[test]
fn zero_byte_limit_produces_unlimited_full_chunks() {
    assert_eq!(speed_chunk_limit(0, 0), SPEED_CHUNK_BYTES);
    assert_eq!(speed_chunk_limit(0, u64::MAX), SPEED_CHUNK_BYTES);
    assert_eq!(speed_chunk_limit(20_000, 10_000), 10_000);
    assert_eq!(speed_chunk_limit(20_000, 20_000), 0);
}

#[test]
fn finite_rate_budget_waits_until_the_accumulated_bytes_are_allowed() {
    assert_eq!(rate_budget_delay(Duration::ZERO, 16_384, 0), None);
    assert_eq!(
        rate_budget_delay(Duration::from_millis(100), 16_384, 64),
        Some(Duration::from_millis(100))
    );
    assert_eq!(rate_budget_delay(Duration::from_secs(3), 16_384, 64), None);
}
