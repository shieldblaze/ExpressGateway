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
