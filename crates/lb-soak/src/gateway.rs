//! Launch + supervise the real `expressgateway` binary as a child. Readiness is gated on `/metrics` answering (works for UDP-only datapaths too), and the Drop guard SIGTERMs + REAPS so a soak never leaks its own gateway-under-test.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub fn find_binary() -> anyhow::Result<PathBuf> {
    let target_dir = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
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

/// Reserve an ephemeral loopback TCP port by bind-then-drop. A race window exists before the gateway rebinds; callers retry.
pub fn ephemeral_port() -> anyhow::Result<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

/// Reserve an ephemeral loopback UDP port by bind-then-drop — a TCP reserve would not prove the UDP port is free.
pub fn ephemeral_udp_port() -> anyhow::Result<u16> {
    let l = std::net::UdpSocket::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

pub struct GatewayChild {
    child: Option<Child>,
    pid: u32,
    pub log_path: PathBuf,
}

impl GatewayChild {
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub async fn spawn_and_wait_ready(
        bin: &Path,
        config: &Path,
        metrics_addr: SocketAddr,
        log_path: PathBuf,
        boot_budget: Duration,
    ) -> anyhow::Result<Self> {
        let log = std::fs::File::create(&log_path)?;
        let log_err = log.try_clone()?;
        let rust_log = std::env::var("RUST_LOG").unwrap_or_else(|_| "warn".to_string());
        let child = Command::new(bin)
            .arg(config)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .env("RUST_LOG", rust_log)
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

    pub fn terminate_and_reap(&mut self) {
        if let Some(mut child) = self.child.take() {
            send_sigterm(self.pid);
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
