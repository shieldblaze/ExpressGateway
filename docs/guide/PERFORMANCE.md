# Performance

This is the canonical performance document for ExpressGateway. Every number
here is from the **S39 perf baseline** and is stated **with its conditions**.
Nothing is rounded up; no headline number appears without the box, the load
shape, and the caveat. The raw data and methodology are in
[`../../audit/perf/s39-perf-baseline.md`](../../audit/perf/s39-perf-baseline.md),
[`../../audit/perf/s39-burnin.md`](../../audit/perf/s39-burnin.md), and
[`../../audit/perf/s39-report.md`](../../audit/perf/s39-report.md).

> **Read this first.** These are **current-state numbers on a single shared
> box**, not a vendor benchmark and not a dedicated-rig result. The limits the
> measurement found were largely the **test rig's**, not the gateway's. No owner
> performance **target** has been set — this baseline establishes honest
> present-state numbers, it does not claim a tuned ceiling.

## Measurement setup (the conditions that scope every number)

- **Box:** AWS `c6a.2xlarge` — **8 vCPU, 15 GiB RAM**, ENA/`ens5`.
- **Co-located loopback:** the load client, the `expressgateway` gateway, **and
  the backend all run on this one 8-core box.** Throughput is therefore a
  **shared-box system number**, not the gateway in isolation. Past a knee
  (gateway ~2 cores) the box oversubscribes and RPS degrades — that is box
  contention, not a gateway regression.
- **Window:** each run = 5 s warmup discarded + **15 s measured**.
- **Harness:** `eg-bench` (closed-loop, per-request RTT) cross-validated against
  `oha` 1.14 (a separate-process production-grade client) for H1/H1s/H2.
- **QUIC client:** uses a 1350-byte max UDP payload (sub-MTU) — H3 / Mode-A
  per-packet figures reflect 1350 B datagrams, not 1500 B / jumbo.

### How to read the three signals

1. **Throughput (RPS) is a SYSTEM number.** Client + gateway + backend share 8
   cores, so absolute peak RPS reflects the whole box. Run-to-run variance is
   **~±20–30 % at the throughput knee** (two independent sweeps; e.g. H1 c=128
   measured 18 343 on one sweep and 11 204 on another). **Peaks are
   approximate.**
2. **The box-independent signal is gateway CPU-µs per request** = (gateway
   CPU% ÷ RPS). This is what you pay per request regardless of how many cores
   the test client steals — it is the honest cross-protocol efficiency
   comparison, and it is **stable** run-to-run.
3. **Latency is cleanest below the box knee** (low/moderate concurrency), where
   it reflects service time rather than queueing.

## Efficiency ranking (the headline you can trust)

Gateway CPU-µs per request — **lower is better, box-independent, stable**:

| Path | CPU-µs / req | At concurrency |
|------|--------------|----------------|
| **WebSocket (H1, echo)** | **~37** | c=32 |
| **HTTP/2** (TLS ALPN-h2 → H2c) | **~59** | c=128 |
| **HTTP/3** (H3-terminate → H2c) | **~101** | c=128 |
| **HTTP/1.1** (→ H1) | **~163** | c=32 |

Ordering: **WS ~37 < H2 ~59 < H3 ~101 < H1 ~163.** H2 and WebSocket multiplex
over few sockets; H1 pays per-connection TCP overhead; H3 pays QUIC crypto + H3
framing yet stays competitive (~101 µs). Source:
[`s39-perf-baseline.md`](../../audit/perf/s39-perf-baseline.md) "Efficiency
ranking".

## Peak throughput on the co-located 8-core box (APPROXIMATE)

These are **system** peaks on the shared box (±20–30 % at the knee). A gateway
alone on dedicated cores would go materially higher — but that was not measured.

