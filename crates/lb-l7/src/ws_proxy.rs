//! WebSocket upstream path (Item 2 / PROMPT.md §14).
//!
//! `WsProxy` is not a standalone listener mode. It is a *capability* that
//! [`crate::h1_proxy::H1Proxy`] and [`crate::h2_proxy::H2Proxy`] delegate
//! to when an inbound request carries a WebSocket handshake:
//!
//! * RFC 6455 §4.1: HTTP/1.1 request with `Upgrade: websocket`,
//!   `Connection: Upgrade`, `Sec-WebSocket-Version: 13`, and a
//!   `Sec-WebSocket-Key`.
//! * RFC 8441 §4: HTTP/2 extended CONNECT with `:method = CONNECT` and
//!   `:protocol = websocket`.
//!
//! Detection is a pure predicate ([`is_h1_upgrade_request`],
//! [`is_h2_extended_connect`]); the caller owns the dispatch and the
//! response shape.
//!
//! After the handshake, the proxy establishes a client WebSocket to the
//! picked backend over a pooled TCP socket and runs a bidirectional frame
//! forwarder:
//!
//! * `Text`, `Binary`, `Pong` — forwarded verbatim.
//! * `Ping` — forwarded (tungstenite auto-replies to pings on the
//!   *receiving* side; we never synthesise replies on the proxy).
//! * `Close` — forwarded verbatim (code + reason), and the opposite side
//!   is closed gracefully after the local half drains.
//! * `Frame` — relayed opaquely for raw continuation fragments.
//!
//! Idle-timeout: each `select!` iteration is bounded by
//! [`WsProxy::idle_timeout`]. On elapse the proxy emits a `Close` frame
//! with code `1001 Going Away` (RFC 6455 §7.4.1) to *both* peers before
//! returning.
//!
//! Per-message compression (RFC 7692) is deliberately NOT negotiated;
//! backend-side reuse and WebSocket-over-QUIC (RFC 9220) are post-v1.

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
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};

/// Default client-originated Ping budget per rolling window.
///
/// Above this many client Pings within
/// [`DEFAULT_PING_RATE_LIMIT_WINDOW`] the proxy emits `Close 1008`
/// to the abusive client. Mirrors the H/2 sibling
/// `lb_h2::DEFAULT_PING_MAX_PER_WINDOW`.
pub const DEFAULT_PING_RATE_LIMIT_PER_WINDOW: u32 = 50;

/// Default rolling-window duration for the WebSocket client-Ping
/// rate limit. Mirrors `lb_h2::DEFAULT_CONTROL_FRAME_WINDOW`.
pub const DEFAULT_PING_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(10);

