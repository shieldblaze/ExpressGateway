//! `eg-soak` — the chaos/soak scenario orchestrator. Runs ONE scenario for a fixed duration against
//! the real `expressgateway` binary, then writes the CSV + `verdict.json` + `summary.txt` + a
//! `soak_complete.marker`.
//!
//!   eg-soak --scenario <name> --duration-secs N [--sample-secs S] [--scale K] --out <dir>
//!   eg-soak --list        (the authoritative scenario list)

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use lb_soak::backends::{self, BackendControl};
use lb_soak::chaos;
use lb_soak::config_gen;
use lb_soak::gateway::{self, GatewayChild};
use lb_soak::loadgen::{self, LoadStats};
use lb_soak::sampler;
use lb_soak::timeseries::{MetricKind, TrendConfig, Verdict};

const SCENARIOS: &[&str] = &[
    "sc1_h1h1",
    "sc1b_h1h2",
    "sc2_h2h2",
    "sc3_slowloris",
    "sc4_modeb",
    "sc5_modea",
    "sc6_413teardown",
    "sc7_h3terminate",
    "sc8_ws_h1",
    "sc8b_ws_h2",
    "sc8c_ws_h3",
    "sc9_grpc_h3",
];

struct Args {
    scenario: String,
    /// Output-file prefix (defaults to `scenario`); lets the same scenario run under two configs
    /// (e.g. Mode B 4-stream vs 1-stream) into one out dir.
    label: String,
    duration: u64,
    sample: u64,
    scale: usize,
    out: PathBuf,
}

fn parse_args() -> anyhow::Result<Option<Args>> {
    let mut scenario = None;
    let mut label = None;
    let mut duration = 120u64;
    let mut sample = 15u64;
    let mut scale = 1usize;
    let mut out = PathBuf::from("soak-out");
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--list" => {
                for s in SCENARIOS {
                    println!("{s}");
                }
                return Ok(None);
            }
            "--scenario" => scenario = it.next(),
            "--label" => label = it.next(),
            "--duration-secs" => duration = it.next().unwrap_or_default().parse().unwrap_or(120),
            "--sample-secs" => sample = it.next().unwrap_or_default().parse().unwrap_or(15),
            "--scale" => scale = it.next().unwrap_or_default().parse().unwrap_or(1).max(1),
            "--out" => out = PathBuf::from(it.next().unwrap_or_default()),
            other => anyhow::bail!("unknown arg {other}"),
        }
    }
    let scenario = scenario.ok_or_else(|| anyhow::anyhow!("--scenario required (or --list)"))?;
    let label = label.unwrap_or_else(|| scenario.clone());
    Ok(Some(Args {
        scenario,
        label,
        duration,
        sample,
        scale,
        out,
    }))
}

struct Running {
    gateway: GatewayChild,
    metrics_addr: SocketAddr,
    gauges: Vec<String>,
    kinds: Vec<(String, MetricKind)>,
    tasks: Vec<JoinHandle<()>>,
    backend_ctrls: Vec<Arc<BackendControl>>,
    quic_stop: Vec<Arc<AtomicBool>>,
    stats: Vec<(String, Arc<LoadStats>)>,
    _tmp: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async_main())
}

