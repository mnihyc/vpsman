use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use tokio::sync::broadcast::error::TryRecvError;
use vpsman_server_core::{JOB_STATUS_COMPLETED, JOB_STATUS_QUEUED, JOB_STATUS_RUNNING};

use crate::{
    model::WsEvent,
    repository::{MemoryState, Repository},
};

#[test]
fn singleflight_auth_key_normalizes_effective_scope_order_and_duplicates() {
    let operator_id = uuid::Uuid::new_v4();
    assert_eq!(
        super::read_singleflight_auth_key(
            operator_id,
            &["fleet:read".to_string(), "config:read".to_string()],
        ),
        super::read_singleflight_auth_key(
            operator_id,
            &[
                "config:read".to_string(),
                "fleet:read".to_string(),
                "fleet:read".to_string(),
            ],
        )
    );
    assert_ne!(
        super::read_singleflight_auth_key(operator_id, &["fleet:read".to_string()]),
        super::read_singleflight_auth_key(operator_id, &["config:read".to_string()]),
    );
}

#[tokio::test]
async fn heavyweight_read_admission_is_shared_and_bounded_across_endpoints() {
    let (events, _) = super::WsEventBus::new(1);
    let first = events.acquire_heavy_read_permit().await.unwrap();
    let second = events.acquire_heavy_read_permit().await.unwrap();
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(20),
            events.acquire_heavy_read_permit(),
        )
        .await
        .is_err(),
        "a third unrelated heavyweight read bypassed global admission",
    );

    let mut queued = Vec::new();
    for _ in 0..super::HEAVY_READ_WAITING {
        let events = events.clone();
        queued.push(tokio::spawn(async move {
            events.acquire_heavy_read_permit().await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while events.heavy_read_waiters.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("heavyweight-read waiters did not register");
    assert_eq!(
        events.acquire_heavy_read_permit().await.unwrap_err().code,
        "heavy_read_admission_busy",
    );
    for waiter in queued {
        waiter.abort();
        let _ = waiter.await;
    }
    drop(first);
    let third = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        events.acquire_heavy_read_permit(),
    )
    .await
    .expect("released heavyweight-read capacity was not reusable")
    .unwrap();
    drop((second, third));
}

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

#[tokio::test]
async fn identical_singleflight_callers_share_one_zero_ttl_computation() {
    let singleflight = super::Singleflight::<usize>::default();
    let computations = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let leader = {
        let singleflight = singleflight.clone();
        let computations = Arc::clone(&computations);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        tokio::spawn(async move {
            singleflight
                .run("same".to_string(), "panic", "panic", move || async move {
                    computations.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    release.notified().await;
                    Ok(17)
                })
                .await
        })
    };
    started.notified().await;

    let mut callers = Vec::new();
    for _ in 0..4 {
        let singleflight = singleflight.clone();
        callers.push(tokio::spawn(async move {
            singleflight
                .run("same".to_string(), "panic", "panic", || async { Ok(99) })
                .await
        }));
    }
    singleflight.wait_for_participants("same", 5).await;
    release.notify_waiters();
    assert_eq!(leader.await.unwrap().unwrap(), 17);
    for caller in callers {
        assert_eq!(caller.await.unwrap().unwrap(), 17);
    }
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    let retry_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run("same".to_string(), "panic", "panic", move || async move {
                retry_computations.fetch_add(1, Ordering::SeqCst);
                Ok(18)
            })
            .await
            .unwrap(),
        18
    );
    assert_eq!(computations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn cancelled_leader_does_not_cancel_or_strand_singleflight_work() {
    let singleflight = super::Singleflight::<usize>::default();
    let computations = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let leader = {
        let singleflight = singleflight.clone();
        let computations = Arc::clone(&computations);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        tokio::spawn(async move {
            singleflight
                .run("cancel".to_string(), "panic", "panic", move || async move {
                    computations.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    release.notified().await;
                    Ok(23)
                })
                .await
        })
    };
    started.notified().await;
    leader.abort();
    let follower = {
        let singleflight = singleflight.clone();
        tokio::spawn(async move {
            singleflight
                .run("cancel".to_string(), "panic", "panic", || async { Ok(99) })
                .await
        })
    };
    singleflight.wait_for_participants("cancel", 2).await;
    release.notify_waiters();
    assert_eq!(follower.await.unwrap().unwrap(), 23);
    assert_eq!(computations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shared_error_and_panic_evict_singleflight_keys() {
    let singleflight = super::Singleflight::<usize>::default();
    let error_computations = Arc::new(AtomicUsize::new(0));
    let error_started = Arc::new(tokio::sync::Notify::new());
    let release_error = Arc::new(tokio::sync::Notify::new());
    let first = {
        let singleflight = singleflight.clone();
        let error_computations = Arc::clone(&error_computations);
        let error_started = Arc::clone(&error_started);
        let release_error = Arc::clone(&release_error);
        tokio::spawn(async move {
            singleflight
                .run("error".to_string(), "panic", "panic", move || async move {
                    error_computations.fetch_add(1, Ordering::SeqCst);
                    error_started.notify_one();
                    release_error.notified().await;
                    Err(crate::error::ApiError::bad_request("shared_error"))
                })
                .await
        })
    };
    error_started.notified().await;
    let second = {
        let singleflight = singleflight.clone();
        tokio::spawn(async move {
            singleflight
                .run("error".to_string(), "panic", "panic", || async { Ok(99) })
                .await
        })
    };
    singleflight.wait_for_participants("error", 2).await;
    release_error.notify_waiters();
    assert_eq!(first.await.unwrap().unwrap_err().code, "shared_error");
    assert_eq!(second.await.unwrap().unwrap_err().code, "shared_error");
    assert_eq!(error_computations.load(Ordering::SeqCst), 1);
    assert_eq!(
        singleflight
            .run("error".to_string(), "panic", "panic", || async { Ok(31) })
            .await
            .unwrap(),
        31
    );

    assert_eq!(
        singleflight
            .run("panic-key".to_string(), "shared_panic", "panic", || async {
                panic!("private panic detail")
            })
            .await
            .unwrap_err()
            .code,
        "shared_panic"
    );
    assert_eq!(
        singleflight
            .run("panic-key".to_string(), "shared_panic", "panic", || async {
                Ok(37)
            })
            .await
            .unwrap(),
        37
    );

    assert_eq!(
        singleflight
            .run(
                "synchronous-panic-key".to_string(),
                "shared_panic",
                "panic",
                || {
                    panic!("private synchronous panic detail");
                    #[allow(unreachable_code)]
                    std::future::ready(Ok(41))
                },
            )
            .await
            .unwrap_err()
            .code,
        "shared_panic"
    );
    assert_eq!(
        singleflight
            .run(
                "synchronous-panic-key".to_string(),
                "shared_panic",
                "panic",
                || async { Ok(43) },
            )
            .await
            .unwrap(),
        43
    );
}
