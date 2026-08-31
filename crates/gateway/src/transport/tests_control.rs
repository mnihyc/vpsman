use super::*;
use crate::state::{
    GatewayClientDispatchFenceState, GatewayCommandEnqueueMarker, GatewaySession,
    GatewaySessionCloseRequest, SESSION_COMMAND_QUEUE_CAPACITY,
};
use vpsman_common::{JobCommand, JobRequest, TerminalControlAck, TerminalControlRequest};

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
            expected_gateway_epoch: Some(state.client_dispatch_fence_epoch),
            payload_hash: "test-payload-hash".to_string(),
            lifecycle_recheck: None,
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
            required_dispatch_fence_owner: None,
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
                required_dispatch_fence_owner: None,
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
                    required_dispatch_fence_owner: None,
                },
                GatewaySessionDisconnect {
                    client_id: "client-b".to_string(),
                    reason: "vps_deleted".to_string(),
                    required_dispatch_fence_owner: None,
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
                    required_dispatch_fence_owner: None,
                },
                GatewaySessionDisconnect {
                    client_id: "client-a".to_string(),
                    reason: "vps_deleted".to_string(),
                    required_dispatch_fence_owner: None,
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

async fn acquire_test_fence(
    state: &GatewayState,
    client_id: &str,
    token: uuid::Uuid,
    purpose: GatewayClientDispatchFencePurpose,
) -> vpsman_common::GatewayClientDispatchFenceOwner {
    acquire_gateway_client_dispatch_fence(
        state,
        GatewayClientDispatchFenceAcquire {
            client_id: client_id.to_string(),
            token,
            purpose,
            supersede_prepared_suspension: false,
        },
    )
    .await
    .expect("test fence acquire")
    .owner
}

fn test_fence_prepare(
    client_id: &str,
    owner: vpsman_common::GatewayClientDispatchFenceOwner,
    purpose: GatewayClientDispatchFencePurpose,
    renewal: bool,
) -> GatewayClientDispatchFencePrepare {
    GatewayClientDispatchFencePrepare {
        client_id: client_id.to_string(),
        token: owner.token,
        gateway_epoch: owner.gateway_epoch,
        generation: owner.generation,
        renewal,
        lease_secs: 60,
        purpose,
    }
}

fn test_fence_promote(
    client_id: &str,
    owner: vpsman_common::GatewayClientDispatchFenceOwner,
    purpose: GatewayClientDispatchFencePurpose,
) -> GatewayClientDispatchFencePromote {
    GatewayClientDispatchFencePromote {
        client_id: client_id.to_string(),
        token: owner.token,
        gateway_epoch: owner.gateway_epoch,
        generation: owner.generation,
        purpose,
    }
}

fn test_fence_clear(
    client_id: &str,
    owner: vpsman_common::GatewayClientDispatchFenceOwner,
    restore_fallback: bool,
) -> GatewayClientDispatchFenceClear {
    GatewayClientDispatchFenceClear {
        client_id: client_id.to_string(),
        expected_token: owner.token,
        gateway_epoch: owner.gateway_epoch,
        expected_generation: owner.generation,
        restore_fallback,
        reason: "test_finalization".to_string(),
    }
}

async fn expire_test_fence(state: &GatewayState, client_id: &str) {
    let mut fences = state.client_dispatch_fences.write().await;
    let fence = fences.get_mut(client_id).expect("test fence");
    let GatewayClientDispatchFenceState::Prepared { expires_at, .. } = &mut fence.state else {
        panic!("test fence must be prepared");
    };
    *expires_at = Instant::now() - Duration::from_secs(1);
}

#[tokio::test]
async fn command_enqueue_before_fence_is_reported_by_prepare_and_promote() {
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
                expected_gateway_epoch: Some(dispatch_state.client_dispatch_fence_epoch),
                payload_hash: "test-payload-hash".to_string(),
                lifecycle_recheck: None,
            },
        )
        .await
    });
    let GatewaySessionMessage::Command(command) = receiver.recv().await.unwrap() else {
        panic!("expected command enqueue");
    };

    let owner = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    let prepared = prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            owner,
            GatewayClientDispatchFencePurpose::Suspension,
            false,
        ),
    )
    .await;
    assert!(prepared.accepted && prepared.fenced);
    assert_eq!(prepared.enqueued_job_ids, vec![job_id]);

    let promoted = promote_gateway_client_dispatch_fence(
        &state,
        test_fence_promote(
            "client-a",
            owner,
            GatewayClientDispatchFencePurpose::Suspension,
        ),
    )
    .await;
    assert!(promoted.accepted && promoted.fenced);
    assert_eq!(promoted.enqueued_job_ids, vec![job_id]);

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
async fn active_owner_rejects_dispatch_and_conflicting_acquire_without_preempting_renewal() {
    let state = GatewayState::default();
    let token_a = uuid::Uuid::new_v4();
    let owner_a = acquire_test_fence(
        &state,
        "client-a",
        token_a,
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                owner_a,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );

    let conflict = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Deletion,
            supersede_prepared_suspension: false,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(conflict, "dispatch_fence_conflict");
    let generation = state.client_dispatch_fence_generations.read().await["client-a"];
    assert_eq!(generation.latest_generation, owner_a.generation);
    assert_eq!(generation.latest_token, Some(token_a));

    let renewed = prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            owner_a,
            GatewayClientDispatchFencePurpose::Suspension,
            true,
        ),
    )
    .await;
    assert!(renewed.accepted && renewed.ownership_continuous);

    let dispatch = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            client_id: "client-a".to_string(),
            request: test_job_request(),
            expected_process_incarnation_id: uuid::Uuid::new_v4(),
            expected_gateway_epoch: Some(state.client_dispatch_fence_epoch),
            payload_hash: "test-payload-hash".to_string(),
            lifecycle_recheck: None,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(dispatch, "agent_suspended:client-a");
}

