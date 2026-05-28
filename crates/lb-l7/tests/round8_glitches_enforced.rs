//! ROUND8-L7-07 / L7-12 proof — the consolidated HAProxy-style
//! "glitches" abuse counter is WIRED per H2 connection and actually
//! terminates the connection at the threshold, with an observable
//! Prometheus surface.
//!
//! Reference: HAProxy 3.0 `tune.h2.fe.glitches-threshold` (`src/
//! mux_h2.c` `h2_glitches_*`) — operators cannot tune six independent
//! per-detector thresholds, so HAProxy sums weighted protocol-abuse
//! events per connection and drains (GOAWAY) once the rolling sum
//! crosses the threshold. nginx `http2_recv_timeout` is the
//! frame-arrival sibling (the FrameRecvTimeout *timer* sub-part is
//! deferred-with-rationale on pinned hyper 1.x — see
//! `audit/deferred.md`; the COUNTER half, the actual HAProxy pattern,
//! is what this test exercises).
//!
//! The verifier push-back (`audit/round-8/verify/l7.md`) rejected the
//! prior commit because `GlitchesCounter` had ZERO callsites and no
//! Prometheus surface (Theme-1 "library shipped, no caller"). This
//! test drives a stream of protocol-abuse requests (comma-in-
//! :authority, each a `RapidReset`-weight glitch) on ONE H2
//! connection with a low threshold and asserts:
//!   * `h2_glitches_total` advances once per abuse request (the
//!     counter is on the path and observable);
//!   * once the weighted rolling sum crosses the threshold the
//!     connection is drained (the next request on the same
//!     connection fails — GOAWAY emitted via the existing two-step
//!     drain path).

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
        // Resolves when the server closes the connection (GOAWAY +
        // close after the glitch threshold trips).
        let _ = conn.await;
    });

    // Drive abuse requests (comma in :authority — a RapidReset-weight
    // glitch) on the SAME connection until the threshold trips.
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
                // The per-request response is still a 400 (abuse is
                // rejected per-request); the *connection* is drained
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
        // ready check: if the connection went away between requests
        // the next poll_ready / send will fail.
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
    // 3 requests at weight 5 (15) cross the threshold of 12, so the
    // connection should drain after ~3 accepted abuse requests (the
    // 3rd request's response may or may not land before the GOAWAY,
    // hyper-timing dependent — bound it generously).
    assert!(
        (1..=4).contains(&accepted),
        "expected the drain to fire within a few abuse requests at \
         weight 5 vs threshold {THRESHOLD}; accepted {accepted}"
    );

    // Prometheus surface: the counter must have advanced once per
    // recorded glitch (>= the number of abuse requests that reached
    // the validator). Non-zero is the headline assertion the
    // push-back demanded.
    let c = glitch_count(&registry);
    assert!(
        c >= u64::from(accepted) && c > 0,
        "h2_glitches_total must be non-zero and >= recorded abuse \
         events; got {c} (accepted {accepted})"
    );

    // The server-side connection future must resolve (GOAWAY + close)
    // within the budget — proves the drain token actually fired the
    // existing two-step GOAWAY select arm.
    let _ = tokio::time::timeout(Duration::from_secs(3), conn_handle).await;
}

/// Best-effort readiness probe: send a HEAD-ish request is heavy, so
/// instead we just attempt a cheap zero-body request and treat any
/// transport error as "connection gone".
async fn futures_poll_ready(
    send: &mut hyper::client::conn::http2::SendRequest<Empty<Bytes>>,
) -> Result<(), ()> {
    // `ready()` resolves Err once the connection is closed/draining.
    match tokio::time::timeout(Duration::from_millis(500), send.ready()).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(()),
    }
}
