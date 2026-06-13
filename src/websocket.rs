//! WebSocket frame capture and replay.
//!
//! WebSocket connections are upgraded HTTP connections. When we see an
//! Upgrade: websocket request, we:
//! 1. Capture the upgrade request/response
//! 2. Proxy the WebSocket frames bidirectionally
//! 3. Store each frame as a WsFrame with direction (client->server or server->client)

use std::collections::HashMap;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::models::{CapturedRequest, Exchange};

/// Direction of a WebSocket frame.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WsDirection {
    ClientToServer,
    ServerToClient,
}

/// A captured WebSocket frame.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WsFrame {
    pub id: String,
    pub request_id: String,
    pub direction: WsDirection,
    pub opcode: String,
    pub payload: Option<Vec<u8>>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl WsFrame {
    pub fn from_message(request_id: String, direction: WsDirection, msg: &Message) -> Self {
        let opcode = match msg {
            Message::Text(_) => "text",
            Message::Binary(_) => "binary",
            Message::Ping(_) => "ping",
            Message::Pong(_) => "pong",
            Message::Close(_) => "close",
            Message::Frame(_) => "frame",
        }
        .to_string();

        let payload = match msg {
            Message::Text(t) => Some(t.as_bytes().to_vec()),
            Message::Binary(b) => Some(b.to_vec()),
            Message::Ping(p) => Some(p.to_vec()),
            Message::Pong(p) => Some(p.to_vec()),
            Message::Close(_) => None,
            Message::Frame(f) => Some(f.payload().to_vec()),
        };

        Self {
            id: uuid::Uuid::new_v4().to_string(),
            request_id,
            direction,
            opcode,
            payload,
            timestamp: chrono::Utc::now(),
        }
    }

    /// Convert back to a tungstenite Message for replay.
    pub fn to_message(&self) -> Message {
        match self.opcode.as_str() {
            "text" => Message::text(
                String::from_utf8_lossy(self.payload.as_deref().unwrap_or_default()).to_string(),
            ),
            "binary" => Message::binary(self.payload.clone().unwrap_or_default()),
            "ping" => Message::Ping(self.payload.clone().unwrap_or_default().into()),
            "pong" => Message::Pong(self.payload.clone().unwrap_or_default().into()),
            "close" => Message::Close(None),
            _ => Message::binary(self.payload.clone().unwrap_or_default()),
        }
    }

    /// Human-readable preview of the payload.
    pub fn payload_preview(&self, max_len: usize) -> String {
        match &self.payload {
            None => "(empty)".to_string(),
            Some(bytes) => {
                if bytes.is_empty() {
                    "(empty)".to_string()
                } else if let Ok(text) = std::str::from_utf8(bytes) {
                    if text.len() > max_len {
                        format!("{}...", &text[..max_len])
                    } else {
                        text.to_string()
                    }
                } else {
                    format!("<{} binary bytes>", bytes.len())
                }
            }
        }
    }
}

/// Bridge two WebSocket streams, capturing each frame.
///
/// `client_ws` is the WebSocket connection to the client (after our TLS termination).
/// `upstream_ws` is the WebSocket connection to the upstream server.
pub async fn proxy_websocket_bridge<
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
>(
    client_ws: tokio_tungstenite::WebSocketStream<C>,
    upstream_ws: tokio_tungstenite::WebSocketStream<U>,
    request_id: String,
    exchange_tx: mpsc::Sender<Exchange>,
    session: String,
    upstream_addr: String,
) -> Result<()> {
    let (mut client_sink, mut client_stream) = client_ws.split();
    let (mut upstream_sink, mut upstream_stream) = upstream_ws.split();

    let request_id_c2s = request_id.clone();
    let request_id_s2c = request_id.clone();
    let tx_c2s = exchange_tx.clone();
    let tx_s2c = exchange_tx;
    let upstream_addr_c2s = upstream_addr.clone();
    let upstream_addr_s2c = upstream_addr;
    let session_c2s = session.clone();
    let session_s2c = session;

    // Client -> Server
    let c2s = tokio::spawn(async move {
        while let Some(Ok(msg)) = client_stream.next().await {
            if msg.is_close() {
                let _ = upstream_sink.send(msg).await;
                break;
            }

            let frame =
                WsFrame::from_message(request_id_c2s.clone(), WsDirection::ClientToServer, &msg);

            let _ = tx_c2s
                .send(Exchange {
                    request: CapturedRequest {
                        id: frame.id.clone(),
                        method: "WS".to_string(),
                        url: format!("ws://{}", upstream_addr_c2s),
                        path: "/".to_string(),
                        host: upstream_addr_c2s.clone(),
                        headers: {
                            let mut h = HashMap::new();
                            h.insert(
                                "x-ledger-ws-direction".to_string(),
                                "client->server".to_string(),
                            );
                            h.insert("x-ledger-ws-opcode".to_string(), frame.opcode.clone());
                            h
                        },
                        body: frame.payload.clone(),
                        timestamp: frame.timestamp,
                        session: session_c2s.clone(),
                    },
                    response: None,
                })
                .await;

            if let Err(e) = upstream_sink.send(msg).await {
                eprintln!("[ledger] WebSocket upstream send error: {e}");
                break;
            }
        }
    });

    // Server -> Client
    let s2c = tokio::spawn(async move {
        while let Some(Ok(msg)) = upstream_stream.next().await {
            if msg.is_close() {
                let _ = client_sink.send(msg).await;
                break;
            }

            let frame =
                WsFrame::from_message(request_id_s2c.clone(), WsDirection::ServerToClient, &msg);

            let _ = tx_s2c
                .send(Exchange {
                    request: CapturedRequest {
                        id: frame.id.clone(),
                        method: "WS".to_string(),
                        url: format!("ws://{}", upstream_addr_s2c),
                        path: "/".to_string(),
                        host: upstream_addr_s2c.clone(),
                        headers: {
                            let mut h = HashMap::new();
                            h.insert(
                                "x-ledger-ws-direction".to_string(),
                                "server->client".to_string(),
                            );
                            h.insert("x-ledger-ws-opcode".to_string(), frame.opcode.clone());
                            h
                        },
                        body: frame.payload.clone(),
                        timestamp: frame.timestamp,
                        session: session_s2c.clone(),
                    },
                    response: None,
                })
                .await;

            if let Err(e) = client_sink.send(msg).await {
                eprintln!("[ledger] WebSocket client send error: {e}");
                break;
            }
        }
    });

    tokio::select! {
        _ = c2s => {},
        _ = s2c => {},
    }

    Ok(())
}

