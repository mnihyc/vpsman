use vpsman_common::NetworkStat;

pub(crate) struct TrafficAccumulation {
    pub(crate) rx_bytes: u64,
    pub(crate) tx_bytes: u64,
    pub(crate) source: String,
    pub(crate) status: String,
    pub(crate) reason: Option<String>,
}

pub(crate) fn traffic_accumulation_for_interface(
    interface: &str,
    counters: Option<NetworkStat>,
) -> TrafficAccumulation {
    if let Some(counters) = counters {
        return TrafficAccumulation {
            rx_bytes: counters.rx_bytes,
            tx_bytes: counters.tx_bytes,
            source: "interface_counters".to_string(),
            status: "ok".to_string(),
            reason: None,
        };
    }
    TrafficAccumulation {
        rx_bytes: 0,
        tx_bytes: 0,
        source: "interface_counters".to_string(),
        status: "missing".to_string(),
        reason: Some(format!("{interface}_not_found")),
    }
}

#[cfg(test)]
#[path = "tests_telemetry_traffic.rs"]
mod tests;
