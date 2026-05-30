# SESSION 20 — Chaos/Soak Suite + First Full-System Soak — REPORT

Status: **DRAFT (soak in progress)** — data-dependent sections marked
`PENDING-COMPLETED-RUN` are filled from the COMPLETED soak's time-series only
(R15). Mechanism + design sections are final.

Base: `feature/chaos-soak-s20` off main `6f9c3523` (S19 promoted, native QUIC
complete). 8-core box, ENA ens5, co-located load (owner-ruled acceptable for a
behavioral/stability soak).

---

## 1. R1 baseline (Phase 0)

`6f9c3523` confirmed; branch + push clean; no S19 strays; ≥25 GB free.
Single-pass baseline GREEN: `fmt --check` clean; `clippy --workspace
--all-targets --all-features -D warnings` exit 0; `cargo test --workspace
--all-features` = **1392 passed / 0 failed / 18 ignored**. The additive
suite's own 40 harness unit tests join the gate. Final ×3 gate:
`PENDING-COMPLETED-RUN`.

## 2. The suite (Phase 1) — `crates/lb-soak` (permanent infra, R11)

External-process soak: launches the REAL `expressgateway` release binary as a
child, drives co-located in-process load + chaos, samples the child's `/proc`
(RSS/fd/threads) + `/metrics` (state-table gauges) into a per-scenario CSV
time-series, then computes a BOUNDED/DRIFT trend verdict. Links NO product
crates (black-box driver). Design of record: `audit/soak/s20-suite-design.md`.

Modules (single-sourced, R12; 40 unit tests; clippy/fmt clean):
`procstat` (/proc footprint) · `metrics` (Prometheus text parser) ·
`timeseries` (CSV + warmup-trimmed median-thirds + slope + monotone-frac trend
analyzer; Trend/Counter/CounterMustBeZero kinds) · `config_gen` (per-datapath
TOML + 0600 self-signed certs) · `backends` (hyper H1/H2 + multi-conn quiche
echo) · `loadgen` (hyper H1/H2 + quiche QUIC drivers) · `chaos` (conn-flood,
slowloris, slow-POST, mid-stream-disconnect, oversize-413-teardown,
rapid-reset, stream-flood) · `gateway` (spawn + `/metrics` readiness +
SIGTERM-reap) · `sampler` (periodic scrape + heartbeat) · bin `eg-soak`
(scenario orchestrator).

### Apparatus validation (Phase 1b) — found + fixed 2 apparatus bugs in smoke

Validated against the real release binary across every datapath. Smoke fixes:
- H2 load/chaos clients used origin-form URIs (`/`); hyper's H2 client requires
  ABSOLUTE-form (`https://authority/`). Fixed all 4 H2 request builders.
- TLS load client rejected the gateway's `is_ca` loopback cert
  (`CaUsedAsEndEntity`); replaced CA-trust with an accept-any verifier (a
  loopback LOAD client, never product) — QUIC's `is_ca` certs unchanged.
Smoke confirmed real traffic on every path (sc1 H1 ok=43k; sc2 H2 ok=200k +
rapid_reset ok=1.07M; sc3 slowloris clean; sc4 Mode B 1-stream ok=3.3k; sc5
Mode A ok=3.6k; sc6 ok). The smoke ALSO surfaced both findings below.

## 3. The soak run (Phase 2)

Scenarios (each = own gateway child + load/chaos + `/proc`+`/metrics` sampler →
own CSV): sc1 H1→H1+conn-flood · sc1b H1→H2+conn-flood · sc2 H2-over-TLS +
rapid-reset + stream-flood · sc3 slowloris + slow-POST · sc4 Mode B (4 streams,
exposes F-S20-1) · sc4b Mode B healthy (1 stream baseline) · sc5 Mode A flood ·
sc6 413-teardown. Sampler @ 15 s.

**Run management (honest record):**
- run1 (all 8 concurrent, default concurrency): the box OVER-saturated (load
  44) — the OS, not the gateway, became the bottleneck: 162 upstream-handshake
  timeouts + one gateway OOM-killed (no panic in its log). Per R2 (don't let
  environmental artifacts pollute findings) the run was aborted at ~35 min and
  the partial time-series archived (`s20-soak-data/run1-partial-overloaded/`).
  It still yielded valid evidence: sc5 Mode A flows climbed LINEARLY 0→62k with
  zero eviction (F-S20-2), and the clean scenarios stayed bounded EVEN under
  thrash.
