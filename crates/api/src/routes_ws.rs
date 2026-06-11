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
    time::{self, Duration},
};

use crate::{model::WsEvent, state::AppState};

#[derive(Debug, Deserialize)]
struct WsClientAuth {
    r#type: String,
    access_token: String,
}

pub(crate) async fn ws_handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    if !authenticate_socket(&mut receiver, &state).await {
        let _ = sender.send(Message::Close(None)).await;
        return;
    }
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

async fn authenticate_socket(receiver: &mut SplitStream<WebSocket>, state: &AppState) -> bool {
    let Ok(Some(Ok(Message::Text(payload)))) =
        time::timeout(Duration::from_secs(10), receiver.next()).await
    else {
        return false;
    };
    let Ok(auth) = serde_json::from_str::<WsClientAuth>(&payload) else {
        return false;
    };
    if auth.r#type != "auth" || auth.access_token.trim().is_empty() {
        return false;
    }
    matches!(
        state
            .repo
            .authenticate_access_token(&auth.access_token)
            .await,
        Ok(Some(_))
    )
}

async fn send_ws_event(sender: &mut SplitSink<WebSocket, Message>, event: &WsEvent) -> bool {
    let Ok(payload) = serde_json::to_string(event) else {
        return false;
    };
    sender.send(Message::Text(payload.into())).await.is_ok()
}
