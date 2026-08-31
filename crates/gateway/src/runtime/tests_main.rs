use super::*;
use crate::state::GatewayClientSuspensionFence;

#[tokio::test]
async fn stale_reconnect_cleanup_does_not_remove_newer_session() {
    let state = GatewayState::default();
    let older_session_id = uuid::Uuid::new_v4();
    let newer_session_id = uuid::Uuid::new_v4();
    let (older_tx, _older_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (newer_tx, _newer_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (older_close_tx, _older_close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    let (newer_close_tx, _newer_close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: older_session_id,
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender: older_tx,
            close_tx: older_close_tx,
        },
    );
    let newer_process_incarnation_id = uuid::Uuid::new_v4();
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: newer_session_id,
            process_incarnation_id: newer_process_incarnation_id,
            sender: newer_tx,
            close_tx: newer_close_tx,
        },
    );

    unregister_session_if_current(&state, "client-a", older_session_id).await;
    assert_eq!(
        state
            .sessions
            .read()
            .await
            .get("client-a")
            .map(|session| session.session_id),
        Some(newer_session_id)
    );

    unregister_session_if_current(&state, "client-a", newer_session_id).await;
    assert!(!state.sessions.read().await.contains_key("client-a"));
}

#[tokio::test]
async fn connection_ownership_finalization_is_exact_once_and_cannot_remove_a_replacement() {
    let state = GatewayState::default();
    let control = GatewayControlClient::new(None, None, GatewayHttpTimeouts::default());
    let ownership = AgentConnectionOwnership::new("127.0.0.1:41000".parse().unwrap());
    ownership.set_client_id("client-a".to_string());
    let (first_tx, _first_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (first_close_tx, _first_close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: ownership.session_id,
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender: first_tx,
            close_tx: first_close_tx,
        },
    );

    ownership
        .finalize(
            &state,
            &control,
            "gateway-a",
            Some("test_complete".to_string()),
        )
        .await;
    assert!(!state.sessions.read().await.contains_key("client-a"));

    let replacement_id = uuid::Uuid::new_v4();
    let (replacement_tx, _replacement_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (replacement_close_tx, _replacement_close_rx) =
        watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: replacement_id,
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender: replacement_tx,
            close_tx: replacement_close_tx,
        },
    );
    ownership
        .finalize(
            &state,
            &control,
            "gateway-a",
            Some("stale_repeat".to_string()),
        )
        .await;

    assert_eq!(
        state
            .sessions
            .read()
            .await
            .get("client-a")
            .map(|session| session.session_id),
        Some(replacement_id)
    );
}

#[test]
fn shutdown_joins_connection_owners_before_forwarding_flush() {
    let source = include_str!("../main.rs");
    assert!(!source.contains("impl Drop for AgentConnectionFinalizer"));

    let (_, shutdown) = source
        .split_once("// Stop every producer first.")
        .expect("gateway shutdown owner");
    let (shutdown, _) = shutdown
        .split_once("match runtime_error")
        .expect("gateway shutdown boundary");
    let stop_listeners = shutdown
        .find("listener_shutdown.send(true)")
        .expect("listener shutdown");
    let join_agents = shutdown
        .find("agent_listener.as_mut().await")
        .expect("agent listener drain");
    let join_control = shutdown
        .find("control_listener.as_mut().await")
        .expect("control listener drain");
    let forwarding_flush = shutdown
        .find("api_client.shutdown_flush(shutdown_flush).await")
        .expect("forwarding shutdown flush");
    let join_forward_consumers = shutdown
        .find("(&mut forward_consumers_task).await")
        .expect("forwarding consumer drain");
    assert!(stop_listeners < join_agents);
    assert!(stop_listeners < join_control);
    assert!(join_agents < forwarding_flush);
    assert!(join_control < forwarding_flush);
    assert!(forwarding_flush < join_forward_consumers);

    let (_, listener) = source
        .split_once("async fn run_agent_listener")
        .expect("agent listener owner");
    let (listener, _) = listener
        .split_once("fn gateway_shutdown_requested")
        .expect("agent listener boundary");
    let stop_accepting = listener.find("drop(listener)").expect("listener close");
    let stop_connections = listener
        .find("connection_shutdown.send(true)")
        .expect("connection shutdown");
    let drain_connections = listener[stop_connections..]
        .find("connections.join_next().await")
        .map(|offset| stop_connections + offset)
        .expect("connection owner drain");
    assert!(stop_accepting < stop_connections);
    assert!(stop_connections < drain_connections);
    assert!(listener.contains(".finalize("));
}

