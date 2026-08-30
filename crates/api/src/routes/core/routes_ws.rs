use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::{IntoResponse, Response},
};
use futures_util::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use serde::Deserialize;
use tokio::{
    sync::broadcast,
    task::JoinHandle,
    time::{self, Duration},
};

use crate::{
    auth_model::AuthContext,
    model::WsEvent,
    security::{operator_has_scope, SCOPE_FLEET_READ},
    state::{AppState, WsEventBus, WsInvalidationDriver, FLEET_TELEMETRY_INVALIDATION_WINDOW},
};

const WS_AUTH_REVALIDATE_SECS: u64 = 30;
const JOB_DETAILS_INVALIDATION_WINDOW: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
struct WsClientAuth {
    r#type: String,
    access_token: String,
}

pub(crate) async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

pub(crate) fn spawn_ws_invalidation_coalescer(
    events: WsEventBus,
    invalidations: WsInvalidationDriver,
) -> JoinHandle<()> {
    tokio::spawn(run_ws_invalidation_coalescer(events, invalidations))
}

async fn run_ws_invalidation_coalescer(events: WsEventBus, invalidations: WsInvalidationDriver) {
    let now = time::Instant::now();
    let mut telemetry_boundaries = time::interval_at(
        now + FLEET_TELEMETRY_INVALIDATION_WINDOW,
        FLEET_TELEMETRY_INVALIDATION_WINDOW,
    );
    telemetry_boundaries.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut job_boundaries = time::interval_at(
        now + JOB_DETAILS_INVALIDATION_WINDOW,
        JOB_DETAILS_INVALIDATION_WINDOW,
    );
    job_boundaries.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = telemetry_boundaries.tick() => {
                if invalidations.take_fleet_telemetry() {
                    // Invalidate immediately before the notification so the
                    // refetch it causes cannot reuse a pre-boundary result.
                    events.invalidate_fleet_telemetry_read_cache();
                    events.publish(WsEvent::FleetTelemetryInvalidated);
                }
            }
            _ = job_boundaries.tick() => {
                let job_ids = invalidations.take_job_ids();
                if !job_ids.is_empty() {
                    events.publish(WsEvent::JobDetailsInvalidated { job_ids });
                }
            }
        }
    }
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let Some(session) = authenticate_socket(&mut receiver, &state).await else {
        let _ = sender.send(Message::Close(None)).await;
        return;
    };
    let mut events = state.events.subscribe();
    let mut auth_revalidate = time::interval(Duration::from_secs(WS_AUTH_REVALIDATE_SECS));
    auth_revalidate.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

    let hello = WsEvent::Hello {
        service: "vpsman-api".to_string(),
        stream: "fleet".to_string(),
    };
    if !send_ws_event(&mut sender, &hello).await {
        return;
    }
    if let Ok(snapshot) = state.fleet_snapshot().await {
        if !send_ws_event(&mut sender, &snapshot).await {
            return;
        }
    }
    auth_revalidate.tick().await;

    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
            _ = auth_revalidate.tick() => {
                let Some(heartbeat) =
                    authenticated_heartbeat_message(&state, &session.access_token).await
                else {
                    let _ = sender.send(Message::Close(None)).await;
                    break;
                };
                if sender.send(heartbeat).await.is_err() {
                    break;
                }
            }
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if !send_ws_event(&mut sender, &event).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Ok(snapshot) = state.fleet_snapshot().await {
                            if !send_ws_event(&mut sender, &snapshot).await {
                                break;
                            }
                        }
                        if !send_ws_event(&mut sender, &WsEvent::FleetTelemetryInvalidated).await {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
struct WsAuthenticatedSession {
    access_token: String,
}

impl WsAuthenticatedSession {
    async fn revalidate(&self, state: &AppState) -> bool {
        authenticate_socket_token(state, &self.access_token).await
    }
}

async fn authenticate_socket(
    receiver: &mut SplitStream<WebSocket>,
    state: &AppState,
) -> Option<WsAuthenticatedSession> {
    let Ok(Some(Ok(Message::Text(payload)))) =
        time::timeout(Duration::from_secs(10), receiver.next()).await
    else {
        return None;
    };
    let Ok(auth) = serde_json::from_str::<WsClientAuth>(&payload) else {
        return None;
    };
    if auth.r#type != "auth" || auth.access_token.trim().is_empty() {
        return None;
    }
    let session = WsAuthenticatedSession {
        access_token: auth.access_token,
    };
    session.revalidate(state).await.then_some(session)
}

pub(crate) async fn authenticate_socket_token(state: &AppState, access_token: &str) -> bool {
    authenticate_socket_context(state, access_token)
        .await
        .is_some()
}

pub(crate) async fn authenticated_heartbeat_message(
    state: &AppState,
    access_token: &str,
) -> Option<Message> {
    authenticate_socket_token(state, access_token)
        .await
        .then(|| Message::Ping(Default::default()))
}

pub(crate) async fn authenticate_socket_context(
    state: &AppState,
    access_token: &str,
) -> Option<AuthContext> {
    match state.repo.authenticate_access_token(access_token).await {
        Ok(Some(context)) if operator_has_scope(&context.operator.scopes, SCOPE_FLEET_READ) => {
            Some(context)
        }
        _ => None,
    }
}

async fn send_ws_event(sender: &mut SplitSink<WebSocket, Message>, event: &WsEvent) -> bool {
    let Ok(payload) = serde_json::to_string(event) else {
        return false;
    };
    sender.send(Message::Text(payload.into())).await.is_ok()
}

#[cfg(test)]
#[path = "tests_routes_ws.rs"]
mod tests;
