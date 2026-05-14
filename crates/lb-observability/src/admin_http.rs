//! Admin HTTP listener.
//!
//! Endpoints (all `GET`-only, loopback-bind expected):
//!
//! | Path        | Body type                | Semantics                                                     |
//! |-------------|--------------------------|---------------------------------------------------------------|
//! | `/metrics`  | Prometheus text 0.0.4    | Registry exposition for the local scraper                     |
//! | `/healthz`  | `text/plain`             | Back-compat alias for `/livez` (REL-2-04)                     |
//! | `/livez`    | `application/json`       | 200 while the runtime is alive; never 503 (REL-2-04)          |
//! | `/readyz`   | `application/json`       | 200 only while `ProbeState::Ready`; 503 otherwise (REL-2-04)  |
//! | `/startupz` | `application/json`       | 200 once `!ProbeState::Starting`; 503 during boot (REL-2-04)  |
//!
//! Intended for loopback scrapes. No TLS, no auth — the operator is
//! expected to bind it to 127.0.0.1 behind a reverse proxy or over a
//! management VPN. mTLS is deliberately out of scope for this pillar.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use http::{Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::Service;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::MetricsRegistry;
use crate::probes::{ProbeRegistry, ProbeState};
use crate::prometheus_exposition::{CONTENT_TYPE, render_text};

/// HTTP request handler that exposes the admin endpoints.
#[derive(Clone)]
struct AdminService {
    registry: Arc<MetricsRegistry>,
    probes: Arc<ProbeRegistry>,
}

impl Service<hyper::Request<Incoming>> for AdminService {
    type Response = Response<Full<Bytes>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, request: hyper::Request<Incoming>) -> Self::Future {
        let reg_arc = Arc::clone(&self.registry);
        let probes = Arc::clone(&self.probes);
        Box::pin(async move { Ok(route(&reg_arc, &probes, &request)) })
    }
}

fn route(
    registry: &MetricsRegistry,
    probes: &ProbeRegistry,
    request: &hyper::Request<Incoming>,
) -> Response<Full<Bytes>> {
    if request.method() != http::Method::GET {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
    }
    match request.uri().path() {
        "/metrics" => {
            let body = render_text(registry);
            Response::builder()
                .status(StatusCode::OK)
                .header(http::header::CONTENT_TYPE, CONTENT_TYPE)
                .body(Full::new(Bytes::from(body)))
                .unwrap_or_else(|_| fallback_500())
        }
        "/healthz" | "/livez" => livez_response(probes),
        "/readyz" => readyz_response(probes),
        "/startupz" => startupz_response(probes),
        _ => plain(StatusCode::NOT_FOUND, "not found\n"),
    }
}

/// REL-2-04: `/livez` — 200 while the runtime is alive. Stays 200
/// even during drain so K8s does not kill the pod mid-shutdown.
fn livez_response(probes: &ProbeRegistry) -> Response<Full<Bytes>> {
    let status = if probes.is_live() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    json_status(status, probes.state())
}

/// REL-2-04: `/readyz` — 200 iff [`ProbeState::Ready`]. 503 during
/// boot and during drain.
fn readyz_response(probes: &ProbeRegistry) -> Response<Full<Bytes>> {
    let state = probes.state();
    let status = if matches!(state, ProbeState::Ready) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    json_status(status, state)
}

/// REL-2-04: `/startupz` — 200 once the startup sequence has
/// completed at least once (i.e. NOT `Starting`).
fn startupz_response(probes: &ProbeRegistry) -> Response<Full<Bytes>> {
    let state = probes.state();
    let status = if probes.is_started() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    json_status(status, state)
}

fn json_status(status: StatusCode, state: ProbeState) -> Response<Full<Bytes>> {
    // Hand-formatted JSON to avoid pulling serde_json into
    // lb-observability for a one-key object. The token vocabulary is
    // a closed set defined in `ProbeState::body_token`, so escaping
    // is unnecessary.
    let body = format!("{{\"status\":\"{}\"}}\n", state.body_token());
    Response::builder()
        .status(status)
        .header(
            http::header::CONTENT_TYPE,
            "application/json; charset=utf-8",
        )
        .body(Full::new(Bytes::from(body)))
        .unwrap_or_else(|_| fallback_500())
}

