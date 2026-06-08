# S39 Bench Harness Design — `eg-bench` (R12 single-sourced)

Status: DESIGN (Phase 0). Implemented by perf-eng, reviewed like code (R5),
cross-validated against `oha` where both can run (author≠verifier on the harness).

## Why a new harness
`lb-soak::loadgen` records only **ok/err counts** (it is a boundedness/leak
harness — "favours connection turnover over pipelining"). Perf characterization
(Phase 1) needs **achieved throughput (RPS) + latency distribution (p50/p99/p999)
+ resource cost (CPU/mem/fd)** per protocol path. `oha` covers H1/H1s/H2 (trusted,
external). The QUIC/H3 family (H3, gRPC-H3, QUIC Mode A passthrough, WS) has no
off-the-shelf loopback tool, so we build `eg-bench`.

## What measures what
| Path            | Tool       | Notes |
|-----------------|------------|-------|
| H1 (plaintext)  | oha + eg-bench | eg-bench h1 cross-validated vs oha |
| H1s (TLS)       | oha        | `--insecure` |
| H2 (TLS, ALPN)  | oha + eg-bench | `--http2 --insecure`; eg-bench h2 cross-val vs oha |
| H3 real-proxy   | eg-bench   | H3 front → H2 backend (config_gen::quic_h3_terminate_h2) |
| gRPC-over-H3    | eg-bench   | H3 front → H2 gRPC backend (matches sc9 datapath) |
| QUIC Mode A     | eg-bench   | passthrough echo bidi-stream RTT |
| WS (H1)         | eg-bench   | echo round-trip RTT |

## eg-bench shape
- New binary `crates/lb-soak/src/bin/eg-bench.rs` + module `crates/lb-soak/src/bench.rs`
  (`pub mod bench;` in lib.rs). **Does NOT touch loadgen** (R3: soak path untouched);
  reuses `gateway`, `backends`, `config_gen`, `procstat`/`sampler`, and the proven
  quiche/TLS connection patterns (copy, don't mutate).
- CLI: `eg-bench --protocol <P> --connections <C> --duration-secs <D> [--warmup-secs <W=5>] [--payload <bytes>] [--out <dir>]`
- Model: **closed-loop**. `C` concurrent workers, each holds ONE connection and
  issues requests back-to-back; per-request RTT (`Instant` delta from request-send
  to full-response-received) recorded into a per-worker `Vec<u64>` micros. Merge →
  sort → exact percentiles (dependency-free; no new Cargo.lock edge). Warmup window
  discarded (cold-start outlier — the soak boot-outlier lesson).
- Throughput = `total_ok / measured_window_secs` (RPS).
- Per-request unit per protocol:
  - h1/h2: one HTTP request → full body collected.
  - h3 / grpc_h3: one H3 request (real backend) → `:status` + full body/trailers.
  - quic_modea: one echo bidi stream send+FIN → full echo received (RTT).
  - ws_h1: one echo frame send → matching echo received (RTT).

## Resource + isolation (R9 — gateway cost ≠ client cost; the S21 lesson)
- Sample the **gateway child** `/proc` (RSS, fds, threads) + CPU% during the
  measured window (reuse `procstat`). Report gateway %CPU at load.
- **Isolation calibration**: for h1/h2, add a `--bypass` flag where the client
  hits the **backend directly** (no gateway). `bypass_rps` is the client+backend
  ceiling; gateway overhead = (bypass_rps vs through_rps) + the p50 latency delta.
  If through-RPS plateaus while gateway %CPU is far below saturation → the CLIENT
  is the ceiling: SAY SO (do not report it as the gateway's throughput).
- For QUIC/H3 (no trivial bypass): report gateway %CPU; a number is the gateway's
  only if the gateway core(s) are the saturated resource. Otherwise flag "client/
  loopback-bound" and report the bottleneck honestly.

## Sweep (the runner, not the binary)
`scripts/perf/run-bench.sh` sweeps `C ∈ {1, 8, 32, 64, 128}` per protocol at a
fixed payload, writing one JSON per (protocol, C). The throughput ceiling is where
RPS plateaus and tail latency climbs; the latency baseline is reported at a stated
concurrency. Keep raw JSON small (summaries, not per-request dumps) — CF-DISK-1 /
the S37 "don't commit 100M CSVs" lesson.

## Output
`audit/perf/s39-bench-data/<protocol>-c<C>.json` (summary only) + the harness
emits a one-line human summary per run. The verdict doc `s39-perf-baseline.md`
tabulates per-path: offered C, achieved RPS, p50/p99/p999, gateway CPU/RSS/fd,
bottleneck. Every headline number reproduced by the verifier (R5/R13).

## Exact code references (copy, don't mutate — R3)
All in `crates/lb-soak/src/`:
- Setup (mirror these): `bin/eg-soak.rs::setup_grpc_h3` (683) / `setup_h1` (272) /
  `setup_quic`; `spawn_gateway` (1005) wraps `gateway::GatewayChild::spawn_and_wait_ready`
  (gateway.rs:79; args bin,cfg,metrics,log,BOOT_BUDGET).
- Config: `config_gen::h1_front` (93), `h1s_front(listener,backend,proto,metrics,certs)`
  (181, TLS ALPN h2/http1.1 — the H2 path), `quic_h3_terminate_h2(listener,backend,
  metrics,&certs,&retry)` (328, H3 front→H2 backend — the H3 datapath),
  `passthrough_mode_a(bind,backend,metrics,&retry,max_conns,idle_ms)` (360, Mode A).
  `generate_certs(dir,sni)`→`Certs{cert,key,ca}` (40). retry.bin path passed but not
  pre-created (gateway mints it on boot — sc9 proves this).
- Backends: `spawn_h1_backend` (203), `spawn_h2_backend` (233, **h2c plaintext**),
  `spawn_grpc_h2_backend` (174), `spawn_quic_echo_backend` (327),
  `spawn_ws_h1_backend(stop)` (271). All take `BackendControl::new()` / a stop flag.
- Client patterns to copy: H1 `h1_keepalive_burst` (108); H2 `h2_tls_connector` (860,
  **pub**) + `h2_stream_batch` (870, absolute-form URI `https://{sni}/`); QUIC
  `quic_client_config` (963) + `quic_session` (989, partial-write loop — F-S20-1);
  H3 `h3_session` (1420) + `with_transport`; gRPC-H3 `grpc_h3_unary_session`; WS-H3
  `ws_mask_frame`/`ws_parse_one`/`ws_h3_drain` (1751+). Transport helpers: `flush`
  (1698), `recv_one` (1715), `random_cid` (1734), `MAX_UDP`=65535 (26).
- The H3 datapath unit: with `quic_h3_terminate_h2`, a plain H3 GET/POST → real H2
  backend response. gRPC-H3 = same datapath + grpc body framing (≈ H3 + framing).
- AcceptAny TLS verifier (loopback only): `AcceptAnyServerCert` (812).

## Validation gates (before any number is trusted)
1. `cargo build --release -p lb-soak --bin eg-bench` clean.
2. clippy `-D warnings` + fmt clean (lb-soak carries the panic-freedom deny block).
3. eg-bench **h1/h2 RPS+p99 agree with oha within noise** (the cross-validation —
   if they disagree, the harness is wrong, not the gateway).
4. A short smoke run per protocol produces non-zero ok, ~zero err, plausible RTT.
