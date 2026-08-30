use uuid::Uuid;

use crate::{
    model_monitoring::{MonitoringShareRecord, MonitoringShareVisibilityView},
    repository_monitoring::monitoring_share_status,
};

#[test]
fn monitoring_share_expiry_is_fail_closed_for_invalid_values() {
    let now = crate::unix_now();
    let mut share = MonitoringShareRecord {
        id: Uuid::new_v4(),
        name: "expiry".to_string(),
        token_secret: "d".repeat(64),
        selector_expression: "*".to_string(),
        targets: Vec::new(),
        visibility: MonitoringShareVisibilityView {
            identity_context: false,
            billing: false,
            system_information: false,
            resources: true,
            network: true,
            traffic: true,
            ping: true,
            detail_history: true,
        },
        expires_at: "not-a-timestamp".to_string(),
        revoked_at: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
    };
    assert_eq!(monitoring_share_status(&share, now), "expired");
    share.expires_at = "2099-01-01 00:00:00+00".to_string();
    assert_eq!(monitoring_share_status(&share, now), "active");
}
