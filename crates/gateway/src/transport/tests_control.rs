use super::*;
use crate::state::{
    GatewayCommandEnqueueMarker, GatewaySession, GatewaySessionCloseRequest,
    SESSION_COMMAND_QUEUE_CAPACITY,
};
use vpsman_common::{JobCommand, JobRequest};

#[test]
fn internal_control_auth_checks_bearer_token_when_configured() {
    let headers = vec![(
        "authorization".to_string(),
        "Bearer expected-token".to_string(),
    )];

    assert!(!authorized_internal_request(&headers, None));
    assert!(authorized_internal_request(
        &headers,
        Some("expected-token")
    ));
    assert!(!authorized_internal_request(&headers, Some("wrong-token")));
    assert!(!authorized_internal_request(&[], Some("expected-token")));
}

#[test]
fn http_header_end_detects_complete_header_block() {
    assert_eq!(find_header_end(b"POST / HTTP/1.1\r\n\r\nbody"), Some(15));
    assert_eq!(find_header_end(b"POST / HTTP/1.1\r\n"), None);
}

#[tokio::test]
async fn full_session_command_queue_returns_busy_error() {
    let state = GatewayState::default();
    let (sender, _receiver) = tokio::sync::mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    for _ in 0..SESSION_COMMAND_QUEUE_CAPACITY {
        let (response, _response_rx) = tokio::sync::oneshot::channel();
        sender
            .try_send(GatewaySessionMessage::Command(Box::new(GatewayCommand {
                request: test_job_request(),
                payload_hash: "test-payload-hash".to_string(),
                response,
            })))
            .unwrap();
    }
    let (close_tx, _close_rx) = tokio::sync::watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );

    let error = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            client_id: "client-a".to_string(),
            request: test_job_request(),
            expected_process_incarnation_id: state
                .sessions
                .read()
                .await
                .get("client-a")
                .unwrap()
                .process_incarnation_id,
            payload_hash: "test-payload-hash".to_string(),
        },
    )
    .await
    .unwrap_err()
    .to_string();

    assert!(error.contains("agent_session_command_queue_full:client-a"));
    assert!(state.command_enqueues.read().await.is_empty());
}

#[tokio::test]
async fn disconnect_bypasses_full_session_command_queue() {
    let state = GatewayState::default();
    let (sender, _receiver) = tokio::sync::mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    for _ in 0..SESSION_COMMAND_QUEUE_CAPACITY {
        let (response, _response_rx) = tokio::sync::oneshot::channel();
        sender
            .try_send(GatewaySessionMessage::Command(Box::new(GatewayCommand {
                request: test_job_request(),
                payload_hash: "test-payload-hash".to_string(),
                response,
            })))
            .unwrap();
    }
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );

    let result = disconnect_gateway_session(
        &state,
        GatewaySessionDisconnect {
            client_id: "client-a".to_string(),
            reason: "client_key_revoked".to_string(),
        },
    )
    .await
    .unwrap();

    assert!(result.accepted);
    assert!(result.disconnected);
    assert!(!state.sessions.read().await.contains_key("client-a"));
    close_rx.changed().await.unwrap();
    assert_eq!(
        close_rx.borrow().as_ref(),
        Some(&GatewaySessionCloseRequest::Graceful(
            "client_key_revoked".to_string()
        ))
    );
}

