use tokio::sync::broadcast::error::TryRecvError;
use vpsman_server_core::{JOB_STATUS_COMPLETED, JOB_STATUS_QUEUED, JOB_STATUS_RUNNING};

use crate::{
    model::WsEvent,
    repository::{MemoryState, Repository},
};

#[test]
fn invalid_hot_reload_keeps_the_last_known_good_suite_config() {
    let path = std::env::temp_dir().join(format!(
        "vpsman-suite-config-last-known-good-{}.toml",
        uuid::Uuid::new_v4()
    ));
    std::fs::write(&path, "version = 1\n\n[capacity]\ndispatcher_batch = 17\n").unwrap();
    let initial = super::load_suite_config_last_known_good(&path).unwrap();
    assert_eq!(initial.capacity.dispatcher_batch, Some(17));

    std::fs::remove_file(&path).unwrap();
    let missing_fallback = super::load_suite_config_last_known_good(&path).unwrap();
    assert_eq!(missing_fallback.capacity.dispatcher_batch, Some(17));

    std::fs::write(&path, "version = 1\n\n[capacity\n").unwrap();
    let fallback = super::load_suite_config_last_known_good(&path).unwrap();
    assert_eq!(fallback.capacity.dispatcher_batch, Some(17));

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn job_finished_publication_requires_a_terminal_refreshed_status() {
    let state = crate::tests::test_app_state(Repository::Memory(MemoryState::default()));
    let mut events = state.events.subscribe();
    let job_id = uuid::Uuid::new_v4();

    for active_status in [JOB_STATUS_QUEUED, JOB_STATUS_RUNNING] {
        assert_eq!(
            state
                .terminal_job_status_after_refresh(job_id, Some(active_status.to_string()))
                .await
                .unwrap(),
            None
        );
        state
            .publish_job_finished_after_refresh(job_id, Some(active_status.to_string()))
            .await
            .unwrap();
        assert!(matches!(events.try_recv(), Err(TryRecvError::Empty)));
    }

    assert_eq!(
        state
            .terminal_job_status_after_refresh(job_id, Some(JOB_STATUS_COMPLETED.to_string()))
            .await
            .unwrap(),
        Some(JOB_STATUS_COMPLETED.to_string())
    );
    state
        .publish_job_finished_after_refresh(job_id, Some(JOB_STATUS_COMPLETED.to_string()))
        .await
        .unwrap();
    assert!(matches!(
        events.try_recv(),
        Ok(WsEvent::JobFinished { job_id: event_job_id, status })
            if event_job_id == job_id && status == JOB_STATUS_COMPLETED
    ));
}
