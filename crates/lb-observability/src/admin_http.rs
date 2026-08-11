//! Admin HTTP listener: `GET` on `/metrics`, `/healthz`, `/livez`, `/readyz`, `/startupz`.
//! NO TLS and NO mTLS. Bearer-token auth is OPTIONAL — [`serve_with_auth`] enforces it on
//! information-bearing endpoints, [`serve_with_probes`] serves everything anonymously; even with a
//! token the transport is plaintext, so expect a loopback bind behind a proxy or management VPN.

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

use lb_security::AdminAuthGate;

use crate::MetricsRegistry;
use crate::probes::{ProbeRegistry, ProbeState};
use crate::prometheus_exposition::{CONTENT_TYPE, render_text};

#[derive(Clone)]
struct AdminService {
    registry: Arc<MetricsRegistry>,
    probes: Arc<ProbeRegistry>,
    /// When present, requests need a bearer token; probes stay EXEMPT so the kubelet can reach them.
    auth: Option<Arc<AdminAuthGate>>,
}

impl Service<hyper::Request<Incoming>> for AdminService {
    type Response = Response<Full<Bytes>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, request: hyper::Request<Incoming>) -> Self::Future {
        let reg_arc = Arc::clone(&self.registry);
        let probes = Arc::clone(&self.probes);
        let auth = self.auth.clone();
        Box::pin(async move { Ok(route(&reg_arc, &probes, auth.as_deref(), &request)) })
    }
}

/// Bearer-token check. Probes are exempt: k8s hits them anonymously and they reveal no secrets.
fn is_probe_path(path: &str) -> bool {
    matches!(path, "/livez" | "/healthz" | "/startupz" | "/readyz")
}

fn route(
    registry: &MetricsRegistry,
    probes: &ProbeRegistry,
    auth: Option<&AdminAuthGate>,
    request: &hyper::Request<Incoming>,
) -> Response<Full<Bytes>> {
    if request.method() != http::Method::GET {
        return plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
    }
    if let Some(gate) = auth {
        if gate.enforced() && !is_probe_path(request.uri().path()) {
            let header = request
                .headers()
                .get(http::header::AUTHORIZATION)
                .and_then(|h| h.to_str().ok());
            if gate.authorize(header).is_err() {
                return plain(StatusCode::FORBIDDEN, "forbidden\n");
            }
        }
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

/// `/livez` — 200 while alive, INCLUDING during drain, or k8s kills the pod mid-shutdown.
fn livez_response(probes: &ProbeRegistry) -> Response<Full<Bytes>> {
    let status = if probes.is_live() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    json_status(status, probes.state())
}

/// `/readyz` — 200 only when [`ProbeState::Ready`]; 503 during boot and drain.
fn readyz_response(probes: &ProbeRegistry) -> Response<Full<Bytes>> {
    let state = probes.state();
    let status = if matches!(state, ProbeState::Ready) {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    json_status(status, state)
}

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
    // Hand-formatted to avoid a serde_json dep; escaping is safe ONLY because `body_token` is closed.
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
    // Unreachable (static inputs), but returned rather than unwrapped: the crate denies `unwrap_used`.
    let mut r = Response::new(Full::new(Bytes::from_static(
        b"internal error building response\n",
    )));
    *r.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    r
}

/// Serve the admin endpoints until `shutdown` fires; a per-connection failure never stops the listener.
pub async fn serve_with_probes(
    registry: Arc<MetricsRegistry>,
    probes: Arc<ProbeRegistry>,
    addr: SocketAddr,
    shutdown: CancellationToken,
) -> io::Result<SocketAddr> {
    serve_with_auth(registry, probes, None, addr, shutdown).await
}

/// [`serve_with_probes`] with optional bearer-token enforcement; probes stay anonymous. Does NOT
/// check the bind address — the caller must have run [`AdminAuthGate::validate_bind`] first.
pub async fn serve_with_auth(
    registry: Arc<MetricsRegistry>,
    probes: Arc<ProbeRegistry>,
    auth: Option<Arc<AdminAuthGate>>,
    addr: SocketAddr,
    shutdown: CancellationToken,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    let enforced = auth.as_ref().is_some_and(|g| g.enforced());
    tracing::info!(
        address = %local,
        bearer_auth = enforced,
        "admin http listener started (/metrics, /livez, /readyz, /startupz, /healthz)"
    );
    let svc = AdminService {
        registry,
        probes,
        auth,
    };

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

/// Back-compat wrapper with its own [`ProbeRegistry`]; readiness can never be flipped through it.
pub async fn serve(
    registry: Arc<MetricsRegistry>,
    addr: SocketAddr,
    shutdown: CancellationToken,
) -> io::Result<SocketAddr> {
    let probes = ProbeRegistry::shared();
    // Forced Ready so legacy `/healthz` consumers keep seeing 200.
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // SEC-2-06: /livez stays anonymous even with the bearer gate on; /metrics 403s without a token.
    #[tokio::test(flavor = "current_thread")]
    async fn test_admin_403_without_token() {
        use http::HeaderValue;
        use lb_security::{AdminAuthGate, AdminTokenHash};

        let reg = Arc::new(MetricsRegistry::new());
        let probes = ProbeRegistry::shared();
        probes.set_ready();
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let token_hash = AdminTokenHash::from_plaintext("super-secret");
        let gate = Arc::new(AdminAuthGate::new(Some(token_hash)));
        let local = serve_with_auth(Arc::clone(&reg), probes, Some(gate), addr, cancel.clone())
            .await
            .unwrap();
        let stream = tokio::net::TcpStream::connect(local).await.unwrap();
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, h1_conn) =
            hyper::client::conn::http1::handshake::<_, http_body_util::Empty<bytes::Bytes>>(io)
                .await
                .unwrap();
        tokio::spawn(h1_conn);
        let req = http::Request::builder()
            .method(http::Method::GET)
            .uri("/metrics")
            .header(http::header::HOST, HeaderValue::from_static("localhost"))
            .body(http_body_util::Empty::new())
            .unwrap();
        let resp = sender.send_request(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "no token → 403");
        cancel.cancel();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