#[tokio::test]
async fn exact_unsuspend_transition_supersedes_only_an_older_prepared_suspension() {
    let state = GatewayState::default();
    let suspension = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                suspension,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );

    let ordinary_conflict = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: false,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(ordinary_conflict, "dispatch_fence_conflict");

    let unsuspend_token = uuid::Uuid::new_v4();
    let unsuspend = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token: unsuspend_token,
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: true,
        },
    )
    .await
    .expect("exact unsuspend transition acquire")
    .owner;
    assert!(unsuspend.generation > suspension.generation);
    let prepared_unsuspend = prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            unsuspend,
            GatewayClientDispatchFencePurpose::Suspension,
            false,
        ),
    )
    .await;
    assert!(prepared_unsuspend.accepted && prepared_unsuspend.fenced);
    assert_eq!(
        prepared_unsuspend.message,
        "dispatch_fence_superseded_prepared_suspension"
    );
    let fallback = state.client_dispatch_fences.read().await["client-a"]
        .fallback()
        .expect("superseded suspension recovery owner");
    assert_eq!(fallback.token, suspension.token);
    assert!(fallback.requires_durable_recheck);

    let cleared =
        clear_gateway_client_dispatch_fence(&state, test_fence_clear("client-a", unsuspend, false))
            .await;
    assert!(cleared.accepted && !cleared.fenced);
    assert!(!state
        .client_dispatch_fences
        .read()
        .await
        .contains_key("client-a"));

    let delayed_promotion = promote_gateway_client_dispatch_fence(
        &state,
        test_fence_promote(
            "client-a",
            suspension,
            GatewayClientDispatchFencePurpose::Suspension,
        ),
    )
    .await;
    assert!(!delayed_promotion.accepted && !delayed_promotion.fenced);
    assert_eq!(
        delayed_promotion.message,
        "dispatch_fence_generation_retired"
    );
    let dispatch = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            client_id: "client-a".to_string(),
            request: test_job_request(),
            expected_process_incarnation_id: uuid::Uuid::new_v4(),
            expected_gateway_epoch: Some(state.client_dispatch_fence_epoch),
            payload_hash: "test-payload-hash".to_string(),
            lifecycle_recheck: None,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(dispatch, "agent_not_online:client-a");

    let deletion = acquire_test_fence(
        &state,
        "client-b",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Deletion,
    )
    .await;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-b",
                deletion,
                GatewayClientDispatchFencePurpose::Deletion,
                false,
            ),
        )
        .await
        .accepted
    );
    let deletion_conflict = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-b".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: true,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(deletion_conflict, "dispatch_fence_conflict");
    assert_eq!(
        state.client_dispatch_fences.read().await["client-b"].owner(),
        deletion
    );

    let rejected_transition_suspension = acquire_test_fence(
        &state,
        "client-c",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-c",
            rejected_transition_suspension,
            GatewayClientDispatchFencePurpose::Suspension,
            false,
        ),
    )
    .await;
    let rejected_unsuspend = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-c".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: true,
        },
    )
    .await
    .expect("rejected unsuspend owner acquire")
    .owner;
    prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-c",
            rejected_unsuspend,
            GatewayClientDispatchFencePurpose::Suspension,
            false,
        ),
    )
    .await;
    let rejected = clear_gateway_client_dispatch_fence(
        &state,
        test_fence_clear("client-c", rejected_unsuspend, true),
    )
    .await;
    assert!(rejected.accepted && rejected.fenced);
    let restored = state.client_dispatch_fences.read().await["client-c"];
    assert_eq!(restored.owner(), rejected_transition_suspension);
    assert!(restored.requires_durable_recheck());
}

