# SESSION 39 — Performance validation + burn-in + watch-item resolution

Box: c6a.2xlarge, 8 vCPU, 15 GiB RAM, 8 GiB swap, ENA on ens5. Fresh-started for
this session (0-min uptime at start — the sc9 re-check requires a clean box).
Base: `main` @ `18afc8ad` (S38 promoted). Branch: `feature/perf-burnin-s39`.
CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target (shared). 40 GiB free at start.

This is a **MEASUREMENT** session: the deliverable is NUMBERS + VERDICTS, not code
(beyond the bench harness scaffolding). Measure honestly; completed runs only (R15);
author≠verifier reproduction (R5/R13).

---

## §0 — Phase 0: box prep + the sc9 fresh-box re-check (CF-S37-SC9-PLATEAU)

### Box prep (done)
- Branch `feature/perf-burnin-s39` cut from `main` @ `18afc8ad` (confirmed tip).
- Fresh box (uptime 0 min at start — no 11h-fragmentation confound).
- Disk: reclaimed 27 GiB stale `eg-target` → **40 GiB free** (≥25 GiB, R9 / CF-DISK-1).
- Swap: 8 GiB swapfile on (R9; the S33 15 GiB-RAM OOM margin).
- Release binaries built clean (2m14s): `expressgateway` + `eg-soak`.

### sc9_grpc_h3 fresh-box re-check — the load-bearing falsifiable test (R13)
**Hypothesis (CF-S37-SC9-PLATEAU):** S37's sc9 plateau of ~37 MB (vs S36's ~22 MB)
was **box fragmentation** (11h uptime + swap thrash), NOT a real B/C/D footprint.
**Falsifiable test:** fresh box + the S36 load (~3M RPCs, identical params:
sc9_grpc_h3 isolated, 1800s, 12s sample, scale 1) → ~22 MB confirms fragmentation
(CF CLOSED); ≠22 MB (≈37 MB) means the +15 MB is real footprint (CF re-opened,
characterize with heaptrack).

| Run | Box state | RSS last-third median | RSS max | vmhwm max | sustained ok/err | churn ok | verdict |
|-----|-----------|----------------------|---------|-----------|------------------|----------|---------|
| S36 post-fix (ref) | fresh | 21.6 MB (22076 KB) | 22.8 MB | 22.9 MB | 2,997,771 / 2994 | 2,015,632 | BOUNDED |
| S37 (CF trigger) | 11h + swap | ~37 MB | — | — | — | — | the anomaly |
| **S39 (this run)** | **fresh (0-min)** | **23.2 MB (23768 KB)** | **24.1 MB** | **24.2 MB** | **2,891,845 / 2889** | **1,970,468** | **BOUNDED** |

Completed run: 1800s, 151 samples (>> the ~60-sample analyzer floor), panic=0,
fds bounded at 12, threads 9. rel_growth +2.7% (non-monotone 54% — sawtooth, not a
leak). Load faithfully matched (~2.89M sustained ≈ S36's 3.0M within churn variance;
err rate 2889 ≈ S36's 2994 — the GOAWAY-race during connection recycling, expected
and bounded).

**VERDICT — CF-S37-SC9-PLATEAU: CLOSED (fragmentation confirmed).**
On a fresh box, sc9 plateaus at **23.2 MB**, within ~1.6 MB (7%) of the S36 21.6 MB
baseline and nowhere near the S37 ~37 MB. The S37 +15 MB was box-state (uptime +
swap fragmentation), not real B/C/D footprint. The S36 recycling fix
(`max_requests_per_h3_connection`, cap→GOAWAY→drain) holds: per-connection stream
state stays bounded under ~3M RPCs; h3_connections_recycled climbed steadily
(GOAWAY at cap 1000 working). The ~1.6 MB run-to-run delta is allocator high-water
noise between two independent BOUNDED runs (this run did marginally LESS work yet
sat marginally higher — i.e. not load-correlated), not a footprint regression.

Artifacts: `audit/perf/s39-sc9-recheck/` (CSV + verdict.json + summary.txt + marker).

---

## §1 — Phase 1: perf characterization (full detail in `s39-perf-baseline.md`)

Harness: `eg-bench` (closed-loop, per-request RTT; new `bench` module + bin in
lb-soak, loadgen untouched) + `oha` 1.14 (independent cross-validation). Co-located
8-core box (client+gateway+backend share cores). Independent verifier passed all 8
measurement-validity checks; oha agrees with eg-bench within ~5% (H1) / ~10–15% (H2).

**VERDICT — no gateway perf defect.** Every path bounded (RSS/fd/threads), err≈0
or explained, panic=0. Efficiency (gateway CPU-µs/req): **WS ~37 < H2 ~59 < H3 ~101
< H1 ~163**. Peak rps on the co-located box: WS ~42.5k · H2 ~32k · H3 ~18.5k · H1
~14–18k; QUIC Mode A passthrough ≈0.6ms + ~11% CPU (throughput harness-bound by the
single-task echo backend). Below-knee latency: H1/H2 p50 0.2–0.6ms p99 0.3–1.5ms;
WS p50 ~0.1ms; H3 ~3.4ms service time (QUIC crypto + H3 + proxy; part client pump).

**Two apparent issues — both proven harness/co-location, not the gateway** (R9 /
the S21 measure-first lesson):
1. ~40ms p999 H2 tail → load-client missing `TCP_NODELAY` (Nagle on H2 control
   frames); fixed → 41ms→7.3ms. Residual high-concurrency tail is co-located
   scheduling/delayed-ACK, **reproduced by the clean oha client** → not a gateway
   floor (the gateway sets nodelay on every socket, verified in source).
2. QUIC Mode A capped ~1650 rps → the single-task test echo backend serializes;
   gateway passthrough idle at 11% CPU.

**No owner perf TARGET set** → these are the honest current-state numbers; nothing
indicates a tuning fix is needed (no Phase-3 source change required on perf grounds).

---

## §2 — Phase 2: extended burn-in  [IN PROGRESS]
Full 12-scenario lb-soak, 14400s (4h) @ 30s sample, scale 1, all concurrent (the
S31/S33 full-suite pattern). Watching slow-leak/fd-growth/degradation over hours.
Early (t~150s): 12/12 alive, panic=0; sc5_modea (passthrough) is the resource-heavy
path (216MB/1307fd, flows churning with eviction firing — the F-S20-2 reclaim) —
the one to watch for plateau vs growth. Verdict pending the COMPLETED run (R15).
