# SESSION 20 — Chaos/Soak Suite + First Full-System Soak — REPORT

Base: `feature/chaos-soak-s20` off main `6f9c3523` (S19 promoted, native QUIC
complete). 8-core box, ENA ens5, co-located load (owner-ruled acceptable for a
behavioral/stability soak). Verdict from the COMPLETED run only (R15).

Outcome: **the chaos/soak suite now EXISTS** (permanent infra), the **first
real soak ran to completion**, and it found **2 real defects** + proved the
system otherwise **survives sustained hostile load with bounded R8 state**.
Native-QUIC-complete + soak-suite-exists ≠ production-ready until the findings
are fixed and a clean re-soak passes (S21).

---

## 1. R1 baseline (Phase 0)

`6f9c3523` confirmed; branch + push; no S19 strays; ≥25 GB free. Single-pass
baseline GREEN: `fmt --check` clean; `clippy --workspace --all-targets
--all-features -D warnings` exit 0; `cargo test --workspace --all-features` =
**1392 passed / 0 failed / 18 ignored**. Final additive ×3 gate: see §6.

## 2. The suite (Phase 1) — `crates/lb-soak` (permanent infra, R11)

External-process soak: launches the REAL `expressgateway` release binary as a
child, drives co-located in-process load + chaos, samples the child's `/proc`
(RSS/fd/threads) + `/metrics` (state gauges) into a per-scenario CSV
time-series, then computes a warmup-trimmed median-thirds + slope + monotone
BOUNDED/DRIFT trend verdict. Links NO product crates (black-box driver).
Design: `s20-suite-design.md`. Modules (single-sourced R12; **40 harness unit
tests**; clippy/fmt clean): procstat · metrics · timeseries · config_gen ·
backends · loadgen · chaos · gateway · sampler · bin `eg-soak`.

### Apparatus validation (Phase 1b) — 2 apparatus bugs found+fixed in smoke
Validated against the real release binary on every datapath. Fixes: (a) H2
clients needed ABSOLUTE-form URIs (hyper H2 requirement); (b) the TLS load
client rejected the gateway's `is_ca` loopback cert (`CaUsedAsEndEntity`) →
accept-any verifier for the loopback LOAD client (never product). Smoke also
surfaced both findings. The apparatus correctly drove real traffic on every
path and faithfully measured the gateway child's footprint (RSS cross-checked
/proc-vs-ps by the verifier, §4).

## 3. The soak run (Phase 2)

8 scenarios, sampler @ 15 s. **Run management (honest record):**
- **run1** (all 8, default concurrency): the BOX over-saturated (load 44) — the
  OS, not the gateway, became the bottleneck (162 upstream-handshake timeouts +
  one gateway OOM-killed, NO panic in its log). Per R2 the run was aborted at
  ~35 min; partial time-series archived (`s20-soak-data/run1-partial-overloaded/`).
  Still yielded evidence: sc5 Mode A flows climbed LINEARLY 0→62k, evicted=0;
  clean scenarios stayed bounded EVEN under thrash.
- **run2** (per-scenario concurrency halved+; sc5 throttled): clean, 90 min,
  **7 scenarios to completion, 361 samples each, ~0 NaN cells**. sc5 Mode A was
  characterized then surgically removed (it is the resource polluter — F-S20-2).
  Archived: `s20-soak-data/run2-clean/`.
- **sc5 Mode A** (F-S20-2): dedicated ISOLATED verdict run (clean box), 150 s.
  Archived: `s20-soak-data/sc5-isolated/`.

**Coverage honesty (R2/R4 — no asterisks):** drove the TCP-front cells
(H1/H2×H1/H2), Mode A, Mode B, and the named priority chaos. H3-front cells
(H3→H1/H2/H3) NOT separately load-driven (Mode B exercises QUIC-terminate
state; functional H3 covered by S7–S13 md_streaming_verify) → S21. XDP/L4 soak
needs root + multi-kernel (F-ESC-1) → out of scope for this userspace soak.

## 4. Completed-run VERDICTS (R15)

**All 7 run2 scenarios BOUNDED over 90 min; panic_total=0 everywhere.** The
system upholds its R8 bounded-state invariants (RSS / fd / connection-table /
stream-table / thread-count all bounded) under sustained hostile load.

