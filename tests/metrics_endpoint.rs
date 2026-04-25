//! Integration tests for the Prometheus admin HTTP listener (Task #21).
//!
//! Spawns `lb_observability::admin_http::serve` on an ephemeral
//! loopback port and asserts the two endpoints render the expected
//! shape.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use lb_observability::{MetricsRegistry, admin_http};

async fn get(addr: SocketAddr, path: &str) -> (http::StatusCode, String) {
    let stream = TcpStream::connect(addr).await.unwrap();
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(io)
        .await
        .unwrap();
    tokio::spawn(conn);
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("host", "localhost")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = sender.send_request(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (parts.status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn metrics_endpoint_serves_text_exposition() {
    let reg = Arc::new(MetricsRegistry::new());
    let counter = reg
        .counter("int_test_hits_total", "integration hits")
        .unwrap();
    counter.inc_by(7);

    let cancel = CancellationToken::new();
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let local = admin_http::serve(Arc::clone(&reg), bind, cancel.clone())
        .await
        .unwrap();

    // Small delay to let the spawned accept loop reach the first select.
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (status, body) = get(local, "/metrics").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        body.contains("int_test_hits_total 7"),
        "body missing counter sample:\n{body}"
    );
    assert!(
        body.contains("# HELP int_test_hits_total integration hits"),
        "body missing HELP line:\n{body}"
    );

    cancel.cancel();
}

#[tokio::test]
async fn metrics_endpoint_healthz_returns_200() {
    let reg = Arc::new(MetricsRegistry::new());
    let cancel = CancellationToken::new();
    let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let local = admin_http::serve(Arc::clone(&reg), bind, cancel.clone())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;

    let (status, body) = get(local, "/healthz").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body, "ok\n");

    let (status, _body) = get(local, "/unknown").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);

    cancel.cancel();
}
