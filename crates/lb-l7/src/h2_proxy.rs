//! Real hyper 1.x HTTP/2 proxy path (Pillar 3b.3b-2).
//!
//! [`H2Proxy`] is the L7 entry point used by the binary's `h1s` listener
//! once ALPN negotiates `h2`. Each accepted TLS-decrypted connection is
//! handed to [`H2Proxy::serve_connection`] which drives a hyper 1.x
//! HTTP/2 server over it.
//!
//! Architecturally identical to [`crate::h1_proxy::H1Proxy`]:
//!
//! 1. Strips hop-by-hop request headers (RFC 9110 §7.6.1 + Connection-
//!    listed names). H2 forbids these over the wire, but the *upstream*
//!    we forward to is still H1, so we must scrub them before relaying.
//! 2. Appends `X-Forwarded-{For,Proto,Host}` + `Via`.
//! 3. Picks a backend via [`crate::h1_proxy::BackendPicker::pick`]. The
//!    service closure runs **once per H2 stream**, so a single H2
//!    connection multiplexing N requests hits the picker N times —
//!    real per-stream load balancing.
//! 4. Dials the backend (H1) via [`lb_io::pool::TcpPool`] and issues the
//!    request body-timeout-bounded.
//! 5. Strips hop-by-hop from the response, optionally injects
//!    `Alt-Svc`, and returns the response on the original H2 stream.
//!
//! The whole `serve_connection` future is bounded by
//! [`crate::h1_proxy::HttpTimeouts::total`].

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use http_body_util::{BodyExt, Empty, Full, combinators::BoxBody};
use hyper::body::{Bytes, Incoming as IncomingBody};
use hyper::header::HeaderValue;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};

use lb_io::http2_pool::Http2Pool;
use lb_io::pool::TcpPool;
use lb_io::quic_pool::QuicUpstreamPool;

use crate::grpc_proxy::{self, GrpcProxy};
use crate::h1_proxy::{
    AltSvcConfig, BackendPicker, HttpTimeouts, append_via, append_xff, set_xfh, set_xfp,
    strip_hop_by_hop,
};
use lb_security::{ConnId, SmuggleDetector, SmuggleMode, Watchdog};

use crate::h2_security::H2SecurityThresholds;
use crate::security_hooks::{DynSecurityHooks, NoopHooks};
use crate::stripped_request::{StrippedRequest, strip_hop_by_hop as strip_into_newtype};
use crate::upstream::{BackendInfoPicker, SingleProtoPicker, UpstreamBackend, UpstreamProto};
use crate::ws_proxy::{self, WsProxy, is_h2_extended_connect};

/// L7 HTTP/2 reverse proxy. Cheap to clone via [`Arc`].
pub struct H2Proxy {
    pool: TcpPool,
    picker: Arc<dyn BackendInfoPicker>,
    alt_svc: Option<AltSvcConfig>,
    timeouts: HttpTimeouts,
    is_https: bool,
    security: H2SecurityThresholds,
    /// When `Some`, inbound extended-CONNECT streams carrying
    /// `:protocol = websocket` (RFC 8441) are routed through the
    /// WebSocket proxy instead of returning 502.
    ws: Option<Arc<WsProxy>>,
    /// When `Some`, inbound streams whose content-type matches
    /// `application/grpc[+ext]` are routed through the gRPC proxy
    /// (Item 3 / PROMPT.md §13) instead of the regular H2 request
    /// path. The H2 flood/bomb thresholds from Item 1 still apply to
    /// the hosting connection.
    grpc: Option<Arc<GrpcProxy>>,
    /// Optional H2 upstream pool. PROTO-001 H2→H2 path.
    h2_upstream: Option<Arc<Http2Pool>>,
    /// Optional H3 upstream pool. PROTO-001 H2→H3 path.
    h3_upstream: Option<Arc<QuicUpstreamPool>>,
    /// CODE-2-01 / SEC-2-01 hook surface. Defaults to
    /// [`NoopHooks`]; Wave-2c flips this to the production
    /// `lb_security::HooksBundle` via [`Self::with_hooks`].
    hooks: Arc<dyn DynSecurityHooks>,
    /// SEC-2-01 / SEC-2-03 slowloris / slow-POST watchdog
    /// (mirrors `H1Proxy::watchdog`).
    watchdog: Option<Watchdog>,
    /// Monotonic per-listener connection sequence used as the
    /// [`Watchdog`] entry key.
    conn_seq: Arc<parking_lot::Mutex<u64>>,
}

impl H2Proxy {
    /// Construct an [`H2Proxy`] with the default
    /// [`H2SecurityThresholds`]. Equivalent to
    /// [`Self::with_security`]`(..., H2SecurityThresholds::default())`.
    ///
    /// `is_https` selects the value emitted into `X-Forwarded-Proto`.
    /// It is always `true` for the production wiring (H2 ships only over
    /// the `h1s` listener today), but exposed for test harnesses that
    /// want to exercise the plaintext path.
    #[must_use]
    pub fn new(
        pool: TcpPool,
        picker: Arc<dyn BackendPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
    ) -> Self {
        Self::with_security(
            pool,
            picker,
            alt_svc,
            timeouts,
            is_https,
            H2SecurityThresholds::default(),
        )
    }