fn plain(status: StatusCode, body: &'static str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from_static(body.as_bytes())))
        .unwrap_or_else(|_| fallback_500())
}

fn fallback_500() -> Response<Full<Bytes>> {
    // Response::builder only fails on invalid header values; the inputs
    // above are static strings, so this branch is unreachable at
    // runtime. We still return a Response rather than panic so
    // `#![deny(clippy::unwrap_used)]` passes.
    let mut r = Response::new(Full::new(Bytes::from_static(
        b"internal error building response\n",
    )));
    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    r
}

/// Bind `addr` and serve admin HTTP requests until `shutdown` fires.
///
/// `probes` is the [`ProbeRegistry`] consulted by the
/// `/livez`/`/readyz`/`/startupz` handlers. The caller (Wave 2c
/// `main.rs`) keeps a clone so it can flip the state on bind / drain.
///
/// The listener runs as a standalone loop; `serve` only returns when
/// the cancellation token is tripped or the bind fails. Per-connection
/// tasks are best-effort; a single bad client never takes the listener
/// down.
///
/// # Errors
///
/// Returns an [`io::Error`] if the TCP bind fails. Successful accepts
/// whose handshake or request handling subsequently errors are logged
/// at `debug` and do not propagate.
pub async fn serve_with_probes(
    registry: Arc<MetricsRegistry>,
    probes: Arc<ProbeRegistry>,
    addr: SocketAddr,
    shutdown: CancellationToken,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    tracing::info!(
        address = %local,
        "admin http listener started (/metrics, /livez, /readyz, /startupz, /healthz)"
    );
    let svc = AdminService { registry, probes };

    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    tracing::info!(address = %local, "admin http listener shutting down");
                    return;
                }
                res = listener.accept() => res,
            };
            let (stream, peer) = match accepted {
                Ok(v) => v,
                Err(e) => {
                    tracing::debug!(error = %e, "admin accept error");
                    continue;
                }
            };
            let svc = svc.clone();
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                if let Err(e) = hyper::server::conn::http1::Builder::new()
                    .keep_alive(true)
                    .serve_connection(io, svc)
                    .await
                {
                    tracing::debug!(peer = %peer, error = %e, "admin http connection ended");
                }
            });
        }
    });

    Ok(local)
}

/// Back-compat wrapper used by call sites that have not yet been
/// updated to thread the [`ProbeRegistry`] through (notably
/// `crates/lb/src/main.rs` until Wave 2c).
///
/// Internally synthesises a stand-alone [`ProbeRegistry`] in the
/// `Starting` state. Callers that need to actually flip readiness
/// must use [`serve_with_probes`] and keep their own clone of the
/// registry.
///
/// # Errors
///
/// Same conditions as [`serve_with_probes`].
pub async fn serve(
    registry: Arc<MetricsRegistry>,
    addr: SocketAddr,
    shutdown: CancellationToken,
) -> io::Result<SocketAddr> {
    let probes = ProbeRegistry::shared();
    // Until the caller wires the real probe registry, mark Ready so
    // that legacy `/healthz` consumers continue to see 200. The
    // Wave-2c switch (REL-2-02) replaces this with a real registry
    // owned by `main.rs`.
    probes.set_ready();
    serve_with_probes(registry, probes, addr, shutdown).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn bind_and_shutdown() {
        let reg = Arc::new(MetricsRegistry::new());
        let probes = ProbeRegistry::shared();
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let local = serve_with_probes(Arc::clone(&reg), probes, addr, cancel.clone())
            .await
            .unwrap();
        assert!(local.port() > 0);
        cancel.cancel();
        // Give the accept loop a tick to notice the cancellation.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