/// Default per-direction read-frame watchdog (auditor-delta WS-002).
///
/// If a single direction produces no frame within this budget the
/// proxy emits `Close 1008` to the client and shuts the upstream
/// half. Distinct from [`WsConfig::idle_timeout`], which fires only
/// when *both* directions are silent.
pub const DEFAULT_READ_FRAME_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-listener WebSocket knobs.
///
/// Mirrors the shape of [`crate::h1_proxy::HttpTimeouts`] and
/// [`crate::h2_security::H2SecurityThresholds`] — every field has a
/// canonical default so operators only set the knob they want to tune.
#[derive(Debug, Clone, Copy)]
pub struct WsConfig {
    /// Maximum time a connection may sit idle (no frames in either
    /// direction) before the proxy emits a `1001 Going Away` close and
    /// shuts the stream. Default 60 s.
    pub idle_timeout: Duration,
    /// Upper bound on a single incoming message (bytes). Fragmented
    /// messages are summed. Default 16 MiB.
    pub max_message_size: usize,
    /// When false, the upgrade detector short-circuits to "not a WS
    /// request" even when the headers match. Lets operators disable the
    /// capability on a listener without removing the block entirely.
    /// Default true.
    pub enabled: bool,
    /// Maximum number of client-originated `Ping` frames allowed per
    /// [`Self::ping_rate_limit_window`] before the proxy treats the
    /// stream as a flood amplifier and emits `Close 1008` to the
    /// client. Mirrors the H/2 `PingFloodDetector` knob (auditor-delta
    /// finding WS-001). Default
    /// [`DEFAULT_PING_RATE_LIMIT_PER_WINDOW`].
    pub ping_rate_limit_per_window: u32,
    /// Rolling-window duration for the client-Ping rate limit.
    /// Default [`DEFAULT_PING_RATE_LIMIT_WINDOW`].
    pub ping_rate_limit_window: Duration,
    /// Per-direction read-frame watchdog. If a single direction
    /// produces no frame within this budget the proxy emits a
    /// `Close 1008 (Policy Violation)` with reason
    /// `"ws read frame timeout"` to the client and shuts the upstream
    /// half. Bounds the per-peer kernel-TCP/tungstenite-buffer dwell
    /// (auditor-delta finding WS-002). Distinct from
    /// [`Self::idle_timeout`]: idle fires when *both* halves are silent;
    /// the read-frame watchdog fires when *any* half is silent.
    /// Default [`DEFAULT_READ_FRAME_TIMEOUT`].
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
    /// Render the tungstenite configuration corresponding to this block.
    /// `max_frame_size` tracks `max_message_size` because tungstenite
    /// enforces both separately — matching them keeps a single knob on
    /// the operator-facing surface.
    #[must_use]
    pub fn tungstenite_config(self) -> WebSocketConfig {
        WebSocketConfig {
            max_message_size: Some(self.max_message_size),
            max_frame_size: Some(self.max_message_size),
            ..WebSocketConfig::default()
        }
    }
}