#[tokio::test]
async fn disconnect_batch_validates_before_mutation_and_preserves_order() {
    let state = GatewayState::default();
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-b".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );

    let empty = disconnect_gateway_sessions(
        &state,
        GatewaySessionDisconnectBatchRequest {
            items: vec![GatewaySessionDisconnect {
                client_id: String::new(),
                reason: "vps_deleted".to_string(),
            }],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(empty, "session_disconnect_batch_empty_request_id");
    assert!(state.sessions.read().await.contains_key("client-b"));

    let duplicate = disconnect_gateway_sessions(
        &state,
        GatewaySessionDisconnectBatchRequest {
            items: vec![
                GatewaySessionDisconnect {
                    client_id: "client-b".to_string(),
                    reason: "vps_deleted".to_string(),
                },
                GatewaySessionDisconnect {
                    client_id: "client-b".to_string(),
                    reason: "vps_deleted".to_string(),
                },
            ],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        duplicate,
        "session_disconnect_batch_duplicate_request_id:client-b"
    );
    assert!(state.sessions.read().await.contains_key("client-b"));

    let result = disconnect_gateway_sessions(
        &state,
        GatewaySessionDisconnectBatchRequest {
            items: vec![
                GatewaySessionDisconnect {
                    client_id: "client-b".to_string(),
                    reason: "vps_deleted".to_string(),
                },
                GatewaySessionDisconnect {
                    client_id: "client-a".to_string(),
                    reason: "vps_deleted".to_string(),
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(result.results[0].client_id, "client-b");
    assert!(result.results[0].disconnected);
    assert_eq!(result.results[1].client_id, "client-a");
    assert!(!result.results[1].disconnected);
    assert!(state.sessions.read().await.is_empty());
    close_rx.changed().await.unwrap();
    assert_eq!(
        close_rx.borrow().as_ref(),
        Some(&GatewaySessionCloseRequest::Graceful(
            "vps_deleted".to_string()
        ))
    );
}

#[tokio::test]
async fn command_enqueue_before_suspension_fence_is_reported_as_protected() {
    let state = GatewayState::default();
    let process_incarnation_id = uuid::Uuid::new_v4();
    let job_id = uuid::Uuid::new_v4();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (close_tx, _close_rx) = tokio::sync::watch::channel(None);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id,
            sender,
            close_tx,
        },
    );
    let dispatch_state = state.clone();
    let dispatch = tokio::spawn(async move {
        dispatch_gateway_command(
            &dispatch_state,
            GatewayCommandDispatch {
                client_id: "client-a".to_string(),
                request: JobRequest {
                    job_id,
                    ..test_job_request()
                },
                expected_process_incarnation_id: process_incarnation_id,
                payload_hash: "test-payload-hash".to_string(),
            },
        )
        .await
    });
    let message = receiver.recv().await.unwrap();
    let GatewaySessionMessage::Command(command) = message else {
        panic!("expected command enqueue");
    };
    let fence = prepare_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFencePrepare {
            client_id: "client-a".to_string(),
            token: uuid::Uuid::new_v4(),
            lease_secs: 60,
        },
    )
    .await;
    assert!(fence.accepted);
    assert_eq!(fence.enqueued_job_ids, vec![job_id]);
    command
        .response
        .send(GatewayCommandDispatchResult {
            client_id: "client-a".to_string(),
            job_id,
            command_version: 1,
            accepted: true,
            message: "accepted".to_string(),
            outputs: Vec::new(),
        })
        .unwrap();
    assert!(dispatch.await.unwrap().unwrap().accepted);
}

#[tokio::test]
async fn suspension_fence_before_enqueue_rejects_even_a_new_session() {
    let state = GatewayState::default();
    let token = uuid::Uuid::new_v4();
    let fence = prepare_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFencePrepare {
            client_id: "client-a".to_string(),
            token,
            lease_secs: 60,
        },
    )
    .await;
    assert!(fence.accepted);
    assert!(fence.enqueued_job_ids.is_empty());
    let process_incarnation_id = uuid::Uuid::new_v4();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (close_tx, _close_rx) = tokio::sync::watch::channel(None);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id,
            sender,
            close_tx,
        },
    );

    let error = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            client_id: "client-a".to_string(),
            request: test_job_request(),
            expected_process_incarnation_id: process_incarnation_id,
            payload_hash: "test-payload-hash".to_string(),
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(error, "agent_suspended:client-a");
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn prepared_fence_expires_or_compensates_but_promoted_fence_requires_committed_clear() {
    let state = GatewayState::default();
    let token = uuid::Uuid::new_v4();
    assert!(
        prepare_gateway_client_suspension_fence(
            &state,
            GatewayClientSuspensionFencePrepare {
                client_id: "client-a".to_string(),
                token,
                lease_secs: 60,
            },
        )
        .await
        .accepted
    );
    assert!(
        clear_gateway_client_suspension_fence(
            &state,
            GatewayClientSuspensionFenceClear {
                client_id: "client-a".to_string(),
                expected_token: Some(token),
                reason: "db_failed".to_string(),
            },
        )
        .await
        .accepted
    );

    let promoted_token = uuid::Uuid::new_v4();
    prepare_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFencePrepare {
            client_id: "client-a".to_string(),
            token: promoted_token,
            lease_secs: 60,
        },
    )
    .await;
    assert!(
        promote_gateway_client_suspension_fence(
            &state,
            GatewayClientSuspensionFencePromote {
                client_id: "client-a".to_string(),
                token: promoted_token,
            },
        )
        .await
        .accepted
    );
    assert!(state.client_suspension_fences.read().await["client-a"]
        .expires_at
        .is_none());
    let mismatched_compensation = clear_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFenceClear {
            client_id: "client-a".to_string(),
            expected_token: Some(uuid::Uuid::new_v4()),
            reason: "stale_compensation".to_string(),
        },
    )
    .await;
    assert!(!mismatched_compensation.accepted);
    assert!(mismatched_compensation.fenced);
    let committed_clear = clear_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFenceClear {
            client_id: "client-a".to_string(),
            expected_token: None,
            reason: "operator_unsuspended".to_string(),
        },
    )
    .await;
    assert!(committed_clear.accepted);
    assert!(!committed_clear.fenced);

    let expiring_token = uuid::Uuid::new_v4();
    prepare_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFencePrepare {
            client_id: "client-b".to_string(),
            token: expiring_token,
            lease_secs: 60,
        },
    )
    .await;
    state
        .client_suspension_fences
        .write()
        .await
        .get_mut("client-b")
        .unwrap()
        .expires_at = Some(Instant::now() - Duration::from_secs(1));
    assert!(!state.client_suspension_fences.read().await["client-b"].active_at(Instant::now()));

    let process_incarnation_id = uuid::Uuid::new_v4();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (close_tx, _close_rx) = tokio::sync::watch::channel(None);
    state.sessions.write().await.insert(
        "client-b".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id,
            sender,
            close_tx,
        },
    );
    let dispatch_state = state.clone();
    let dispatch = tokio::spawn(async move {
        dispatch_gateway_command(
            &dispatch_state,
            GatewayCommandDispatch {
                client_id: "client-b".to_string(),
                request: test_job_request(),
                expected_process_incarnation_id: process_incarnation_id,
                payload_hash: "test-payload-hash".to_string(),
            },
        )
        .await
    });
    let GatewaySessionMessage::Command(command) = receiver.recv().await.unwrap() else {
        panic!("expected command after prepared fence lease expiry");
    };
    let job_id = command.request.job_id;
    command
        .response
        .send(GatewayCommandDispatchResult {
            client_id: "client-b".to_string(),
            job_id,
            command_version: 1,
            accepted: true,
            message: "accepted".to_string(),
            outputs: Vec::new(),
        })
        .unwrap();
    assert!(dispatch.await.unwrap().unwrap().accepted);
}

