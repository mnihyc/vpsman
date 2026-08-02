use super::*;

const TEST_VNSTAT_PRESET_ARGV: &str = "/opt/vpsman/vnstat";

#[test]
fn vnstat_preset_uses_configured_base_argv() {
    let mut config = AgentConfig::default();
    config.network.runtime_vnstat_argv = vec![TEST_VNSTAT_PRESET_ARGV.to_string()];

    let command = vnstat_preset_command(&config, "ovpn42").unwrap();

    assert_eq!(
        command.argv,
        vec![
            TEST_VNSTAT_PRESET_ARGV.to_string(),
            "--json".to_string(),
            "-i".to_string(),
            "ovpn42".to_string()
        ]
    );
}

#[test]
fn parses_flat_and_vnstat_preset_traffic_payloads() {
    let flat = serde_json::json!({
        "rx_bytes": 1234,
        "tx_bytes": 5678,
    });
    assert_eq!(parse_flat_traffic_json(&flat), Some((1234, 5678)));

    let vnstat_preset = serde_json::json!({
        "interfaces": [{
            "traffic": {
                "total": [
                    { "rx": 100, "tx": 200 },
                    { "rx": 7, "tx": 9 }
                ]
            }
        }]
    });
    assert_eq!(
        parse_vnstat_preset_traffic_json(&vnstat_preset),
        Some((107, 209))
    );
}