#[tokio::test]
async fn newer_suspension_starts_only_after_exact_unsuspend_cleanup_finishes() {
    let state = GatewayState::default();
    let unsuspend = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: true,
        },
    )
    .await
    .expect("exact unsuspend transition acquire")
    .owner;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                unsuspend,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );

    let blocked_suspend = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: false,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(blocked_suspend, "dispatch_fence_conflict");

    let cleared =
        clear_gateway_client_dispatch_fence(&state, test_fence_clear("client-a", unsuspend, false))
            .await;
    assert!(cleared.accepted && !cleared.fenced);

    let suspension = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                suspension,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );
    assert!(
        promote_gateway_client_dispatch_fence(
            &state,
            test_fence_promote(
                "client-a",
                suspension,
                GatewayClientDispatchFencePurpose::Suspension,
            ),
        )
        .await
        .accepted
    );

    let delayed_unsuspend_cleanup =
        clear_gateway_client_dispatch_fence(&state, test_fence_clear("client-a", unsuspend, false))
            .await;
    assert!(!delayed_unsuspend_cleanup.accepted && delayed_unsuspend_cleanup.fenced);
    assert_eq!(
        state.client_dispatch_fences.read().await["client-a"].owner(),
        suspension
    );
}

#[tokio::test]
async fn failed_unsuspend_without_fallback_expires_to_durable_recheck() {
    let state = GatewayState::default();
    let unsuspend = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token: uuid::Uuid::new_v4(),
            purpose: GatewayClientDispatchFencePurpose::Suspension,
            supersede_prepared_suspension: true,
        },
    )
    .await
    .expect("exact unsuspend transition acquire")
    .owner;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                unsuspend,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );
    assert!(state.client_dispatch_fences.read().await["client-a"]
        .fallback()
        .is_none());

    expire_test_fence(&state, "client-a").await;
    let error = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            client_id: "client-a".to_string(),
            request: test_job_request(),
            expected_process_incarnation_id: uuid::Uuid::new_v4(),
            expected_gateway_epoch: Some(state.client_dispatch_fence_epoch),
            payload_hash: "test-payload-hash".to_string(),
            lifecycle_recheck: None,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("agent_lifecycle_recheck_required:"));
    assert!(matches!(
        state.client_dispatch_fences.read().await["client-a"].state,
        GatewayClientDispatchFenceState::DurableRecheck { fallback: None }
    ));
}

