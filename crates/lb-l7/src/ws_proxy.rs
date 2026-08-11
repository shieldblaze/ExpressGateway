//! WebSocket upstream path — a capability the H1/H2 proxies delegate to, not a
//! listener mode. `Ping` is forwarded, never answered here: tungstenite
//! auto-replies on the RECEIVING side. Per-message compression (RFC 7692) is
//! deliberately NOT negotiated.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::StreamExt;
use futures_util::{SinkExt, TryStreamExt};
use hyper::header::{HeaderName, HeaderValue};
use hyper::{Method, Request};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::Utf8Bytes;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};

/// Client-originated Ping budget per window; above it, `Close 1008`.
pub const DEFAULT_PING_RATE_LIMIT_PER_WINDOW: u32 = 50;

/// Default rolling window for the WebSocket client-Ping rate limit.
pub const DEFAULT_PING_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);

/// Per-direction read-frame watchdog (WS-002). Distinct from
/// [`WsConfig::idle_timeout`], which fires only when BOTH are silent.
pub const DEFAULT_READ_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-listener WebSocket knobs; every field has a canonical default.
#[derive(Debug, Clone, Copy)]
pub struct WsConfig {
    /// Idle budget (no frames in EITHER direction) → `1001 Going Away`.
    pub idle_timeout: Duration,
    /// Upper bound on one incoming message; fragments are summed.
    pub max_message_size: usize,
    /// When false the upgrade detector short-circuits to "not a WS request".
    pub enabled: bool,
    /// WS-001: client `Ping` budget per [`Self::ping_rate_limit_window`]
    /// before the proxy treats the stream as a flood amplifier.
    pub ping_rate_limit_per_window: u32,
    /// Rolling-window duration for the client-Ping rate limit.
    pub ping_rate_limit_window: Duration,
    /// Per-direction read-frame watchdog (WS-002). Distinct from
    /// [`Self::idle_timeout`]: idle needs BOTH halves silent, this one ANY.
    pub read_frame_timeout: Duration,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            idle_timeout: Duration::from_secs(60),
            max_message_size: 16 * 1024 * 1024,
            enabled: true,
            ping_rate_limit_per_window: DEFAULT_PING_RATE_LIMIT_PER_WINDOW,
            ping_rate_limit_window: DEFAULT_PING_RATE_LIMIT_WINDOW,
            read_frame_timeout: DEFAULT_READ_FRAME_TIMEOUT,
        }
    }
}

impl WsConfig {
    /// Render the tungstenite configuration for this block.
    ///
    /// F-S27-2 — `max_write_buffer_size` is BOUNDED, not tungstenite's
    /// `usize::MAX`. SCOPE (measured): defensive hardening, NOT the full fix —
    /// it does NOT bound the WS-over-H2 tunnel, where the upgraded stream
    /// buffers inside the `h2` crate's `SendStream` below this layer.
    ///
    /// tungstenite invariants the value MUST satisfy: a single legal max-size
    /// frame must fit (cap `>= max_frame_size`, else `WriteBufferFull`), and
    /// `assert_valid` PANICS unless `max_write_buffer_size > write_buffer_size`.
    #[must_use]
    pub fn tungstenite_config(self) -> WebSocketConfig {
        let defaults = WebSocketConfig::default();
        // `WebSocketConfig` is `#[non_exhaustive]` since tungstenite 0.29, so
        // the chaining setters replace the struct literal.
        defaults
            .max_message_size(Some(self.max_message_size))
            .max_frame_size(Some(self.max_message_size))
            // Saturating: a `max_message_size` near `usize::MAX` must not wrap.
            .max_write_buffer_size(
                self.max_message_size
                    .saturating_add(defaults.write_buffer_size),
            )
    }
}

/// Does `req` carry a valid RFC 6455 §4.1 handshake?
#[must_use]
pub fn is_h1_upgrade_request<B>(req: &Request<B>) -> bool {
    if req.method() != Method::GET {
        return false;
    }
    let hdrs = req.headers();
    if !header_contains_token(hdrs, &hyper::header::UPGRADE, "websocket") {
        return false;
    }
    if !header_contains_token(hdrs, &hyper::header::CONNECTION, "upgrade") {
        return false;
    }
    let version_ok = hdrs
        .get(&SEC_WEBSOCKET_VERSION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.trim() == "13");
    if !version_ok {
        return false;
    }
    hdrs.get(&SEC_WEBSOCKET_KEY)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| !s.trim().is_empty())
}

