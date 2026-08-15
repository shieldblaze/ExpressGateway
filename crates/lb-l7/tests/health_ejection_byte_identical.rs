//! R3 PROOF — while every backend is healthy, adding G5 ejection changes routing NOT AT ALL.
//!
//! Differential: the same request stream through two `H1Proxy` instances over the same three
//! healthy backends, one with `HealthFilteredPicker` + a live `HealthRegistry` and one with the
//! bare `RoundRobinUpstreams`. Both the backend SEQUENCE and the response bytes must match
//! element-wise — sequence, not merely set, because a wrapper that consumed an extra inner pick
//! would preserve the set while rotating the order.
//!
//! A differential test that cannot fail is worthless, so `divergence_is_detectable` pre-ejects one
//! backend and asserts the SAME harness reports a difference. Without that arm, an
//! `assert_eq!(bare, wrapped)` would also pass against a wrapper that does nothing.
//!
//! The mechanical half of this proof (inner picks consumed per outer pick) is asserted directly in
//! `lb_l7::upstream`'s unit tests; this file proves it survives the whole request path.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lb_health::{AdmissionGate, EjectionPolicy, HealthRegistry, UpstreamErrorClass};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{
    BackendInfoPicker, HealthFilteredPicker, RoundRobinUpstreams, UpstreamBackend,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const REQUESTS: usize = 60;

/// Backend that stamps its own id into a response header, so each request's server is identifiable
/// from the wire without depending on body framing.
async fn spawn_id_backend(id: char) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {}
                    }
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nX-Backend-Id: {id}\r\nContent-Length: 2\r\n\r\nok"
                    );
                    if sock.write_all(resp.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

async fn spawn_gateway(
    picker: Arc<dyn BackendInfoPicker>,
    health: Option<Arc<HealthRegistry>>,
) -> SocketAddr {
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    let mut proxy = H1Proxy::with_multi_proto(
        pool,
        picker,
        None,
        HttpTimeouts {
            header: Duration::from_secs(2),
            body: Duration::from_secs(2),
            total: Duration::from_secs(5),
            head: Duration::from_secs(5),
        },
        false,
    );
    if let Some(h) = health {
        proxy = proxy.with_health(h);
    }
    let proxy = Arc::new(proxy);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let p = Arc::clone(&proxy);
            tokio::spawn(async move {
                let _ = p.serve_connection(sock, peer).await;
            });
        }
    });
    addr
}

fn round_robin(addrs: &[SocketAddr]) -> Arc<dyn BackendInfoPicker> {
    Arc::new(
        RoundRobinUpstreams::new(addrs.iter().copied().map(UpstreamBackend::h1).collect()).unwrap(),
    )
}

/// The full response text of one request on a fresh connection.
async fn request_raw(gw: SocketAddr) -> String {
    let mut client = TcpStream::connect(gw).await.unwrap();
    client
        .write_all(b"GET /r HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let mut buf = [0u8; 1024];
    // Two short reads are enough for these tiny responses; the loop just tolerates a split head.
    for _ in 0..2 {
        match tokio::time::timeout(Duration::from_millis(1500), client.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
            Ok(Ok(n)) => {
                out.extend_from_slice(&buf[..n]);
                if out.windows(4).any(|w| w == b"\r\n\r\n") && out.ends_with(b"ok") {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Which backend served, read off the stamped header. `'?'` marks a response that never named one,
/// which would itself make the sequences differ rather than silently comparing equal.
fn served_by(resp: &str) -> char {
    let lower = resp.to_ascii_lowercase();
    lower
        .find("x-backend-id:")
        .and_then(|i| resp.get(i + "x-backend-id:".len()..))
        .and_then(|rest| rest.trim_start().chars().next())
        .unwrap_or('?')
}

async fn sequence(gw: SocketAddr, n: usize) -> Vec<char> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(served_by(&request_raw(gw).await));
    }
    out
}

async fn three_backends() -> Vec<SocketAddr> {
    vec![
        spawn_id_backend('A').await,
        spawn_id_backend('B').await,
        spawn_id_backend('C').await,
    ]
}

/// R3: the health-filtered gateway and the bare gateway produce the SAME backend sequence and the
/// SAME response bytes while everything is healthy.
#[tokio::test]
async fn healthy_routing_is_identical_to_the_unwrapped_build() {
    let addrs = three_backends().await;

    let bare_gw = spawn_gateway(round_robin(&addrs), None).await;
    let health = Arc::new(HealthRegistry::new(EjectionPolicy::default(), &addrs));
    let wrapped_gw = spawn_gateway(
        Arc::new(HealthFilteredPicker::new(
            round_robin(&addrs),
            Arc::clone(&health) as Arc<dyn AdmissionGate>,
            addrs.len(),
        )),
        Some(Arc::clone(&health)),
    )
    .await;

    let bare_seq = sequence(bare_gw, REQUESTS).await;
    let wrapped_seq = sequence(wrapped_gw, REQUESTS).await;

    assert!(
        !bare_seq.contains(&'?'),
        "the rig must identify every server: {bare_seq:?}"
    );
    assert_eq!(
        bare_seq, wrapped_seq,
        "R3: with every backend healthy, ejection must not perturb the routing SEQUENCE"
    );
    assert_eq!(
        health.ejected_count(),
        0,
        "nothing may be ejected on an all-healthy run"
    );
    assert_eq!(health.ejections_total(), 0);

    // Byte-for-byte on the response itself, not just the routing decision.
    let bare_body = request_raw(bare_gw).await;
    let wrapped_body = request_raw(wrapped_gw).await;
    assert_eq!(
        bare_body, wrapped_body,
        "R3: identical requests to identical backends must yield identical bytes"
    );
}

/// NON-VACUITY for the test above. With one backend pre-ejected, the SAME harness must report a
/// difference — otherwise `assert_eq!(bare_seq, wrapped_seq)` proves nothing at all.
#[tokio::test]
async fn divergence_is_detectable() {
    let addrs = three_backends().await;
    let policy = EjectionPolicy::default();

    let bare_gw = spawn_gateway(round_robin(&addrs), None).await;
    let health = Arc::new(HealthRegistry::new(policy, &addrs));

    // Pre-eject the SECOND backend through the real API, before any traffic.
    let ejected = addrs.get(1).copied().unwrap();
    for _ in 0..policy.consecutive_failures {
        health.record(ejected, UpstreamErrorClass::Transport.outcome());
    }
    assert_eq!(health.ejected_count(), 1, "precondition for the control");

    let wrapped_gw = spawn_gateway(
        Arc::new(HealthFilteredPicker::new(
            round_robin(&addrs),
            Arc::clone(&health) as Arc<dyn AdmissionGate>,
            addrs.len(),
        )),
        Some(Arc::clone(&health)),
    )
    .await;

    let bare_seq = sequence(bare_gw, REQUESTS).await;
    let wrapped_seq = sequence(wrapped_gw, REQUESTS).await;

    assert_ne!(
        bare_seq, wrapped_seq,
        "the harness MUST be able to see an ejection — otherwise the R3 proof is vacuous"
    );
    assert!(
        bare_seq.contains(&'B'),
        "the bare build routes to every backend: {bare_seq:?}"
    );
    assert!(
        !wrapped_seq.contains(&'B'),
        "the ejected backend must not appear: {wrapped_seq:?}"
    );
    assert!(wrapped_seq.contains(&'A') && wrapped_seq.contains(&'C'));
}