#[tokio::test]
async fn same_token_acquire_requires_the_same_purpose_before_or_after_prepare() {
    let state = GatewayState::default();
    let token = uuid::Uuid::new_v4();
    let owner = acquire_test_fence(
        &state,
        "client-a",
        token,
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    let preprepare_conflict = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token,
            purpose: GatewayClientDispatchFencePurpose::Deletion,
            supersede_prepared_suspension: false,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(preprepare_conflict, "dispatch_fence_token_purpose_conflict");

    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                owner,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );
    let postprepare_conflict = acquire_gateway_client_dispatch_fence(
        &state,
        GatewayClientDispatchFenceAcquire {
            client_id: "client-a".to_string(),
            token,
            purpose: GatewayClientDispatchFencePurpose::Deletion,
            supersede_prepared_suspension: false,
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(
        postprepare_conflict,
        "dispatch_fence_token_purpose_conflict"
    );
}

#[tokio::test]
async fn exact_clear_tombstones_delayed_initial_prepare_and_renewal() {
    let state = GatewayState::default();
    let owner_before_prepare = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    assert!(
        clear_gateway_client_dispatch_fence(
            &state,
            test_fence_clear("client-a", owner_before_prepare, true),
        )
        .await
        .accepted
    );
    let delayed_prepare = prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            owner_before_prepare,
            GatewayClientDispatchFencePurpose::Suspension,
            false,
        ),
    )
    .await;
    assert!(!delayed_prepare.accepted);
    assert_eq!(delayed_prepare.message, "dispatch_fence_generation_stale");
    assert!(!state
        .client_dispatch_fences
        .read()
        .await
        .contains_key("client-a"));

    let owner = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                owner,
                GatewayClientDispatchFencePurpose::Suspension,
                false,
            ),
        )
        .await
        .accepted
    );
    assert!(
        clear_gateway_client_dispatch_fence(&state, test_fence_clear("client-a", owner, true))
            .await
            .accepted
    );
    let delayed_renewal = prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            owner,
            GatewayClientDispatchFencePurpose::Suspension,
            true,
        ),
    )
    .await;
    assert!(!delayed_renewal.accepted);
    assert!(!state
        .client_dispatch_fences
        .read()
        .await
        .contains_key("client-a"));
}

#[tokio::test]
async fn expired_prepare_becomes_a_non_consuming_per_request_durable_recheck_barrier() {
    let state = GatewayState::default();
    let owner = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Deletion,
    )
    .await;
    assert!(
        prepare_gateway_client_dispatch_fence(
            &state,
            test_fence_prepare(
                "client-a",
                owner,
                GatewayClientDispatchFencePurpose::Deletion,
                false,
            ),
        )
        .await
        .accepted
    );
    expire_test_fence(&state, "client-a").await;

    let dispatch_without_proof = || GatewayCommandDispatch {
        client_id: "client-a".to_string(),
        request: test_job_request(),
        expected_process_incarnation_id: uuid::Uuid::new_v4(),
        expected_gateway_epoch: Some(state.client_dispatch_fence_epoch),
        payload_hash: "test-payload-hash".to_string(),
        lifecycle_recheck: None,
    };
    for _ in 0..2 {
        let error = dispatch_gateway_command(&state, dispatch_without_proof())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("agent_lifecycle_recheck_required:"));
        assert!(error.contains(&owner.token.to_string()));
    }
    assert!(matches!(
        state.client_dispatch_fences.read().await["client-a"].state,
        GatewayClientDispatchFenceState::DurableRecheck { .. }
    ));

    let proved = dispatch_gateway_command(
        &state,
        GatewayCommandDispatch {
            lifecycle_recheck: Some(owner),
            ..dispatch_without_proof()
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(proved, "agent_not_online:client-a");
    assert!(matches!(
        state.client_dispatch_fences.read().await["client-a"].state,
        GatewayClientDispatchFenceState::DurableRecheck { .. }
    ));
}

#[tokio::test]
async fn replacement_has_one_flat_fallback_and_retired_finalizers_cannot_remove_it() {
    let state = GatewayState::default();
    let suspension = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            suspension,
            GatewayClientDispatchFencePurpose::Suspension,
            false,
        ),
    )
    .await;
    assert!(
        promote_gateway_client_dispatch_fence(
            &state,
            test_fence_promote(
                "client-a",
                suspension,
                GatewayClientDispatchFencePurpose::Suspension,
            ),
        )
        .await
        .accepted
    );

    let deletion = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Deletion,
    )
    .await;
    let replacement = prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            deletion,
            GatewayClientDispatchFencePurpose::Deletion,
            false,
        ),
    )
    .await;
    assert!(replacement.accepted && !replacement.ownership_continuous);
    let fence = state.client_dispatch_fences.read().await["client-a"];
    let fallback = fence.fallback().expect("flat suspension fallback");
    assert_eq!(fallback.token, suspension.token);
    assert!(!fallback.requires_durable_recheck);

    let compensated =
        clear_gateway_client_dispatch_fence(&state, test_fence_clear("client-a", deletion, true))
            .await;
    assert!(compensated.accepted && compensated.fenced);
    assert_eq!(
        state.client_dispatch_fences.read().await["client-a"].owner(),
        suspension
    );

    let stale_finalizer = clear_gateway_client_dispatch_fence(
        &state,
        test_fence_clear("client-a", suspension, false),
    )
    .await;
    assert!(!stale_finalizer.accepted);
    assert_eq!(stale_finalizer.message, "dispatch_fence_generation_retired");
    assert_eq!(
        state.client_dispatch_fences.read().await["client-a"].owner(),
        suspension
    );
}