| Scenario | Verdict | Key evidence (90 min / 361 samples) |
|----------|---------|--------------------------------------|
| sc1 H1→H1 + conn-flood | BOUNDED | RSS 25016↔24964 KB (+0.2%); h1 ok=7.8M, conn-flood ok=79.5M, 0 err |
| sc1b H1→H2 + conn-flood | BOUNDED | RSS +0.4%; h1 ok=6.0M, conn-flood ok=94.9M, 0 err |
| sc2 H2 + rapid-reset + stream-flood | BOUNDED | RSS +1.0%, fds flat 18; **rapid-reset ok=227.7M**, 0 err — CVE-2023-44487 defense holds, zero growth |
| sc3 slowloris + slow-POST | BOUNDED | accept_inflight steady 148, fds flat 209; baseline ok=15.8M, 0 err — timeouts reap |
| sc4 Mode B (4-stream) | BOUNDED state | RSS flat 17944 KB, fds 11, conns≤4, streams≤4; **quic_load ok=0/err=23349** (the F-S20-1 stall) — state bounded despite 100% session failure |
| sc4b Mode B healthy (1-stream) | BOUNDED | RSS flat 29588 KB, fds 11; quic_load ok=18475/err=8208 (errs = co-located-box handshake timeouts, not a leak) |
| sc6 413-teardown | BOUNDED | RSS flat, fds 15; oversize-teardown ok=2.4M **err=0**, mid-stream ok=9.8M |

Independent verification (R13): `PENDING-VERIFIER` (see §7 when filled).

## 5. FINDINGS (Phase 3) — 2 real defects, both characterized + tiered

Full mechanism notes: `s20-findings-wip.md`.

### F-S20-1 — Mode B multi-concurrent-stream relay STALL (priority #1; S18 lineage)
**Tier:** STABILITY/LIVENESS (bounded state — does NOT leak). **Carry to S21.**
- **Trigger (proven, deterministic):** EXACTLY 4 concurrent client bidi streams
  on one Mode B connection → stall; 3 streams works (`ok=20533/err=0`), 4 →
  `ok=0/err=6`.
- **Symptom:** the 4th/highest stream **sid12** receives only **1212 of 4096**
  echo bytes then wedges (no further data, no FIN); the connection idle-closes.
  A **partial-data relay stall** (NOT pure FIN-loss).
- **Isolation:** Mode A with the IDENTICAL client + 4 streams works → gateway-
  relay-specific, not apparatus.
- **NOT a config limit:** front grants `initial_max_streams_bidi(16)`/10 MB
  (listener.rs:439-443), upstream `(64)`/10 MB (main.rs:861-865) — both far
  above 4 streams / 16 KB. So it is a relay-LOGIC interaction at 4 concurrent
  streams.
- **Completed-run impact:** sc4_modeb soaked 90 min at 100% session failure yet
  RSS/fd/conn/stream all FLAT → liveness bug, no state leak.
- **S21 fix-investigation start:** trace `relay_streams`/`run_dual_pump`
  (raw_proxy.rs) under 4 concurrent bidi streams; repro = 4-stream knee +
  highest-stream + 1212/4096-partial signature. Needs a deterministic
  multi-stream regression test.

### F-S20-2 — Mode A passthrough flow/fd/task retention, no idle reclaim (priority #4)
**Tier:** STABILITY/RESOURCE + effective accept-DoS. **Carry to S21.**
- **Mechanism (code):** each `FlowEntry` (passthrough.rs:193) pins a backend
  UDP socket fd + 2 pump tasks; reclamation is LRU-only at
  2×`max_quic_connections` (200k default), **NO idle sweep** (the only
  `tokio::time` is a per-recv timeout, L1418). Passthrough can't observe the
  encrypted CONNECTION_CLOSE, so closed-connection flows persist.
- **Isolated verdict run = DRIFT:** flows 0→56457 (monotone 100%, slope
  1009/sample), fds 11→28240 (DRIFT; verifier measured **0.5 fd/flow** = ~1
  backend UDP socket per flow), RSS 8→330 MB (DRIFT), **evicted_total=0
  throughout** (zero reclaim). Corroborated by run1 (0→62k linear) + run2 + the
  independent verifier (§7).
- **Plateau nuance (R13 correction):** the curve plateaus at ~56k flows at low
  concurrency (and was still climbing at 62k+ at high concurrency in run1).
  This session did NOT definitively isolate WHETHER the plateau is the gateway
  saturating (unable to admit new flows) vs the soak driver reaching a churn
  steady state — `evicted_total` stayed 0 and the fd ulimit (524288) was not
  hit, so it is NOT an LRU or OS-fd cap. The PROVEN finding is the absence of
  idle reclamation (flows only ever rise, never shrink; would only LRU-evict at
  2×cap = 200k); the saturation-knee mechanism is a carry-forward to
  investigate alongside the fix.