#[tokio::test]
async fn registering_replacement_session_closes_displaced_session() {
    let state = GatewayState::default();
    let (older_tx, _older_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (newer_tx, _newer_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (older_close_tx, mut older_close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    let (newer_close_tx, _newer_close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    let newer_session_id = uuid::Uuid::new_v4();

    register_session_after_accepted_hello(
        &state,
        "client-a",
        GatewaySession {
            session_id: uuid::Uuid::new_v4(),
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender: older_tx,
            close_tx: older_close_tx,
        },
    )
    .await;
    register_session_after_accepted_hello(
        &state,
        "client-a",
        GatewaySession {
            session_id: newer_session_id,
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender: newer_tx,
            close_tx: newer_close_tx,
        },
    )
    .await;

    older_close_rx.changed().await.unwrap();
    assert_eq!(
        older_close_rx.borrow().as_ref(),
        Some(&GatewaySessionCloseRequest::Graceful(
            "replaced_by_new_session".to_string()
        ))
    );
    assert_eq!(
        state
            .sessions
            .read()
            .await
            .get("client-a")
            .map(|session| session.session_id),
        Some(newer_session_id)
    );
}

#[tokio::test]
async fn accepted_hello_clears_the_local_fence_and_registers_under_one_client_owner() {
    let state = std::sync::Arc::new(GatewayState::default());
    let fence_token = uuid::Uuid::new_v4();
    state.client_suspension_fences.write().await.insert(
        "client-a".to_string(),
        GatewayClientSuspensionFence {
            token: fence_token,
            expires_at: None,
        },
    );
    let lifecycle_owner = state.client_lifecycle_owner("client-a").await;
    let lifecycle_guard = lifecycle_owner.write().await;
    let session_id = uuid::Uuid::new_v4();
    let (sender, _receiver) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (close_tx, _close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);

    let registration = {
        let state = std::sync::Arc::clone(&state);
        tokio::spawn(async move {
            register_session_after_accepted_hello(
                &state,
                "client-a",
                GatewaySession {
                    session_id,
                    process_incarnation_id: uuid::Uuid::new_v4(),
                    sender,
                    close_tx,
                },
            )
            .await;
        })
    };
    tokio::task::yield_now().await;

    assert_eq!(
        state.client_suspension_fences.read().await["client-a"].token,
        fence_token
    );
    assert!(!state.sessions.read().await.contains_key("client-a"));

    drop(lifecycle_guard);
    tokio::time::timeout(std::time::Duration::from_secs(1), registration)
        .await
        .expect("accepted registration must not wait on remote control")
        .expect("accepted registration task");
    assert!(!state
        .client_suspension_fences
        .read()
        .await
        .contains_key("client-a"));
    assert_eq!(
        state
            .sessions
            .read()
            .await
            .get("client-a")
            .map(|session| session.session_id),
        Some(session_id)
    );
}

#[tokio::test]
async fn api_rejection_only_terminates_the_exact_current_session() {
    let state = GatewayState::default();
    let stale_session_id = uuid::Uuid::new_v4();
    let current_session_id = uuid::Uuid::new_v4();
    let (sender, _receiver) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (close_tx, mut close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: current_session_id,
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );

    assert!(
        !invalidate_agent_session_if_current(
            &state,
            "client-a",
            stale_session_id,
            "gateway_session_not_active",
        )
        .await
    );
    assert_eq!(
        state
            .sessions
            .read()
            .await
            .get("client-a")
            .map(|session| session.session_id),
        Some(current_session_id)
    );
    assert!(close_rx.has_changed().is_ok_and(|changed| !changed));

    assert!(
        invalidate_agent_session_if_current(
            &state,
            "client-a",
            current_session_id,
            "gateway_session_not_active",
        )
        .await
    );
    assert!(!state.sessions.read().await.contains_key("client-a"));
    close_rx.changed().await.unwrap();
    assert_eq!(
        close_rx.borrow().as_ref(),
        Some(&GatewaySessionCloseRequest::Immediate(
            "gateway_session_not_active".to_string()
        ))
    );
}

#[tokio::test]
async fn committed_telemetry_refreshes_only_the_exact_current_session_route() {
    let state = GatewayState::default();
    let stale_session_id = uuid::Uuid::new_v4();
    let current_session_id = uuid::Uuid::new_v4();
    let fence_token = uuid::Uuid::new_v4();
    let (sender, _receiver) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
    let (close_tx, _close_rx) = watch::channel(None::<GatewaySessionCloseRequest>);
    state.sessions.write().await.insert(
        "client-a".to_string(),
        GatewaySession {
            session_id: current_session_id,
            process_incarnation_id: uuid::Uuid::new_v4(),
            sender,
            close_tx,
        },
    );
    state.client_suspension_fences.write().await.insert(
        "client-a".to_string(),
        GatewayClientSuspensionFence {
            token: fence_token,
            expires_at: None,
        },
    );

    assert!(
        !refresh_client_route_after_committed_telemetry(&state, "client-a", stale_session_id,)
            .await
    );
    assert_eq!(
        state.client_suspension_fences.read().await["client-a"].token,
        fence_token
    );

    assert!(
        refresh_client_route_after_committed_telemetry(&state, "client-a", current_session_id,)
            .await
    );
    assert!(!state
        .client_suspension_fences
        .read()
        .await
        .contains_key("client-a"));
}

#[test]
fn internal_token_startup_validation_rejects_missing_short_or_placeholder() {
    assert!(required_internal_token(None).is_err());
    assert!(required_internal_token(Some("short")).is_err());
    assert!(required_internal_token(Some("change-me-internal-token")).is_err());
    assert!(required_internal_token(Some("dev-internal-token-change-me-32chars")).is_err());
    assert!(required_internal_token(Some("replace-with-random-token-at-least-32-chars")).is_err());
    assert!(required_internal_token(Some("real-internal-token-value-32-plus-chars")).is_ok());
}

#[test]
fn gateway_bind_defaults_to_loopback() {
    with_cleared_gateway_env(&["VPSMAN_GATEWAY_BIND"], || {
        let args = Args::parse_from(["vpsman-gateway"]);
        assert_eq!(args.bind, "127.0.0.1:9443");
    });
}

#[test]
fn gateway_telemetry_http_ownership_defaults_to_eight_and_rejects_invalid_limits() {
    with_cleared_gateway_env(&["VPSMAN_GATEWAY_TELEMETRY_IN_FLIGHT"], || {
        let args = Args::parse_from(["vpsman-gateway"]);
        assert_eq!(args.telemetry_in_flight, 8);
        for invalid in ["0", "513"] {
            assert!(
                Args::try_parse_from(["vpsman-gateway", "--telemetry-in-flight", invalid,])
                    .is_err()
            );
        }
    });
}

#[test]
fn gateway_suite_capacity_sets_restart_scoped_telemetry_http_ownership() {
    with_cleared_gateway_env(&["VPSMAN_GATEWAY_TELEMETRY_IN_FLIGHT"], || {
        let suite =
            SuiteConfig::parse("version = 1\n\n[capacity]\ngateway_telemetry_in_flight = 12\n")
                .unwrap();
        let mut args = test_args();

        args.apply_suite_config(&suite).unwrap();

        assert_eq!(args.telemetry_in_flight, 12);
    });
}

#[test]
fn socket_peer_canonicalization_only_unmaps_ipv4_mapped_ipv6() {
    let ipv4 = "192.0.2.10:59443".parse().unwrap();
    let ipv4_mapped = "[::ffff:192.0.2.10]:59443".parse().unwrap();
    let ipv6 = "[2001:db8::10]:59443".parse().unwrap();

    assert_eq!(canonicalize_peer_addr(ipv4), ipv4);
    assert_eq!(canonicalize_peer_addr(ipv4_mapped), ipv4);
    assert_eq!(canonicalize_peer_addr(ipv6), ipv6);
}

#[test]
fn runtime_mode_requires_identity_key_and_privilege_verifier() {
    let mut args = test_args();

    args.private_key_hex = None;
    assert!(validate_gateway_runtime_mode(&args)
        .unwrap_err()
        .to_string()
        .contains("VPSMAN_GATEWAY_PRIVATE_KEY_HEX is required"));

    args.private_key_hex = Some("11".repeat(32));
    args.privilege_verifier_key_hex = None;
    assert!(validate_gateway_runtime_mode(&args)
        .unwrap_err()
        .to_string()
        .contains("VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX is required"));

    args.privilege_verifier_key_hex = Some("11".repeat(32));
    args.api_url = None;
    assert!(validate_gateway_runtime_mode(&args)
        .unwrap_err()
        .to_string()
        .contains("VPSMAN_API_URL is required"));

    args.api_url = Some("http://127.0.0.1:8080".to_string());
    validate_gateway_runtime_mode(&args).unwrap();

    args.api_url = Some("https://127.0.0.1:8080".to_string());
    assert!(validate_gateway_runtime_mode(&args)
        .unwrap_err()
        .to_string()
        .contains("must use http://"));
}

#[test]
fn agent_connection_admission_records_rejection_when_full() {
    let permits = Arc::new(Semaphore::new(0));
    let client = GatewayControlClient::new(
        Some("http://127.0.0.1:8080".to_string()),
        None,
        GatewayHttpTimeouts::default(),
    );
    let peer = "127.0.0.1:10000".parse().unwrap();

    assert!(try_acquire_agent_connection_permit(&permits, &client, peer).is_none());
    assert_eq!(
        client
            .forward_metrics()
            .snapshot()
            .rejected_agent_connections,
        1
    );
}

#[test]
fn telemetry_client_id_must_match_authenticated_session() {
    validate_telemetry_session_client_id(Some("client-a"), "client-a").unwrap();
    assert_eq!(
        validate_telemetry_session_client_id(None, "client-a")
            .unwrap_err()
            .to_string(),
        "telemetry_before_hello"
    );
    assert_eq!(
        validate_telemetry_session_client_id(Some("client-a"), "client-b")
            .unwrap_err()
            .to_string(),
        "telemetry_client_id_mismatch"
    );
}

#[test]
fn gateway_runtime_config_reloads_suite_file_from_base_args() {
    with_cleared_gateway_env(GATEWAY_HOT_RELOAD_ENV, || {
        let path = temp_suite_config_path("gateway-hot-reload");
        std::fs::write(&path, gateway_runtime_toml(45, 31, 4, 5, 6, 7, 900)).unwrap();
        let mut args = test_args();
        args.suite_config = path.clone();

        let runtime = load_gateway_runtime_config(&args).unwrap();

        assert_eq!(runtime.reconnect_grace_secs, 45);
        assert_eq!(runtime.dispatch_ack_secs, 31);
        assert_eq!(runtime.http_timeouts.connect.as_secs(), 4);
        assert_eq!(runtime.http_timeouts.write.as_secs(), 5);
        assert_eq!(runtime.http_timeouts.read.as_secs(), 6);
        assert_eq!(runtime.http_timeouts.event_post.as_secs(), 7);
        assert_eq!(runtime.forward_config.command_output_event_ttl_secs, 900);

        std::fs::write(&path, gateway_runtime_toml(75, 41, 8, 9, 10, 11, 1800)).unwrap();

        let runtime = load_gateway_runtime_config(&args).unwrap();
        assert_eq!(runtime.reconnect_grace_secs, 75);
        assert_eq!(runtime.dispatch_ack_secs, 41);
        assert_eq!(runtime.http_timeouts.connect.as_secs(), 8);
        assert_eq!(runtime.http_timeouts.write.as_secs(), 9);
        assert_eq!(runtime.http_timeouts.read.as_secs(), 10);
        assert_eq!(runtime.http_timeouts.event_post.as_secs(), 11);
        assert_eq!(runtime.forward_config.command_output_event_ttl_secs, 1800);

        let _ = std::fs::remove_file(path);
    });
}

fn test_args() -> Args {
    Args {
        bind: "127.0.0.1:0".to_string(),
        control_bind: "127.0.0.1:0".to_string(),
        suite_config: std::path::PathBuf::from("config/vpsman.toml"),
        private_key_hex: Some("11".repeat(32)),
        expect_client_public_key_hex: None,
        api_url: Some("http://127.0.0.1:8080".to_string()),
        internal_token: Some("real-internal-token-value-32-plus-chars".to_string()),
        privilege_verifier_key_hex: Some("11".repeat(32)),
        gateway_id: "test-gateway".to_string(),
        reconnect_grace_secs: 60,
        internal_http_connect_secs: 10,
        internal_http_write_secs: 10,
        internal_http_read_secs: 15,
        event_post_secs: 15,
        dispatch_ack_secs: 30,
        spool_dir: std::path::PathBuf::from("./runtime/gateway-spool"),
        spool_ram_max_bytes: 1024 * 1024 * 1024,
        spool_disk_max_bytes: 4 * 1024 * 1024 * 1024,
        spool_shutdown_flush_secs: 30,
        command_output_event_ttl_secs: DEFAULT_COMMAND_OUTPUT_EVENT_TTL_SECS,
        telemetry_in_flight: DEFAULT_TELEMETRY_IN_FLIGHT,
    }
}

const GATEWAY_HOT_RELOAD_ENV: &[&str] = &[
    "VPSMAN_GATEWAY_RECONNECT_GRACE_SECS",
    "VPSMAN_INTERNAL_HTTP_CONNECT_SECS",
    "VPSMAN_INTERNAL_HTTP_WRITE_SECS",
    "VPSMAN_INTERNAL_HTTP_READ_SECS",
    "VPSMAN_EVENT_POST_SECS",
    "VPSMAN_DISPATCH_ACK_SECS",
    "VPSMAN_GATEWAY_SPOOL_DIR",
    "VPSMAN_GATEWAY_SPOOL_RAM_MAX_BYTES",
    "VPSMAN_GATEWAY_SPOOL_DISK_MAX_BYTES",
    "VPSMAN_GATEWAY_SPOOL_SHUTDOWN_FLUSH_SECS",
    "VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS",
    "VPSMAN_GATEWAY_TELEMETRY_IN_FLIGHT",
];

static GATEWAY_SUITE_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_cleared_gateway_env<R>(names: &[&str], run: impl FnOnce() -> R) -> R {
    let _guard = GATEWAY_SUITE_ENV_LOCK.lock().unwrap();
    let saved = names
        .iter()
        .map(|name| (*name, std::env::var_os(name)))
        .collect::<Vec<_>>();
    for name in names {
        std::env::remove_var(name);
    }
    let result = run();
    for (name, value) in saved {
        if let Some(value) = value {
            std::env::set_var(name, value);
        } else {
            std::env::remove_var(name);
        }
    }
    result
}

fn temp_suite_config_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("vpsman-{label}-{}.toml", uuid::Uuid::new_v4()))
}

fn gateway_runtime_toml(
    reconnect_grace_secs: u64,
    dispatch_ack_secs: u64,
    connect_secs: u64,
    write_secs: u64,
    read_secs: u64,
    event_post_secs: u64,
    command_output_event_ttl_secs: u64,
) -> String {
    format!(
        r#"version = 1

[gateway]
reconnect_grace_secs = {reconnect_grace_secs}
command_output_event_ttl_secs = {command_output_event_ttl_secs}

[timeout]
dispatch_ack_secs = {dispatch_ack_secs}
internal_http_connect_secs = {connect_secs}
internal_http_write_secs = {write_secs}
internal_http_read_secs = {read_secs}
event_post_secs = {event_post_secs}
"#
    )
}