#[tokio::test]
async fn deletion_disconnect_requires_the_exact_persistent_nonretired_owner() {
    let state = GatewayState::default();
    let process_incarnation_id = uuid::Uuid::new_v4();
    let (sender, _receiver) = tokio::sync::mpsc::channel(1);
    let (close_tx, mut close_rx) = tokio::sync::watch::channel(None);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id,
            sender,
            close_tx,
        },
    );

    let first = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Deletion,
    )
    .await;
    prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            first,
            GatewayClientDispatchFencePurpose::Deletion,
            false,
        ),
    )
    .await;
    let premature = disconnect_gateway_session(
        &state,
        GatewaySessionDisconnect {
            client_id: "client-a".to_string(),
            reason: "vps_deleted".to_string(),
            required_dispatch_fence_owner: Some(first),
        },
    )
    .await
    .unwrap();
    assert!(!premature.disconnected);
    assert!(state.sessions.read().await.contains_key("client-a"));

    promote_gateway_client_dispatch_fence(
        &state,
        test_fence_promote(
            "client-a",
            first,
            GatewayClientDispatchFencePurpose::Deletion,
        ),
    )
    .await;
    let second = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Deletion,
    )
    .await;
    prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            second,
            GatewayClientDispatchFencePurpose::Deletion,
            false,
        ),
    )
    .await;
    let stale = disconnect_gateway_session(
        &state,
        GatewaySessionDisconnect {
            client_id: "client-a".to_string(),
            reason: "stale_delete".to_string(),
            required_dispatch_fence_owner: Some(first),
        },
    )
    .await
    .unwrap();
    assert!(!stale.disconnected);
    assert!(state.sessions.read().await.contains_key("client-a"));

    promote_gateway_client_dispatch_fence(
        &state,
        test_fence_promote(
            "client-a",
            second,
            GatewayClientDispatchFencePurpose::Deletion,
        ),
    )
    .await;
    let exact = disconnect_gateway_session(
        &state,
        GatewaySessionDisconnect {
            client_id: "client-a".to_string(),
            reason: "vps_deleted".to_string(),
            required_dispatch_fence_owner: Some(second),
        },
    )
    .await
    .unwrap();
    assert!(exact.accepted && exact.disconnected);
    close_rx.changed().await.unwrap();
    assert!(!state.sessions.read().await.contains_key("client-a"));
}