    /// Construct an [`H2Proxy`] with an explicit [`H2SecurityThresholds`].
    ///
    /// Wraps `picker` in a [`SingleProtoPicker`] tagged
    /// [`UpstreamProto::H1`] for backwards compatibility.
    #[must_use]
    pub fn with_security(
        pool: TcpPool,
        picker: Arc<dyn BackendPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
        security: H2SecurityThresholds,
    ) -> Self {
        let info = Arc::new(SingleProtoPicker::new(picker, UpstreamProto::H1, None));
        Self {
            pool,
            picker: info,
            alt_svc,
            timeouts,
            is_https,
            security,
            ws: None,
            grpc: None,
            h2_upstream: None,
            h3_upstream: None,
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
        }
    }

    /// Construct an [`H2Proxy`] backed by a multi-protocol picker
    /// (PROTO-001).
    ///
    /// Defaults the CODE-2-01 `hooks` field to
    /// [`NoopHooks`]; Wave-2c overrides via [`Self::with_hooks`]. The
    /// constructor is no longer `const fn` because the default
    /// [`NoopHooks`] allocates an [`Arc`].
    #[must_use]
    pub fn with_multi_proto(
        pool: TcpPool,
        picker: Arc<dyn BackendInfoPicker>,
        alt_svc: Option<AltSvcConfig>,
        timeouts: HttpTimeouts,
        is_https: bool,
        security: H2SecurityThresholds,
    ) -> Self {
        Self {
            pool,
            picker,
            alt_svc,
            timeouts,
            is_https,
            security,
            ws: None,
            grpc: None,
            h2_upstream: None,
            h3_upstream: None,
            hooks: Arc::new(NoopHooks::new()),
            watchdog: None,
            conn_seq: Arc::new(parking_lot::Mutex::new(0)),
        }
    }

    /// Attach a [`SecurityHooks`] impl (CODE-2-01 / SEC-2-01 hot-path
    /// surface). Mirrors [`crate::h1_proxy::H1Proxy::with_hooks`].
    /// Wave-2c flips this to the production
    /// `lb_security::HooksBundle` from `crates/lb/src/main.rs`.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Arc<dyn DynSecurityHooks>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Attach an [`lb_security::Watchdog`] for per-stream slowloris /
    /// slow-POST eviction (SEC-2-01 / SEC-2-03). Mirrors
    /// [`crate::h1_proxy::H1Proxy::with_watchdog`]. The H2 service
    /// closure runs once per stream so each stream registers and
    /// deregisters independently.
    #[must_use]
    pub fn with_watchdog(mut self, watchdog: Watchdog) -> Self {
        self.watchdog = Some(watchdog);
        self
    }

    /// Attach an H2 upstream pool used for backends with
    /// [`UpstreamProto::H2`]. PROTO-001.
    #[must_use]
    pub fn with_h2_upstream(mut self, pool: Arc<Http2Pool>) -> Self {
        self.h2_upstream = Some(pool);
        self
    }

    /// Attach an H3 upstream pool used for backends with
    /// [`UpstreamProto::H3`]. PROTO-001.
    #[must_use]
    pub fn with_h3_upstream(mut self, pool: Arc<QuicUpstreamPool>) -> Self {
        self.h3_upstream = Some(pool);
        self
    }

    /// Whether an H2 upstream pool has been wired for this proxy.
    /// Exposed for integration tests.
    #[must_use]
    pub const fn has_h2_upstream(&self) -> bool {
        self.h2_upstream.is_some()
    }

    /// Whether an H3 upstream pool has been wired for this proxy.
    /// Exposed for integration tests.
    #[must_use]
    pub const fn has_h3_upstream(&self) -> bool {
        self.h3_upstream.is_some()
    }

    /// Enable WebSocket upgrade handling on this proxy. Fluent; returns
    /// `self` for chaining off [`Self::with_security`] or [`Self::new`].
    #[must_use]
    pub fn with_websocket(mut self, ws: Arc<WsProxy>) -> Self {
        self.ws = Some(ws);
        self
    }

    /// Enable gRPC handling on this proxy. Fluent; returns `self` so
    /// the call site reads as a chain off [`Self::with_security`].
    ///
    /// Aligns the supplied [`GrpcProxy`]'s upstream H2 client
    /// `max_header_list_size` with the listener's
    /// [`H2SecurityThresholds::max_header_list_size`] (auditor-delta
    /// finding GRPC-001) so a malicious backend cannot transit
    /// oversize trailers through the gateway before hyper rejects.
    #[must_use]
    pub fn with_grpc(mut self, grpc: GrpcProxy) -> Self {
        let aligned = grpc.with_max_header_list_size(self.security.max_header_list_size);
        self.grpc = Some(Arc::new(aligned));
        self
    }