| Path | Peak RPS (concurrency) | Gateway CPU @ peak | Bottleneck |
|------|------------------------|--------------------|------------|
| **WebSocket (H1)** | **~42.5k** (c=32) | 157 % | box CPU; cheapest/req |
| **HTTP/2** | **~32k** (c=128) | 191 % | not yet saturated; box/client |
| **HTTP/3** | **~18.5k** (c=128) | 188 % | client pump + box; QUIC crypto |
| **HTTP/1.1** | **~14–18k** (c=32–128) | 224–252 % | box CPU (per-conn TCP) |
| **QUIC Mode A passthrough** | **harness-bound** (~1.65k, see below) | ~11 % (idle) | **test echo backend**, NOT the gateway |

(CPU% > 100 % means multiple cores; the box has 8.) Source:
[`s39-perf-baseline.md`](../../audit/perf/s39-perf-baseline.md) "Headline".

## Per-path detail

### HTTP/1.1 (→ H1 backend)

| c | rps | p50 | p99 | p999 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|------|---------|--------|-----|
| 1 | 4139 | 237 µs | 321 µs | 527 µs | 90 | 9.4 | 13 |
| 8 | 11919 | 627 µs | 1488 µs | 2910 µs | 163 | 12.4 | 27 |
| 32 | 13777 | 2052 µs | 7919 µs | 12725 µs | 224 | 16.9 | 73 |
| 128 | 18343 | 6560 µs | 15956 µs | 22442 µs | 252 | 28.0 | 261 |

The ~1 % err is keep-alive **recycle reconnects** (the backend closes idle
keep-alive conns; the client counts the reconnect as one err) — not a gateway
fault. fds grow ~2/conn (TCP per-connection cost).

### HTTP/2 (TLS, ALPN-h2 front → H2c backend)

| c | rps | p50 | p99 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|---------|--------|-----|
| 1 | 3915 | 241 µs | 295 µs | 66 | 10.7 | 13 |
| 8 | 14543 | 519 µs | 1072 µs | 158 | 12.7 | 20 |
| 32 | 24148 | 1234 µs | 2555 µs | 177 | 16.0 | 44 |
| 128 | 32191 | 3753 µs | 6998 µs | 191 | 24.0 | 140 |

err = 0. At c ≤ 32 (box not oversubscribed) p999 ≤ 5 ms; the ~42 ms p999 that
appears at c ≥ 64 is the **co-located box saturating** (a TCP delayed-ACK stall
on ~0.1 % of requests), not a gateway latency floor — the gateway sets
`TCP_NODELAY` on every socket (verified in source). The "H2 front" is the `h1s`
listener's ALPN-h2 leg (an honest H2 front, configured via the `h1s` listener
type).

### HTTP/3 (H3-terminate front → H2c backend)

| c | rps | p50 | p99 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|---------|--------|-----|
| 1 | 294 | 3474 µs | 3587 µs | 11 | 12.5 | 12 |
| 8 | 2064 | 3407 µs | 7003 µs | 46 | 17.4 | 12 |
| 32 | 8003 | 3852 µs | 7667 µs | 106 | 26.5 | 12 |
| 128 | 18566 | 6501 µs | 11623 µs | 188 | 47.7 | **12** |

err ~0.1 % = the expected GOAWAY-race during connection recycling (the S36
fix working). **fds stay at 12 at every concurrency** — correct QUIC behaviour
(one UDP socket demuxes all connections by CID). The ~3.4 ms single-connection
service time is **client + gateway combined** (the closed-loop quiche pump is
genuine client-side work); the gateway sits at 11 % CPU at c=1, so single-stream
latency is service-time, not gateway saturation.

### QUIC Mode A passthrough — HARNESS-BOUND (read carefully)

| c | rps | p50 | gw CPU% | note |
|---|-----|-----|---------|------|
| 1 | 1585 | 624 µs | 10 | gateway passthrough adds ~0.6 ms |
| 32 | 1645 | 19459 µs | 11 | single-queue serializer |
| 128 | 1662 | 77800 µs | 11 | gateway idle throughout |

