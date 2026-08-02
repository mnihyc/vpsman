use super::*;

#[test]
fn parses_linux_ping_latency_and_loss() {
    let parsed = parse_ping_output(
        "3 packets transmitted, 2 received, 33.3333% packet loss, time 400ms\n\
         rtt min/avg/max/mdev = 10.100/12.300/14.500/1.200 ms\n",
    );
    assert_eq!(parsed["transmitted"], 3);
    assert_eq!(parsed["received"], 2);
    assert_eq!(parsed["latency_avg_ms"], 12.3);
    assert_eq!(parsed["healthy"], true);
}

#[test]
fn failed_ping_is_unhealthy_without_inventing_latency() {
    let parsed = parse_ping_output("3 packets transmitted, 0 received, 100% packet loss\n");
    assert_eq!(parsed["received"], 0);
    assert_eq!(parsed["packet_loss_ratio"], 1.0);
    assert_eq!(parsed["latency_avg_ms"], serde_json::Value::Null);
    assert_eq!(parsed["healthy"], false);
}