#[tokio::test]
async fn gateway_epoch_mismatch_requires_a_fresh_db_bound_retry() {
    let state = GatewayState::default();
    let request = || GatewayCommandDispatch {
        client_id: "client-a".to_string(),
        request: test_job_request(),
        expected_process_incarnation_id: uuid::Uuid::new_v4(),
        expected_gateway_epoch: None,
        payload_hash: "test-payload-hash".to_string(),
        lifecycle_recheck: None,
    };
    let error = dispatch_gateway_command(&state, request())
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(
        error,
        format!(
            "agent_gateway_epoch_recheck_required:{}",
            state.client_dispatch_fence_epoch
        )
    );
    let with_epoch = GatewayCommandDispatch {
        expected_gateway_epoch: Some(state.client_dispatch_fence_epoch),
        ..request()
    };
    assert_eq!(
        dispatch_gateway_command(&state, with_epoch)
            .await
            .unwrap_err()
            .to_string(),
        "agent_not_online:client-a"
    );
}

#[tokio::test]
async fn deletion_fence_blocks_constructive_terminal_control_but_not_close() {
    let state = GatewayState::default();
    let process_incarnation_id = uuid::Uuid::new_v4();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
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
    let owner = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Deletion,
    )
    .await;
    prepare_gateway_client_dispatch_fence(
        &state,
        test_fence_prepare(
            "client-a",
            owner,
            GatewayClientDispatchFencePurpose::Deletion,
            false,
        ),
    )
    .await;

    let terminal_session_id = uuid::Uuid::new_v4();
    let input_error = dispatch_terminal_control(
        &state,
        GatewayTerminalControl {
            client_id: "client-a".to_string(),
            expected_process_incarnation_id: process_incarnation_id,
            request: TerminalControlRequest {
                request_id: uuid::Uuid::new_v4(),
                session_id: terminal_session_id,
                action: TerminalControlAction::Input {
                    data_base64: "YQ==".to_string(),
                },
            },
        },
    )
    .await
    .unwrap_err()
    .to_string();
    assert_eq!(input_error, "agent_lifecycle_fenced:client-a");
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    let close_request_id = uuid::Uuid::new_v4();
    let close_state = state.clone();
    let close = tokio::spawn(async move {
        dispatch_terminal_control(
            &close_state,
            GatewayTerminalControl {
                client_id: "client-a".to_string(),
                expected_process_incarnation_id: process_incarnation_id,
                request: TerminalControlRequest {
                    request_id: close_request_id,
                    session_id: terminal_session_id,
                    action: TerminalControlAction::Close {
                        reason: Some("operator".to_string()),
                    },
                },
            },
        )
        .await
    });
    let GatewaySessionMessage::TerminalControl(control) = receiver.recv().await.unwrap() else {
        panic!("close must remain subtractive terminal cleanup");
    };
    control
        .response
        .send(TerminalControlAck {
            request_id: close_request_id,
            session_id: terminal_session_id,
            action: "close".to_string(),
            accepted: true,
            status: "closed".to_string(),
            message: "closed".to_string(),
            input_seq: None,
            written_bytes: None,
            cols: None,
            rows: None,
        })
        .unwrap();
    assert!(close.await.unwrap().unwrap().ack.accepted);
}

#[tokio::test]
async fn dispatch_fence_batch_validation_happens_before_mutation() {
    let state = GatewayState::default();
    let owner = acquire_test_fence(
        &state,
        "client-a",
        uuid::Uuid::new_v4(),
        GatewayClientDispatchFencePurpose::Suspension,
    )
    .await;
    let duplicate = prepare_gateway_client_dispatch_fence_batch(
        &state,
        GatewayClientDispatchFencePrepareBatchRequest {
            items: vec![
                test_fence_prepare(
                    "client-a",
                    owner,
                    GatewayClientDispatchFencePurpose::Suspension,
                    false,
                ),
                test_fence_prepare(
                    "client-a",
                    owner,
                    GatewayClientDispatchFencePurpose::Suspension,
                    false,
                ),
            ],
        },
    )
    .await
    .unwrap_err();
    assert_eq!(
        duplicate,
        "dispatch_fence_batch_duplicate_client_id:client-a"
    );
    assert!(state.client_dispatch_fences.read().await.is_empty());
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
