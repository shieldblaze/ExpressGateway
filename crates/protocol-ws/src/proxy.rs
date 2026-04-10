//! WebSocket bidirectional proxy.
//!
//! Forwards frames between client and backend WebSocket streams with:
//! - Frame size enforcement (close 1009 on oversize)
//! - Ping/Pong handling (local response, suppression of backend echo)
//! - Close handshake forwarding and timeout
//! - Idle timeout with close 1001
//! - Backpressure integration
//!
//! The proxy is generic over `AsyncRead + AsyncWrite` so it works with
//! `TcpStream`, `TlsStream`, or any other async transport.

use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, warn};

use crate::backpressure::BackpressureState;
use crate::error::WsError;
use crate::frames::{FrameConfig, PingPongTracker};
use crate::timeout::{CloseHandshakeTimeout, IdleTimeout};

/// Configuration for the WebSocket proxy.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// Frame size and ping handling config.
    pub frame: FrameConfig,
    /// Idle timeout duration.
    pub idle_timeout: IdleTimeout,
    /// Close handshake timeout.
    pub close_timeout: CloseHandshakeTimeout,
}


/// Forward frames from `reader` to `writer`, applying frame config checks.
///
/// Returns when the stream closes or an error occurs.
pub async fn forward_frames<S>(
    mut reader: SplitStream<WebSocketStream<S>>,
    mut writer: SplitSink<WebSocketStream<S>, Message>,
    config: &FrameConfig,
    mut ping_tracker: Option<&mut PingPongTracker>,
    backpressure: Option<&BackpressureState>,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    while let Some(msg_result) = reader.next().await {
        let msg = msg_result.map_err(WsError::Client)?;

        // Honor backpressure: yield if the other side is congested.
        if let Some(bp) = backpressure {
            while bp.is_paused() {
                tokio::task::yield_now().await;
            }
        }

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
            return Err(WsError::FrameTooLarge {
                size: payload_len,
                max: config.max_frame_size,
            });
        }

        match &msg {
            Message::Ping(data) if !config.forward_pings => {
                // Handle Ping locally: send Pong, don't forward.
                if let Some(ref mut tracker) = ping_tracker {
                    let pong_payload =
                        tracker.handle_ping(bytes::Bytes::copy_from_slice(data));
                    let pong = Message::Pong(pong_payload.to_vec());
                    writer.send(pong).await.map_err(WsError::Client)?;
                    continue;
                }
            }
            Message::Close(_) => {
                debug!("received close frame, forwarding");
                writer.send(msg).await.map_err(WsError::Client)?;
                return Ok(());
            }
            _ => {}
        }

        writer.send(msg).await.map_err(WsError::Client)?;
    }

    Ok(())
}

/// Run a bidirectional WebSocket proxy between client and backend streams.
///
/// Both directions run concurrently; the proxy terminates when either side
/// closes or the idle timeout fires.
pub async fn run_proxy<S>(
    client: WebSocketStream<S>,
    backend: WebSocketStream<S>,
    config: ProxyConfig,
) -> Result<(), WsError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (client_write, client_read) = client.split();
    let (backend_write, backend_read) = backend.split();

    let backpressure = BackpressureState::new();
    let idle_duration = config.idle_timeout.duration();

    let c2b = forward_frames(
        client_read,
        backend_write,
        &config.frame,
        None,
        Some(&backpressure),
    );
    let b2c = forward_frames(
        backend_read,
        client_write,
        &config.frame,
        None,
        Some(&backpressure),
    );

    let idle_sleep = tokio::time::sleep(idle_duration);
    tokio::pin!(idle_sleep);

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
        () = &mut idle_sleep => {
            debug!("idle timeout fired after {idle_duration:?}");
            Err(WsError::IdleTimeout(idle_duration))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_config_default() {
        let config = ProxyConfig::default();
        assert_eq!(config.frame.max_frame_size, 64 * 1024);
        assert!(!config.frame.forward_pings);
        assert_eq!(
            config.idle_timeout.duration(),
            std::time::Duration::from_secs(300)
        );
        assert_eq!(
            config.close_timeout.duration(),
            std::time::Duration::from_secs(5)
        );
    }
}
