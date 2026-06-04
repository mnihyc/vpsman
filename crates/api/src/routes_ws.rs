use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    response::{IntoResponse, Response},
};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use tokio::sync::broadcast;

use crate::{
    error::ApiError,
    model::{WsAuthQuery, WsEvent},
    state::AppState,
};

pub(crate) async fn ws_handler(
    State(state): State<AppState>,
    Query(query): Query<WsAuthQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if state.repo.auth_required() {
        let Some(token) = query.access_token.as_deref() else {
            return ApiError::unauthorized("missing_websocket_token").into_response();
        };
        match state.repo.authenticate_access_token(token).await {
            Ok(Some(_operator)) => {}
            Ok(None) => return ApiError::unauthorized("invalid_websocket_token").into_response(),
            Err(error) => return ApiError::from(error).into_response(),
        }
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut events = state.events.subscribe();

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

    loop {
        tokio::select! {
            message = receiver.next() => {
                match message {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
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
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

async fn send_ws_event(sender: &mut SplitSink<WebSocket, Message>, event: &WsEvent) -> bool {
    let Ok(payload) = serde_json::to_string(event) else {
        return false;
    };
    sender.send(Message::Text(payload.into())).await.is_ok()
}