#[tokio::test]
async fn repeated_prepare_keeps_exact_client_enqueue_protection() {
    let state = GatewayState::default();
    let token = uuid::Uuid::new_v4();
    let protected_job_id = uuid::Uuid::new_v4();
    let expired_job_id = uuid::Uuid::new_v4();
    state
        .command_enqueues
        .write()
        .await
        .entry("client-a".to_string())
        .or_default()
        .insert(
            protected_job_id,
            GatewayCommandEnqueueMarker {
                generation: uuid::Uuid::new_v4(),
                expires_at: Instant::now() + Duration::from_secs(120),
            },
        );
    state
        .command_enqueues
        .write()
        .await
        .entry("client-b".to_string())
        .or_default()
        .insert(
            expired_job_id,
            GatewayCommandEnqueueMarker {
                generation: uuid::Uuid::new_v4(),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );

    let first = prepare_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFencePrepare {
            client_id: "client-a".to_string(),
            token,
            lease_secs: 60,
        },
    )
    .await;
    let retry = prepare_gateway_client_suspension_fence(
        &state,
        GatewayClientSuspensionFencePrepare {
            client_id: "client-a".to_string(),
            token,
            lease_secs: 60,
        },
    )
    .await;
    assert_eq!(first.enqueued_job_ids, vec![protected_job_id]);
    assert_eq!(retry.enqueued_job_ids, vec![protected_job_id]);
    assert!(retry.accepted && retry.fenced);

    assert_eq!(
        state.prune_expired_command_enqueues(Instant::now()).await,
        1,
        "exact-client reads leave unrelated expiry to the cleanup owner"
    );
    assert_eq!(state.command_enqueues.read().await.len(), 1);
}

