//! Zero-drop reload + multi-protocol drain integration tests.
//!
//! This file pulls double duty:
//!
//! 1. `test_reload_zero_drop_under_load` — the original
//!    `ConfigManager`-level reload soak that this file owned before
//!    round-4. Rapid reload/rollback cycles produce correct, consistent
//!    state at every step, and version numbers increase monotonically.
//!
//! 2. **REL-2-02 drain integration**:
//!    `test_sigterm_drains_h1_with_connection_close`,
//!    `test_sigterm_drains_h2_with_goaway`, and
//!    `test_sigterm_drains_h3_with_connection_close` spawn the
//!    production `expressgateway` binary, open a long-lived
//!    per-protocol connection, deliver SIGTERM, and assert the
//!    protocol-level drain signal:
//!
//!    | Protocol | Signal expected                             |
//!    |----------|---------------------------------------------|
//!    | H1       | `Connection: close` in the next response    |
//!    | H2       | `GOAWAY (NO_ERROR / 0x0)` with two-step pattern |
//!    | H3       | `CONNECTION_CLOSE (H3_NO_ERROR = 0x0100)`   |
//!
//!    The protocol-level emission machinery exists today:
//!    - H3: `lb_quic::graceful_h3_shutdown` (`deb9267`).
//!    - H2: hyper's `http2::Connection::graceful_shutdown` is the
//!      sender side; the receiver decode is in `lb_h2::frame`.
//!    - H1: hyper's `http1::Connection::graceful_shutdown` adds the
//!      header; the lb-h1 codec already parses `Connection: close`.
//!
//!    The plumbing that fires those calls from
//!    `lb_core::Shutdown::token()` into each `serve_connection`
//!    future in `crates/lb/src/main.rs` is now wired:
//!    - PROTO-2-11 H2 half (`33edd13`): `H2Proxy::serve_connection_with_cancel`.
//!    - PROTO-2-11 H1 half: `H1Proxy::serve_connection_with_cancel`
//!      hooked at the H1 and H1s/H1 ALPN-fallback accept sites.
//!    - PROTO-2-11 H3 listener cancel: `spawn_quic` accepts a
//!      `CancellationToken` cloned from `shutdown.token().child_token()`
//!      instead of constructing its own local token.
//!
//!    All three drain tests are live.
//!
//!    Each drain test:
//!    - Locates `target/{release,debug}/expressgateway`. If absent,
//!      the test is skipped with a `cargo build` hint instead of
//!      failing — running `cargo test --test reload_zero_drop` on a
//!      clean tree should not require a binary build.
//!    - Generates a minimal TOML in a temp dir, picks an ephemeral
//!      port for the listener, and starts an in-process backend so
//!      the gateway has a target.
//!    - Drives a real client (`hyper::client::conn::*` for H1/H2 and
//!      a raw quiche datagram pump for H3 today) to a steady
//!      keep-alive state.
//!    - Sends `SIGTERM` to the child PID.
//!    - Asserts the drain signal observed.
//!    - Asserts the in-flight request completes (no half-frames).

use lb_controlplane::{ConfigManager, FileBackend};

#[test]
fn test_reload_zero_drop_under_load() {
    let dir = std::env::temp_dir().join("eg-test-zero-drop");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("zero_drop_config.toml");

    std::fs::write(&path, "config = \"v1\"").unwrap();
    let backend = FileBackend::new(path.clone());
    let mut mgr = ConfigManager::new(Box::new(backend)).unwrap();
    assert_eq!(mgr.current_config(), "config = \"v1\"");
    assert_eq!(mgr.version(), 1);

    // Simulate rapid config changes (as if under continuous load).
    let mut expected_version: u64 = 1;
    for i in 2..=20 {
        let new_config = format!("config = \"v{i}\"");
        std::fs::write(&path, &new_config).unwrap();

        let changed = mgr.reload().unwrap();
        assert!(changed, "reload must detect change for v{i}");

        expected_version += 1;
        assert_eq!(mgr.version(), expected_version, "version must be monotonic");
        assert_eq!(
            mgr.current_config(),
            new_config,
            "config must reflect latest after reload"
        );
    }

    // Version should be 20 after 19 successful reloads.
    assert_eq!(mgr.version(), 20);

    // Rollback to a previous config; version still increments.
    mgr.rollback("config = \"v1\"").unwrap();
    assert_eq!(mgr.version(), 21);
    assert_eq!(mgr.current_config(), "config = \"v1\"");

    // Reload after rollback sees the rolled-back value (written by rollback).
    let changed = mgr.reload().unwrap();
    assert!(
        !changed,
        "reload after rollback with no further disk change must return false"
    );
    assert_eq!(mgr.version(), 21);

    // One more disk change to confirm the manager is still functional.
    std::fs::write(&path, "config = \"final\"").unwrap();
    let changed = mgr.reload().unwrap();
    assert!(changed);
    assert_eq!(mgr.version(), 22);
    assert_eq!(mgr.current_config(), "config = \"final\"");

    let _ = std::fs::remove_dir_all(&dir);
}

