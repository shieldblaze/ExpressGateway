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

    /// Deliver an arbitrary signal to the child via raw `kill(2)`.
    fn raw_signal(child: &Child, sig: i32) {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let rc = unsafe { kill(child.id() as i32, sig) };
        assert!(
            rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(3),
            "kill(sig={sig}) returned {rc}"
        );
    }

    /// Deliver SIGHUP (signal 1) — the config-reload trigger.
    fn sighup(child: &Child) {
        raw_signal(child, 1);
    }

    /// Wait (bounded) for the child to exit; returns true if it exited
    /// within `budget`. Mutates the child to reap it.
    fn wait_exit(child: &mut Child, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return false,
            }
        }
        false
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

    /// Config variant that also binds a loopback admin/metrics listener so
    /// a test can scrape `/metrics` and assert the reload counters.
    fn write_config_with_metrics(
        dir: &Path,
        listener_port: u16,
        metrics_port: u16,
        backend: SocketAddr,
    ) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
readiness_settle_ms = 100

[observability]
metrics_bind = "127.0.0.1:{metrics_port}"

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

    /// Scrape `GET /metrics` from the admin listener and return the value
    /// of `metric_name` (the first matching sample line's trailing number),
    /// or `None` if absent.
    fn scrape_metric(metrics_addr: &SocketAddr, metric_name: &str) -> Option<f64> {
        let mut s = TcpStream::connect_timeout(metrics_addr, Duration::from_secs(2)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();
        s.write_all(b"GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .ok()?;
        s.flush().ok();
        let mut body = Vec::new();
        let _ = s.read_to_end(&mut body);
        let text = String::from_utf8_lossy(&body);
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            // Match "name" or "name{labels}" then a trailing value.
            let matches_name = line.split_once('{').map_or_else(
                || line.split_whitespace().next() == Some(metric_name),
                |(n, _)| n == metric_name,
            );
            if matches_name {
                if let Some(v) = line.split_whitespace().last() {
                    if let Ok(n) = v.parse::<f64>() {
                        return Some(n);
                    }
                }
            }
        }
        None
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

    /// Drive a single keep-alive connection issuing requests until the
    /// gateway sends `Connection: close` (the `max_keepalive_requests`
    /// cap), and return how many requests were served before the close.
    /// Bounded so a never-closing connection can't hang the test.
    fn requests_until_keepalive_close(addr: &SocketAddr, max_probe: u32) -> u32 {
        let Ok(mut s) = TcpStream::connect_timeout(addr, Duration::from_secs(2)) else {
            return 0;
        };
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();
        for n in 1..=max_probe {
            let head = "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: keep-alive\r\n\r\n";
            if s.write_all(head.as_bytes()).is_err() {
                return n - 1;
            }
            s.flush().ok();
            let mut acc = Vec::new();
            let mut buf = [0u8; 1024];
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut got_close = false;
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(b) => {
                        acc.extend_from_slice(&buf[..b]);
                        if let Some(end) = find_subslice(&acc, b"\r\n\r\n") {
                            if acc.len() >= end + 4 + 2 {
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
            let text = String::from_utf8_lossy(&acc).to_lowercase();
            if text.contains("connection: close") {
                got_close = true;
            }
            if got_close {
                return n; // the cap-th response carried Connection: close
            }
        }
        max_probe + 1 // never closed within the probe budget
    }

    /// Config variant with a `[runtime].max_keepalive_requests` cap (a
    /// process-wide, RESTART-REQUIRED field) + a metrics listener.
    fn write_config_keepalive(
        dir: &Path,
        listener_port: u16,
        metrics_port: u16,
        backend: SocketAddr,
        keepalive_cap: u32,
    ) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
readiness_settle_ms = 100
max_keepalive_requests = {keepalive_cap}

[observability]
metrics_bind = "127.0.0.1:{metrics_port}"

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
    /// nothing; the old config stays fully live and traffic keeps flowing
    /// AND the config_reload_failed_total metric bumps (observability).
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_b_invalid_reload_no_blip() {
        let Ok(bin) = find_binary() else {
            eprintln!("SKIP: binary absent");
            return;
        };
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        // Boot with a metrics listener so we can assert the failure counter.
        let mut child_opt = None;
        let mut listener_opt = None;
        let mut metrics_opt = None;
        let mut dir_opt = None;
        for _ in 0..5 {
            let lp = ephemeral_port();
            let mp = ephemeral_port();
            if lp == mp {
                continue;
            }
            let laddr: SocketAddr = format!("127.0.0.1:{lp}").parse().unwrap();
            let maddr: SocketAddr = format!("127.0.0.1:{mp}").parse().unwrap();
            let dir = unique_temp_dir("b");
            let cfg = write_config_with_metrics(&dir, lp, mp, addr_a);
            if let Some(child) = try_spawn_gateway(&bin, &cfg, laddr) {
                child_opt = Some(child);
                listener_opt = Some(laddr);
                metrics_opt = Some(maddr);
                dir_opt = Some(dir);
                break;
            }
            std::fs::remove_dir_all(&dir).ok();
        }
        let child = ChildGuard(Some(child_opt.expect("gateway bound")));
        let listener = listener_opt.unwrap();
        let metrics = metrics_opt.unwrap();
        let dir = dir_opt.unwrap();

        assert_eq!(get_backend_id(&listener).as_deref(), Some("A"));
        let failed_before = scrape_metric(&metrics, "config_reload_failed_total").unwrap_or(0.0);

        // Write a config that PARSES as TOML but FAILS lb_config
        // validation: an `h1` listener carrying a `[listeners.tls]` block
        // is rejected by `validate_config` ("has [listeners.tls] but
        // protocol is h1"). This is a genuine validation FAILURE (not a
        // valid-but-restart-required reload), so validate-first must
        // reject it, keep the live config, and bump the failure counter.
        let bad = "[[listeners]]\naddress = \"127.0.0.1:18080\"\nprotocol = \"h1\"\n\
                   [listeners.tls]\ncert_path = \"/nonexistent.crt\"\n\
                   key_path = \"/nonexistent.key\"\n\
                   [[listeners.backends]]\naddress = \"127.0.0.1:9\"\nweight = 1\n";
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

        // The failure counter must have bumped (validate-first rejected
        // the bad config) — the observable form of "nothing applied".
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut failed_after = failed_before;
        while Instant::now() < deadline {
            failed_after = scrape_metric(&metrics, "config_reload_failed_total").unwrap_or(0.0);
            if failed_after > failed_before {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            failed_after > failed_before,
            "config_reload_failed_total must bump on an invalid SIGHUP \
             (before={failed_before}, after={failed_after})"
        );

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

    /// OWNER REQUIRED: reload-summary-matches-observed-behavior — the
    /// honesty invariant in BOTH directions, on the wire. A single SIGHUP
    /// changes one SWAPPABLE field (backends) AND one process-wide
    /// RESTART-REQUIRED field (`[runtime].max_keepalive_requests`).
    ///
    /// SWAPPABLE side: the backend change is reported applied (metric
    /// `config_reload_applied_swappable_total{field="listener.l7"}` bumped)
    /// AND observably takes effect (a NEW conn routes to the new backend).
    ///
    /// RESTART-REQUIRED side: the keepalive change is reported not-applied
    /// (metric `config_reload_restart_required_fields_total{field=
    /// "max_keepalive_requests"}` bumped) AND observably does NOT take
    /// effect (a NEW conn's keep-alive cap stays at the BOOT value).
    ///
    /// This guards BOTH the silent-no-op bug (restart-required claimed
    /// applied) AND the inverse-dishonesty bug (a field silently taking
    /// effect while reported not-applied). The per-field CLASSIFICATION for
    /// every config field is exhaustively covered by the `lb-config`
    /// `reload` diff unit tests; this proves the report tracks observed
    /// behaviour on the wire for a representative field from EACH bucket.
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn proof_reload_summary_matches_observed_behavior() {
        let Ok(bin) = find_binary() else {
            eprintln!("SKIP: binary absent");
            return;
        };
        let (backend_a, addr_a) = spawn_tagged_backend("A");
        let (backend_b, addr_b) = spawn_tagged_backend("B");
        const BOOT_CAP: u32 = 2;
        const NEW_CAP: u32 = 6;

        // Boot: backend A, keep-alive cap = 2, metrics on.
        let mut child_opt = None;
        let mut listener_opt = None;
        let mut metrics_opt = None;
        let mut dir_opt = None;
        for _ in 0..5 {
            let lp = ephemeral_port();
            let mp = ephemeral_port();
            if lp == mp {
                continue;
            }
            let laddr: SocketAddr = format!("127.0.0.1:{lp}").parse().unwrap();
            let maddr: SocketAddr = format!("127.0.0.1:{mp}").parse().unwrap();
            let dir = unique_temp_dir("sum");
            write_config_keepalive(&dir, lp, mp, addr_a, BOOT_CAP);
            if let Some(child) = try_spawn_gateway(&bin, &dir.join("gateway.toml"), laddr) {
                child_opt = Some(child);
                listener_opt = Some(laddr);
                metrics_opt = Some(maddr);
                dir_opt = Some(dir);
                break;
            }
            std::fs::remove_dir_all(&dir).ok();
        }
        let child = ChildGuard(Some(child_opt.expect("gateway bound")));
        let listener = listener_opt.unwrap();
        let metrics = metrics_opt.unwrap();
        let dir = dir_opt.unwrap();

        // Baseline: backend A, keep-alive closes at the BOOT cap (2).
        assert_eq!(get_backend_id(&listener).as_deref(), Some("A"));
        assert_eq!(
            requests_until_keepalive_close(&listener, 20),
            BOOT_CAP,
            "baseline keep-alive cap must be the boot value {BOOT_CAP}"
        );

        // SIGHUP changing, on the SAME listener address (so it stays
        // serving): two SWAPPABLE fields — backends A->B AND
        // max_keepalive_requests 2->6 — PLUS one RESTART-REQUIRED change:
        // a SECOND listener added at a fresh bind port.
        let added_port = ephemeral_port();
        let added_addr: SocketAddr = format!("127.0.0.1:{added_port}").parse().unwrap();
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
readiness_settle_ms = 100
max_keepalive_requests = {NEW_CAP}

[observability]
metrics_bind = "127.0.0.1:{}"

[[listeners]]
address = "127.0.0.1:{}"
protocol = "h1"
[[listeners.backends]]
address = "{addr_b}"
weight = 1

[[listeners]]
address = "127.0.0.1:{added_port}"
protocol = "h1"
[[listeners.backends]]
address = "{addr_a}"
weight = 1
"#,
            metrics.port(),
            listener.port(),
        );
        std::fs::write(dir.join("gateway.toml"), toml).expect("write reload config");
        sighup(child.0.as_ref().unwrap());

        // SWAPPABLE #1 applied + observable: a NEW conn observes backend B.
        assert!(
            await_reload_effect(&listener, "B", Duration::from_secs(5)),
            "swappable backend change must observably take effect (new conn -> B)"
        );
        // SWAPPABLE #2 applied + observable: a NEW conn's keep-alive cap is
        // now the NEW value (6), not the boot value (2).
        assert_eq!(
            requests_until_keepalive_close(&listener, 20),
            NEW_CAP,
            "swappable max_keepalive_requests must observably take effect — \
             a new conn must now close at the new cap {NEW_CAP}, not the boot {BOOT_CAP}"
        );
        // RESTART-REQUIRED reported + NOT observable: the added listener's
        // port was NOT silently bound.
        assert!(
            TcpStream::connect_timeout(&added_addr, Duration::from_millis(300)).is_err(),
            "the added listener's bind port must NOT be bound (restart-required, not applied)"
        );

        // SUMMARY matches in BOTH directions: applied-swappable bumped for
        // BOTH listener.l7 (backends) AND max_keepalive_requests; the
        // restart-required listener.added bumped.
        let applied_l7 = scrape_metric_labeled(
            &metrics,
            "config_reload_applied_swappable_total",
            "listener.l7",
        )
        .unwrap_or(0.0);
        let applied_keepalive = scrape_metric_labeled(
            &metrics,
            "config_reload_applied_swappable_total",
            "max_keepalive_requests",
        )
        .unwrap_or(0.0);
        let rr_added = scrape_metric_labeled(
            &metrics,
            "config_reload_restart_required_fields_total",
            "listener.added",
        )
        .unwrap_or(0.0);
        assert!(
            applied_l7 >= 1.0,
            "applied_swappable_total{{field=listener.l7}} must bump (backends applied)"
        );
        assert!(
            applied_keepalive >= 1.0,
            "applied_swappable_total{{field=max_keepalive_requests}} must bump (cap applied)"
        );
        assert!(
            rr_added >= 1.0,
            "restart_required_fields_total{{field=listener.added}} must bump (added listener not applied)"
        );

        drop(child);
        drop((backend_a, backend_b));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Scrape a labelled metric sample: the value of the line for
    /// `name{...field="<field>"...}`.
    fn scrape_metric_labeled(addr: &SocketAddr, name: &str, field: &str) -> Option<f64> {
        let mut s = TcpStream::connect_timeout(addr, Duration::from_secs(2)).ok()?;
        s.set_read_timeout(Some(Duration::from_secs(3))).ok();
        s.write_all(b"GET /metrics HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .ok()?;
        s.flush().ok();
        let mut body = Vec::new();
        let _ = s.read_to_end(&mut body);
        let text = String::from_utf8_lossy(&body);
        let needle = format!("field=\"{field}\"");
        for line in text.lines() {
            if line.starts_with(name) && line.contains(&needle) {
                if let Some(v) = line.split_whitespace().last() {
                    if let Ok(n) = v.parse::<f64>() {
                        return Some(n);
                    }
                }
            }
        }
        None
    }

    /// SIGNAL-LOSS REGRESSION (S37-C, R6): a terminal signal landing right
    /// after a non-terminal one must NOT be lost. The pre-fix
    /// `wait_for_lifecycle_signal` re-installed the signal streams every
    /// loop iteration, so a SIGTERM arriving in the re-install window after
    /// a SIGHUP/SIGUSR1 was silently dropped and the gateway never drained.
    /// With the streams installed once and reused, the terminal signal
    /// latches and the gateway exits. Tests both the SIGHUP→SIGTERM and
    /// SIGUSR1→SIGTERM orderings.
    #[test]
    #[cfg_attr(not(unix), ignore)]
    fn signal_loss_terminal_after_nonterminal_still_drains() {
        if find_binary().is_err() {
            eprintln!("SKIP: binary absent");
            return;
        }
        for nonterminal in [1_i32 /* SIGHUP */, 10 /* SIGUSR1 */] {
            let (backend_a, addr_a) = spawn_tagged_backend("A");
            let (mut child, listener, _cfg, dir) = boot("sigloss", addr_a);
            let child_inner = child.0.take().expect("child present");
            assert_eq!(get_backend_id(&listener).as_deref(), Some("A"));

            // Fire the non-terminal signal, then immediately the terminal
            // SIGTERM (15) — the window the pre-fix code lost it in.
            raw_signal(&child_inner, nonterminal);
            raw_signal(&child_inner, 15);

            // The gateway must exit (drain) within a generous budget.
            let mut owned = child_inner;
            let drained = wait_exit(&mut owned, Duration::from_secs(15));
            let _ = owned.kill();
            let _ = owned.wait();
            assert!(
                drained,
                "after a non-terminal signal ({nonterminal}) + SIGTERM, the gateway must drain \
                 and exit — a lost terminal signal would hang here"
            );
            drop(backend_a);
            std::fs::remove_dir_all(&dir).ok();
        }
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

// ── async survival proofs: long-lived gRPC bidi + WS tunnel across SIGHUP ──
//
// The owner's R13 requires that an in-flight gRPC mid-RPC AND a live WS
// tunnel COMPLETE correctly across a backend-swap SIGHUP. Both run over the
// listeners whose proxies the increment holds in Arc<ArcSwap<_>>; the
// captured-snapshot design covers them by construction (the whole proxy Arc
// is load_full()-snapshotted once at accept, so a long-lived stream keeps
// its captured proxy/backend for the connection's life regardless of a
// concurrent .store()). These tests PROVE that on the wire.
#[cfg(unix)]
mod hup_async {
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};

    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::Message;

    // Reuse the sync module's binary/port/dir/signal helpers via super.
    fn find_binary() -> Option<PathBuf> {
        let target_dir = std::env::var("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let m = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
                PathBuf::from(m).join("target")
            });
        for p in ["release", "debug"] {
            let c = target_dir.join(p).join("expressgateway");
            if c.is_file() {
                return Some(c);
            }
        }
        None
    }

    static SEQ: AtomicU64 = AtomicU64::new(0);
    fn temp_dir(tag: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let d = std::env::temp_dir().join(format!(
            "eg-hupa-{tag}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    fn ephemeral_port() -> u16 {
        let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    }

    fn sighup(child: &Child) {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let _ = unsafe { kill(child.id() as i32, 1) };
    }

    struct ChildGuard(Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    async fn tcp_ready(addr: SocketAddr, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if tokio::net::TcpStream::connect(addr).await.is_ok() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }

    // ── WS-over-H1 tunnel survival ──────────────────────────────────────

    /// A WS echo backend that prefixes every echoed Text with `id|` so the
    /// client can tell WHICH backend the tunnel is wired to.
    async fn spawn_ws_echo_backend(id: &'static str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = l.local_addr().unwrap();
        let h = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let ws = match tokio_tungstenite::accept_async(sock).await {
                        Ok(w) => w,
                        Err(_) => return,
                    };
                    let (mut tx, mut rx) = ws.split();
                    while let Some(Ok(msg)) = rx.next().await {
                        if let Message::Text(t) = msg {
                            let tagged = Message::Text(format!("{id}|{t}").into());
                            if tx.send(tagged).await.is_err() {
                                break;
                            }
                        } else if matches!(msg, Message::Close(_)) {
                            break;
                        }
                    }
                    let _ = tx.close().await;
                });
            }
        });
        (addr, h)
    }

    fn write_ws_config(dir: &std::path::Path, listener_port: u16, backend: SocketAddr) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{listener_port}"
protocol = "h1"

[listeners.websocket]
enabled = true

[[listeners.backends]]
address = "{backend}"
weight = 1
"#
        );
        let p = dir.join("gateway.toml");
        std::fs::write(&p, toml).unwrap();
        p
    }

    /// R13 (live WS tunnel survives reload): open a WS echo tunnel through
    /// the gateway to backend A, SIGHUP-swap the backend set to B, then
    /// keep using the SAME tunnel — it must still echo via backend A (the
    /// captured snapshot), and a NEW tunnel must reach backend B.
    #[tokio::test]
    async fn proof_ws_tunnel_survives_reload() {
        let Some(bin) = find_binary() else {
            eprintln!("SKIP: binary absent");
            return;
        };
        let (addr_a, _ja) = spawn_ws_echo_backend("A").await;
        let (addr_b, _jb) = spawn_ws_echo_backend("B").await;

        // Boot on a retried fresh port.
        let mut guard = None;
        let mut lport = 0u16;
        let mut dir = PathBuf::new();
        for _ in 0..5 {
            let p = ephemeral_port();
            let laddr: SocketAddr = format!("127.0.0.1:{p}").parse().unwrap();
            let d = temp_dir("ws");
            write_ws_config(&d, p, addr_a);
            let child = Command::new(&bin)
                .arg(d.join("gateway.toml"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env("RUST_LOG", "warn")
                .spawn()
                .expect("spawn");
            if tcp_ready(laddr, Duration::from_secs(10)).await {
                guard = Some(ChildGuard(child));
                lport = p;
                dir = d;
                break;
            }
            let mut c = child;
            let _ = c.kill();
            let _ = c.wait();
            std::fs::remove_dir_all(&d).ok();
        }
        let guard = guard.expect("gateway bound");
        let listener: SocketAddr = format!("127.0.0.1:{lport}").parse().unwrap();

        // Open a live tunnel to A.
        let (mut live, _) = tokio_tungstenite::connect_async(format!("ws://{listener}/"))
            .await
            .expect("ws connect");
        live.send(Message::Text("one".into())).await.unwrap();
        let echo1 = next_text(&mut live).await;
        assert_eq!(
            echo1, "A|one",
            "live tunnel must echo via backend A pre-reload"
        );

        // SIGHUP: swap backend set to B.
        write_ws_config(&dir, lport, addr_b);
        sighup(&guard.0);

        // New tunnels must reach B (reload took effect) — poll.
        let mut new_ok = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok((mut nt, _)) =
                tokio_tungstenite::connect_async(format!("ws://{listener}/")).await
            {
                nt.send(Message::Text("probe".into())).await.ok();
                if next_text(&mut nt).await == "B|probe" {
                    new_ok = true;
                    let _ = nt.close(None).await;
                    break;
                }
                let _ = nt.close(None).await;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(new_ok, "after SIGHUP a NEW WS tunnel must reach backend B");

        // The LIVE tunnel (captured snapshot) must STILL echo via A.
        live.send(Message::Text("two".into())).await.unwrap();
        let echo2 = next_text(&mut live).await;
        assert_eq!(
            echo2, "A|two",
            "the pre-reload WS tunnel must keep echoing via backend A (survives the reload)"
        );
        let _ = live.close(None).await;

        drop(guard);
        std::fs::remove_dir_all(&dir).ok();
    }

    async fn next_text<S>(ws: &mut S) -> String
    where
        S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => return t.to_string(),
                Ok(Some(Ok(_))) => {}
                _ => return String::new(),
            }
            if Instant::now() > deadline {
                return String::new();
            }
        }
    }

    // ── gRPC bidi mid-RPC survival over h1s (TLS + ALPN h2) ─────────────

    use http_body_util::{BodyExt, StreamBody};
    use hyper::body::{Bytes, Frame, Incoming};
    use hyper::{Request, Response};
    use hyper_util::rt::{TokioExecutor, TokioIo};

    type ClientBody = http_body_util::combinators::BoxBody<Bytes, std::convert::Infallible>;

    /// Encode one gRPC length-prefixed frame (1 compressed flag + 4 BE len
    /// + payload).
    fn grpc_frame(payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(5 + payload.len());
        v.push(0u8);
        v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// An h2 (cleartext) gRPC echo backend: for each inbound gRPC frame it
    /// echoes back the SAME frame with the payload prefixed by `id|`, then
    /// closes with `grpc-status: 0`. Also sets `x-backend-id: <id>` on the
    /// response headers so the client can identify the serving backend.
    /// Bidi: it reads the request body stream frame-by-frame and writes
    /// each echo as it arrives.
    async fn spawn_grpc_echo_backend(
        id: &'static str,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = l.local_addr().unwrap();
        let h = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let builder = hyper::server::conn::http2::Builder::new(TokioExecutor::new());
                    let svc =
                        hyper::service::service_fn(move |req: Request<Incoming>| async move {
                            // TRUE BIDI: respond with 200 HEADERS immediately,
                            // then stream one tagged echo DATA frame per inbound
                            // request DATA frame AS IT ARRIVES (not after the
                            // whole body), and send grpc-status:0 trailers when
                            // the request body ends. This lets the client send
                            // its second frame AFTER the reload on the SAME open
                            // stream and observe the echo from THIS backend —
                            // the mid-RPC-survives property.
                            let (btx, brx) = tokio::sync::mpsc::channel::<
                                Result<Frame<Bytes>, std::convert::Infallible>,
                            >(16);
                            tokio::spawn(async move {
                                let mut body = req.into_body();
                                let mut acc: Vec<u8> = Vec::new();
                                while let Some(Ok(frame)) = body.frame().await {
                                    if let Ok(data) = frame.into_data() {
                                        acc.extend_from_slice(&data);
                                        // Drain whole gRPC frames from `acc`.
                                        let mut i = 0usize;
                                        while i + 5 <= acc.len() {
                                            let len = u32::from_be_bytes([
                                                acc[i + 1],
                                                acc[i + 2],
                                                acc[i + 3],
                                                acc[i + 4],
                                            ])
                                                as usize;
                                            if i + 5 + len > acc.len() {
                                                break; // partial — wait for more
                                            }
                                            let payload = &acc[i + 5..i + 5 + len];
                                            let mut tagged = format!("{id}|").into_bytes();
                                            tagged.extend_from_slice(payload);
                                            let _ = btx
                                                .send(Ok(Frame::data(Bytes::from(grpc_frame(
                                                    &tagged,
                                                )))))
                                                .await;
                                            i += 5 + len;
                                        }
                                        acc.drain(..i);
                                    }
                                }
                                let mut trailers = hyper::HeaderMap::new();
                                trailers.insert(
                                    "grpc-status",
                                    hyper::header::HeaderValue::from_static("0"),
                                );
                                let _ = btx.send(Ok(Frame::trailers(trailers))).await;
                            });
                            let stream = futures_util::stream::unfold(brx, |mut rx| async move {
                                rx.recv().await.map(|f| (f, rx))
                            });
                            let body = BodyExt::boxed(StreamBody::new(stream));
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .status(200)
                                    .header("content-type", "application/grpc+proto")
                                    .header("x-backend-id", id)
                                    .body(body)
                                    .unwrap(),
                            )
                        });
                    let _ = builder.serve_connection(TokioIo::new(sock), svc).await;
                });
            }
        });
        (addr, h)
    }

    fn write_h1s_grpc_config(
        dir: &std::path::Path,
        listener_port: u16,
        backend: SocketAddr,
        cert: &std::path::Path,
        key: &std::path::Path,
    ) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{listener_port}"
