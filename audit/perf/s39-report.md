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

## §2 — Phase 2: extended burn-in (full detail in `s39-burnin.md`)
Full 12-scenario lb-soak, **14400s (4h)**, 481 samples/scenario, scale 1, all
concurrent (the S31/S33 full-suite pattern). COMPLETED run (R15).

**VERDICT — CLEAN over 4h: 11/12 BOUNDED, 1 analyzer-DRIFT (explained, not a leak),
panic=0 across billions of ops.** R8 confirmed over HOURS: CVE-2023-44487 H2
rapid-reset held **320M resets** bounded; F-MD-4 H3 reset-flood **49M** bounded;
261M conn-floods bounded; WS/gRPC/QUIC all bounded; zero gateway errors on every
non-adversarial path. No slow memory leak, no fd/thread growth — the "pages after
days of uptime" class did not appear.

The one DRIFT (**sc3_slowloris**) is NOT a leak: `rss_kb` BOUNDED (flat ~36MB),
15.5M requests zero errors, panic=0; the DRIFT is on `accept_inflight`/`fds`, which
**capped at ~144 = the harness's offered 144 concurrent slow connections** (96
slowloris + 48 slow-post workers) and plateaued. The analyzer trips on the
contention-delayed ramp (co-located first-2h ~50 → recovered ~146 as the box
settled) — the known sc3 ramp-to-cap false-positive. **Isolated re-run confirmed
BOUNDED** (accept_inflight flat ~52, RSS flat ~25MB — the co-located rise was
co-location-induced reaper slowdown, not gateway accumulation; RSS flat regardless
of accept_inflight = no per-connection buffering = R8 holds). sc5_modea (the
resource-heavy passthrough path, 289MB) plateaued BOUNDED after a ~1h
approach-to-equilibrium that an 1800s spot-check would have mis-flagged.

The extended duration was load-bearing: it correctly classified both sc5_modea
(ramp→plateau→BOUNDED) and sc3 (contention ramp→cap), which short runs would
mis-call. Artifacts: `audit/perf/s39-burnin/` + `audit/perf/s39-sc3-isolated/`.

---

## §3 — Verdict + promote

### Watch-item resolution
- **CF-S37-SC9-PLATEAU: CLOSED** — fresh-box re-check returned to ~23MB (the S36
  baseline), not the S37 ~37MB → the +15MB was box fragmentation (uptime + swap),
  not real B/C/D footprint. The S36 recycling fix holds under 3M RPCs.
- **CF-S38-RELOAD-BOOT-FLAKE / CF-S37-D6-H2PROXY-FLAKY** — not re-triggered this
  session (no reload/H2-proxy flake observed across the perf campaign + 4h burn-in;
  carried as known pre-existing, deterministic-deflake still optional).
- **CF-S37-D-TOKIO-1.52-RELAY** — out of scope this session (its own investigation;
  tokio held <1.52). Carried.

### R1 gate
Full `cargo test --workspace --all-features ×3` is **run by main's CI** (GitHub
runners — adequate disk). Locally (this 67G box is disk-constrained for the ~38–40G
`--all-features` build, CF-DISK-1; the only reclaimable space was the user's
unrelated Java caches, NOT deleted) the **feasible pre-flight gate** ran:
`cargo check --workspace --all-features --all-targets` + `cargo test -p lb-soak
--all-features` (the one crate changed) + clippy `--all-targets -D warnings` + fmt.
Sufficient because the change is **test-harness-only and purely additive** (new
`bench` module + `eg-bench` bin; loadgen + all production crates byte-identical,
reviewer-confirmed; zero new tests). **Result: GREEN** — `check --all-features
--all-targets` 0 errors (whole workspace + every test target type-checks),
`test -p lb-soak --all-features` 49 passed/0 failed, `clippy --workspace
--all-targets --all-features -D warnings` clean, `fmt --check` clean. CI's full
`--all-features ×3` is the authoritative post-merge run.

### Burn-in finding carried (LOW, operability)
**CF-S39-H3-REJECT-LOG-SPAM** — the H3-terminate front logs a WARN per request on
the reject/backendless paths (sc7 produced 24.8M log lines / 3.3G over 4h). A
log-amplification/disk-fill vector under adversarial H3 floods if logs aren't
rate-limited/rotated. Not a correctness/memory issue (RSS bounded). See s39-burnin.md.

### No production source change
This session changed NO production source — only the lb-soak test-harness crate
(additive). So no R3/R5 production-gating applies; nothing to regress. No perf
tuning fix was indicated (the gateway is efficient + bounded; the limits found were
the test rig's, not the gateway's).

### Session verdict
**SESSION 39 COMPLETE — pre-prod validation DONE.**
- Perf baseline established per protocol (no gateway defect; efficiency
  WS<H2<H3<H1; cross-validated + independently reviewed).
- Extended burn-in CLEAN over 4h (R8 holds over hours; the one DRIFT explained +
  isolated-confirmed as a co-location analyzer artifact, not a leak).
- sc9 fragmentation question CLOSED.
- No owner perf TARGET was set; the numbers are the honest current-state and meet a
  reasonable pre-prod bar. The system is VERIFIED-CORRECT, PERFORMANCE-
  CHARACTERIZED, and BURNED-IN → ready for a controlled production pilot.

[GATE RESULT + PROMOTE COMMIT + POST-MERGE CI — appended after the gate runs]