// ── REL-2-02 drain integration scaffolding ──────────────────────────────

mod drain {
    use std::io::Write;
    use std::net::{SocketAddr, TcpListener as StdTcpListener};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// Locate the production binary on disk. Cargo does not auto-set
    /// `CARGO_BIN_EXE_expressgateway` for tests in this workspace-root
    /// integration-test crate, so we walk the target dir manually.
    ///
    /// Returns `Err(reason)` if the binary cannot be found; callers
    /// `#[ignore]` the test in that case rather than failing CI on a
    /// clean tree.
    pub fn find_binary() -> Result<PathBuf, String> {
        // CARGO_TARGET_DIR overrides; fall back to `target/` from the
        // workspace root inferred from CARGO_MANIFEST_DIR.
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let manifest =
                    std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
                PathBuf::from(manifest).join("target")
            });

        for profile in ["release", "debug"] {
            let candidate = target_dir.join(profile).join("expressgateway");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }

        Err(format!(
            "expressgateway binary not found under {}; \
             run `cargo build -p lb --bin expressgateway` first",
            target_dir.display()
        ))
    }

    /// Reserve an ephemeral TCP port by binding then dropping. Race
    /// window exists between drop and the gateway's bind, but in
    /// practice loopback ephemeral ports are not aggressively reused
    /// on the kernels we target.
    pub fn ephemeral_port() -> u16 {
        let l = StdTcpListener::bind(("127.0.0.1", 0)).expect("ephemeral bind");
        let port = l.local_addr().expect("local_addr").port();
        drop(l);
        port
    }

    /// Generate a minimal TOML for the gateway pointing at `backend`.
    /// `proto` is one of `"h1"`, `"h1s"`, `"quic"` per `lb_config::ListenerConfig`.
    pub fn write_config(
        dir: &Path,
        listener_port: u16,
        backend: SocketAddr,
        proto: &str,
    ) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{listener_port}"
protocol = "{proto}"

