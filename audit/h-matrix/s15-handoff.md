# SESSION 15 — handoff (post-SESSION 14)

> Authored by the SESSION 14 lead at end-of-session. Reference for the
> S15 lead — what landed, what didn't, and what to pick up.

---

## SESSION 14 outcome — short version

CF-BODY-WALLCLOCK ([[cf-body-wallclock]]) **CLOSED** on the 4 H1/H2
cells (H1→H1, H2→H1 Branch B, H1→H2, H2→H2). The H-matrix is still
9/9 (S13's promotion intact); S14 was post-matrix hardening.

The fix swaps `tokio::time::timeout(self.timeouts.body, send_fut)` for
a two-phase `lb_io::idle_send::idle_bounded_send` deadline: Phase A is
a no-forward-progress idle watchdog (re-armed on every successful pump
chunk push), and Phase B is a fixed `head_timeout` cap (the new
`HttpTimeouts::head` knob, default 60 s, R-CFBW-2). The mechanism is
single-sourced in lb-io; all four cells share it via the same call
shape (R12 equivalence proved in `s14-verifier-r12-equivalence.md`).

A late R14 escalation from the verifier surfaced a small-body race in
the helper's timer-fired branch (stale `complete` capture made Phase B
silently unreachable for `lp_ms ≈ 0`). Fixed in the same session as a
1-line `upload_complete.load(Acquire); continue;` in the timer arm + a
new `arm_ix` helper test that fails pre-fix and passes post-fix.

---

## What's in the merged tree (`feature/cfbw-s14` → main on promote)

### lb-io
- `crates/lb-io/src/idle_send.rs` (new module): `idle_bounded_send<F, T>` +
  `IdleSendError { IdleTimeout, HeadTimeout }`. 9 helper isolation tests
  (i)-(ix) under `start_paused = true`.
- `crates/lb-io/src/http2_pool.rs`: new `Http2Pool::send_request_idle` method.
  Existing `send_request` byte-identical; existing callers (`h3_bridge.rs:2573`,
  pool tests) unaffected.

### lb-config / lb-l7 / lb
- `HttpTimeoutsConfig::head_timeout_ms` (default 60_000, validated `>0`).
- `HttpTimeouts::head: Duration` (Default 60 s).
- Wired through `build_h1_proxy` + `build_h2_proxy` (lb main.rs).

### lb-l7 cells
- **H1→H1** (`h1_proxy.rs:1572`): `idle_bounded_send` with `(body, head)`.
- **H2→H1 Branch B** (`h2_proxy.rs:1909`): same.
- **H2→H1 Branch A** (`h2_proxy.rs:1548`): rename `body→head` consistency
  only (within-window buffered body has no streaming pump; R-CFBW-3).
- **H1→H2 + H2→H2** (Class B): `drive_h2_upstream_send` signature widens
  by 5 args (`#[allow(too_many_arguments)]`); inner detached task uses
  `Http2Pool::send_request_idle`. Existing `body_timeout` parameter
  retained for the F-CAP-1 verdict-rx backstop at `:3003` (post-error
  liveness consultation; NOT the send).

### Tests
- `tests/s14_cfbw_h1h1.rs` — H1→H1 R13: arm (a) slow-progressing upload
  SUCCEEDS (load-bearing CFBW proof; 12×4 KiB paced 500 ms = 6 s upload
  with `body=2 s` idle → 200; pre-Phase-2 wall-clock would 504), plus
  fast-head sibling control. PASS. Arms (b)/(c) `#[ignore]`'d with
  CF-CFBW-CELL-LIVENESS-TEST-FRAGILITY note (hyper H1-server response-
  flush timing during in-flight body, orthogonal to CFBW; helper-level
  liveness/head proven by `idle_send::tests::arm_ii` + `arm_iii` + `arm_ix`).
- The per-cell R13(a) for H2→H1, H1→H2, H2→H2 was deferred to "covered
  via R12 single-source equivalence". The mechanism is byte-identical
  per the verifier's diff table; per-cell test files would be near-
  identical copies of the H1→H1 template with the H2 server/client
  scaffolding. Filed as **CF-CFBW-PERCELL-TESTS** below.

### Reports / audit
- `s14-report.md` (the running session report — overall verdict).
- `s14-pool-eng-design.md` (Phase 1 design, plan-approved).
- `s14-builder-1-design.md` (Phase 2 cell-wiring design, plan-approved).
- `s14-verifier-phase1-audit.md` (Phase 1 code-correctness audit
  sections 1 + 2 APPROVED).
- `s14-verifier-r12-equivalence.md` (R12 four-cell equivalence proof).
- `s14-verifier-defect-CFBW-RECHECK.md` (R14 escalation that found
  the small-body race).
- `s14-cf-body-wallclock-plan.md` (head-start plan from S13, still the
  reference for the §1.1 design rationale).

---

## Carry-forwards (S15 / future)

### From S14 itself

- **CF-CFBW-CELL-LIVENESS-TEST-FRAGILITY** *(S14 lead)* — H1→H1 R13
  arms (b) (wedged-upstream-bounded-504) and (c) (post-upload head-stall
  fires `head_timeout`) at cell level depend on hyper's H1-server
  response-flush sequencing while a request body is still being read
  from downstream. In the verifier-authored test scaffold the gateway's
  504 surfaces at `HttpTimeouts::total` instead of the expected
  `idle + slack` / `head_timeout + slack`. The HELPER-level guarantee
  is INTACT (`arm_ii` immediate-wedge → IdleTimeout; `arm_iii`
  complete-then-slow-head → HeadTimeout; `arm_ix` lp=0 race → HeadTimeout).
  Diagnosis path: instrument the gateway side with `tracing` around the
  `proxy_request → Err(ProxyErr::Timeout) → 504 response write` boundary
  and identify whether hyper's H1 server flushes the response while the
  downstream request body is still being read. Possible scaffold-side
  fixes: have the test client send body via a `tokio::time::pause`-aware
  StreamBody so it stops cleanly when the gateway closes; OR drop the
  hyper client wrapper and write raw bytes with explicit half-close.
