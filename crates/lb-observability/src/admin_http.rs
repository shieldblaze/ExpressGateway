//! Admin HTTP listener: `GET /metrics` (text exposition) and `GET /healthz`.
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
use crate::prometheus_exposition::{CONTENT_TYPE, render_text};

/// HTTP request handler that exposes the two admin endpoints.
#[derive(Clone)]
struct AdminService {
    registry: Arc<MetricsRegistry>,
}

impl Service<hyper::Request<Incoming>> for AdminService {
    type Response = Response<Full<Bytes>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, request: hyper::Request<Incoming>) -> Self::Future {
        let reg_arc = Arc::clone(&self.registry);
        Box::pin(async move { Ok(route(&reg_arc, &request)) })
    }
}

fn route(registry: &MetricsRegistry, request: &hyper::Request<Incoming>) -> Response<Full<Bytes>> {
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
        "/healthz" => plain(StatusCode::OK, "ok\n"),
        _ => plain(StatusCode::NOT_FOUND, "not found\n"),
    }
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
pub async fn serve(
    registry: Arc<MetricsRegistry>,
    addr: SocketAddr,
    shutdown: CancellationToken,
) -> io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;
    tracing::info!(address = %local, "admin http listener started (/metrics, /healthz)");
    let svc = AdminService { registry };

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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn bind_and_shutdown() {
        let reg = Arc::new(MetricsRegistry::new());
        let cancel = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let local = serve(Arc::clone(&reg), addr, cancel.clone()).await.unwrap();
        assert!(local.port() > 0);
        cancel.cancel();
        // Give the accept loop a tick to notice the cancellation.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
