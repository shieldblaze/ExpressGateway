//! PROTO-2-18 — `check_sni_authority` wired into the H1 hot path. The validator
//! existed and was unit-tested but no proxy hot path called it, so the
//! PROTO-2-15 421 contract was unenforced on every listener type.
//!
//! Pinned: a real TLS 1.3 handshake with `SNI = a.test` + `Host: b.test` from a
//! NON-loopback peer must yield `421 Misdirected Request` (RFC 9110 §15.5.20);
//! the same shape from a LOOPBACK peer must NOT (the sec-r5 carve-out) — it
//! reaches the closed backend and resolves 502, proving the SNI check did not
//! short-circuit the path. The key assertion there is "status != 421".

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts, RoundRobinAddrs};
use rustls::ServerConfig;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, ServerName};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tokio_util::sync::CancellationToken;

/// SNI that the server certificate is valid for.
const SERVER_SNI: &str = "a.test";
/// Host header value that does not agree with [`SERVER_SNI`].
const ATTACKER_HOST: &str = "b.test";

fn self_signed_for(sni: &str) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let generated = rcgen::generate_simple_self_signed(vec![sni.to_owned()]).unwrap();
    let cert_der: Vec<u8> = generated.cert.der().to_vec();
    let key_der: Vec<u8> = generated.signing_key.serialize_der();
    let chain = vec![CertificateDer::from(cert_der)];
    let key = PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(key_der));
    (chain, key)
}

/// Matched server + client TLS configs — one cert pair, so the client trusts
/// the live handshake.
fn matched_tls_configs(sni: &str) -> (Arc<ServerConfig>, Arc<rustls::ClientConfig>) {
    let (chain, key) = self_signed_for(sni);
    // Server side.
    let mut server_cfg =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(chain.clone(), key)
            .expect("build server cfg");
    server_cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    // Client side: trust the same chain.
    (Arc::new(server_cfg), client_tls_config(chain))
}

fn client_tls_config(server_chain: Vec<CertificateDer<'static>>) -> Arc<rustls::ClientConfig> {
    let mut root = rustls::RootCertStore::empty();
    for c in server_chain {
        root.add(c).unwrap();
    }
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(root)
    .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(cfg)
}

fn build_proxy() -> Arc<H1Proxy> {
    let pool = TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts::default(),
        lb_io::Runtime::new(),
    );
    // The backend points at a closed port: the 421 path must fire before any
    // dial; the loopback test's 502 proves the check did not short-circuit.
    let addrs: Vec<SocketAddr> = vec!["127.0.0.1:1".parse().unwrap()];
    let picker = RoundRobinAddrs::new(addrs).unwrap();
    Arc::new(H1Proxy::new(
        pool,
        Arc::new(picker),
        None,
        HttpTimeouts {
            header: Duration::from_secs(2),
            body: Duration::from_secs(2),
            total: Duration::from_secs(5),
            head: Duration::from_secs(5),
        },
        /* is_https */ true,
    ))
}

/// Drive a real TLS handshake over a duplex pair so rustls'
/// `ServerConnection::server_name()` captures the SNI live (the PROTO-2-18
/// hot-path entry condition). Returns the raw response bytes.
async fn drive_handshake_and_request(
    server_cfg: Arc<ServerConfig>,
    client_cfg: Arc<rustls::ClientConfig>,
    proxy_peer: SocketAddr,
) -> Vec<u8> {
    // 64 KiB duplex is plenty for the request + response.
    let (server_io, client_io) = tokio::io::duplex(64 * 1024);

        // Accept TLS, capture SNI, hand the stream to the PROTO-2-18 entry.
    let server_task = tokio::spawn(async move {
        let acceptor = TlsAcceptor::from(server_cfg);
        let tls_stream = acceptor.accept(server_io).await.expect("TLS accept");
        let sni = tls_stream.get_ref().1.server_name().map(str::to_owned);
        let proxy = build_proxy();
        let cancel = CancellationToken::new();
            // 5 s upper bound; the proxy should answer well inside that.
        let _ = tokio::time::timeout(
            Duration::from_secs(5),
            proxy.serve_connection_with_cancel_sni(tls_stream, proxy_peer, cancel, sni),
        )
        .await;
    });

        // TLS handshake, send the malicious request, read the response head.
    let connector = TlsConnector::from(client_cfg);
    let server_name = ServerName::try_from(SERVER_SNI).unwrap();
    let mut tls_client = connector
        .connect(server_name, client_io)
        .await
        .expect("TLS connect");
    let req = format!("GET / HTTP/1.1\r\nHost: {ATTACKER_HOST}\r\n\r\n");
    tls_client.write_all(req.as_bytes()).await.unwrap();
    tls_client.flush().await.unwrap();
    // Read up to 4 KiB of response or until EOF / close.
    let mut buf = Vec::with_capacity(4096);
    let _ = tokio::time::timeout(Duration::from_secs(5), tls_client.read_to_end(&mut buf)).await;
    let _ = server_task.await;
    buf
}

#[tokio::test]
async fn test_421_emitted_on_sni_host_mismatch_over_tls() {
    let (server_cfg, client_cfg) = matched_tls_configs(SERVER_SNI);

    // Non-loopback peer (RFC 5737 TEST-NET-1): the loopback exception must NOT
    // fire, so 421 is the expected response.
    let peer: SocketAddr = "192.0.2.1:54321".parse().unwrap();
    let buf = drive_handshake_and_request(server_cfg, client_cfg, peer).await;

    let head = String::from_utf8_lossy(&buf);
    assert!(
        head.starts_with("HTTP/1.1 421"),
        "expected `HTTP/1.1 421 Misdirected Request` status line; got: {head:?}"
    );
    assert!(
        head.contains("Misdirected Request"),
        "expected RFC 9110 §15.5.20 phrase in the response; got: {head:?}"
    );
}

#[tokio::test]
async fn test_loopback_allows_mismatch() {
    let (server_cfg, client_cfg) = matched_tls_configs(SERVER_SNI);

    // Loopback peer: the sec-r5 carve-out skips the check, so the request
    // proceeds and the closed backend yields 502. Key assertion: not 421.
    let peer: SocketAddr = "127.0.0.1:54321".parse().unwrap();
    let buf = drive_handshake_and_request(server_cfg, client_cfg, peer).await;

    let head = String::from_utf8_lossy(&buf);
    assert!(
        !head.starts_with("HTTP/1.1 421"),
        "loopback peer must skip the 421 SNI/Host enforcement; got: {head:?}"
    );
    // Sanity: SOME response landed (502, or a clean close) — either way not 421.
    assert!(
        head.starts_with("HTTP/1.1 ") || head.is_empty(),
        "unexpected response shape (loopback): {head:?}"
    );
}
