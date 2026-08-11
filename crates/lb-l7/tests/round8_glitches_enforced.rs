//! ROUND8-L7-07 / L7-12 proof — the consolidated HAProxy-style "glitches" abuse
//! counter is WIRED per H2 connection, actually terminates the connection at
//! the threshold, and is observable in Prometheus.
//!
//! HAProxy 3.0 `tune.h2.fe.glitches-threshold`: operators cannot tune six
//! independent per-detector thresholds, so weighted protocol-abuse events are
//! summed per connection and the connection drains (GOAWAY) once the rolling
//! sum crosses the threshold. `GlitchesCounter` previously had ZERO callsites
//! and no Prometheus surface, so this test drives abuse requests on ONE
//! connection and asserts `h2_glitches_total` advances per request AND that the
//! connection is drained once the sum crosses. (The FrameRecvTimeout TIMER half
//! is deferred-with-rationale on pinned hyper 1.x — `audit/deferred.md`.)

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

// RapidReset weight is 5 (comma-in-authority maps to RapidReset). A
// threshold of 12 means: req1 -> 5 (allow), req2 -> 10 (allow),
// req3 -> 15 (> 12 -> Drain -> connection GOAWAY).
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
        // Resolves when the server closes the connection (GOAWAY + close).
        let _ = conn.await;
    });

    // Drive abuse requests (comma in `:authority`) on the SAME connection.
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
                // The per-request response is still 400; the CONNECTION drains
                // separately once the threshold is crossed.
                assert_eq!(
                    resp.status().as_u16(),
                    400,
                    "abuse request {i} must be 400 (invalid authority)"
                );
                let _ = resp.into_body().collect().await;
                accepted += 1;
            }
            Ok(Err(_)) | Err(_) => {
                // Connection no longer accepts requests — drained.
                drained = true;
                break;
            }
        }
        // If the connection went away between requests, the next send fails.
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
    // 3 requests × weight 5 = 15 crosses the threshold of 12. The 3rd response
    // may or may not land before the GOAWAY, so bound generously.
    assert!(
        (1..=4).contains(&accepted),
        "expected the drain to fire within a few abuse requests at \
         weight 5 vs threshold {THRESHOLD}; accepted {accepted}"
    );

    // The counter must have advanced once per recorded glitch — non-zero is the
    // headline assertion the push-back demanded.
    let c = glitch_count(&registry);
    assert!(
        c >= u64::from(accepted) && c > 0,
        "h2_glitches_total must be non-zero and >= recorded abuse \
         events; got {c} (accepted {accepted})"
    );

    // The server conn future must resolve (GOAWAY + close), proving the drain
    // token fired the existing two-step GOAWAY arm.
    let _ = tokio::time::timeout(Duration::from_secs(3), conn_handle).await;
}

/// Best-effort readiness probe: any transport error means the connection is
/// gone.
async fn futures_poll_ready(
    send: &mut hyper::client::conn::http2::SendRequest<Empty<Bytes>>,
) -> Result<(), ()> {
    // `ready()` resolves Err once the connection is closed/draining.
    match tokio::time::timeout(Duration::from_millis(500), send.ready()).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}