/// Does `req` carry an RFC 8441 extended CONNECT for WebSocket? hyper exposes
/// `:protocol` via the [`hyper::ext::Protocol`] extension.
#[must_use]
pub fn is_h2_extended_connect<B>(req: &Request<B>) -> bool {
    if req.method() != Method::CONNECT {
        return false;
    }
    req.extensions()
        .get::<hyper::ext::Protocol>()
        .is_some_and(|p| p.as_str().eq_ignore_ascii_case("websocket"))
}

/// WebSocket reverse proxy. Cheap to clone via [`Arc`].
pub struct WsProxy {
    cfg: WsConfig,
}

impl WsProxy {
    /// Construct with the supplied configuration.
    #[must_use]
    pub const fn new(cfg: WsConfig) -> Self {
        Self { cfg }
    }

    /// The [`WsConfig`] in effect.
    #[must_use]
    pub const fn config(&self) -> WsConfig {
        self.cfg
    }

    /// Frame-level proxy loop; both halves must already be post-handshake
    /// (server-role and client-role respectively).
    ///
    /// # Errors
    /// The first tungstenite error on either half. An idle-timeout is a clean
    /// close, NOT an error.
    pub async fn proxy_frames<C, B>(
        self: Arc<Self>,
        client_ws: WebSocketStream<C>,
        backend_ws: WebSocketStream<B>,
    ) -> Result<(), tokio_tungstenite::tungstenite::Error>
    where
        C: AsyncRead + AsyncWrite + Unpin,
        B: AsyncRead + AsyncWrite + Unpin,
    {
        let (mut client_tx, mut client_rx) = client_ws.split();
        let (mut backend_tx, mut backend_rx) = backend_ws.split();

        let idle = self.cfg.idle_timeout;
        let read_frame = self.cfg.read_frame_timeout;
        let ping_window = self.cfg.ping_rate_limit_window;
        let ping_max: usize = self.cfg.ping_rate_limit_per_window as usize;
        let mut client_ping_log: VecDeque<Instant> = VecDeque::new();

        loop {
            // `idle` is the both-sides-silent envelope; the inner per-direction
            // `read_frame` (WS-002) fires even while the other half produces.
            let step = tokio::time::timeout(idle, async {
                tokio::select! {
                    biased;
                    c = tokio::time::timeout(read_frame, client_rx.try_next()) => c
                        .map_or_else(
                            |_| Direction::ReadFrameTimeout,
                            Direction::ClientToBackend,
                        ),
                    b = tokio::time::timeout(read_frame, backend_rx.try_next()) => b
                        .map_or_else(
                            |_| Direction::ReadFrameTimeout,
                            Direction::BackendToClient,
                        ),
                }
            })
            .await;

            match step {
                Err(_) => {
                    let away = CloseFrame {
                        code: CloseCode::Away,
                        reason: Utf8Bytes::from_static("idle timeout"),
                    };
                    let _ = client_tx.send(Message::Close(Some(away.clone()))).await;
                    let _ = backend_tx.send(Message::Close(Some(away))).await;
                    return Ok(());
                }
                Ok(Direction::ReadFrameTimeout) => {
                    // WS-002: Close 1008 to the client, clean Close upstream.
                    let frame = CloseFrame {
                        code: CloseCode::Policy,
                        reason: Utf8Bytes::from_static("ws read frame timeout"),
                    };
                    let _ = client_tx.send(Message::Close(Some(frame))).await;
                    let _ = client_tx.close().await;
                    let _ = backend_tx.send(Message::Close(None)).await;
                    let _ = backend_tx.close().await;
                    return Ok(());
                }
                Ok(Direction::ClientToBackend(Ok(Some(msg)))) => {
                    // WS-001: rate-limit client Pings so the gateway cannot
                    // amplify a flood at the backend. Backend→client Pings are
                    // NOT gated — the backend is the would-be victim.
                    if matches!(msg, Message::Ping(_)) {
                        let now = Instant::now();
                        client_ping_log.push_back(now);
                        while let Some(&front) = client_ping_log.front() {
                            if now.saturating_duration_since(front) > ping_window {
                                client_ping_log.pop_front();
                            } else {
                                break;
                            }
                        }
                        if client_ping_log.len() > ping_max {
                            let frame = CloseFrame {
                                code: CloseCode::Policy,
                                reason: Utf8Bytes::from_static("ping flood: rate limit exceeded"),
                            };
                            let _ = client_tx.send(Message::Close(Some(frame))).await;
                            let _ = client_tx.close().await;
                            let _ = backend_tx.send(Message::Close(None)).await;
                            let _ = backend_tx.close().await;
                            return Ok(());
                        }
                    }
                    let is_close = matches!(msg, Message::Close(_));
                    // F-S27-2 — bound the FORWARDING send too: with
                    // `max_write_buffer_size` capped this `send().await` can
                    // PARK, and unbounded it would hang the relay forever
                    // (bounded memory, unreclaimed connection — a different
                    // DoS). Reuse the `read_frame` budget so a wedged WRITE is
                    // reclaimed exactly like a wedged READ.
                    match tokio::time::timeout(read_frame, backend_tx.send(msg)).await {
                        Ok(res) => res?,
                        Err(_) => return close_backpressure(&mut client_tx, &mut backend_tx).await,
                    }
                    if is_close {
                        let _ = client_tx.close().await;
                        return Ok(());
                    }
                }
                Ok(Direction::BackendToClient(Ok(Some(msg)))) => {
                    let is_close = matches!(msg, Message::Close(_));
                    // F-S27-2 — symmetric bound on the client-facing forward.
                    match tokio::time::timeout(read_frame, client_tx.send(msg)).await {
                        Ok(res) => res?,
                        Err(_) => return close_backpressure(&mut client_tx, &mut backend_tx).await,
                    }
                    if is_close {
                        let _ = backend_tx.close().await;
                        return Ok(());
                    }
                }
                Ok(Direction::ClientToBackend(Ok(None))) => {
                    // No Close frame — forward one so the backend can finish.
                    let _ = backend_tx.send(Message::Close(None)).await;
                    return Ok(());
                }
                Ok(Direction::BackendToClient(Ok(None))) => {
                    let _ = client_tx.send(Message::Close(None)).await;
                    return Ok(());
                }
                Ok(Direction::ClientToBackend(Err(e)) | Direction::BackendToClient(Err(e))) => {
                    return Err(e);
                }
            }
        }
    }
}

