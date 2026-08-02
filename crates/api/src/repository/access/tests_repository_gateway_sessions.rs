use super::*;
use crate::{model::AgentView, repository::Repository};

fn session_event(client_id: &str, session_id: uuid::Uuid) -> GatewaySessionLifecycleIngest {
    session_event_from(client_id, session_id, "203.0.113.10")
}

fn session_event_from(
    client_id: &str,
    session_id: uuid::Uuid,
    remote_ip: &str,
) -> GatewaySessionLifecycleIngest {
    GatewaySessionLifecycleIngest {
        gateway_id: "gateway-a".to_string(),
        client_id: client_id.to_string(),
        session_id,
        noise_public_key_hex: Some("ab".repeat(32)),
        remote_ip: Some(remote_ip.to_string()),
        agent_version: Some("test".to_string()),
        reason: None,
    }
}

#[tokio::test]
async fn memory_gateway_sessions_do_not_disconnect_newer_active_session() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    memory.agents.write().await.push(AgentView {
        id: "client-a".to_string(),
        display_name: "client-a".to_string(),
        status: "offline".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    });
    let older = uuid::Uuid::new_v4();
    let newer = uuid::Uuid::new_v4();

    repo.record_gateway_session_started(&session_event("client-a", older))
        .await
        .unwrap();
    repo.record_gateway_session_started(&session_event("client-a", newer))
        .await
        .unwrap();
    let sessions = repo.list_gateway_sessions(10).await.unwrap();
    assert_eq!(sessions.len(), 2);
    assert_eq!(
        sessions
            .iter()
            .find(|session| session.id == older)
            .unwrap()
            .status,
        "expired"
    );
    assert_eq!(
        sessions
            .iter()
            .find(|session| session.id == newer)
            .unwrap()
            .status,
        "active"
    );
    let mut ended = session_event("client-a", older);
    ended.reason = Some("replaced".to_string());
    repo.record_gateway_session_ended(&ended).await.unwrap();

    assert_eq!(memory.agents.read().await[0].status.as_str(), "online");
    assert_eq!(
        memory.agents.read().await[0].registration_ip.as_deref(),
        Some("203.0.113.10")
    );
    assert_eq!(
        memory.agents.read().await[0].last_ip.as_deref(),
        Some("203.0.113.10")
    );
    let listed_sessions = repo.list_gateway_sessions(10).await.unwrap();
    let active_session = listed_sessions
        .iter()
        .find(|session| session.id == newer)
        .unwrap();
    assert_eq!(active_session.remote_ip.as_deref(), Some("203.0.113.10"));
    assert_eq!(active_session.agent_version, "test");
    assert_eq!(listed_sessions.len(), 2);

    repo.record_gateway_session_ended(&session_event("client-a", newer))
        .await
        .unwrap();
    assert_eq!(
        memory.agents.read().await[0].status.as_str(),
        "disconnected"
    );
}

#[tokio::test]
async fn active_gateway_session_match_requires_current_session_and_incarnation() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    let process_incarnation_id = uuid::Uuid::new_v4();
    memory.agents.write().await.push(AgentView {
        id: "client-a".to_string(),
        display_name: "client-a".to_string(),
        status: "online".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: Some(process_incarnation_id),
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    });
    let older = uuid::Uuid::new_v4();
    let newer = uuid::Uuid::new_v4();
    repo.record_gateway_session_started(&session_event("client-a", older))
        .await
        .unwrap();
    repo.record_gateway_session_started(&session_event("client-a", newer))
        .await
        .unwrap();

    assert!(repo
        .active_gateway_session_matches("gateway-a", "client-a", newer, process_incarnation_id)
        .await
        .unwrap());
    assert!(!repo
        .active_gateway_session_matches("gateway-a", "client-a", older, process_incarnation_id,)
        .await
        .unwrap());
    assert!(!repo
        .active_gateway_session_matches("gateway-a", "client-a", newer, uuid::Uuid::new_v4(),)
        .await
        .unwrap());
}

#[tokio::test]
async fn delayed_session_end_does_not_rewind_observed_connection_ip() {
    let repo = Repository::Memory(MemoryState::default());
    let Repository::Memory(memory) = &repo else {
        unreachable!();
    };
    memory.agents.write().await.push(AgentView {
        id: "client-a".to_string(),
        display_name: "client-a".to_string(),
        status: "offline".to_string(),
        tags: Vec::new(),
        registration_ip: None,
        last_ip: None,
        last_seen_at: None,
        arch: None,
        internal_build_number: 1,
        process_incarnation_id: None,
        stale_since: None,
        stale_reason: None,
        capabilities: Default::default(),
    });
    let older = uuid::Uuid::new_v4();
    let newer = uuid::Uuid::new_v4();
    repo.record_gateway_session_started(&session_event_from("client-a", older, "198.51.100.10"))
        .await
        .unwrap();
    repo.record_gateway_session_started(&session_event_from("client-a", newer, "2001:db8::20"))
        .await
        .unwrap();

    repo.record_gateway_session_ended(&session_event_from("client-a", newer, "2001:db8::20"))
        .await
        .unwrap();
    repo.record_gateway_session_ended(&session_event_from("client-a", older, "198.51.100.10"))
        .await
        .unwrap();

    let agents = memory.agents.read().await;
    assert_eq!(agents[0].registration_ip.as_deref(), Some("198.51.100.10"));
    assert_eq!(agents[0].last_ip.as_deref(), Some("2001:db8::20"));
}