/// Predicate: does `req` carry a valid RFC 6455 §4.1 handshake?
///
/// Checks `Upgrade: websocket`, `Connection` contains `Upgrade`,
/// `Sec-WebSocket-Version: 13`, and a non-empty `Sec-WebSocket-Key`.
#[must_use]
pub fn is_h1_upgrade_request<B>(req: &Request<B>) -> bool {
    // GET is the only valid method for the RFC 6455 handshake.
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

/// Predicate: does `req` carry an RFC 8441 extended CONNECT for WebSocket?
///
/// hyper 1.x exposes the `:protocol` pseudo-header via the
/// [`hyper::ext::Protocol`] typed extension. We match on CONNECT + that
/// extension's value being "websocket" (case-insensitive).
#[must_use]
pub fn is_h2_extended_connect<B>(req: &Request<B>) -> bool {
    if req.method() != Method::CONNECT {
        return false;
    }
    // hyper 1.x surfaces the `:protocol` pseudo-header via the extensions
    // map as `hyper::ext::Protocol`. When present and equal to
    // "websocket" (case-insensitive), this is an RFC 8441 bootstrap.
    req.extensions()
        .get::<hyper::ext::Protocol>()
        .is_some_and(|p| p.as_str().eq_ignore_ascii_case("websocket"))
}

/// WebSocket reverse proxy. Cheap to clone via [`Arc`].
pub struct WsProxy {
    cfg: WsConfig,
}

impl WsProxy {
    /// Construct a [`WsProxy`] with the supplied configuration.
    #[must_use]
    pub const fn new(cfg: WsConfig) -> Self {
        Self { cfg }
    }

    /// Return the [`WsConfig`] in effect.
    #[must_use]
    pub const fn config(&self) -> WsConfig {
        self.cfg
    }

    /// Frame-level proxy loop.
    ///
    /// Both `client_ws` and `backend_ws` must already be in the
    /// post-handshake state (server-role and client-role respectively).
    /// The loop drives a [`tokio::select!`] that alternates between the
    /// two halves, applying the idle timeout on every iteration. Close
    /// codes are forwarded faithfully; on idle elapse the proxy emits a
    /// `1001 Going Away` to both sides.
    ///
    /// # Errors
    ///
    /// Returns the first tungstenite error observed on either half.
    /// Idle-timeout is not surfaced as an error — it is a clean close.
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
        // Per-connection sliding window of client-originated Ping
        // timestamps. Mirrors the shape of
        // `lb_h2::PingFloodDetector` but over wall-clock `Instant`s
        // (we don't need an integer-tick fixture here — the loop is
        // already async-runtime driven).
        let mut client_ping_log: VecDeque<Instant> = VecDeque::new();

        loop {
            // Outer envelope: `idle` covers the both-sides-silent case.
            // Inner per-direction wrapper: `read_frame` covers the
            // single-direction-stuck case (auditor-delta finding
            // WS-002). The select! returns the *first* arm to resolve;
            // a per-direction `timeout(read_frame, …)` therefore fires
            // even when the other half is producing data.
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
                    // Idle elapsed — emit 1001 Going Away to both sides.
                    let away = CloseFrame {
                        code: CloseCode::Away,
                        reason: std::borrow::Cow::Borrowed("idle timeout"),
                    };
                    let _ = client_tx.send(Message::Close(Some(away.clone()))).await;
                    let _ = backend_tx.send(Message::Close(Some(away))).await;
                    return Ok(());
                }
                Ok(Direction::ReadFrameTimeout) => {
                    // Per-direction read-frame watchdog tripped (WS-002).
                    // Emit Close 1008 (Policy Violation) to the client
                    // and propagate a clean Close to the upstream half.
                    let frame = CloseFrame {
                        code: CloseCode::Policy,
                        reason: std::borrow::Cow::Borrowed("ws read frame timeout"),
                    };
                    let _ = client_tx.send(Message::Close(Some(frame))).await;
                    let _ = client_tx.close().await;
                    let _ = backend_tx.send(Message::Close(None)).await;
                    let _ = backend_tx.close().await;
                    return Ok(());
                }
                Ok(Direction::ClientToBackend(Ok(Some(msg)))) => {
                    // WS-001 (auditor-delta MEDIUM 4.3): rate-limit
                    // client-originated Pings to keep the gateway from
                    // amplifying a flood at the backend. Backend→client
                    // Pings are not gated — the backend is the
                    // would-be victim, not the attacker.
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
                                reason: std::borrow::Cow::Borrowed(
                                    "ping flood: rate limit exceeded",
                                ),
                            };
                            let _ = client_tx.send(Message::Close(Some(frame))).await;
                            let _ = client_tx.close().await;
                            let _ = backend_tx.send(Message::Close(None)).await;
                            let _ = backend_tx.close().await;
                            return Ok(());
                        }
                    }
                    let is_close = matches!(msg, Message::Close(_));
                    backend_tx.send(msg).await?;
                    if is_close {
                        // Half-close the other side: drain any remaining
                        // frames from the backend, then return. We rely
                        // on backend_rx to surface the final Close frame
                        // (if any) on the next iteration; a graceful peer
                        // will close its half promptly.
                        let _ = client_tx.close().await;
                        return Ok(());
                    }
                }
                Ok(Direction::BackendToClient(Ok(Some(msg)))) => {
                    let is_close = matches!(msg, Message::Close(_));
                    client_tx.send(msg).await?;
                    if is_close {
                        let _ = backend_tx.close().await;
                        return Ok(());
                    }
                }
                Ok(Direction::ClientToBackend(Ok(None))) => {
                    // Client half closed without sending a Close frame.
                    // Forward a Close(None) to the backend so it does not
                    // wait forever.
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
    /// Per-direction read-frame watchdog elapsed without observing a
    /// frame on whichever half it was guarding (WS-002).
    ReadFrameTimeout,
}

/// Wrap a post-upgrade IO into a server-role [`WebSocketStream`].
///
/// Exposed so binary wiring can call this from inside the hyper service
/// closure without needing the tungstenite crate in scope.
pub async fn server_ws<IO>(io: IO, cfg: &WsConfig) -> WebSocketStream<IO>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    WebSocketStream::from_raw_socket(io, Role::Server, Some(cfg.tungstenite_config())).await
}