enum Direction<T> {
    ClientToBackend(T),
    BackendToClient(T),
    /// WS-002 per-direction read-frame watchdog elapsed.
    ReadFrameTimeout,
}

/// F-S27-2 — clean teardown when a FORWARDING `send().await` backpressures past
/// the per-direction budget, so a wedged WRITE is reclaimed like a wedged READ.
/// Best-effort: the peer that backpressured may reject this Close too — the
/// point is to stop polling the producer and release the connection.
async fn close_backpressure<C, B>(
    client_tx: &mut C,
    backend_tx: &mut B,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    C: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
    B: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = CloseFrame {
        code: CloseCode::Policy,
        reason: Utf8Bytes::from_static("ws backpressure/write timeout"),
    };
    let _ = client_tx.send(Message::Close(Some(frame))).await;
    let _ = client_tx.close().await;
    let _ = backend_tx.send(Message::Close(None)).await;
    let _ = backend_tx.close().await;
    Ok(())
}

/// Wrap a post-upgrade IO into a server-role [`WebSocketStream`].
pub async fn server_ws<IO>(io: IO, cfg: &WsConfig) -> WebSocketStream<IO>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    WebSocketStream::from_raw_socket(io, Role::Server, Some(cfg.tungstenite_config())).await
}

/// Wrap a handshaked client stream into a client-role [`WebSocketStream`].
pub async fn client_ws<IO>(io: IO, cfg: &WsConfig) -> WebSocketStream<IO>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    WebSocketStream::from_raw_socket(io, Role::Client, Some(cfg.tungstenite_config())).await
}

/// WS-over-H3 (RFC 9220) — drive the upstream RFC 6455 client handshake over an
/// already-dialed backend `stream`, returning the client-role stream and the
/// upstream-selected `sec-websocket-protocol`. The dial timeout (→ 504) is the
/// caller's; a handshake refusal here maps to the returned `Err` (→ 502).
///
/// # Errors
/// A human-readable message if the URI is malformed or the handshake fails.
pub async fn dial_backend_ws(
    stream: tokio::net::TcpStream,
    backend_addr: std::net::SocketAddr,
    path: &str,
    subprotocols: Option<&str>,
    cfg: &WsConfig,
) -> Result<(WebSocketStream<tokio::net::TcpStream>, Option<String>), String> {
    let uri = format!("ws://{backend_addr}{path}")
        .parse()
        .map_err(|e| format!("upstream ws uri build failed: {e}"))?;
    let mut builder = tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
    if let Some(protocols) = subprotocols {
        for p in protocols.split(',') {
            let p = p.trim();
            if !p.is_empty() {
                builder = builder.with_sub_protocol(p);
            }
        }
    }
    let (backend_ws, resp) = tokio_tungstenite::client_async_with_config(
        builder,
        stream,
        Some(cfg.tungstenite_config()),
    )
    .await
    .map_err(|e| format!("upstream handshake failed: {e}"))?;
    // RFC 8441 §5: the caller echoes this in the extended CONNECT 200.
    let negotiated = resp
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    Ok((backend_ws, negotiated))
}

