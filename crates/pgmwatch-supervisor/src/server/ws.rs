use crate::supervisor::{SupervisorState, WsBroadcast};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum WsClientCmd {
    Ack { channel_id: String },
    ForceClip { channel_id: String },
    TogglePfl { channel_id: String },
    ResetLufs { channel_id: Option<String> },
    SetIntermission { group_id: Option<String>, active: bool },
    StartMatch { label: String },
    EndMatch,
    RequestSnapshot,
}

pub(super) async fn ws_handler(
    ws: WebSocketUpgrade,
    State(st): State<Arc<SupervisorState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_ws_client(socket, st))
}

async fn handle_ws_client(mut socket: WebSocket, st: Arc<SupervisorState>) {
    let initial = st.snapshot().await;
    if let Ok(txt) = serde_json::to_string(&WsBroadcast::Snapshot {
        snapshot: Box::new(initial),
    }) {
        let _ = socket.send(Message::Text(txt.into())).await;
    }

    let mut rx = st.ws_tx.subscribe();
    let mut shutdown_rx = st.subscribe_shutdown();
    loop {
        if *shutdown_rx.borrow() {
            let _ = socket.send(Message::Close(None)).await;
            break;
        }
        tokio::select! {
            _ = shutdown_rx.changed() => {
                let _ = socket.send(Message::Close(None)).await;
                break;
            }
            bcast = rx.recv() => {
                match bcast {
                    Ok(ev) => {
                        if let Ok(txt) = serde_json::to_string(&ev)
                            && socket.send(Message::Text(txt.into())).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::debug!(skipped, "WebSocket client lagged; resyncing snapshot");
                        let snap = st.snapshot_for_periodic_ws().await;
                        if let Ok(txt) = serde_json::to_string(&WsBroadcast::Snapshot {
                            snapshot: Box::new(snap),
                        }) && socket.send(Message::Text(txt.into())).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        if let Ok(cmd) = serde_json::from_str::<WsClientCmd>(&txt) {
                            match cmd {
                                WsClientCmd::Ack { channel_id } => st.ack_channel(&channel_id).await,
                                WsClientCmd::ForceClip { channel_id } => st.force_clip(&channel_id).await,
                                WsClientCmd::TogglePfl { channel_id } => { st.toggle_pfl(&channel_id).await; }
                                WsClientCmd::ResetLufs { channel_id } => {
                                    st.reset_lufs(channel_id.as_deref()).await;
                                }
                                WsClientCmd::SetIntermission { group_id, active } => st.set_intermission(group_id.as_deref(), active).await,
                                WsClientCmd::StartMatch { label } => {
                                    let trimmed = label.trim();
                                    if !trimmed.is_empty() && trimmed.len() <= 120 {
                                        st.start_match(trimmed.to_string()).await;
                                    }
                                }
                                WsClientCmd::EndMatch => { st.end_active_match().await; }
                                WsClientCmd::RequestSnapshot => {
                                    let snap = st.snapshot().await;
                                    if let Ok(s) = serde_json::to_string(&WsBroadcast::Snapshot {
                                        snapshot: Box::new(snap),
                                    }) {
                                        let _ = socket.send(Message::Text(s.into())).await;
                                    }
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
        }
    }
}
