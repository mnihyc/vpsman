const CLIENT_POLICY_SUPPRESSION_LOCK_PREFIX: &str = "vpsman.client_policy_suppression:";

pub fn client_policy_suppression_lock_key(client_id: &str) -> String {
    format!("{CLIENT_POLICY_SUPPRESSION_LOCK_PREFIX}{client_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_policy_suppression_key_is_namespaced() {
        assert_eq!(
            client_policy_suppression_lock_key("edge-a"),
            "vpsman.client_policy_suppression:edge-a"
        );
    }
}
