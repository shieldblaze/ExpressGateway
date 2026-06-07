//! S37-B — protocol-complete config: real-binary boot matrix + negative
//! boot proofs.
//!
//! POSITIVE boots (each SERVED listener protocol must boot + accept from
//! a VALID minimal TOML): tcp, h1, h1s, and quic (H3-terminate). These
//! drive the REAL `expressgateway` binary through its production config
//! path (`read_to_string` -> `parse_config` -> `validate_config` ->
//! listener spawn loop), proving that the byte-shape a `config/examples/`
//! file uses actually binds.
//!
//! NEGATIVE boots (the binary must REFUSE to start): an unserved listener
//! protocol (`h2`) and an unknown key (`deny_unknown_fields`). These
//! assert config-time rejection at the production entry point — the same
//! failure the operator gets, not just a unit test of `validate_config`.
//! The per-error-class negative coverage of `parse_config` /
//! `validate_config` lives in `crates/lb-config/src/lib.rs` unit tests;
//! these two prove the binary actually runs that validation at boot and
//! aborts (non-zero exit) rather than starting a half-configured gateway.
//!
//! Self-contained scaffolding mirrors
//! `tests/h3_s3_inflight_h1_drain_proof.rs` (find_binary / unique temp
//! dir / spawn + boot-wait) so this file couples to no other test.

#![cfg(unix)]

use std::io::Read;
use std::net::{SocketAddr, TcpListener as StdTcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

// ── minimal scaffolding (self-contained copy) ──────────────────────

fn find_binary() -> Result<PathBuf, String> {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(manifest).join("target")
        });
    for profile in ["release", "debug"] {
        let candidate = target_dir.join(profile).join("expressgateway");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "expressgateway binary not found under {}; run \
         `cargo build -p lb --bin expressgateway` first",
        target_dir.display()
    ))
}

fn ephemeral_port() -> u16 {
    let l = StdTcpListener::bind(("127.0.0.1", 0)).expect("ephemeral bind");
    let port = l.local_addr().expect("local_addr").port();
    drop(l);
    port
}

/// A real backend that holds a listening TCP port for the lifetime of the
/// test so the gateway's startup backend-resolve loop succeeds. We never
/// actually serve a request through it — these tests prove BOOT, not data
/// flow.
fn hold_backend() -> (StdTcpListener, SocketAddr) {
    let l = StdTcpListener::bind(("127.0.0.1", 0)).expect("backend bind");
    let addr = l.local_addr().expect("backend local_addr");
    (l, addr)
}

fn unique_temp_dir(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("eg-cfgboot-{tag}-{pid}-{nanos}-{seq}"));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

fn write_key_0600(path: &Path, pem: &str) {
    std::fs::write(path, pem).expect("write key");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).expect("chmod key 0600");
}

/// Generate a self-signed cert+key into `dir`, returning their paths.
fn gen_certs(dir: &Path) -> (PathBuf, PathBuf) {
    let generated =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_string(), "localhost".into()])
            .expect("self-signed cert");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    std::fs::write(&cert_path, generated.cert.pem()).expect("write cert");
    write_key_0600(&key_path, &generated.signing_key.serialize_pem());
    (cert_path, key_path)
}

fn boot_timeout() -> Duration {
    std::env::var("LB_TEST_BOOT_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(30))
}

/// Spawn the binary against `config` and wait until it accepts a TCP
/// connection on `addr` (the data-plane listener) — proving a full boot.
fn spawn_and_wait_tcp(bin: &Path, config: &Path, addr: SocketAddr) -> Child {
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
            return child;
        }
        // If the process already exited, fail fast with its output.
        if let Ok(Some(status)) = child.try_wait() {
            let mut out = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut out);
            }
            panic!("gateway exited during boot (status {status}); stderr:\n{out}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("gateway did not start accepting on {addr}");
}

/// Spawn the binary against a UDP/QUIC config and wait until the binary
/// logs readiness (there is no TCP listener to poll for the QUIC-only
/// case). We detect readiness via the "service open for traffic" line the
/// binary emits on STDOUT once every listener has bound. (The binary's
/// `lb_observability::init_tracing` installs a `tracing_subscriber::fmt`
/// subscriber, which writes to stdout by default; `LB_LOG_FORMAT=text` is
/// set so the substring is not JSON-escaped.)
fn spawn_and_wait_ready_log(bin: &Path, config: &Path) -> Child {
    use std::io::BufRead;
    let mut child = Command::new(bin)
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("RUST_LOG", "info")
        .env("LB_LOG_FORMAT", "text")
        .spawn()
        .expect("spawn expressgateway");
    let stdout = child.stdout.take().expect("piped stdout");
    let mut reader = std::io::BufReader::new(stdout);
    let deadline = Instant::now() + boot_timeout();
    let mut captured = String::new();
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF — process closed its stdout
            Ok(_) => {
                captured.push_str(&line);
                if line.contains("service open for traffic") || line.contains("probes flipped") {
                    return child;
                }
            }
            Err(_) => break,
        }
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            panic!(
                "gateway exited during boot (status {status});\nstdout:\n{captured}\nstderr:\n{err}"
            );
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    panic!("gateway did not reach readiness; stdout so far:\n{captured}");
}