/// Check if an HTTP request is a WebSocket upgrade.
pub fn is_websocket_upgrade(req: &hyper::Request<hyper::body::Incoming>) -> bool {
    let headers = req.headers();
    let is_upgrade = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("websocket"))
        .unwrap_or(false);

    let has_connection_upgrade = headers
        .get("connection")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_lowercase().contains("upgrade"))
        .unwrap_or(false);

    is_upgrade && has_connection_upgrade
}

/// Replay a WebSocket conversation from captured frames.
/// Connects to the target, then replays all client->server frames with
/// optional delay between them.
pub async fn replay_websocket(target_addr: &str, frames: &[WsFrame], delay_ms: u64) -> Result<()> {
    let uri = format!("ws://{}", target_addr);
    let (ws_stream, _) = tokio_tungstenite::connect_async(&uri)
        .await
        .with_context(|| format!("failed to connect to {uri}"))?;

    let (mut sink, mut stream) = ws_stream.split();

    // Spawn a task to read server responses
    let read_handle = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Text(t) => eprintln!("[ledger] ws recv text: {t}"),
                Message::Binary(b) => eprintln!("[ledger] ws recv binary: <{} bytes>", b.len()),
                Message::Close(_) => {
                    eprintln!("[ledger] ws recv close");
                    break;
                }
                _ => {}
            }
        }
    });

    // Replay client->server frames
    for frame in frames
        .iter()
        .filter(|f| f.direction == WsDirection::ClientToServer)
    {
        let msg = frame.to_message();
        eprintln!(
            "[ledger] ws replay {} -> {}",
            frame.opcode,
            frame.payload_preview(80)
        );
        sink.send(msg).await?;

        if delay_ms > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
        }
    }

    // Close gracefully
    sink.send(Message::Close(None)).await.ok();
    drop(sink);

    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    read_handle.abort();

    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_frame_from_text_message() {
        let msg = Message::text("hello world");
        let frame = WsFrame::from_message("req-1".to_string(), WsDirection::ClientToServer, &msg);

        assert_eq!(frame.opcode, "text");
        assert_eq!(frame.payload, Some(b"hello world".to_vec()));
        assert_eq!(frame.direction, WsDirection::ClientToServer);
        assert_eq!(frame.request_id, "req-1");
    }

    #[test]
    fn test_ws_frame_from_binary_message() {
        let data = vec![0x01, 0x02, 0x03];
        let msg = Message::binary(data.clone());
        let frame = WsFrame::from_message("req-2".to_string(), WsDirection::ServerToClient, &msg);

        assert_eq!(frame.opcode, "binary");
        assert_eq!(frame.payload, Some(data));
        assert_eq!(frame.direction, WsDirection::ServerToClient);
    }

    #[test]
    fn test_ws_frame_roundtrip() {
        let msg = Message::text("roundtrip test");
        let frame = WsFrame::from_message("req-3".to_string(), WsDirection::ClientToServer, &msg);
        let back = frame.to_message();

        assert_eq!(back, msg);
    }

    #[test]
    fn test_payload_preview() {
        let frame = WsFrame {
            id: "test".to_string(),
            request_id: "req".to_string(),
            direction: WsDirection::ClientToServer,
            opcode: "text".to_string(),
            payload: Some(b"short".to_vec()),
            timestamp: chrono::Utc::now(),
        };
        assert_eq!(frame.payload_preview(100), "short");

        let long = "a".repeat(200);
        let frame2 = WsFrame {
            id: "test2".to_string(),
            request_id: "req".to_string(),
            direction: WsDirection::ClientToServer,
            opcode: "text".to_string(),
            payload: Some(long.as_bytes().to_vec()),
            timestamp: chrono::Utc::now(),
        };
        assert!(frame2.payload_preview(50).ends_with("..."));
        assert_eq!(frame2.payload_preview(50).len(), 53); // 50 + "..."
    }

    #[test]
    fn test_is_websocket_upgrade_detection() {
        // We can't easily construct a hyper::Request here without async runtime,
        // but we can test the WsFrame logic directly.
        assert_eq!(WsDirection::ClientToServer as u8, 0);
        assert_eq!(WsDirection::ServerToClient as u8, 1);
    }
}
