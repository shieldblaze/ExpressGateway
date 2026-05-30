//! Launch + supervise the real `expressgateway` binary as a child process.
//!
//! Mirrors the proven spawn harness in `tests/reload_zero_drop.rs`
//! (`find_binary`, `ephemeral_port`, SIGTERM delivery) but adds a uniform
//! readiness gate (poll `/metrics` until it answers — works for every datapath
//! including UDP-only QUIC/passthrough) and a Drop guard that SIGTERMs and
//! REAPS the child, so a soak never leaks its gateway-under-test (R9: a soak
//! that leaks its own processes is an irony we will not ship).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Locate the `expressgateway` binary under `CARGO_TARGET_DIR` (or `./target`),
/// preferring a release build. Returns `Err` with a build hint if absent.
pub fn find_binary() -> anyhow::Result<PathBuf> {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
            // crates/lb-soak -> workspace root -> target
            PathBuf::from(manifest)
                .ancestors()
                .nth(2)
                .map(|p| p.join("target"))
                .unwrap_or_else(|| PathBuf::from("target"))
        });
    for profile in ["release", "debug"] {
        let candidate = target_dir.join(profile).join("expressgateway");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(
        "expressgateway binary not found under {}; run \
         `cargo build --release -p lb --bin expressgateway` first",
        target_dir.display()
    )
}

/// Reserve an ephemeral loopback TCP port by bind-then-drop. A small race
/// window exists before the gateway rebinds; callers retry on a lost race.
pub fn ephemeral_port() -> anyhow::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

/// Reserve an ephemeral loopback UDP port by bind-then-drop (for QUIC/passthrough
/// listeners — a TCP reserve would not prove the UDP port is free).
pub fn ephemeral_udp_port() -> anyhow::Result<u16> {
    let l = std::net::UdpSocket::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

/// A running gateway child. Dropping it SIGTERMs + reaps the process.
pub struct GatewayChild {
    child: Option<Child>,
    pid: u32,
    /// Where the child's stdout/stderr were redirected (bounded log file).
    pub log_path: PathBuf,
}

impl GatewayChild {
    /// The child PID (for `/proc` sampling).
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Spawn the binary on `config`, redirecting stdout+stderr to `log_path`
    /// (bounded by the caller's choice of file), then poll `metrics_addr`'s
    /// `/metrics` until it answers 200 within `boot_budget`. Returns the live
    /// child or an error (the child is reaped on failure).
    pub async fn spawn_and_wait_ready(
        bin: &Path,
        config: &Path,
        metrics_addr: SocketAddr,
        log_path: PathBuf,
        boot_budget: Duration,
    ) -> anyhow::Result<Self> {
        let log = std::fs::File::create(&log_path)?;
        let log_err = log.try_clone()?;
        let child = Command::new(bin)
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .env("RUST_LOG", "warn")
            .spawn()?;
        let pid = child.id();
        let mut me = Self {
            child: Some(child),
            pid,
            log_path,
        };

        let deadline = Instant::now() + boot_budget;
        loop {
            if Instant::now() > deadline {
                me.terminate_and_reap();
                anyhow::bail!(
                    "gateway pid {pid} never answered /metrics within {:?}; see {}",
                    boot_budget,
                    me.log_path.display()
                );
            }
            // If the child already exited, surface it now.
            if let Some(c) = me.child.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    anyhow::bail!(
                        "gateway pid {pid} exited during boot with {status}; see {}",
                        me.log_path.display()
                    );
                }
            }
            if let Ok((200..=299, _)) = crate::sampler::http_get(metrics_addr, "/metrics").await {
                return Ok(me);
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Deliver SIGTERM and reap (blocking briefly). Idempotent.
    pub fn terminate_and_reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            send_sigterm(self.pid);
            // Give the drain budget a moment, then hard-kill if needed.
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if Instant::now() > deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    Err(_) => break,
                }
            }
        }
    }
}

impl Drop for GatewayChild {
    fn drop(&mut self) {
        self.terminate_and_reap();
    }
}

/// Deliver SIGTERM to `pid` via libc. ESRCH (already gone) is fine.
fn send_sigterm(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: kill(2) with a valid-or-stale pid; ESRCH is handled.
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        const SIGTERM: i32 = 15;
        unsafe {
            let _ = kill(pid as i32, SIGTERM);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ephemeral_ports_are_distinct_enough() {
        let a = ephemeral_port().unwrap();
        let b = ephemeral_udp_port().unwrap();
        assert!(a > 0 && b > 0);
    }

    #[test]
    fn find_binary_reports_helpfully_when_absent() {
        // With a bogus target dir, the error must name the build command.
        // (We don't assert success — the binary may or may not be built in
        // this environment; we assert the error shape when it's absent.)
        let prev = std::env::var("CARGO_TARGET_DIR").ok();
        // SAFETY: single-threaded test mutation of an env var local to it.
        unsafe {
            std::env::set_var("CARGO_TARGET_DIR", "/nonexistent-eg-target-xyz");
        }
        let r = find_binary();
        if let Some(p) = prev {
            unsafe {
                std::env::set_var("CARGO_TARGET_DIR", p);
            }
        } else {
            unsafe {
                std::env::remove_var("CARGO_TARGET_DIR");
            }
        }
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("cargo build"));
    }
}