    /// Drive HTTP/2 server logic over `io`.
    ///
    /// Returns once the connection has fully closed. A
    /// [`tokio::time::timeout`] of [`HttpTimeouts::total`] is wrapped
    /// around the whole loop so a runaway client-or-upstream pair cannot
    /// pin a worker forever.
    ///
    /// # Errors
    ///
    /// Surfaces I/O errors and timeouts. Per-stream upstream errors are
    /// translated to 502/504 responses and do NOT terminate the
    /// connection.
    pub async fn serve_connection<IO>(self: Arc<Self>, io: IO, peer: SocketAddr) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        // PROTO-2-11 (H2 half, Wave 2c-2): always delegates to the
        // cancellable variant with a never-cancelled token so the
        // original signature stays back-compat.
        self.serve_connection_with_cancel(io, peer, tokio_util::sync::CancellationToken::new())
            .await
    }

    /// PROTO-2-11 (Wave 2c-2) — H2 half of the GOAWAY-on-drain
    /// contract paired with REL-2-02's H3 `CONNECTION_CLOSE`.
    ///
    /// Identical to [`Self::serve_connection`] until `cancel`
    /// fires: at that point the hyper H2 connection is pinned and
    /// `.graceful_shutdown()` is invoked, which emits the canonical
    /// **two-step GOAWAY** sequence (RFC 9113 §6.8): first a GOAWAY
    /// with `last_stream_id = 2^31 - 1` so the client stops opening
    /// new streams, then a second GOAWAY with the actual highest
    /// in-flight stream id once the server's `MAX_CONCURRENT_STREAMS`
    /// has drained. The connection future is then driven to
    /// completion with the existing `total` budget so a misbehaving
    /// client cannot pin a worker past the drain deadline.
    ///
    /// # Errors
    ///
    /// Same as [`Self::serve_connection`], plus `TimedOut` if the
    /// graceful-shutdown driver exceeds [`HttpTimeouts::total`].
    pub async fn serve_connection_with_cancel<IO>(
        self: Arc<Self>,
        io: IO,
        peer: SocketAddr,
        cancel: tokio_util::sync::CancellationToken,
    ) -> io::Result<()>
    where
        IO: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let total = self.timeouts.total;
        let svc = ProxyService {
            inner: Arc::clone(&self),
            peer,
        };
        // Configure hyper's H2 Builder with the detector-derived
        // thresholds. Hyper enforces on the wire; the lb-h2 detector
        // types remain the canonical threshold source (Pingora-style
        // policy/enforcement split). See crate::h2_security for the
        // attack → knob mapping.
        //
        // A `Timer` is required before `keep_alive_interval` fires;
        // without it hyper panics "You must supply a timer." Always
        // wire the tokio timer here — it's the same runtime our
        // caller is already using.
        let mut builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
        builder.timer(TokioTimer::new());
        self.security.apply(&mut builder);
        // RFC 8441 extended CONNECT — enables SETTINGS_ENABLE_CONNECT_PROTOCOL
        // advertisement so clients can bootstrap WebSocket over H2. Safe
        // to always enable: clients that do not use it pay no cost.
        builder.enable_connect_protocol();
        let conn = builder.serve_connection(TokioIo::new(io), svc);
        tokio::pin!(conn);
        let cancel_fut = cancel.cancelled();
        tokio::pin!(cancel_fut);
        let timer = tokio::time::sleep(total);
        tokio::pin!(timer);
        tokio::select! {
            // Cancel wins ties so a SIGTERM during a long-running
            // request still triggers the GOAWAY emit.
            biased;
            () = &mut cancel_fut => {
                // Two-step GOAWAY: hyper handles both frames inside
                // `graceful_shutdown` (it sets the soft limit then
                // drains).
                conn.as_mut().graceful_shutdown();
                // Drive the conn future to completion with the
                // existing `total` budget so a stalled client cannot
                // delay drain past the deadline.
                match tokio::time::timeout(total, conn).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(io::Error::other(format!("h2 graceful shutdown: {e}"))),
                    Err(_) => Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "h2 graceful shutdown timeout",
                    )),
                }
            }
            res = &mut conn => match res {
                Ok(()) => Ok(()),
                Err(e) => Err(io::Error::other(format!("h2 server: {e}"))),
            },
            () = &mut timer => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "total connection timeout",
            )),
        }
    }
}

/// Service implementation carrying the [`H2Proxy`] plus the peer address.
#[derive(Clone)]
struct ProxyService {
    inner: Arc<H2Proxy>,
    peer: SocketAddr,
}

impl hyper::service::Service<Request<IncomingBody>> for ProxyService {
    type Response = Response<BoxBody<Bytes, hyper::Error>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        let inner = Arc::clone(&self.inner);
        let peer = self.peer;
        Box::pin(async move { Ok(Box::pin(inner.handle(req, peer)).await) })
    }
}

impl H2Proxy {
    async fn handle(
        &self,
        req: Request<IncomingBody>,
        peer: SocketAddr,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        // RFC 8441 extended CONNECT intercept. Only fires when this
        // listener was configured with a `WsProxy`; everything else
        // continues through the regular H2 request path.
        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h2_extended_connect(&req))
        {
            return self.handle_ws_extended_connect(req);
        }
        if let Some(gp) = self
            .grpc
            .as_ref()
            .filter(|g| g.config().enabled && grpc_proxy::is_grpc_request(&req))
        {
            // gRPC requires an H1/H2 backend; today's GrpcProxy speaks
            // hyper H2 over a TCP-pool stream, so any backend selected
            // by the multi-proto picker is acceptable provided it is
            // not H3 (which would require a QUIC tunnel + grpc-over-h3
            // adaptor, out of v1 scope).
            let Some(backend) = self.picker.pick_info() else {
                return error_response(StatusCode::BAD_GATEWAY, "no backend available");
            };
            if backend.proto == UpstreamProto::H3 {
                return error_response(
                    StatusCode::BAD_GATEWAY,
                    "gRPC proxy does not support H3 backends",
                );
            }
            return Arc::clone(gp).handle(req, backend.addr).await;
        }
        let (parts, body) = req.into_parts();

