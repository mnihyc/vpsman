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