protocol = "h1s"

[listeners.tls]
cert_path = "{cert}"
key_path = "{key}"

[[listeners.backends]]
address = "{backend}"
protocol = "h2"
weight = 1
"#,
            cert = cert.display(),
            key = key.display(),
        );
        let p = dir.join("gateway.toml");
        std::fs::write(&p, toml).unwrap();
        p
    }

    /// A rustls ClientConfig that accepts ANY server cert (self-signed
    /// test cert) and advertises ALPN h2.
    fn insecure_h2_client_config() -> std::sync::Arc<tokio_rustls::rustls::ClientConfig> {
        use tokio_rustls::rustls;
        #[derive(Debug)]
        struct NoVerify;
        impl rustls::client::danger::ServerCertVerifier for NoVerify {
            fn verify_server_cert(
                &self,
                _e: &rustls::pki_types::CertificateDer<'_>,
                _i: &[rustls::pki_types::CertificateDer<'_>],
                _s: &rustls::pki_types::ServerName<'_>,
                _o: &[u8],
                _n: rustls::pki_types::UnixTime,
            ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
                Ok(rustls::client::danger::ServerCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _m: &[u8],
                _c: &rustls::pki_types::CertificateDer<'_>,
                _d: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _m: &[u8],
                _c: &rustls::pki_types::CertificateDer<'_>,
                _d: &rustls::DigitallySignedStruct,
            ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error>
            {
                Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
                use rustls::SignatureScheme::*;
                vec![
                    RSA_PKCS1_SHA256,
                    RSA_PKCS1_SHA384,
                    RSA_PKCS1_SHA512,
                    ECDSA_NISTP256_SHA256,
                    ECDSA_NISTP384_SHA384,
                    RSA_PSS_SHA256,
                    RSA_PSS_SHA384,
                    RSA_PSS_SHA512,
                    ED25519,
                ]
            }
        }
        let mut cfg = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(NoVerify))
            .with_no_client_auth();
        cfg.alpn_protocols = vec![b"h2".to_vec()];
        std::sync::Arc::new(cfg)
    }

    /// Open a gRPC client-streaming request to the gateway over TLS+h2
    /// whose request body is fed from `body_rx` (so the client can send a
    /// frame BEFORE and a frame AFTER the SIGHUP on the SAME open h2
    /// stream). Returns a JoinHandle for the response: the gateway's H2
    /// path buffers a small request body before dialing the upstream, so
    /// `send_request` only resolves once the body ends — the caller drives
    /// the body channel (and drops it) BEFORE awaiting this handle. The
    /// backend the request reaches is the one CAPTURED in the h2_proxy
    /// snapshot at accept time (backend A), regardless of when the gateway
    /// dials it — that captured-snapshot routing across the reload is the
    /// property under test.
    async fn grpc_open(
        listener: SocketAddr,
        body_rx: tokio::sync::mpsc::Receiver<Bytes>,
    ) -> tokio::task::JoinHandle<Response<Incoming>> {
        use tokio_rustls::TlsConnector;
        let tcp = tokio::net::TcpStream::connect(listener).await.unwrap();
        let connector = TlsConnector::from(insecure_h2_client_config());
        let server_name =
            tokio_rustls::rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
        let tls = connector.connect(server_name, tcp).await.unwrap();
        let (mut sender, conn) = hyper::client::conn::http2::handshake::<_, _, ClientBody>(
            TokioExecutor::new(),
            TokioIo::new(tls),
        )
        .await
        .unwrap();
        tokio::spawn(async move {
            let _ = conn.await;
        });
        // Adapt the mpsc receiver into a Stream of body frames without a
        // `tokio-stream` dev-dep: unfold over the receiver, yielding one
        // `Frame::data` per channel message until the sender is dropped.
        let stream = futures_util::stream::unfold(body_rx, |mut rx| async move {
            rx.recv()
                .await
                .map(|b| (Ok::<_, std::convert::Infallible>(Frame::data(b)), rx))
        });
        let body = BodyExt::boxed(StreamBody::new(stream));
        let req = Request::builder()
            .method("POST")
            .uri("/echo.Service/Bidi")
            .header("content-type", "application/grpc+proto")
            .header("te", "trailers")
            .body(body)
            .unwrap();
        // Spawn the request so the caller can keep feeding the body channel
        // concurrently; the handle resolves to the response once headers
        // arrive (after the gateway buffers + forwards + backend replies).
        tokio::spawn(async move { sender.send_request(req).await.unwrap() })
    }

    /// R13 (in-flight gRPC RPC survives reload): open a gRPC streaming
    /// request over the h1s (TLS/h2) listener to backend A, send one
    /// frame, SIGHUP-swap the backend set to B, send a SECOND frame on the
    /// SAME open request stream, then close the request. The whole RPC
    /// must complete via backend A (the captured snapshot) — both frames
    /// echoed with the `A|` tag, x-backend-id=A, grpc-status: 0 — proving
    /// the mid-RPC stream was NOT reset or misrouted by the reload.
    #[tokio::test]
    async fn proof_grpc_rpc_survives_reload() {
        let Some(bin) = find_binary() else {
            eprintln!("SKIP: binary absent");
            return;
        };
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let (addr_a, _ja) = spawn_grpc_echo_backend("A").await;
        let (addr_b, _jb) = spawn_grpc_echo_backend("B").await;

        // Self-signed cert for the h1s listener.
        let dir0 = temp_dir("grpc");
        let generated =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".into()])
                .unwrap();
        let cert_p = dir0.join("g.crt");
        let key_p = dir0.join("g.key");
        std::fs::write(&cert_p, generated.cert.pem()).unwrap();
        std::fs::write(&key_p, generated.signing_key.serialize_pem()).unwrap();

        // Boot the h1s gateway on a retried fresh port.
        let mut guard = None;
        let mut lport = 0u16;
        let mut dir = PathBuf::new();
        for _ in 0..5 {
            let p = ephemeral_port();
            let laddr: SocketAddr = format!("127.0.0.1:{p}").parse().unwrap();
            let d = temp_dir("grpc");
            write_h1s_grpc_config(&d, p, addr_a, &cert_p, &key_p);
            let child = Command::new(&bin)
                .arg(d.join("gateway.toml"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env("RUST_LOG", "warn")
                .spawn()
                .expect("spawn");
            if tcp_ready(laddr, Duration::from_secs(10)).await {
                guard = Some(ChildGuard(child));
                lport = p;
                dir = d;
                break;
            }
            let mut c = child;
            let _ = c.kill();
            let _ = c.wait();
            std::fs::remove_dir_all(&d).ok();
        }
        let guard = guard.expect("h1s gateway bound");
        let listener: SocketAddr = format!("127.0.0.1:{lport}").parse().unwrap();

        // Open the gRPC client-streaming RPC; the h2 request stream stays
        // OPEN across the SIGHUP — frame 1 is sent now, frame 2 AFTER the
        // reload, on the SAME stream. The gateway's h2_proxy snapshot is
        // captured at accept (backend A); the whole RPC must therefore land
        // on A regardless of the reload.
        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(8);
        tx.send(Bytes::from(grpc_frame(b"one"))).await.unwrap();
        let resp_handle = grpc_open(listener, rx).await;

        // SIGHUP: swap the backend set to B while the RPC stream is OPEN.
        write_h1s_grpc_config(&dir, lport, addr_b, &cert_p, &key_p);
        sighup(&guard.0);
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Send frame 2 on the SAME open stream AFTER the reload, then end
        // the request body so the gateway forwards the (buffered) request.
        tx.send(Bytes::from(grpc_frame(b"two"))).await.unwrap();
        drop(tx);

        // The response must come from backend A (captured snapshot) with
        // both frames echoed and grpc-status: 0.
        let resp = tokio::time::timeout(Duration::from_secs(10), resp_handle)
            .await
            .expect("response did not arrive in time")
            .expect("request task panicked");
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("x-backend-id")
                .map(|v| v.to_str().unwrap()),
            Some("A"),
            "the RPC (stream open across the SIGHUP) must be served by backend A"
        );
        let collected = resp.into_body().collect().await.unwrap();
        let trailers = collected.trailers().cloned();
        let echoed = parse_grpc_payloads(&collected.to_bytes());
        assert!(
            echoed.iter().any(|p| p == "A|one"),
            "frame 1 must be echoed by backend A: {echoed:?}"
        );
        assert!(
            echoed.iter().any(|p| p == "A|two"),
            "frame 2 (sent AFTER the SIGHUP, same open stream) must STILL be echoed by \
             backend A — the mid-RPC stream survived the reload: {echoed:?}"
        );
        assert_eq!(
            trailers
                .as_ref()
                .and_then(|t| t.get("grpc-status"))
                .map(|v| v.to_str().unwrap()),
            Some("0"),
            "the RPC must complete with grpc-status 0 across the reload"
        );

        drop(guard);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir0).ok();
    }

    fn parse_grpc_payloads(buf: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i + 5 <= buf.len() {
            let len = u32::from_be_bytes([buf[i + 1], buf[i + 2], buf[i + 3], buf[i + 4]]) as usize;
            let start = i + 5;
            let end = (start + len).min(buf.len());
            out.push(String::from_utf8_lossy(&buf[start..end]).to_string());
            i = end;
        }
        out
    }

    // ── H2 backend-reload over h1s (TLS + ALPN h2 → H1 backend) ─────────
    //
    // The committed proof for the H2 reload path (the H2Proxy serve +
    // H2->H1 forward path). An ALPN-h2 client hits an h1s listener that
    // forwards to an H1 backend; a SIGHUP swaps the backend set; a NEW h2
    // connection observes the new backend. Exercises the H2Proxy serve
    // path deterministically (no throughput-fragile fcap timing), which
    // also restores the lb-l7/h2_proxy.rs coverage the llvm-cov-
    // instrumented fcap tests drop below charter under instrumentation.

    /// A plain HTTP/1.1 backend (async hyper) that tags every 200 with
    /// `x-backend-id: <id>`. The h1s listener's H2 leg forwards H2->H1 to
    /// it, so this drives the H2Proxy serve+forward path.
    async fn spawn_tagged_h1_backend(
        id: &'static str,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let l = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = l.local_addr().unwrap();
        let h = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = l.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    let builder = hyper::server::conn::http1::Builder::new();
                    let svc =
                        hyper::service::service_fn(move |_req: Request<Incoming>| async move {
                            let body = BodyExt::boxed(http_body_util::Full::new(
                                Bytes::from_static(b"ok"),
                            ));
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .status(200)
                                    .header("x-backend-id", id)
                                    .body(body)
                                    .unwrap(),
                            )
                        });
                    let _ = builder.serve_connection(TokioIo::new(sock), svc).await;
                });
            }
        });
        (addr, h)
    }

    fn write_h1s_h1backend_config(
        dir: &std::path::Path,
        listener_port: u16,
        backend: SocketAddr,
        cert: &std::path::Path,
        key: &std::path::Path,
    ) -> PathBuf {
        let toml = format!(
            r#"
[runtime]
drain_timeout_ms = 3000
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
            cert = cert.display(),
            key = key.display(),
        );
        let p = dir.join("gateway.toml");
        std::fs::write(&p, toml).unwrap();
        p
    }

    /// Issue one GET over a fresh TLS+ALPN-h2 connection to the gateway;
    /// return the `x-backend-id` response header (or None on failure).
    async fn h2_get_backend_id(listener: SocketAddr) -> Option<String> {
        use tokio_rustls::TlsConnector;
        let tcp = tokio::net::TcpStream::connect(listener).await.ok()?;
        let connector = TlsConnector::from(insecure_h2_client_config());
        let server_name =
            tokio_rustls::rustls::pki_types::ServerName::try_from("127.0.0.1").ok()?;
        let tls = connector.connect(server_name, tcp).await.ok()?;
        let (mut sender, conn) = hyper::client::conn::http2::handshake::<_, _, ClientBody>(
            TokioExecutor::new(),
            TokioIo::new(tls),
        )
        .await
        .ok()?;
        tokio::spawn(async move {
            let _ = conn.await;
        });
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(BodyExt::boxed(http_body_util::Empty::<Bytes>::new()))
            .ok()?;
        let resp = tokio::time::timeout(Duration::from_secs(5), sender.send_request(req))
            .await
            .ok()?
            .ok()?;
        resp.headers()
            .get("x-backend-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    /// R13 / D6: H2 backend hot-reload. An ALPN-h2 client hits an h1s
    /// listener forwarding to H1 backend A; SIGHUP swaps the backend set to
    /// B; a NEW h2 connection must observe backend B (reload took effect on
    /// the H2 serve path), proving H2 backend reload works AND covering the
    /// H2Proxy serve/forward lines deterministically.
    #[tokio::test]
    async fn proof_h2_backend_reload_takes_effect() {
        let Some(bin) = find_binary() else {
            eprintln!("SKIP: binary absent");
            return;
        };
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
        let (addr_a, _ja) = spawn_tagged_h1_backend("A").await;
        let (addr_b, _jb) = spawn_tagged_h1_backend("B").await;

        let dir0 = temp_dir("h2cov");
        let generated =
            rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".into()])
                .unwrap();
        let cert_p = dir0.join("g.crt");
        let key_p = dir0.join("g.key");
        std::fs::write(&cert_p, generated.cert.pem()).unwrap();
        std::fs::write(&key_p, generated.signing_key.serialize_pem()).unwrap();

        // Boot the h1s gateway on a retried fresh port.
        let mut guard = None;
        let mut lport = 0u16;
        let mut dir = PathBuf::new();
        for _ in 0..5 {
            let p = ephemeral_port();
            let laddr: SocketAddr = format!("127.0.0.1:{p}").parse().unwrap();
            let d = temp_dir("h2cov");
            write_h1s_h1backend_config(&d, p, addr_a, &cert_p, &key_p);
            let child = Command::new(&bin)
                .arg(d.join("gateway.toml"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env("RUST_LOG", "warn")
                .spawn()
                .expect("spawn");
            if tcp_ready(laddr, Duration::from_secs(10)).await {
                guard = Some(ChildGuard(child));
                lport = p;
                dir = d;
                break;
            }
            let mut c = child;
            let _ = c.kill();
            let _ = c.wait();
            std::fs::remove_dir_all(&d).ok();
        }
        let guard = guard.expect("h1s gateway bound");
        let listener: SocketAddr = format!("127.0.0.1:{lport}").parse().unwrap();

        // Baseline: an H2 request reaches backend A.
        assert_eq!(
            h2_get_backend_id(listener).await.as_deref(),
            Some("A"),
            "baseline H2 request must reach backend A"
        );

        // SIGHUP: swap the backend set A -> B.
        write_h1s_h1backend_config(&dir, lport, addr_b, &cert_p, &key_p);
        sighup(&guard.0);

        // A NEW h2 connection must observe backend B (reload took effect on
        // the H2 serve path).
        let mut took_effect = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if h2_get_backend_id(listener).await.as_deref() == Some("B") {
                took_effect = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            took_effect,
            "after SIGHUP a NEW H2 connection must reach backend B (H2 backend reload took effect)"
        );

        drop(guard);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir0).ok();
    }
}
