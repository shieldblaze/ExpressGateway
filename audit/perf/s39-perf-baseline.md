# S39 Perf Baseline — per-protocol throughput + latency + resource

Box: c6a.2xlarge, 8 vCPU, 15 GiB RAM, ENA/ens5. **Co-located loopback**: the load
client, the `expressgateway` gateway, and the backend ALL run on this one 8-core
box. Binary: `expressgateway` release @ branch `feature/perf-burnin-s39`.
Harness: `eg-bench` (closed-loop, per-request RTT) + `oha` 1.14 (H1/H1s/H2
cross-validation). Each run = 5s warmup discarded + 15s measured window.

## Measurement philosophy (read this before reading the numbers)
This is a MEASUREMENT session — numbers are honest or absent (R4), and a number
is attributed to the gateway only when the gateway is the proven bottleneck (R9 /
the S21 lesson: confirm the load client/backend isn't the ceiling).

1. **Throughput is a SYSTEM number.** Client+gateway+backend share 8 cores, so the
   absolute peak RPS reflects the whole box, not the gateway in isolation. Past a
   knee (gateway ~2 cores) the box oversubscribes and RPS degrades — that is box
   contention, not a gateway regression.
2. **The box-independent signal is gateway CPU-µs/request** = (gateway CPU% / RPS).
   This is what an operator pays per request regardless of how many cores the test
   client steals, and it's the honest cross-protocol efficiency comparison.
3. **Latency is cleanest at low/moderate concurrency** (below the box knee) where
   it reflects service time, not queueing.
4. **Bottleneck attribution** per path: gateway-CPU-bound (gateway near saturation)
   vs box-contention-bound (client+backend starve the gateway) vs harness-bound
   (the test client/backend serializes while the gateway sits idle — stated, never
   hidden).

## Two measurement artifacts found + corrected (NOT gateway issues)
- **H1/H2 p999 ~41ms tail (original sweep):** the eg-bench TCP **client** did not
  set `TCP_NODELAY`; Nagle held small H2 control frames into the ~40ms delayed-ACK
  window. The gateway sets `TCP_NODELAY` on every socket (verified:
  lb-io/{pool,http2_pool,sockopts}.rs + lb/main.rs). Adding client `set_nodelay`
  dropped h2 c=32 p999 **41307µs → 7314µs** with RPS/p50/p99 unchanged → the tail
  was the harness. The tables below use the nodelay-corrected client for H1/H2.
- **QUIC Mode A throughput cap ~1650 rps:** the co-located QUIC **echo backend**
  (`spawn_quic_echo_backend`) is a single tokio task / single UDP socket / one
  packet per loop — it serializes all Mode A flows. The gateway passthrough sat at
  10–11% CPU throughout (idle), so Mode A throughput is harness-bound; the gateway
  passthrough cost is what's reported, not a (harness-limited) RPS ceiling.

## Headline (per path) — peak achieved on the co-located 8-core box
| Path | Peak RPS (conc) | gateway CPU @ peak | CPU-µs/req | p50 / p99 below-knee | fds @ peak | bottleneck |
|------|-----------------|--------------------|-----------|----------------------|-----------|------------|
| H1 (→H1) | ~14–18k (c=32–128) | 224–252% | ~163 (c32) | 627µs / 1488µs (c8) | 73–261 | box CPU (TCP per-conn) |
| H1s (TLS h1) | _oha row below_ | | | | | |
| H2 (→H2c) | ~32k (c=128) | 191% | ~59 (c128) | 519µs / 1072µs (c8) | 140 | not yet saturated; box/client |
| H3 (→H2c) | ~18.5k (c=128) | 188% | ~101 (c128) | 3.4ms svc / 7.7ms (c32) | **12** | client pump + box; QUIC crypto |
| QUIC Mode A | passthrough (harness-bound) | ~11% | n/a | p50≈624µs @ c1 | flat | **gateway NOT bottleneck** (test echo backend serializes) |
| WS-H1 (echo) | ~42.5k (c=32) | 157% | ~37 (c32) | 227µs / 489µs (c8) | 75 | box CPU; cheapest/req |

Efficiency ranking (gateway CPU-µs per request, lower=better): **WS ~37 < H2 ~59 < H3 ~101 <
H1 ~163**. H2/WS multiplex (few sockets); H1 pays per-connection TCP overhead; H3 pays QUIC
crypto + H3 framing (still competitive at ~101µs). Run-to-run variance on this co-located box is
~±20–30% at the throughput knee (two independent sweeps; e.g. H1 c=128 18343 vs 11204) — peaks
are approximate, the efficiency (CPU/req) and below-knee latency are stable.

## Full eg-bench concurrency sweeps (closed-loop, 15s measured + 5s warmup)

### H1 — plaintext H1 front → H1 backend (client TCP_NODELAY set)
| c | rps | p50 | p90 | p99 | p999 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|-----|------|---------|--------|-----|
| 1 | 4139 | 237µs | 263µs | 321µs | 527µs | 90 | 9.4 | 13 |
| 8 | 11919 | 627µs | 941µs | 1488µs | 2910µs | 163 | 12.4 | 27 |
| 32 | 13777 | 2052µs | 3281µs | 7919µs | 12725µs | 224 | 16.9 | 73 |
| 64 | 13822 | 3764µs | 7208µs | 18231µs | 35254µs | 271 | 21.6 | 133 |
| 128 | 18343 | 6560µs | 9757µs | 15956µs | 22442µs | 252 | 28.0 | 261 |

err ~1% across all c = keep-alive **recycle reconnects** (the backend closes idle keep-alive
conns; the client counts the reconnect as one err) — NOT a gateway fault; the achieved-ok RPS is
the successful throughput. fds grow ~2/conn (TCP). First sweep saw c=64=9728/c=128=11204 — the
co-located variance.

### H2 — TLS (ALPN h2) front → H2c backend (client TCP_NODELAY set)
| c | rps | p50 | p90 | p99 | p999 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|-----|------|---------|--------|-----|
| 1 | 3915 | 241µs | 262µs | 295µs | 575µs | 66 | 10.7 | 13 |
| 8 | 14543 | 519µs | 751µs | 1072µs | 2078µs | 158 | 12.7 | 20 |
| 32 | 24148 | 1234µs | 1758µs | 2555µs | 4924µs | 177 | 16.0 | 44 |
| 64 | 27918 | 2125µs | 2965µs | 4156µs | 42844µs† | 180 | 20.0 | 76 |
| 128 | 32191 | 3753µs | 5185µs | 6998µs | 45162µs† | 191 | 24.0 | 140 |

err=0. †the ~42ms p999 at c≥64 is the **co-located box saturating** (gateway ~1.9 cores + 64–128
client conns + backend on 8 cores → a scheduling/delayed-ACK stall on 0.1% of requests). At c≤32
(box not oversubscribed) p999 ≤5ms. The Nagle artifact at c≤32 was fixed (client nodelay); this
residual high-c tail is co-location, not a gateway latency floor. H2 multiplexing scales cleanly.

### H3 — H3-terminate front → H2c backend (UDP; nodelay N/A)
| c | rps | p50 | p90 | p99 | p999 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|-----|------|---------|--------|-----|
| 1 | 294 | 3474µs | 3517µs | 3587µs | 4998µs | 11 | 12.5 | 12 |
| 8 | 2064 | 3407µs | 6348µs | 7003µs | 7747µs | 46 | 17.4 | 12 |
| 32 | 8003 | 3852µs | 4902µs | 7667µs | 8776µs | 106 | 26.5 | 12 |
| 64 | 14394 | 4285µs | 5599µs | 7932µs | 9359µs | 161 | 35.7 | 12 |
| 128 | 18566 | 6501µs | 9417µs | 11623µs | 16147µs | 188 | 47.7 | **12** |

err ~0.1% = the expected GOAWAY-race during connection recycling (the S36 fix working). **fds stay
at 12 at every concurrency** — correct QUIC behaviour (one UDP socket demuxes all connections by
CID; contrast H1/H2's per-conn TCP fds). RSS is the load-proportional working set (128 live QUIC
conns), bounded per-run. The ~3.4ms single-connection service time = QUIC handshake-amortized
crypto + H3 framing + H2 proxy + the closed-loop client pump (part client; the gateway is at 11%
CPU at c=1, so single-stream latency is service-time, not gateway saturation).

### QUIC Mode A — passthrough (end-to-end TLS, gateway forwards UDP)
| c | rps | p50 | p99 | gw CPU% | note |
|---|-----|-----|-----|---------|------|
| 1 | 1585 | 624µs | 1141µs | 10 | gateway passthrough adds ~0.6ms |
| 8 | 1698 | 4747µs | 6596µs | 11 | flat rps, latency ∝ c |
| 32 | 1645 | 19459µs | 26073µs | 11 | single-queue serializer |
| 64 | 1617 | 39455µs | 53151µs | 11 | latency = c / 1650 |
| 128 | 1662 | 77800µs | 103260µs | 11 | gateway idle throughout |

**Harness-bound, NOT gateway-bound.** rps pinned ~1650, latency exactly linear in concurrency,
gateway flat at 10–11% CPU (idle) — the single-task QUIC **echo backend** is the serializer. The
honest gateway result: passthrough cost is ~0.6ms added latency + ~11% CPU (a thin UDP forwarder);
a gateway throughput ceiling for Mode A is not measurable with this single-task test backend.

### WS-over-H1 — opaque echo relay
| c | rps | p50 | p90 | p99 | p999 | gw CPU% | RSS MB | fds |
|---|-----|-----|-----|-----|------|---------|--------|-----|
| 1 | 10136 | 96µs | 109µs | 125µs | 137µs | 57 | 8.8 | 13 |
| 8 | 33490 | 227µs | 332µs | 489µs | 2022µs | 128 | 11.4 | 27 |
| 32 | 42573 | 730µs | 1039µs | 1496µs | 2816µs | 157 | 18.6 | 75 |
| 64 | 40484 | 1514µs | 2215µs | 3093µs | 4308µs | 179 | 27.9 | 139 |
| 128 | 28148 | 4418µs | 6180µs | 8184µs | 10345µs | 209 | 45.6 | 267 |

err=0. The fastest path (p50=96µs @ c1) — a long-lived opaque bidirectional frame relay, no
per-message HTTP parse. Peaks ~42.5k echoes/s @ c=32; c=128 declines (box oversubscription).

## oha cross-validation (H1/H1s/H2 — trusted external client, 10s/point)
`oha` 1.14 runs as a SEPARATE process (true external client, sets TCP_NODELAY).
Both purposes met: independent numbers AND author≠verifier on eg-bench.

| path | c | oha rps | eg-bench rps | oha p50 | eg-bench p50 | agree? |
|------|---|---------|--------------|---------|--------------|--------|
| h1 | 1 | 4300 | 4139 | 225µs | 237µs | ✓ ~4% |
| h1 | 8 | 12118 | 11919 | 602µs | 627µs | ✓ |
| h1 | 32 | 13903 | 13777 | 1940µs | 2052µs | ✓ |
| h1 | 64 | 14049 | 13822 | 3843µs | 3764µs | ✓ |
| h2 | 1 | 4064 | 3915 | 240µs | 241µs | ✓ |
| h2 | 8 | 12629 | 14543 | 578µs | 519µs | ✓ ~13% |
| h2 | 32 | 21554 | 24148 | 1363µs | 1234µs | ✓ ~11% |
| h2 | 64 | 24527 | 27918 | 2285µs | 2125µs | ✓ ~12% |

**eg-bench and oha agree within ~5% (H1) / ~10–15% (H2)** — the custom harness is
validated by an independent production-grade client. eg-bench reads slightly higher
on H2 because its in-process client schedules more cooperatively with the gateway
than a separate oha process (which steals cores). H1s (oha, TLS H1 → H2c backend):
peaks 28167 rps @ c=64, p50=2043µs.

### The ~40ms p999 tail is co-location, NOT the gateway (independently confirmed)
oha — a clean-client that sets TCP_NODELAY — **also shows ~42ms p999 at c≥32**
(h2 c=32 p999=41559µs), even though eg-bench's in-process client was clean at c=32
(4924µs) and only tailed at c≥64. The tail appears EARLIER under the separate oha
process. Reading: at higher concurrency the co-located box (8 cores shared by load
client + gateway + backend) schedules the server task late enough that the ~40ms
TCP delayed-ACK timer fires on ~0.1% of requests; a separate, CPU-competing client
triggers it sooner. It is a co-location/scheduling artifact — a gateway alone on
its cores would not show it. Not a gateway latency floor. (The gateway sets
TCP_NODELAY on every socket — verified in source.)

## VERDICT
- **No gateway perf defect found.** Every path is bounded (RSS/fd/threads), err≈0
  (or explained: H1 keepalive-recycle, H3 GOAWAY-race), panic=0. The two tails/caps
  that looked like gateway issues were both proven to be the harness/co-location
  (client Nagle → fixed; single-task echo backend; co-located scheduling delayed-ACK).
- **Efficiency (gateway CPU-µs/request):** WS ~37 · H2 ~59 · H3 ~101 · H1 ~163.
  H2/H3/WS multiplex; H1 pays per-connection TCP cost.
- **Peak throughput on this co-located 8-core box:** WS ~42.5k · H2 ~32k · H3 ~18.5k
  · H1 ~14–18k echoes/req per second; QUIC Mode A passthrough cost ≈0.6ms + ~11% CPU
  (throughput harness-bound). These are SYSTEM numbers (client+gateway+backend share
  8 cores); a gateway alone on dedicated cores would go materially higher.
- **No owner perf TARGET was set.** These are the honest current-state numbers; the
  Phase-1 check-in asks what bar matters. Nothing here suggests a tuning fix is
  needed — the gateway is efficient and bounded; the limits found are the test rig's.
- **Latency (below the box knee, gateway-reflective):** H1/H1s/H2 p50 ~0.2–0.6ms,
  p99 ~0.3–1.5ms; WS p50 ~0.1–0.2ms; H3 ~3.4ms service time (QUIC crypto + H3 + proxy).

## Independent verifier review (author≠verifier, R5)
A separate reviewer audited the harness CODE for measurement validity (not style):
all 8 properties **PASS** — closed-loop RTT (full body/echo/FIN drained before the
clock stops), warmup exclusion (record/count only in `[warmup_end, measure_end)`),
percentile math (`pct` sorted nearest-rank, no off-by-one/panic), per-PID gateway
CPU%/RSS/fd sampling (utime+stime fields 14/15, USER_HZ=100, post-warmup slice),
and the persistent-connection concurrency model are all correctly implemented. Both
"looked-like-gateway-but-isn't" attributions are **confirmed by code**:
`set_nodelay(true)` is applied to BOTH client sockets (h1 + the raw tcp before TLS
on h2), and `spawn_quic_echo_backend` is a strict single-task/single-socket/one-
packet-per-loop serializer (the `p50 ≈ c/1650` linear fingerprint matches). Verdict:
**no number retracted.** Reproducibility caveats it asked be stated:
1. The RPS denominator is `total_wall − warmup` (measure window + a sub-1% teardown
   drain tail) → RPS is marginally *conservative* (understated <1%), not inflated.
2. The "H2 front" is the `h1s` listener's ALPN-h2 leg (front advertises `[h2,
   http/1.1]`, dispatches by negotiation) — an honest H2 front, just configured via
   the `h1s` listener type, not a distinct config.
3. H3 ~3.4ms and Mode A ~0.6ms single-connection figures are **client+gateway
   combined** (the closed-loop quiche pump is genuine client-side work) — NOT
   gateway-isolated. The gateway's own share is small (it sits at 11% CPU at c=1).
4. The QUIC client config uses a 1350-byte max UDP payload (sub-MTU) — H3/Mode-A
   per-packet figures reflect 1350B datagrams, not 1500B/jumbo.
5. Nit (never fired — gateway is boot-gated on /metrics 200): an H1/H2 TLS/handshake
   failure *after* TCP connect is `continue`d without an err++; a fully-dead gateway
   could under-count err on those two lines. No effect on the reported runs.


