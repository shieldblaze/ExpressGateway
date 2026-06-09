//! `eg-bench` — the S39 closed-loop perf characterization driver (R12).
//!
//! Sets up the SAME real `expressgateway` binary + backends/config the soak uses
//! (reusing `lb_soak::{gateway,backends,config_gen}`), then drives one protocol
//! path at a fixed concurrency for a fixed window, timing every request into a
//! latency distribution (`lb_soak::bench`). Reports achieved RPS + p50/p99/p999
//! + the gateway child's resource cost (RSS/fd/CPU%) at load.
//!
//! Usage:
//!   eg-bench --protocol <P> --connections <C> --duration-secs <D> \
//!            [--warmup-secs <W=5>] [--payload <bytes=0>] [--out <dir>] [--label <s>]
//!   P ∈ { h1, h2, h3, quic_modea, ws_h1 }
//!
//! Stability/teardown: the gateway child is reaped on drop (SIGTERM); backends
//! stop on their flag — no process outlives the run (R9).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::too_many_arguments)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use lb_soak::backends::{self, BackendControl};
use lb_soak::bench::{self, BenchSummary};
use lb_soak::config_gen;
use lb_soak::gateway::{self, GatewayChild};
use lb_soak::procstat;

const BOOT_BUDGET: Duration = Duration::from_secs(30);
/// Linux USER_HZ (clock ticks/sec) — 100 on x86_64 Linux (the measurement box).
const USER_HZ: f64 = 100.0;

struct Args {
    protocol: String,
    connections: usize,
    duration_secs: u64,
    warmup_secs: u64,
    payload: usize,
    out: Option<PathBuf>,
    label: Option<String>,
    /// When nonzero, run "serve only": set up gateway+backend, print the
    /// listener (so an external tool — oha — can drive it for the H1/H1s/H2
    /// cross-validation), idle for N secs, tear down. No internal load is run.
    serve_secs: u64,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut protocol = None;
    let mut connections = 32usize;
    let mut duration_secs = 30u64;
    let mut warmup_secs = 5u64;
    let mut payload = 0usize;
    let mut out = None;
    let mut label = None;
    let mut serve_secs = 0u64;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--protocol" => protocol = it.next(),
            "--connections" | "-c" => {
                connections = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(connections)
            }
            "--duration-secs" => {
                duration_secs = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(duration_secs)
            }
            "--warmup-secs" => {
                warmup_secs = it
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(warmup_secs)
            }
            "--payload" => payload = it.next().and_then(|v| v.parse().ok()).unwrap_or(payload),
            "--out" => out = it.next().map(PathBuf::from),
            "--label" => label = it.next(),
            "--serve-secs" => serve_secs = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--list" => {
                println!("protocols: h1 h2 h3 quic_modea ws_h1");
                std::process::exit(0);
            }
            other => anyhow::bail!("unknown arg {other} (try --list)"),
        }
    }
    Ok(Args {
        protocol: protocol.ok_or_else(|| anyhow::anyhow!("--protocol required (or --list)"))?,
        connections,
        duration_secs,
        warmup_secs,
        payload,
        out,
        label,
        serve_secs,
    })
}

fn metrics_addr() -> anyhow::Result<SocketAddr> {
    Ok(format!("127.0.0.1:{}", gateway::ephemeral_port()?).parse()?)
}
fn tcp_addr(port: u16) -> SocketAddr {
    format!("127.0.0.1:{port}")
        .parse()
        .expect("loopback addr parse")
}

/// One resource sample of the gateway child.
#[derive(Clone, Copy)]
struct ResSample {
    elapsed: f64,
    ticks: u64,
    rss_kb: u64,
    vmhwm_kb: u64,
    fds: u64,
    threads: u64,
}

