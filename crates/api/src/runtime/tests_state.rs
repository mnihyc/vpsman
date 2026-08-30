use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

#[test]
fn dispatcher_owners_cover_exact_gateway_phases_without_using_batch_as_throttle() {
    let config = super::DispatcherRuntimeConfig::default();
    assert_eq!(config.immediate_claim_limit(), 64);
    assert_eq!(config.gateway_dispatch_attempt_timeout_secs(), 60);
    assert_eq!(config.gateway_dispatch_attempt_lease_secs(), 65);
    assert_eq!(config.control_deadline_extra_secs(), 105);

    let mut transaction_bounded = config;
    transaction_bounded.batch_limit = 17;
    assert_eq!(transaction_bounded.immediate_claim_limit(), 17);
}

#[test]
fn routing_terminal_retry_classification_discards_only_invalid_evidence() {
    for code in [
        "network_routing_result_plan_id_invalid",
        "network_routing_result_missing",
        "network_routing_result_invalid",
        "network_routing_result_contract_mismatch",
    ] {
        assert!(super::network_routing_terminal_error_is_permanent(code));
    }
    assert!(!super::network_routing_terminal_error_is_permanent(
        "internal_server_error"
    ));
}

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
async fn singleflight_bypasses_only_new_distinct_keys_at_the_hard_in_flight_limit() {
    let singleflight = super::Singleflight::<usize>::default();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let mut leaders = Vec::with_capacity(super::MAX_SINGLEFLIGHT_ENTRIES);
    for index in 0..super::MAX_SINGLEFLIGHT_ENTRIES {
        let singleflight = singleflight.clone();
        let release = Arc::clone(&release);
        leaders.push(tokio::spawn(async move {
            singleflight
                .run(
                    format!("held-{index}"),
                    "panic",
                    "panic",
                    move || async move {
                        release.acquire().await.unwrap().forget();
                        Ok(index)
                    },
                )
                .await
        }));
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if singleflight.entries.lock().await.len() == super::MAX_SINGLEFLIGHT_ENTRIES {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("singleflight leaders did not fill the bounded registry");

    let distinct_overflow = {
        let singleflight = singleflight.clone();
        tokio::spawn(async move {
            singleflight
                .run(
                    "distinct-overflow".to_string(),
                    "panic",
                    "panic",
                    || async { Ok(997) },
                )
                .await
        })
    };
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), distinct_overflow)
            .await
            .expect("distinct-key overflow waited on saturated registry capacity")
            .unwrap()
            .unwrap(),
        997
    );
    assert_eq!(
        singleflight.entries.lock().await.len(),
        super::MAX_SINGLEFLIGHT_ENTRIES,
        "direct overflow must not expand the deduplication registry"
    );

    const FOLLOWERS: usize = 4;
    let mut followers = Vec::with_capacity(FOLLOWERS);
    for _ in 0..FOLLOWERS {
        let singleflight = singleflight.clone();
        followers.push(tokio::spawn(async move {
            singleflight
                .run("held-0".to_string(), "panic", "panic", || async {
                    panic!("same-key follower must not become a direct leader")
                })
                .await
        }));
    }
    singleflight
        .wait_for_participants("held-0", FOLLOWERS + 1)
        .await;
    release.add_permits(1);
    for follower in followers {
        assert_eq!(
            follower.await.unwrap().unwrap(),
            0,
            "same-key caller must still join at the hard registry bound"
        );
    }
    release.add_permits(super::MAX_SINGLEFLIGHT_ENTRIES - 1);
    for leader in leaders {
        leader.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn completed_zero_ttl_and_error_singleflight_results_are_not_later_discoverable() {
    let singleflight = super::Singleflight::<usize>::default();
    let entry = Arc::new(super::SingleflightEntry::new(0));
    *entry.result.lock().await = Some(super::CachedSingleflightResult {
        value: Ok(17),
        expires_at: None,
    });
    entry.completed.store(true, Ordering::Release);
    singleflight
        .entries
        .lock()
        .await
        .insert("completed".to_string(), entry);

    let computations = Arc::new(AtomicUsize::new(0));
    let load_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run(
                "completed".to_string(),
                "panic",
                "panic",
                move || async move {
                    load_computations.fetch_add(1, Ordering::SeqCst);
                    Ok(19)
                },
            )
            .await
            .unwrap(),
        19,
        "a zero-TTL completion escaped its overlapping caller set"
    );
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    let error_entry = Arc::new(super::SingleflightEntry::new(0));
    *error_entry.result.lock().await = Some(super::CachedSingleflightResult {
        value: Err(super::SharedApiError::from_api_error(
            crate::error::ApiError::bad_request("finished_error"),
        )),
        expires_at: None,
    });
    error_entry.completed.store(true, Ordering::Release);
    singleflight
        .entries
        .lock()
        .await
        .insert("completed-error".to_string(), error_entry);

    let retry_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run(
                "completed-error".to_string(),
                "panic",
                "panic",
                move || async move {
                    retry_computations.fetch_add(1, Ordering::SeqCst);
                    Ok(29)
                },
            )
            .await
            .unwrap(),
        29,
        "a completed error escaped its overlapping caller set"
    );
    assert_eq!(computations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn busy_completed_singleflight_result_cannot_strand_a_capacity_waiter() {
    let singleflight = super::Singleflight::<usize>::default();
    let completed = Arc::new(super::SingleflightEntry::new(0));
    *completed.result.lock().await = Some(super::CachedSingleflightResult {
        value: Ok(17),
        expires_at: Some(tokio::time::Instant::now() + std::time::Duration::from_secs(60)),
    });
    completed.completed.store(true, Ordering::Release);
    {
        let mut entries = singleflight.entries.lock().await;
        for index in 0..(super::MAX_SINGLEFLIGHT_ENTRIES - 1) {
            entries.insert(
                format!("in-flight-{index}"),
                Arc::new(super::SingleflightEntry::new(0)),
            );
        }
        entries.insert("busy-completed".to_string(), Arc::clone(&completed));
    }

    // Model a follower cloning a large cached payload while a new key arrives.
    // Registry eviction must use completion visibility, not availability of
    // this short-held mutex.
    let _busy_result = completed.result.lock().await;
    assert_eq!(
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            singleflight.run("overflow".to_string(), "panic", "panic", || async {
                Ok(23)
            }),
        )
        .await
        .expect("a busy completed result stranded registry capacity")
        .unwrap(),
        23
    );
}

