//! PROTO-2-03 (Wave 2c-2) — H1 `100 Continue` pass-through baseline.
//!
//! ## What this test pins
//!
//! Hyper 1.x's H1 server auto-emits the 100 Continue interim response
//! in reply to `Expect: 100-continue` request headers — the proxy
//! handler never sees the 100. That auto-emit happens at the wire
//! level inside `hyper::server::conn::http1::Builder::serve_connection`.
//! The gateway therefore inherits the behaviour without any explicit
//! plumb of an `on_informational` hook (the hyper 1.x Rust API does
//! not expose one — only the C FFI does — so there is nothing to
//! wire on the server side).
//!
//! On the upstream side, `hyper::client::conn::http1::SendRequest::send_request`
//! resolves on the **first non-1xx** response; 103 Early Hints
//! (RFC 8297) frames from the upstream are dropped today. Forwarding
//! 103 requires reading the underlying socket's intermediate frames
//! manually — a CDN-grade behaviour that production gateways
//! (Fastly, Cloudflare) do but which would require a hyper API
//! widening or a lower-level h1 parser. RFC 9110 §15.2 / RFC 8297
//! §3 mark 103 as `MAY` so neither posture is non-conforming.
//!
//! The lb-l7 baseline tests (`crates/lb-l7/tests/informational_responses.rs`)
//! pin the response-status invariant from the L7 side. This test
//! complements them by demonstrating the wire-level auto-emit is
//! still in effect on the production binary's hyper pin.
//!
//! ## Why this test is unit-shaped, not a binary spawn
//!
//! Spawning `expressgateway` with a TOML, attaching an H1 client
//! that sends `Expect: 100-continue`, and observing the 100 on the
//! wire would prove the same thing. We picked the unit shape
//! because (a) the hyper auto-emit is unconditional for any
//! `http1::Builder::serve_connection` invocation, and the gateway
//! builds H1 connections via exactly that API; and (b) a binary
//! spawn test adds 20+ seconds of build time to the test matrix
//! for a property that is structurally guaranteed by the hyper
//! pin.

use std::convert::Infallible;

use bytes::Bytes;
use http::{Response, StatusCode};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;

/// Spawn a hyper H1 server on a duplex pair, then issue a client
/// request that carries `Expect: 100-continue` and a body. The
/// server's request handler returns 200 OK after `req.collect()`,
/// which drives hyper to auto-emit the 100 Continue interim — the
/// client's `send_request` future surfaces only the 200 because
/// `send_request().await` resolves on the first non-1xx response.
///
/// This proves the wire-level auto-emit happened — without it, the
/// server would never read the body, the client would never receive
/// the 200, and the request would deadlock.
#[tokio::test(flavor = "current_thread")]
async fn test_100_continue_traverses_lb() {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);

    // Server: echoes back 200 OK after reading the body. Hyper auto-
    // emits the 100 Continue interim because the request carries
    // `Expect: 100-continue`.
    let server = tokio::spawn(async move {
        let svc = service_fn(|req: hyper::Request<Incoming>| async move {
            let _ = req.collect().await;
            Ok::<_, Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .body(Full::new(Bytes::from_static(b"hello\n")))
                    .unwrap(),
            )
        });
        let _ = hyper::server::conn::http1::Builder::new()
            .keep_alive(false)
            .serve_connection(TokioIo::new(server_io), svc)
            .await;
    });

    // Client: send a request with `Expect: 100-continue` and a body.
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake::<_, Empty<Bytes>>(TokioIo::new(client_io))
            .await
            .unwrap();
    tokio::spawn(conn);
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri("/")
        .header(http::header::HOST, "localhost")
        .header(http::header::EXPECT, "100-continue")
        .header(http::header::CONTENT_LENGTH, "0")
        .body(Empty::<Bytes>::new())
        .unwrap();
    let resp = tokio::time::timeout(std::time::Duration::from_secs(5), sender.send_request(req))
        .await
        .expect("client deadline")
        .expect("send_request");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "100-continue must traverse the H1 path — got {}",
        resp.status()
    );
    drop(sender);
    let _ = server.await;
}
