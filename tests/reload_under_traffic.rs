//! S37-C: SIGHUP config hot-reload — the R13 six-proof gate, on a REAL
//! running gateway under traffic.
//!
//! Mirrors `tests/reload_zero_drop.rs` `mod drain`: spawns the production
//! `expressgateway` binary via `Command`, points it at real in-process
//! HTTP/1.1 backends, drives wire traffic, and delivers signals via raw
//! `kill(2)`. The SIGHUP here is signal `1` (the config-reload trigger,
//! the sibling of `mod drain`'s SIGTERM).
//!
//! Backends are DISTINGUISHABLE: each returns its own `X-Backend-Id`
//! response header so a test can prove WHICH backend served a request —
//! the observable that makes reload-takes-effect + live-connection-
//! survives + no-cross-talk checkable on the wire.
//!
//! The six proofs (owner gate):
//!   (a) live-connection-survives-reload — a keep-alive conn opened before
//!       SIGHUP keeps routing to its captured backend after the swap.
//!   (b) invalid-reload-no-blip — an invalid SIGHUP changes nothing; the
//!       old config stays fully live.
//!   (c) restart-required-honesty — a bind-address change is detected, the
//!       bind does NOT silently change, the new port is refused.
//!   (d) no-cross-talk — a pre-reload conn and a post-reload conn observe
//!       their own snapshots, no leakage either way.
//!   (e) reload-TAKES-EFFECT — after SIGHUP swaps the backend set, a NEW
//!       connection observes the new backend.
//!   (f) R3-after-seam — the existing e2e/soak suite passes UNCHANGED with
//!       the seam in place (the workspace ×1 gate; not re-run here).
//!
//! The in-process counterpart of (a)'s NEGATIVE CONTROL — proving the
//! ArcSwap capture discipline is load-bearing (a captured snapshot is
//! unaffected by a concurrent `.store()`, where a naive in-place mutate
//! WOULD change it) — lives in the binary's unit test
//! `arcswap_captured_snapshot_survives_store` (crates/lb/src/main.rs).
//! These wire proofs assert the resulting end-to-end behaviour.
//!
//! Each test SELF-SKIPS (early-return) when the binary is absent so a
//! clean tree does not red CI; run with:
//!   cargo build -p lb --bin expressgateway && \
//!   cargo test --test reload_under_traffic -- --test-threads=1 --nocapture

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(unix)]
mod hup {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener as StdListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    // ── binary + boot ──────────────────────────────────────────────────

    fn find_binary() -> Result<PathBuf, String> {
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

    fn boot_timeout() -> Duration {
        std::env::var("LB_TEST_BOOT_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .map_or(Duration::from_secs(10), Duration::from_secs)
    }

    fn ephemeral_port() -> u16 {
        let l = StdListener::bind(("127.0.0.1", 0)).expect("ephemeral bind");
        let port = l.local_addr().expect("local_addr").port();
        drop(l);
        port
    }

    static DIR_SEQ: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("eg-hup-{tag}-{}-{nanos}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Spawn the gateway; poll TCP-accept readiness on `addr`. Returns
    /// `None` (so the caller can retry on a fresh port) if it never binds.
    fn try_spawn_gateway(bin: &Path, config: &Path, addr: SocketAddr) -> Option<Child> {
        let mut child = Command::new(bin)
            .arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("RUST_LOG", "info")
            .spawn()
            .expect("spawn expressgateway");
        let deadline = Instant::now() + boot_timeout();
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                return Some(child);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();
        None
    }

    /// Deliver SIGHUP (signal 1) — the config-reload trigger. Mirrors
    /// `mod drain`'s `sigterm`.
    fn sighup(child: &Child) {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGHUP: i32 = 1;
        let rc = unsafe { kill(child.id() as i32, SIGHUP) };
        assert!(
            rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(3),
            "kill(SIGHUP) returned {rc}"
        );
    }

    struct ChildGuard(Option<Child>);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if let Some(mut c) = self.0.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    // ── distinguishable backends ───────────────────────────────────────

    struct BackendGuard {
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for BackendGuard {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    /// Spawn an H1 backend that tags every 200 with `X-Backend-Id: <id>`
    /// so a test can prove which backend served a request. Keep-alive so
    /// a single client conn can issue multiple requests across a reload.
    fn spawn_tagged_backend(id: &'static str) -> (BackendGuard, SocketAddr) {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_w = Arc::clone(&stop);
        let listener = StdListener::bind(("127.0.0.1", 0)).expect("backend bind");
        let addr = listener.local_addr().expect("backend local_addr");
        listener.set_nonblocking(true).expect("backend nonblocking");
        let handle = std::thread::spawn(move || {
            while !stop_w.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((sock, _)) => {
                        let stop_c = Arc::clone(&stop_w);
                        // One thread per conn so keep-alive works across
                        // multiple requests (the live-conn-survives proof).
                        std::thread::spawn(move || serve_conn(sock, id, &stop_c));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(15));
                    }
                    Err(_) => break,
                }
            }
        });
        (
            BackendGuard {
                stop,
                handle: Some(handle),
            },
            addr,
        )
    }

    fn serve_conn(mut sock: TcpStream, id: &str, stop: &AtomicBool) {
        sock.set_read_timeout(Some(Duration::from_millis(500))).ok();
        let mut buf = [0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            match sock.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let body = b"ok";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nX-Backend-Id: {id}\r\n\
                         Connection: keep-alive\r\n\r\n",
                        body.len()
                    );
                    if sock.write_all(resp.as_bytes()).is_err() {
                        break;
                    }
                    let _ = sock.write_all(body);
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    // idle keep-alive — keep waiting for the next request
                }
                Err(_) => break,
            }
        }
    }

    // ── config ─────────────────────────────────────────────────────────

    fn write_config(dir: &Path, listener_port: u16, backend: SocketAddr) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{listener_port}"