static SEC_WEBSOCKET_VERSION: HeaderName = HeaderName::from_static("sec-websocket-version");
static SEC_WEBSOCKET_KEY: HeaderName = HeaderName::from_static("sec-websocket-key");

/// Case-insensitive token containment in an RFC 7230 §7 comma-separated list.
fn header_contains_token(headers: &hyper::HeaderMap, name: &HeaderName, token: &str) -> bool {
    for v in headers.get_all(name) {
        let Ok(s) = v.to_str() else { continue };
        for part in s.split(',') {
            if part.trim().eq_ignore_ascii_case(token) {
                return true;
            }
        }
    }
    false
}

/// Response header block for a successful RFC 6455 handshake; `None` if
/// `Sec-WebSocket-Key` is missing or malformed (the caller then rejects 400).
#[must_use]
pub fn build_handshake_response_headers<B>(
    req: &Request<B>,
) -> Option<Vec<(HeaderName, HeaderValue)>> {
    let key = req.headers().get(&SEC_WEBSOCKET_KEY)?.to_str().ok()?;
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
    let mut v = Vec::with_capacity(3);
    v.push((
        hyper::header::UPGRADE,
        HeaderValue::from_static("websocket"),
    ));
    v.push((
        hyper::header::CONNECTION,
        HeaderValue::from_static("Upgrade"),
    ));
    let accept_val = HeaderValue::from_str(&accept).ok()?;
    v.push((HeaderName::from_static("sec-websocket-accept"), accept_val));
    Some(v)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use http_body_util::Empty;
    use hyper::body::Bytes;
    use tokio::io::{DuplexStream, duplex};

    fn ws_request() -> Request<Empty<Bytes>> {
        Request::builder()
            .method("GET")
            .uri("/chat")
            .header(hyper::header::HOST, "example.com")
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Empty::<Bytes>::new())
            .unwrap()
    }

    #[test]
    fn upgrade_request_detected_correctly() {
        let req = ws_request();
        assert!(is_h1_upgrade_request(&req));
    }

    #[test]
    fn non_upgrade_request_passes_through() {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header(hyper::header::HOST, "example.com")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(!is_h1_upgrade_request(&req));
    }

    #[test]
    fn rejects_non_get() {
        let req = Request::builder()
            .method("POST")
            .uri("/chat")
            .header(hyper::header::HOST, "example.com")
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(!is_h1_upgrade_request(&req));
    }

    #[test]
    fn rejects_wrong_version() {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header("sec-websocket-version", "8")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(!is_h1_upgrade_request(&req));
    }

    #[test]
    fn rejects_missing_key() {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header("sec-websocket-version", "13")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(!is_h1_upgrade_request(&req));
    }

    #[test]
    fn connection_token_list_accepts_additional_tokens() {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "keep-alive, Upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(is_h1_upgrade_request(&req));
    }

    #[test]
    fn rfc8441_extended_connect_detected() {
        let mut req: Request<Empty<Bytes>> = Request::builder()
            .method(Method::CONNECT)
            .uri("example.com")
            .body(Empty::<Bytes>::new())
            .unwrap();
        req.extensions_mut()
            .insert(hyper::ext::Protocol::from_static("websocket"));
        assert!(is_h2_extended_connect(&req));
    }

    #[test]
    fn plain_connect_not_websocket() {
        // CONNECT without `:protocol` is a plain tunnel, not a WS bootstrap.
        let req: Request<Empty<Bytes>> = Request::builder()
            .method(Method::CONNECT)
            .uri("example.com")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(!is_h2_extended_connect(&req));
    }

    #[test]
    fn handshake_response_headers_includes_accept() {
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header(hyper::header::UPGRADE, "websocket")
            .header(hyper::header::CONNECTION, "Upgrade")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let resp = build_handshake_response_headers(&req).unwrap();
        let accept = resp
            .iter()
            .find(|(n, _)| n == "sec-websocket-accept")
            .map(|(_, v)| v.to_str().unwrap().to_owned())
            .unwrap();
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    // Idle path: neither side produces a frame → Ok(()) plus Close(1001).
    #[tokio::test]
    async fn close_code_1001_on_idle_timeout() {
        let (client_proxy_io, client_observer_io): (DuplexStream, DuplexStream) = duplex(4096);
        let (backend_proxy_io, _backend_observer_io): (DuplexStream, DuplexStream) = duplex(4096);

        let cfg = WsConfig {
            idle_timeout: Duration::from_millis(150),
            max_message_size: 64 * 1024,
            enabled: true,
            ping_rate_limit_per_window: DEFAULT_PING_RATE_LIMIT_PER_WINDOW,
            ping_rate_limit_window: DEFAULT_PING_RATE_LIMIT_WINDOW,
            // Watchdog above `idle_timeout` so this exercises the idle path.
            read_frame_timeout: Duration::from_secs(30),
        };
        let proxy = Arc::new(WsProxy::new(cfg));

        let client_ws_proxy = server_ws(client_proxy_io, &cfg).await;
        let backend_ws_proxy = client_ws(backend_proxy_io, &cfg).await;
        let client_observer_ws = client_ws(client_observer_io, &cfg).await;

        let handle =
            tokio::spawn(
                async move { proxy.proxy_frames(client_ws_proxy, backend_ws_proxy).await },
            );

        let mut observer = client_observer_ws;
        let timeout = Duration::from_secs(2);
        let msg = tokio::time::timeout(timeout, observer.next())
            .await
            .expect("observer did not receive a frame before outer timeout")
            .expect("stream ended before Close frame")
            .expect("tungstenite error on observer");
        match msg {
            Message::Close(Some(frame)) => {
                assert_eq!(frame.code, CloseCode::Away, "expected 1001 Going Away");
            }
            other => panic!("expected Close(1001), got {other:?}"),
        }
        let _ = handle.await;
    }

    /// F-S27-2 — a flooding backend pushes at a client that NEVER reads; with
    /// the `max_write_buffer_size` bound the relay parks and the backend's
    /// pushed count PLATEAUS. Reverting the bound flips this RED.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn backpressure_plateaus_producer_when_consumer_stalls() {
        use std::sync::atomic::{AtomicU64, Ordering};

        const MSG_BYTES: usize = 1024;
        const FLOOD: u64 = 4_096;
        const CEILING: u64 = 256; // decisive vs 4096; far above the true plateau.

        let cfg = WsConfig {
            idle_timeout: Duration::from_secs(30),
            read_frame_timeout: Duration::from_secs(30),
            max_message_size: 16 * 1024,
            enabled: true,
            ..WsConfig::default()
        };

        let (client_proxy_io, client_observer_io): (DuplexStream, DuplexStream) = duplex(4096);
        let (backend_proxy_io, backend_observer_io): (DuplexStream, DuplexStream) = duplex(4096);

        let proxy = Arc::new(WsProxy::new(cfg));
        let client_ws_proxy = server_ws(client_proxy_io, &cfg).await;
        let backend_ws_proxy = client_ws(backend_proxy_io, &cfg).await;
        let mut backend = server_ws(backend_observer_io, &cfg).await;
        // The client observer NEVER reads — that is what fills the write buffer.
        let _client_observer = client_ws(client_observer_io, &cfg).await;

        let relay = tokio::spawn(async move {
            let _ = proxy.proxy_frames(client_ws_proxy, backend_ws_proxy).await;
        });

        let pushed = Arc::new(AtomicU64::new(0));
        let pushed_bg = Arc::clone(&pushed);
        let flood = tokio::spawn(async move {
            let payload = vec![0xCDu8; MSG_BYTES];
            for _ in 0..FLOOD {
                if backend
                    .feed(Message::Binary(payload.clone().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                if backend.flush().await.is_err() {
                    break;
                }
                pushed_bg.fetch_add(1, Ordering::Relaxed);
            }
        });

        // Unbounded, the relay would drain the whole flood in this window.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let n = pushed.load(Ordering::Relaxed);
        eprintln!("F-S27-2 duplex plateau: backend pushed {n} / {FLOOD} (ceiling {CEILING})");
        assert!(
            n > 0,
            "non-vacuous: the backend must have pushed at least one frame, got {n}"
        );
        assert!(
            n < CEILING,
            "R8(ii) VIOLATION: with the consumer stalled the backend pushed {n} of \
             {FLOOD} frames — the gateway is NOT backpressuring (expected a plateau \
             < {CEILING}). The `max_write_buffer_size` bound is not in effect."
        );

        flood.abort();
        relay.abort();
    }

    /// R10 — the write-wedge liveness guard `close_backpressure`: a peer that
    /// stops draining must get `Close 1008` and an `Ok(())` teardown, not a
    /// hung task.
    ///
    /// DETERMINISM: the SHORT `read_frame_timeout` (200 ms) against a generous
    /// `idle_timeout` (30 s) guarantees the Close observed is the write-timeout
    /// 1008, not the 1001 idle close.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_backpressure_1008_on_forward_write_timeout() {
        // SHORT read-frame budget (the trigger) + GENEROUS idle (so the
        // both-silent envelope cannot pre-empt it) + SMALL max_message_size.
        let cfg = WsConfig {
            idle_timeout: Duration::from_secs(30),
            read_frame_timeout: Duration::from_millis(200),
            max_message_size: 16 * 1024,
            enabled: true,
            ..WsConfig::default()
        };

        // A TINY backend pipe so `backend_tx.send()` cannot complete.
        let (client_proxy_io, client_observer_io): (DuplexStream, DuplexStream) = duplex(4096);
        let (backend_proxy_io, backend_observer_io): (DuplexStream, DuplexStream) = duplex(256);

        let proxy = Arc::new(WsProxy::new(cfg));
        let client_ws_proxy = server_ws(client_proxy_io, &cfg).await;
        let backend_ws_proxy = client_ws(backend_proxy_io, &cfg).await;
        // Role::Client so its frames are MASKED (RFC 6455) — the relay's client
        // half is Role::Server and rejects unmasked frames. `backend_observer_io`
        // is intentionally NEVER read: that is what wedges the forward send.
        let client_observer = client_ws(client_observer_io, &cfg).await;
        let (mut obs_tx, mut obs_rx) = client_observer.split();

        let relay_done = tokio::spawn(async move {
            // Returns Ok(()) from the close_backpressure arm.
            proxy.proxy_frames(client_ws_proxy, backend_ws_proxy).await
        });

        // Flood far past the relay's bounded write buffer; the eventual `feed`
        // wedge is expected, not a failure.
        let flood = tokio::spawn(async move {
            let payload = vec![0xABu8; 1024];
            for _ in 0..4096u32 {
                if obs_tx
                    .feed(Message::Binary(payload.clone().into()))
                    .await
                    .is_err()
                    || obs_tx.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let observe = tokio::spawn(async move {
            loop {
                match obs_rx.next().await {
                    Some(Ok(Message::Close(Some(frame)))) => return Some(frame),
                    Some(Ok(_)) => {} // skip any queued data frames
                    Some(Err(_)) | None => return None,
                }
            }
        });

        // DELAYED drainer: it must NOT read during the flood (so the send
        // wedges at ~200 ms) but MUST drain shortly after, so
        // `close_backpressure`'s own un-timed `send(Close(None))` completes.
        let backend_drainer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let mut backend = server_ws(backend_observer_io, &cfg).await;
            while let Some(Ok(_)) = backend.next().await {}
        });

        let result = tokio::time::timeout(Duration::from_secs(8), relay_done)
            .await
            .expect("proxy_frames must return within the bound (close_backpressure tears down)")
            .expect("relay task panicked");
        assert!(
            result.is_ok(),
            "close_backpressure returns Ok(()) (clean teardown), got {result:?}"
        );

        flood.abort();
        backend_drainer.abort();

        // 1008 with the write-timeout reason proves `close_backpressure` fired,
        // not the idle 1001 path.
        let frame = tokio::time::timeout(Duration::from_secs(2), observe)
            .await
            .expect("observer task must finish")
            .expect("observer task panicked")
            .expect("client observer must receive the Close frame from close_backpressure");
        assert_eq!(
            frame.code,
            CloseCode::Policy,
            "write-wedge teardown must emit Close 1008 (Policy Violation), got {:?}",
            frame.code
        );
        assert!(
            frame.reason.contains("backpressure/write timeout"),
            "Close reason must name the write-timeout guard, got {:?}",
            frame.reason
        );
    }
}
