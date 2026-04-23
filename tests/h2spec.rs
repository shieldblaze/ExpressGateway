//! h2spec conformance test (Pillar 3b.3b-2).
//!
//! Spawns the gateway's H2s listener on an ephemeral port, then invokes
//! the `h2spec` binary (<https://github.com/summerwind/h2spec>) as a
//! subprocess. Asserts exit code 0.
//!
//! The test is a graceful **skip** — not `#[ignore]` — when `h2spec` is
//! not on `PATH`. A `#[ignore]` would hide the test from the default
//! `cargo test` run and silently let CI drift. Skipping with an
//! `eprintln!` lets the test always execute and report its decision.
//!
//! Install instructions live in `DEPLOYMENT.md`.

use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use lb_l7::h2_proxy::H2Proxy;
use lb_security::{TicketRotator, build_server_config};
use parking_lot::Mutex;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1 as srv_h1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lb_io::Runtime;
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use std::convert::Infallible;

const SAN_HOST: &str = "expressgateway.test";

fn h2spec_on_path() -> Option<std::path::PathBuf> {
    let out = Command::new("which").arg("h2spec").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    if path.is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(path))
    }
}

fn make_cert_for(san: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![san.to_string()]).unwrap();
    let cert_der = generated.cert.der().to_vec();
    let key_der = generated.key_pair.serialize_der();
    (
        vec![CertificateDer::from(cert_der)],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
    )
}

async fn spawn_static_backend() -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let local = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let svc = service_fn(|_req: Request<Incoming>| async move {
                    Ok::<_, Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from_static(b"ok")))
                            .unwrap(),
                    )
                });
                let _ = srv_h1::Builder::new()
                    .serve_connection(TokioIo::new(sock), svc)
                    .await;
            });
        }
    });
    local
}

#[tokio::test]
async fn h2spec_generic_conformance() {
    let Some(h2spec_bin) = h2spec_on_path() else {
        eprintln!(
            "h2spec not installed; skipping conformance test. \
             Install per DEPLOYMENT.md to enable."
        );
        return;
    };

    // Bring up a minimal H2s listener.
    let backend_addr = spawn_static_backend().await;
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: false,
            rcvbuf: None,
            sndbuf: None,
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    );
    let picker = Arc::new(RoundRobinAddrs::new(vec![backend_addr]).unwrap());
    let h1_proxy = Arc::new(H1Proxy::new(
        pool.clone(),
        Arc::clone(&picker) as _,
        None,
        HttpTimeouts::default(),
        true,
    ));
    let h2_proxy = Arc::new(H2Proxy::new(
        pool,
        picker as _,
        None,
        HttpTimeouts::default(),
        true,
    ));

    let (cert_chain, key) = make_cert_for(SAN_HOST);
    let rot = TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
    let rot_arc = Arc::new(Mutex::new(rot));
    let alpn: &[&[u8]] = &[b"h2", b"http/1.1"];
    let server_cfg = build_server_config(rot_arc, cert_chain, key, alpn).unwrap();
    let acceptor = TlsAcceptor::from(server_cfg);

    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        loop {
            let Ok((sock, peer)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            let h1 = Arc::clone(&h1_proxy);
            let h2 = Arc::clone(&h2_proxy);
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(sock).await else {
                    return;
                };
                let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
                if alpn.as_deref() == Some(b"h2".as_ref()) {
                    let _ = h2.serve_connection(tls, peer).await;
                } else {
                    let _ = h1.serve_connection(tls, peer).await;
                }
            });
        }
    });

    // Invoke h2spec. `-t` runs TLS mode; `-k` skips cert validation so
    // the self-signed cert is accepted without us having to mount the
    // root CA into the child process. Timeout keeps CI bounded.
    let output = Command::new(&h2spec_bin)
        .args([
            "-h",
            "127.0.0.1",
            "-p",
            &port.to_string(),
            "-t",
            "-k",
            "--timeout",
            "2",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let Ok(out) = output else {
        eprintln!("h2spec spawn failed; skipping");
        return;
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        eprintln!("h2spec stdout:\n{stdout}");
        eprintln!("h2spec stderr:\n{stderr}");
        panic!(
            "h2spec failed with exit status: {:?}. See stderr above.",
            out.status.code()
        );
    }
    eprintln!("h2spec passed ({} bytes stdout)", stdout.len());
}