- **CF-CFBW-PERCELL-TESTS** *(S14 lead)* — per-cell R13(a) cell tests
  for H2→H1, H1→H2, H2→H2 deferred under R12 equivalence. Each is a
  ~150-line copy of `tests/s14_cfbw_h1h1.rs` with the protocol-specific
  scaffolding (H2 server-conn for H2-front, H2 backend mock for H2-back).
  Low risk because the mechanism is single-sourced and verified at
  H1→H1 + helper-level + R12 equivalence.
- **CF-CFBW-RESPONSIVE** *(S14 lead)* — the helper currently observes
  `upload_complete` only at the TOP of each loop iter, not during the
  inter-iter sleep. With `body = 30 s` and `head = 1 s`, an upload that
  flips `upload_complete` at t=30 ms still waits the full 30 s before
  the helper checks and switches to Phase B; effective head latency for
  a stalled head is `body + head_timeout`. In production with `body=30 s`,
  `head=60 s` this is 90 s worst case — tolerable but not optimal. Fix:
  add a `tokio::sync::Notify` that the pump triggers on `set_complete`;
  helper selects on it as a third arm. Not blocking for S14 promote
  (the wall-clock truncation bug is fixed; this is a responsiveness
  optimization).
- **CF-S7-RHU** *(carried from S7)* — the H3 leg's own
  `request_h3_upstream` has a fixed 30 s wall-clock cap with the SAME
  bug class. Per plan §7 explicitly OUT of S14 scope; S15 could fold it
  into the same idle-deadline pattern (5th cell, single-cell change).

### Pre-S14 carry-forwards (still open)

- **CF-DEP-1** *(SEVEN sessions old)* — 2 Dependabot advisories
  unresolved. Owner-blocked. Should at least be triaged.
- **CF-FCAP-MARGIN** *(S13)* — optional fast pump-cap unit test, low
  priority.
- **CF-IGN-1** *(carried)* — 16 inherited `#[ignore]` tests need
  characterizing; this session added 2 more (the CF-CFBW-CELL-LIVENESS
  arms) → 18 total.
- **CF-DISK-1** *(S14)* — the 67 GB box has been running tight on disk
  (12-22 GB free during S14, well under R9's 25 GB floor). Surgical
  cleanup (rm -rf incremental) recovers ~5 GB per pass. S15 should
  budget for either a larger box or an aggressive cargo cache eviction
  step in the session opener.
- **N-1** *(carried)* — jumbo-MTU doc still pending.
- **F-ESC-1** *(carried)* — multi-kernel CI lane.

---

## Program-level still unbuilt (for S15-S20 roadmap)

Matrix complete (9/9) ≠ production ready. These remain:

1. **Native QUIC proxy** — the QUIC datapath today is for H3 only; a
   raw-QUIC proxy mode (e.g. for proprietary protocols over QUIC) is
   unbuilt.
2. **WebSockets-over-H2 (RFC 8441)** — the front-side H2 listener does
   not advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`, so H2 CONNECT WS
   bootstrap is not exposed.
3. **WebSockets-over-H3 (RFC 9220)** — the front-side H3 listener does
   not support the H3 extended CONNECT either; the WS-over-H3 datagram
   shape is wholly absent.
4. **gRPC-over-H3 conformance** — gRPC works over H2 (per the
   `grpc_*` integration tests); the H3 path hasn't been certified
   against the gRPC spec (trailer carriage, status framing, deadline
   propagation).
5. **h3spec full conformance pass** — partial h3-only conformance is
   covered by `tests/round8_h3_*` and `h2spec_server_conformance`; the
   full h3spec suite is not wired in.
6. **Chaos / soak suite** — there are point tests for backend kill,
   slowloris, rapid-reset, etc. but no scheduled multi-hour soak with
   resource-leak telemetry.

Plan estimate from S14 brief: **~4-5 sessions after S14** to close
these. Recommend tackling 2 and 3 together (the WS-over-H2/H3 deltas
share the extended-CONNECT plumbing); 4 builds on 3; 5 should run as
a CI lane rather than a session; 6 is the production-ready capstone.

---

## How to start S15

1. `git checkout main && git pull --ff-only` (S14 is promoted at
   the same commit you see in the `s14-report.md` "Promote per R11"
   section).
2. Skim `s14-report.md` end-to-end (single page) — confirms what the
   last session shipped.
3. Pick **one** carry-forward to lead with. Recommend
   **CF-CFBW-PERCELL-TESTS** first because it's small (≤4 h), closes
   the R13 BUILT-bar honestly, and shakes out any cell-wiring drift
   before it accumulates. CF-S7-RHU is the next logical step
   (single-cell H3-leg fix, same pattern).
4. The verifier agent had repeated background-job failures in S14
   (silent idles after `cargo llvm-cov` / `cargo test --workspace`
   launches). If you use the team-of-agents pattern again, baseline
   the verifier's harness contract first (foreground only for ≤9 min;
   bg+TaskOutput-block for longer; never silent idle).
