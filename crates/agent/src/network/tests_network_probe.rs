use super::*;

#[test]
fn parses_linux_ping_latency_and_loss() {
    let parsed = parse_ping_measurement(
        "3 packets transmitted, 2 received, 33.3333% packet loss, time 400ms\n\
         rtt min/avg/max/mdev = 10.100/12.300/14.500/1.200 ms\n",
    );
    assert_eq!(parsed.transmitted, 3);
    assert_eq!(parsed.received, 2);
    assert_eq!(parsed.latency_avg_ms, Some(12.3));
    assert!(parsed.healthy);
}

#[test]
fn parses_three_field_ping_latency_without_inventing_mdev() {
    let parsed = parse_ping_measurement(
        "3 packets transmitted, 3 received, 0% packet loss\n\
         round-trip min/avg/max = 1.100/2.200/3.300 ms\n",
    );
    assert_eq!(parsed.transmitted, 3);
    assert_eq!(parsed.received, 3);
    assert_eq!(parsed.latency_min_ms, Some(1.1));
    assert_eq!(parsed.latency_avg_ms, Some(2.2));
    assert_eq!(parsed.latency_max_ms, Some(3.3));
    assert_eq!(parsed.latency_mdev_ms, None);
    assert!(parsed.healthy);
}

#[test]
fn failed_ping_is_unhealthy_without_inventing_latency() {
    let parsed = parse_ping_measurement("3 packets transmitted, 0 received, 100% packet loss\n");
    assert_eq!(parsed.received, 0);
    assert_eq!(parsed.packet_loss_ratio, 1.0);
    assert_eq!(parsed.latency_avg_ms, None);
    assert!(!parsed.healthy);
}

#[test]
fn unparseable_ping_is_a_complete_loss_not_a_healthy_zero() {
    let parsed = parse_ping_measurement("unexpected ping output\n");
    assert_eq!(parsed.transmitted, 0);
    assert_eq!(parsed.received, 0);
    assert_eq!(parsed.packet_loss_ratio, 1.0);
    assert_eq!(parsed.latency_avg_ms, None);
    assert!(!parsed.healthy);
}
