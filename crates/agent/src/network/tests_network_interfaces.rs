use super::*;

#[test]
fn parses_proc_net_dev_counters() {
    let counters = network_counters_from_proc_net_dev(
        "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n  lo: 12 0 0 0 0 0 0 0 34 0 0 0 0 0 0 0\neth0: 1000 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n",
    )
    .unwrap();

    assert_eq!(counters.get("lo").unwrap().rx_bytes, 12);
    assert_eq!(counters.get("lo").unwrap().tx_bytes, 34);
    assert_eq!(counters.get("eth0").unwrap().rx_bytes, 1000);
    assert_eq!(counters.get("eth0").unwrap().tx_bytes, 2000);
}

#[test]
fn malformed_proc_net_dev_is_an_explicit_source_error() {
    let error = network_counters_from_proc_net_dev(
        "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\neth0: broken 0 0 0 0 0 0 0 2000 0 0 0 0 0 0 0\n",
    )
    .unwrap_err();
    assert!(error.to_string().contains("RX counter"));
}

#[test]
fn validates_interface_names_before_reporting() {
    assert!(valid_interface_name("eth0"));
    assert!(valid_interface_name("wg-east"));
    assert!(!valid_interface_name(""));
    assert!(!valid_interface_name("../eth0"));
    assert!(!valid_interface_name("bad\nname"));
}
