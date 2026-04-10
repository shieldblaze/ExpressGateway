//! WebSocket bidirectional proxy.
//!
//! Forwards frames between client and backend using `tokio-tungstenite`.

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};

use crate::frames::{FrameConfig, PingPongTracker};

/// Errors from the WebSocket proxy.
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("client connection error: {0}")]
    Client(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("backend connection error: {0}")]
    Backend(String),

    #[error("frame too large: {size} > {max}")]
    FrameTooLarge { size: usize, max: usize },

    #[error("idle timeout")]
    IdleTimeout,
}

/// Forward frames from `reader` to `writer`, applying frame config checks.
///
/// Returns when the stream closes or an error occurs.
pub async fn forward_frames(
    mut reader: SplitStream<WebSocketStream<TcpStream>>,
    mut writer: SplitSink<WebSocketStream<TcpStream>, Message>,
    config: &FrameConfig,
    mut ping_tracker: Option<&mut PingPongTracker>,
) -> Result<(), ProxyError> {
    while let Some(msg_result) = reader.next().await {
        let msg = msg_result.map_err(|e| ProxyError::Backend(e.to_string()))?;

        // Check frame size for data frames.
        let payload_len = match &msg {
            Message::Text(t) => t.len(),
            Message::Binary(b) => b.len(),
            _ => 0,
        };

        if config.is_oversize(payload_len) {
            debug!(
                payload_len,
                max = config.max_frame_size,
                "frame too large, sending close 1009"
            );
            let close_msg =
                Message::Close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Size,
                    reason: "frame too large".into(),
                }));
            let _ = writer.send(close_msg).await;
            return Err(ProxyError::FrameTooLarge {
                size: payload_len,
                max: config.max_frame_size,
            });
        }

        match &msg {
            Message::Ping(data) => {
                // Handle Ping locally: send Pong, don't forward.
                if let Some(ref mut tracker) = ping_tracker {
                    let pong_payload = tracker.handle_ping(bytes::Bytes::copy_from_slice(data));
                    let pong = Message::Pong(pong_payload.to_vec());
                    writer
                        .send(pong)
                        .await
                        .map_err(|e| ProxyError::Backend(e.to_string()))?;
                    continue;
                }
            }
            Message::Close(_) => {
                debug!("received close frame, forwarding");
                writer
                    .send(msg)
                    .await
                    .map_err(|e| ProxyError::Backend(e.to_string()))?;
                return Ok(());
            }
            _ => {}
        }

        writer
            .send(msg)
            .await
            .map_err(|e| ProxyError::Backend(e.to_string()))?;
    }

    Ok(())
}

/// Run a bidirectional WebSocket proxy between client and backend streams.
///
/// Both directions run concurrently; the proxy terminates when either side
/// closes.
pub async fn run_proxy(
    client: WebSocketStream<TcpStream>,
    backend: WebSocketStream<TcpStream>,
    config: FrameConfig,
) -> Result<(), ProxyError> {
    let (client_write, client_read) = client.split();
    let (backend_write, backend_read) = backend.split();

    let c2b = forward_frames(client_read, backend_write, &config, None);
    let b2c = forward_frames(backend_read, client_write, &config, None);

    tokio::select! {
        result = c2b => {
            if let Err(e) = &result {
                warn!("client->backend error: {e}");
            }
            result
        }
        result = b2c => {
            if let Err(e) = &result {
                warn!("backend->client error: {e}");
            }
            result
        }
    }
}