#[tokio::test]
async fn suspension_fence_batch_rejects_invalid_shape_before_any_mutation() {
    let state = GatewayState::default();
    let token = uuid::Uuid::new_v4();

    let empty = prepare_gateway_client_suspension_fence_batch(
        &state,
        GatewayClientSuspensionFencePrepareBatchRequest { items: Vec::new() },
    )
    .await
    .unwrap_err();
    assert!(empty.contains("size_out_of_range"));

    let duplicate = prepare_gateway_client_suspension_fence_batch(
        &state,
        GatewayClientSuspensionFencePrepareBatchRequest {
            items: vec![
                GatewayClientSuspensionFencePrepare {
                    client_id: "client-a".to_string(),
                    token,
                    lease_secs: 60,
                },
                GatewayClientSuspensionFencePrepare {
                    client_id: "client-a".to_string(),
                    token: uuid::Uuid::new_v4(),
                    lease_secs: 60,
                },
            ],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        duplicate,
        "suspension_fence_batch_duplicate_client_id:client-a"
    );

    let oversized = prepare_gateway_client_suspension_fence_batch(
        &state,
        GatewayClientSuspensionFencePrepareBatchRequest {
            items: (0..=GATEWAY_CLIENT_SUSPENSION_FENCE_BATCH_MAX_ITEMS)
                .map(|index| GatewayClientSuspensionFencePrepare {
                    client_id: format!("client-{index}"),
                    token: uuid::Uuid::new_v4(),
                    lease_secs: 60,
                })
                .collect(),
        },
    )
    .await
    .unwrap_err();
    assert!(oversized.contains("size_out_of_range"));
    assert!(state.client_suspension_fences.read().await.is_empty());
}

#[tokio::test]
async fn suspension_fence_batches_preserve_order_and_isolate_per_client_conflicts() {
    let state = GatewayState::default();
    let client_a_token = uuid::Uuid::new_v4();
    let client_b_token = uuid::Uuid::new_v4();
    let protected_job_id = uuid::Uuid::new_v4();
    state
        .command_enqueues
        .write()
        .await
        .entry("client-a".to_string())
        .or_default()
        .insert(
            protected_job_id,
            GatewayCommandEnqueueMarker {
                generation: uuid::Uuid::new_v4(),
                expires_at: Instant::now() + Duration::from_secs(120),
            },
        );
    assert!(
        prepare_gateway_client_suspension_fence(
            &state,
            GatewayClientSuspensionFencePrepare {
                client_id: "client-b".to_string(),
                token: client_b_token,
                lease_secs: 60,
            },
        )
        .await
        .accepted
    );

    let prepared = prepare_gateway_client_suspension_fence_batch(
        &state,
        GatewayClientSuspensionFencePrepareBatchRequest {
            items: vec![
                GatewayClientSuspensionFencePrepare {
                    client_id: "client-b".to_string(),
                    token: uuid::Uuid::new_v4(),
                    lease_secs: 60,
                },
                GatewayClientSuspensionFencePrepare {
                    client_id: "client-a".to_string(),
                    token: client_a_token,
                    lease_secs: 60,
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(prepared.results[0].client_id, "client-b");
    assert!(!prepared.results[0].accepted);
    assert_eq!(prepared.results[1].client_id, "client-a");
    assert!(prepared.results[1].accepted);
    assert_eq!(prepared.results[1].enqueued_job_ids, vec![protected_job_id]);

    let promoted = promote_gateway_client_suspension_fence_batch(
        &state,
        GatewayClientSuspensionFencePromoteBatchRequest {
            items: vec![
                GatewayClientSuspensionFencePromote {
                    client_id: "client-b".to_string(),
                    token: uuid::Uuid::new_v4(),
                },
                GatewayClientSuspensionFencePromote {
                    client_id: "client-a".to_string(),
                    token: client_a_token,
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(promoted.results[0].client_id, "client-b");
    assert!(!promoted.results[0].accepted);
    assert_eq!(promoted.results[1].client_id, "client-a");
    assert!(promoted.results[1].accepted);

    let cleared = clear_gateway_client_suspension_fence_batch(
        &state,
        GatewayClientSuspensionFenceClearBatchRequest {
            items: vec![
                GatewayClientSuspensionFenceClear {
                    client_id: "client-a".to_string(),
                    expected_token: None,
                    reason: "committed_unsuspend".to_string(),
                },
                GatewayClientSuspensionFenceClear {
                    client_id: "client-b".to_string(),
                    expected_token: Some(uuid::Uuid::new_v4()),
                    reason: "stale_compensation".to_string(),
                },
            ],
        },
    )
    .await
    .unwrap();
    assert_eq!(cleared.results[0].client_id, "client-a");
    assert!(cleared.results[0].accepted);
    assert!(!cleared.results[0].fenced);
    assert_eq!(cleared.results[1].client_id, "client-b");
    assert!(!cleared.results[1].accepted);
    assert!(cleared.results[1].fenced);
}

#[tokio::test]
async fn failed_same_key_enqueue_cannot_erase_a_later_dispatch_marker() {
    let state = GatewayState::default();
    let key = ("client-a".to_string(), uuid::Uuid::new_v4());
    let first_marker = GatewayCommandEnqueueMarker {
        generation: uuid::Uuid::new_v4(),
        expires_at: Instant::now() + Duration::from_secs(60),
    };
    let second_marker = GatewayCommandEnqueueMarker {
        generation: uuid::Uuid::new_v4(),
        // Equal expiry proves rollback ownership is the unique generation,
        // not clock resolution or a timestamp comparison.
        expires_at: first_marker.expires_at,
    };
    assert!(state
        .command_enqueues
        .write()
        .await
        .entry(key.0.clone())
        .or_default()
        .insert(key.1, first_marker)
        .is_none());
    let second_prior = state
        .command_enqueues
        .write()
        .await
        .entry(key.0.clone())
        .or_default()
        .insert(key.1, second_marker);

    rollback_failed_command_enqueue(&state, key.clone(), first_marker, None).await;
    assert_eq!(
        state
            .command_enqueues
            .read()
            .await
            .get(&key.0)
            .and_then(|client_enqueues| client_enqueues.get(&key.1)),
        Some(&second_marker)
    );

    rollback_failed_command_enqueue(&state, key.clone(), second_marker, second_prior).await;
    assert_eq!(
        state
            .command_enqueues
            .read()
            .await
            .get(&key.0)
            .and_then(|client_enqueues| client_enqueues.get(&key.1)),
        Some(&first_marker)
    );
}

fn test_job_request() -> JobRequest {
    JobRequest {
        job_id: uuid::Uuid::new_v4(),
        command_version: 1,
        command: JobCommand::Shell {
            argv: vec!["/bin/true".to_string()],
            pty: false,
        },
        max_timeout_secs: 30,
    }
}
