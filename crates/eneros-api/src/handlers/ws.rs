use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;

use crate::app::{AppState, WsClient};

/// GET /ws — WebSocket upgrade handler
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

/// Handle an individual WebSocket connection
async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let client_id = uuid::Uuid::new_v4().to_string();

    // Create channel for sending messages to this client
    let (tx, mut rx) = mpsc::channel::<String>(100);

    // Register client
    {
        let client = WsClient {
            id: client_id.clone(),
            sender: tx,
        };
        state.ws_clients.write().push(client);
    }

    tracing::info!("WebSocket client connected: {}", client_id);

    // Spawn task to forward messages from channel to WebSocket
    let send_id = client_id.clone();
    let send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if ws_sender
                .send(axum::extract::ws::Message::Text(msg))
                .await
                .is_err()
            {
                break;
            }
        }
        tracing::info!("WebSocket send task ended for client {}", send_id);
    });

    // Read messages from WebSocket (handle ping/pong, detect disconnect)
    let recv_id = client_id.clone();
    let recv_state = state.clone();
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(text)) => {
                // Echo text messages back or handle commands
                let _ = text;
            }
            Ok(axum::extract::ws::Message::Ping(data)) => {
                // axum auto-replies with Pong
                let _ = data;
            }
            Ok(axum::extract::ws::Message::Close(_)) => {
                break;
            }
            Err(_) => {
                break;
            }
            _ => {}
        }
    }

    // Remove client on disconnect
    {
        let mut clients = recv_state.ws_clients.write();
        clients.retain(|c| c.id != recv_id);
    }
    tracing::info!("WebSocket client disconnected: {}", client_id);

    // Abort the send task
    send_task.abort();
}