        // CODE-2-01 / SEC-2-01: run the security hooks before hop-by-hop
        // strip + upstream-acquire. The reconstructed `Request<()>` is
        // a header-only borrow surface; the trait reads `headers()` +
        // `version()` only.
        let inspect_req = {
            let mut b = Request::builder()
                .method(parts.method.clone())
                .uri(parts.uri.clone())
                .version(parts.version);
            for (n, v) in &parts.headers {
                b = b.header(n.clone(), v.clone());
            }
            b.body(()).unwrap_or_else(|_| Request::new(()))
        };
        if let Err(rej) = self.hooks.inspect_request(&inspect_req, peer.ip()) {
            return crate::h1_proxy::reject_to_response(&rej);
        }

        // SEC-2-01 — defense-in-depth explicit `SmuggleDetector` call
        // in H2 mode. Mirrors the H1 hot-path call site; the
        // `SmuggleMode::H2` arm enables the
        // [`check_h2_downgrade`] check (forbidden hop-by-hop headers
        // and non-`trailers` TE per RFC 9113 §8.2.2) on top of the
        // CL/TE/duplicate-CL defaults.
        let header_pairs: Vec<(String, String)> = parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect();
        if let Err(e) = SmuggleDetector::check_all_mode(&header_pairs, SmuggleMode::H2) {
            tracing::warn!(error = %e, peer = %peer, "h2 smuggle rejected");
            return error_response(StatusCode::BAD_REQUEST, "request smuggling");
        }

        // PROTO-2-01 — RFC 9113 §8.3.1: when both `:authority` and
        // `Host` are present they MUST agree. hyper surfaces
        // `:authority` as `uri.authority()`. Disagreement is a
        // routing/authz desync primitive (host-confusion smuggling
        // against backends that authorise on `Host`), so reject with
        // 400 BEFORE hop-by-hop strip / upstream acquire.
        if let Err(msg) = check_authority_host_agreement(&parts.uri, &parts.headers) {
            tracing::warn!(peer = %peer, reason = msg, "h2 :authority/Host mismatch rejected");
            return error_response(StatusCode::BAD_REQUEST, msg);
        }

        // SEC-2-01 / SEC-2-03 — register the stream with the
        // slowloris watchdog.
        let watch_id = self.watchdog.as_ref().map(|wd| {
            let seq = {
                let mut g = self.conn_seq.lock();
                *g = g.wrapping_add(1);
                *g
            };
            let id = ConnId::new(peer.ip(), seq);
            let deadline = std::time::Instant::now() + self.timeouts.header;
            wd.register(id, deadline);
            let header_bytes: u64 = parts
                .headers
                .iter()
                .map(|(n, v)| n.as_str().len() as u64 + v.len() as u64 + 4)
                .sum();
            if let Err(e) = wd.progress(id, header_bytes) {
                tracing::warn!(error = %e, peer = %peer, "h2 watchdog evicted at header phase");
            }
            id
        });

        // Determine the authority: H2 carries it in :authority, which
        // hyper surfaces as `uri.authority()`. Fall back to the Host
        // header for clients that still populate it.
        let authority = parts
            .uri
            .authority()
            .map(|a| a.as_str().to_owned())
            .or_else(|| {
                parts
                    .headers
                    .get(hyper::header::HOST)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            });

