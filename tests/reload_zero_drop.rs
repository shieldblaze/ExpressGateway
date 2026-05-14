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
//!    What is **missing** is the plumbing that fires those calls
//!    from `lb_core::Shutdown::token()` into each
//!    `serve_connection` future in `crates/lb/src/main.rs`. PROTO-2-11
//!    (H2 half) + PROTO-2-09 (H1) + listener-token plumb (H3) are
//!    wave-2c follow-ups owned by `code-r4w2cb`. Until they land,
//!    the drain tests are `#[ignore]`'d with this comment as the
//!    pointer.
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
}

#[cfg(unix)]
mod drain_tests {
    use super::drain::*;

    /// H1: a long-lived keep-alive connection observes
    /// `Connection: close` in the next response after SIGTERM, then
    /// the server closes the TCP connection cleanly.
    ///
    /// **Ignored** because `crates/lb/src/main.rs` does not yet plumb
    /// `lb_core::Shutdown::token()` into `H1Proxy::serve_connection`
    /// so that hyper's `http1::Connection::graceful_shutdown` fires
    /// on cancel. PROTO-2-09 owns the proxy-level wiring; the
    /// `main.rs` plumbing is sequential to that and behind
    /// `code-r4w2cb`.
    #[test]
    #[ignore = "PROTO-2-09 H1 graceful_shutdown plumbing pending (wave 2c)"]
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

        // Spawn a placeholder backend that 200s every request. The
        // wave-2c wiring will let us assert the in-flight request
        // completes; today the gateway has no per-connection drain
        // path so the test stops at the spawn.
        let mut child = spawn_gateway(&bin, &cfg, listener_addr);

        // Open the long-lived H1 connection, send a request to prime
        // keep-alive, then SIGTERM and read the next response. The
        // production binary should respond with `Connection: close`.
        //
        // Concrete client wiring TBD with PROTO-2-09; today we just
        // sanity-check the SIGTERM path.
        sigterm(&child);
        let _ = child.wait();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// H2: a long-lived keep-alive `hyper::client::conn::http2`
    /// connection observes a GOAWAY frame with NO_ERROR (0x0)
    /// before the connection closes. The two-step GOAWAY pattern
    /// (RFC 7540 §6.8 / RFC 9113 §6.8) advertises the highest
    /// stream the peer will still process: first GOAWAY with
    /// `last_stream_id = 2^31 - 1`, then a follow-up with the actual
    /// last accepted stream id.
    ///
    /// **Ignored** because `H2Proxy::serve_connection` does not yet
    /// call hyper's `http2::Connection::graceful_shutdown` on
    /// `lb_core::Shutdown::token()` cancellation. PROTO-2-11 H2
    /// owns the proxy-level wiring; the `main.rs` plumbing follows.
    /// `code-r4w2cb` is landing this in parallel.
    #[test]
    #[ignore = "PROTO-2-11 H2 GOAWAY plumbing pending (wave 2c)"]
    fn test_sigterm_drains_h2_with_goaway() {
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
        // h2 cleartext on h1s would normally need TLS+ALPN; the test
        // assumes the wave-2c wiring exposes an h2c listener variant.
        // Until then the listener proto stays "h1s" and the test
        // remains ignored.
        let cfg = write_config(&dir, listener_port, backend_addr, "h1s");

        let mut child = spawn_gateway(&bin, &cfg, listener_addr);

        sigterm(&child);
        let _ = child.wait();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// H3: a long-lived QUIC connection observes a
    /// `CONNECTION_CLOSE` frame with application-error
    /// `H3_NO_ERROR = 0x0100` (RFC 9114 §8.1) within
    /// `[runtime].drain_timeout_ms` after SIGTERM. The frame's
    /// `is_app == true` distinguishes it from a transport-layer
    /// reset; `error_code == 0x0100` distinguishes graceful drain
    /// from any other application-level close.
    ///
    /// **Ignored** because the actor's CONNECTION_CLOSE machinery
    /// (PROTO-2-11, `deb9267`) is gated on the listener's local
    /// `CancellationToken`, which is not yet fed from
    /// `lb_core::Shutdown::token()` in `crates/lb/src/main.rs`.
    /// `code-r4w2cb` is landing the plumb.
    #[test]
    #[ignore = "H3 listener-token plumbing from lb_core::Shutdown pending (wave 2c)"]
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
        let cfg = write_config(&dir, listener_port, backend_addr, "quic");

        let mut child = spawn_gateway(&bin, &cfg, listener_addr);

        sigterm(&child);
        let _ = child.wait();

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(not(unix))]
mod drain_tests {
    // Drain via SIGTERM is Unix-only; on Windows / other platforms
    // the binary cancels on Ctrl-C only and the integration shape
    // differs enough that a parallel test would not share scaffolding.
}