#[tokio::test(start_paused = true)]
async fn completed_singleflight_result_is_reused_only_within_bounded_ttl() {
    let singleflight = super::Singleflight::<usize>::with_ttl(std::time::Duration::from_millis(30));
    let computations = Arc::new(AtomicUsize::new(0));

    let first_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run("ttl".to_string(), "panic", "panic", move || async move {
                Ok(first_computations.fetch_add(1, Ordering::SeqCst) + 1)
            })
            .await
            .unwrap(),
        1
    );
    let cached_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run("ttl".to_string(), "panic", "panic", move || async move {
                Ok(cached_computations.fetch_add(1, Ordering::SeqCst) + 1)
            })
            .await
            .unwrap(),
        1,
        "a completed read should serve the short post-completion cache window"
    );
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    tokio::time::advance(std::time::Duration::from_millis(45)).await;
    let expired_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run("ttl".to_string(), "panic", "panic", move || async move {
                Ok(expired_computations.fetch_add(1, Ordering::SeqCst) + 1)
            })
            .await
            .unwrap(),
        2,
        "the cache must not turn a telemetry snapshot into a persistent value"
    );
}

#[tokio::test(start_paused = true)]
async fn monitoring_singleflight_notifications_preserve_one_second_ttl_but_invalidation_refreshes()
{
    assert_eq!(
        super::MONITORING_READ_CACHE_TTL,
        std::time::Duration::from_secs(1)
    );
    let (events, _invalidations) = super::WsEventBus::new(16);
    let computations = Arc::new(AtomicUsize::new(0));

    let first_computations = Arc::clone(&computations);
    let first = events
        .singleflight_monitoring_cards("steady-telemetry".to_string(), move || async move {
            let value = first_computations.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total: value,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(first.total, 1);

    for _ in 0..100 {
        events.notify_fleet_telemetry();
    }
    let cached_computations = Arc::clone(&computations);
    let cached = events
        .singleflight_monitoring_cards("steady-telemetry".to_string(), move || async move {
            let value = cached_computations.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total: value,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(cached.total, 1);
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    tokio::time::advance(std::time::Duration::from_millis(999)).await;
    let boundary_computations = Arc::clone(&computations);
    let inside_boundary = events
        .singleflight_monitoring_cards("steady-telemetry".to_string(), move || async move {
            let value = boundary_computations.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total: value,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(inside_boundary.total, 1);

    tokio::time::advance(std::time::Duration::from_millis(2)).await;
    let expired_computations = Arc::clone(&computations);
    let expired = events
        .singleflight_monitoring_cards("steady-telemetry".to_string(), move || async move {
            let value = expired_computations.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total: value,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(expired.total, 2);

    events.invalidate_fleet_telemetry();
    let refreshed_computations = Arc::clone(&computations);
    let refreshed = events
        .singleflight_monitoring_cards("steady-telemetry".to_string(), move || async move {
            let value = refreshed_computations.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(crate::model::MonitoringCardsPageView {
                items: Vec::new(),
                offset: 0,
                limit: 1,
                total: value,
                next_offset: None,
            })
        })
        .await
        .unwrap();
    assert_eq!(refreshed.total, 3);
    assert_eq!(computations.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn invalidation_keeps_the_running_leader_and_coalesces_one_trailing_refresh() {
    let singleflight = super::Singleflight::<usize>::with_ttl(std::time::Duration::from_secs(60));
    let computations = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let leader = {
        let singleflight = singleflight.clone();
        let computations = Arc::clone(&computations);
        let started = Arc::clone(&started);
        let release = Arc::clone(&release);
        tokio::spawn(async move {
            singleflight
                .run(
                    "invalidate-running".to_string(),
                    "panic",
                    "panic",
                    move || async move {
                        let value = computations.fetch_add(1, Ordering::SeqCst) + 1;
                        started.notify_one();
                        release.acquire().await.unwrap().forget();
                        Ok(value)
                    },
                )
                .await
        })
    };
    started.notified().await;

    singleflight.clear();
    let follower = {
        let singleflight = singleflight.clone();
        let computations = Arc::clone(&computations);
        tokio::spawn(async move {
            singleflight
                .run(
                    "invalidate-running".to_string(),
                    "panic",
                    "panic",
                    move || async move { Ok(computations.fetch_add(1, Ordering::SeqCst) + 1) },
                )
                .await
        })
    };
    singleflight
        .wait_for_participants("invalidate-running", 2)
        .await;
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    release.add_permits(1);
    assert_eq!(leader.await.unwrap().unwrap(), 1);
    assert_eq!(follower.await.unwrap().unwrap(), 2);
    assert_eq!(computations.load(Ordering::SeqCst), 2);

    let cached_computations = Arc::clone(&computations);
    assert_eq!(
        singleflight
            .run(
                "invalidate-running".to_string(),
                "panic",
                "panic",
                move || async move { Ok(cached_computations.fetch_add(1, Ordering::SeqCst) + 1) },
            )
            .await
            .unwrap(),
        2,
        "the invalidated leader must not replace the trailing cached result"
    );
    assert_eq!(computations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn repeated_invalidation_during_a_read_never_fans_out_concurrent_followers() {
    const FOLLOWERS: usize = 8;
    let singleflight = super::Singleflight::<usize>::with_ttl(std::time::Duration::from_secs(60));
    let computations = Arc::new(AtomicUsize::new(0));
    let initial_started = Arc::new(tokio::sync::Notify::new());
    let initial_release = Arc::new(tokio::sync::Semaphore::new(0));
    let trailing_started = Arc::new(tokio::sync::Notify::new());
    let trailing_release = Arc::new(tokio::sync::Semaphore::new(0));
    let leader = {
        let singleflight = singleflight.clone();
        let computations = Arc::clone(&computations);
        let initial_started = Arc::clone(&initial_started);
        let initial_release = Arc::clone(&initial_release);
        tokio::spawn(async move {
            singleflight
                .run(
                    "steady-invalidation".to_string(),
                    "panic",
                    "panic",
                    move || async move {
                        let value = computations.fetch_add(1, Ordering::SeqCst) + 1;
                        initial_started.notify_one();
                        initial_release.acquire().await.unwrap().forget();
                        Ok(value)
                    },
                )
                .await
        })
    };
    initial_started.notified().await;

    singleflight.clear();
    let mut followers = Vec::new();
    for _ in 0..FOLLOWERS {
        let singleflight = singleflight.clone();
        let computations = Arc::clone(&computations);
        let trailing_started = Arc::clone(&trailing_started);
        let trailing_release = Arc::clone(&trailing_release);
        followers.push(tokio::spawn(async move {
            singleflight
                .run(
                    "steady-invalidation".to_string(),
                    "panic",
                    "panic",
                    move || async move {
                        let value = computations.fetch_add(1, Ordering::SeqCst) + 1;
                        trailing_started.notify_one();
                        trailing_release.acquire().await.unwrap().forget();
                        Ok(value)
                    },
                )
                .await
        }));
    }
    singleflight
        .wait_for_participants("steady-invalidation", FOLLOWERS + 1)
        .await;
    for _ in 0..32 {
        singleflight.clear();
    }
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    initial_release.add_permits(1);
    assert_eq!(leader.await.unwrap().unwrap(), 1);
    trailing_started.notified().await;
    singleflight
        .wait_for_participants("steady-invalidation", FOLLOWERS)
        .await;
    assert_eq!(
        computations.load(Ordering::SeqCst),
        2,
        "repeated invalidation created more than one trailing leader"
    );
    trailing_release.add_permits(1);
    for follower in followers {
        assert_eq!(follower.await.unwrap().unwrap(), 2);
    }
    assert_eq!(computations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn invalidation_after_completion_forces_one_fresh_cached_result() {
    let singleflight = super::Singleflight::<usize>::with_ttl(std::time::Duration::from_secs(60));
    let computations = Arc::new(AtomicUsize::new(0));
    for expected in [1, 1] {
        let computations = Arc::clone(&computations);
        assert_eq!(
            singleflight
                .run(
                    "post-completion".to_string(),
                    "panic",
                    "panic",
                    move || async move { Ok(computations.fetch_add(1, Ordering::SeqCst) + 1) },
                )
                .await
                .unwrap(),
            expected
        );
    }
    assert_eq!(computations.load(Ordering::SeqCst), 1);

    singleflight.clear();
    for expected in [2, 2] {
        let computations = Arc::clone(&computations);
        assert_eq!(
            singleflight
                .run(
                    "post-completion".to_string(),
                    "panic",
                    "panic",
                    move || async move { Ok(computations.fetch_add(1, Ordering::SeqCst) + 1) },
                )
                .await
                .unwrap(),
            expected
        );
    }
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
