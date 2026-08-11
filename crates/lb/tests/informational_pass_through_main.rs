//! PROTO-2-03 (Wave 2c-2) — H1 `100 Continue` pass-through baseline.

use std::convert::Infallible;

use bytes::Bytes;
use http::{Response, StatusCode};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;

/// Spawn a hyper H1 server on a duplex pair, then issue a client request that carries `Expect: 100-continue` and a body.
#[tokio::test(flavor = "current_thread")]
async fn test_100_continue_traverses_lb() {
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);

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