fn kill(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Spawn the binary against an INVALID config and assert it exits
/// non-zero quickly (validation/parse aborts boot — no listener binds).
fn assert_boot_refused(bin: &Path, config: &Path) -> String {
    let mut child = Command::new(bin)
        .arg(config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("RUST_LOG", "info")
        .spawn()
        .expect("spawn expressgateway");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut out = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut out);
            }
            assert!(
                !status.success(),
                "binary must REFUSE an invalid config (non-zero exit); got success.\n{out}"
            );
            return out;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("binary did not exit on an invalid config within 30s (it must abort boot)");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ── POSITIVE boots — each served listener protocol ─────────────────

#[test]
fn boot_tcp_listener() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("tcp");
    let (_backend, backend_addr) = hold_backend();
    let port = ephemeral_port();
    let listen: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let toml = format!(
        r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{port}"
protocol = "tcp"

[[listeners.backends]]
address = "{backend_addr}"
weight = 1
"#
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    let child = spawn_and_wait_tcp(&bin, &cfg, listen);
    kill(child);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn boot_h1_listener() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("h1");
    let (_backend, backend_addr) = hold_backend();
    let port = ephemeral_port();
    let listen: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let toml = format!(
        r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{port}"
protocol = "h1"

[[listeners.backends]]
address = "{backend_addr}"
protocol = "h1"
weight = 1
"#
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    let child = spawn_and_wait_tcp(&bin, &cfg, listen);
    kill(child);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn boot_h1s_listener() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("h1s");
    let (_backend, backend_addr) = hold_backend();
    let (cert, key) = gen_certs(&dir);
    let port = ephemeral_port();
    let listen: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let toml = format!(
        r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{port}"
protocol = "h1s"

[listeners.tls]
cert_path = "{cert}"
key_path = "{key}"

[[listeners.backends]]
address = "{backend_addr}"
protocol = "h1"
weight = 1
"#,
        cert = cert.display(),
        key = key.display(),
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    // The TLS listener accepts a raw TCP connection (the TLS handshake
    // happens after accept), so the TCP-connect boot probe is sufficient.
    let child = spawn_and_wait_tcp(&bin, &cfg, listen);
    kill(child);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn boot_quic_h3_terminate_listener() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("quic");
    let (cert, key) = gen_certs(&dir);
    let port = ephemeral_port();
    let retry = dir.join("retry.secret");
    // H3-terminate is backend-less in the production binary (F-S26-1), so
    // no [[listeners.backends]] block — just the QUIC listener. The retry
    // secret file is minted on first boot when absent.
    let toml = format!(
        r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{port}"
protocol = "quic"

[listeners.quic]
cert_path = "{cert}"
key_path = "{key}"
retry_secret_path = "{retry}"
"#,
        cert = cert.display(),
        key = key.display(),
        retry = retry.display(),
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    let child = spawn_and_wait_ready_log(&bin, &cfg);
    kill(child);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn boot_quic_mode_b_listener() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("modeb");
    let (cert, key) = gen_certs(&dir);
    // A separate CA file for the backend-verify path. We reuse the same
    // self-signed cert as a stand-in CA bundle — boot only loads it, it
    // does not dial the backend at startup.
    let (ca, _ca_key) = (cert.clone(), key.clone());
    let port = ephemeral_port();
    let retry = dir.join("retry.secret");
    let backend = ephemeral_port();
    let toml = format!(
        r#"
[runtime]
drain_timeout_ms = 5000
readiness_settle_ms = 100

[[listeners]]
address = "127.0.0.1:{port}"
protocol = "quic"

[listeners.quic]
cert_path = "{cert}"
key_path = "{key}"
retry_secret_path = "{retry}"

[listeners.quic.raw_proxy]
backend_addr = "127.0.0.1:{backend}"
sni = "backend.test"
backend_ca_path = "{ca}"
"#,
        cert = cert.display(),
        key = key.display(),
        retry = retry.display(),
        ca = ca.display(),
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    let child = spawn_and_wait_ready_log(&bin, &cfg);
    kill(child);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── NEGATIVE boots — the binary must refuse to start ───────────────

#[test]
fn boot_refused_unserved_protocol() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("neg-proto");
    let (_backend, backend_addr) = hold_backend();
    let port = ephemeral_port();
    // `h2` is NOT a served listener protocol (S37-B). The binary must
    // abort at validation, not start a half-configured listener.
    let toml = format!(
        r#"
[[listeners]]
address = "127.0.0.1:{port}"
protocol = "h2"

[[listeners.backends]]
address = "{backend_addr}"
weight = 1
"#
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    let out = assert_boot_refused(&bin, &cfg);
    assert!(
        out.contains("validation") || out.contains("not a served"),
        "refusal should mention validation/served-protocol; stderr:\n{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn boot_refused_unknown_key() {
    let bin = match find_binary() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let dir = unique_temp_dir("neg-key");
    let (_backend, backend_addr) = hold_backend();
    let port = ephemeral_port();
    // A typo'd runtime key (`max_keepalv_requests`) — deny_unknown_fields
    // turns this into a parse-time abort instead of a silent drop.
    let toml = format!(
        r#"
[[listeners]]
address = "127.0.0.1:{port}"
protocol = "tcp"

[[listeners.backends]]
address = "{backend_addr}"
weight = 1

[runtime]
max_keepalv_requests = 5
"#
    );
    let cfg = dir.join("gateway.toml");
    std::fs::write(&cfg, toml).expect("write config");
    let out = assert_boot_refused(&bin, &cfg);
    assert!(
        out.contains("parse") || out.contains("unknown field") || out.contains("max_keepalv"),
        "refusal should mention the parse/unknown-field error; stderr:\n{out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