/// Read utime+stime (clock ticks) from `/proc/<pid>/stat`. Robust to a `comm`
/// containing spaces/parens (split on the final ')').
fn read_proc_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rparen = stat.rfind(')')?;
    let rest = stat.get(rparen + 2..)?;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // After comm: state(0) ppid(1) ... utime(11) stime(12)
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    Some(utime + stime)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args = parse_args()?;
    let bin = gateway::find_binary()?;
    let label = args.label.clone().unwrap_or_else(|| args.protocol.clone());
    let workdir = std::env::temp_dir().join(format!(
        "eg-bench-{}-{}-{}",
        args.protocol,
        args.connections,
        std::process::id()
    ));
    std::fs::create_dir_all(&workdir)?;

    eprintln!(
        "[eg-bench] protocol={} conns={} duration={}s warmup={}s payload={} bin={}",
        args.protocol,
        args.connections,
        args.duration_secs,
        args.warmup_secs,
        args.payload,
        bin.display()
    );

    // Keep backends + their control/stop flags alive for the whole run.
    let mut ctrls: Vec<Arc<BackendControl>> = Vec::new();
    let mut stops: Vec<Arc<AtomicBool>> = Vec::new();

    let metrics = metrics_addr()?;
    let cfg = workdir.join("gateway.toml");

    // Per-protocol setup: produce (target, optional sni, optional ca).
    enum Target {
        Tcp(SocketAddr),
        Quic {
            target: SocketAddr,
            sni: String,
            ca: PathBuf,
        },
    }

    let target: Target = match args.protocol.as_str() {
        "h1" => {
            let ctrl = BackendControl::new();
            let backend = backends::spawn_h1_backend(Arc::clone(&ctrl)).await?;
            ctrls.push(ctrl);
            let listener = tcp_addr(gateway::ephemeral_port()?);
            std::fs::write(&cfg, config_gen::h1_front(listener, backend, "h1", metrics))?;
            Target::Tcp(listener)
        }
        "h2" => {
            let ctrl = BackendControl::new();
            let backend = backends::spawn_h2_backend(Arc::clone(&ctrl)).await?;
            ctrls.push(ctrl);
            let listener = tcp_addr(gateway::ephemeral_port()?);
            let certs = config_gen::generate_certs(&workdir, "soak-front")?;
            std::fs::write(
                &cfg,
                config_gen::h1s_front(listener, backend, "h2", metrics, &certs),
            )?;
            Target::Quic {
                target: listener,
                sni: "soak-front".to_string(),
                ca: certs.ca,
            }
        }
        "h3" => {
            let ctrl = BackendControl::new();
            let backend = backends::spawn_h2_backend(Arc::clone(&ctrl)).await?;
            ctrls.push(ctrl);
            let listener = tcp_addr(gateway::ephemeral_udp_port()?);
            let certs = config_gen::generate_certs(&workdir, "soak-front")?;
            let retry = workdir.join("retry.bin");
            std::fs::write(
                &cfg,
                config_gen::quic_h3_terminate_h2(listener, backend, metrics, &certs, &retry),
            )?;
            Target::Quic {
                target: listener,
                sni: "soak-front".to_string(),
                ca: certs.ca,
            }
        }
        "quic_modea" => {
            let backend_dir = workdir.join("backend");
            std::fs::create_dir_all(&backend_dir)?;
            let backend_certs = config_gen::generate_certs(&backend_dir, "soak-backend")?;
            let stop = Arc::new(AtomicBool::new(false));
            let backend = backends::spawn_quic_echo_backend(
                backend_certs.cert.clone(),
                backend_certs.key.clone(),
                Arc::clone(&stop),
            )?;
            stops.push(stop);
            let listener = tcp_addr(gateway::ephemeral_udp_port()?);
            let retry = workdir.join("retry.bin");
            std::fs::write(
                &cfg,
                config_gen::passthrough_mode_a(listener, backend, metrics, &retry, 100_000, 60_000),
            )?;
            Target::Quic {
                target: listener,
                sni: "soak-backend".to_string(),
                ca: backend_certs.ca,
            }
        }
        "ws_h1" => {
            let stop = Arc::new(AtomicBool::new(false));
            let backend = backends::spawn_ws_h1_backend(Arc::clone(&stop)).await?;
            stops.push(stop);
            let listener = tcp_addr(gateway::ephemeral_port()?);
            std::fs::write(
                &cfg,
                config_gen::h1_front_ws(listener, backend, metrics, 3600, 3600),
            )?;
            Target::Tcp(listener)
        }
        other => anyhow::bail!("unknown protocol {other} (try --list)"),
    };

    let mut gw = GatewayChild::spawn_and_wait_ready(
        &bin,
        &cfg,
        metrics,
        workdir.join("gateway.log"),
        BOOT_BUDGET,
    )
    .await?;
    let gw_pid = gw.pid();
    eprintln!("[eg-bench] gateway ready pid={gw_pid} metrics={metrics}");

    // Serve-only mode (oha cross-validation for H1/H1s/H2): print the listener
    // + (for TLS/QUIC) SNI/CA, idle, then tear down. No internal load.
    if args.serve_secs > 0 {
        match &target {
            Target::Tcp(addr) => println!("LISTENER={addr} PID={gw_pid}"),
            Target::Quic { target, sni, ca } => {
                println!(
                    "LISTENER={target} SNI={sni} CA={} PID={gw_pid}",
                    ca.display()
                );
            }
        }
        println!("READY");
        tokio::time::sleep(Duration::from_secs(args.serve_secs)).await;
        gw.terminate_and_reap();
        for s in &stops {
            s.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        for c in &ctrls {
            c.stop();
        }
        let _ = std::fs::remove_dir_all(&workdir);
        return Ok(());
    }

    // Background resource sampler on the gateway child.
    let samples: Arc<std::sync::Mutex<Vec<ResSample>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let sampler_stop = Arc::new(AtomicBool::new(false));
    let s_samples = Arc::clone(&samples);
    let s_stop = Arc::clone(&sampler_stop);
    let sampler_t0 = Instant::now();
    let sampler = tokio::spawn(async move {
        while !s_stop.load(std::sync::atomic::Ordering::Relaxed) {
            if let Some(fp) = procstat::sample_pid(gw_pid) {
                let ticks = read_proc_ticks(gw_pid).unwrap_or(0);
                if let Ok(mut v) = s_samples.lock() {
                    v.push(ResSample {
                        elapsed: sampler_t0.elapsed().as_secs_f64(),
                        ticks,
                        rss_kb: fp.rss_kb,
                        vmhwm_kb: fp.vmhwm_kb,
                        fds: fp.fds,
                        threads: fp.threads,
                    });
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });

    let warmup = Duration::from_secs(args.warmup_secs);
    let measure = Duration::from_secs(args.duration_secs);
    let run_t0 = Instant::now();
    let run = match &target {
        Target::Tcp(addr) if args.protocol == "h1" => {
            bench::bench_h1(*addr, args.connections, warmup, measure, args.payload).await
        }
        Target::Tcp(addr) if args.protocol == "ws_h1" => {
            bench::bench_ws_h1(*addr, args.connections, warmup, measure, args.payload).await
        }
        Target::Quic { target, sni, ca } if args.protocol == "h2" => {
            bench::bench_h2(
                *target,
                sni.clone(),
                ca.clone(),
                args.connections,
                warmup,
                measure,
                args.payload,
            )
            .await
        }
        Target::Quic { target, sni, ca } if args.protocol == "h3" => {
            bench::bench_h3(
                *target,
                sni.clone(),
                ca.clone(),
                args.connections,
                warmup,
                measure,
                args.payload,
            )
            .await
        }
        Target::Quic { target, sni, ca } if args.protocol == "quic_modea" => {
            bench::bench_quic_modea(
                *target,
                sni.clone(),
                ca.clone(),
                args.connections,
                warmup,
                measure,
                args.payload,
            )
            .await
        }
        _ => anyhow::bail!("protocol/target mismatch for {}", args.protocol),
    };
    let measured_wall = run_t0.elapsed().as_secs_f64() - args.warmup_secs as f64;

    // Stop the sampler + tear down.
    sampler_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = sampler.await;
    gw.terminate_and_reap();
    for s in &stops {
        s.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    for c in &ctrls {
        c.stop();
    }

    // Resource stats over the MEASURE window (drop warmup-prefix samples).
    let res = {
        let v = samples.lock().map(|g| g.clone()).unwrap_or_default();
        let warm = args.warmup_secs as f64;
        let win: Vec<&ResSample> = v.iter().filter(|s| s.elapsed >= warm).collect();
        let max_rss = win.iter().map(|s| s.rss_kb).max().unwrap_or(0);
        let max_vmhwm = win.iter().map(|s| s.vmhwm_kb).max().unwrap_or(0);
        let max_fds = win.iter().map(|s| s.fds).max().unwrap_or(0);
        let max_thr = win.iter().map(|s| s.threads).max().unwrap_or(0);
        // CPU% across the window: tick delta / USER_HZ / wall * 100 (can exceed
        // 100 = multi-core).
        let cpu_pct = match (win.first(), win.last()) {
            (Some(a), Some(b)) if b.elapsed > a.elapsed => {
                let dt = b.elapsed - a.elapsed;
                let dticks = b.ticks.saturating_sub(a.ticks) as f64;
                (dticks / USER_HZ) / dt * 100.0
            }
            _ => 0.0,
        };
        (max_rss, max_vmhwm, max_fds, max_thr, cpu_pct)
    };

    let summary = BenchSummary::from_run(
        &args.protocol,
        args.connections,
        args.payload,
        args.warmup_secs as f64,
        measured_wall.max(0.0),
        run,
    );

    println!("{}", summary.human());
    println!(
        "  gateway: cpu={:.0}% rss_max={:.1}MB vmhwm_max={:.1}MB fds_max={} threads_max={}",
        res.4,
        res.0 as f64 / 1024.0,
        res.1 as f64 / 1024.0,
        res.2,
        res.3
    );

    if let Some(dir) = &args.out {
        std::fs::create_dir_all(dir)?;
        let json = format!(
            "{{\"summary\":{s},\"gateway\":{{\"cpu_pct\":{cpu:.1},\"rss_max_kb\":{rss},\
             \"vmhwm_max_kb\":{vm},\"fds_max\":{fds},\"threads_max\":{thr}}}}}",
            s = summary.to_json(),
            cpu = res.4,
            rss = res.0,
            vm = res.1,
            fds = res.2,
            thr = res.3,
        );
        let path = dir.join(format!("{label}-c{}.json", args.connections));
        std::fs::write(&path, json)?;
        eprintln!("[eg-bench] wrote {}", path.display());
    }

    // Best-effort workdir cleanup (certs/logs only — small).
    let _ = std::fs::remove_dir_all(&workdir);
    Ok(())
}