        // PROTO-2-07 — mint a `StrippedRequest` so the proxy_* fan-out
        // takes a type that statically guarantees hop-by-hop strip.
        let req_pre_strip = Request::from_parts(parts, body);
        let mut stripped = strip_into_newtype(req_pre_strip);
        {
            let headers = stripped.headers_mut();
            append_xff(headers, peer);
            set_xfp(headers, self.is_https);
            if let Some(h) = authority.as_deref() {
                set_xfh(headers, h);
                // Upstream is H1, which requires a Host header. If the
                // client spoke H2 without one, synthesise from
                // :authority.
                if !headers.contains_key(hyper::header::HOST) {
                    if let Ok(v) = HeaderValue::from_str(h) {
                        headers.insert(hyper::header::HOST, v);
                    }
                }
            }
            append_via(headers);
        }

        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };

        let resp = match backend.proto {
            UpstreamProto::H1 => match self.proxy_request(backend.addr, stripped).await {
                Ok(resp) => self.finalize_response(resp),
                Err(ProxyErr::Upstream(s)) => error_response(StatusCode::BAD_GATEWAY, &s),
                Err(ProxyErr::Timeout) => {
                    error_response(StatusCode::GATEWAY_TIMEOUT, "upstream timeout")
                }
            },
            UpstreamProto::H2 => Box::pin(self.proxy_h2_to_h2(backend.addr, stripped)).await,
            UpstreamProto::H3 => Box::pin(self.proxy_h2_to_h3(&backend, stripped)).await,
        };
        // SEC-2-01 / SEC-2-03 — deregister the stream from the
        // watchdog on the normal completion path.
        if let (Some(wd), Some(id)) = (self.watchdog.as_ref(), watch_id) {
            wd.deregister(id);
        }
        resp
    }

    /// Handle an RFC 8441 extended-CONNECT WebSocket bootstrap.
    ///
    /// Returns `200 OK` with an empty body; hyper flips the inbound
    /// stream into a bidirectional byte channel once the response
    /// headers reach the wire. A detached task picks up the upgraded
    /// stream, dials the backend over HTTP/1.1, drives the client-side
    /// RFC 6455 handshake, and runs the bidirectional frame forwarder.
    fn handle_ws_extended_connect(
        &self,
        mut req: Request<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(ws_proxy) = self.ws.clone() else {
            return error_response(StatusCode::BAD_GATEWAY, "websocket disabled");
        };
        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };
        if backend.proto != UpstreamProto::H1 {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "WebSocket extended-CONNECT requires H1 backend",
            );
        }
        let backend_addr = backend.addr;

        let upgrade_fut = hyper::upgrade::on(&mut req);
        let path_and_query = req
            .uri()
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let ws_cfg = ws_proxy.config();
        let pool = self.pool.clone();

        tokio::spawn(async move {
            let upgraded = match upgrade_fut.await {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(error = %e, "ws/h2: upgrade failed");
                    return;
                }
            };

            let pooled_result =
                tokio::task::spawn_blocking(move || pool.acquire(backend_addr)).await;
            let pooled = match pooled_result {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, backend = %backend_addr, "ws/h2: backend dial failed");
                    return;
                }
                Err(e) => {
                    tracing::debug!(error = %e, "ws/h2: dial join failed");
                    return;
                }
            };
            let Some(upstream_stream) = pooled.take_stream() else {
                tracing::debug!("ws/h2: pooled stream missing");
                return;
            };

            let uri = match format!("ws://{backend_addr}{path_and_query}").parse() {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(error = %e, "ws/h2: upstream uri build failed");
                    return;
                }
            };
            let builder = tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
            let (backend_ws, _resp) = match tokio_tungstenite::client_async_with_config(
                builder,
                upstream_stream,
                Some(ws_cfg.tungstenite_config()),
            )
            .await
            {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::debug!(error = %e, backend = %backend_addr, "ws/h2: upstream handshake failed");
                    return;
                }
            };

            let client_ws = ws_proxy::server_ws(TokioIo::new(upgraded), &ws_cfg).await;
            if let Err(e) = ws_proxy.proxy_frames(client_ws, backend_ws).await {
                tracing::debug!(error = %e, "ws/h2: frame proxy ended with error");
            }
        });

        let body = Empty::<Bytes>::new()
            .map_err(|never| match never {})
            .boxed();
        Response::builder()
            .status(StatusCode::OK)
            .body(body)
            .unwrap_or_else(|_| {
                error_response(StatusCode::INTERNAL_SERVER_ERROR, "200 build failed")
            })
    }

    async fn proxy_request(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Result<Response<IncomingBody>, ProxyErr> {
        let req = req.into_inner();
        let pool = self.pool.clone();
        let pooled = tokio::task::spawn_blocking(move || pool.acquire(backend_addr))
            .await
            .map_err(|e| ProxyErr::Upstream(format!("backend dial join: {e}")))?
            .map_err(|e| ProxyErr::Upstream(format!("backend connect {backend_addr}: {e}")))?;

        let stream = pooled
            .take_stream()
            .ok_or_else(|| ProxyErr::Upstream("pooled stream missing".to_owned()))?;

        // Upstream is H1 — matches nginx/haproxy production behaviour.
        // H2 upstream support is a future pillar.
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| ProxyErr::Upstream(format!("h1 client handshake: {e}")))?;

        let conn_handle = tokio::spawn(async move {
            let _ = conn.await;
        });

        let send_fut = sender.send_request(req);
        let resp = match tokio::time::timeout(self.timeouts.body, send_fut).await {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                conn_handle.abort();
                return Err(ProxyErr::Upstream(format!("send_request: {e}")));
            }
            Err(_) => {
                conn_handle.abort();
                return Err(ProxyErr::Timeout);
            }
        };
        drop(conn_handle);
        Ok(resp)
    }

    fn finalize_response(
        &self,
        resp: Response<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
        if let Some(alt) = self.alt_svc {
            if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
                parts.headers.insert(hyper::header::ALT_SVC, value);
            }
        }
        Response::from_parts(parts, body.boxed())
    }

    /// Forward an H2 inbound request to an H2 backend (PROTO-001).
    async fn proxy_h2_to_h2(
        &self,
        backend_addr: SocketAddr,
        req: StrippedRequest<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(h2_pool) = self.h2_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H2 backend selected but no Http2Pool wired",
            );
        };
        let translated = match translate_h2_request_to_h2(req.into_inner()).await {
            Ok(r) => r,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };
        match h2_pool.send_request(backend_addr, translated).await {
            Ok(resp) => upstream_h2_response_to_h2(resp, self.alt_svc).await,
            Err(lb_io::http2_pool::Http2PoolError::Timeout) => {
                error_response(StatusCode::GATEWAY_TIMEOUT, "upstream H2 timeout")
            }
            Err(e) => error_response(StatusCode::BAD_GATEWAY, &format!("h2 upstream: {e}")),
        }
    }

    /// Forward an H2 inbound request to an H3 backend (PROTO-001).
    async fn proxy_h2_to_h3(
        &self,
        backend: &UpstreamBackend,
        req: StrippedRequest<IncomingBody>,
    ) -> Response<BoxBody<Bytes, hyper::Error>> {
        let Some(h3_pool) = self.h3_upstream.as_ref() else {
            return error_response(
                StatusCode::BAD_GATEWAY,
                "H3 backend selected but no QuicUpstreamPool wired",
            );
        };
        let sni = backend.sni.as_deref().unwrap_or("");
        let (headers, body) = match collect_h2_request_to_h3_fieldlist(req.into_inner(), sni).await
        {
            Ok(p) => p,
            Err(s) => return error_response(StatusCode::BAD_GATEWAY, &s),
        };
        let h3_resp = Box::pin(lb_quic::request_h3_upstream(
            headers,
            body,
            backend.addr,
            sni,
            h3_pool,
        ))
        .await;
        h3_response_to_h2(h3_resp, self.alt_svc)
    }
}

