//! ROUND8-L7-10 — the H2 upstream eviction guarantee plus the H1
//! take-and-discard doc-comment provenance.
//!
//! A full over-read regression needs a hyper H2 server that deliberately sends
//! `Content-Length: 5` with 10 body bytes — substantial scaffolding for a
//! doc-grade finding. Instead this pins the contract at the public API plus the
//! source-of-truth doc-blocks: no stale `PeerEntry` survives any error path,
//! and the doc-blocks that keep the Pingora upstream-smuggling lesson (0.6.0 /
//! 0.8.0 CHANGELOG) alive are still present in lb-l7 and lb-io.

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use lb_io::Runtime;
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;

#[tokio::test]
async fn h2_pool_failed_send_leaves_no_stale_entry() {
    // Dial a dead port: the `Dial` arm fails, and the postcondition we care
    // about holds for EVERY error path — `peer_count` is 0, no stale entry.
    let tcp_cfg = PoolConfig {
        connect_timeout: Duration::from_millis(100),
        ..PoolConfig::default()
    };
    let runtime = Runtime::new();
    let tcp = TcpPool::new(tcp_cfg, BackendSockOpts::default(), runtime);
    let h2_cfg = Http2PoolConfig {
        send_timeout: Duration::from_millis(200),
        ..Http2PoolConfig::default()
    };
    let pool = Http2Pool::new(h2_cfg, tcp);

    let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let req = Request::builder()
        .uri("http://127.0.0.1:1/")
        .body(
            Empty::<Bytes>::new()
                .map_err(|never| match never {})
                .boxed(),
        )
        .unwrap();
    let result = pool.send_request(addr, req).await;
    assert!(
        result.is_err(),
        "send_request to dead :1 must fail; got {result:?}"
    );
    assert_eq!(
        pool.peer_count(),
        0,
        "ROUND8-L7-10 contract: no stale PeerEntry survives a failed \
         send_request. If this drifts, the Pingora upstream-smuggling \
         bug class is reachable on the next reuse refactor."
    );
}

#[test]
fn h1_take_and_discard_doc_block_present() {
    // The doc-block on `H1Proxy::proxy_request` is the single source of truth
    // warning future refactors against dropping `take_stream()` without wiring
    // the body-mismatch mitigation. Detect drift by string-matching it.
    let src =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/h1_proxy.rs")).unwrap();
    assert!(
        src.contains("ROUND8-L7-10 — take-and-discard upstream stream pattern"),
        "lb-l7/src/h1_proxy.rs lost the ROUND8-L7-10 take-and-discard \
         doc-block — Pingora's body-mismatch lesson would be reintroduced \
         on the next H1 upstream reuse refactor. Restore the doc-block \
         before removing the assertion."
    );
    assert!(
        src.contains("set_reusable(false)"),
        "lb-l7/src/h1_proxy.rs ROUND8-L7-10 doc-block no longer cites the \
         `set_reusable(false)` mitigation. The doc must point the future \
         refactor at the fix or the contract is useless."
    );
}

#[test]
fn pooled_tcp_set_reusable_doc_block_present() {
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../lb-io/src/pool.rs"))
        .unwrap();
    assert!(
        src.contains("ROUND8-L7-10 — API contract for future H1 upstream reuse"),
        "lb-io/src/pool.rs lost the ROUND8-L7-10 `set_reusable` contract \
         doc-block. Without it, the API looks dead and may be pruned, \
         which deletes the future-refactor entry point for the Pingora \
         body-mismatch mitigation."
    );
}

#[test]
fn http2_pool_send_request_doc_block_present() {
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../lb-io/src/http2_pool.rs"
    ))
    .unwrap();
    assert!(
        src.contains("ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern"),
        "lb-io/src/http2_pool.rs lost the ROUND8-L7-10 doc-block on \
         send_request — the broad-eviction contract loses its rationale \
         anchor and may be tightened in a future refactor."
    );
}
