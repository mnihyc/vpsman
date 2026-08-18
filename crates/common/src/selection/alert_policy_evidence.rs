use serde_json::Value;

use crate::payload_hash;

/// Builds the cross-binary immutable identity for one state-source revision.
///
/// Fleet labels, selector metadata, and rendered presentation are deliberately
/// excluded. They have their own monotonic scope-revision evidence and must not
/// turn one unchanged authoritative state into multiple Count confirmations.
pub fn alert_policy_state_source_event_id(
    source_kind: &str,
    natural_key: &str,
    observed_at_unix_nanos: i64,
    payload: &Value,
) -> String {
    // Hash only fields that the selected source schema exposes to policy
    // expressions. API repair and worker transition adapters intentionally
    // carry different presentation/scope envelopes; those must still name the
    // same immutable source revision.
    let identity = match source_kind {
        "agent.status" | "agent.access" => serde_json::json!({
            "status": payload.get("status").cloned().unwrap_or(Value::Null),
        }),
        "tunnel.adapter" => serde_json::json!({
            "adapter_success": payload.pointer("/adapter/success").cloned().unwrap_or(Value::Null),
            "interface": payload.get("interface").cloned().unwrap_or(Value::Null),
            "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
        }),
        "tunnel.traffic" => serde_json::json!({
            "traffic_status": payload.pointer("/traffic/status").cloned().unwrap_or(Value::Null),
            "interface": payload.get("interface").cloned().unwrap_or(Value::Null),
            "reason": payload.get("reason").cloned().unwrap_or(Value::Null),
        }),
        _ => serde_json::json!({
            "status": payload.get("status").cloned().unwrap_or(Value::Null),
            "source_status": payload.get("source_status").cloned().unwrap_or(Value::Null),
        }),
    };
    format!(
        "{natural_key}:{observed_at_unix_nanos}:{}",
        payload_hash(identity.to_string().as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn state_identity_ignores_subject_and_presentation_but_keeps_source_truth() {
        let first = json!({
            "status": "offline",
            "source_status": "offline",
            "client_id": "vps-a",
            "reason": "old name currently reports offline",
            "subject": {"display_name":"old name","tags":["old"]}
        });
        let renamed = json!({
            "status": "offline",
            "source_status": "offline",
            "client_id": "vps-a",
            "reason": "new name currently reports offline",
            "subject": {"display_name":"new name","tags":["new"]}
        });
        let recovered = json!({
            "status": "online",
            "source_status": "online",
            "client_id": "vps-a",
            "reason": "new name currently reports online",
            "subject": {"display_name":"new name","tags":["new"]}
        });
        assert_eq!(
            alert_policy_state_source_event_id("agent.status", "vps-a", 1, &first),
            alert_policy_state_source_event_id("agent.status", "vps-a", 1, &renamed)
        );
        assert_ne!(
            alert_policy_state_source_event_id("agent.status", "vps-a", 1, &first),
            alert_policy_state_source_event_id("agent.status", "vps-a", 1, &recovered)
        );
    }

    #[test]
    fn cross_binary_envelopes_share_state_identity() {
        let api = serde_json::json!({
            "status": "offline",
            "source_status": "offline",
            "client_id": "vps-a",
            "reason": "API presentation",
            "subject": {"display_name":"API name","tags":["blue"]},
            "capability_privilege_mode": null
        });
        let worker = serde_json::json!({
            "status": "offline",
            "source_status": "offline",
            "client_id": "vps-a",
            "reason": "worker presentation",
            "subject": {"display_name":"worker name","tags":[]},
            "worker_only_envelope": true
        });
        assert_eq!(
            alert_policy_state_source_event_id("agent.status", "vps-a", 7, &api),
            alert_policy_state_source_event_id("agent.status", "vps-a", 7, &worker)
        );
    }
}