- run2 (per-scenario concurrency halved+; sc5 throttled): clean. sc5 Mode A was
  run to characterization then SURGICALLY REMOVED (it is the box's resource
  polluter — see F-S20-2); the other 7 scenarios soaked clean.
  Duration 90 min. `PENDING-COMPLETED-RUN` for the final per-scenario verdicts.
- sc5 Mode A (F-S20-2) verdict: from a dedicated ISOLATED run (Phase 3).

Coverage honesty (R2/R4 — no asterisks): the soak drives the TCP-front cells
(H1/H2×H1/H2), Mode A, Mode B, and the named priority chaos. H3-front cells
(H3→H1/H2/H3) are NOT separately load-driven (Mode B exercises QUIC-terminate
state; functional H3 paths are covered by the S7–S13 md_streaming_verify
suites) — carried to S21, stated not asterisked. XDP/L4 soak needs root +
multi-kernel (F-ESC-1) — out of scope for this userspace-datapath soak.

## 4. Findings (Phase 3)

See `audit/soak/s20-findings-wip.md` for the full mechanism notes. Summary:

### F-S20-1 — Mode B multi-concurrent-stream relay STALL (priority #1, S18 lineage)
Tier: STABILITY/LIVENESS. With ≥2 concurrent client bidi streams (payload +
immediate FIN) on one Mode B connection, the relay reliably fails to deliver
one stream's echo+FIN to the client (client relay-timeout). Bisected
deterministic; Mode A with the IDENTICAL client + 4 streams works → relay-
specific, not apparatus. Mechanism locus: `raw_proxy.rs` stream relay; S18
RELAY-STALL class under multi-stream concurrency the s16_b2_multistream test
did not exercise. Exact stream + data-vs-FIN: `PENDING-DIAGNOSTIC-RUN`.
State-leak under sustained load: `PENDING-COMPLETED-RUN` (run2 sc4 mid-run
preliminary: RSS/fd flat — stall does not appear to leak state).
Reproduced-by-verifier: `PENDING (R13)`. Disposition: characterize + verify
this session; FIX is S21 (relay-logic correctness fix + deterministic
multi-stream regression test).

### F-S20-2 — Mode A passthrough flow/fd/task retention, no idle reclaim (priority #4)
Tier: STABILITY/RESOURCE (+ effective accept-DoS). Each `FlowEntry` pins a
backend UDP socket (fd) + 2 pump tasks; reclamation is LRU-only at
2×`max_quic_connections` (200k default), NO idle sweep. Passthrough can't see
encrypted CONNECTION_CLOSE, so closed-connection flows persist. Evidence: run1
linear 0→62k flows (no plateau, evicted=0); run2 rapid 0→56k then SATURATION
plateau BELOW the LRU cap — the gateway saturates on unreclaimed-flow recv-
timeout wakeups and stops accepting new connections (effective accept-DoS at
~56k retained flows). Isolated verdict run: `PENDING`. Reproduced-by-verifier:
`PENDING (R13)`. Fix (S21): periodic idle-timeout sweep evicting flows past an
idle threshold (Katran/Pingora-style) — bounds the table by LIVE connections.

## 5. Clean scenarios (bounded evidence) — `PENDING-COMPLETED-RUN`
(run2 mid-run preliminary @ t≈43 min, all bounded, panic=0: sc1/sc1b/sc2 RSS
flat; sc3 slowloris accept_inflight steady 148; sc4/sc4b Mode B RSS/fd flat;
sc6 flat. Final BOUNDED verdicts + time-series from the completed run.)

## 6. R1 ×3 regression gate — `PENDING-COMPLETED-RUN`

## 7. S21 handoff (prioritized fix list) — see `audit/soak/s21-handoff.md`

## VERDICT
`PENDING` → "SESSION 20 COMPLETE — suite built, soak run, N findings
characterized (M fixed, K carried to S21)".