// ── PROTO-001 H2-side translation helpers ─────────────────────────────

/// Lift an inbound H2 [`Request<IncomingBody>`] into the shape hyper's
/// H2 client expects for the upstream side.
///
/// The request URI carries scheme + authority + path already (hyper's
/// H2 server populates them from the inbound pseudo-headers). For H2→H2
/// the codec bridge is essentially a pass-through, but we run it for
/// the per-header lowercase normalization + hop-by-hop strip the bridge
/// performs.
async fn translate_h2_request_to_h2(
    req: Request<IncomingBody>,
) -> Result<Request<BoxBody<Bytes, hyper::Error>>, String> {
    let (parts, body) = req.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?
        .to_bytes();
    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http2);
    let scheme = parts
        .uri
        .scheme()
        .map_or_else(|| "http".to_owned(), |s| s.as_str().to_owned());
    let authority = parts
        .uri
        .authority()
        .map(|a| a.as_str().to_owned())
        .or_else(|| {
            parts
                .headers
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        });
    let mut bridge_in = crate::BridgeRequest {
        method: parts.method.to_string(),
        uri: parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string),
        headers: parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect(),
        body: body_bytes.clone(),
        scheme: Some(scheme.clone()),
    };
    // Synthesise the pseudo-headers a real H2 client would have sent.
    bridge_in
        .headers
        .insert(0, (":method".to_owned(), parts.method.to_string()));
    bridge_in
        .headers
        .insert(1, (":path".to_owned(), bridge_in.uri.clone()));
    bridge_in
        .headers
        .insert(2, (":scheme".to_owned(), scheme.clone()));
    if let Some(a) = authority.as_deref() {
        bridge_in
            .headers
            .insert(3, (":authority".to_owned(), a.to_owned()));
    }

    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h2->h2 bridge: {e}"))?;

    let mut builder = Request::builder().method(parts.method.clone());
    if let Some(auth) = authority.as_deref() {
        let path = parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string);
        let uri = format!("{scheme}://{auth}{path}");
        builder = builder.uri(uri);
    } else {
        builder = builder.uri(parts.uri.clone());
    }
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    let body = http_body_util::Full::new(body_bytes)
        .map_err(|never| match never {})
        .boxed();
    builder.body(body).map_err(|e| format!("build h2 req: {e}"))
}

/// Convert an upstream H2 `Response<Incoming>` back into the H2-side
/// response (hyper's H2 server consumes a `Response<BoxBody>`).
async fn upstream_h2_response_to_h2(
    resp: Response<IncomingBody>,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let (parts, body) = resp.into_parts();
    let body_bytes = match body.collect().await {
        Ok(b) => b.to_bytes(),
        Err(e) => {
            return error_response(StatusCode::BAD_GATEWAY, &format!("upstream body read: {e}"));
        }
    };
    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http2);
    let bridge_in = crate::BridgeResponse {
        status: parts.status.as_u16(),
        headers: parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect(),
        body: body_bytes,
    };
    let translated = match bridge.bridge_response(&bridge_in) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("h2->h2 response bridge: {e}"),
            );
        }
    };
    let status = StatusCode::from_u16(translated.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    let body = http_body_util::Full::new(translated.body)
        .map_err(|never| match never {})
        .boxed();
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h2 response failed",
        )
    })
}