**The ~1.65k RPS ceiling is the TEST BACKEND, not the gateway.** The co-located
QUIC echo backend is a single tokio task / single UDP socket / one packet per
loop — it serializes all flows (the `p50 ≈ c / 1650` linear fingerprint
confirms it). The gateway passthrough sat at **10–11 % CPU (idle)** throughout.
The honest **gateway** result: passthrough adds **~0.6 ms latency + ~11 % CPU**
(a thin UDP forwarder); a gateway throughput ceiling for Mode A is **not
measurable** with this single-task test backend.

### WebSocket over H1 (opaque echo relay)

| c | rps | p50 | p99 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|---------|--------|-----|
| 1 | 10136 | 96 µs | 125 µs | 57 | 8.8 | 13 |
| 8 | 33490 | 227 µs | 489 µs | 128 | 11.4 | 27 |
| 32 | 42573 | 730 µs | 1496 µs | 157 | 18.6 | 75 |
| 128 | 28148 | 4418 µs | 8184 µs | 209 | 45.6 | 267 |

err = 0. The fastest path (p50 = 96 µs @ c=1) — a long-lived opaque
bidirectional frame relay with no per-message HTTP parse. Peaks ~42.5k echoes/s
@ c=32; c=128 declines (box oversubscription).

### Independent cross-validation (oha)

`oha` 1.14 (a separate-process, production-grade client) agrees with `eg-bench`
within **~5 % (H1)** and **~10–15 % (H2)**, confirming the custom harness. The
harness code was also independently audited (author ≠ verifier): all 8
measurement properties PASS, **no number retracted**. Details:
[`s39-perf-baseline.md`](../../audit/perf/s39-perf-baseline.md) "oha
cross-validation" + "Independent verifier review".

## Stability: 4-hour burn-in

A full 12-scenario soak ran **concurrently for 4 hours** (14 400 s, 481
samples/scenario) on the same box class (`c6a.2xlarge`, 8 vCPU). Result:
**11/12 scenarios BOUNDED, panic = 0**, with bounded RSS / fd / threads on every
path over **billions of operations**.

R8 bounded-memory held over hours (not just minutes):

- **320 M H2 rapid-resets** (CVE-2023-44487 defense) — bounded, panic = 0.
- **49 M H3 reset-floods** — bounded.
- **261 M connection-floods** — bounded.

The single non-BOUNDED scenario (sc3 slowloris) was an **analyzer
false-positive**: its memory was flat (~36 MB) and it served 15.5 M requests
with zero errors; the flagged metric (`accept_inflight`) converged to exactly
the injector's offered slow-connection count (~144) and capped there — correct
R8 slowloris behaviour. An isolated recheck was BOUNDED. Source:
[`s39-burnin.md`](../../audit/perf/s39-burnin.md).

**Verdict (from the perf session):** no gateway performance defect was found —
every path is bounded (RSS/fd/threads), err ≈ 0 (or explained), panic = 0.

## What was NOT characterized (deferred tiers)

These are **not measured** and you should not assume numbers for them:

- **io_uring throughput tier** — the runtime can use io_uring (Linux 5.1+) or
  fall back to epoll, but the io_uring **performance tier was not
  characterized** in this baseline.
- **XDP / kernel-offload L4 throughput** — the XDP data plane is off by default
  and its throughput was **not measured** here (the L4 path was validated for
  correctness and live attach, not benchmarked for peak PPS).
- **Dedicated-rig / multi-box throughput** — every number above is from a
  **single co-located 8-core box** (load client + gateway + backend sharing
  cores). A dedicated-rig or distributed measurement was not run; do not
  extrapolate the co-located peaks to isolated-gateway capacity.
- **No owner performance target** has been set. This baseline is the honest
  present state; it does not represent a tuned or maximal configuration.

For capacity planning, treat the **CPU-µs/req efficiency** figures as the
reliable, transferable signal and the **peak RPS** figures as approximate,
co-located system numbers. See
[`capabilities.md`](capabilities.md) "Deferred performance tiers".