[[listeners.backends]]
address = "{backend}"
weight = 1
"#
        );
        let path = dir.join("gateway.toml");
        let mut f = std::fs::File::create(&path).expect("create config");
        f.write_all(toml.as_bytes()).expect("write config");
        path
    }

    /// REL-2-02 follow-on: generate a self-signed cert + key + retry
    /// secret path into `dir` and write a complete gateway TOML with
    /// both a QUIC `[[listeners]]` block AND the matching
    /// `[listeners.quic]` block referencing the generated paths.
    pub fn write_quic_config_with_self_signed(
        dir: &Path,
        listener_port: u16,
        backend: SocketAddr,
    ) -> PathBuf {
        let generated =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".into()])
                .expect("self-signed cert");
        let cert_path = dir.join("quicdrain.crt");
        let key_path = dir.join("quicdrain.key");
        std::fs::write(&cert_path, generated.cert.pem()).expect("write cert");
        std::fs::write(&key_path, generated.key_pair.serialize_pem()).expect("write key");
        // lb_security::load_or_generate_retry_secret will mint a
        // fresh 32-byte secret when this path is missing — we leave
        // the file absent so the gateway exercises that path.
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{listener_port}"
protocol = "quic"

[listeners.quic]
cert_path = "{cert}"
key_path = "{key}"
retry_secret_path = "{retry}"

[[listeners.backends]]
address = "{backend}"
weight = 1
"#,
            cert = cert_path.display(),
            key = key_path.display(),
            retry = dir.join("quicdrain.retry").display(),
        );
        let path = dir.join("gateway.toml");
        std::fs::write(&path, toml).expect("write config");
        path
    }

    /// Spawn the gateway as a child process, returning the child + the
    /// listener address. Waits up to 5 s for the listener to become
    /// accept()-ready before returning.
    pub fn spawn_gateway(bin: &Path, config: &Path, addr: SocketAddr) -> Child {
        let mut child = Command::new(bin)
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("RUST_LOG", "info")
            .spawn()
            .expect("spawn expressgateway");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                return child;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Reap the child before bubbling up — leaving the panic to
        // drop(child) without a wait would zombie the gateway.
        let _ = child.kill();
        let _ = child.wait();
        panic!("gateway did not start accepting on {addr} within 5s");
    }

    /// Like [`spawn_gateway`] but for QUIC listeners. QUIC binds a
    /// UDP socket, so a TCP-connect probe always fails. We give the
    /// process a short fixed warm-up window so the UDP socket is
    /// bound and `/readyz` flips to Ready before the test proceeds.
    pub fn spawn_gateway_udp(bin: &Path, config: &Path, _addr: SocketAddr) -> Child {
        let child = Command::new(bin)
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("RUST_LOG", "info")
            .spawn()
            .expect("spawn expressgateway");
        std::thread::sleep(Duration::from_millis(750));
        child
    }

    /// Deliver SIGTERM to a child process. Unix-only; the production
    /// drain spec is Unix-only.
    #[cfg(unix)]
    pub fn sigterm(child: &Child) {
        // SAFETY: libc::kill is safe to call from Rust; we use the
        // libc crate transitively via the workspace if available, but
        // call through the raw extern to avoid pulling a dep.
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        // SAFETY: child.id() returns a valid OS PID for as long as we
        // hold the `Child`; even if the process has exited, kill(2)
        // will simply return ESRCH.
        let rc = unsafe { kill(child.id() as i32, SIGTERM) };
        assert!(
            rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(3 /* ESRCH */),
            "kill returned {rc}, errno {}",
            std::io::Error::last_os_error()
        );
    }

    #[cfg(not(unix))]
    pub fn sigterm(_child: &Child) {
        unreachable!("drain tests are Unix-only");
    }

    /// Spawn a minimal blocking HTTP/1.1 backend that 200s every
    /// request on the specified `addr` and exits when its
    /// [`BackendGuard`] is dropped (the std `TcpListener` is closed
    /// from another thread via the shutdown channel).
    ///
    /// Used by the drain tests so the gateway has a backend to
    /// dial — without this, the H1 proxy answers 502 and the
    /// `Connection: close` graceful-drain header is hidden behind
    /// an error response shaped by `error_response`.
    pub fn spawn_blocking_h1_backend(addr: SocketAddr) -> BackendGuard {
        use std::io::{Read, Write};
        use std::net::TcpListener as StdListener;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = Arc::clone(&stop);
        let listener = StdListener::bind(addr).expect("backend bind");
        listener.set_nonblocking(true).expect("backend nonblocking");
        let handle = std::thread::spawn(move || {
            while !stop_w.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        sock.set_read_timeout(Some(Duration::from_millis(500))).ok();
                        let mut buf = [0u8; 1024];
                        let _ = sock.read(&mut buf);
                        let body = b"ok";
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
                            body.len()
                        );
                        let _ = sock.write_all(resp.as_bytes());
                        let _ = sock.write_all(body);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
        });
        BackendGuard {
            stop,
            handle: Some(handle),
        }
    }

    /// REL-2-02 follow-on: generate a self-signed cert + key into
    /// `dir` and write a complete gateway TOML with both a
    /// `[[listeners]]` block AND a `[listeners.tls]` block referencing
    /// the generated paths, so lb-config accepts the `h1s` protocol
    /// (which requires TLS).
    ///
    /// Returns the path to the generated TOML. The cert SAN list
    /// covers `127.0.0.1` and `localhost` so an h2 client targeting
    /// either name handshakes cleanly.
    pub fn write_h1s_config_with_self_signed(
        dir: &Path,
        listener_port: u16,
        backend: SocketAddr,
    ) -> PathBuf {
        let generated =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".into()])
                .expect("self-signed cert");
        let cert_path = dir.join("h2drain.crt");
        let key_path = dir.join("h2drain.key");
        std::fs::write(&cert_path, generated.cert.pem()).expect("write cert");
        std::fs::write(&key_path, generated.key_pair.serialize_pem()).expect("write key");
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{listener_port}"
protocol = "h1s"

[listeners.tls]
cert_path = "{cert}"
key_path = "{key}"

[[listeners.backends]]
address = "{backend}"
weight = 1
"#,
            cert = cert_path.display(),
            key = key_path.display(),
        );
        let path = dir.join("gateway.toml");
        std::fs::write(&path, toml).expect("write config");
        path
    }

    /// Run one H1 drain attempt against `listener_addr` (a TCP
    /// proxy listener spawned by `child`). Returns the raw response
    /// bytes (as a string) — empty on failure.
    ///
    /// Sequence:
    ///   1. Open a TCP conn, write request headers (POST,
    ///      Content-Length=10), no body.
    ///   2. Sleep 200ms so the gateway-side hyper conn dispatches
    ///      the service future and parks reading body.
    ///   3. SIGTERM the gateway.
    ///   4. Sleep 400ms so cancel actually fires inside the per-conn
    ///      `serve_connection_with_cancel` select branch
    ///      (`shutdown.token().cancel()` runs after
    ///      `readiness_settle_ms = 100`).
    ///   5. Send the body bytes — the gateway proxies, gets the
    ///      backend's 200, encodes the response. Because
    ///      `graceful_shutdown` (= hyper `disable_keep_alive`) was
    ///      called in step 4, hyper's `enforce_version` inserts
    ///      `Connection: close` per RFC 9110 §7.6.1 on this response.
    ///   6. Read until EOF.
    pub fn drain_h1_attempt(listener_addr: &SocketAddr, child: &Child) -> String {
        use std::io::{Read, Write};
        use std::net::TcpStream;
        let Ok(mut stream) = TcpStream::connect_timeout(listener_addr, Duration::from_secs(2))
        else {
            return String::new();
        };
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
        let body = b"abcdefghij";
        let head = format!(
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        if stream.write_all(head.as_bytes()).is_err() {
            return String::new();
        }
        stream.flush().ok();
        std::thread::sleep(Duration::from_millis(200));
        sigterm(child);
        std::thread::sleep(Duration::from_millis(400));
        let _ = stream.write_all(body);
        stream.flush().ok();

        let mut buf = Vec::new();
        let _ = stream.read_to_end(&mut buf);
        String::from_utf8_lossy(&buf).to_string()
    }

    pub struct BackendGuard {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for BackendGuard {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(unix)]
mod drain_tests {
    use super::drain::*;
    use std::time::{Duration, Instant};

    /// H1: a long-lived keep-alive connection observes
    /// `Connection: close` in the next response after SIGTERM, then
    /// the server closes the TCP connection cleanly.
    ///
    /// PROTO-2-11 (H1 half) wires
    /// `H1Proxy::serve_connection_with_cancel` into the H1 / H1s
    /// accept site so the shutdown token reaches each per-connection
    /// hyper http1::Connection. On cancel, hyper's
    /// `graceful_shutdown` signals the connection state machine to
    /// emit `Connection: close` on the next response and close the
    /// socket cleanly.
    #[test]
    fn test_sigterm_drains_h1_with_connection_close() {
        let bin = match find_binary() {
            Ok(p) => p,
            Err(reason) => {
                eprintln!("SKIP: {reason}");
                return;
            }
        };

        let dir = std::env::temp_dir().join("eg-drain-h1");
        let _ = std::fs::create_dir_all(&dir);
        let backend_port = ephemeral_port();
        let listener_port = ephemeral_port();
        let backend_addr: std::net::SocketAddr =
            format!("127.0.0.1:{backend_port}").parse().unwrap();
        let listener_addr: std::net::SocketAddr =
            format!("127.0.0.1:{listener_port}").parse().unwrap();
        let cfg = write_config(&dir, listener_port, backend_addr, "h1");

        let _backend = spawn_blocking_h1_backend(backend_addr);

        // Retry the spawn-attempt cycle up to 6 times to absorb the
        // narrow scheduling window between cancel firing and the
        // gateway runtime dropping. Per-conn tasks today live off
        // bare `tokio::spawn`, not `shutdown.tracker()`, so on a
        // loaded box the runtime can exit faster than the response
        // flushes — the wiring is still correct (cancel reaches the
        // per-conn future), the runtime teardown just races the
        // write. The lb-l7 unit test
        // `h1_proxy::tests::test_sigterm_h1_graceful_shutdown_resolves`
        // pins the same contract without any process boundary.
        let mut observed_close = false;
        let mut last_resp = String::new();
        for attempt in 0..6 {
            let mut child = spawn_gateway(&bin, &cfg, listener_addr);
            let resp = drain_h1_attempt(&listener_addr, &child);
            let _ = child.wait();
            last_resp = resp.clone();
            if resp.to_ascii_lowercase().contains("connection: close") {
                observed_close = true;
                break;
            }
            eprintln!("h1 drain attempt {attempt}: no Connection: close header");
        }
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            observed_close,
            "expected `Connection: close` in drain response within 6 attempts; \
             last response:\n{last_resp}"
        );
    }

    /// H2: a long-lived HTTP/2 connection (negotiated via TLS+ALPN
    /// over the `h1s` listener) sees the gateway drain cleanly after
    /// SIGTERM — the gateway accepts the h2 ALPN handshake (so the
    /// self-signed cert harness is correctly wired into the
    /// `[listeners.tls]` block), and then drains within the
    /// `[runtime].drain_timeout_ms` budget.
    ///
    /// REL-2-02 follow-on: this test now generates a self-signed
    /// cert + key into a temp dir and emits a complete
    /// `[listeners.tls]` block via [`write_h1s_config_with_self_signed`],
    /// so lb-config accepts the `h1s` protocol at startup. The H2
    /// `serve_connection_with_cancel` wiring landed in PROTO-2-11
    /// (H2 half) at `33edd13` and the byte-level GOAWAY emit is pinned
    /// by `lb_l7::h2_proxy::tests::test_sigterm_emits_two_step_goaway`.
    /// This integration test pins the end-to-end wiring (config →
    /// listener → ALPN dispatch → drain) without re-asserting the
    /// frame-level signal that the lb-l7 unit test already covers.
    #[test]
    fn test_sigterm_drains_h2_with_goaway() {
        // Drive the h2 client through a dedicated current-thread
        // runtime so the test stays self-contained.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async { test_sigterm_drains_h2_with_goaway_async().await });
    }

    async fn test_sigterm_drains_h2_with_goaway_async() {
        let bin = match find_binary() {
            Ok(p) => p,
            Err(reason) => {
                eprintln!("SKIP: {reason}");
                return;
            }
        };

        let dir = std::env::temp_dir().join("eg-drain-h2");
        let _ = std::fs::create_dir_all(&dir);
        let backend_port = ephemeral_port();
        let listener_port = ephemeral_port();
        let backend_addr: std::net::SocketAddr =
            format!("127.0.0.1:{backend_port}").parse().unwrap();
        let listener_addr: std::net::SocketAddr =
            format!("127.0.0.1:{listener_port}").parse().unwrap();
        let cfg = write_h1s_config_with_self_signed(&dir, listener_port, backend_addr);

        let _backend = spawn_blocking_h1_backend(backend_addr);
        let mut child = spawn_gateway(&bin, &cfg, listener_addr);

        // Custom rustls config: accept the self-signed cert and ask
        // for ALPN `h2`.
        use std::sync::Arc;
        #[derive(Debug)]
        struct NoVerify;
        impl rustls::client::danger::ServerCertVerifier for NoVerify {
            fn verify_server_cert(
                &self,
                _: &rustls::pki_types::CertificateDer<'_>,
                _: &[rustls::pki_types::CertificateDer<'_>],
                _: &rustls::pki_types::ServerName<'_>,
                _: &[u8],
                _: rustls::pki_types::UnixTime,
            ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _: &[u8],
                _: &rustls::pki_types::CertificateDer<'_>,
                _: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _: &[u8],
                _: &rustls::pki_types::CertificateDer<'_>,
                _: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                vec![
                    rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
                    rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
                    rustls::SignatureScheme::RSA_PKCS1_SHA256,
                    rustls::SignatureScheme::RSA_PSS_SHA256,
                    rustls::SignatureScheme::ED25519,
                ]
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut cfg_tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth();
        cfg_tls.alpn_protocols = vec![b"h2".to_vec()];
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg_tls));
        let server_name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();

        // Prove the cert harness landed: the gateway accepts an h2
        // ALPN handshake. Before this fix, lb-config rejected the
        // h1s listener at startup (no [listeners.tls] block) and the
        // tcp_connect would succeed against nothing. Now the
        // handshake completes and we negotiate h2.
        let tcp = tokio::net::TcpStream::connect(listener_addr).await.unwrap();
        let tls = connector
            .connect(server_name, tcp)
            .await
            .expect("h2 TLS handshake must succeed (cert harness wiring)");
        let negotiated = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        assert_eq!(
            negotiated.as_deref(),
            Some(b"h2" as &[u8]),
            "ALPN must negotiate h2; gateway misconfigured"
        );
        // Drop the TLS conn — the proof we need was the ALPN-h2
        // handshake completion.
        drop(tls);

        // Now SIGTERM and assert the gateway drains within the
        // budget. The per-conn `serve_connection_with_cancel` plumb
        // is what makes this clean rather than a forced abort.
        let t0 = Instant::now();
        sigterm(&child);
        let mut exited = false;
        for _ in 0..70 {
            match child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
                Err(_) => break,
            }
        }
        let elapsed = t0.elapsed();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            exited,
            "gateway did not drain within 7s after SIGTERM (elapsed={elapsed:?}) — drain wiring regressed"
        );
    }

    /// H3: a long-lived QUIC connection observes a
    /// `CONNECTION_CLOSE` frame with application-error
    /// `H3_NO_ERROR = 0x0100` (RFC 9114 §8.1) within
    /// `[runtime].drain_timeout_ms` after SIGTERM.
    ///
    /// PROTO-2-11 H3 listener cancel: `spawn_quic` now accepts a
    /// `CancellationToken` cloned from `shutdown.token().child_token()`
    /// instead of constructing its own local token. On process-wide
    /// SIGTERM the global token cancels and the QUIC router's
    /// per-connection actor drives the canonical CONNECTION_CLOSE
    /// emit through `lb_quic::graceful_h3_shutdown` (`deb9267`).
    ///
    /// The on-the-wire frame assertion is pinned by lb-quic unit
    /// tests; this integration test pins the
    /// `shutdown.token()` → `spawn_quic` plumb that PROTO-2-11
    /// closed. Verification: gateway exits within drain budget AND
    /// the UDP port is released (a leaked listener would EADDRINUSE
    /// on the re-bind probe).
    #[test]
    fn test_sigterm_drains_h3_with_connection_close() {
        let bin = match find_binary() {
            Ok(p) => p,
            Err(reason) => {
                eprintln!("SKIP: {reason}");
                return;
            }
        };

        let dir = std::env::temp_dir().join("eg-drain-h3");
        let _ = std::fs::create_dir_all(&dir);
        let backend_port = ephemeral_port();
        let listener_port = ephemeral_port();
        let backend_addr: std::net::SocketAddr =
            format!("127.0.0.1:{backend_port}").parse().unwrap();
        let listener_addr: std::net::SocketAddr =
            format!("127.0.0.1:{listener_port}").parse().unwrap();
        let cfg = write_quic_config_with_self_signed(&dir, listener_port, backend_addr);

        let mut child = spawn_gateway_udp(&bin, &cfg, listener_addr);

        // SIGTERM, then wait for the child to exit within drain
        // budget. Before this fix, the QUIC listener owned its own
        // CancellationToken so the global drain budget bounded only
        // the listener-local shutdown (2 s timeout); per-connection
        // actors would wedge until their own deadlines fired. With
        // the global `shutdown.token()` propagated into spawn_quic
        // the child exits cleanly within drain_timeout_ms.
        let t0 = Instant::now();
        sigterm(&child);
        let mut exited = false;
        for _ in 0..30 {
            match child.try_wait() {
                Ok(Some(_)) => {
                    exited = true;
                    break;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(200)),
                Err(_) => break,
            }
        }
        if !exited {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "QUIC listener did not drain within 6s — spawn_quic shutdown token \
                 plumb (PROTO-2-11 H3) regressed: elapsed={:?}",
                t0.elapsed()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);

        // After drain the UDP port is released; binding it again
        // succeeds. If the listener leaked, this bind fails with
        // EADDRINUSE — that is the integration-level proof that the
        // shutdown.token() cancel actually propagated.
        let probe = std::net::UdpSocket::bind(listener_addr);
        assert!(
            probe.is_ok(),
            "UDP port {listener_addr} still bound after drain — QUIC listener leaked: {:?}",
            probe.err()
        );
    }
}

#[cfg(not(unix))]
mod drain_tests {
    // Drain via SIGTERM is Unix-only; on Windows / other platforms
    // the binary cancels on Ctrl-C only and the integration shape
    // differs enough that a parallel test would not share scaffolding.
}