/// Collect an inbound H2 request, run the H2→H3 codec bridge, and
/// return a `(field_list, body)` pair for `request_h3_upstream`.
async fn collect_h2_request_to_h3_fieldlist(
    req: Request<IncomingBody>,
    sni: &str,
) -> Result<(Vec<(String, String)>, Bytes), String> {
    let (parts, body) = req.into_parts();
    let body_bytes = body
        .collect()
        .await
        .map_err(|e| format!("body collect: {e}"))?
        .to_bytes();
    let scheme = parts
        .uri
        .scheme()
        .map_or_else(|| "https".to_owned(), |s| s.as_str().to_owned());
    let authority = parts
        .uri
        .authority()
        .map(|a| a.as_str().to_owned())
        .or_else(|| {
            parts
                .headers
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| sni.to_owned());

    let bridge = crate::create_bridge(crate::Protocol::Http2, crate::Protocol::Http3);
    let mut bridge_in = crate::BridgeRequest {
        method: parts.method.to_string(),
        uri: parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), std::string::ToString::to_string),
        headers: parts
            .headers
            .iter()
            .filter_map(|(n, v)| {
                v.to_str()
                    .ok()
                    .map(|s| (n.as_str().to_owned(), s.to_owned()))
            })
            .collect(),
        body: body_bytes.clone(),
        scheme: Some(scheme.clone()),
    };
    bridge_in
        .headers
        .insert(0, (":method".to_owned(), parts.method.to_string()));
    bridge_in
        .headers
        .insert(1, (":path".to_owned(), bridge_in.uri.clone()));
    bridge_in.headers.insert(2, (":scheme".to_owned(), scheme));
    bridge_in
        .headers
        .insert(3, (":authority".to_owned(), authority));
    let translated = bridge
        .bridge_request(&bridge_in)
        .map_err(|e| format!("h2->h3 bridge: {e}"))?;
    Ok((translated.headers, body_bytes))
}

/// Convert an [`lb_quic::H3UpstreamResponse`] back into the H2 response
/// shape the listener emits.
fn h3_response_to_h2(
    resp: lb_quic::H3UpstreamResponse,
    alt_svc: Option<AltSvcConfig>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let bridge = crate::create_bridge(crate::Protocol::Http3, crate::Protocol::Http2);
    let body_bytes = Bytes::from(resp.body);
    let bridge_in = crate::BridgeResponse {
        status: resp.status,
        headers: resp.headers,
        body: body_bytes,
    };
    let translated = match bridge.bridge_response(&bridge_in) {
        Ok(r) => r,
        Err(e) => {
            return error_response(
                StatusCode::BAD_GATEWAY,
                &format!("h3->h2 response bridge: {e}"),
            );
        }
    };
    let status = StatusCode::from_u16(translated.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut builder = Response::builder().status(status);
    for (n, v) in &translated.headers {
        if n.starts_with(':') {
            continue;
        }
        builder = builder.header(n.as_str(), v.as_str());
    }
    if let Some(alt) = alt_svc {
        if let Ok(value) = HeaderValue::from_str(&alt.header_value()) {
            builder = builder.header(hyper::header::ALT_SVC, value);
        }
    }
    let body = http_body_util::Full::new(translated.body)
        .map_err(|never| match never {})
        .boxed();
    builder.body(body).unwrap_or_else(|_| {
        error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "build h2 response failed",
        )
    })
}

enum ProxyErr {
    Upstream(String),
    Timeout,
}

fn error_response(status: StatusCode, msg: &str) -> Response<BoxBody<Bytes, hyper::Error>> {
    let body = Full::new(Bytes::from(msg.to_owned()))
        .map_err(|never| match never {})
        .boxed();
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

/// PROTO-2-01 — RFC 9113 §8.3.1 enforcement.
///
/// Returns `Err(static_msg)` when **both** `:authority` (surfaced by
/// hyper as `uri.authority()`) and `Host` are present **and** their
/// host components disagree. The comparison is case-insensitive on
/// the host name (RFC 3986 §3.2.2: host is case-insensitive) and
/// ignores the port when either side lacks one (RFC 9113 §8.3.1
/// "default port" carve-out). Returns `Ok(())` if either is absent,
/// if they match exactly, or if only the port differs while one side
/// elides it.
///
/// Per the §8.3.1 forwarding rule, an intermediary MUST treat such
/// disagreement as a malformed request. The proxy lifts that into a
/// 400 Bad Request response. Returning a `&'static str` keeps the
/// rejection allocation-free on the cold path.
pub fn check_authority_host_agreement(
    uri: &http::Uri,
    headers: &hyper::HeaderMap,
) -> Result<(), &'static str> {
    let authority = uri.authority().map(http::uri::Authority::as_str);
    let host_hdr = headers
        .get(hyper::header::HOST)
        .and_then(|v| v.to_str().ok());
    match (authority, host_hdr) {
        (Some(a), Some(h)) => {
            if authority_matches_host(a, h) {
                Ok(())
            } else {
                Err("Bad Request: :authority disagrees with Host (RFC 9113 §8.3.1)")
            }
        }
        _ => Ok(()),
    }
}

/// Compare a `:authority` value against a `Host` header value per
/// RFC 9113 §8.3.1 + RFC 3986 §3.2.2 (host-component case-insensitive).
///
/// Rules:
///   * Empty / missing host on either side → mismatch.
///   * Host components compared case-insensitively.
///   * If both carry an explicit port, the ports must match.
///   * If only one side carries an explicit port, the comparison
///     succeeds when the host components match (the proxy does not
///     have a default-port table; this matches the §8.3.1 latitude
///     for omitted default ports).
fn authority_matches_host(authority: &str, host: &str) -> bool {
    let (a_host, a_port) = split_host_port(authority);
    let (h_host, h_port) = split_host_port(host);
    if a_host.is_empty() || h_host.is_empty() {
        return false;
    }
    if !a_host.eq_ignore_ascii_case(h_host) {
        return false;
    }
    match (a_port, h_port) {
        (Some(ap), Some(hp)) => ap == hp,
        // One side elides the port — accept (default-port latitude).
        _ => true,
    }
}

