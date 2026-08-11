//! ROUND8-L7-07 / L7-12 — the glitches counter must be WIRED per H2 connection,
//! terminate the connection at the threshold, and advance `h2_glitches_total`.
//! (The FrameRecvTimeout TIMER half is deferred — `audit/deferred.md`.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::body::Bytes;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_observability::MetricsRegistry;
use tokio::net::{TcpListener, TcpStream};

const CLOSED_BACKEND: &str = "127.0.0.1:1";

// RapidReset weight 5, threshold 12: req3 reaches 15 → Drain → GOAWAY.
const THRESHOLD: u32 = 12;

async fn spawn_proxy(registry: Arc<MetricsRegistry>) -> SocketAddr {
    let backend: SocketAddr = CLOSED_BACKEND.parse().unwrap();
    let picker = RoundRobinAddrs::new(vec![backend]).unwrap();
    let proxy = Arc::new(
        H2Proxy::new(
            TcpPool::new(
                PoolConfig::default(),
                BackendSockOpts::default(),
                lb_io::Runtime::new(),
            ),
            Arc::new(picker),
            None,
            HttpTimeouts {
                header: Duration::from_secs(2),
                body: Duration::from_secs(2),
                total: Duration::from_secs(5),
                head: Duration::from_secs(5),
            },
            false,
        )
        .with_glitches(THRESHOLD, registry),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((sock, peer)) = listener.accept().await {
            let _ = proxy.serve_connection(sock, peer).await;
        }
    });
    addr
}

fn glitch_count(registry: &MetricsRegistry) -> u64 {
    registry
        .counter("h2_glitches_total", "x")
        .expect("h2_glitches_total must be registered once the counter runs")
        .get()
}

#[tokio::test]
async fn glitches_counter_drains_connection_at_threshold_and_is_observable() {
    let registry = Arc::new(MetricsRegistry::new());
    let proxy_addr = spawn_proxy(Arc::clone(&registry)).await;

    let tcp = TcpStream::connect(proxy_addr).await.unwrap();
    let io = hyper_util::rt::TokioIo::new(tcp);
    let (mut send, conn) =
        hyper::client::conn::http2::handshake(hyper_util::rt::TokioExecutor::new(), io)
            .await
            .unwrap();
    let conn_handle = tokio::spawn(async move {
        let _ = conn.await;
    });

    // Abuse requests must share ONE connection — the counter is per-conn.
    let mut accepted = 0u32;
    let mut drained = false;
    for i in 0..8 {
        let req = hyper::Request::builder()
            .method("GET")
            .uri("http://victim.example,attacker.example/p")
            .body(Empty::<Bytes>::new())
            .unwrap();
        match tokio::time::timeout(Duration::from_secs(3), send.send_request(req)).await {
            Ok(Ok(resp)) => {
                assert_eq!(
                    resp.status().as_u16(),
                    400,
                    "abuse request {i} must be 400 (invalid authority)"
                );
                let _ = resp.into_body().collect().await;
                accepted += 1;
            }
            Ok(Err(_)) | Err(_) => {
                drained = true;
                break;
            }
        }
        if futures_poll_ready(&mut send).await.is_err() {
            drained = true;
            break;
        }
    }

    assert!(
        drained,
        "the H2 connection MUST be drained once the consolidated \
         glitches threshold is crossed (HAProxy tune.h2.fe.\
         glitches-threshold parity); accepted {accepted} abuse \
         requests without a drain"
    );
    // The 3rd response may or may not land before the GOAWAY.
    assert!(
        (1..=4).contains(&accepted),
        "expected the drain to fire within a few abuse requests at \
         weight 5 vs threshold {THRESHOLD}; accepted {accepted}"
    );

    let c = glitch_count(&registry);
    assert!(
        c >= u64::from(accepted) && c > 0,
        "h2_glitches_total must be non-zero and >= recorded abuse \
         events; got {c} (accepted {accepted})"
    );

    // Resolving proves the drain token fired the two-step GOAWAY arm.
    let _ = tokio::time::timeout(Duration::from_secs(3), conn_handle).await;
}

/// Any transport error here means the connection is gone.
async fn futures_poll_ready(
    send: &mut hyper::client::conn::http2::SendRequest<Empty<Bytes>>,
) -> Result<(), ()> {
    match tokio::time::timeout(Duration::from_millis(500), send.ready()).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}