- **S21 fix:** add a periodic idle-timeout sweep evicting flows past an idle
  threshold (Katran/Pingora-style) — bounds the table by LIVE connections.
  Needs a regression test (flows reclaim after idle).

### NON-finding (honest): CF-S19-TLS-TEARDOWN-413 did NOT reproduce
sc6's oversize-413-over-TLS + teardown injector ran 90 min / 2.4M attempts with
**err=0** — the S19-escalated TLS-teardown-vs-413-head race did NOT surface with
this injector. NOT cleared — my injector likely doesn't hit the exact
contention window. S21: a more targeted trigger (concurrent TLS renegotiation /
precise mid-413-flush abort). Stated, not asterisked.

## 6. R1 ×3 regression gate — GREEN

`fmt --all --check` clean; `clippy --workspace --all-targets --all-features -D
warnings` exit 0; `cargo test --workspace --all-features` ×3 ALL-PASS (exit 0
each, **0 failed** across all suites): **1432 passed / 0 failed / 18 ignored**
per pass = baseline 1392 + the 40 new `lb-soak` harness unit tests (ADDITIVE,
zero regression to the prior 1392). The suite is gate-clean and does not weaken
any existing gate (R3). (Passes 1–2 from the first gate run — both read to
completion, exit 0, 0 failures — + a fresh pass 3 confirming.)

## 7. Independent verification (R13) — author ≠ verifier

A separate agent independently reproduced both findings with its OWN runs
(lead's numbers not consulted during measurement):
- **F-S20-1: REPRODUCED.** 3 streams → ok=8863/err=0; 4 streams → ok=0/err=3;
  every error `sid12=1212/4096` (bit-for-bit match); Mode A 4-stream control →
  ok=15592/err=0 (gateway-relay-specific); state flat (rss 7.6→11.3 MB, fds
  11→12, conns/streams steady at 1) — no leak.
- **F-S20-2: REPRODUCED.** flows 0→56457, fds 11→28240 (exactly 0.5 fd/flow),
  evicted_total=0 throughout. Code-confirmed: gauge = `table.len()`; eviction
  LRU-only on `len()≥2×cap`; no time/idle path.
- **Measurement sanity:** /proc VmRSS (58032 kB) vs `ps rss` (58088 kB) agree —
  the sampler's RSS is trustworthy.
- **Clean verdicts:** spot-checked sc2 (227M rapid-resets, BOUNDED) + sc4
  (stall but bounded state) — AGREE with the archived summaries.
- **Verifier's refinement (incorporated):** the F-S20-2 plateau is a
  driver/gateway steady state, NOT an LRU or OS-fd cap — see §5 nuance. Core
  finding (no idle reclaim) stands. No other discrepancies.

## 8. S21 handoff — `s21-handoff.md` (prioritized fix list)

## VERDICT
**SESSION 20 COMPLETE — suite built, soak run, 2 findings characterized (0
fixed, 2 carried to S21).**
- The chaos/soak suite now EXISTS (`crates/lb-soak`, permanent infra), is
  self-tested (40 unit tests in the ×3 gate), and ran the first full-system
  soak to completion (run2: 7 scenarios × 90 min × 361 samples).
- The system upholds its R8 bounded-state invariants under sustained hostile
  load: all clean scenarios BOUNDED, panic_total=0, CVE-2023-44487 defense held
  under 227M rapid-resets, slowloris/conn-flood/stream-flood all bounded.
- 2 real defects found, both mechanism-proven + independently reproduced (R13)
  + tiered, carried to S21 with repro + fix shape: F-S20-1 (Mode B 4-concurrent-
  stream relay stall — liveness, no state leak) and F-S20-2 (Mode A passthrough
  flow/fd retention, no idle reclaim). Per R6 the default for soak findings is
  characterize+carry; neither is a one-line fix that lands safely without its
  own design + R13 regression proof (S21's job).
- R1 ×3 gate GREEN (§6); no regression (R3). Promoted to main `--no-ff` (R11).
- Native QUIC complete + soak-suite-exists ≠ production-ready until S21 fixes
  the findings and a clean re-soak passes.