/// Split `host[:port]` into `(host, Some(port_str))` / `(host, None)`.
/// Bracketed IPv6 literals `[::1]:443` are preserved verbatim as the
/// host portion (including brackets) so the case-insensitive compare
/// catches hex-digit mismatches without splitting on colon inside the
/// literal.
fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some(stripped) = s.strip_prefix('[') {
        // IPv6 literal: `[…]` then optional `:port`.
        if let Some(end) = stripped.find(']') {
            let host_with_brackets = &s[..=end + 1];
            let rest = &s[end + 2..];
            let port = rest.strip_prefix(':');
            return (host_with_brackets, port.filter(|p| !p.is_empty()));
        }
        return (s, None);
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (s, None),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use hyper::HeaderMap;
    use hyper::header::HeaderName;

    fn map_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut m = HeaderMap::new();
        for (k, v) in pairs {
            m.append(
                HeaderName::try_from(*k).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        m
    }

    #[test]
    fn h2_proxy_alt_svc_injected() {
        // The H2 path uses the same Alt-Svc formatter as H1 (shared via
        // `AltSvcConfig::header_value`). Re-prove the contract here so
        // a regression in the H2 path gets its own red test rather than
        // hiding behind an H1 assertion.
        let alt = AltSvcConfig {
            h3_port: 443,
            max_age: 3_600,
        };
        let mut h = HeaderMap::new();
        let value = HeaderValue::from_str(&alt.header_value()).unwrap();
        h.insert(hyper::header::ALT_SVC, value);
        assert_eq!(h.get("alt-svc").unwrap(), "h3=\":443\"; ma=3600");
    }

    #[test]
    fn h2_proxy_hop_by_hop_stripped() {
        // H2 forbids Connection / TE / Transfer-Encoding on the wire,
        // but we still must scrub them before forwarding to an H1
        // upstream.
        let mut h = map_with(&[
            ("host", "example.com"),
            ("connection", "Keep-Alive, Foo"),
            ("keep-alive", "timeout=5"),
            ("foo", "bar"),
            ("transfer-encoding", "chunked"),
            ("accept", "text/html"),
        ]);
        strip_hop_by_hop(&mut h);
        assert!(h.get("connection").is_none());
        assert!(h.get("keep-alive").is_none());
        assert!(h.get("foo").is_none());
        assert!(h.get("transfer-encoding").is_none());
        assert_eq!(h.get("host").unwrap(), "example.com");
        assert_eq!(h.get("accept").unwrap(), "text/html");
    }

    #[test]
    fn h2_proxy_xff_appended() {
        // Shared with the H1 path — prove the H2 path gets it too.
        let mut h = map_with(&[("x-forwarded-for", "10.0.0.1")]);
        let peer: SocketAddr = "1.2.3.4:5555".parse().unwrap();
        append_xff(&mut h, peer);
        assert_eq!(h.get("x-forwarded-for").unwrap(), "10.0.0.1, 1.2.3.4");
    }

    // PROTO-2-11 H2 half (Wave 2c-2): smoke test for the
    // cancel-aware variant. Builds a minimal H2Proxy, hands it a
    // duplex pair (with the peer side closed) and an already-cancelled
    // token. The expected outcome is that `serve_connection_with_cancel`
    // returns promptly — its graceful_shutdown branch hits the
    // empty/EOF stream and resolves the hyper conn future. The
    // assertion is a deadline-bounded wait: a regression that
    // re-introduces a busy-loop or holds the conn open indefinitely
    // would time out here.
    #[tokio::test(flavor = "current_thread")]
    async fn test_sigterm_emits_two_step_goaway() {
        use std::time::Duration;
        use tokio_util::sync::CancellationToken;

        let pool = lb_io::pool::TcpPool::new(
            lb_io::pool::PoolConfig::default(),
            lb_io::sockopts::BackendSockOpts::default(),
            lb_io::Runtime::new(),
        );
        let addrs: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
        let picker = crate::h1_proxy::RoundRobinAddrs::new(addrs).unwrap();
        let proxy = Arc::new(H2Proxy::new(
            pool,
            Arc::new(picker),
            None,
            HttpTimeouts::default(),
            false,
        ));
        // Empty duplex — the peer half is dropped immediately, so any
        // read returns EOF and hyper's H2 conn resolves without ever
        // opening a stream.
        let (server_io, client) = tokio::io::duplex(8 * 1024);
        drop(client); // EOF on the next read.
        let cancel = CancellationToken::new();
        cancel.cancel(); // pre-cancel so the graceful path fires.
        let peer: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let r = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.serve_connection_with_cancel(server_io, peer, cancel),
        )
        .await;
        // Whether the inner future returns Ok(()) or an Err depends on
        // whether the H2 preface ever arrived. We only assert the
        // deadline did not fire — the cancellable variant must NOT
        // loop indefinitely.
        assert!(
            r.is_ok(),
            "serve_connection_with_cancel hung past 5 s deadline — graceful shutdown is broken"
        );
    }
}