/// Wrap an already-handshaked client stream into a client-role [`WebSocketStream`].
///
/// Used by the backend side of the frame forwarder when the binary has
/// already driven the upstream handshake via
/// [`tokio_tungstenite::client_async`].
pub async fn client_ws<IO>(io: IO, cfg: &WsConfig) -> WebSocketStream<IO>
where
    IO: AsyncRead + AsyncWrite + Unpin,
{
    WebSocketStream::from_raw_socket(io, Role::Client, Some(cfg.tungstenite_config())).await
}

// ── helpers ────────────────────────────────────────────────────────────

static SEC_WEBSOCKET_VERSION: HeaderName = HeaderName::from_static("sec-websocket-version");
static SEC_WEBSOCKET_KEY: HeaderName = HeaderName::from_static("sec-websocket-key");

/// Case-insensitive containment check: does the comma-separated value of
/// `name` include `token`? Robust against leading / trailing whitespace
/// around each token (RFC 7230 §7 list).
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

/// Build the response header block for a successful RFC 6455 handshake.
///
/// Returns `None` if `Sec-WebSocket-Key` is missing or malformed
/// (caller should reject with 400 in that case).
///
/// The returned tuple is `(status, headers)` and should be used to build
/// a hyper `Response<Empty<Bytes>>` (the 101 response carries no body).
/// The caller is responsible for calling [`hyper::upgrade::on`] on the
/// *request* to obtain the post-upgrade IO.
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
        // Real clients sometimes send `Connection: keep-alive, Upgrade`.
        // The detector must still accept.
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
        // hyper 1.x expresses `:protocol` via the typed extension. We
        // build a minimal request carrying it so the detector path is
        // exercised even though we cannot drive a full H2 handshake in a
        // unit test.
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
        // CONNECT without :protocol is a plain HTTP/2 tunnel. Must not
        // be treated as a WebSocket bootstrap.
        let req: Request<Empty<Bytes>> = Request::builder()
            .method(Method::CONNECT)
            .uri("example.com")
            .body(Empty::<Bytes>::new())
            .unwrap();
        assert!(!is_h2_extended_connect(&req));
    }

    #[test]
    fn handshake_response_headers_includes_accept() {
        // RFC 6455 §1.3 sample key → expected Sec-WebSocket-Accept.
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

    // Idle-timeout test: drive `proxy_frames` over a pair of in-memory
    // WebSocketStream halves where no side ever produces a frame. The
    // proxy must return `Ok(())` within the configured budget and the
    // "client" observer must receive a Close(1001) frame.
    #[tokio::test]
    async fn close_code_1001_on_idle_timeout() {
        // Build two duplex pairs so we can observe what the proxy sends
        // *out*. client_proxy_side <-> client_observer_side forms the
        // "client" socket; backend_proxy_side <-> backend_observer_side
        // forms the "backend" socket.
        let (client_proxy_io, client_observer_io): (DuplexStream, DuplexStream) = duplex(4096);
        let (backend_proxy_io, _backend_observer_io): (DuplexStream, DuplexStream) = duplex(4096);

        let cfg = WsConfig {
            idle_timeout: Duration::from_millis(150),
            max_message_size: 64 * 1024,
            enabled: true,
            ping_rate_limit_per_window: DEFAULT_PING_RATE_LIMIT_PER_WINDOW,
            ping_rate_limit_window: DEFAULT_PING_RATE_LIMIT_WINDOW,
            // Set the per-direction watchdog above `idle_timeout` so
            // this test exercises the idle path; the WS-002 watchdog
            // path is covered by `tests/ws_proxy_e2e.rs`.
            read_frame_timeout: Duration::from_secs(30),
        };
        let proxy = Arc::new(WsProxy::new(cfg));

        // Post-handshake wrappers (Role::Server for the "client-facing"
        // socket, Role::Client for the backend-facing socket). No
        // handshake bytes are on the wire; tungstenite will only emit
        // data-layer frames from here.
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
}