async fn async_main() -> anyhow::Result<()> {
    let Some(args) = parse_args()? else {
        return Ok(());
    };
    let _ = rustls::crypto::ring::default_provider().install_default();

    std::fs::create_dir_all(&args.out)?;
    let workdir = args.out.join(format!("{}-work", args.label));
    std::fs::create_dir_all(&workdir)?;

    let bin = gateway::find_binary()?;
    let cancel = CancellationToken::new();

    eprintln!(
        "[eg-soak] scenario={} duration={}s sample={}s scale={} bin={}",
        args.scenario,
        args.duration,
        args.sample,
        args.scale,
        bin.display()
    );

    let mut running = setup_scenario(&args, &bin, &workdir, cancel.clone()).await?;
    let pid = running.gateway.pid();
    eprintln!(
        "[eg-soak] gateway ready pid={pid} metrics={} — soaking {}s",
        running.metrics_addr, args.duration
    );

    let dur = Duration::from_secs(args.duration);
    let cancel_timer = cancel.clone();
    let timer = tokio::spawn(async move {
        tokio::time::sleep(dur).await;
        cancel_timer.cancel();
    });

    let ts = sampler::run_sampler(
        pid,
        running.metrics_addr,
        running.gauges.clone(),
        Duration::from_secs(args.sample),
        cancel.clone(),
        &args.label,
    )
    .await;

    timer.abort();
    for t in running.tasks.drain(..) {
        let _ = tokio::time::timeout(Duration::from_secs(8), t).await;
    }
    for c in &running.backend_ctrls {
        c.stop();
    }
    for s in &running.quic_stop {
        s.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    running.gateway.terminate_and_reap();

    let verdicts = ts.analyze(&TrendConfig::default(), &running.kinds);
    let overall_drift = verdicts.iter().any(|v| v.verdict == Verdict::Drift);

    let csv_path = args.out.join(format!("{}.csv", args.label));
    std::fs::write(&csv_path, ts.to_csv())?;

    let summary = render_summary(&args, &ts, &verdicts, &running.stats, overall_drift);
    std::fs::write(
        args.out.join(format!("{}.summary.txt", args.label)),
        &summary,
    )?;
    print!("{summary}");

    let json = render_json(&args, &ts, &verdicts, &running.stats, overall_drift);
    std::fs::write(
        args.out.join(format!("{}.verdict.json", args.label)),
        serde_json::to_string_pretty(&json)?,
    )?;

    std::fs::write(
        args.out
            .join(format!("{}.soak_complete.marker", args.label)),
        format!(
            "scenario={} duration_secs={} samples={} overall={}\n",
            args.scenario,
            args.duration,
            ts.len(),
            if overall_drift { "DRIFT" } else { "BOUNDED" }
        ),
    )?;

    eprintln!(
        "[eg-soak] DONE scenario={} samples={} overall={}",
        args.scenario,
        ts.len(),
        if overall_drift {
            "DRIFT(finding)"
        } else {
            "BOUNDED"
        }
    );
    Ok(())
}

/// Boot budget for the gateway child (cold release start on a busy box).
const BOOT_BUDGET: Duration = Duration::from_secs(40);

async fn setup_scenario(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    match args.scenario.as_str() {
        "sc1_h1h1" => setup_h1(args, bin, workdir, cancel, "h1").await,
        "sc1b_h1h2" => setup_h1(args, bin, workdir, cancel, "h2").await,
        "sc2_h2h2" => setup_h2h2(args, bin, workdir, cancel).await,
        "sc3_slowloris" => setup_slowloris(args, bin, workdir, cancel).await,
        "sc4_modeb" => setup_quic(args, bin, workdir, cancel, true).await,
        "sc5_modea" => setup_quic(args, bin, workdir, cancel, false).await,
        "sc6_413teardown" => setup_413(args, bin, workdir, cancel).await,
        "sc7_h3terminate" => setup_h3_terminate(args, bin, workdir, cancel).await,
        "sc8_ws_h1" => setup_ws_h1(args, bin, workdir, cancel).await,
        "sc8b_ws_h2" => setup_ws_h2(args, bin, workdir, cancel).await,
        "sc8c_ws_h3" => setup_ws_h3(args, bin, workdir, cancel).await,
        "sc9_grpc_h3" => setup_grpc_h3(args, bin, workdir, cancel).await,
        other => anyhow::bail!("unknown scenario {other} (try --list)"),
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(default)
}

fn metrics_addr() -> anyhow::Result<SocketAddr> {
    Ok(format!("127.0.0.1:{}", gateway::ephemeral_port()?).parse()?)
}

fn tcp_addr(port: u16) -> SocketAddr {
    format!("127.0.0.1:{port}").parse().expect("addr")
}

/// sc1 / sc1b — H1 front → H1 or H2 backend, sustained keep-alive + churn + connection-flood.
async fn setup_h1(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
    backend_proto: &str,
) -> anyhow::Result<Running> {
    let ctrl = BackendControl::new();
    let backend = if backend_proto == "h2" {
        backends::spawn_h2_backend(Arc::clone(&ctrl)).await?
    } else {
        backends::spawn_h1_backend(Arc::clone(&ctrl)).await?
    };
    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_port()?);
    let toml = config_gen::h1_front(listener, backend, backend_proto, metrics);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let mut tasks = Vec::new();
    let mut stats = Vec::new();
    let load = LoadStats::new();
    stats.push(("h1_load".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_h1_load(
        listener,
        6 * args.scale,
        Arc::clone(&load),
        cancel.clone(),
    )));
    let flood = LoadStats::new();
    stats.push(("conn_flood".into(), Arc::clone(&flood)));
    tasks.push(tokio::spawn(chaos::run_conn_flood(
        listener,
        4 * args.scale,
        Arc::clone(&flood),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: tcp_gauges(),
        kinds: tcp_kinds(),
        tasks,
        backend_ctrls: vec![ctrl],
        quic_stop: vec![],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc2 — H2-over-TLS front → H2 backend, sustained H2 + rapid-reset + stream-flood.
async fn setup_h2h2(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ctrl = BackendControl::new();
    let backend = backends::spawn_h2_backend(Arc::clone(&ctrl)).await?;
    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_port()?);
    let certs = config_gen::generate_certs(workdir, "localhost")?;
    let toml = config_gen::h1s_front(listener, backend, "h2", metrics, &certs);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sni = "localhost".to_string();
    let mut tasks = Vec::new();
    let mut stats = Vec::new();
    let load = LoadStats::new();
    stats.push(("h2_load".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_h2_load(
        listener,
        sni.clone(),
        certs.ca.clone(),
        4 * args.scale,
        Arc::clone(&load),
        cancel.clone(),
    )));
    let rr = LoadStats::new();
    stats.push(("rapid_reset".into(), Arc::clone(&rr)));
    tasks.push(tokio::spawn(chaos::run_rapid_reset(
        listener,
        sni.clone(),
        certs.ca.clone(),
        2 * args.scale,
        Arc::clone(&rr),
        cancel.clone(),
    )));
    tasks.push(tokio::spawn(chaos::run_stream_flood(
        listener,
        sni,
        certs.ca.clone(),
        args.scale,
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: tcp_gauges(),
        kinds: tcp_kinds(),
        tasks,
        backend_ctrls: vec![ctrl],
        quic_stop: vec![],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc3 — slowloris + slow-POST against an H1 front (low healthy baseline).
async fn setup_slowloris(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ctrl = BackendControl::new();
    let backend = backends::spawn_h1_backend(Arc::clone(&ctrl)).await?;
    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_port()?);
    let toml = config_gen::h1_front(listener, backend, "h1", metrics);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let mut tasks = Vec::new();
    let mut stats = Vec::new();
    tasks.push(tokio::spawn(chaos::run_slowloris(
        listener,
        96 * args.scale,
        cancel.clone(),
    )));
    tasks.push(tokio::spawn(chaos::run_slow_post(
        listener,
        48 * args.scale,
        cancel.clone(),
    )));
    let base = LoadStats::new();
    stats.push(("h1_baseline".into(), Arc::clone(&base)));
    tasks.push(tokio::spawn(loadgen::run_h1_load(
        listener,
        4,
        Arc::clone(&base),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: tcp_gauges(),
        kinds: tcp_kinds(),
        tasks,
        backend_ctrls: vec![ctrl],
        quic_stop: vec![],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc4 (Mode B terminate) / sc5 (Mode A passthrough) — QUIC load + datagram flood against a real
/// QUIC echo backend.
async fn setup_quic(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
    mode_b: bool,
) -> anyhow::Result<Running> {
    let backend_dir = workdir.join("backend");
    std::fs::create_dir_all(&backend_dir)?;
    let backend_certs = config_gen::generate_certs(&backend_dir, "soak-backend")?;
    let quic_stop = Arc::new(AtomicBool::new(false));
    let backend = backends::spawn_quic_echo_backend(
        backend_certs.cert.clone(),
        backend_certs.key.clone(),
        Arc::clone(&quic_stop),
    )?;

    let metrics = metrics_addr()?;
    let listen = tcp_addr(gateway::ephemeral_udp_port()?);

    // Cert the client must trust, and the SNI it sends, differ by mode.
    let (client_ca, client_sni): (PathBuf, String);
    let cfg = workdir.join("gateway.toml");
    let (gauges, kinds) = if mode_b {
        let front_certs = config_gen::generate_certs(workdir, "soak-front")?;
        let retry = workdir.join("retry.bin");
        let toml = config_gen::quic_mode_b(
            listen,
            backend,
            "soak-backend",
            metrics,
            &front_certs,
            &retry,
            &backend_certs.ca,
            1024,
            256,
        );
        std::fs::write(&cfg, toml)?;
        client_ca = front_certs.ca.clone();
        client_sni = "soak-front".to_string();
        (modeb_gauges(), modeb_kinds())
    } else {
        // Mode A: end-to-end; client trusts the BACKEND cert. F-S20-2: a short idle-flow reaper
        // window so reclamation is visible within a bounded soak (product default 60s).
        let retry = workdir.join("retry.bin");
        let idle_ms = env_usize("QUIC_FLOW_IDLE_MS", 10_000) as u64;
        let toml =
            config_gen::passthrough_mode_a(listen, backend, metrics, &retry, 100_000, idle_ms);
        std::fs::write(&cfg, toml)?;
        client_ca = backend_certs.ca.clone();
        client_sni = "soak-backend".to_string();
        (modea_gauges(), modea_kinds())
    };
    let _ = &backend_certs;

    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let conc = env_usize("QUIC_CONCURRENCY", 4) * args.scale;
    let streams = env_usize("QUIC_STREAMS", 4);
    let payload = env_usize("QUIC_PAYLOAD", 4096);
    let dgrams = env_usize("QUIC_DGRAMS", 8);

    let mut tasks = Vec::new();
    let mut stats = Vec::new();
    let load = LoadStats::new();
    stats.push(("quic_load".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_quic_load(
        listen,
        client_sni,
        client_ca,
        conc,
        streams,
        payload,
        dgrams,
        Arc::clone(&load),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges,
        kinds,
        tasks,
        backend_ctrls: vec![],
        quic_stop: vec![quic_stop],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc6 — CF-S19-TLS-TEARDOWN-413: oversize requests over TLS torn down mid-reply, plus mid-stream
/// disconnects.
async fn setup_413(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ctrl = BackendControl::new();
    let backend = backends::spawn_h1_backend(Arc::clone(&ctrl)).await?;
    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_port()?);
    let certs = config_gen::generate_certs(workdir, "localhost")?;
    let toml = config_gen::h1s_front(listener, backend, "h1", metrics, &certs);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sni = "localhost".to_string();
    let mut tasks = Vec::new();
    let mut stats = Vec::new();
    let ov = LoadStats::new();
    stats.push(("oversize_teardown".into(), Arc::clone(&ov)));
    tasks.push(tokio::spawn(chaos::run_oversize_teardown(
        listener,
        sni,
        certs.ca.clone(),
        4 * args.scale,
        Arc::clone(&ov),
        cancel.clone(),
    )));
    let md = LoadStats::new();
    stats.push(("mid_stream_disconnect".into(), Arc::clone(&md)));
    tasks.push(tokio::spawn(chaos::run_mid_stream_disconnect(
        listener,
        4 * args.scale,
        Arc::clone(&md),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: tcp_gauges(),
        kinds: tcp_kinds(),
        tasks,
        backend_ctrls: vec![ctrl],
        quic_stop: vec![],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc7 — H3-terminate front (`quiche::h3` ingress): sustained REAL H3 client load + an H3
/// RST/STOP_SENDING flood. The config deliberately carries NO backend, so what is exercised is the
/// INGRESS + the inline-400 decoded egress + F-MD-4 + the no-backend-drop path — not a live relay.
/// The bounded-state signal is therefore the OS footprint + `panic_total=0`: this front exposes no
/// dedicated Prometheus state gauge.
async fn setup_h3_terminate(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_udp_port()?);
    let front_certs = config_gen::generate_certs(workdir, "soak-front")?;
    let retry = workdir.join("retry.bin");
    let toml = config_gen::quic_h3_terminate(listener, metrics, &front_certs, &retry);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sni = "soak-front".to_string();
    let ca = front_certs.ca.clone();

    let conc = env_usize("H3_CONCURRENCY", 4) * args.scale;
    let reqs = env_usize("H3_REQS_PER_CONN", 8);
    let reset_conc = env_usize("H3_RESET_CONCURRENCY", 2) * args.scale;

    let mut tasks = Vec::new();
    let mut stats = Vec::new();
    let load = LoadStats::new();
    stats.push(("h3_load".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_h3_load(
        listener,
        sni.clone(),
        ca.clone(),
        conc,
        reqs,
        Arc::clone(&load),
        cancel.clone(),
    )));
    let flood = LoadStats::new();
    stats.push(("h3_reset_flood".into(), Arc::clone(&flood)));
    tasks.push(tokio::spawn(loadgen::run_h3_reset_flood(
        listener,
        sni,
        ca,
        reset_conc,
        Arc::clone(&flood),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: h3term_gauges(),
        kinds: h3term_kinds(),
        tasks,
        backend_ctrls: vec![],
        quic_stop: vec![],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc9_grpc_h3 (S29) — gRPC-over-HTTP/3 soak: a `quic` H3-terminate front → an H2 gRPC echo
/// backend, driving the REAL backend-proxied response-trailer path the F-S29-1 fix corrected.
/// Leak-class signal: `fds` (each in-flight RPC pins a client udp + a pooled backend tcp) +
/// RSS/VmHWM + `panic_total == 0`. Every RPC verifies `grpc-status: 0` in-client, so a trailer-drop
/// regression surfaces as `err()`.
async fn setup_grpc_h3(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ctrl = BackendControl::new();
    let backend = backends::spawn_grpc_h2_backend(Arc::clone(&ctrl)).await?;

    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_udp_port()?);
    let front_certs = config_gen::generate_certs(workdir, "soak-front")?;
    let retry = workdir.join("retry.bin");
    let toml = config_gen::quic_h3_terminate_h2(listener, backend, metrics, &front_certs, &retry);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sni = "soak-front".to_string();
    let ca = front_certs.ca.clone();

    let sustained = env_usize("GRPC_SUSTAINED", 6) * args.scale;
    let churn = env_usize("GRPC_CHURN", 6) * args.scale;
    let rpcs_per_cycle = env_usize("GRPC_CHURN_RPCS", 4);

    let mut tasks = Vec::new();
    let mut stats = Vec::new();

    let load = LoadStats::new();
    stats.push(("grpc_h3_sustained".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_grpc_h3_load(
        listener,
        sni.clone(),
        ca.clone(),
        sustained,
        Arc::clone(&load),
        cancel.clone(),
    )));

    let chstats = LoadStats::new();
    stats.push(("grpc_h3_churn".into(), Arc::clone(&chstats)));
    tasks.push(tokio::spawn(loadgen::run_grpc_h3_churn(
        listener,
        sni,
        ca,
        churn,
        rpcs_per_cycle,
        Arc::clone(&chstats),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: h3term_gauges(),
        kinds: h3term_kinds(),
        tasks,
        backend_ctrls: vec![ctrl],
        quic_stop: vec![],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc8_ws_h1 (S27) — WebSocket-over-HTTP/1.1 soak: an `h1` front with `[listeners.websocket]`
/// enabled → a tungstenite WS echo backend, so the long-lived `WsProxy::proxy_frames` relay runs
/// end to end. Sustained held tunnels + open→echo→CLEAN-close churn (the F-S20-2 RECLAIM probe).
/// Bounded-state signal: `fds` — every live tunnel pins a client fd + a backend fd + the relay
/// task, so a bounded series IS the no-leak proof. (`accept_inflight` is scraped but is NOT the
/// discriminant — see `ws_gauges`.)
async fn setup_ws_h1(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ws_stop = Arc::new(AtomicBool::new(false));
    let backend = backends::spawn_ws_h1_backend(Arc::clone(&ws_stop)).await?;

    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_port()?);
    // Generous WS idle close (1001) so sustained clients stay up and the churn clients control
    // their own cadence; the read-frame watchdog still bounds a wedged half.
    let ws_idle = env_usize("WS_IDLE_SECS", 120) as u64;
    let ws_read_frame = env_usize("WS_READ_FRAME_SECS", 30) as u64;
    let toml = config_gen::h1_front_ws(listener, backend, metrics, ws_idle, ws_read_frame);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sustained = env_usize("WS_SUSTAINED", 8) * args.scale;
    let churn = env_usize("WS_CHURN", 8) * args.scale;
    let frames_per_cycle = env_usize("WS_CHURN_FRAMES", 4);

    let mut tasks = Vec::new();
    let mut stats = Vec::new();

    let load = LoadStats::new();
    stats.push(("ws_sustained".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_ws_h1_load(
        listener,
        sustained,
        Arc::clone(&load),
        cancel.clone(),
    )));

    let chstats = LoadStats::new();
    stats.push(("ws_churn".into(), Arc::clone(&chstats)));
    tasks.push(tokio::spawn(loadgen::run_ws_h1_churn(
        listener,
        churn,
        frames_per_cycle,
        Arc::clone(&chstats),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: ws_gauges(),
        kinds: ws_kinds(),
        tasks,
        backend_ctrls: vec![],
        quic_stop: vec![ws_stop],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc8b_ws_h2 (S27) — WebSocket-over-HTTP/2 soak (RFC 8441 extended CONNECT) over an `h1s` front
/// with `h2_extended_connect = true` (the CF-S27-2 opt-in). Same leak class as sc8_ws_h1.
/// F-S27-2 NOTE: the load client READS NORMALLY (drains DATA + releases flow-control capacity) — it
/// intentionally does NOT exercise the gated H2 unbounded-buffer DoS. Discriminant: `fds` + RSS +
/// `panic_total = 0`.
async fn setup_ws_h2(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ws_stop = Arc::new(AtomicBool::new(false));
    let backend = backends::spawn_ws_h1_backend(Arc::clone(&ws_stop)).await?;

    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_port()?);
    let certs = config_gen::generate_certs(workdir, "localhost")?;
    let ws_idle = env_usize("WS_IDLE_SECS", 120) as u64;
    let ws_read_frame = env_usize("WS_READ_FRAME_SECS", 30) as u64;
    let toml = config_gen::h1s_front_ws(listener, backend, metrics, &certs, ws_idle, ws_read_frame);
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sni = "localhost".to_string();
    let ca = certs.ca.clone();
    let sustained = env_usize("WS_SUSTAINED", 8) * args.scale;
    let churn = env_usize("WS_CHURN", 8) * args.scale;
    let frames_per_cycle = env_usize("WS_CHURN_FRAMES", 4);

    let mut tasks = Vec::new();
    let mut stats = Vec::new();

    let load = LoadStats::new();
    stats.push(("ws_sustained".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_ws_h2_load(
        listener,
        sni.clone(),
        ca.clone(),
        sustained,
        Arc::clone(&load),
        cancel.clone(),
    )));

    let chstats = LoadStats::new();
    stats.push(("ws_churn".into(), Arc::clone(&chstats)));
    tasks.push(tokio::spawn(loadgen::run_ws_h2_churn(
        listener,
        sni,
        ca,
        churn,
        frames_per_cycle,
        Arc::clone(&chstats),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: ws_gauges(),
        kinds: ws_kinds(),
        tasks,
        backend_ctrls: vec![],
        quic_stop: vec![ws_stop],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

/// sc8c_ws_h3 (S28) — WebSocket-over-HTTP/3 soak (RFC 9220 extended CONNECT): a `quic` H3-terminate
/// front with `h3_extended_connect = true` → the same H1 WS echo backend, running the
/// single-sourced `proxy_frames` relay over the bounded `H3WsTunnel`. Same leak class as sc8_ws_h1;
/// signal is `fds`.
async fn setup_ws_h3(
    args: &Args,
    bin: &std::path::Path,
    workdir: &std::path::Path,
    cancel: CancellationToken,
) -> anyhow::Result<Running> {
    let ws_stop = Arc::new(AtomicBool::new(false));
    let backend = backends::spawn_ws_h1_backend(Arc::clone(&ws_stop)).await?;

    let metrics = metrics_addr()?;
    let listener = tcp_addr(gateway::ephemeral_udp_port()?);
    let front_certs = config_gen::generate_certs(workdir, "soak-front")?;
    let retry = workdir.join("retry.bin");
    let ws_idle = env_usize("WS_IDLE_SECS", 120) as u64;
    let ws_read_frame = env_usize("WS_READ_FRAME_SECS", 30) as u64;
    let toml = config_gen::quic_h3_terminate_ws(
        listener,
        backend,
        metrics,
        &front_certs,
        &retry,
        ws_idle,
        ws_read_frame,
    );
    let cfg = workdir.join("gateway.toml");
    std::fs::write(&cfg, toml)?;
    let gw = spawn_gateway(bin, &cfg, metrics, workdir).await?;

    let sni = "soak-front".to_string();
    let ca = front_certs.ca.clone();

    let sustained = env_usize("WS_SUSTAINED", 8) * args.scale;
    let churn = env_usize("WS_CHURN", 8) * args.scale;
    let frames_per_cycle = env_usize("WS_CHURN_FRAMES", 4);

    let mut tasks = Vec::new();
    let mut stats = Vec::new();

    let load = LoadStats::new();
    stats.push(("ws_h3_sustained".into(), Arc::clone(&load)));
    tasks.push(tokio::spawn(loadgen::run_ws_h3_load(
        listener,
        sni.clone(),
        ca.clone(),
        sustained,
        Arc::clone(&load),
        cancel.clone(),
    )));

    let chstats = LoadStats::new();
    stats.push(("ws_h3_churn".into(), Arc::clone(&chstats)));
    tasks.push(tokio::spawn(loadgen::run_ws_h3_churn(
        listener,
        sni,
        ca,
        churn,
        frames_per_cycle,
        Arc::clone(&chstats),
        cancel.clone(),
    )));

    Ok(Running {
        gateway: gw,
        metrics_addr: metrics,
        gauges: ws_gauges(),
        kinds: ws_kinds(),
        tasks,
        backend_ctrls: vec![],
        quic_stop: vec![ws_stop],
        stats,
        _tmp: workdir.to_path_buf(),
    })
}

async fn spawn_gateway(
    bin: &std::path::Path,
    cfg: &std::path::Path,
    metrics: SocketAddr,
    workdir: &std::path::Path,
) -> anyhow::Result<GatewayChild> {
    let log = workdir.join("gateway.log");
    GatewayChild::spawn_and_wait_ready(bin, cfg, metrics, log, BOOT_BUDGET).await
}

fn tcp_gauges() -> Vec<String> {
    vec!["accept_inflight".into(), "panic_total".into()]
}
fn tcp_kinds() -> Vec<(String, MetricKind)> {
    vec![("panic_total".into(), MetricKind::CounterMustBeZero)]
}
/// sc8_ws_h1 connection-leak DISCRIMINANT: the OS file-descriptor count (`fds`). Every live WS
/// tunnel pins a client fd + a backend fd + the relay task, so a bounded series IS the no-leak
/// proof.
///
/// `accept_inflight` is deliberately NOT the discriminant: under open/close churn it is a SAWTOOTH
/// at a low integer baseline and the relative-growth Trend analyzer mis-flags a 0→2 wiggle as +200%
/// once the first-third median lands on a trough. It is still scraped into the CSV.
fn ws_gauges() -> Vec<String> {
    vec!["panic_total".into()]
}
fn ws_kinds() -> Vec<(String, MetricKind)> {
    vec![("panic_total".into(), MetricKind::CounterMustBeZero)]
}
/// H3-terminate has NO dedicated Prometheus state-table family, so the bounded-state proof is the
/// OS footprint plus the universal `panic_total`.
fn h3term_gauges() -> Vec<String> {
    vec!["panic_total".into()]
}
fn h3term_kinds() -> Vec<(String, MetricKind)> {
    vec![("panic_total".into(), MetricKind::CounterMustBeZero)]
}
fn modeb_gauges() -> Vec<String> {
    vec![
        "quic_modeb_connections".into(),
        "quic_modeb_streams_active".into(),
        "quic_modeb_datagrams_dropped_total".into(),
        "panic_total".into(),
    ]
}
fn modeb_kinds() -> Vec<(String, MetricKind)> {
    vec![
        (
            "quic_modeb_datagrams_dropped_total".into(),
            MetricKind::Counter,
        ),
        ("panic_total".into(), MetricKind::CounterMustBeZero),
    ]
}
fn modea_gauges() -> Vec<String> {
    vec![
        "quic_passthrough_flows".into(),
        "quic_passthrough_flows_evicted_total".into(),
        "panic_total".into(),
    ]
}
fn modea_kinds() -> Vec<(String, MetricKind)> {
    vec![
        (
            "quic_passthrough_flows_evicted_total".into(),
            MetricKind::Counter,
        ),
        ("panic_total".into(), MetricKind::CounterMustBeZero),
    ]
}

fn render_summary(
    args: &Args,
    ts: &lb_soak::timeseries::TimeSeries,
    verdicts: &[lb_soak::timeseries::ColumnVerdict],
    stats: &[(String, Arc<LoadStats>)],
    overall_drift: bool,
) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "=== SOAK {} — {} ({}s, {} samples) ===",
        args.scenario,
        if overall_drift {
            "DRIFT (finding)"
        } else {
            "BOUNDED"
        },
        args.duration,
        ts.len()
    );
    for v in verdicts {
        let _ = writeln!(
            s,
            "  {:>10} [{:>6}] {} — first={:.0} last={:.0} min={:.0} max={:.0}",
            v.column,
            v.verdict.as_str(),
            v.note,
            v.first,
            v.last,
            v.min,
            v.max
        );
    }
    let _ = writeln!(s, "  load:");
    for (name, st) in stats {
        let _ = writeln!(s, "    {name}: ok={} err={}", st.ok_count(), st.err_count());
    }
    s
}

fn render_json(
    args: &Args,
    ts: &lb_soak::timeseries::TimeSeries,
    verdicts: &[lb_soak::timeseries::ColumnVerdict],
    stats: &[(String, Arc<LoadStats>)],
    overall_drift: bool,
) -> serde_json::Value {
    let cols: Vec<serde_json::Value> = verdicts
        .iter()
        .map(|v| {
            serde_json::json!({
                "column": v.column,
                "kind": v.kind_str,
                "verdict": v.verdict.as_str(),
                "n": v.n,
                "first": v.first,
                "last": v.last,
                "min": v.min,
                "max": v.max,
                "first_third_median": v.first_third_median,
                "last_third_median": v.last_third_median,
                "rel_growth": v.rel_growth,
                "slope_per_sample": v.slope_per_sample,
                "monotone_frac": v.monotone_frac,
                "note": v.note,
            })
        })
        .collect();
    let load: Vec<serde_json::Value> = stats
        .iter()
        .map(|(name, st)| {
            serde_json::json!({ "name": name, "ok": st.ok_count(), "err": st.err_count() })
        })
        .collect();
    serde_json::json!({
        "scenario": args.scenario,
        "label": args.label,
        "duration_secs": args.duration,
        "sample_secs": args.sample,
        "scale": args.scale,
        "samples": ts.len(),
        "overall": if overall_drift { "DRIFT" } else { "BOUNDED" },
        "columns": cols,
        "load": load,
    })
}