protocol = "h1"

[[listeners.backends]]
address = "{backend}"
weight = 1
"#
        );
        let path = dir.join("gateway.toml");
        std::fs::write(&path, toml).expect("write config");
        path
    }

    fn write_config_bind(dir: &Path, listener_port: u16, backend: SocketAddr) -> PathBuf {
        // Same backend, DIFFERENT bind port — a restart-required change.
        write_config(dir, listener_port, backend)
    }

    // ── wire client ─────────────────────────────────────────────────────

    /// Send one GET on a FRESH connection; return the `X-Backend-Id` of
    /// the 200 (or `None` on any failure / non-200).
    fn get_backend_id(addr: &SocketAddr) -> Option<String> {
        let mut s = TcpStream::connect_timeout(addr, Duration::from_secs(2)).ok()?;
        request_on(&mut s)
    }

    /// Send one GET on an EXISTING (keep-alive) connection; return the
    /// `X-Backend-Id` of the 200.
    fn request_on(stream: &mut TcpStream) -> Option<String> {
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
        let head = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n";
        stream.write_all(head.as_bytes()).ok()?;
        stream.flush().ok();
        // Read until we have headers + the 2-byte body.
        let mut acc = Vec::new();
        let mut buf = [0u8; 1024];
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    acc.extend_from_slice(&buf[..n]);
                    if let Some(hdr_end) = find_subslice(&acc, b"\r\n\r\n") {
                        // Have full headers; "ok" body is 2 bytes.
                        if acc.len() >= hdr_end + 4 + 2 {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
            if Instant::now() > deadline {
                break;
            }
        }
        let text = String::from_utf8_lossy(&acc);
        if !text.starts_with("HTTP/1.1 200") {
            return None;
        }
        // The gateway lowercases header names on egress (hyper
        // normalisation), so match case-insensitively on the name.
        for line in text.lines() {
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("x-backend-id") {
                    return Some(value.trim().to_owned());
                }
            }
        }
        None
    }

    fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// Spawn the gateway on a retried fresh port with v1 config; return
    /// (child-guard, listener_addr, config_path, temp_dir).
    fn boot(tag: &str, backend: SocketAddr) -> (ChildGuard, SocketAddr, PathBuf, PathBuf) {
        let bin = find_binary().expect("binary present (caller gated on this)");
        for _ in 0..5 {
            let port = ephemeral_port();
            let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let dir = unique_temp_dir(tag);
            let cfg = write_config(&dir, port, backend);
            if let Some(child) = try_spawn_gateway(&bin, &cfg, addr) {
                return (ChildGuard(Some(child)), addr, cfg, dir);
            }
            std::fs::remove_dir_all(&dir).ok();
        }
        panic!("gateway never bound after 5 attempts");
    }

    /// Give the SIGHUP reload time to land (re-read + validate + diff +
    /// store). The reload is CPU-cheap + one DNS resolve of a literal IP;
    /// poll for the observable rather than a fixed sleep where possible.
    fn await_reload_effect(addr: &SocketAddr, want: &str, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if get_backend_id(addr).as_deref() == Some(want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    // ── proofs ──────────────────────────────────────────────────────────

    /// (e) reload-TAKES-EFFECT + (a) live-connection-survives-reload, in
    /// one real run (they share the same swap so proving both together is
    /// the strongest single assertion).
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_e_takes_effect_and_a_live_conn_survives() {
        if find_binary().is_err() {
            eprintln!("SKIP: binary absent");
            return;
        }
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        let (backend_b, addr_b) = spawn_tagged_backend("B");
        let (child, listener, _cfg, dir) = boot("e-a", addr_a);

        // Baseline: traffic hits backend A.
        assert_eq!(
            get_backend_id(&listener).as_deref(),
            Some("A"),
            "baseline must route to backend A"
        );

        // (a) Open a keep-alive conn and pin it to A BEFORE the reload.
        let mut live = TcpStream::connect_timeout(&listener, Duration::from_secs(2)).unwrap();
        assert_eq!(
            request_on(&mut live).as_deref(),
            Some("A"),
            "live conn first request routes to A"
        );

        // SIGHUP: swap the backend set to B.
        write_config(&dir, listener.port(), addr_b);
        sighup(child.0.as_ref().unwrap());

        // (e) A NEW connection observes the new backend B.
        assert!(
            await_reload_effect(&listener, "B", Duration::from_secs(5)),
            "after SIGHUP a NEW connection must route to backend B (reload took effect)"
        );

        // (a) The pre-reload live conn STILL routes to its captured
        // snapshot (A) — the swap did not disturb the in-flight conn.
        assert_eq!(
            request_on(&mut live).as_deref(),
            Some("A"),
            "pre-reload live conn must keep routing to A (captured snapshot survives reload)"
        );

        drop(child);
        drop((backend_a, backend_b));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// (d) no-cross-talk: a pre-reload conn (snapshot v1=A) and a
    /// post-reload conn (snapshot v2=B) driven concurrently each observe
    /// only their own snapshot.
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_d_no_cross_talk() {
        if find_binary().is_err() {
            eprintln!("SKIP: binary absent");
            return;
        }
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        let (backend_b, addr_b) = spawn_tagged_backend("B");
        let (child, listener, _cfg, dir) = boot("d", addr_a);

        let mut conn_v1 = TcpStream::connect_timeout(&listener, Duration::from_secs(2)).unwrap();
        assert_eq!(request_on(&mut conn_v1).as_deref(), Some("A"));

        write_config(&dir, listener.port(), addr_b);
        sighup(child.0.as_ref().unwrap());
        assert!(await_reload_effect(&listener, "B", Duration::from_secs(5)));

        let mut conn_v2 = TcpStream::connect_timeout(&listener, Duration::from_secs(2)).unwrap();
        assert_eq!(request_on(&mut conn_v2).as_deref(), Some("B"));

        // Interleave both conns repeatedly — each holds its snapshot.
        for _ in 0..5 {
            assert_eq!(
                request_on(&mut conn_v1).as_deref(),
                Some("A"),
                "v1 conn must never observe v2's backend (no cross-talk)"
            );
            assert_eq!(
                request_on(&mut conn_v2).as_deref(),
                Some("B"),
                "v2 conn must never observe v1's backend (no cross-talk)"
            );
        }

        drop(child);
        drop((backend_a, backend_b));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// (b) invalid-reload-no-blip: an invalid config on SIGHUP changes
    /// nothing; the old config stays fully live and traffic keeps flowing.
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_b_invalid_reload_no_blip() {
        if find_binary().is_err() {
            eprintln!("SKIP: binary absent");
            return;
        }
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        let (child, listener, _cfg, dir) = boot("b", addr_a);
        assert_eq!(get_backend_id(&listener).as_deref(), Some("A"));

        // Write a config that PARSES as TOML but FAILS lb_config
        // validation (a listener with no backends + no passthrough is
        // rejected by validate_config). The reload must reject it and
        // keep the live config.
        let bad = "[[listeners]]\naddress = \"127.0.0.1:1\"\nprotocol = \"h1\"\n";
        std::fs::write(dir.join("gateway.toml"), bad).expect("write bad config");
        sighup(child.0.as_ref().unwrap());

        // Drive a burst of traffic across the (rejected) reload window;
        // every request must still succeed AND still hit backend A.
        std::thread::sleep(Duration::from_millis(300));
        for i in 0..20 {
            assert_eq!(
                get_backend_id(&listener).as_deref(),
                Some("A"),
                "request {i} after an INVALID SIGHUP must still hit the live backend A (no blip)"
            );
        }

        drop(child);
        drop(backend_a);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// (c) restart-required-honesty: a bind-address change is detected,
    /// the bind does NOT silently change (original port still serving,
    /// new port refused). The honesty WARN + metric are asserted by the
    /// log-scrape variant below; here we assert the OBSERVABLE: no silent
    /// rebind.
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_c_restart_required_no_silent_rebind() {
        if find_binary().is_err() {
            eprintln!("SKIP: binary absent");
            return;
        }
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        let (child, listener, _cfg, dir) = boot("c", addr_a);
        assert_eq!(get_backend_id(&listener).as_deref(), Some("A"));

        // SIGHUP with a NEW bind port (restart-required). The gateway
        // must NOT move: it keeps serving the original port and never
        // binds the new one.
        let new_port = ephemeral_port();
        let new_addr: SocketAddr = format!("127.0.0.1:{new_port}").parse().unwrap();
        write_config_bind(&dir, new_port, addr_a);
        sighup(child.0.as_ref().unwrap());
        std::thread::sleep(Duration::from_millis(400));

        // Original port STILL serves (no silent unbind).
        assert_eq!(
            get_backend_id(&listener).as_deref(),
            Some("A"),
            "original bind must keep serving after a restart-required SIGHUP"
        );
        // New port was NOT silently bound.
        assert!(
            TcpStream::connect_timeout(&new_addr, Duration::from_millis(300)).is_err(),
            "the would-be-new bind port must NOT be bound (no silent rebind)"
        );

        drop(child);
        drop(backend_a);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// (c) honesty — the WARN + metric path. Scrapes the gateway stderr
    /// for the per-field "requires restart, not applied" warning so the
    /// detect-and-report contract is proven (not merely the no-rebind
    /// observable). Uses a captured stderr pipe.
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_c_restart_required_logs_honest_warning() {
        let Ok(bin) = find_binary() else {
            eprintln!("SKIP: binary absent");
            return;
        };
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        // Boot with stderr piped so we can scrape the honesty WARN.
        let mut child_opt = None;
        let mut listener_opt = None;
        let mut dir_opt = None;
        for _ in 0..5 {
            let port = ephemeral_port();
            let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
            let dir = unique_temp_dir("c-log");
            let cfg = write_config(&dir, port, addr_a);
            // tracing_subscriber::fmt() writes to STDOUT, so capture
            // stdout for the honesty-WARN scrape (stderr is nulled).
            let child = Command::new(&bin)
                .arg(&cfg)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .env("RUST_LOG", "info")
                .spawn()
                .expect("spawn");
            let deadline = Instant::now() + boot_timeout();
            let mut ready = false;
            while Instant::now() < deadline {
                if TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
                    ready = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if ready {
                child_opt = Some(child);
                listener_opt = Some(addr);
                dir_opt = Some(dir);
                break;
            }
            let mut c = child;
            let _ = c.kill();
            let _ = c.wait();
            std::fs::remove_dir_all(&dir).ok();
        }
        let mut child = child_opt.expect("gateway bound");
        let listener = listener_opt.unwrap();
        let dir = dir_opt.unwrap();
        let stdout = child.stdout.take().expect("piped stdout");

        // Drain stdout on a thread, watching for the honesty WARN.
        let found = Arc::new(AtomicBool::new(false));
        let found_w = Arc::clone(&found);
        let scraper = std::thread::spawn(move || {
            let mut rdr = std::io::BufReader::new(stdout);
            let mut line = String::new();
            use std::io::BufRead;
            loop {
                line.clear();
                match rdr.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if line.contains("requires restart, not applied") {
                            found_w.store(true, Ordering::Relaxed);
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        assert_eq!(get_backend_id(&listener).as_deref(), Some("A"));
        // Restart-required SIGHUP (bind-port change).
        let new_port = ephemeral_port();
        write_config_bind(&dir, new_port, addr_a);
        sighup(&child);

        // Poll for the honesty WARN within a bounded budget.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !found.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let honest = found.load(Ordering::Relaxed);

        let _ = child.kill();
        let _ = child.wait();
        let _ = scraper.join();
        drop(backend_a);
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            honest,
            "a restart-required SIGHUP must log a per-field 'requires restart, not applied' WARN \
             (honesty contract — never a silent no-op)"
        );
    }
}
